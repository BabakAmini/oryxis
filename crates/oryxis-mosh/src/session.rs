//! A mosh session, shaped like every other transport a pane can hold.
//!
//! The pane takes bytes. That is not a simplification: `Backend::process`
//! filters `screen`'s window titles out of the stream, scans it for the
//! highlight rules that carry an action, and sniffs OSC 7 / 133 / 9 for
//! the working directory, the prompt marks and desktop notifications.
//! A transport that handed over a grid instead would silently lose all
//! of it. So this drives `mosh_rs::MoshSession` and publishes what it
//! says the terminal is missing, which is bytes, on the same channel
//! shape Telnet and Serial use.
//!
//! The screen those bytes are computed against is alacritty, the SAME
//! emulator the pane draws with, so there is one implementation and one
//! opinion about the screen rather than two. See [`crate::screen`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mosh_rs::{Base64Key, MoshSession as Protocol};
use tokio::sync::mpsc;

/// The screen the protocol keeps its states on. See the module note.
type Screen = crate::screen::AlacrittyScreen;

/// How long the driving task sleeps when the session has nothing due.
///
/// Not a poll interval for INPUT: what the user types arrives on a
/// channel that wakes the task at once. This only bounds how long an
/// idle session goes between looking at its own clock, which is what
/// keeps the link-health figures moving on a link that has gone quiet.
const IDLE_TICK: Duration = Duration::from_millis(100);

/// Why a session could not be started or could not go on.
#[derive(Debug, thiserror::Error)]
pub enum MoshError {
    /// The key the host announced is not a key.
    #[error("the session key from the host is malformed")]
    BadKey,
    /// The UDP socket could not be opened or the address is unusable.
    #[error("could not open a session to {0}: {1}")]
    Unreachable(String, String),
}

/// A live mosh session.
///
/// The same surface a `TelnetSession` offers, so the pane that holds it
/// does not have to know which one it has.
#[derive(Debug)]
pub struct MoshSession {
    writer_tx: mpsc::UnboundedSender<Vec<u8>>,
    resize_tx: mpsc::UnboundedSender<(u16, u16)>,
    alive: Arc<AtomicBool>,
    /// Set when the user or the app wants the session to end, so the
    /// driving task says goodbye to the server rather than vanishing
    /// and leaving it to time out with a shell still open.
    closing: Arc<AtomicBool>,
}

impl MoshSession {
    /// Open a session against a server that has already announced
    /// itself, and start driving it.
    ///
    /// `host` is where the SSH connection went, not what the server
    /// reported: the server binds the address the SSH session arrived
    /// on (`mosh-server -s`), which is the address already known to be
    /// reachable from here. A jump chain makes that the LAST hop, and
    /// this is the one place that difference matters.
    pub fn connect(
        host: &str,
        port: u16,
        key: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Vec<u8>>), MoshError> {
        let key = Base64Key::from_printable(key).map_err(|_| MoshError::BadKey)?;
        // The screen is supplied rather than asked for: `connect_with_size`
        // only exists for the built-in one, which is not in the build.
        let protocol = Protocol::connect_with_screen(
            host,
            port,
            &key,
            Screen::new(rows, cols),
        )
        .map_err(|e| MoshError::Unreachable(format!("{host}:{port}"), e.to_string()))?;

        let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (resize_tx, resize_rx) = mpsc::unbounded_channel::<(u16, u16)>();
        let alive = Arc::new(AtomicBool::new(true));
        let closing = Arc::new(AtomicBool::new(false));

        tokio::spawn(drive(
            protocol,
            output_tx,
            writer_rx,
            resize_rx,
            Arc::clone(&alive),
            Arc::clone(&closing),
        ));

        Ok((Self { writer_tx, resize_tx, alive, closing }, output_rx))
    }

    /// Send what the user typed.
    pub fn write(&self, data: &[u8]) -> Result<(), MoshError> {
        let _ = self.writer_tx.send(data.to_vec());
        Ok(())
    }

