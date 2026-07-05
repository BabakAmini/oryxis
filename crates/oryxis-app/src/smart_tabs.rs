//! Smart tabs: command start/end timing and background-tab attention.
//!
//! Built on the same OSC 133 shell-integration marks that drive the
//! command-history capture: `OutputStart` stamps a command's start (and
//! adopts the command line the input capture last saw submitted),
//! `CommandEnd` resolves it with the exit code, and a prompt without a
//! `CommandEnd` still closes it (integrations that only emit A/B/C).
//! Hosts without shell integration get the quiet-period heuristic
//! instead: output arriving after [`QUIET_PERIOD`] of silence on a pane
//! the user is not watching is "activity" (the `tail -f` / long-build
//! resuming case).
//!
//! The dispatcher (`PtyOutput`) owns the policy: a finished command earns
//! a dot + notification only when it ran at least the configured
//! threshold AND its tab was not being watched; activity earns a dot
//! always but a notification only on the rising edge (a dot appearing),
//! so a chatty background pane can't spam. Attention is cleared by
//! viewing the tab.

use std::time::{Duration, Instant};

use oryxis_terminal::{PositionedShellMark, ShellMark};

/// Minimum silence before new output on an unwatched pane counts as
/// activity. Below this, ordinary command output cadence would flag
/// half the background tabs on every batch.
pub(crate) const QUIET_PERIOD: Duration = Duration::from_secs(30);

/// Longest command text carried into a notification body. Anything
/// longer is elided; the notification is a cue, not a transcript.
const MAX_NOTIFY_CMD: usize = 60;

/// A command in flight on a pane. Stamped at `OutputStart`, resolved at
/// `CommandEnd` or the next prompt; only integrated hosts ever have one.
pub(crate) struct CommandRun {
    /// The submitted command line when the capture saw it; `None` when
    /// the echo check failed or the capture didn't run.
    pub text: Option<String>,
    pub started: Instant,
}

/// Why a background tab wants the user's eye. `Ord` is the display
/// priority: a failure outranks a success, which outranks activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TabAttention {
    /// Output resumed after a quiet period.
    Activity,
    /// A long-running command finished successfully (or with no exit
    /// code reported).
    FinishedOk,
    /// A long-running command finished with a nonzero exit code.
    FinishedFail,
}

/// A command that just finished, as reported by [`observe_marks`].
pub(crate) struct FinishedCommand {
    pub text: Option<String>,
    pub elapsed: Duration,
    /// Exit code when the shell reported one (`OSC 133 ; D ; <code>`).
    pub exit: Option<i32>,
}

impl FinishedCommand {
    /// Only an explicit nonzero exit code counts as failure; a prompt
    /// without a `D` mark reports no code and reads as success.
    pub fn failed(&self) -> bool {
        self.exit.is_some_and(|c| c != 0)
    }
}

/// Advance the pane's running-command state with one output batch's OSC
/// 133 marks and return every command that finished inside it. `now` is
/// passed in so tests control the clock.
pub(crate) fn observe_marks(
    running: &mut Option<CommandRun>,
    last_submitted: &mut Option<String>,
    marks: &[PositionedShellMark],
    now: Instant,
) -> Vec<FinishedCommand> {
    let mut finished = Vec::new();
    for m in marks {
        match m.mark {
            ShellMark::OutputStart => {
                // A C over an unresolved run (missed D, e.g. a kill -9'd
                // shell integration) drops the old one silently: there is
                // no honest duration to report for it.
                *running = Some(CommandRun { text: last_submitted.take(), started: now });
            }
            ShellMark::CommandEnd(code) => {
                if let Some(run) = running.take() {
                    finished.push(FinishedCommand {
                        text: run.text,
                        elapsed: now.duration_since(run.started),
                        exit: code,
                    });
                }
            }
            // A prompt without a D still ends the command (integrations
            // that only emit A/B/C); no exit code is available. A fresh
            // prompt also voids any submitted-line stash: nothing typed
            // at it has run yet.
            ShellMark::PromptStart | ShellMark::PromptEnd => {
                if let Some(run) = running.take() {
                    finished.push(FinishedCommand {
                        text: run.text,
                        elapsed: now.duration_since(run.started),
                        exit: None,
                    });
                }
                *last_submitted = None;
            }
        }
    }
    finished
}

