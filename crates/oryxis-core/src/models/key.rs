use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKey {
    pub id: Uuid,
    pub label: String,
    pub fingerprint: String,
    pub algorithm: KeyAlgorithm,
    pub public_key: String,
    pub file_ref: String,
    pub has_passphrase: bool,
    /// Whether the Oryxis ssh-agent may serve this key (B1). Defaults
    /// to true so keys are exposed out of the box and old payloads /
    /// DB rows read as exposed (the `keepalive_interval` legacy-default
    /// precedent); the agent feature itself is off by default, so this
    /// only takes effect once the user turns the agent on.
    #[serde(default = "default_true")]
    pub expose_via_agent: bool,
    /// An OpenSSH user certificate for this key (B2): the full
    /// `ssh-ed25519-cert-v01@openssh.com AAAA... comment` public line.
    /// Public material (like `public_key`), so it lives in a plaintext
    /// column. `None` = no certificate. `#[serde(default)]` so old
    /// payloads / rows read as no-cert.
    #[serde(default)]
    pub certificate: Option<String>,
    /// Whether the vault holds private material for this key (B3): a
    /// security-key / public-only row (`import_public_key`) has a NULL
    /// private column and can only authenticate through an external
    /// agent, never as a local `Key` / `Certificate` credential. This is
    /// a LOCAL, computed hint, populated from the DB by `list_keys`, so
    /// it never travels over sync / portable (`#[serde(skip)]`); a
    /// deserialized payload defaults to `false` and the next `list_keys`
    /// sets the real value.
    #[serde(skip)]
    pub has_private: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn default_true() -> bool {
    true
}

impl SshKey {
    pub fn new(label: impl Into<String>, algorithm: KeyAlgorithm) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            fingerprint: String::new(),
            algorithm,
            public_key: String::new(),
            file_ref: String::new(),
            has_passphrase: false,
            expose_via_agent: true,
            certificate: None,
            // A freshly built model has no persisted private yet; the
            // save path attaches one (or not, for public-only imports)
            // and `list_keys` reports the truth on reload.
            has_private: false,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyAlgorithm {
    Ed25519,
    Rsa2048,
    Rsa3072,
    Rsa4096,
    EcdsaP256,
    EcdsaP384,
    EcdsaP521,
    /// FIDO2 security-key algorithms (B3). The private half is a handle
    /// usable only through the authenticator, so these rows carry public
    /// material only (NULL private column) and authenticate via an
    /// external ssh-agent that talks to the hardware.
    SkEd25519,
    SkEcdsaP256,
}

impl KeyAlgorithm {
    /// Whether this is a FIDO2 security-key algorithm: public-only in the
    /// vault, signed by the hardware token through an external agent.
    pub fn is_security_key(&self) -> bool {
        matches!(self, Self::SkEd25519 | Self::SkEcdsaP256)
    }
}

impl std::fmt::Display for KeyAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ed25519 => write!(f, "Ed25519"),
            Self::Rsa2048 => write!(f, "RSA 2048"),
            Self::Rsa3072 => write!(f, "RSA 3072"),
            Self::Rsa4096 => write!(f, "RSA 4096"),
            Self::EcdsaP256 => write!(f, "ECDSA P-256"),
            Self::EcdsaP384 => write!(f, "ECDSA P-384"),
            Self::EcdsaP521 => write!(f, "ECDSA P-521"),
            Self::SkEd25519 => write!(f, "Ed25519-SK"),
            Self::SkEcdsaP256 => write!(f, "ECDSA-SK P-256"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload written before B2 (no `certificate` key) and before B1
    /// (no `expose_via_agent`) must still deserialize, defaulting the new
    /// fields to no-cert and exposed. Mirrors the connection legacy tests.
    #[test]
    fn legacy_payload_defaults_new_fields() {
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "label": "old-key",
            "fingerprint": "SHA256:abc",
            "algorithm": "Ed25519",
            "public_key": "ssh-ed25519 AAAA...",
            "file_ref": "",
            "has_passphrase": false,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let key: SshKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.certificate, None);
        assert!(key.expose_via_agent);
    }

    /// The security-key variants round-trip through serde by name, so
    /// sync / portable payloads carry them without any wire change.
    #[test]
    fn sk_algorithms_serialize_round_trip() {
        for algo in [KeyAlgorithm::SkEd25519, KeyAlgorithm::SkEcdsaP256] {
            let json = serde_json::to_string(&algo).unwrap();
            let back: KeyAlgorithm = serde_json::from_str(&json).unwrap();
            assert_eq!(back, algo);
            assert!(back.is_security_key());
        }
        assert!(!KeyAlgorithm::Ed25519.is_security_key());
    }

    /// A certificate round-trips through serde as a plain string field.
    #[test]
    fn certificate_serializes_round_trip() {
        let mut key = SshKey::new("k", KeyAlgorithm::Ed25519);
        key.certificate = Some("ssh-ed25519-cert-v01@openssh.com AAAA... user@host".to_string());
        let json = serde_json::to_string(&key).unwrap();
        let back: SshKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back.certificate, key.certificate);
    }
}
