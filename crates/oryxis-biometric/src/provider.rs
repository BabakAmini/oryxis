//! The provider contract and the always-unavailable fallback.

use thiserror::Error;

/// Failure modes shared by every platform provider.
#[derive(Debug, Error)]
pub enum BioError {
    /// The platform / session cannot biometrically unlock (no hardware,
    /// no keystore, an unsupported target). The caller should hide the
    /// feature rather than surface this as an error.
    #[error("biometric unlock is unavailable on this platform or session")]
    Unavailable,

    /// No secret is stored for this account yet (never enrolled, or it
    /// was cleared). The caller should fall back to the password field.
    #[error("no biometric secret is enrolled for this vault")]
    NotEnrolled,

    /// The user cancelled the prompt or failed the presence check.
    #[error("biometric verification was cancelled or denied")]
    Denied,

    /// The underlying OS keystore / prompt API returned an error. The
    /// string is for logs and the honest error line, not for control
    /// flow.
    #[error("biometric backend error: {0}")]
    Backend(String),
}

/// A platform biometric / keystore backend.
///
/// Implementors store a single secret (the vault master password) per
/// `account` and release it only after a user-presence check. `account`
/// is an opaque, per-vault key chosen by the caller; implementors namespace
/// it into their store however is idiomatic (a Credential Manager target,
/// a Keychain service, a Secret Service attribute).
pub trait BiometricProvider: Send + Sync {
    /// Whether enroll / retrieve can work right now. Cheap and
    /// side-effect free (no prompt): it gates whether the UI offers the
    /// feature at all.
    fn is_available(&self) -> bool;

    /// Persist `secret` for `account`, replacing any existing value.
    fn enroll(&self, account: &str, secret: &str) -> Result<(), BioError>;

    /// Raise the presence prompt and return the stored secret on success.
    fn retrieve(&self, account: &str) -> Result<String, BioError>;

    /// Remove any stored secret for `account`. Removing a missing entry
    /// is success, not an error (callers disable unconditionally).
    fn clear(&self, account: &str) -> Result<(), BioError>;
}

/// Fallback provider for targets with no biometric/keystore support. It
/// is never available, so every real operation is rejected before it can
/// run; the app treats `is_available() == false` as "hide the feature".
pub struct UnavailableProvider;

impl BiometricProvider for UnavailableProvider {
    fn is_available(&self) -> bool {
        false
    }

    fn enroll(&self, _account: &str, _secret: &str) -> Result<(), BioError> {
        Err(BioError::Unavailable)
    }

    fn retrieve(&self, _account: &str) -> Result<String, BioError> {
        Err(BioError::Unavailable)
    }

    fn clear(&self, _account: &str) -> Result<(), BioError> {
        // Clearing on an unavailable backend is a no-op success so the
        // caller's unconditional `disable()` never spuriously errors.
        Ok(())
    }
}
