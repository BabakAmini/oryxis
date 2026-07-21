use super::*;

// ---------------------------------------------------------------------------
// SSH Session
// ---------------------------------------------------------------------------

/// Result of a non-interactive command execution.
pub struct ExecResult {
    pub exit_code: u32,
    pub stdout: String,
    pub stderr: String,
}

/// A live SSH session with a remote PTY channel.
pub struct SshSession {
    /// Shared SSH handle, kept alive for port forward tasks to open channels.
    pub(crate) _handle: Arc<tokio::sync::Mutex<client::Handle<ClientHandler>>>,
    pub(crate) writer_tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Forwarded to the SSH channel as `window-change` requests so the
    /// remote shell sees SIGWINCH and re-renders for the new viewport.
    /// Without this, apps like `top` keep rendering for the original
    /// columns and our local alacritty wraps the overflow into extra
    /// rows ("double line" effect).
    pub(crate) resize_tx: mpsc::UnboundedSender<(u16, u16)>,
    pub(crate) reader_task: tokio::task::JoinHandle<()>,
    pub(crate) writer_task: tokio::task::JoinHandle<()>,
    pub(crate) port_forward_tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Rolling link-quality figures (RTT / jitter / stalls), fed by
    /// `quality_task` and surfaced in the terminal performance HUD.
    pub(crate) net_quality: Arc<NetQuality>,
    /// The RTT prober behind `net_quality`; aborted on close so a
    /// closed session stops pinging.
    pub(crate) quality_task: tokio::task::JoinHandle<()>,
    /// Latched by `close()` so teardown runs exactly once even when both
    /// an explicit close and the `Drop` backstop fire.
    pub(crate) closed: std::sync::atomic::AtomicBool,
    /// Cap on how long `open_sftp` (and the per-sibling open in the
    /// transfer pool) wait before giving up. Set by `SshEngine`'s
    /// builder so the user can tune it from the SFTP settings panel.
    pub(crate) sftp_open_timeout: std::time::Duration,
}

impl std::fmt::Debug for SshSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSession")
            .field("alive", &self.is_alive())
            .finish()
    }
}

impl SshSession {
    pub fn write(&self, data: &[u8]) -> Result<(), SshError> {
        self.writer_tx
            .send(data.to_vec())
            .map_err(|e| SshError::Channel(format!("write failed: {}", e)))
    }

