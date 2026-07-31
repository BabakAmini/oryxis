//! Kill the process behind a listening port (issue #96).
//!
//! The Monitor tab surfaces the host's listening sockets; this module is
//! what turns "port 8080 is nginx" into a signal actually delivered on
//! the host. Two halves:
//!
//! - **Command synthesis** (pure, unit-tested): every string that
//!   reaches the host is built here, and the ONLY value ever
//!   interpolated is a `u32` PID. A process name is display data and
//!   never enters a command line; a sudo password never does either (it
//!   travels on stdin, because a command line is visible to every user
//!   on the host through `ps`).
//! - **The runner**: a re-resolve → escalate → signal pipeline on an
//!   exec channel multiplexed on the pane's live session, the same
//!   pattern the probe uses. Nothing is ever typed into the user's PTY.
//!
//! The re-resolve is the load-bearing part. The PID shown in the confirm
//! dialog comes from a sample that may be seconds old, and PIDs are
//! recycled: signalling a remembered number could hit a process that
//! took over the slot after a restart. So the runner asks the host again
//! who owns the port RIGHT NOW, and refuses when the answer changed
//! under the user instead of killing the wrong thing.

use std::sync::Arc;

use oryxis_ssh::SshSession;

/// Cap on one remote step (resolve, sudo check, signal). Generous
/// enough for a loaded host's socket table, short enough that a wedged
/// one reports back before the user gives up on the dialog.
pub(crate) const KILL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Which signal the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KillSignal {
    /// `SIGTERM`: the polite default, lets the service shut down.
    Term,
    /// `SIGKILL`: uninterruptible, no cleanup. Explicit user choice.
    Force,
}

impl KillSignal {
    /// POSIX signal NAME (not number): `kill -s TERM` is portable
    /// across busybox / dash / bash, while numbers differ per platform.
    fn name(self) -> &'static str {
        match self {
            KillSignal::Term => "TERM",
            KillSignal::Force => "KILL",
        }
    }

    /// i18n key describing the signal in the confirm dialog.
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            KillSignal::Term => "monitor_kill_signal_term",
            KillSignal::Force => "monitor_kill_signal_kill",
        }
    }
}

/// How a remote command is escalated.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum SudoMode {
    /// Run as the login user.
    Direct,
    /// `sudo -n`: NOPASSWD, or a credential the host already cached.
    NonInteractive,
    /// `sudo -S`, password fed on stdin. Only reached after ONE
    /// successful validation (see [`resolve_sudo_mode`]).
    Password(String),
}

// Hand-written so a `dbg!` / tracing line on the pipeline can never
// print the host password the `Password` arm carries.
impl std::fmt::Debug for SudoMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SudoMode::Direct => f.write_str("Direct"),
            SudoMode::NonInteractive => f.write_str("NonInteractive"),
            SudoMode::Password(_) => f.write_str("Password(<redacted>)"),
        }
    }
}

impl SudoMode {
    /// Prefix `command` with the escalation this mode calls for.
    ///
    /// `-p ''` empties sudo's prompt so the password request doesn't
    /// land in the captured stderr and get shown back to the user as an
    /// error line.
    pub(crate) fn wrap(&self, command: &str) -> String {
        match self {
            SudoMode::Direct => command.to_string(),
            SudoMode::NonInteractive => format!("sudo -n {command}"),
            SudoMode::Password(_) => format!("sudo -S -p '' {command}"),
        }
    }

    /// Bytes to write on the command's stdin before reading it back.
    fn stdin(&self) -> Option<Vec<u8>> {
        match self {
            SudoMode::Password(pw) => Some(format!("{pw}\n").into_bytes()),
            _ => None,
        }
    }

    /// Whether this run is escalated at all, which decides whether a
    /// failure can still be retried with sudo.
    pub(crate) fn escalated(&self) -> bool {
        !matches!(self, SudoMode::Direct)
    }
}

/// The `kill` invocation for one PID.
///
/// Wrapped in `sh -c` for the same reason the probe batch is: the exec
/// channel hands the string to the user's LOGIN shell, and `kill -s` is
/// not spelled the same way in csh/tcsh. The wrapper also means the
/// sudo forms run the SHELL BUILTIN, so a minimal host without a
/// `/bin/kill` binary still works.
pub(crate) fn kill_command(pid: u32, signal: KillSignal) -> String {
    // The only interpolated value is a u32 and a fixed signal literal,
    // so the single-quoted wrapper can never be broken out of.
    let cmd = format!("sh -c 'kill -s {} {pid}'", signal.name());
    debug_assert_eq!(cmd.matches('\'').count(), 2);
    cmd
}

