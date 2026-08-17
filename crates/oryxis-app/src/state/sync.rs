//! Sync feature state: settings, live engine handles, and the two transient
//! sync forms (device pairing + the SFTP transport). Grouped off the `Oryxis`
//! god-struct as the deferred `SyncState` bag, part of the modules-by-feature
//! direction (field grouping only; the dispatch/view split is separate).

use tokio::sync::oneshot;

use oryxis_vault::SyncPeerRow;

use super::{
    DiscoveredPeerInfo, FolderSyncForm, GitSyncForm, SecretInput, SftpSyncForm,
    SyncPairingForm, WebdavSyncForm,
};
use crate::sync_runtime::SyncRuntime;

/// All sync settings + runtime + transient form state. Settings hydrate from
/// the `settings` table on boot; the runtime handles (`runtime`, `abort_tx`)
/// live only while sync is active. Not `Clone` (holds a oneshot sender and the
/// live engine); the manual `Default` reproduces the boot-time defaults.
pub(crate) struct SyncState {
    /// Whether sync is enabled.
    pub(crate) enabled: bool,
    /// `"manual"` or `"auto"`.
    pub(crate) mode: String,
    /// When on, sync wraps connection / identity / proxy-identity payloads
    /// with their decrypted passwords so peers can mirror them. Off by
    /// default; passwords stay device-local until the user opts in.
    pub(crate) passwords: bool,
    /// This device's display name in the peer list.
    pub(crate) device_name: String,
    /// Signaling endpoint URL (empty == not set).
    pub(crate) signaling_url: String,
    /// Bearer token for the signaling endpoint. Empty == not configured.
    pub(crate) signaling_token: String,
    /// Relay endpoint URL (empty == not set).
    pub(crate) relay_url: String,
    /// Listen port as a string (`"0"` == ephemeral).
    pub(crate) listen_port: String,
    /// Paired peers loaded from the vault.
    pub(crate) peers: Vec<SyncPeerRow>,
    /// Last status line shown in the Sync panel.
    pub(crate) status: Option<String>,
    /// Live P2P sync engine, present only while sync is enabled. Holds a
    /// dedicated vault handle plus the QUIC / mDNS background tasks.
    pub(crate) runtime: Option<SyncRuntime>,
    /// One-shot receiver for an in-flight off-thread engine spawn.
    /// `Some` only between `start_sync_engine()` firing the task and
    /// the `SyncMessage::EngineSpawned` landing. `stop_sync_engine`
    /// drops it to abandon a spawn whose engine is still coming up
    /// (the task's `send` then fails and the fresh runtime drops,
    /// stopping the engine's background tasks).
    pub(crate) pending_engine:
        Option<oneshot::Receiver<crate::sync_runtime::EngineSpawnResult>>,
    /// Mirrors `runtime.is_some()` for cheap UI checks.
    pub(crate) engine_running: bool,
    /// Transient device-pairing UI (hosted code / link, join inputs, and which
    /// pairing sub-view the Sync panel shows).
    pub(crate) pairing: SyncPairingForm,
    /// Live mDNS-discovered peers on the LAN. Deduped by `device_id`.
    pub(crate) discovered: Vec<DiscoveredPeerInfo>,
    /// `Sync Now` in flight. Drives the Cancel button + suppresses re-clicks.
    pub(crate) in_progress: bool,
    /// One-shot abort channel for the in-flight `Sync Now` task. The task
    /// races `sync_now().await` against this receiver, so `Cancel` immediately
    /// drops the QUIC connection.
    pub(crate) abort_tx: Option<oneshot::Sender<()>>,
    /// Visible heartbeat counter for signaling re-registers. Bumps on every
    /// successful `SignalingRegistered` event so the user can confirm the
    /// heartbeat is alive.
    pub(crate) signaling_tick: u32,
    /// Sync transport: `"p2p"` (QUIC + mDNS + relay, the default) or `"sftp"`
    /// (reconcile against one encrypted snapshot file on an SFTP host). A
    /// device runs one transport at a time; the two don't bridge.
    pub(crate) transport: String,
    /// Transient state for the SFTP sync transport (snapshot host, remote
    /// path, host picker) plus the in-flight round's progress + last
    /// outcome.
    pub(crate) sftp: SftpSyncForm,
    /// Transient state for the folder sync transport (path) plus the
    /// in-flight round's progress + last outcome.
    pub(crate) folder: FolderSyncForm,
    pub(crate) webdav: WebdavSyncForm,
    /// Transient state for the Git sync transport (remote URL) plus the
    /// in-flight round's progress + last outcome.
    pub(crate) git: GitSyncForm,
    /// Edit buffer for the shared snapshot-transport group passphrase
    /// (SFTP / folder / Git / WebDAV all derive one key from the same
    /// `sync_sftp_passphrase` row). NEVER pre-filled with the stored
    /// value and NEVER written through on typing: a masked pre-filled
    /// field silently APPENDS when the user types into it, and a
    /// write-through on keystrokes lets an accidental edit swap the
    /// group key under the existing snapshot ("Decryption failed (wrong
    /// key?)" on the next round). Starts empty; untouched = preserve
    /// the stored value, typed = the round's key candidate (committed
    /// to storage only when a round succeeds with it), cleared = no
    /// passphrase.
    pub(crate) passphrase_input: SecretInput,
    /// Whether a group passphrase is stored. Drives the read-only
    /// masked display (no stored value, no mask) and the edit mode's
    /// match hint. Hydrated at boot from `get_sync_sftp_passphrase`;
    /// the value itself never reaches state.
    pub(crate) passphrase_known: bool,
    /// "Change passphrase" edit mode: with a stored passphrase the field
    /// is READ-ONLY until the user asks to change it, so a mistyped
    /// re-entry can't swap the group key under the existing snapshot.
    /// While editing, the input is live and the match hint below it
    /// compares against the stored value.
    pub(crate) passphrase_editing: bool,
    /// The editable field's id while an edit is open, so the blur probe
    /// can tell "still on the field" from "focus moved away" (an empty
    /// edit abandoned by a click elsewhere returns to the read-only
    /// mask). `None` outside edit mode.
    pub(crate) passphrase_field_id: Option<&'static str>,
    /// Last drawn bounds of the editable field, so the blur probe can
    /// decide a click by position: iced does not blur a text input when
    /// a button or blank is clicked, so "did the click land on the
    /// field?" is a geometry question, not a focus question. Zeroed
    /// until the edit field renders once.
    pub(crate) passphrase_field_bounds: crate::widgets::BoundsCell,
    /// Live "does the typed value equal the stored passphrase?" state,
    /// recomputed per keystroke by `set_sync_passphrase`. Powers the
    /// green / warning hint under the field: re-entering the saved
    /// passphrase keeps the group working, typing anything else makes
    /// the existing snapshot undecryptable until it matches again.
    /// `None` while the field is untouched or no passphrase is stored.
    pub(crate) passphrase_matches: Option<bool>,
    /// The key an IN-FLIGHT round sealed its snapshot with, armed at the
    /// start of the round and spent when it finishes: success stores it
    /// as the group key, failure drops it. Committing this instead of
    /// re-reading `passphrase_input` is what makes the stored key equal
    /// the key the remote snapshot actually carries. Nothing freezes the
    /// field while a round runs (`in_progress` only disables the button)
    /// and a round costs an Argon2id derivation plus a network trip, so
    /// a user still correcting the field would otherwise leave storage
    /// holding a key the snapshot was never sealed with. `None` outside
    /// a round, and for any round keyed from storage (nothing to
    /// commit).
    pub(crate) passphrase_sealed: Option<zeroize::Zeroizing<String>>,
    /// "Set up your own relay" wizard (Settings > Sync > P2P): inputs,
    /// generated-artifact format, and the reachability test state.
    pub(crate) relay_wizard: RelayWizardForm,
    /// Last signaling outcome, kept separately from `status` (which any
    /// later event overwrites) so the panel can show a persistent
    /// signaling health line: `Ok(addr)` / `Err(reason)`.
    pub(crate) signaling_last: Option<Result<String, String>>,
}

