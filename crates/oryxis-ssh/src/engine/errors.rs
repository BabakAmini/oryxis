use super::*;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SshError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Authentication failed")]
    AuthFailed,

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Russh error: {0}")]
    Russh(#[from] russh::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Key error: {0}")]
    Key(String),

    #[error("Proxy error: {0}")]
    Proxy(String),

    #[error("Jump host error: {0}")]
    JumpHost(String),
}

/// Which SSH negotiation category had no common algorithm. Mirrors the
/// per-host override categories so the UI can expand exactly the right
/// one (or all) on a legacy-fallback retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegCategory {
    Kex,
    HostKey,
    Cipher,
    Mac,
}

/// A "no common algorithm" handshake failure, surfaced structurally so
/// the app can offer "this server only speaks legacy X, connect anyway?"
/// rather than parsing an error string.
#[derive(Debug, Clone)]
pub struct NegotiationFailure {
    pub category: NegCategory,
    /// The algorithms the server offered for the failed category.
    pub server_offers: Vec<String>,
}

impl SshError {
    /// If this is a russh "no common algorithm" failure, return the
    /// failed category and what the server offered. Compression failures
    /// are not user-actionable here, so they map to `None`.
    pub fn negotiation_failure(&self) -> Option<NegotiationFailure> {
        let SshError::Russh(russh::Error::NoCommonAlgo { kind, theirs, .. }) = self else {
            return None;
        };
        let category = match kind {
            russh::AlgorithmKind::Kex => NegCategory::Kex,
            russh::AlgorithmKind::Key => NegCategory::HostKey,
            russh::AlgorithmKind::Cipher => NegCategory::Cipher,
            russh::AlgorithmKind::Mac => NegCategory::Mac,
            russh::AlgorithmKind::Compression => return None,
        };
        Some(NegotiationFailure {
            category,
            server_offers: theirs.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Client handler
// ---------------------------------------------------------------------------

/// Result of checking a host key against known hosts.
#[derive(Debug, Clone)]
pub enum HostKeyStatus {
    /// Host is known and fingerprint matches, accept silently.
    Known,
    /// Host is known but fingerprint CHANGED, potential MITM.
    Changed { old_fingerprint: String },
    /// Host is not known, need to ask the user.
    Unknown,
}

/// Query about a host key that the UI must answer.
#[derive(Debug, Clone)]
pub struct HostKeyQuery {
    pub hostname: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    pub status: HostKeyStatus,
}

/// Sync callback that checks known hosts and returns the status.
pub type HostKeyCheckCallback = Arc<dyn Fn(&str, u16, &str, &str) -> HostKeyStatus + Send + Sync>;

/// Channel for asking the UI to verify a host key. The UI sends `true` (accept) or `false` (reject).
pub type HostKeyAskSender = tokio::sync::mpsc::Sender<(HostKeyQuery, tokio::sync::oneshot::Sender<bool>)>;

/// A single keyboard-interactive prompt line. `prompt` is the raw label
/// the server sent (e.g. `"Password:"`, `"Verification code:"`) and must
/// be rendered verbatim, never translated. `echo` says whether the typed
/// answer should be visible (`true`) or masked (`false`).
#[derive(Debug, Clone)]
pub struct KbiPromptField {
    pub prompt: String,
    pub echo: bool,
}

/// A keyboard-interactive challenge round the UI must answer. `name` and
/// `instructions` are server-provided headers (e.g. `"Two-factor
/// authentication"`); both can be empty. One round can carry several
/// prompts (password + OTP, etc.).
#[derive(Debug, Clone)]
pub struct KbiQuery {
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<KbiPromptField>,
}

/// Channel for asking the UI to answer a keyboard-interactive round. The
/// UI sends `Some(answers)` (one per prompt, in order) or `None` to
/// cancel the authentication.
pub type KbiAskSender =
    tokio::sync::mpsc::Sender<(KbiQuery, tokio::sync::oneshot::Sender<Option<Vec<String>>>)>;

/// How a keyboard-interactive exchange ended. `Rejected` (server said no,
/// or no answer source was available) and `Cancelled` (the user dismissed
/// the prompt) are kept apart so callers can fall back to another method
/// after a refusal without ever re-prompting after an explicit cancel.
/// `Partial` is RFC 4252 partial success: the exchange itself was
/// accepted, but the server requires one more of the carried methods
/// before granting access (issue #125).
pub(crate) enum KbiOutcome {
    Success,
    Rejected,
    Cancelled,
    Partial(russh::MethodSet),
}