    /// Notify the remote shell that the local viewport changed shape.
    /// Errors are swallowed because resize requests fire often and a
    /// dropped one is cosmetically ugly but never fatal.
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.resize_tx.send((cols, rows));
    }

    /// Hand out a clone of the resize sender so the terminal state can
    /// forward viewport changes directly without round-tripping a message.
    pub fn resize_sender(&self) -> mpsc::UnboundedSender<(u16, u16)> {
        self.resize_tx.clone()
    }

    /// Hand out a clone of the input sender so the terminal emulator can
    /// answer in-band queries (cursor position report, device attributes,
    /// DECRQM, ...) directly on the channel. Remote programs block waiting
    /// for these replies; without the back-channel they hang with the tty
    /// in raw mode, which looks like a full terminal freeze.
    pub fn write_sender(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.writer_tx.clone()
    }

    /// Open a fresh SFTP subsystem channel on this session, the SSH
    /// connection multiplexes, so the original PTY channel keeps running.
    /// Wrapped in the engine-configured timeout to keep `open_sftp` from
    /// hanging the UI when a server doesn't speak the sftp subsystem.
    pub async fn open_sftp(&self) -> Result<crate::sftp::SftpClient, SshError> {
        let timeout = self.sftp_open_timeout;
        let handle_for_exec = self._handle.clone();
        let inner = async {
            let handle = self._handle.lock().await;
            let channel = handle
                .channel_open_session()
                .await
                .map_err(|e| SshError::Channel(format!("sftp channel open: {e}")))?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|e| SshError::Channel(format!("sftp subsystem: {e}")))?;
            let session = russh_sftp::client::SftpSession::new(channel.into_stream())
                .await
                .map_err(|e| SshError::Channel(format!("sftp init: {e}")))?;
            Ok::<_, SshError>(session)
        };
        let session = tokio::time::timeout(timeout, inner)
            .await
            .map_err(|_| {
                SshError::Channel(format!(
                    "sftp open timed out after {}s",
                    timeout.as_secs()
                ))
            })??;
        Ok(crate::sftp::SftpClient::new(session, handle_for_exec, timeout))
    }

    /// Run a short, silent command on a side channel of this live session
    /// and return its stdout. Same shape as `detect_os` (which predates
    /// it), generalized so callers can supply the command: the host
    /// monitor batches its whole `/proc` read into one `sh -c` per tick,
    /// keeping the cost at a single channel round trip.
    ///
    /// Nothing reaches the user's PTY, and the shared handle lock is
    /// released as soon as the channel is open so other tasks (SFTP,
    /// forwards) aren't blocked while the command runs. Returns `None` on
    /// any channel failure or if the command outlives `timeout`.
    pub async fn probe(
        &self,
        command: &str,
        timeout: std::time::Duration,
    ) -> Option<String> {
        let handle = self._handle.lock().await;
        let mut channel = handle.channel_open_session().await.ok()?;
        channel.exec(true, command).await.ok()?;
        drop(handle); // release so other tasks can use the shared handle

        // Hard cap on collected output: probe payloads are a few KB, and
        // the host side is untrusted, so an unbounded collect would let a
        // hostile (or misconfigured) command stream hundreds of MB into
        // memory within the timeout window. Generous headroom for a busy
        // host's df/socket tables; excess is dropped, not an error.
        const PROBE_STDOUT_CAP: usize = 512 * 1024;
        let mut stdout = Vec::new();
        let collect = async {
            loop {
                match channel.wait().await {
                    // Once the cap is hit the guard stops matching and
                    // excess data falls through to `_` (drained, dropped).
                    Some(russh::ChannelMsg::Data { data })
                        if stdout.len() < PROBE_STDOUT_CAP =>
                    {
                        let room = PROBE_STDOUT_CAP - stdout.len();
                        stdout.extend_from_slice(&data[..data.len().min(room)]);
                    }
                    Some(russh::ChannelMsg::Eof)
                    | Some(russh::ChannelMsg::ExitStatus { .. })
                    | None => break,
                    _ => {}
                }
            }
        };
        tokio::time::timeout(timeout, collect).await.ok()?;
        Some(String::from_utf8_lossy(&stdout).into_owned())
    }

    pub fn is_alive(&self) -> bool {
        // Three death signals, any one of which means the session is
        // unusable: an explicit `close()` (latch, the task aborts it
        // triggers land asynchronously), the reader task having exited
        // (EOF / exit-status / transport drop; the writer task alone
        // can't notice, it blocks on its queue forever when nothing
        // writes), and the writer channel being gone.
        !self.closed.load(std::sync::atomic::Ordering::SeqCst)
            && !self.reader_task.is_finished()
            && !self.writer_tx.is_closed()
    }

    /// Point-in-time link-quality figures for this session (RTT probe
    /// window). See [`NetQualitySnapshot`].
    pub fn net_quality(&self) -> NetQualitySnapshot {
        self.net_quality.snapshot()
    }

    /// Tear the session down. Idempotent: only the first call acts.
    ///
    /// Aborts the reader / writer / port-forward tasks (releasing any
    /// locally bound `-L` listeners) and disconnects the underlying SSH
    /// connection so the remote side tears its half down too. Aborting
    /// the reader task drops the output channel sender, so the app-side
    /// output stream ends cleanly (recv returns `None`) instead of
    /// hanging on a dead session.
    pub fn close(&self) {
        use std::sync::atomic::Ordering;
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.reader_task.abort();
        self.writer_task.abort();
        self.quality_task.abort();
        for task in &self.port_forward_tasks {
            task.abort();
        }
        // Politely disconnect the transport. Needs a runtime to spawn
        // on; when close() runs outside one (e.g. a late Drop during
        // process shutdown) the aborts above already killed the tasks
        // and the TCP socket dies with the last handle clone.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let handle = Arc::clone(&self._handle);
            rt.spawn(async move {
                let h = handle.lock().await;
                let _ = h
                    .disconnect(russh::Disconnect::ByApplication, "session closed", "")
                    .await;
            });
        }
    }

    /// Detect the remote OS by executing a silent probe on a side channel
    /// (no output goes to the user's PTY). Parses `/etc/os-release` for
    /// Linux; falls back to `uname -s` for non-Linux (Darwin, FreeBSD…).
    ///
    /// Returns `Some("ubuntu" | "debian" | "alpine" | "rhel" | "fedora" |
    /// "arch" | "amzn" | "centos" | "rocky" | "alma" | "darwin" | "freebsd"
    /// | "openbsd" | "netbsd")` or `None` on any parse / channel failure.
    pub async fn detect_os(&self) -> Option<String> {
        let cmd = "cat /etc/os-release 2>/dev/null; echo '---OXYXIS-SEP---'; uname -s";
        let handle = self._handle.lock().await;
        let mut channel = handle.channel_open_session().await.ok()?;
        channel.exec(true, cmd).await.ok()?;
        drop(handle); // release so other tasks can use the shared handle

        let mut stdout = Vec::new();
        let collect = async {
            loop {
                match channel.wait().await {
                    Some(russh::ChannelMsg::Data { data }) => stdout.extend_from_slice(&data),
                    Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::ExitStatus { .. }) | None => break,
                    _ => {}
                }
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(6), collect).await.is_err() {
            return None;
        }

        let text = String::from_utf8_lossy(&stdout);
        let mut parts = text.split("---OXYXIS-SEP---");
        let os_release = parts.next().unwrap_or("");
        let uname_s = parts.next().unwrap_or("").trim();

        // Try /etc/os-release first: `ID=ubuntu` (may be quoted).
        for line in os_release.lines() {
            if let Some(rest) = line.strip_prefix("ID=") {
                let id = rest.trim().trim_matches('"').trim_matches('\'').to_lowercase();
                if !id.is_empty() { return Some(id); }
            }
        }
        // Fallback: uname -s → darwin / freebsd / openbsd / netbsd / linux.
        let u = uname_s.to_lowercase();
        if !u.is_empty() && u != "linux" { return Some(u); }
        None
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        // Backstop: an SshSession dropped without an explicit close()
        // must not leak its tokio tasks, the live SSH connection, or
        // any bound port-forward listeners.
        self.close();
    }
}