/// Record one output batch's arrival time and report whether it broke a
/// quiet period. Must run on every batch (watched or not) so the clock
/// stays honest; the caller decides whether a broken silence matters.
/// The first output ever is never activity (that's the connect banner).
pub(crate) fn quiet_activity(last_output: &mut Option<Instant>, now: Instant) -> bool {
    let was_quiet =
        last_output.is_some_and(|t| now.duration_since(t) >= QUIET_PERIOD);
    *last_output = Some(now);
    was_quiet
}

/// Merge a new attention cause into a pane's slot, keeping the higher
/// priority one. Returns whether the slot was empty before (the rising
/// edge, which is what gates an activity notification).
pub(crate) fn raise_attention(
    slot: &mut Option<TabAttention>,
    new: TabAttention,
) -> bool {
    let was_empty = slot.is_none();
    if slot.is_none_or(|cur| new > cur) {
        *slot = Some(new);
    }
    was_empty
}

/// Notification body for a finished command, e.g.
/// `"cargo build" finished (2m 10s)`. The pane label travels separately
/// (OS-notification title / toast prefix), not in the template.
/// `with_cmd = false` forces the generic template: Privacy Mode strips
/// the command line from surfaced text, since command args can carry
/// secrets and the OS notification center persists plaintext.
pub(crate) fn finished_body(f: &FinishedCommand, with_cmd: bool) -> String {
    let duration = format_duration(f.elapsed);
    let cmd = f.text.as_deref().filter(|_| with_cmd).map(elide_command);
    let key = match (&cmd, f.failed()) {
        (Some(_), false) => "smart_cmd_finished",
        (None, false) => "smart_cmd_finished_generic",
        (Some(_), true) => "smart_cmd_failed",
        (None, true) => "smart_cmd_failed_generic",
    };
    let mut body = crate::i18n::t(key)
        .replace("{duration}", &duration)
        .replace("{code}", &f.exit.unwrap_or_default().to_string());
    if let Some(cmd) = cmd {
        body = body.replace("{cmd}", &cmd);
    }
    body
}

/// `92s` -> `1m 32s`, `3700s` -> `1h 1m`; sub-second runs read as `1s`
/// (they can only get here through a pathological threshold of 0).
pub(crate) fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}s", s.max(1))
    }
}

/// Cap a command line for a notification body, on a char boundary.
fn elide_command(cmd: &str) -> String {
    if cmd.chars().count() <= MAX_NOTIFY_CMD {
        cmd.to_string()
    } else {
        let cut: String = cmd.chars().take(MAX_NOTIFY_CMD).collect();
        format!("{cut}\u{2026}")
    }
}

/// The long-command threshold choices offered in Settings > Terminal:
/// `(seconds, display label)`. `0` = the finished half is off (activity
/// detection stays). Duration labels are locale-neutral on purpose.
pub(crate) fn threshold_options() -> Vec<(u32, String)> {
    vec![
        (0, crate::i18n::t("smart_threshold_off").to_string()),
        (5, "5 s".to_string()),
        (10, "10 s".to_string()),
        (30, "30 s".to_string()),
        (60, "1 min".to_string()),
        (300, "5 min".to_string()),
    ]
}

