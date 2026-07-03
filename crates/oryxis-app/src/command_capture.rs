//! Command-history capture: decides, per submitted line of input, whether it
//! was a shell command and what its text was.
//!
//! Two paths, chosen by whether the host's shell emits OSC 133 marks:
//!
//! - **Integrated** (`PromptState::AtPrompt`): input submitted at the prompt
//!   is a command by definition; its text is read back from the grid at the
//!   `PromptEnd` position (so history recall and tab completion come out
//!   right), immediately when the echo is already on screen, or at
//!   `OutputStart` when it isn't yet (paste with a trailing newline). Input
//!   submitted while `Busy` is a running program's stdin, sudo passwords,
//!   `read` answers, and is never recorded. This gate is what makes the
//!   capture safe on integrated hosts: secrets typed mid-command can't
//!   reach the vault by construction.
//!
//! - **Heuristic** (`PromptState::NoIntegration`): the line the cursor sits
//!   on must contain a classic prompt marker (`$ `, `# `, ...), and the text
//!   after the marker must match the input-tracker mirror (proof the input
//!   was echoed, so not a password). A tainted mirror (history recall, tab
//!   completion) trusts the echoed grid text instead. No marker, or no echo,
//!   means no record: this path prefers missing a command over recording a
//!   secret or garbage.
//!
//! Both paths honor the `HISTCONTROL=ignorespace` convention: a command
//! starting with a space is deliberately not recorded.

use crate::state::{Pane, PendingCapture, PromptState};
use oryxis_terminal::{PositionedShellMark, ShellMark};

/// Upper bound on a recorded command. Anything longer is a paste blob, not a
/// command worth resurfacing in the History tab.
const MAX_COMMAND: usize = 4096;

/// Feed user input `bytes` through the pane's line-editor mirror and return
/// the commands captured by Enter presses inside this write. Call on every
/// user-originated write to the pane's PTY/SSH channel (and only those:
/// programmatic secrets like the sudo-password autofill must bypass this).
pub(crate) fn observe_input(pane: &mut Pane, bytes: &[u8]) -> Vec<String> {
    let lines = pane.input_tracker.feed(bytes);
    if lines.is_empty() {
        return Vec::new();
    }
    let Ok(term) = pane.terminal.lock() else {
        return Vec::new();
    };
    let mut captured = Vec::new();
    // The grid only reflects state up to the last output batch, so within
    // one write it can vouch for at most one submission; later lines of a
    // multi-line unbracketed paste land after prompts we haven't seen yet
    // (one of them may be a password prompt) and are skipped outright on
    // the heuristic path, or deferred to the mark cycle on the integrated
    // path (where `AtPrompt` consumes itself after the first submit).
    let mut heuristic_done = false;
    for line in lines {
        if term.is_alt_screen() {
            continue;
        }
        match pane.prompt {
            PromptState::AtPrompt { abs_line, col } => {
                pane.prompt = PromptState::Busy;
                match term.logical_line_from_abs(abs_line, col) {
                    Some(text) if !text.trim().is_empty() => {
                        if let Some(cmd) = sanitize_command(&text) {
                            captured.push(cmd);
                        }
                    }
                    // Echo still in flight (paste + newline in one write):
                    // resolve at OutputStart, but only when something was
                    // actually submitted.
                    Some(_) if line.tainted || !line.text.trim().is_empty() => {
                        pane.pending_capture =
                            Some(PendingCapture { b_abs: abs_line, b_col: col });
                    }
                    _ => {}
                }
            }
            PromptState::Busy => {}
            PromptState::NoIntegration => {
                if heuristic_done {
                    continue;
                }
                heuristic_done = true;
                let Some(grid_line) = term.cursor_logical_line() else {
                    continue;
                };
                let Some(stripped) = strip_prompt(&grid_line) else {
                    continue;
                };
                let cmd = if line.tainted {
                    // The mirror is unreliable; the echoed grid text is the
                    // best truth available (it shows the recalled/completed
                    // command).
                    stripped.to_string()
                } else if stripped.trim() == line.text.trim() {
                    // Echo check passed: the typed text is on screen, so it
                    // was not an unechoed password.
                    line.text.clone()
                } else {
                    continue;
                };
                if let Some(cmd) = sanitize_command(&cmd) {
                    captured.push(cmd);
                }
            }
        }
    }
    captured
}