/// Re-read the host's listening sockets, using the very command the
/// monitor probe uses so the shared parser sees the shapes it was
/// written for.
pub(crate) fn resolve_ports_command() -> String {
    let cmd = format!("sh -c '{}'", super::probe::LISTENING_SOCKETS_CMD);
    debug_assert_eq!(cmd.matches('\'').count(), 2);
    cmd
}

/// What the pipeline did, or why it didn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KillOutcome {
    /// The signal was delivered.
    Killed { pid: u32 },
    /// Nothing listens on that port any more, so there is nothing left
    /// to signal. Not a failure: the user's goal is already met.
    AlreadyGone,
    /// Some OTHER process owns the port now. Refused rather than
    /// signalled: the user confirmed a different target.
    Changed { pid: u32 },
    /// No PID could be resolved. `sudo` records whether escalation was
    /// already tried, so the dialog knows if a retry is worth offering.
    NoPid { sudo: bool },
    /// sudo wants a password and the host has none stored.
    SudoNeedsPassword,
    /// sudo refused: wrong stored password, or the user isn't a sudoer.
    SudoDenied,
    /// The command ran and failed. `sudo` gates the retry offer.
    Failed { message: String, sudo: bool },
    /// The channel died or the step outlived [`KILL_TIMEOUT`].
    Unreachable,
}

impl KillOutcome {
    /// Whether the dialog should close and toast instead of parking on
    /// an error the user has to read.
    pub(crate) fn is_settled(&self) -> bool {
        matches!(self, KillOutcome::Killed { .. } | KillOutcome::AlreadyGone)
    }

    /// Whether "Retry with sudo" applies: only for the failures
    /// escalation can actually fix, and only when it wasn't tried yet.
    pub(crate) fn can_retry_with_sudo(&self) -> bool {
        match self {
            KillOutcome::NoPid { sudo } | KillOutcome::Failed { sudo, .. } => !sudo,
            _ => false,
        }
    }

    /// The user-facing line, resolved against the active language.
    pub(crate) fn message(&self) -> String {
        use crate::i18n::t;
        match self {
            KillOutcome::Killed { pid } => {
                t("monitor_kill_ok").replacen("{pid}", &pid.to_string(), 1)
            }
            KillOutcome::AlreadyGone => t("monitor_kill_gone").to_string(),
            KillOutcome::Changed { pid } => {
                t("monitor_kill_changed").replacen("{pid}", &pid.to_string(), 1)
            }
            KillOutcome::NoPid { .. } => t("monitor_kill_no_pid").to_string(),
            KillOutcome::SudoNeedsPassword => t("monitor_kill_sudo_password").to_string(),
            KillOutcome::SudoDenied => t("monitor_kill_sudo_denied").to_string(),
            KillOutcome::Failed { message, .. } => {
                t("monitor_kill_failed").replacen("{error}", message, 1)
            }
            KillOutcome::Unreachable => t("monitor_kill_unreachable").to_string(),
        }
    }
}

/// Where the confirm dialog is in the flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KillPhase {
    /// Waiting for the user to confirm (or cancel).
    Confirm,
    /// A run is in flight on the session.
    Running,
    /// The run came back with something the user has to read. Success
    /// closes the dialog instead of landing here.
    Failed(KillOutcome),
}

/// The kill confirmation's live state.
///
/// Parked on [`super::MonitorState`] rather than on `Oryxis` so the
/// existing monitor sweeps (`forget` on disconnect, `monitor_reset_all`
/// on vault lock and on the feature toggle) drop a half-finished kill
/// dialog for free, the same way they drop the sample windows.
#[derive(Debug, Clone)]
pub(crate) struct PendingKill {
    /// Host the port belongs to; also what the sweeps match on.
    pub conn_id: uuid::Uuid,
    /// Host label, for the "this stops a live service on X" warning.
    pub host: String,
    pub port: u16,
    pub proto: &'static str,
    /// Best-effort name from the probe; display only, never a command.
    pub process: Option<String>,
    /// What the user is confirming. `None` = the host wouldn't say, so
    /// the run has to resolve it under sudo first.
    pub pid: Option<u32>,
    pub signal: KillSignal,
    /// Whether the next run escalates. Starts true exactly when there
    /// is no PID (nothing else can work), and latches on after a
    /// "Retry with sudo".
    pub sudo: bool,
    pub phase: KillPhase,
}

