//! Native serial-line "session" for Oryxis.
//!
//! Mirrors `oryxis-ssh` / `oryxis-telnet`'s session shape so the
//! terminal pane holds every transport behind one enum:
//! [`SerialSession::open`] returns a session handle plus an unbounded
//! output receiver, and the handle exposes `write` / `resize` (a
//! no-op, a serial line has no window size) / `is_alive` / `close`.
//!
//! There is no protocol negotiation: raw bytes flow both ways. Two
//! device-facing conveniences the wire itself doesn't provide are
//! handled here because a raw line offers no equivalent of SSH/Telnet
//! ECHO:
//!
//! - **line ending**: the terminal sends a bare `\r` for Enter (and
//!   app-injected lines end with a bare `\n`); the configured
//!   [`SerialLineEnding`](oryxis_core::models::serial::SerialLineEnding)
//!   maps either to CR / LF / CR LF on the wire.
//! - **local echo**: when enabled, user input written through
//!   [`SerialSession::write`] is echoed back into the output stream so
//!   a non-echoing device still shows typing.
//!
//! Both conveniences apply to USER input only. Programmatic wire
//! writes ([`SerialSession::write_sender`]: in-band terminal replies,
//! ZMODEM protocol frames) go out byte-exact and silent; echoing them
//! would feed protocol replies back into their own driver, and the
//! line-ending map would corrupt raw frames containing 0x0D.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oryxis_core::models::serial::{
    SerialFlowControl, SerialLineEnding, SerialParams, SerialParity, SerialStopBits,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;

#[derive(Debug, thiserror::Error)]
pub enum SerialError {
    #[error("Failed to open serial port {path}: {source}")]
    Open {
        path: String,
        source: tokio_serial::Error,
    },
}

/// Everything needed to open a serial line: the OS port path plus the
/// line parameters (a copy of the model's `SerialParams`).
#[derive(Debug, Clone)]
pub struct SerialConfig {
    /// OS port path (`COM3`, `/dev/ttyUSB0`, ...).
    pub path: String,
    pub params: SerialParams,
}

/// A live serial line.
pub struct SerialSession {
    /// Terminal input; the writer task maps Enter to the line ending
    /// and (optionally) echoes locally before writing to the port.
    writer_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Programmatic wire writes (protocol replies, ZMODEM frames):
    /// written to the port verbatim, never echoed, never re-encoded.
    raw_tx: mpsc::UnboundedSender<Vec<u8>>,
    reader_task: tokio::task::JoinHandle<()>,
    writer_task: tokio::task::JoinHandle<()>,
    closed: AtomicBool,
    /// Set by the reader on its way out, BEFORE it drops the output
    /// sender. See [`SerialSession::is_alive`] for why the order is the
    /// whole point.
    reader_done: Arc<AtomicBool>,
}

impl std::fmt::Debug for SerialSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialSession")
            .field("alive", &self.is_alive())
            .finish()
    }
}

/// Map a `data_bits` count to the driver enum, clamping an out-of-range
/// value back to 8 (the only sane fallback, and what the editor offers
/// by default).
fn data_bits(n: u8) -> tokio_serial::DataBits {
    match n {
        5 => tokio_serial::DataBits::Five,
        6 => tokio_serial::DataBits::Six,
        7 => tokio_serial::DataBits::Seven,
        _ => tokio_serial::DataBits::Eight,
    }
}

fn parity(p: SerialParity) -> tokio_serial::Parity {
    match p {
        SerialParity::None => tokio_serial::Parity::None,
        SerialParity::Odd => tokio_serial::Parity::Odd,
        SerialParity::Even => tokio_serial::Parity::Even,
    }
}

fn stop_bits(s: SerialStopBits) -> tokio_serial::StopBits {
    match s {
        SerialStopBits::One => tokio_serial::StopBits::One,
        SerialStopBits::Two => tokio_serial::StopBits::Two,
    }
}