/// Apply an output batch's OSC 133 marks to the pane's prompt state and
/// resolve any deferred capture. `term` must be the pane's locked terminal,
/// with the batch already processed, so the grid rows the marks point at are
/// exactly the ones the shell just drew.
pub(crate) fn observe_output_marks(
    pane_prompt: &mut PromptState,
    pending: &mut Option<PendingCapture>,
    term: &oryxis_terminal::TerminalState,
    marks: &[PositionedShellMark],
) -> Vec<String> {
    let mut captured = Vec::new();
    for m in marks {
        match m.mark {
            ShellMark::PromptEnd => {
                // A fresh prompt invalidates any unresolved capture: the
                // shell never confirmed a command ran (empty Enter, Ctrl+C).
                *pending = None;
                *pane_prompt = PromptState::AtPrompt { abs_line: m.abs_line, col: m.col };
            }
            ShellMark::PromptStart => {
                *pending = None;
                *pane_prompt = PromptState::Busy;
            }
            ShellMark::OutputStart => {
                if let Some(p) = pending.take()
                    && let Some(text) = term.logical_line_from_abs(p.b_abs, p.b_col)
                    && let Some(cmd) = sanitize_command(&text)
                {
                    captured.push(cmd);
                }
                *pane_prompt = PromptState::Busy;
            }
            ShellMark::CommandEnd(_) => *pane_prompt = PromptState::Busy,
        }
    }
    captured
}

/// Final gate before a command reaches the vault. Refuses the
/// `HISTCONTROL=ignorespace` convention (leading space = keep out of
/// history), blank lines, paste blobs past [`MAX_COMMAND`] and anything
/// carrying control characters (a healthy grid read never has them).
pub(crate) fn sanitize_command(text: &str) -> Option<String> {
    if text.starts_with(' ') {
        return None;
    }
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_COMMAND {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control() && c != '\t' && c != '\n') {
        return None;
    }
    Some(trimmed.to_string())
}

/// Find the earliest classic prompt terminator in `line` and return what
/// follows it. `None` when the line carries no recognizable prompt, which on
/// the heuristic path means "don't record" (REPLs, raw program prompts).
pub(crate) fn strip_prompt(line: &str) -> Option<&str> {
    const MARKERS: [&str; 6] = ["$ ", "# ", "% ", "\u{276f} ", "\u{279c} ", "> "];
    let (pos, len) = MARKERS
        .iter()
        .filter_map(|m| line.find(m).map(|i| (i, m.len())))
        .min()?;
    Some(&line[pos + len..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_prompt_takes_earliest_marker() {
        // The prompt's `$ ` wins over the one inside the command.
        assert_eq!(
            strip_prompt("wilson@web:~$ echo $ ok"),
            Some("echo $ ok")
        );
        assert_eq!(strip_prompt("root@db:/etc# systemctl restart nginx"), Some("systemctl restart nginx"));
        assert_eq!(strip_prompt("\u{276f} git status"), Some("git status"));
        assert_eq!(strip_prompt("Password:"), None);
        assert_eq!(strip_prompt("Continue? [y/N] y"), None);
    }

    #[test]
    fn sanitize_respects_ignorespace_and_bounds() {
        assert_eq!(sanitize_command("ls -la").as_deref(), Some("ls -la"));
        assert_eq!(sanitize_command("ls -la  ").as_deref(), Some("ls -la"));
        assert!(sanitize_command(" secret --token=x").is_none(), "leading space = ignorespace");
        assert!(sanitize_command("   ").is_none());
        assert!(sanitize_command("").is_none());
        assert!(sanitize_command(&"x".repeat(5000)).is_none());
        assert!(sanitize_command("a\u{7}b").is_none(), "control chars never come from a healthy grid read");
    }
}
