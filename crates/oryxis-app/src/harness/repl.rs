//! Line-protocol front-end of the headless harness.
//!
//! One command per stdin line; every response line is prefixed with
//! `== ` so it can be told apart from tracing output sharing stdout:
//! `== ok`, `== fail <instruction>`, `== timeout ...`,
//! `== shot <path>`, `== error <reason>`. The command grammar lives in
//! `commands::dispatch`, shared with the `--harness-serve` daemon.

use std::io::{BufRead as _, Write as _};

use iced::Program;

use super::commands::{self, Control};
use super::{Options, Pump, Session};

pub(super) fn serve<P>(program: P, options: Options) -> iced::Result
where
    P: Program + 'static,
    P::Message: super::OsEventMessages,
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

        let control =
            commands::dispatch(&mut session, &program, command, &mut |msg| respond(msg));
        match control {
            Control::Continue => {}
            Control::Quit | Control::Dead => break,
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
