//! Headless, probe-only connection for the multi-host monitor
//! dashboard (issue #95).
//!
//! A [`MonitorConn`] is the authenticated transport and nothing else:
//! no PTY, no shell channel, no reader/writer tasks. The host sees a
//! login and one short-lived exec channel per poll, exactly like the
//! per-session monitor's side channels, so an idle dashboard entry
//! costs the server nothing but a TCP connection. Hosts with a live
//! terminal tab never go through here (the dashboard reuses the tab's
//! [`SshSession`](super::SshSession) instead).

use super::*;

/// A probe-only SSH connection: the shared authenticated handle plus a
/// close latch. Cheap to clone via `Arc` at the call sites.
pub struct MonitorConn {
    handle: Arc<tokio::sync::Mutex<client::Handle<ClientHandler>>>,
    /// Latched by [`close`](Self::close) so teardown runs exactly once
    /// even when both an explicit close and the `Drop` backstop fire.
    closed: std::sync::atomic::AtomicBool,
}

impl MonitorConn {
    pub(crate) fn new(handle: client::Handle<ClientHandler>) -> Self {
        Self {
            handle: Arc::new(tokio::sync::Mutex::new(handle)),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Run a short, silent command and return its stdout. Same contract
    /// (and same implementation) as [`SshSession::probe`]: bounded
    /// output, bounded time, the handle lock released as soon as the
    /// channel is open.
    pub async fn probe(
        &self,
        command: &str,
        timeout: std::time::Duration,
    ) -> Option<String> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        super::session::probe_on(&self.handle, command, timeout).await
    }

    /// Whether the transport can still carry a probe. Without reader /
    /// writer tasks the only signals are the close latch and the russh
    /// handle's own closed flag (set when the peer drops the TCP
    /// connection or sends a disconnect).
    pub fn is_alive(&self) -> bool {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        match self.handle.try_lock() {
            Ok(handle) => !handle.is_closed(),
            // A held lock means a probe is mid-open on a live handle.
            Err(_) => true,
        }
    }

    /// Tear the connection down. Idempotent: only the first call acts.
    pub fn close(&self) {
        use std::sync::atomic::Ordering;
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // Politely disconnect the transport; outside a runtime (late
        // Drop during shutdown) the TCP socket dies with the handle.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let handle = Arc::clone(&self.handle);
            rt.spawn(async move {
                let h = handle.lock().await;
                let _ = h
                    .disconnect(russh::Disconnect::ByApplication, "monitor closed", "")
                    .await;
            });
        }
    }
}

impl Drop for MonitorConn {
    fn drop(&mut self) {
        self.close();
    }
}

impl std::fmt::Debug for MonitorConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitorConn")
            .field(
                "closed",
                &self.closed.load(std::sync::atomic::Ordering::SeqCst),
            )
            .finish()
    }
}
