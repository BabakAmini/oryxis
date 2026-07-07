//! Linux (and non-macOS unix) provider over the freedesktop Secret
//! Service, i.e. the login keyring (GNOME Keyring, KWallet's SS bridge,
//! KeePassXC's SS integration, ...).
//!
//! Honesty note: Linux has no standard biometric API, so there is no
//! fingerprint prompt here. The secret is guarded by the user's *unlocked
//! login session* (the keyring is unlocked at login), which is the same
//! protection every Linux password manager relies on. The app's UI must
//! present this as "OS keystore", not "fingerprint", on this platform.

use keyring::{Entry, Error as KeyringError};

use crate::provider::{BioError, BiometricProvider};

/// Secret Service backend. Each vault's secret is one keyring item keyed
/// by (`SERVICE`, account).
pub struct SecretServiceStore;

/// Fixed Secret Service "service" component; the per-vault account is the
/// "username" component, so two vaults land in distinct items.
const SERVICE: &str = "oryxis-vault-unlock";

/// A benign account name used only to probe whether the Secret Service
/// bus answers at all. It is never enrolled, so a well-behaved backend
/// reports `NoEntry` for it (which means "reachable, just empty").
const PROBE_ACCOUNT: &str = "__oryxis_availability_probe__";

impl SecretServiceStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(account: &str) -> Result<Entry, BioError> {
        Entry::new(SERVICE, account).map_err(map_err)
    }
}

impl Default for SecretServiceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl BiometricProvider for SecretServiceStore {
    fn is_available(&self) -> bool {
        // Probe the bus with a read that is expected to miss: `NoEntry`
        // proves the Secret Service is reachable and the keyring is
        // usable, whereas a platform/storage error means no service (a
        // headless box, a locked or absent keyring). Any other outcome
        // (somehow a hit) also means the bus works.
        match Entry::new(SERVICE, PROBE_ACCOUNT).and_then(|e| e.get_password()) {
            Ok(_) => true,
            Err(KeyringError::NoEntry) => true,
            Err(_) => false,
        }
    }

    fn enroll(&self, account: &str, secret: &str) -> Result<(), BioError> {
        Self::entry(account)?.set_password(secret).map_err(map_err)
    }

    fn retrieve(&self, account: &str) -> Result<String, BioError> {
        // The keyring may prompt to unlock the login collection here if it
        // is locked; on success it returns the secret. No biometric gate
        // on this platform (see the module note).
        Self::entry(account)?.get_password().map_err(|e| match e {
            KeyringError::NoEntry => BioError::NotEnrolled,
            other => map_err(other),
        })
    }

    fn clear(&self, account: &str) -> Result<(), BioError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) => Ok(()),
            // Nothing to remove is success, so `disable()` is idempotent.
            Err(KeyringError::NoEntry) => Ok(()),
            Err(other) => Err(map_err(other)),
        }
    }
}

/// Collapse a keyring error into the provider contract. `NoEntry` is
/// handled at the call sites where it carries meaning (not-enrolled vs
/// clear-noop), so here it just falls through to `Backend`.
fn map_err(e: KeyringError) -> BioError {
    match e {
        KeyringError::NoEntry => BioError::NotEnrolled,
        other => BioError::Backend(other.to_string()),
    }
}
