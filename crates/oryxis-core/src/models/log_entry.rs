use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: Uuid,
    pub connection_label: String,
    pub hostname: String,
    pub event: LogEvent,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogEvent {
    Connected,
    Disconnected,
    AuthFailed,
    Error,
    /// A sync peer OVERWROTE something that decides where or how a
    /// connection is made (its address, its proxy, a group default, a
    /// known host, an auto-starting forward, a login script).
    ///
    /// Only overwrites, never the peer's new entities: replication is
    /// the feature, and a first sync would otherwise write a line per
    /// host. What has no other trace is a route that was already
    /// working and now points somewhere else, decided on another
    /// machine, applied here with no prompt and no visible change
    /// beyond a counter.
    SyncApplied,
}

impl std::fmt::Display for LogEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected => write!(f, "Connected"),
            Self::Disconnected => write!(f, "Disconnected"),
            Self::AuthFailed => write!(f, "Auth Failed"),
            Self::Error => write!(f, "Error"),
            Self::SyncApplied => write!(f, "Sync Applied"),
        }
    }
}

impl LogEntry {
    pub fn new(label: &str, hostname: &str, event: LogEvent, message: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            connection_label: label.into(),
            hostname: hostname.into(),
            event,
            message: message.into(),
            timestamp: chrono::Utc::now(),
        }
    }
}
