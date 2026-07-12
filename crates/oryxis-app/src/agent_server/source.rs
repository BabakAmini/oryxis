//! The key source an agent connection signs against.
//!
//! The protocol layer ([`super::protocol`]) is generic over this trait
//! so it can be exercised with a [`MockKeySource`] in tests and backed
//! by the vault in production (`VaultKeySource`, Phase 2). The trait is
//! deliberately narrow: an agent is a read-only signing oracle, so it
//! only ever lists public keys and signs, never adds or removes.
//!
//! Security contract for the vault-backed impl: [`sign`](AgentKeySource::sign)
//! decrypts exactly one private key, uses it, and drops it; nothing is
//! cached across calls. A locked vault makes [`list`](AgentKeySource::list)
//! return empty and [`sign`](AgentKeySource::sign) fail, so an external
//! `git` sees "agent has no identities" instead of a hang.

use ssh_key::HashAlg;

/// One public key the agent advertises: its SSH wire blob (the bytes an
/// `IDENTITIES_ANSWER` carries) and a human comment (the vault label).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentPublicKey {
    /// The public key encoded as an SSH wire blob (`KeyData` encoding).
    pub blob: Vec<u8>,
    /// Comment shown by `ssh-add -l` etc.; the vault key's label.
    pub comment: String,
}

/// Why a sign request could not be answered. The protocol layer maps
/// every variant to a single `FAILURE` on the wire (the ssh-agent
/// protocol has no error detail), but the distinction drives logging.
#[derive(Debug)]
pub(crate) enum AgentSignError {
    /// No exposed key matches the requested blob.
    UnknownKey,
    /// The vault is locked (or the key is gone); nothing to sign with.
    Unavailable,
    /// The key material could not be parsed or the signature failed.
    SignFailed(String),
    /// The user (or a timeout) denied the per-signature confirmation.
    Denied,
}

/// The RSA hash the client asked for, decoded from the `SIGN_REQUEST`
/// flag bits. Ed25519 / ECDSA ignore it (their hash is fixed by the
/// algorithm).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignHash {
    /// No RSA flag set: default to SHA-256. Legacy ssh-rsa (SHA-1) is
    /// deprecated and rejected by modern servers, so we never emit it;
    /// a client that sets no flag is rare and SHA-256 is universally
    /// accepted.
    Default,
    Sha256,
    Sha512,
}

impl SignHash {
    /// The `ssh-key` hash for RSA signing (`None` for non-RSA keys,
    /// which the caller passes through unchanged).
    pub(crate) fn rsa_hash(self) -> Option<HashAlg> {
        match self {
            Self::Sha512 => Some(HashAlg::Sha512),
            // Default and explicit 256 both sign with SHA-256.
            Self::Default | Self::Sha256 => Some(HashAlg::Sha256),
        }
    }
}

/// A read-only source of SSH keys for the agent to serve. `Send + Sync`
/// so one instance is shared across per-connection tasks.
pub(crate) trait AgentKeySource: Send + Sync {
    /// The keys to advertise right now. Empty while the vault is locked
    /// or when the user has exposed none.
    fn list(&self) -> Vec<AgentPublicKey>;

    /// Sign `data` with the key whose wire blob is `key_blob`, honoring
    /// the requested RSA hash. Returns the SSH signature wire blob (the
    /// `string signature` a `SIGN_RESPONSE` carries).
    fn sign(
        &self,
        key_blob: &[u8],
        data: &[u8],
        hash: SignHash,
    ) -> Result<Vec<u8>, AgentSignError>;
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use ssh_encoding::Encode;
    use ssh_key::PrivateKey;

    /// In-memory key source for protocol tests: holds decrypted keys
    /// directly (a test convenience the vault-backed impl never does).
    pub(crate) struct MockKeySource {
        keys: Vec<(Vec<u8>, PrivateKey, String)>,
        pub locked: std::sync::atomic::AtomicBool,
    }

    impl MockKeySource {
        pub(crate) fn new(keys: Vec<(PrivateKey, String)>) -> Self {
            let keys = keys
                .into_iter()
                .map(|(k, label)| {
                    let mut blob = Vec::new();
                    k.public_key().key_data().encode(&mut blob).unwrap();
                    (blob, k, label)
                })
                .collect();
            Self {
                keys,
                locked: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    impl AgentKeySource for MockKeySource {
        fn list(&self) -> Vec<AgentPublicKey> {
            if self.locked.load(std::sync::atomic::Ordering::Relaxed) {
                return Vec::new();
            }
            self.keys
                .iter()
                .map(|(blob, _, comment)| AgentPublicKey {
                    blob: blob.clone(),
                    comment: comment.clone(),
                })
                .collect()
        }

        fn sign(
            &self,
            key_blob: &[u8],
            data: &[u8],
            hash: SignHash,
        ) -> Result<Vec<u8>, AgentSignError> {
            use ssh_key::private::KeypairData;
            use signature::Signer;

            if self.locked.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(AgentSignError::Unavailable);
            }
            let (_, key, _) = self
                .keys
                .iter()
                .find(|(blob, _, _)| blob == key_blob)
                .ok_or(AgentSignError::UnknownKey)?;

            let sig: ssh_key::Signature = match key.key_data() {
                KeypairData::Rsa(pair) => Signer::try_sign(&(pair, hash.rsa_hash()), data)
                    .map_err(|e| AgentSignError::SignFailed(e.to_string()))?,
                _ => Signer::try_sign(key, data)
                    .map_err(|e| AgentSignError::SignFailed(e.to_string()))?,
            };
            let mut out = Vec::new();
            sig.encode(&mut out)
                .map_err(|e| AgentSignError::SignFailed(e.to_string()))?;
            Ok(out)
        }
    }
}
