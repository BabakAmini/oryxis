//! Windows provider: Windows Hello for the presence prompt, Credential
//! Manager for the secret at rest.
//!
//! The master password is written as a per-user CRED_TYPE_GENERIC
//! credential (Credential Manager encrypts it under the user's DPAPI
//! master key, so it never sits in clear on disk). Every `retrieve` first
//! raises `UserConsentVerifier` (Windows Hello: face / fingerprint / PIN)
//! and only reads the credential once the user passes. The credential
//! itself is not additionally Hello-gated by the OS, so the gate is our
//! explicit `verify()` call ahead of the read; that is the same shape
//! every Hello-backed password manager on Windows uses.
//!
//! Not locally compiled (the dev host is Linux); this path is verified by
//! CI on windows-latest and by manual QA.

use std::future::IntoFuture;

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Security::Credentials::UI::{
    UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

use crate::provider::{BioError, BiometricProvider};

/// Credential Manager target-name prefix; the per-vault account is
/// appended so two vaults never share a credential.
const TARGET_PREFIX: &str = "OryxisVaultUnlock:";

pub struct WindowsHello;

impl WindowsHello {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsHello {
    fn default() -> Self {
        Self::new()
    }
}

/// Fully-qualified Credential Manager target for an account.
fn target_of(account: &str) -> HSTRING {
    HSTRING::from(format!("{TARGET_PREFIX}{account}"))
}

/// Raise the Windows Hello prompt and map the outcome to the contract.
fn verify() -> Result<(), BioError> {
    let message = HSTRING::from("Unlock your Oryxis vault");
    let op = UserConsentVerifier::RequestVerificationAsync(&message)
        .map_err(|e| BioError::Backend(e.message()))?;
    match pollster::block_on(op.into_future()) {
        Ok(UserConsentVerificationResult::Verified) => Ok(()),
        // Canceled / DeviceBusy / RetriesExhausted / DisabledByPolicy /
        // NotConfigured all mean "not authenticated now"; the caller falls
        // back to the typed password.
        Ok(_) => Err(BioError::Denied),
        Err(e) => Err(BioError::Backend(e.message())),
    }
}

impl BiometricProvider for WindowsHello {
    fn is_available(&self) -> bool {
        let Ok(op) = UserConsentVerifier::CheckAvailabilityAsync() else {
            return false;
        };
        matches!(
            pollster::block_on(op.into_future()),
            Ok(UserConsentVerifierAvailability::Available)
        )
    }

    fn enroll(&self, account: &str, secret: &str) -> Result<(), BioError> {
        let target = target_of(account);
        let mut blob = secret.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            // CredWriteW copies the string; casting the const pointer to
            // mut is sound because the call does not mutate it.
            TargetName: PWSTR(target.as_ptr() as *mut u16),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // CredWriteW overwrites an existing credential of the same target,
        // so enroll is idempotent / re-enrollable without a prior delete.
        unsafe { CredWriteW(&cred, 0) }.map_err(|e| BioError::Backend(e.message()))
    }

    fn retrieve(&self, account: &str) -> Result<String, BioError> {
        // Presence check first: no Hello, no read.
        verify()?;

        let target = target_of(account);
        let mut pcred: *mut CREDENTIALW = std::ptr::null_mut();
        unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut pcred,
            )
            .map_err(|e| {
                if e.code() == ERROR_NOT_FOUND.to_hresult() {
                    BioError::NotEnrolled
                } else {
                    BioError::Backend(e.message())
                }
            })?;

            let cred = &*pcred;
            let bytes =
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize);
            let secret = String::from_utf8_lossy(bytes).into_owned();
            CredFree(pcred as *const core::ffi::c_void);
            Ok(secret)
        }
    }

    fn clear(&self, account: &str) -> Result<(), BioError> {
        let target = target_of(account);
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(e) => Err(BioError::Backend(e.message())),
        }
    }
}
