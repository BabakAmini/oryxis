use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::login_script::LoginStep;

/// A reusable expect/send login automation, attached to hosts via
/// `Connection.login_script_id`. Mirrors `ProxyIdentity`: same
/// lifecycle (create, edit, delete with cascade null), same reason for
/// being shared rather than per-host, one bastion usually fronts many
/// assets and only the asset id differs between them.
///
/// The per-host half of the configuration does NOT live here: variable
/// values are `Connection.login_script_vars`, and the credential the
/// script types is an encrypted column on the connection. This type is
/// patterns and references only, which is why it can be stored,
/// synced and exported as plain JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoginScript {
    pub id: Uuid,
    pub name: String,
    /// Ordered; see `crate::login_script` for the execution semantics.
    pub steps: Vec<LoginStep>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl LoginScript {
    pub fn new(name: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            steps: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login_script::{ExpectPattern, SecretRef, SendPayload};

    #[test]
    fn new_login_script_defaults() {
        let s = LoginScript::new("jumpserver");
        assert_eq!(s.name, "jumpserver");
        assert!(s.steps.is_empty());
    }

    #[test]
    fn login_script_serialization_roundtrip() {
        let mut s = LoginScript::new("koko");
        s.steps = vec![
            LoginStep {
                expect: Some(ExpectPattern::Suffix("opt>".into())),
                send: SendPayload::Text("{asset}".into()),
                timeout_ms: 0,
                optional: false,
            },
            LoginStep {
                expect: Some(ExpectPattern::Suffix("password:".into())),
                send: SendPayload::Secret(SecretRef::TargetPassword),
                timeout_ms: 15_000,
                optional: false,
            },
        ];
        let json = serde_json::to_string(&s).unwrap();
        let de: LoginScript = serde_json::from_str(&json).unwrap();
        assert_eq!(de, s);
    }
}
