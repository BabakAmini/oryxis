//! Live Telnet session, mirroring `oryxis_ssh::SshSession`'s surface
//! (`write` / `resize` / `is_alive` / `close` plus an unbounded output
//! receiver) so the terminal pane can hold either transport behind one
//! enum without caring which protocol is underneath.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::autologin::AutoLogin;
use crate::negotiation::{Negotiator, encode_input};

#[derive(Debug, thiserror::Error)]
pub enum TelnetError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Connection timed out")]
    Timeout,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Everything needed to dial a Telnet host. Credentials are optional:
/// when present they feed NEW-ENVIRON (`USER`) and the prompt-driven
/// autofill; when absent the user just types at the server's prompts.
#[derive(Debug, Clone)]
pub struct TelnetConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Terminal type answered to RFC 1091 TERMINAL-TYPE SEND.
    pub term: String,
    /// Per-host charset label (e.g. `"Big5"`). `None` / UTF-8 forwards
    /// the byte stream untouched; anything else transcodes both ways,
    /// mirroring the SSH engine's PTY transcoding.
    pub encoding: Option<String>,
    pub connect_timeout: Duration,
}

impl Default for TelnetConfig {
    fn default() -> Self {
        TelnetConfig {
            host: String::new(),
            port: 23,
            username: None,
            password: None,
            term: "xterm-256color".to_string(),
            encoding: None,
            connect_timeout: Duration::from_secs(15),
        }
    }
}

/// How long after connect the credential autofill stays armed. Login
/// banners on slow gear can take a while; past this, a `password:`
/// string in ordinary output can never trigger an injection.
const AUTOLOGIN_WINDOW: Duration = Duration::from_secs(60);

/// A live Telnet session over raw TCP.
pub struct TelnetSession {
    /// Application-level input (terminal keystrokes / in-band replies).
    /// The writer task applies charset + NVT encoding before the wire.
    writer_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Viewport changes, reported to the server via NAWS when active.
    resize_tx: mpsc::UnboundedSender<(u16, u16)>,
    reader_task: tokio::task::JoinHandle<()>,
    writer_task: tokio::task::JoinHandle<()>,
    /// Latched by `close()` so teardown runs exactly once even when
    /// both an explicit close and the `Drop` backstop fire.
    closed: AtomicBool,
}

impl std::fmt::Debug for TelnetSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelnetSession")
            .field("alive", &self.is_alive())
            .finish()
    }
}

