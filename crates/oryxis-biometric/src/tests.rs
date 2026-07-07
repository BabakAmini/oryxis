//! Orchestrator lifecycle tests against the in-memory mock. These pin the
//! decisions that would otherwise only surface on a device with a real
//! keystore: enroll-then-unlock, refresh-on-rotation, disable-clears, and
//! the availability / consent gates.

use crate::mock::MockProvider;
use crate::provider::{BioError, BiometricProvider};
use crate::BiometricVault;

use std::sync::Arc;

/// Build a `BiometricVault` over a shared mock so the test can still poke
/// the mock's availability / consent toggles after handing it off.
fn vault_with(mock: Arc<MockProvider>, account: &str) -> BiometricVault {
    // A thin forwarder lets the test keep its own `Arc` handle while the
    // vault owns a `Box<dyn BiometricProvider>`.
    struct Shared(Arc<MockProvider>);
    impl BiometricProvider for Shared {
        fn is_available(&self) -> bool {
            self.0.is_available()
        }
        fn enroll(&self, account: &str, secret: &str) -> Result<(), BioError> {
            self.0.enroll(account, secret)
        }
        fn retrieve(&self, account: &str) -> Result<String, BioError> {
            self.0.retrieve(account)
        }
        fn clear(&self, account: &str) -> Result<(), BioError> {
            self.0.clear(account)
        }
    }
    BiometricVault::with_provider(Box::new(Shared(mock)), account)
}

#[test]
fn enroll_then_unlock_returns_the_password() {
    let mock = Arc::new(MockProvider::new());
    let bv = vault_with(mock.clone(), "vault:a");

    bv.enroll("hunter2").unwrap();
    assert!(mock.has("vault:a"));
    assert_eq!(bv.unlock_secret().unwrap(), "hunter2");
}

#[test]
fn refresh_replaces_the_stored_password() {
    let mock = Arc::new(MockProvider::new());
    let bv = vault_with(mock.clone(), "vault:a");

    bv.enroll("old-pw").unwrap();
    bv.refresh("new-pw").unwrap();
    // A rotation that forgot to update the store would return the stale
    // password here; that is exactly the silent break this pins against.
    assert_eq!(bv.unlock_secret().unwrap(), "new-pw");
}

#[test]
fn disable_clears_and_makes_unlock_fail_not_enrolled() {
    let mock = Arc::new(MockProvider::new());
    let bv = vault_with(mock.clone(), "vault:a");

    bv.enroll("pw").unwrap();
    bv.disable().unwrap();
    assert!(!mock.has("vault:a"));
    assert!(matches!(bv.unlock_secret(), Err(BioError::NotEnrolled)));
}

#[test]
fn disable_is_idempotent_when_nothing_enrolled() {
    let mock = Arc::new(MockProvider::new());
    let bv = vault_with(mock, "vault:a");
    // Never enrolled: disable must still succeed so the caller can wire it
    // to an unconditional opt-out.
    bv.disable().unwrap();
}

#[test]
fn denied_prompt_surfaces_as_denied() {
    let mock = Arc::new(MockProvider::new());
    let bv = vault_with(mock.clone(), "vault:a");

    bv.enroll("pw").unwrap();
    mock.set_consent(false);
    assert!(matches!(bv.unlock_secret(), Err(BioError::Denied)));
    // The secret is still stored: a denied prompt must not wipe it, so a
    // retry (or the typed password) still works.
    mock.set_consent(true);
    assert_eq!(bv.unlock_secret().unwrap(), "pw");
}

#[test]
fn unavailable_backend_rejects_enroll_and_unlock() {
    let mock = Arc::new(MockProvider::new());
    mock.set_available(false);
    let bv = vault_with(mock, "vault:a");

    assert!(!bv.is_available());
    assert!(matches!(bv.enroll("pw"), Err(BioError::Unavailable)));
    assert!(matches!(bv.unlock_secret(), Err(BioError::Unavailable)));
}

#[test]
fn accounts_are_isolated() {
    let mock = Arc::new(MockProvider::new());
    let a = vault_with(mock.clone(), "vault:a");
    let b = vault_with(mock.clone(), "vault:b");

    a.enroll("pw-a").unwrap();
    b.enroll("pw-b").unwrap();
    // Two vaults sharing one OS store must not read each other's secret.
    assert_eq!(a.unlock_secret().unwrap(), "pw-a");
    assert_eq!(b.unlock_secret().unwrap(), "pw-b");

    a.disable().unwrap();
    // Clearing one leaves the other intact.
    assert!(matches!(a.unlock_secret(), Err(BioError::NotEnrolled)));
    assert_eq!(b.unlock_secret().unwrap(), "pw-b");
}

#[test]
fn unavailable_provider_is_never_available() {
    use crate::provider::UnavailableProvider;
    let bv = BiometricVault::with_provider(Box::new(UnavailableProvider), "vault:a");
    assert!(!bv.is_available());
    assert!(matches!(bv.unlock_secret(), Err(BioError::Unavailable)));
    // disable() stays a no-op success even here.
    bv.disable().unwrap();
}