fn flow_control(f: SerialFlowControl) -> tokio_serial::FlowControl {
    match f {
        SerialFlowControl::None => tokio_serial::FlowControl::None,
        SerialFlowControl::Software => tokio_serial::FlowControl::Software,
        SerialFlowControl::Hardware => tokio_serial::FlowControl::Hardware,
    }
}

/// Map terminal input bytes onto the wire: a line submit becomes the
/// configured line ending. A submit is the Enter key's bare `\r`, an
/// app-injected line's bare `\n` (snippets, startup commands, autofill
/// all terminate with `\n`), or an explicit `\r\n` pair, which
/// collapses so a CR LF from the terminal stays a single ending rather
/// than doubling.
fn encode_input(data: &[u8], ending: SerialLineEnding) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 2);
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'\r' => {
                out.extend_from_slice(ending.bytes());
                if data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            // A lone LF (any CR-paired one was consumed above) is an
            // app-injected line submit; map it like Enter.
            b'\n' => out.extend_from_slice(ending.bytes()),
            b => out.push(b),
        }
        i += 1;
    }
    out
}

impl SerialSession {
    /// Open the port and start the read / write pumps. Returns the
    /// session plus the raw output stream; the receiver ends (`None`)
    /// when the port errors or is unplugged.
    pub fn open(
        config: SerialConfig,
    ) -> Result<(SerialSession, mpsc::UnboundedReceiver<Vec<u8>>), SerialError> {
        let params = config.params;
        let port = tokio_serial::new(&config.path, params.baud)
            .data_bits(data_bits(params.data_bits))
            .parity(parity(params.parity))
            .stop_bits(stop_bits(params.stop_bits))
            .flow_control(flow_control(params.flow_control))
            .open_native_async()
            .map_err(|source| SerialError::Open {
                path: config.path.clone(),
                source,
            })?;
        Ok(Self::run(port, params))
    }