impl TelnetSession {
    /// Dial the host and run option negotiation. Returns the session
    /// plus the decoded output stream; the receiver ends (`None`) when
    /// the server closes the connection.
    pub async fn connect(
        config: TelnetConfig,
    ) -> Result<(TelnetSession, mpsc::UnboundedReceiver<Vec<u8>>), TelnetError> {
        let addr = format!("{}:{}", config.host, config.port);
        let stream = tokio::time::timeout(config.connect_timeout, TcpStream::connect(&addr))
            .await
            .map_err(|_| TelnetError::Timeout)?
            .map_err(|e| TelnetError::ConnectionFailed(format!("{addr}: {e}")))?;
        // Interactive session: keystroke latency beats segment
        // coalescing (PuTTY defaults TCP_NODELAY on for the same
        // reason). Best-effort, some stacks refuse it.
        let _ = stream.set_nodelay(true);

        let (negotiator, greeting) =
            Negotiator::new(&config.term, config.username.as_deref());
        let negotiator = Arc::new(StdMutex::new(negotiator));

        // Resolve the per-host charset once. `None` (or UTF-8) means
        // passthrough both ways.
        let enc: Option<&'static encoding_rs::Encoding> = config
            .encoding
            .as_deref()
            .and_then(|n| encoding_rs::Encoding::for_label(n.as_bytes()))
            .filter(|e| *e != encoding_rs::UTF_8);

        let (output_tx, output_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (resize_tx, mut resize_rx) = mpsc::unbounded_channel::<(u16, u16)>();
        // Raw protocol bytes (negotiation replies), written verbatim.
        let (wire_tx, mut wire_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        let _ = wire_tx.send(greeting);

        let (mut read_half, mut write_half) = stream.into_split();

        // Reader task: socket -> negotiator -> decoded output stream.
        // Also hosts the credential autofill, which watches the decoded
        // output and types back through the ordinary input path.
        let reader_neg = Arc::clone(&negotiator);
        let autologin_tx = writer_tx.clone();
        let mut autologin = AutoLogin::new(
            config.username.clone(),
            config.password.clone(),
            AUTOLOGIN_WINDOW,
        );
        let reader_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            // Stateful decoder so a multi-byte char split across two
            // reads still decodes correctly. `None` for UTF-8.
            let mut decoder = enc.map(|e| e.new_decoder());
            loop {
                let n = match read_half.read(&mut buf).await {
                    Ok(0) => {
                        tracing::info!("Telnet connection closed by server");
                        break;
                    }
                    Ok(n) => n,
                    Err(e) => {
                        tracing::info!("Telnet read error: {}", e);
                        break;
                    }
                };
                let step = {
                    let mut neg = reader_neg.lock().expect("negotiator lock");
                    neg.receive(&buf[..n])
                };
                if !step.wire.is_empty() && wire_tx.send(step.wire).is_err() {
                    break;
                }
                if step.app.is_empty() {
                    continue;
                }
                let out = match &mut decoder {
                    Some(dec) => {
                        let mut s = String::with_capacity(step.app.len() + 16);
                        let _ = dec.decode_to_string(&step.app, &mut s, false);
                        s.into_bytes()
                    }
                    None => step.app,
                };
                if !autologin.exhausted()
                    && let Some(line) = autologin.observe(&out)
                {
                    // Through the normal input path so charset + NVT
                    // encoding apply, exactly as if the user typed it.
                    let _ = autologin_tx.send(line);
                }
                if output_tx.send(out).is_err() {
                    break;
                }
            }
            // Dropping output_tx here ends the app-side stream cleanly
            // (recv returns None) instead of hanging on a dead session.
        });

        // Writer task: input encoding + negotiation replies + NAWS.
        // Owning all three sources in one select keeps the socket's
        // write half in a single place, like the SSH engine's writer.
        let writer_neg = Arc::clone(&negotiator);
        let writer_task = tokio::spawn(async move {
            loop {
                let bytes: Option<Vec<u8>> = tokio::select! {
                    raw = wire_rx.recv() => match raw {
                        Some(b) => Some(b),
                        None => break, // reader gone
                    },
                    data = writer_rx.recv() => match data {
                        Some(data) => {
                            // Terminal input arrives as UTF-8; encode to
                            // the host charset first, then apply the NVT
                            // rules (IAC doubling, CR -> CR LF) to the
                            // charset bytes, in that order, a legacy
                            // multi-byte sequence may contain 0xFF.
                            let data = match enc {
                                Some(e) => {
                                    let text = String::from_utf8_lossy(&data);
                                    let (encoded, _, _) = e.encode(&text);
                                    encoded.into_owned()
                                }
                                None => data,
                            };
                            Some(encode_input(&data))
                        }
                        None => break, // session closed
                    },
                    size = resize_rx.recv() => match size {
                        Some((cols, rows)) => {
                            let mut neg = writer_neg.lock().expect("negotiator lock");
                            // None while NAWS is off; the size is still
                            // recorded for the enable-time report.
                            neg.set_window(cols, rows)
                        }
                        None => None,
                    },
                };
                let Some(bytes) = bytes else { continue };
                if let Err(e) = write_half.write_all(&bytes).await {
                    tracing::error!("Telnet write error: {}", e);
                    break;
                }
                if let Err(e) = write_half.flush().await {
                    tracing::error!("Telnet flush error: {}", e);
                    break;
                }
            }
        });

        Ok((
            TelnetSession {
                writer_tx,
                resize_tx,
                reader_task,
                writer_task,
                closed: AtomicBool::new(false),
            },
            output_rx,
        ))
    }

    pub fn write(&self, data: &[u8]) -> Result<(), TelnetError> {
        self.writer_tx
            .send(data.to_vec())
            .map_err(|e| TelnetError::ConnectionFailed(format!("write failed: {}", e)))
    }