    /// Tell the far end the window changed shape.
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.resize_tx.send((cols, rows));
    }

    /// The channel a caller can hand to something that produces input
    /// of its own, the way the other transports expose theirs.
    pub fn write_sender(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.writer_tx.clone()
    }

    /// The resize channel, same reason.
    pub fn resize_sender(&self) -> mpsc::UnboundedSender<(u16, u16)> {
        self.resize_tx.clone()
    }

    /// Whether the session is still running.
    ///
    /// A mosh session is alive across a network that is not: losing the
    /// path does NOT end it, which is the whole point of the protocol,
    /// so this stays true through a change of address and only goes
    /// false once the far end has actually gone.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// End the session, letting the server hear about it.
    ///
    /// Asks rather than aborts: a mosh server whose client vanishes
    /// holds the shell open until it times out, and a user who closed a
    /// tab does not expect to find it still there.
    pub fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
}

/// The task that owns the protocol session.
///
/// Everything the session needs to be told arrives on a channel, and
/// everything it produces goes out on one, so the only thing that ever
/// touches `Protocol` is this task. That is what lets the session be
/// held behind an `Arc` by a pane without a lock.
async fn drive(
    mut protocol: Protocol<Screen>,
    output_tx: mpsc::UnboundedSender<Vec<u8>>,
    mut writer_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut resize_rx: mpsc::UnboundedReceiver<(u16, u16)>,
    alive: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
) {
    let mut said_goodbye = false;
    loop {
        if closing.load(Ordering::SeqCst) && !said_goodbye {
            protocol.shutdown();
            said_goodbye = true;
        }

        // How long there is before the protocol needs to send. Waiting
        // exactly that long is what keeps a keystroke from sitting out
        // a fixed poll interval, which measured as the difference
        // between this being as fast as the C++ client and being seven
        // times slower.
        let wait = Duration::from_millis(protocol.wait_time_ms()).min(IDLE_TICK);

        tokio::select! {
            // Biased so input is taken before the timer: a keystroke
            // that arrived while the timer was expiring goes out in
            // THIS cycle rather than the next one.
            biased;
            Some(bytes) = writer_rx.recv() => protocol.send_input(&bytes),
            Some((cols, rows)) = resize_rx.recv() => {
                protocol.send_resize(i32::from(cols), i32::from(rows));
            }
            () = tokio::time::sleep(wait) => {}
        }

        if let Err(error) = protocol.pump_ready() {
            tracing::warn!(%error, "mosh session ended");
            break;
        }

        // What the terminal is missing, and nothing more. Empty on most
        // passes, which is what makes a retransmitted frame free.
        let frame = protocol.render();
        if !frame.is_empty() && output_tx.send(frame).is_err() {
            // Nobody is drawing this any more.
            break;
        }

        if protocol.finished() {
            break;
        }
    }
    alive.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_that_is_not_a_key_is_refused_before_a_socket_is_opened() {
        let opened = MoshSession::connect("127.0.0.1", 1, "not a key", 80, 24);
        assert!(matches!(opened, Err(MoshError::BadKey)));
    }

    #[tokio::test]
    async fn a_session_with_nowhere_to_go_still_opens_and_stays_alive() {
        // Pointed at a port nothing answers on. mosh is a protocol for
        // links that are not working yet, so opening has to succeed and
        // the session has to stay up: a client that gave up here would
        // give up on a laptop that had not joined the wifi.
        let (session, _rx) = MoshSession::connect(
            "127.0.0.1",
            1,
            "AAAAAAAAAAAAAAAAAAAAAA",
            80,
            24,
        )
        .expect("a socket is all it takes to start");
        assert!(session.is_alive());
        session.write(b"x").expect("input is queued, not delivered");
        session.resize(100, 30);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(session.is_alive(), "silence is not the end of a mosh session");
    }
}
