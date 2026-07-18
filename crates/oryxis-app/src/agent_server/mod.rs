//! Oryxis as an ssh-agent: expose the vault's keys to external tools
//! (git, WSL, VS Code Remote, rsync) over the standard ssh-agent
//! protocol, so they authenticate with vault-stored keys without a
//! private key ever touching disk, and (opt-in, Phase 4) accept keys
//! pushed in by tools like KeePassXC into an in-memory roster that is
//! never persisted (issue #54).
//!
//! Phase 1: the wire protocol ([`protocol`]), the key source
//! abstraction ([`source`]) and the unix listener ([`listener`]), all
//! provable against russh's public `AgentClient`. Phase 2 adds the
//! `AgentRuntime` (a dedicated unlocked vault handle, mirroring
//! `sync_runtime`), the Settings toggle, the per-signature confirm
//! modal and the lock wiring. Phase 3 adds the Windows named pipe
//! (`listener::windows`, a per-user DACL restricting the pipe to the
//! current user's SID). The DACL is the one part a Linux cross-check
//! cannot verify; its acceptance test is "a second local user
//! connecting to the pipe must be DENIED".
//!
//! Why not russh's `agent::server::serve`: its `Agent` trait has no
//! identity-supply hook (keys live in a private `KeyStore` filled only
//! by ADD_IDENTITY), so backing it with the vault would mean holding
//! every DECRYPTED key in russh's map for the whole unlocked window,
//! defeating the decrypt-at-sign model. We own the small frozen
//! protocol instead and use russh's `AgentClient` as the test oracle.

pub(crate) mod listener;
pub(crate) mod protocol;
pub(crate) mod source;

use std::sync::Arc;

use protocol::{ConfirmAsk, ConfirmMode};
use source::VaultKeySource;

