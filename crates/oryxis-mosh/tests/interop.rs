//! The crate against a real `mosh-server`.
//!
//! Ignored by default because it needs one running. The bootstrap half
//! is pure and tested without a server; this is the half that can only
//! be answered by the thing it interoperates with.
//!
//! ```text
//! mosh-server new -i 127.0.0.1 -p 60123 -- /bin/bash
//! MOSH_RS_TEST_PORT=60123 MOSH_RS_TEST_KEY=<the 22-char key it printed> \
//!     cargo test -p oryxis-mosh --test interop -- --ignored --nocapture
//! ```
//!
//! One server per test: `mosh-server` serves a single session, so a
//! second test against the same one fails.

use std::time::{Duration, Instant};

use oryxis_mosh::MoshSession;

fn endpoint() -> (u16, String) {
    let port = std::env::var("MOSH_RS_TEST_PORT").expect("MOSH_RS_TEST_PORT");
    let key = std::env::var("MOSH_RS_TEST_KEY").expect("MOSH_RS_TEST_KEY");
    (port.parse().expect("a port"), key)
}

/// Collect what the session says the terminal is missing, until `want`
/// shows up or the clock runs out.
async fn until(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    seen: &mut Vec<u8>,
    want: &str,
    limit: Duration,
) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(frame)) => seen.extend_from_slice(&frame),
            Ok(None) => return false,
            Err(_) => return false,
        }
        if String::from_utf8_lossy(seen).contains(want) {
            return true;
        }
    }
    false
}

/// A shell, a command, and its output back: the whole point.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live mosh-server; see the module docs"]
async fn a_shell_answers_through_the_session() {
    let (port, key) = endpoint();
    let (session, mut rx) =
        MoshSession::connect("127.0.0.1", port, &key, 80, 24).expect("open the session");

    let mut seen = Vec::new();
    assert!(
        until(&mut rx, &mut seen, "$", Duration::from_secs(10)).await,
        "no prompt arrived: {:?}",
        String::from_utf8_lossy(&seen)
    );

    session.write(b"echo ORYXIS-MOSH-OK\r").expect("send");
    assert!(
        until(&mut rx, &mut seen, "ORYXIS-MOSH-OK", Duration::from_secs(10)).await,
        "the command never came back: {:?}",
        String::from_utf8_lossy(&seen)
    );
    assert!(session.is_alive());
}

/// What the pane is handed has to be ESCAPES, not a grid, because
/// everything downstream of it reads the byte stream: the highlight
/// rule triggers, the OSC 7 working directory, the prompt marks.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live mosh-server; see the module docs"]
async fn what_arrives_is_a_byte_stream_a_terminal_can_eat() {
    let (port, key) = endpoint();
    let (_session, mut rx) =
        MoshSession::connect("127.0.0.1", port, &key, 80, 24).expect("open the session");

    let mut seen = Vec::new();
    assert!(
        until(&mut rx, &mut seen, "$", Duration::from_secs(10)).await,
        "no prompt arrived"
    );
    assert!(
        seen.contains(&0x1b),
        "a frame with no escape in it is not a terminal stream: {:?}",
        String::from_utf8_lossy(&seen)
    );
}

/// A resize has to reach the shell, or the server paints for a window
/// that is not there and every cursor position lands wrong.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live mosh-server; see the module docs"]
async fn a_resize_reaches_the_shell() {
    let (port, key) = endpoint();
    let (session, mut rx) =
        MoshSession::connect("127.0.0.1", port, &key, 80, 24).expect("open the session");

    let mut seen = Vec::new();
    assert!(until(&mut rx, &mut seen, "$", Duration::from_secs(10)).await, "no prompt");

    session.resize(100, 30);
    session.write(b"stty size\r").expect("send");
    assert!(
        until(&mut rx, &mut seen, "30 100", Duration::from_secs(10)).await,
        "the shell never saw 30x100: {:?}",
        String::from_utf8_lossy(&seen)
    );
}

/// Closing says goodbye rather than vanishing. A server whose client
/// disappears holds the shell open until it times out, and a user who
/// closed a tab does not expect to find it still running.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live mosh-server; see the module docs"]
async fn closing_ends_the_session_rather_than_abandoning_it() {
    let (port, key) = endpoint();
    let (session, mut rx) =
        MoshSession::connect("127.0.0.1", port, &key, 80, 24).expect("open the session");

    let mut seen = Vec::new();
    assert!(until(&mut rx, &mut seen, "$", Duration::from_secs(10)).await, "no prompt");

    session.close();
    let deadline = Instant::now() + Duration::from_secs(10);
    while session.is_alive() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!session.is_alive(), "the session never finished shutting down");
}