impl PendingKill {
    /// A fresh confirmation for one port row.
    pub(crate) fn new(
        conn_id: uuid::Uuid,
        host: String,
        port: &super::model::PortStat,
        signal: KillSignal,
    ) -> Self {
        Self {
            conn_id,
            host,
            port: port.port,
            proto: port.proto,
            process: port.process.clone(),
            pid: port.pid,
            // Without a PID the login user demonstrably can't see the
            // socket's owner, so an unescalated run has nothing to
            // target; start escalated instead of failing on purpose.
            sudo: port.pid.is_none(),
            signal,
            phase: KillPhase::Confirm,
        }
    }
}

/// First non-empty line of a command's diagnostics, clipped so a host
/// that answers with a wall of text can't blow the dialog open.
fn first_line(stderr: &str, stdout: &str) -> String {
    let pick = |s: &str| {
        s.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
    };
    let mut line = pick(stderr).or_else(|| pick(stdout)).unwrap_or_default();
    const MAX: usize = 160;
    if line.chars().count() > MAX {
        line = line.chars().take(MAX).collect::<String>() + "…";
    }
    line
}

/// Decide how to escalate, spending AT MOST ONE failing password
/// attempt.
///
/// `sudo -n true` answers structurally (exit 0 = escalation is free),
/// so the common NOPASSWD / cached-credential path never touches the
/// stored password and never depends on parsing sudo's localized
/// messages. Only when that fails is the stored password tried, exactly
/// once: every rejected attempt counts toward sudo's failure limit,
/// which can mail root and lock the account, so a wrong password must
/// not be replayed by the resolve step and then again by the signal.
async fn resolve_sudo_mode(
    session: &Arc<SshSession>,
    password: Option<String>,
) -> Result<SudoMode, KillOutcome> {
    let probe = SudoMode::NonInteractive;
    let Some(res) = session
        .exec_capture(&probe.wrap("true"), None, KILL_TIMEOUT)
        .await
    else {
        return Err(KillOutcome::Unreachable);
    };
    if res.exit_code == 0 {
        return Ok(probe);
    }
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return Err(KillOutcome::SudoNeedsPassword);
    };
    let with_pw = SudoMode::Password(password);
    let Some(res) = session
        .exec_capture(&with_pw.wrap("true"), with_pw.stdin(), KILL_TIMEOUT)
        .await
    else {
        return Err(KillOutcome::Unreachable);
    };
    if res.exit_code == 0 {
        // Validated, so reusing it for the resolve and the signal can
        // no longer produce a FAILED attempt.
        Ok(with_pw)
    } else {
        Err(KillOutcome::SudoDenied)
    }
}

/// Ask the host who owns `(port, proto)` right now.
async fn resolve_owner(
    session: &Arc<SshSession>,
    mode: &SudoMode,
    port: u16,
    proto: &str,
) -> Result<Option<u32>, KillOutcome> {
    let cmd = mode.wrap(&resolve_ports_command());
    let Some(res) = session.exec_capture(&cmd, mode.stdin(), KILL_TIMEOUT).await else {
        return Err(KillOutcome::Unreachable);
    };
    let ports = super::probe::parse_listening_ports_any(&res.stdout);
    match ports.iter().find(|p| p.port == port && p.proto == proto) {
        // The port stopped listening between the sample and now.
        None => Err(KillOutcome::AlreadyGone),
        Some(row) => Ok(row.pid),
    }
}