/// The socket / pipe path to show in Settings and setup snippets, or
/// `None` on a platform without the listener (pre-Phase-3 Windows).
pub(crate) fn listener_socket_display() -> Option<String> {
    #[cfg(unix)]
    {
        listener::agent_socket_path().map(|p| p.display().to_string())
    }
    #[cfg(windows)]
    {
        listener::agent_pipe_name()
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// The live agent: a dedicated unlocked vault handle serving keys over
/// the socket while the feature is on. Mirrors `SyncRuntime`: its own
/// `VaultStore` handle on the same DB file, a background accept task,
/// and an event receiver the app pumps into the update loop. Dropping
/// it aborts the task and removes the socket.
pub(crate) struct AgentRuntime {
    source: Arc<VaultKeySource>,
    /// The accept task(s): the Oryxis socket/pipe, plus the OpenSSH
    /// alias pipe when the opt-in is on (Windows only).
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// The OpenSSH alias could not be taken (name busy, most likely
    /// the Windows agent service): the main listener still runs; this
    /// is shown inline under the alias toggle.
    pub(crate) alias_error: Option<String>,
    #[cfg(unix)]
    socket_path: Option<std::path::PathBuf>,
}

impl AgentRuntime {
    /// Open a dedicated vault handle, bind the socket and start
    /// serving; returns a receiver of per-signature prompts the app
    /// surfaces. `confirm` prompts for every signature; even with it
    /// off, keys added under a CONFIRM constraint prompt (which is why
    /// the channel always exists). `allow_add` accepts external
    /// ADD/REMOVE into the in-memory roster. `openssh_alias`
    /// additionally serves the standard OpenSSH pipe name when free
    /// (Windows; ignored elsewhere). `master_password` is `Some` for a
    /// password-protected vault, `None` for a passwordless one
    /// (mirrors the sync runtime).
    ///
    /// Returns the bind error (so the toggle can revert) on failure;
    /// an alias-bind problem is NOT fatal and lands in `alias_error`.
    pub(crate) fn spawn(
        db_path: &std::path::Path,
        master_password: Option<&str>,
        confirm: bool,
        allow_add: bool,
        openssh_alias: bool,
    ) -> Result<(Self, tokio::sync::mpsc::UnboundedReceiver<ConfirmAsk>), String> {
        let mut vault = oryxis_vault::VaultStore::open(db_path)
            .map_err(|e| format!("open agent vault handle: {e}"))?;
        match master_password {
            Some(pw) => vault
                .unlock(pw)
                .map_err(|e| format!("unlock agent vault handle: {e}"))?,
            None => vault
                .open_without_password()
                .map_err(|e| format!("open agent vault handle: {e}"))?,
        }
        let source = Arc::new(VaultKeySource::new(vault, allow_add));

        let (confirm_tx, confirm_rx) = tokio::sync::mpsc::unbounded_channel();
        let confirm_mode = ConfirmMode {
            sender: Some(confirm_tx),
            all: confirm,
        };

        #[cfg(unix)]
        {
            // The alias is a Windows concept (there is no fixed agent
            // path on unix; SSH_AUTH_SOCK points wherever the user
            // says); the setting is simply inert here.
            let _ = openssh_alias;
            let socket_path = listener::agent_socket_path();
            let src = source.clone();
            // Bind synchronously enough to surface a busy-socket error:
            // the accept loop is spawned, but a bind failure returns
            // from `serve_unix` immediately and the task ends; we probe
            // for that by attempting the bind here is not trivial in an
            // async fn, so instead the accept loop reports through the
            // task and the first real connection would fail. To keep
            // the toggle honest we do a pre-bind check below.
            if let Some(path) = &socket_path {
                pre_bind_check(path)?;
            }
            let task = tokio::spawn(async move {
                if let Err(e) = listener::serve_unix(src, confirm_mode).await {
                    tracing::warn!(target = "oryxis::agent", error = %e, "agent listener stopped");
                }
            });
            Ok((
                Self {
                    source,
                    tasks: vec![task],
                    alias_error: None,
                    socket_path,
                },
                confirm_rx,
            ))
        }
        #[cfg(windows)]
        {
            // Create the anti-squat FIRST pipe instance synchronously here,
            // BEFORE the toggle is confirmed: a squatter (or any other bind
            // failure) then reverts the toggle with a clear error instead of
            // leaving it on with a dead listener behind a `tracing::warn`.
            // This replaces the old probe-then-bind-in-task pair (one
            // authoritative mechanism, no TOCTOU). Creating the server needs
            // the same entered-runtime context the `tokio::spawn` below
            // needs, which we already have.
            let name = listener::agent_pipe_name()
                .ok_or_else(|| "no agent pipe name on this platform".to_string())?;
            let first = listener::create_first_instance(&name)
                .map_err(|e| format!("agent pipe unavailable: {e}"))?;
            let src = source.clone();
            let confirm_main = confirm_mode.clone();
            let mut tasks = vec![tokio::spawn(async move {
                if let Err(e) = listener::serve_pipe(name, first, src, confirm_main).await {
                    tracing::warn!(target = "oryxis::agent", error = %e, "agent listener stopped");
                }
            })];

            // The OpenSSH alias is best-effort: a busy name (the real
            // agent service, another agent) must not take the whole
            // feature down, so its bind failure is surfaced, not returned.
            let mut alias_error = None;
            if openssh_alias {
                let alias = listener::openssh_pipe_name();
                match listener::create_first_instance(&alias) {
                    Ok(first) => {
                        let src = source.clone();
                        tasks.push(tokio::spawn(async move {
                            if let Err(e) =
                                listener::serve_pipe(alias, first, src, confirm_mode).await
                            {
                                tracing::warn!(
                                    target = "oryxis::agent",
                                    error = %e,
                                    "openssh alias listener stopped",
                                );
                            }
                        }));
                    }
                    Err(e) => alias_error = Some(format!("openssh alias unavailable: {e}")),
                }
            }
            Ok((
                Self {
                    source,
                    tasks,
                    alias_error,
                },
                confirm_rx,
            ))
        }
        #[cfg(not(any(unix, windows)))]
        {
            // No listener transport on this platform; the toggle stays
            // hidden (listener_socket_display returns None).
            let _ = (source.clone(), confirm_mode, openssh_alias);
            Ok((
                Self {
                    source,
                    tasks: Vec::new(),
                    alias_error: None,
                },
                confirm_rx,
            ))
        }
    }

    /// Flip the source's gate + lock the dedicated handle on vault lock.
    pub(crate) fn lock(&self) {
        self.source.lock();
    }

    /// Re-unlock the dedicated handle when the app vault unlocks.
    pub(crate) fn unlock(&self, master_password: Option<&str>) {
        self.source.unlock(master_password);
    }

    /// Abort the accept task(s) and remove the socket.
    pub(crate) fn shutdown(self) {
        // Drop does the work; `self` is consumed for call-site clarity.
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        // Aborting the accept task(s) only stops NEW connections. The
        // per-connection tasks are detached and each holds a clone of
        // `source`; without this they would keep listing and signing
        // with vault keys after the feature is toggled off (or a
        // settings restart drops this runtime), invisibly if `confirm`
        // is off. Locking the source flips the shared gate every
        // sign/list/add checks and sweeps ephemeral keys, so those
        // survivors go dark immediately. Safe across a stop+start
        // restart: `spawn` always builds a fresh `source`, so the new
        // runtime is unaffected by locking this old one.
        self.source.lock();
        for task in &self.tasks {
            task.abort();
        }
        #[cfg(unix)]
        if let Some(path) = &self.socket_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Surface a busy socket before spawning the accept loop, so the
/// enable toggle can revert with a clear error instead of a silent
/// dead listener. Removes a stale (dead) socket file so the bind can
/// proceed.
#[cfg(unix)]
fn pre_bind_check(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    // A live agent already owns the path: refuse.
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return Err("another agent is already listening on the socket".to_string());
    }
    // Stale file from a crash: remove it so `serve_unix` can bind.
    std::fs::remove_file(path).map_err(|e| format!("remove stale socket: {e}"))
}
