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