/// Display label for the currently configured threshold. An off-list
/// value (hand-edited vault) renders as raw seconds rather than lying
/// with the nearest choice.
pub(crate) fn threshold_label(secs: u32) -> String {
    threshold_options()
        .into_iter()
        .find(|(s, _)| *s == secs)
        .map(|(_, l)| l)
        .unwrap_or_else(|| format!("{secs} s"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(mark: ShellMark) -> PositionedShellMark {
        PositionedShellMark { mark, abs_line: 0, col: 0 }
    }

    #[test]
    fn command_lifecycle_c_then_d_reports_exit_and_duration() {
        let t0 = Instant::now();
        let mut running = None;
        let mut submitted = Some("cargo build".to_string());
        let f = observe_marks(
            &mut running,
            &mut submitted,
            &[mark(ShellMark::OutputStart)],
            t0,
        );
        assert!(f.is_empty());
        assert!(running.is_some());
        assert!(submitted.is_none(), "OutputStart consumes the stash");

        let f = observe_marks(
            &mut running,
            &mut submitted,
            &[mark(ShellMark::CommandEnd(Some(1)))],
            t0 + Duration::from_secs(95),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].text.as_deref(), Some("cargo build"));
        assert_eq!(f[0].exit, Some(1));
        assert!(f[0].failed());
        assert_eq!(f[0].elapsed, Duration::from_secs(95));
        assert!(running.is_none());
    }

    #[test]
    fn fast_command_in_one_batch_reads_as_zero_elapsed() {
        // `ls`: C and D arrive in the same output batch, same `now`.
        let t0 = Instant::now();
        let mut running = None;
        let mut submitted = Some("ls".to_string());
        let f = observe_marks(
            &mut running,
            &mut submitted,
            &[
                mark(ShellMark::OutputStart),
                mark(ShellMark::CommandEnd(Some(0))),
                mark(ShellMark::PromptStart),
                mark(ShellMark::PromptEnd),
            ],
            t0,
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].elapsed, Duration::ZERO);
        assert!(!f[0].failed());
    }

    #[test]
    fn prompt_without_d_still_finishes_without_exit_code() {
        let t0 = Instant::now();
        let mut running = None;
        let mut submitted = None;
        observe_marks(&mut running, &mut submitted, &[mark(ShellMark::OutputStart)], t0);
        let f = observe_marks(
            &mut running,
            &mut submitted,
            &[mark(ShellMark::PromptEnd)],
            t0 + Duration::from_secs(12),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].exit, None);
        assert!(!f[0].failed(), "no exit code reads as success");
    }

    #[test]
    fn prompt_voids_a_stale_submitted_stash() {
        let t0 = Instant::now();
        let mut running = None;
        let mut submitted = Some("stale".to_string());
        observe_marks(&mut running, &mut submitted, &[mark(ShellMark::PromptEnd)], t0);
        assert!(submitted.is_none());
    }

    #[test]
    fn quiet_activity_needs_a_prior_gap() {
        let t0 = Instant::now();
        let mut last = None;
        assert!(!quiet_activity(&mut last, t0), "first output is the banner, not activity");
        assert!(!quiet_activity(&mut last, t0 + Duration::from_secs(5)));
        assert!(quiet_activity(
            &mut last,
            t0 + Duration::from_secs(5) + QUIET_PERIOD
        ));
        // The clock was updated by the hit; an immediate follow-up batch
        // is not a fresh quiet break.
        assert!(!quiet_activity(
            &mut last,
            t0 + Duration::from_secs(6) + QUIET_PERIOD
        ));
    }

    #[test]
    fn attention_priority_and_rising_edge() {
        let mut slot = None;
        assert!(raise_attention(&mut slot, TabAttention::Activity), "rising edge");
        assert_eq!(slot, Some(TabAttention::Activity));
        assert!(!raise_attention(&mut slot, TabAttention::FinishedOk));
        assert_eq!(slot, Some(TabAttention::FinishedOk), "higher priority replaces");
        assert!(!raise_attention(&mut slot, TabAttention::Activity));
        assert_eq!(slot, Some(TabAttention::FinishedOk), "lower priority doesn't downgrade");
        assert!(!raise_attention(&mut slot, TabAttention::FinishedFail));
        assert_eq!(slot, Some(TabAttention::FinishedFail));
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(Duration::from_secs(0)), "1s");
        assert_eq!(format_duration(Duration::from_secs(42)), "42s");
        assert_eq!(format_duration(Duration::from_secs(92)), "1m 32s");
        assert_eq!(format_duration(Duration::from_secs(3700)), "1h 1m");
    }

    #[test]
    fn notification_body_elides_a_paste_blob_command() {
        let f = FinishedCommand {
            text: Some("x".repeat(200)),
            elapsed: Duration::from_secs(70),
            exit: Some(0),
        };
        let body = finished_body(&f, true);
        assert!(body.contains('\u{2026}'));
        assert!(body.contains("1m 10s"));
        assert!(!body.contains(&"x".repeat(80)));
    }

    #[test]
    fn privacy_body_never_carries_the_command_line() {
        let f = FinishedCommand {
            text: Some("mysql -psecret".to_string()),
            elapsed: Duration::from_secs(70),
            exit: Some(1),
        };
        let body = finished_body(&f, false);
        assert!(!body.contains("mysql"), "redacted body must drop the command");
        assert!(body.contains("1m 10s"), "duration is not sensitive");
    }
}
