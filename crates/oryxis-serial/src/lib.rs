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
//! - **line ending**: the terminal sends a bare `\r` for Enter; the
//!   configured [`SerialLineEnding`](oryxis_core::models::serial::SerialLineEnding)
//!   maps it to CR / LF / CR LF on the wire.
//! - **local echo**: when enabled, written bytes are echoed back into
//!   the output stream so a non-echoing device still shows typing.

use std::sync::atomic::{AtomicBool, Ordering};

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
    reader_task: tokio::task::JoinHandle<()>,
    writer_task: tokio::task::JoinHandle<()>,
    closed: AtomicBool,
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

/// Map terminal input bytes onto the wire: the terminal emits a bare
/// `\r` for Enter, which becomes the configured line ending. Any `\r`
/// already paired with `\n` collapses so a CR LF from the terminal
/// stays a single ending rather than doubling.
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
        // Local-echo side channel: writer -> reader. Kept alive as long
        // as the writer lives; when it dies the reader's `recv()` yields
        // `None` and that select branch simply disables itself.
        let (echo_tx, mut echo_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        // Reader task: port + echo -> output stream. Raw passthrough (no
        // sniffing, no transcode): the terminal emulator owns decoding.
        let reader_task = tokio::spawn(async move {
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
            // Dropping `output_tx` here ends the app-side stream cleanly
            // (recv -> None), so the disconnect propagates on unplug.
        });

        // Writer task: terminal input -> line-ending map -> optional
        // local echo (via the reader) -> port.
        let local_echo = params.local_echo;
        let line_ending = params.line_ending;
        let writer_task = tokio::spawn(async move {
            while let Some(data) = writer_rx.recv().await {
                let encoded = encode_input(&data, line_ending);
                if local_echo {
                    // Echo the exact wire form (Enter as the configured
                    // ending) through the reader so it shares the one
                    // `output_tx` owner. A send error means the reader
                    // (and thus the port) is gone; the write below then
                    // fails too and ends the task.
                    let _ = echo_tx.send(encoded.clone());
                }
                if let Err(e) = write_half.write_all(&encoded).await {
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
                reader_task,
                writer_task,
                closed: AtomicBool::new(false),
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

    /// Hand out a clone of the input sender so the terminal emulator can
    /// answer in-band queries directly, same contract as the SSH/Telnet
    /// sessions' back-channel.
    pub fn write_sender(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.writer_tx.clone()
    }

    pub fn is_alive(&self) -> bool {
        !self.writer_tx.is_closed()
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