    /// Spawn the read / write pumps over an already-open duplex stream.
    /// Split out from [`open`] so the disconnect invariant is testable
    /// over a mock duplex without a real serial device.
    ///
    /// Invariant: the READER is the sole owner of `output_tx`, so "the
    /// stream ends" (`output_rx` yields `None`) is exactly "the reader
    /// saw EOF / an error", i.e. the device is gone. Local echo is
    /// therefore routed THROUGH the reader (writer -> `echo_tx` ->
    /// reader -> `output_tx`) rather than the writer holding its own
    /// `output_tx` clone, which would keep the stream open forever
    /// after an unplug and leave the tab a dead input sink.
    fn run<S>(stream: S, params: SerialParams) -> (SerialSession, mpsc::UnboundedReceiver<Vec<u8>>)
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut read_half, mut write_half) = tokio::io::split(stream);
        let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // Programmatic wire writes: verbatim, no echo, no line-ending
        // map (see `write_sender`).
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // Local-echo side channel: writer -> reader. Kept alive as long
        // as the writer lives; when it dies the reader's `recv()` yields
        // `None` and that select branch simply disables itself.
        let (echo_tx, mut echo_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // Reader -> writer death cascade: the sender lives in the reader
        // task and drops when it exits (EOF / unplug / read error), which
        // resolves the receiver and ends the writer too. Without it the
        // writer would park in `recv()` forever after an unplug (the
        // session still holds both input senders), and `is_alive()`
        // would keep reporting a dead port as alive.
        let (reader_gone_tx, mut reader_gone_rx) = tokio::sync::oneshot::channel::<()>();

        // Published by the reader before the output sender is dropped,
        // so the session reads as dead by the time the app notices the
        // stream ended. See `is_alive`.
        let reader_done = Arc::new(AtomicBool::new(false));
        let reader_flag = Arc::clone(&reader_done);

        // Reader task: port + echo -> output stream. Raw passthrough (no
        // sniffing, no transcode): the terminal emulator owns decoding.
        let reader_task = tokio::spawn(async move {
            // Owned so it drops (firing the writer cascade) when this
            // task ends, however it ends.
            let _reader_gone = reader_gone_tx;
            let mut buf = vec![0u8; 4096];
            loop {
                tokio::select! {
                    read = read_half.read(&mut buf) => match read {
                        Ok(0) => {
                            tracing::info!("Serial port closed (EOF)");
                            break;
                        }
                        Ok(n) => {
                            if output_tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            // A physical unplug surfaces here; end the stream.
                            tracing::info!("Serial read error: {}", e);
                            break;
                        }
                    },
                    // Disabled once the writer drops `echo_tx` (recv ->
                    // None); the read branch keeps the loop alive.
                    Some(echo) = echo_rx.recv() => {
                        if output_tx.send(echo).is_err() {
                            break;
                        }
                    }
                }
            }
            // Dead BEFORE silent, and in that order on purpose: the
            // app takes the end of this stream as the disconnect notice
            // and asks `is_alive()` before acting on it, so the answer
            // has to be settled first. Dropping `output_tx` then ends
            // the app-side stream cleanly (recv -> None), so the
            // disconnect propagates on unplug.
            reader_flag.store(true, Ordering::SeqCst);
            drop(output_tx);
        });

        // Writer task: user input gets the line-ending map and the
        // optional local echo (via the reader); raw wire writes go out
        // byte-exact and silent. Both channels close together (the
        // session holds both senders), so either `None` ends the task.
        let local_echo = params.local_echo;
        let line_ending = params.line_ending;
        let writer_task = tokio::spawn(async move {
            loop {
                let bytes = tokio::select! {
                    data = writer_rx.recv() => match data {
                        Some(data) => {
                            let encoded = encode_input(&data, line_ending);
                            if local_echo {
                                // Echo the exact wire form (Enter as the
                                // configured ending) through the reader so
                                // it shares the one `output_tx` owner. A
                                // send error means the reader (and thus the
                                // port) is gone; the write below then fails
                                // too and ends the task.
                                let _ = echo_tx.send(encoded.clone());
                            }
                            encoded
                        }
                        None => break,
                    },
                    data = raw_rx.recv() => match data {
                        Some(data) => data,
                        None => break,
                    },
                    // Reader died (unplug / EOF): stop too, so the input
                    // channels close and `is_alive()` flips promptly,
                    // mirroring the telnet writer's reader-gone break.
                    _ = &mut reader_gone_rx => break,
                };
                if let Err(e) = write_half.write_all(&bytes).await {
                    tracing::error!("Serial write error: {}", e);
                    break;
                }
                if let Err(e) = write_half.flush().await {
                    tracing::error!("Serial flush error: {}", e);
                    break;
                }
            }
        });

        (
            SerialSession {
                writer_tx,
                raw_tx,
                reader_task,
                writer_task,
                closed: AtomicBool::new(false),
                reader_done,
            },
            output_rx,
        )
    }

    pub fn write(&self, data: &[u8]) -> Result<(), SerialError> {
        // A closed channel means the port is gone; drop silently, the
        // dead-session sweep surfaces it as a disconnect elsewhere.
        let _ = self.writer_tx.send(data.to_vec());
        Ok(())
    }

    /// Hand out a clone of the RAW wire sender so the terminal emulator
    /// can answer in-band queries directly and the ZMODEM driver can
    /// write protocol frames, same contract as the SSH/Telnet sessions'
    /// back-channel. Bytes sent here reach the port verbatim: no local
    /// echo (echoing protocol replies would divert them straight back
    /// into the ZMODEM driver and corrupt the transfer) and no
    /// line-ending mapping (raw frames contain 0x0D bytes that must
    /// survive untouched).
    pub fn write_sender(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.raw_tx.clone()
    }

    pub fn is_alive(&self) -> bool {
        // `reader_done` is what makes this answer ORDERED rather than
        // merely eventual. The app reads the end of the output stream as
        // the disconnect notice and asks this before acting on it; the
        // writer channel closes only after the oneshot cascade above has
        // woken the WRITER TASK and it has returned, which is a separate
        // task and therefore a separate scheduling decision. A notice
        // that overtook it would read as coming from a session the pane
        // has already replaced, and be discarded. The flag is set by the
        // reader itself, before it drops the output sender.
        !self.reader_done.load(Ordering::SeqCst) && !self.writer_tx.is_closed()
    }

    /// Tear the session down. Idempotent: only the first call acts.
    /// Aborting both tasks drops the port handle (closing it) and the
    /// output sender (ending the app-side stream cleanly).
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