/// Resolve → escalate → signal, on exec channels multiplexed over the
/// pane's live session.
///
/// `expected_pid` is what the user confirmed. When the host now reports
/// a different owner the run REFUSES: the confirmation was for that
/// process, and a service that restarted in the meantime is not the
/// thing the user agreed to kill. `None` (the host never revealed a
/// PID) means the user confirmed "whatever owns this port", so whatever
/// the escalated resolve finds is a legitimate target.
pub(crate) async fn run_kill(
    session: Arc<SshSession>,
    port: u16,
    proto: &'static str,
    expected_pid: Option<u32>,
    signal: KillSignal,
    sudo: bool,
    password: Option<String>,
) -> KillOutcome {
    let mode = if sudo {
        match resolve_sudo_mode(&session, password).await {
            Ok(mode) => mode,
            Err(outcome) => return outcome,
        }
    } else {
        SudoMode::Direct
    };

    let pid = match resolve_owner(&session, &mode, port, proto).await {
        Ok(Some(pid)) => pid,
        Ok(None) => return KillOutcome::NoPid { sudo: mode.escalated() },
        Err(outcome) => return outcome,
    };
    if let Some(expected) = expected_pid
        && expected != pid
    {
        return KillOutcome::Changed { pid };
    }

    let cmd = mode.wrap(&kill_command(pid, signal));
    let Some(res) = session.exec_capture(&cmd, mode.stdin(), KILL_TIMEOUT).await else {
        return KillOutcome::Unreachable;
    };
    if res.exit_code == 0 {
        return KillOutcome::Killed { pid };
    }
    // No message matching: sudo and coreutils translate their errors, so
    // a locale-based classifier would silently mis-route on a non-English
    // host. The host's own words are shown verbatim, and the retry offer
    // keys off whether escalation was already used, which is a fact we
    // own rather than a string we guessed at.
    KillOutcome::Failed {
        message: first_line(&res.stderr, &res.stdout),
        sudo: mode.escalated(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_command_interpolates_only_the_pid() {
        assert_eq!(kill_command(1234, KillSignal::Term), "sh -c 'kill -s TERM 1234'");
        assert_eq!(kill_command(7, KillSignal::Force), "sh -c 'kill -s KILL 7'");
        // The login shell may be csh/fish, so the wrapper is mandatory,
        // and it only works while the payload has no single quote.
        for signal in [KillSignal::Term, KillSignal::Force] {
            let cmd = kill_command(u32::MAX, signal);
            assert!(cmd.starts_with("sh -c '") && cmd.ends_with('\''));
            assert_eq!(cmd.matches('\'').count(), 2);
        }
    }

    #[test]
    fn sudo_modes_wrap_and_feed_the_password_off_the_command_line() {
        let cmd = kill_command(42, KillSignal::Term);
        assert_eq!(SudoMode::Direct.wrap(&cmd), cmd);
        assert_eq!(
            SudoMode::NonInteractive.wrap(&cmd),
            "sudo -n sh -c 'kill -s TERM 42'"
        );
        let pw = SudoMode::Password("hunter2".into());
        let escalated = pw.wrap(&cmd);
        assert_eq!(escalated, "sudo -S -p '' sh -c 'kill -s TERM 42'");
        // `ps` on the host must never be able to show the secret.
        assert!(!escalated.contains("hunter2"));
        assert_eq!(pw.stdin().unwrap(), b"hunter2\n".to_vec());
        assert!(SudoMode::NonInteractive.stdin().is_none());
        assert!(SudoMode::Direct.stdin().is_none());
        // Debug must not leak it either.
        assert_eq!(format!("{pw:?}"), "Password(<redacted>)");
    }

    #[test]
    fn only_direct_runs_can_still_be_escalated() {
        assert!(!SudoMode::Direct.escalated());
        assert!(SudoMode::NonInteractive.escalated());
        assert!(SudoMode::Password(String::new()).escalated());
    }

    #[test]
    fn the_resolve_command_reuses_the_probe_socket_line() {
        let cmd = resolve_ports_command();
        assert!(cmd.starts_with("sh -c '") && cmd.ends_with('\''));
        assert_eq!(cmd.matches('\'').count(), 2);
        assert!(cmd.contains("ss -tulnp"));
        assert!(cmd.contains("netstat -tulnp"));
    }

    #[test]
    fn retry_with_sudo_is_offered_only_where_it_can_help() {
        assert!(KillOutcome::NoPid { sudo: false }.can_retry_with_sudo());
        assert!(!KillOutcome::NoPid { sudo: true }.can_retry_with_sudo());
        assert!(
            KillOutcome::Failed { message: "denied".into(), sudo: false }
                .can_retry_with_sudo()
        );
        assert!(
            !KillOutcome::Failed { message: "denied".into(), sudo: true }
                .can_retry_with_sudo()
        );
        // Escalation cannot undo these, so no retry is dangled.
        for outcome in [
            KillOutcome::Killed { pid: 1 },
            KillOutcome::AlreadyGone,
            KillOutcome::Changed { pid: 2 },
            KillOutcome::SudoNeedsPassword,
            KillOutcome::SudoDenied,
            KillOutcome::Unreachable,
        ] {
            assert!(!outcome.can_retry_with_sudo(), "{outcome:?}");
        }
    }

    #[test]
    fn settled_outcomes_close_the_dialog() {
        assert!(KillOutcome::Killed { pid: 1 }.is_settled());
        // Nothing left to signal is the user's goal, not an error.
        assert!(KillOutcome::AlreadyGone.is_settled());
        assert!(!KillOutcome::Changed { pid: 1 }.is_settled());
        assert!(!KillOutcome::Unreachable.is_settled());
    }

    #[test]
    fn first_line_prefers_stderr_and_clips_a_flood() {
        assert_eq!(first_line("\n  boom  \nmore", "out"), "boom");
        // Empty diagnostics fall through to stdout.
        assert_eq!(first_line("   \n", "fallback"), "fallback");
        assert_eq!(first_line("", ""), "");
        let flood = "x".repeat(500);
        let clipped = first_line(&flood, "");
        assert_eq!(clipped.chars().count(), 161);
        assert!(clipped.ends_with('…'));
    }
}
