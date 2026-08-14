//! `SyncRuntime`: owns the live `SyncEngine` while P2P sync is enabled.
//!
//! The engine needs an `Arc<Mutex<VaultStore>>`, but `Oryxis` holds its
//! vault as a plain `Option<VaultStore>` (one owner, ~160 call sites).
//! Rather than refactor every one, the runtime opens its OWN
//! `VaultStore` handle on the same database file. SQLite WAL mode makes
//! concurrent handles safe; the `busy_timeout` set in `VaultStore::open`
//! covers the rare two-writer overlap.
//!
//! Known v1 limitation: rotating the vault master password leaves this
//! handle's derived key stale, so the engine has to be restarted
//! (toggle sync off then on). The persisted `DeviceIdentity` blob
//! itself survives rotation via `re_encrypt_sync_device_identity`.

use std::sync::{Arc, Mutex};

use iced::Task;
use tokio::sync::mpsc;

use oryxis_sync::crypto::DeviceIdentity;
use oryxis_sync::{SyncConfig, SyncEngine, SyncError, SyncEvent, SyncHandle, SyncMode};
use oryxis_vault::VaultStore;

use crate::app::{SyncMessage, Message, Oryxis};

/// Live sync engine owned by `Oryxis` while sync is enabled.
pub(crate) struct SyncRuntime {
    engine: SyncEngine,
    handle: SyncHandle,
}

/// Result of an off-thread engine spawn: the runtime plus its event
/// stream, or the failure. Neither the runtime nor the receiver is
/// `Clone`, so this crosses the task boundary through a oneshot rather
/// than riding a message payload.
pub(crate) type EngineSpawnResult =
    Result<(SyncRuntime, mpsc::UnboundedReceiver<SyncEvent>), SyncError>;

impl SyncRuntime {
    /// Open a dedicated vault handle, build the engine, start its
    /// background tasks, and hand back the event receiver so the caller
    /// can pump it into a `Task::stream`.
    ///
    /// `master_password` is `Some` when the vault has a user password
    /// (we unlock the second handle with it) and `None` when the vault
    /// auto-opens without one (mirrors the boot path in `boot.rs`).
    ///
    /// Must be awaited on the Tokio runtime (`start()` calls
    /// `tokio::spawn`). The expensive half — SQLite open, the Argon2id
    /// unlock (deliberately tuned to ~1s) and the device identity — runs
    /// on a blocking thread: this used to run synchronously on the UI
    /// thread, freezing the app for the whole derivation every time the
    /// engine started (transport switch, boot, vault unlock).
    pub(crate) async fn spawn(
        config: SyncConfig,
        device_name: String,
        db_path: std::path::PathBuf,
        master_password: Option<String>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SyncEvent>), SyncError> {
        // Dedicated handle on the same SQLite file as the app's vault.
        let (vault, identity) = tokio::task::spawn_blocking(move || {
            let mut vault = VaultStore::open(&db_path)
                .map_err(|e| SyncError::Vault(format!("open sync vault handle: {e}")))?;
            match master_password.as_deref() {
                Some(pw) => vault
                    .unlock(pw)
                    .map_err(|e| SyncError::Vault(format!("unlock sync vault handle: {e}")))?,
                None => vault
                    .open_without_password()
                    .map_err(|e| SyncError::Vault(format!("open sync vault handle: {e}")))?,
            }
            // Persistent device identity, generated + stored on first run.
            let identity = DeviceIdentity::load_or_generate(&vault, &device_name)?;
            Ok::<_, SyncError>((Arc::new(Mutex::new(vault)), identity))
        })
        .await
        .map_err(|e| SyncError::Vault(format!("join: {e}")))??;

        let mut engine = SyncEngine::new(config, identity, vault);
        let event_rx = engine
            .take_events()
            .expect("a freshly created engine always has its event receiver");
        if let Err(e) = engine.start() {
            // A partial `start()` (QUIC up, mDNS not) leaves spawned
            // tasks running; stop them before reporting the failure so a
            // quick toggle off/on can't stack engines.
            engine.stop();
            return Err(e);
        }
        let handle = engine.handle();

        Ok((Self { engine, handle }, event_rx))
    }

    /// A cloneable handle for triggering a manual sync off-thread.
    pub(crate) fn handle(&self) -> SyncHandle {
        self.handle.clone()
    }

    /// Stop all background tasks. Idempotent.
    pub(crate) fn stop(&mut self) {
        self.engine.stop();
    }
}

impl Drop for SyncRuntime {
    fn drop(&mut self) {
        // Belt-and-braces: an explicit `stop_sync_engine` is the normal
        // path, but a dropped runtime must never leave the QUIC socket
        // and background tasks dangling.
        self.engine.stop();
    }
}

