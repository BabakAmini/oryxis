//! SSH-agent server UI + runtime state (B1). Off by default; the
//! runtime is `Some` only while the feature toggle is on and the vault
//! is available.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// A pending per-signature confirmation shown to the user. Holds the
/// oneshot responder inside an `Arc<Mutex<Option<_>>>` so the message
/// carrying it stays `Clone` (a bare `oneshot::Sender` is not).
#[derive(Clone, Debug)]
pub(crate) struct AgentConfirmCard {
    pub key_comment: String,
    pub key_fingerprint: String,
    pub peer: Option<String>,
    /// Taken and fired exactly once on the user's decision.
    pub responder: Arc<Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>,
}

/// Which generated setup snippet a Copy button targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSnippetKind {
    /// `export SSH_AUTH_SOCK=...` for a shell profile.
    ShellEnv,
    /// `Host *\n  IdentityAgent <path>` for ~/.ssh/config.
    SshConfig,
}

#[derive(Default)]
pub(crate) struct AgentState {
    /// Persisted `agent_server_enabled` (default false). Mirrors the
    /// live runtime's presence.
    pub enabled: bool,
    /// Persisted `agent_server_confirm` (default true): prompt on every
    /// signature.
    pub confirm: bool,
    /// The live runtime while the feature is on.
    pub runtime: Option<crate::agent_server::AgentRuntime>,
    /// A bind / start error, shown inline under the toggle (and reverts
    /// the toggle).
    pub error: Option<String>,
    /// The confirm prompt currently on screen, if any.
    pub pending_confirm: Option<AgentConfirmCard>,
    /// The "remember this key this session" checkbox state for the
    /// on-screen prompt.
    pub confirm_always: bool,
    /// "Always allow this key this session" grants, keyed by SHA-256
    /// fingerprint. Swept on lock and on toggle-off.
    pub session_grants: HashSet<String>,
}
