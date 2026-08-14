use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: Uuid,
    pub label: String,
    pub command: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// Optional free-form group name (snippet "folder"). Name-based on
    /// purpose: it rides sync and portable export as plain data with
    /// no new entity type, and the UIs derive the section list from
    /// the snippets themselves.
    #[serde(default)]
    pub group: Option<String>,
    /// Optional custom hotkey that runs this snippet in a focused
    /// terminal, stored in the app's serialized binding format
    /// ("ctrl+shift+k"). Lives on the snippet itself so deleting the
    /// snippet deletes the shortcut with it, by construction.
    #[serde(default)]
    pub hotkey: Option<String>,
    /// Install script (issue #147): a one-time host setup rather than a
    /// command run often. Drives its own affordances (a confirmation
    /// showing the full body before anything is sent, and the per-host
    /// "already ran here" memory); `#[serde(default)]` so snippets from
    /// older peers and exports read as ordinary commands.
    #[serde(default)]
    pub install: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Snippet {
    pub fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            command: command.into(),
            description: None,
            tags: Vec::new(),
            group: None,
            hotkey: None,
            install: false,
            created_at: now,
            updated_at: now,
        }
    }
}
