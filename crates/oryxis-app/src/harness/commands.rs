//! Shared command dispatch for the line-protocol front-ends (REPL on
//! stdin/stdout, TCP server behind `--harness-ctl`).
//!
//! One command in, zero or more response lines out through the `out`
//! sink (the caller adds the `== ` framing), plus a [`Control`] verdict
//! so the front-end knows when the driver asked to shut down or the
//! emulator died. Keeping this in one place means the REPL and the
//! daemon can never drift apart in grammar.

use std::path::PathBuf;
use std::time::Duration;

use iced::Program;
use iced_test::core::clipboard::Content;

use super::{Pump, RunOutcome, Session, format_text_entry, parse_quoted};

pub(super) const HELP: &str = "\
instructions: click [right] \"Text\"|#id|(x, y) / press / release / move <target>
              scroll [pixels] (dx, dy) [<target>] / type \"text\"
              type enter|escape|tab|backspace / type ctrl+k / type ctrl+shift+f
              press enter / release tab / expect \"Text\"
harness:      screenshot [name] / texts / find \"Text\" / clipboard [\"text\"]
              wait <ms> / settle [idle_ms] / timeout <ms> / save <path.ice>
              reset [wipe] / status / help / quit
responses:    == ok | == fail <instr> | == timeout | == shot <path> | == error <..>";

/// What the front-end should do after a command was dispatched.
pub(super) enum Control {
    Continue,
    /// The driver sent `quit` / `exit`: shut the front-end down.
    Quit,
    /// The emulator channel closed: nothing further can run.
    Dead,
}

/// Executes one already-trimmed, non-empty, non-comment command line
/// against the session, emitting response lines through `out`.
pub(super) fn dispatch<P>(
    session: &mut Session<P>,
    program: &P,
    command: &str,
    out: &mut dyn FnMut(String),
) -> Control
where
    P: Program + 'static,
{
    let (head, rest) = match command.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (command, ""),
    };

    match head {
        "quit" | "exit" => return Control::Quit,
        "help" => {
            for line in HELP.lines() {
                out(line.to_owned());
            }
        }
        "status" => {
            out(format!(
                "status home={} shots={} viewport={}x{} timeout_ms={} history={}",
                session.home.display(),
                session.shots.display(),
                session.viewport.width,
                session.viewport.height,
                session.timeout.as_millis(),
                session.history.len(),
            ));
        }
        "screenshot" => match session.screenshot(program, rest) {
            Ok((path, _png)) => {
                session.record(command);
                out(format!("shot {}", path.display()));
            }
            Err(reason) => out(format!("error {reason}")),
        },
        "texts" => match session.texts(program) {
            Ok(entries) => {
                let count = entries.len();
                for (text, bounds) in entries {
                    out(format_text_entry(&text, bounds));
                }
                out(format!("ok {count} texts"));
            }
            Err(reason) => out(format!("error {reason}")),
        },
        "find" => match parse_quoted(rest) {
            Some(needle) => match session.texts(program) {
                Ok(entries) => {
                    let matches: Vec<_> = entries
                        .into_iter()
                        .filter(|(text, _)| text.contains(&needle))
                        .collect();
                    let count = matches.len();
                    for (text, bounds) in matches {
                        out(format_text_entry(&text, bounds));
                    }
                    out(format!("ok {count} matches"));
                }
                Err(reason) => out(format!("error {reason}")),
            },
            None => out("error find wants a quoted string: find \"Hosts\"".into()),
        },
        "wait" => match rest.parse::<u64>() {
            Ok(ms) => {
                session.wait(program, Duration::from_millis(ms.min(600_000)));
                session.record(command);
                out("ok".into());
            }
            Err(_) => out("error wait wants milliseconds: wait 500".into()),
        },
        "settle" => {
            let idle = rest.parse::<u64>().unwrap_or(250).clamp(10, 5_000);
            session.settle(program, Duration::from_millis(idle), Duration::from_secs(30));
            session.record(format!("settle {idle}"));
            out("ok".into());
        }
        "timeout" => match rest.parse::<u64>() {
            Ok(ms) => {
                session.timeout = Duration::from_millis(ms.clamp(100, 600_000));
                session.record(command);
                out("ok".into());
            }
            Err(_) => out("error timeout wants milliseconds: timeout 30000".into()),
        },
        "clipboard" => {
            if rest.is_empty() {
                match session.emulator.clipboard() {
                    Some(Content::Text(text)) => out(format!("clipboard {text:?}")),
                    Some(Content::Html(html)) => out(format!("clipboard html {html:?}")),
                    Some(Content::Files(files)) => out(format!("clipboard files {files:?}")),
                    Some(_) => out("clipboard <non-text content>".into()),
                    None => out("clipboard empty".into()),
                }
            } else {
                match parse_quoted(rest) {
                    Some(text) => {
                        session.emulator.set_clipboard(Some(Content::Text(text)));
                        out("ok".into());
                    }
                    None => out(
                        "error clipboard wants a quoted string: clipboard \"secret\"".into(),
                    ),
                }
            }
        }
        "save" => {
            if rest.is_empty() {
                out("error save wants a path: save tests/e2e/flow.ice".into());
            } else {
                match session.save_ice(&PathBuf::from(rest)) {
                    Ok(_content) => out(format!(
                        "ok saved {} instructions to {rest}",
                        session.history.len()
                    )),
                    Err(reason) => out(format!("error {reason}")),
                }
            }
        }
        "reset" => {
            let wipe = rest == "wipe";
            if !rest.is_empty() && !wipe {
                out("error reset takes nothing or `wipe`: reset wipe".into());
            } else {
                match session.reset(program, wipe) {
                    Ok(Pump::Ready) => out("ok".into()),
                    Ok(Pump::Timeout) => out("timeout (boot still settling)".into()),
                    Ok(Pump::Failed(instruction)) => out(format!("fail {instruction}")),
                    Ok(Pump::Closed) => {
                        out("error emulator channel closed".into());
                        return Control::Dead;
                    }
                    Err(reason) => out(format!("error {reason}")),
                }
            }
        }
        _ => match session.run_line(program, command) {
            RunOutcome::Done => out("ok".into()),
            RunOutcome::Failed(instruction) => out(format!("fail {instruction}")),
            RunOutcome::Timeout => {
                out("timeout (tasks still pending; `settle` may absorb them)".into());
            }
            RunOutcome::Closed => {
                out("error emulator channel closed".into());
                return Control::Dead;
            }
            RunOutcome::Parse(error) => out(format!("error {error}")),
        },
    }
    Control::Continue
}