/// Which deployment artifact the relay wizard renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RelayWizardFormat {
    #[default]
    Compose,
    Systemd,
    Caddy,
}

/// State for the "Set up your own relay" wizard. Everything transient;
/// the outcome that persists is the signaling URL + token written to
/// settings when the reachability test passes.
#[derive(Default)]
pub(crate) struct RelayWizardForm {
    pub open: bool,
    /// Bare domain (no scheme), e.g. `relay.example.com`.
    pub domain: String,
    /// Public HTTPS port; empty or `443` keeps the URL portless.
    pub port: String,
    /// Bearer token baked into the generated artifacts; generated on
    /// first open, regenerable.
    pub token: String,
    pub format: RelayWizardFormat,
    /// Reachability test in flight (disables the Test button).
    pub testing: bool,
    /// Last test outcome; `Err` carries the reason line.
    pub result: Option<Result<(), String>>,
    /// The `(base_url, token)` pair the in-flight probe is actually
    /// testing, captured when the Test starts. The result handler
    /// adopts these values, never the live form, so edits made during
    /// the probe can't be persisted untested. Any domain / port /
    /// token change clears it, which also invalidates the in-flight
    /// probe: a missing snapshot when the result arrives discards the
    /// result entirely and the user re-tests.
    pub testing_snapshot: Option<(String, String)>,
}

