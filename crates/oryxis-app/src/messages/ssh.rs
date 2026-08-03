//! SSH connect flow: connect/quick-connect, progress, banners, host-key verify, keyboard-interactive, legacy-algo fallback, OS detect, wrapped by [`crate::messages::Message::Ssh`]. Handled by `Oryxis::handle_ssh`.

use uuid::Uuid;
use crate::state::{ConnectionStep};

use super::Message;

#[derive(Debug, Clone)]
pub enum SshMessage {
    ConnectSsh(usize),
    /// Connect an ad-hoc quick-connect host (never persisted). The entry
    /// is inserted into `quick_connects` keyed by its connection id; a
    /// retry for an id already present reuses the stored entry so
    /// in-place mutations (expanded legacy algorithms) survive.
    QuickConnect(Box<crate::state::QuickConnectEntry>),
    SshProgress(ConnectionStep, String),
    /// Pre-auth banner (RFC 4252 §5.4) for the connect in progress:
    /// shown on the progress card and written to the tab's terminal.
    SshBanner(String),
    /// Pre-auth banner for a split-pane connect (no progress card):
    /// written straight to that pane's terminal.
    SshPaneBanner(Uuid, String),
    SshConnected(Uuid, crate::state::TerminalTransport),  // (pane_id, transport)
    SshDisconnected(Uuid),  // (pane_id)
    SshError(String),
    /// Handshake hit "no common algorithm". Prompts the legacy-fallback
    /// dialog for `conn_id` (the failed category + what the server offered).
    SshNoCommonAlgo {
        conn_id: uuid::Uuid,
        category: oryxis_ssh::NegCategory,
        server_offers: Vec<String>,
        /// Action to re-run after the user enables legacy algorithms (the
        /// originating connect: terminal / SFTP / port-forward / backup).
        retry: Box<Message>,
    },
    /// Accept the legacy fallback: enable the legacy algorithms on the
    /// pending host and reconnect. `remember` persists the change.
    LegacyAlgoAccept { remember: bool },
    LegacyAlgoCancel,
    SshHostKeyVerify(oryxis_ssh::HostKeyQuery),
    SshHostKeyReject,
    SshHostKeyContinue,
    SshHostKeyAcceptAndSave,
    /// A keyboard-interactive challenge round arrived from the engine.
    /// The `Option<Uuid>` is the quick-connect entry id when the prompt
    /// belongs to an ad-hoc connect (it unlocks the saved identity / key
    /// selector in the modal); `None` for saved hosts.
    SshKbiPrompt(Option<Uuid>, oryxis_ssh::KbiQuery),
    /// User edited the answer for prompt `usize` in the current round.
    SshKbiInput(usize, super::Redacted),
    /// User submitted all answers for the current round.
    SshKbiSubmit,
    /// User cancelled the interactive auth.
    SshKbiCancel,
    /// User picked a saved identity / key for a quick-connect host (from
    /// the interactive-prompt modal or the failed-connect screen). Mutates
    /// the ephemeral entry and retries the connect with it.
    QuickAuthSwitch(Uuid, crate::state::QuickAuthChoice),
    SshCloseProgress,
    SshEditFromProgress,
    SshRetry,
    /// A pane's SSH connect failed; surface the error inside the pane.
    PaneConnectError(Uuid, String),
    OsDetected(Uuid, Option<String>),
}
