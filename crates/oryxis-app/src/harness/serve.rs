//! TCP daemon front-end of the headless harness, paired with the
//! `--harness-ctl` one-shot client.
//!
//! The emulated app is stateful (unlocked vault, navigated screens,
//! live terminal sessions), so per-command CLI ergonomics need a
//! long-lived process holding the emulator plus a thin client that
//! delivers commands to it:
//!
//! ```text
//! oryxis --harness-serve &                 # daemon, boots the app once
//! oryxis --harness-ctl 'click "Keychain"'  # CLI feel, state preserved
//! oryxis --harness-ctl screenshot panel    # prints the PNG path
//! oryxis --harness-ctl quit                # shuts the daemon down
//! ```
//!
//! Wire protocol: the REPL line protocol verbatim, over one TCP
//! connection per batch. The client sends command lines, half-closes
//! its write side, and reads `== `-prefixed response lines until the
//! server closes. Localhost only; the sandbox holds no real secrets
//! (dev feature, `$HOME` redirected before boot like every mode).

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use iced::Program;

use super::commands::{self, Control};
use super::{Options, Pump, Session};

/// Default daemon port ("ORYX" on a phone keypad). Override with
/// `--port` on both `--harness-serve` and `--harness-ctl`.
pub(super) const DEFAULT_PORT: u16 = 6799;

/// How long the daemon waits for the next command line on an open
/// connection before dropping it, so a stuck client can't wedge the
/// single-threaded accept loop forever.
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn serve<P>(program: P, options: Options, port: u16) -> iced::Result
where
    P: Program + 'static,
    P::Message: super::OsEventMessages,
{
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!(
                "oryxis harness: cannot bind {addr}: {e} (another daemon running? \
                 stop it with --harness-ctl quit, or pick a --port)"
            );
            std::process::exit(2);
        }
    };

    let (mut session, boot) = Session::new(&program, &options);
    let boot_note = match boot {
        Pump::Ready => "idle",
        Pump::Timeout => "boot tasks still settling",
        Pump::Failed(_) => "boot reported a failure",
        Pump::Closed => {
            eprintln!("oryxis harness: emulator channel closed during boot");
            std::process::exit(1);
        }
    };
    // The launcher greps this line to know the daemon is up.
    println!(
        "harness listening on {addr} ({boot_note}) home={} shots={}",
        options.home.display(),
        session.shots.display(),
    );
    let _ = std::io::stdout().flush();

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        let _ = stream.set_read_timeout(Some(CLIENT_READ_TIMEOUT));
        match handle_connection(&mut session, &program, stream) {
            Control::Continue => {}
            Control::Quit | Control::Dead => break,
        }
    }

    Ok(())
}

/// Serves one client connection: command lines in, `== ` response
/// lines out, until the client half-closes or asks to quit.
fn handle_connection<P>(
    session: &mut Session<P>,
    program: &P,
    stream: TcpStream,
) -> Control
where
    P: Program + 'static,
    P::Message: super::OsEventMessages,
{
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return Control::Continue,
    };
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let Ok(line) = line else { break };
        let command = line.trim();
        if command.is_empty() || command.starts_with('#') {
            continue;
        }

        // Absorb whatever the subscriptions produced while we were
        // blocked on the socket, so commands act on fresh state.
        session.drain(program);

        let mut failed = false;
        let control = commands::dispatch(session, program, command, &mut |msg| {
            failed |= writeln!(writer, "== {msg}").is_err();
        });
        let _ = writer.flush();
        match control {
            Control::Continue => {}
            Control::Quit => {
                let _ = writeln!(writer, "== bye");
                let _ = writer.flush();
                return Control::Quit;
            }
            Control::Dead => {
                let _ = writer.flush();
                return Control::Dead;
            }
        }
        if failed {
            // Client went away mid-response; drop the connection.
            break;
        }
    }
    Control::Continue
}

/// The `--harness-ctl` one-shot client. Sends the given command (or
/// every stdin line when none was given) to the daemon and prints the
/// raw response. Never returns: exits 0 when every command line
/// succeeded, 1 when any response reported `error` / `fail`, 2 when
/// the daemon is unreachable.
pub(super) fn ctl(port: u16, command: Option<String>) -> ! {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!(
                "oryxis harness: no daemon on {addr}: {e}\n\
                 start one with: cargo run -q -p oryxis-app --features harness -- --harness-serve &"
            );
            std::process::exit(2);
        }
    };
    // Generous: a `reset wipe` re-boots the whole app before answering.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(180)));

    let payload = match command {
        Some(command) => command,
        None => {
            let mut buffer = String::new();
            if std::io::stdin().read_to_string(&mut buffer).is_err() {
                eprintln!("oryxis harness: reading stdin failed");
                std::process::exit(2);
            }
            buffer
        }
    };

    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(e) => {
            eprintln!("oryxis harness: {e}");
            std::process::exit(2);
        }
    };
    for line in payload.lines() {
        if writeln!(writer, "{line}").is_err() {
            eprintln!("oryxis harness: daemon closed the connection early");
            std::process::exit(2);
        }
    }
    // Half-close: tells the daemon the batch is complete, so it
    // closes its side once every response was written and our read
    // loop below terminates on EOF.
    let _ = stream.shutdown(std::net::Shutdown::Write);

    let mut failed = false;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        failed |= line.starts_with("== error") || line.starts_with("== fail");
        println!("{line}");
    }
    let _ = std::io::stdout().flush();
    std::process::exit(i32::from(failed));
}