impl Oryxis {
    /// Build a `SyncConfig` from the current in-memory sync settings.
    fn build_sync_config(&self) -> SyncConfig {
        let mut config = SyncConfig {
            enabled: true,
            mode: if self.sync.mode == "auto" {
                SyncMode::Auto
            } else {
                SyncMode::Manual
            },
            relay_url: if self.sync.relay_url.trim().is_empty() {
                None
            } else {
                Some(self.sync.relay_url.clone())
            },
            listen_port: self.sync.listen_port.trim().parse().unwrap_or(0),
            auto_interval_secs: 300,
            ..SyncConfig::default()
        };
        // A signaling URL typed in Settings overrides the build-time
        // default; an explicit empty string switches the engine back
        // to LAN-only (`None`). The token follows the same rule and
        // is `None` (so the signaling client sends no `Authorization`)
        // when the field is empty.
        let url = self.sync.signaling_url.trim();
        config.signaling_url = if url.is_empty() {
            None
        } else {
            Some(url.to_string())
        };
        let token = self.sync.signaling_token.trim();
        config.signaling_token = if token.is_empty() {
            None
        } else {
            Some(token.to_string())
        };
        config
    }

    /// Whether the selected transport is the one with a background
    /// engine. Every snapshot transport (SFTP, folder, Git, WebDAV)
    /// reconciles on demand and must leave QUIC and mDNS down.
    ///
    /// An ALLOWLIST on purpose. The four callers used to ask
    /// `transport != "sftp"`, written when SFTP was the only other
    /// option, and each new transport silently inherited a live P2P
    /// engine: the user picked a folder and the app was still
    /// advertising itself on the LAN and syncing over QUIC behind the
    /// file it thought it was writing. Phrased this way, a transport
    /// nobody taught this function about runs no engine, which is the
    /// safe answer, including for a settings row written by a newer
    /// build.
    pub(crate) fn sync_uses_p2p(&self) -> bool {
        self.sync.transport == "p2p"
    }

    /// Whether a snapshot round may run right now: only with the vault
    /// unlocked. The automated tick already checks this, but the manual
    /// "Sync now" buttons relied on the lock screen simply not rendering
    /// their view. That holds today, yet it makes a security invariant
    /// depend on view gating rather than stating it locally; each
    /// `run_*_sync_round` calls this so the guard travels with the code
    /// that needs the master key. A soft auto-lock keeps `self.vault`
    /// set (established sessions survive it), so the `let Some(vault)`
    /// check inside those functions is NOT the gate.
    pub(crate) fn sync_round_allowed(&self) -> bool {
        self.vault_ui.state == crate::state::VaultState::Unlocked
    }

    /// Spawn the sync engine from current settings, off the UI thread.
    /// Returns a `Task` that resolves to `SyncMessage::EngineSpawned`
    /// when the spawn finishes; that handler stores the runtime and
    /// returns the `Task::stream` pumping `EngineEvent`s. No-op
    /// (`Task::none`) if the engine is already running (or a spawn is
    /// in flight) or the vault isn't available.
    pub(crate) fn start_sync_engine(&mut self) -> Task<Message> {
        if self.sync.runtime.is_some() || self.sync.pending_engine.is_some() {
            return Task::none();
        }
        let Some(vault) = &self.vault else {
            return Task::none();
        };
        let db_path = vault.db_path().to_path_buf();
        let config = self.build_sync_config();
        let device_name = if self.sync.device_name.trim().is_empty() {
            "oryxis-device".to_string()
        } else {
            self.sync.device_name.clone()
        };
        let master_password = self.master_password.clone();

        // The spawn unlocks the vault (Argon2id, ~1s) and binds QUIC /
        // mDNS; it must not run on the UI thread (it used to, and
        // selecting P2P froze the app for the whole derivation). The
        // runtime and its event receiver aren't Clone, so they come
        // back through a oneshot and a signal message; the handler
        // stores the runtime and pumps the event stream.
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sync.pending_engine = Some(rx);
        Task::perform(
            async move {
                let result =
                    SyncRuntime::spawn(config, device_name, db_path, master_password).await;
                let _ = tx.send(result);
                Message::Sync(SyncMessage::EngineSpawned)
            },
            |msg| msg,
        )
    }

    /// Stop the sync engine if it is running. Idempotent. Also drops a
    /// pending spawn: its oneshot receiver disappears, the spawn task's
    /// `send` fails, and the just-built runtime drops, which stops the
    /// engine's background tasks.
    pub(crate) fn stop_sync_engine(&mut self) {
        if let Some(mut runtime) = self.sync.runtime.take() {
            runtime.stop();
        }
        self.sync.pending_engine = None;
        self.sync.engine_running = false;
    }
}
