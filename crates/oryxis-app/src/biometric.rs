//! App-side glue for local biometric unlock (see the `oryxis-biometric`
//! crate). This module owns the mapping from "the currently open vault" to
//! a per-vault [`BiometricVault`], plus the stable account id the OS
//! keystore is keyed on.
//!
//! The lifecycle (enroll on opt-in, refresh on rotation, disable on
//! opt-out or password removal, retrieve on the lock screen) is driven
//! from `dispatch_vault.rs`; the crate's own orchestrator holds the
//! platform-independent logic and is unit-tested there.

use oryxis_biometric::BiometricVault;
use sha2::{Digest, Sha256};

use crate::app::Oryxis;

/// Derive the OS-keystore account id for a vault from its database path.
///
/// A SHA-256 of the canonical path keeps the id stable across app updates
/// (a non-cryptographic `Hash` is not guaranteed stable across builds, so
/// it would silently orphan the stored secret after an upgrade) while
/// keeping the raw path (which contains the user's home directory) out of
/// the credential label. The `bio1:` prefix namespaces the scheme so a
/// future format change can coexist.
fn account_for(vault_path: &str) -> String {
    let digest = Sha256::digest(vault_path.as_bytes());
    // 16 bytes of the digest is ample to avoid collisions across a user's
    // handful of vaults while staying short in the credential store.
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("bio1:{hex}")
}

impl Oryxis {
    /// Build a [`BiometricVault`] bound to the open vault's account, or
    /// `None` when no vault is open. Cheap to construct (it only wraps the
    /// platform provider), so callers make one per operation rather than
    /// caching it.
    pub(crate) fn biometric_vault(&self) -> Option<BiometricVault> {
        let path = self.vault.as_ref()?.db_path().to_string_lossy().to_string();
        Some(BiometricVault::new(account_for(&path)))
    }

    /// Whether the biometric-unlock affordance should be offered at all:
    /// the platform supports it, the user opted in, and the vault actually
    /// has a master password to store (a passwordless vault has nothing to
    /// gate).
    pub(crate) fn biometric_unlock_offered(&self) -> bool {
        self.setting_biometric_unlock_enabled
            && self.biometric_available
            && self.vault_ui.has_user_password
    }

    /// Best-effort refresh of the stored secret after a successful unlock
    /// or a password rotation, so the OS keystore always holds the current
    /// master password. Silent by design (enroll never prompts); a backend
    /// error is logged, not surfaced, since the typed password still works.
    pub(crate) fn biometric_reenroll(&self, master_password: &str) {
        if !self.biometric_unlock_enabled_and_available() {
            return;
        }
        if let Some(bv) = self.biometric_vault()
            && let Err(e) = bv.enroll(master_password)
        {
            tracing::warn!("biometric re-enroll failed: {e}");
        }
    }

    /// Forget any stored secret for the open vault. Idempotent; used when
    /// the user opts out or removes the vault password.
    pub(crate) fn biometric_forget(&self) {
        if let Some(bv) = self.biometric_vault()
            && let Err(e) = bv.disable()
        {
            tracing::warn!("biometric disable failed: {e}");
        }
    }

    /// The setting is on and the platform can service it. Distinct from
    /// [`Self::biometric_unlock_offered`] in that it does not require a
    /// user password (the caller has already established one when
    /// enrolling).
    fn biometric_unlock_enabled_and_available(&self) -> bool {
        self.setting_biometric_unlock_enabled && self.biometric_available
    }
}

#[cfg(test)]
mod tests {
    use super::account_for;

    #[test]
    fn account_is_stable_and_path_specific() {
        // Same path -> same id (stable across calls / builds via SHA-256).
        assert_eq!(account_for("/home/u/.oryxis/vault.db"), account_for("/home/u/.oryxis/vault.db"));
        // Different vaults -> different ids (no cross-vault secret bleed).
        assert_ne!(
            account_for("/home/u/.oryxis/vault.db"),
            account_for("/home/u/.oryxis/work.db")
        );
        // The raw path never appears in the id (only its digest).
        assert!(!account_for("/home/u/.oryxis/vault.db").contains("/home/u"));
    }
}
