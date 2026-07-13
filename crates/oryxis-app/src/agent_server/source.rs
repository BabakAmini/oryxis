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

/// The production [`AgentKeySource`]: a dedicated unlocked `VaultStore`
/// handle (its own SQLite handle on the same file, like `sync_runtime`;
/// WAL keeps concurrent handles safe). Keys are advertised by their
/// public blob; the private key is decrypted only inside [`sign`] and
/// dropped immediately. A `locked` gate flips on vault lock so the
/// agent goes dark without tearing down the listener.
pub(crate) struct VaultKeySource {
    vault: std::sync::Mutex<oryxis_vault::VaultStore>,
    locked: std::sync::atomic::AtomicBool,
}

impl VaultKeySource {
    pub(crate) fn new(vault: oryxis_vault::VaultStore) -> Self {
        Self {
            vault: std::sync::Mutex::new(vault),
            locked: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Flip the gate and lock the dedicated handle (zeroize its key) so
    /// nothing can be decrypted while the app vault is locked.
    pub(crate) fn lock(&self) {
        self.locked.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut v) = self.vault.lock() {
            v.lock();
        }
    }

    /// Re-unlock the dedicated handle with the master password and open
    /// the gate. `None` password re-opens a passwordless vault.
    pub(crate) fn unlock(&self, master_password: Option<&str>) {
        if let Ok(mut v) = self.vault.lock() {
            let ok = match master_password {
                Some(pw) => v.unlock(pw).is_ok(),
                None => v.open_without_password().is_ok(),
            };
            if ok {
                self.locked.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    /// The public blob (KeyData wire encoding) of a stored key's
    /// OpenSSH public line, or `None` when it does not parse.
    fn blob_of(public_line: &str) -> Option<Vec<u8>> {
        use ssh_encoding::Encode;
        let pk = ssh_key::PublicKey::from_openssh(public_line).ok()?;
        let mut blob = Vec::new();
        pk.key_data().encode(&mut blob).ok()?;
        Some(blob)
    }
}

impl AgentKeySource for VaultKeySource {
    fn list(&self) -> Vec<AgentPublicKey> {
        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Vec::new();
        }
        let Ok(vault) = self.vault.lock() else {
            return Vec::new();
        };
        vault
            .list_keys()
            .unwrap_or_default()
            .into_iter()
            // Security-key rows (B3) are public-only: signing happens on
            // the hardware token via the EXTERNAL agent, so listing them
            // here would advertise identities this agent can never sign
            // for (the `sign` lookup below would hit a NULL private).
            // Only rows we actually hold a private for: security-key /
            // public-only rows (NULL private) can never be signed here.
            // Gating on `has_private` (not just the sk- algorithm) keeps
            // list() symmetric with sign() below and covers a plain
            // public-only import too.
            .filter(|k| k.expose_via_agent && k.has_private)
            .filter_map(|k| {
                Self::blob_of(&k.public_key).map(|blob| AgentPublicKey {
                    blob,
                    comment: k.label,
                })
            })
            .collect()
    }

    fn sign(
        &self,
        key_blob: &[u8],
        data: &[u8],
        hash: SignHash,
    ) -> Result<Vec<u8>, AgentSignError> {
        use ssh_encoding::Encode;
        use ssh_key::private::KeypairData;
        use ssh_key::PrivateKey;

        if self.locked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AgentSignError::Unavailable);
        }
        let vault = self.vault.lock().map_err(|_| AgentSignError::Unavailable)?;

        // Find the exposed key whose public blob matches, then decrypt
        // exactly that one private key.
        let key = vault
            .list_keys()
            .map_err(|e| AgentSignError::SignFailed(e.to_string()))?
            .into_iter()
            .find(|k| {
                k.expose_via_agent
                    && k.has_private
                    && Self::blob_of(&k.public_key).as_deref() == Some(key_blob)
            })
            .ok_or(AgentSignError::UnknownKey)?;

        let pem = vault
            .get_key_private(&key.id)
            .map_err(|e| AgentSignError::SignFailed(e.to_string()))?
            .ok_or(AgentSignError::Unavailable)?;
        // The decrypted key is zeroized on drop by ssh_key.
        let private = PrivateKey::from_openssh(&pem)
            .map_err(|e| AgentSignError::SignFailed(e.to_string()))?;

        let sig: ssh_key::Signature = match private.key_data() {
            KeypairData::Rsa(pair) => signature::Signer::try_sign(&(pair, hash.rsa_hash()), data)
                .map_err(|e| AgentSignError::SignFailed(e.to_string()))?,
            _ => signature::Signer::try_sign(&private, data)
                .map_err(|e| AgentSignError::SignFailed(e.to_string()))?,
        };
        let mut out = Vec::new();
        sig.encode(&mut out)
            .map_err(|e| AgentSignError::SignFailed(e.to_string()))?;
        Ok(out)
    }
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

/// Integration tests for the production [`VaultKeySource`] against a
/// real on-disk vault, exercising the two-handle model the runtime
/// uses (the app writes through one `VaultStore`, the agent reads
/// through its own). These cover the runtime guarantees the protocol
/// tests only prove on the mock: a live `expose_via_agent` toggle
/// crossing handles, and the lock gate.
#[cfg(test)]
mod vault_source_tests {
    use super::*;
    use oryxis_vault::{generate_key, GenerateSpec, VaultStore};

    /// Open a fresh password-protected vault, save one exposed key
    /// through the "app" handle, and return the temp dir (kept alive),
    /// the db path, and the key id.
    fn seed_vault() -> (tempfile::TempDir, std::path::PathBuf, uuid::Uuid) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.db");
        let mut app = VaultStore::open(&path).unwrap();
        app.set_master_password("master").unwrap();
        let generated = generate_key("agent-key", "", GenerateSpec::Ed25519).unwrap();
        let id = generated.key.id;
        app.save_key(&generated.key, Some(&generated.private_pem)).unwrap();
        (dir, path, id)
    }

    /// A second handle on the same file, unlocked, wrapped as the
    /// production source, mirroring `AgentRuntime::spawn`.
    fn agent_source(path: &std::path::Path) -> VaultKeySource {
        let mut agent = VaultStore::open(path).unwrap();
        agent.unlock("master").unwrap();
        VaultKeySource::new(agent)
    }

    #[test]
    fn exposed_key_lists_and_signs() {
        use signature::Verifier;
        use ssh_encoding::Decode;

        let (_dir, path, _id) = seed_vault();
        let source = agent_source(&path);

        let listed = source.list();
        assert_eq!(listed.len(), 1, "the exposed key is advertised");

        // The advertised blob signs, and the signature verifies against
        // the same public key: the decrypt-at-sign path is real crypto.
        let blob = listed[0].blob.clone();
        let data = b"cross-handle sign";
        let sig_blob = source.sign(&blob, data, SignHash::Default).unwrap();
        let sig = ssh_key::Signature::decode(&mut sig_blob.as_slice()).unwrap();
        let key_data = ssh_key::public::KeyData::decode(&mut blob.as_slice()).unwrap();
        key_data.verify(data, &sig).expect("signature verifies");
    }

    #[test]
    fn expose_toggle_crosses_handles_live() {
        let (_dir, path, id) = seed_vault();
        let source = agent_source(&path);
        assert_eq!(source.list().len(), 1);
        let blob = source.list()[0].blob.clone();

        // Flip the flag through a SEPARATE app handle (the write path
        // the UI uses), then confirm the already-running agent source
        // drops the key on its very next read. This is the guarantee
        // the "Hidden from agent" menu item makes.
        let app = {
            let mut v = VaultStore::open(&path).unwrap();
            v.unlock("master").unwrap();
            v
        };
        let mut key = app.list_keys().unwrap().into_iter().find(|k| k.id == id).unwrap();
        key.expose_via_agent = false;
        app.save_key(&key, None).unwrap();

        assert!(source.list().is_empty(), "hidden key is no longer advertised");
        // The sign path re-checks the flag independently: even a stale
        // blob from a client that listed earlier cannot sign now.
        assert!(
            matches!(source.sign(&blob, b"x", SignHash::Default), Err(AgentSignError::UnknownKey)),
            "a hidden key refuses to sign",
        );

        // Re-expose: the key comes back on the next read, no restart.
        key.expose_via_agent = true;
        app.save_key(&key, None).unwrap();
        assert_eq!(source.list().len(), 1, "re-exposed key is advertised again");
    }

    #[test]
    fn lock_gate_hides_and_restores_roster() {
        let (_dir, path, _id) = seed_vault();
        let source = agent_source(&path);
        assert_eq!(source.list().len(), 1);
        let blob = source.list()[0].blob.clone();

        source.lock();
        assert!(source.list().is_empty(), "a locked vault serves no identities");
        assert!(
            matches!(source.sign(&blob, b"x", SignHash::Default), Err(AgentSignError::Unavailable)),
            "a locked vault cannot sign",
        );

        source.unlock(Some("master"));
        assert_eq!(source.list().len(), 1, "unlock restores the roster");
        assert!(source.sign(&blob, b"x", SignHash::Default).is_ok(), "unlock restores signing");
    }
}
