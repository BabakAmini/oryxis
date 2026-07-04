//! Interop test over a REAL OS pseudo-terminal (not the in-memory
//! `tokio::io::duplex` the unit tests use). A pty pair is the closest
//! thing to a real serial device available in CI without hardware: it
//! proves `tokio-serial` can actually open a tty path and that bytes
//! round-trip both directions through the OS line discipline.
//!
//! Unix-only (ptys don't exist on Windows); the CI Linux job runs it.

#![cfg(unix)]

use std::io::{Read, Write};
use std::time::Duration;

use nix::pty::openpty;
use nix::sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr};
use oryxis_core::models::serial::SerialParams;
use oryxis_serial::{SerialConfig, SerialSession};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_trips_over_a_real_pty() {
    let pty = openpty(None, None).expect("openpty");
    // Raw the slave so there's no echo / line-buffering between the two
    // ends (otherwise a payload without a newline never arrives under
    // canonical mode). tokio-serial re-raws it on open too; this is
    // belt-and-suspenders so the test can't hang.
    {
        let mut t = tcgetattr(&pty.slave).expect("tcgetattr");
        cfmakeraw(&mut t);
        tcsetattr(&pty.slave, SetArg::TCSANOW, &t).expect("tcsetattr");
    }
    let slave_path = nix::unistd::ttyname(&pty.slave).expect("ttyname");
    // Let the SerialSession own the slave via its own open-by-path; the
    // pts survives on the still-open master.
    drop(pty.slave);
    let mut master = std::fs::File::from(pty.master);

    let (session, mut rx) = SerialSession::open(SerialConfig {
        path: slave_path.to_string_lossy().into_owned(),
        params: SerialParams::default(),
    })
    .expect("open serial on the pty slave");

    // Device -> terminal: bytes written to the master surface as pane
    // output. Accumulate across reads in case the pump splits them.
    master.write_all(b"hello-from-device").expect("write master");
    master.flush().unwrap();
    let mut seen = Vec::new();
    while !seen.windows(17).any(|w| w == b"hello-from-device") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("no serial output")
            .expect("serial stream closed");
        seen.extend_from_slice(&chunk);
    }

    // Terminal -> device: input written to the session reaches the
    // master. No CR, so the line-ending map is a no-op and bytes pass
    // through 1:1.
    session.write(b"hi-device").expect("session write");
    let read_back = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 64];
        // The session flushed, so the bytes are waiting in the master.
        while !out.windows(9).any(|w| w == b"hi-device") {
            let n = master.read(&mut buf).expect("read master");
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    });
    let out = tokio::time::timeout(Duration::from_secs(5), read_back)
        .await
        .expect("master read timed out")
        .unwrap();
    assert!(
        out.windows(9).any(|w| w == b"hi-device"),
        "device never saw the input: {out:?}"
    );

    session.close();
}