impl RelayWizardForm {
    /// `https://domain[:port]` from the form, `None` while the domain
    /// is empty. Scheme prefixes typed by the user are tolerated.
    pub fn base_url(&self) -> Option<String> {
        let domain = self
            .domain
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        if domain.is_empty() {
            return None;
        }
        let port = self.port.trim();
        if port.is_empty() || port == "443" {
            Some(format!("https://{domain}"))
        } else {
            Some(format!("https://{domain}:{port}"))
        }
    }

    /// The ready-to-paste server file for the selected format. Content
    /// mirrors SELF_HOSTING.md; comments stay English like any file we
    /// generate for a server.
    pub fn artifact(&self) -> String {
        let domain = self.domain.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
        let domain = if domain.is_empty() { "relay.example.com" } else { domain };
        match self.format {
            RelayWizardFormat::Compose => format!(
                "# docker-compose.yml\nservices:\n  oryxis-relay:\n    image: ghcr.io/wilsonglasser/oryxis-relay:latest\n    restart: unless-stopped\n    ports:\n      - \"127.0.0.1:8080:8080\"\n    environment:\n      ORYXIS_RELAY_TOKEN: \"{token}\"\n",
                token = self.token
            ),
            RelayWizardFormat::Systemd => format!(
                "# /etc/systemd/system/oryxis-relay.service\n# Binary: grab the relay-v* asset from the GitHub releases page\n# (or `cargo build --release -p oryxis-relay`).\n[Unit]\nDescription=Oryxis sync relay\nAfter=network.target\n\n[Service]\nType=simple\nUser=oryxis\nExecStart=/usr/local/bin/oryxis-relay --port 8080\nEnvironment=ORYXIS_RELAY_TOKEN={token}\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy=multi-user.target\n",
                token = self.token
            ),
            RelayWizardFormat::Caddy => format!(
                "# Caddyfile (TLS via Let's Encrypt is automatic)\n# Long-poll: keep proxy read timeouts above 150s if you front\n# this with nginx instead (proxy_read_timeout 180s).\n{domain} {{\n    reverse_proxy 127.0.0.1:8080\n}}\n"
            ),
        }
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "manual".into(),
            passwords: false,
            device_name: String::new(),
            // The engine config exposes `signaling_url` / `signaling_token`
            // as `Option<String>`; the app state uses a plain `String`
            // (empty == not set) so a Settings text input can drive it.
            // Empty by default: fresh installs are LAN-only, internet
            // backends are an explicit choice. Devices that were riding
            // the old baked-in hosted URL get it written into their
            // settings once by the boot migration (see boot/load.rs).
            signaling_url: String::new(),
            signaling_token: String::new(),
            relay_url: String::new(),
            listen_port: "0".into(),
            peers: Vec::new(),
            status: None,
            runtime: None,
            pending_engine: None,
            engine_running: false,
            pairing: SyncPairingForm::default(),
            discovered: Vec::new(),
            in_progress: false,
            abort_tx: None,
            signaling_tick: 0,
            transport: "p2p".into(),
            sftp: SftpSyncForm::default(),
            folder: FolderSyncForm::default(),
            webdav: WebdavSyncForm::default(),
            git: GitSyncForm::default(),
            passphrase_input: SecretInput::default(),
            passphrase_known: false,
            passphrase_editing: false,
            passphrase_field_id: None,
            passphrase_field_bounds: crate::widgets::new_bounds_cell(),
            passphrase_matches: None,
            passphrase_sealed: None,
            relay_wizard: RelayWizardForm::default(),
            signaling_last: None,
        }
    }
}