impl Drop for SerialSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_maps_to_configured_line_ending() {
        assert_eq!(encode_input(b"ls\r", SerialLineEnding::Cr), b"ls\r".to_vec());
        assert_eq!(encode_input(b"ls\r", SerialLineEnding::Lf), b"ls\n".to_vec());
        assert_eq!(
            encode_input(b"ls\r", SerialLineEnding::CrLf),
            b"ls\r\n".to_vec()
        );
        // A terminal CR LF collapses to a single ending, not two.
        assert_eq!(
            encode_input(b"ls\r\n", SerialLineEnding::CrLf),
            b"ls\r\n".to_vec()
        );
        assert_eq!(
            encode_input(b"ls\r\n", SerialLineEnding::Lf),
            b"ls\n".to_vec()
        );
    }

    #[test]
    fn bare_lf_maps_to_configured_line_ending() {
        // App-injected lines (snippet Run, startup command, autofill)
        // terminate with a bare LF; it must submit exactly like Enter.
        assert_eq!(encode_input(b"ls\n", SerialLineEnding::Cr), b"ls\r".to_vec());
        assert_eq!(encode_input(b"ls\n", SerialLineEnding::Lf), b"ls\n".to_vec());
        assert_eq!(
            encode_input(b"ls\n", SerialLineEnding::CrLf),
            b"ls\r\n".to_vec()
        );
        // A CR LF pair still collapses to ONE ending, never two.
        assert_eq!(
            encode_input(b"ls\r\n", SerialLineEnding::Cr),
            b"ls\r".to_vec()
        );
        // Mid-string mix: CR, lone LF and CR LF each submit once.
        assert_eq!(
            encode_input(b"a\rb\nc\r\nd", SerialLineEnding::Cr),
            b"a\rb\rc\rd".to_vec()
        );
        assert_eq!(
            encode_input(b"a\rb\nc\r\nd", SerialLineEnding::CrLf),
            b"a\r\nb\r\nc\r\nd".to_vec()
        );
    }

    #[test]
    fn non_enter_bytes_pass_through() {
        assert_eq!(
            encode_input(&[0x03, b'a', 0x1b], SerialLineEnding::Cr),
            vec![0x03, b'a', 0x1b]
        );
    }

    #[test]
    fn data_bits_clamps_out_of_range_to_eight() {
        assert_eq!(data_bits(8), tokio_serial::DataBits::Eight);
        assert_eq!(data_bits(5), tokio_serial::DataBits::Five);
        assert_eq!(data_bits(9), tokio_serial::DataBits::Eight);
        assert_eq!(data_bits(0), tokio_serial::DataBits::Eight);
    }

    /// The disconnect invariant: when the underlying stream reaches EOF
    /// (device unplugged), `output_rx` must yield the pending bytes and
    /// then close (`None`), even with `local_echo` on, holding the echo
    /// side channel. A regression of the "writer keeps the stream open"
    /// bug would hang here forever.
    #[test]
    fn stream_closes_on_eof_even_with_local_echo() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            // A duplex pair: writing into `device` feeds the session's
            // read half; dropping `device` gives the read half EOF.
            let (session_end, mut device) = tokio::io::duplex(256);
            let params = SerialParams {
                local_echo: true,
                ..SerialParams::default()
            };
            let (session, mut output) = SerialSession::run(session_end, params);

            device.write_all(b"hello").await.unwrap();
            device.flush().await.unwrap();
            // Close the device end: the read half now EOFs.
            drop(device);

            // Collect until the stream closes. Bounded so a regression
            // (stream never closes) fails the test instead of hanging CI.
            let mut seen = Vec::new();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match tokio::time::timeout_at(deadline, output.recv()).await {
                    Ok(Some(chunk)) => seen.extend_from_slice(&chunk),
                    Ok(None) => break, // stream closed: the invariant holds
                    Err(_) => panic!("output stream never closed on EOF (dead-sink regression)"),
                }
            }
            assert_eq!(seen, b"hello".to_vec());
            // The session is a live handle until dropped/closed.
            session.close();
        });
    }

    /// After an unplug the session must read as DEAD BY THE TIME the
    /// output stream ends, not merely soon after.
    ///
    /// The app takes the end of this stream as the disconnect notice
    /// (`SshDisconnected`) and asks `is_alive()` before acting on it,
    /// discarding a notice whose pane still holds a live transport as
    /// one from a session the pane already replaced. So a session that
    /// is still reporting alive at that instant gets its own disconnect
    /// thrown away, and the tab reads connected over a dead port until
    /// the 30 s liveness sweep happens to catch it (longer still while
    /// the vault is soft-locked, which unmounts that sweep).
    ///
    /// What this test is and is not: it PASSES on the old code too,
    /// because the death cascade that closes the input channels wins its
    /// scheduling here every time. So this is a contract guard, not a
    /// race reproduction, and it is deliberately written without a sleep
    /// or a retry loop (which is what it replaced) so it keeps asserting
    /// "already settled" rather than "settles soon". The property is
    /// made true by construction instead: the reader stores
    /// `reader_done` before dropping the output sender, in the same task
    /// with no await between.
    #[test]
    fn is_alive_is_false_by_the_time_the_stream_ends() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (session_end, device) = tokio::io::duplex(256);
            let (session, mut output) = SerialSession::run(session_end, SerialParams::default());
            assert!(session.is_alive());

            // Unplug: the read half EOFs, only the reader task notices
            // directly.
            drop(device);
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match tokio::time::timeout_at(deadline, output.recv()).await {
                    Ok(Some(_)) => continue,
                    Ok(None) => break,
                    Err(_) => panic!("output stream never closed on unplug"),
                }
            }
            assert!(
                !session.is_alive(),
                "the stream ended while the session still read as alive: \
                 the app would discard this disconnect as stale",
            );
        });
    }

    /// Protocol writes through `write_sender` must reach the port
    /// byte-exact with NO local echo and NO line-ending mapping, while
    /// user writes keep both. A regression here echoes ZMODEM replies
    /// back into the transfer's own divert and corrupts it.
    #[test]
    fn wire_writes_bypass_echo_and_line_ending_mapping() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (session_end, mut device) = tokio::io::duplex(1024);
            let params = SerialParams {
                local_echo: true,
                line_ending: SerialLineEnding::Lf,
                ..SerialParams::default()
            };
            let (session, mut output) = SerialSession::run(session_end, params);
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);

            // A protocol frame with a CR that the Lf ending would eat.
            let frame = vec![0x2a, 0x18, b'\r', 0x00, b'A'];
            session.write_sender().send(frame.clone()).unwrap();
            let mut buf = vec![0u8; 64];
            let n = tokio::time::timeout_at(deadline, device.read(&mut buf))
                .await
                .expect("device never saw the wire frame")
                .unwrap();
            assert_eq!(&buf[..n], &frame[..], "wire frame was re-encoded");

            // A user keystroke line: Enter mapped to LF, echoed locally.
            session.write(b"ls\r").unwrap();
            let n = tokio::time::timeout_at(deadline, device.read(&mut buf))
                .await
                .expect("device never saw the user input")
                .unwrap();
            assert_eq!(&buf[..n], b"ls\n");
            // The FIRST echo on the output stream is the user input; the
            // wire frame (sent earlier) must not have been echoed at all.
            let echoed = tokio::time::timeout_at(deadline, output.recv())
                .await
                .expect("echo never arrived")
                .unwrap();
            assert_eq!(echoed, b"ls\n".to_vec(), "wire frame leaked into the echo");

            session.close();
        });
    }

    #[test]
    fn opening_a_missing_port_is_an_error_not_a_panic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let result = SerialSession::open(SerialConfig {
                path: "/dev/oryxis-does-not-exist".into(),
                params: SerialParams::default(),
            });
            assert!(matches!(result, Err(SerialError::Open { .. })));
        });
    }
}