    /// Notify the server that the local viewport changed shape (NAWS).
    /// Errors are swallowed, resize fires often and a dropped one is
    /// cosmetically ugly but never fatal.
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.resize_tx.send((cols, rows));
    }

    /// Clone of the resize sender so the terminal state can forward
    /// viewport changes directly without round-tripping a message.
    pub fn resize_sender(&self) -> mpsc::UnboundedSender<(u16, u16)> {
        self.resize_tx.clone()
    }

    /// Clone of the input sender so the terminal emulator can answer
    /// in-band queries (cursor position report, device attributes, ...)
    /// directly, same contract as the SSH session's back-channel.
    pub fn write_sender(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.writer_tx.clone()
    }

    pub fn is_alive(&self) -> bool {
        !self.writer_tx.is_closed()
    }

    /// Tear the session down. Idempotent: only the first call acts.
    /// Aborting both tasks drops the socket halves, which closes the
    /// TCP connection, and drops the output sender, which ends the
    /// app-side stream cleanly.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

impl Drop for TelnetSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    const IAC: u8 = 255;
    const DO: u8 = 253;
    const SB: u8 = 250;
    const SE: u8 = 240;

    /// End-to-end against an in-process fake telnetd: negotiation
    /// replies arrive, the login prompt is auto-answered, data flows
    /// decoded, and server close ends the output stream.
    #[tokio::test]
    async fn session_negotiates_logs_in_and_streams() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Ask for the terminal type and accept NAWS.
            sock.write_all(&[IAC, DO, 24, IAC, DO, 31]).await.unwrap();
            sock.write_all(&[IAC, SB, 24, 1, IAC, SE]).await.unwrap();
            sock.write_all(b"router login: ").await.unwrap();

            // Collect everything the client sends until the login line
            // arrives (negotiation replies + "admin\r\n").
            let mut got: Vec<u8> = Vec::new();
            let mut buf = [0u8; 512];
            while !got.windows(7).any(|w| w == b"admin\r\n") {
                let n = sock.read(&mut buf).await.unwrap();
                assert!(n > 0, "client hung up early");
                got.extend_from_slice(&buf[..n]);
            }
            // TERMINAL-TYPE IS answer must carry the configured term.
            let mut ttype_is = vec![IAC, SB, 24, 0];
            ttype_is.extend_from_slice(b"xterm-256color");
            ttype_is.extend_from_slice(&[IAC, SE]);
            assert!(
                got.windows(ttype_is.len()).any(|w| w == ttype_is),
                "no TERMINAL-TYPE IS reply in {got:?}"
            );
            // NAWS report for the DO we sent.
            assert!(
                got.windows(3).any(|w| w == [IAC, SB, 31]),
                "no NAWS report in {got:?}"
            );

            // Greet with data containing an escaped IAC, then hang up.
            sock.write_all(b"welcome ").await.unwrap();
            sock.write_all(&[IAC, IAC]).await.unwrap();
            sock.write_all(b" done").await.unwrap();
        });

        let (session, mut output) = TelnetSession::connect(TelnetConfig {
            host: addr.ip().to_string(),
            port: addr.port(),
            username: Some("admin".into()),
            password: None,
            ..TelnetConfig::default()
        })
        .await
        .unwrap();

        // Drain output until the post-login greeting shows up decoded.
        let mut seen: Vec<u8> = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let chunk = tokio::time::timeout_at(deadline, output.recv())
                .await
                .expect("timed out waiting for output");
            match chunk {
                Some(b) => seen.extend_from_slice(&b),
                None => break, // server closed after sending everything
            }
            if seen.windows(6).any(|w| w == [b' ', IAC, b' ', b'd', b'o', b'n']) {
                break;
            }
        }
        let expected: &[u8] = &[
            b'w', b'e', b'l', b'c', b'o', b'm', b'e', b' ', IAC, b' ', b'd', b'o', b'n', b'e',
        ];
        assert!(
            seen.windows(expected.len()).any(|w| w == expected),
            "decoded output missing greeting: {seen:?}"
        );

        server.await.unwrap();
        session.close();
        // After the server hangs up and close() aborts the writer, the
        // session must report dead (channel closed).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while session.is_alive() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "session still alive after close"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Connecting to a dead port surfaces a clean error, not a hang.
    #[tokio::test]
    async fn connect_refused_is_an_error() {
        // Bind-then-drop guarantees an unused port.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let result = TelnetSession::connect(TelnetConfig {
            host: addr.ip().to_string(),
            port: addr.port(),
            connect_timeout: Duration::from_secs(5),
            ..TelnetConfig::default()
        })
        .await;
        assert!(matches!(
            result,
            Err(TelnetError::ConnectionFailed(_)) | Err(TelnetError::Timeout)
        ));
    }
}
