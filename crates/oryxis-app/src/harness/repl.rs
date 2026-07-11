//! Line-protocol front-end of the headless harness.
//!
//! One command per stdin line; every response line is prefixed with
//! `== ` so it can be told apart from tracing output sharing stdout:
//! `== ok`, `== fail <instruction>`, `== timeout ...`,
//! `== shot <path>`, `== error <reason>`.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::time::Duration;

use iced::Program;
use iced_test::core::clipboard::Content;

use super::{Options, Pump, RunOutcome, Session, format_text_entry, parse_quoted};

const HELP: &str = "\
instructions: click [right] \"Text\"|#id|(x, y) / press / release / move <target>
              scroll [pixels] (dx, dy) [<target>] / type \"text\"
              type enter|escape|tab|backspace / type ctrl+k / type ctrl+shift+f
              press enter / release tab / expect \"Text\"
harness:      screenshot [name] / texts / find \"Text\" / clipboard [\"text\"]
              wait <ms> / settle [idle_ms] / timeout <ms> / save <path.ice>
              reset [wipe] / help / quit
responses:    == ok | == fail <instr> | == timeout | == shot <path> | == error <..>";

pub(super) fn serve<P>(program: P, options: Options) -> iced::Result
where
    P: Program + 'static,
{
    let (mut session, boot) = Session::new(&program, &options);
    match boot {
        Pump::Ready => {}
        Pump::Timeout => respond("boot timeout (continuing; try `settle` or `wait`)"),
        Pump::Failed(instruction) => respond(format!("boot fail {instruction}")),
        Pump::Closed => {
            respond("error emulator channel closed during boot");
            return Ok(());
        }
    }
    respond(format!(
        "harness ready home={} shots={} viewport={}x{} mode={:?}",
        options.home.display(),
        session.shots.display(),
        options.viewport.width,
        options.viewport.height,
        options.mode,
    ));

    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        // Absorb whatever the subscriptions produced while we were
        // blocked on stdin, so commands act on fresh state.
        session.drain(&program);

        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let command = line.trim();
        if command.is_empty() || command.starts_with('#') {
            continue;
        }

        let (head, rest) = match command.split_once(char::is_whitespace) {
            Some((head, rest)) => (head, rest.trim()),
            None => (command, ""),
        };

        match head {
            "quit" | "exit" => break,
            "help" => {
                for line in HELP.lines() {
                    respond(line);
                }
            }
            "screenshot" => match session.screenshot(&program, rest) {
                Ok((path, _png)) => {
                    session.record(command);
                    respond(format!("shot {}", path.display()));
                }
                Err(reason) => respond(format!("error {reason}")),
            },
            "texts" => match session.texts(&program) {
                Ok(entries) => {
                    let count = entries.len();
                    for (text, bounds) in entries {
                        respond(format_text_entry(&text, bounds));
                    }
                    respond(format!("ok {count} texts"));
                }
                Err(reason) => respond(format!("error {reason}")),
            },
            "find" => match parse_quoted(rest) {
                Some(needle) => match session.texts(&program) {
                    Ok(entries) => {
                        let matches: Vec<_> = entries
                            .into_iter()
                            .filter(|(text, _)| text.contains(&needle))
                            .collect();
                        let count = matches.len();
                        for (text, bounds) in matches {
                            respond(format_text_entry(&text, bounds));
                        }
                        respond(format!("ok {count} matches"));
                    }
                    Err(reason) => respond(format!("error {reason}")),
                },
                None => respond("error find wants a quoted string: find \"Hosts\""),
            },
            "wait" => match rest.parse::<u64>() {
                Ok(ms) => {
                    session.wait(&program, Duration::from_millis(ms.min(600_000)));
                    session.record(command);
                    respond("ok");
                }
                Err(_) => respond("error wait wants milliseconds: wait 500"),
            },
            "settle" => {
                let idle = rest.parse::<u64>().unwrap_or(250).clamp(10, 5_000);
                session.settle(
                    &program,
                    Duration::from_millis(idle),
                    Duration::from_secs(30),
                );
                session.record(format!("settle {idle}"));
                respond("ok");
            }
            "timeout" => match rest.parse::<u64>() {
                Ok(ms) => {
                    session.timeout = Duration::from_millis(ms.clamp(100, 600_000));
                    session.record(command);
                    respond("ok");
                }
                Err(_) => respond("error timeout wants milliseconds: timeout 30000"),
            },
            "clipboard" => {
                if rest.is_empty() {
                    match session.emulator.clipboard() {
                        Some(Content::Text(text)) => respond(format!("clipboard {text:?}")),
                        Some(Content::Html(html)) => {
                            respond(format!("clipboard html {html:?}"));
                        }
                        Some(Content::Files(files)) => {
                            respond(format!("clipboard files {files:?}"));
                        }
                        Some(_) => respond("clipboard <non-text content>"),
                        None => respond("clipboard empty"),
                    }
                } else {
                    match parse_quoted(rest) {
                        Some(text) => {
                            session.emulator.set_clipboard(Some(Content::Text(text)));
                            respond("ok");
                        }
                        None => respond(
                            "error clipboard wants a quoted string: clipboard \"secret\"",
                        ),
                    }
                }
            }
            "save" => {
                if rest.is_empty() {
                    respond("error save wants a path: save tests/e2e/flow.ice");
                } else {
                    match session.save_ice(&PathBuf::from(rest)) {
                        Ok(_content) => respond(format!(
                            "ok saved {} instructions to {rest}",
                            session.history.len()
                        )),
                        Err(reason) => respond(format!("error {reason}")),
                    }
                }
            }
            "reset" => {
                let wipe = rest == "wipe";
                if !rest.is_empty() && !wipe {
                    respond("error reset takes nothing or `wipe`: reset wipe");
                } else {
                    match session.reset(&program, wipe) {
                        Ok(Pump::Ready) => respond("ok"),
                        Ok(Pump::Timeout) => respond("timeout (boot still settling)"),
                        Ok(Pump::Failed(instruction)) => {
                            respond(format!("fail {instruction}"));
                        }
                        Ok(Pump::Closed) => {
                            respond("error emulator channel closed");
                            break;
                        }
                        Err(reason) => respond(format!("error {reason}")),
                    }
                }
            }
            _ => match session.run_line(&program, command) {
                RunOutcome::Done => respond("ok"),
                RunOutcome::Failed(instruction) => respond(format!("fail {instruction}")),
                RunOutcome::Timeout => {
                    respond("timeout (tasks still pending; `settle` may absorb them)");
                }
                RunOutcome::Closed => {
                    respond("error emulator channel closed");
                    break;
                }
                RunOutcome::Parse(error) => respond(format!("error {error}")),
            },
        }
    }

    respond("bye");
    Ok(())
}

/// Protocol response: one line, `== ` prefixed (so it can't be
/// confused with tracing output on the same stream), flushed
/// immediately because stdout is block-buffered when piped.
fn respond(message: impl AsRef<str>) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "== {}", message.as_ref());
    let _ = stdout.flush();
}
