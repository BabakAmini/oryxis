//! P2P + SFTP-snapshot sync messages, wrapped by
//! [`crate::messages::Message::Sync`]. Handled by `Oryxis::handle_sync`
//! (the SFTP-snapshot round logic lives in `dispatch_sftp_sync`).

#[derive(Debug, Clone)]
pub enum SyncMessage {
    ToggleEnabled,
    TogglePasswords,
    ModeChanged(String),
    DeviceNameChanged(String),
    SignalingUrlChanged(String),
    /// Bearer token text-input change. Persisted to the vault settings
    /// table; an empty string leaves the request without an
    /// `Authorization` header (fine for unauthenticated signaling).
    SignalingTokenChanged(super::Redacted),
    RelayUrlChanged(String),
    ListenPortChanged(String),
    // "Set up your own relay" wizard (Settings > Sync > P2P).
    WizardToggle,
    WizardDomainChanged(String),
    WizardPortChanged(String),
    WizardFormatChanged(crate::state::RelayWizardFormat),
    WizardRegenToken,
    WizardTest,
    WizardTestResult(Result<(), String>),
    StartPairing,
    UnpairDevice(uuid::Uuid),
    Now,
    /// Top-level result of a manual `SyncNow`. Per-peer outcomes arrive
    /// separately as `SyncEngineEvent`s; this only carries a vault-level
    /// failure (e.g. the lock could not be taken).
    NowFinished(Result<(), String>),
    /// An event emitted by the running `SyncEngine` (peer discovered,
    /// sync completed, pairing progress, ...), pumped in from the
    /// engine's event channel via `Task::stream`.
    EngineEvent(oryxis_sync::SyncEvent),
    /// Stop hosting the pairing code and return to the idle pairing view.
    CancelHostingPairing,
    /// Switch the pairing panel into "join with a code" mode.
    JoinPairingRequested,
    /// Text-input change for the joiner's 6-digit code field.
    JoinCodeChanged(String),
    /// Text-input change for the joiner's `ip:port` host-address field.
    JoinTargetChanged(String),
    /// Joiner pressed Connect: dial the entered address with the code.
    JoinPairingConnect,
    /// Joiner backed out of the join form, return to the idle view.
    JoinPairingCancel,
    /// Text-input change for the joiner's `oryxis://pair/...` link
    /// field (the cross-network alternative to code + address).
    JoinLinkChanged(String),
    /// Joiner pressed Connect with link: parse the link, look the
    /// device id up on the signaling server, run the handshake.
    JoinPairingByLink,
    /// User clicked Pair on a row in the live discovered-devices
    /// list. Switches to the Joining sub-view and pre-fills the
    /// host-address field with the discovered peer's `ip:port`.
    PairWithDiscovered(uuid::Uuid),
    /// Abort the in-flight `Sync Now` Task. Fires the oneshot the
    /// task is racing against; the task lands back as
    /// `SyncNowFinished(Err("Cancelled"))` and clears the flags.
    CancelInProgress,
    /// Switch the sync transport between `"p2p"` and `"sftp"`. Persists
    /// the setting, stops the P2P engine when leaving `p2p`, and starts
    /// it when returning.
    TransportChanged(String),
    /// Pick the host the SFTP-sync snapshot file lives on (also closes
    /// the picker modal).
    SftpHostChanged(uuid::Uuid),
    /// Open the "Select a host" modal for the SFTP-sync backup host.
    SftpHostPickerOpen,
    /// Close that modal without changing the selection.
    SftpHostPickerClose,
    /// Search-filter text change inside the host picker modal.
    SftpHostPickerSearch(String),
    /// Text-input change for the SFTP-sync remote path.
    SftpPathChanged(String),
    /// Text-input change for the SFTP-sync passphrase (persisted
    /// encrypted on change).
    SftpPassphraseChanged(super::Redacted),
    /// Auto-cadence timer fired (transport `sftp`, mode `auto`): run a
    /// sync round if one isn't already in flight.
    SftpTick,
    /// An SFTP-sync round finished. `Ok` carries a short status summary;
    /// `Err` a human-readable failure. A failed round never overwrites
    /// the remote snapshot (see `dispatch_sftp_sync`).
    SftpDone(Result<String, String>),
}
