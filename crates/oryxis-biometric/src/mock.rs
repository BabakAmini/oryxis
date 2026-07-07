//! In-memory provider for unit-testing the orchestrator without any OS
//! keystore. Models the two axes a real backend varies on: whether the
//! backend is available at all, and whether the presence check passes.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::provider::{BioError, BiometricProvider};

/// A fake keystore. `available` mirrors real hardware/session presence;
/// `consent` mirrors whether the user passes the prompt on `retrieve`.
/// Both are flippable mid-test to exercise the deny / unavailable paths.
pub struct MockProvider {
    available: Mutex<bool>,
    consent: Mutex<bool>,
    store: Mutex<HashMap<String, String>>,
}

impl MockProvider {
    /// A working, consenting store (the happy path).
    pub fn new() -> Self {
        Self {
            available: Mutex::new(true),
            consent: Mutex::new(true),
            store: Mutex::new(HashMap::new()),
        }
    }

    /// Toggle backend availability (simulate no hardware / locked session).
    pub fn set_available(&self, v: bool) {
        *self.available.lock().unwrap() = v;
    }

    /// Toggle whether the next `retrieve` passes the presence check.
    pub fn set_consent(&self, v: bool) {
        *self.consent.lock().unwrap() = v;
    }

    /// Test-only peek at whether a secret is stored for `account`.
    pub fn has(&self, account: &str) -> bool {
        self.store.lock().unwrap().contains_key(account)
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BiometricProvider for MockProvider {
    fn is_available(&self) -> bool {
        *self.available.lock().unwrap()
    }

    fn enroll(&self, account: &str, secret: &str) -> Result<(), BioError> {
        if !self.is_available() {
            return Err(BioError::Unavailable);
        }
        self.store
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn retrieve(&self, account: &str) -> Result<String, BioError> {
        if !self.is_available() {
            return Err(BioError::Unavailable);
        }
        if !*self.consent.lock().unwrap() {
            return Err(BioError::Denied);
        }
        self.store
            .lock()
            .unwrap()
            .get(account)
            .cloned()
            .ok_or(BioError::NotEnrolled)
    }

    fn clear(&self, account: &str) -> Result<(), BioError> {
        self.store.lock().unwrap().remove(account);
        Ok(())
    }
}
