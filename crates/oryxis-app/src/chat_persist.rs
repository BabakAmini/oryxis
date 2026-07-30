//! Saving AI conversations to the vault.
//!
//! The chat sidebar keeps its turns in the tab, which means they die with
//! it. This flushes them into `chat_conversations` / `chat_messages` as
//! they settle, so past conversations can be read back on the History
//! screen (issue #105 asked where they were).
//!
//! Append-only and incremental: each flush writes the turns the vault has
//! not seen yet, so a long conversation never rewrites what is already
//! stored. Flushes happen where the history is STABLE (a reply finished, a
//! tool call resolved), never mid-stream, so a half-streamed answer is
//! never what gets saved.

use crate::app::Oryxis;
use crate::state::{ChatMessage, ChatRole};

impl Oryxis {
    /// Persist whatever turns of `tabs[idx]`'s conversation the vault does
    /// not have yet. Cheap and idempotent, so callers can fire it at every
    /// settle point without checking whether anything changed.
    pub(crate) fn flush_chat_history(&mut self, idx: usize) {
        // Read everything out first. The vault is owned by `self`, so
        // holding a borrow of it while touching `self.tabs` would not
        // compile; collecting owned turns keeps the two phases apart.
        let Some(tab) = self.tabs.get(idx) else { return };

        // Only turns worth reading back. A pending prompt has not happened
        // yet, and an empty assistant bubble is the placeholder a stream
        // fills in, so neither is a turn.
        let savable: Vec<SavedTurn> = tab
            .chat_history
            .iter()
            .filter(|m| is_savable(m))
            .map(SavedTurn::from)
            .collect();
        if savable.is_empty() {
            return;
        }

        // The history can shrink (a popped placeholder or pending bubble),
        // which would leave the cursor past the end and silently skip
        // turns. Rewrite from scratch in that case: it is rare, and
        // correctness beats saving a few inserts.
        let mut start = tab.chat_persisted;
        let restart = start > savable.len();
        if restart {
            start = 0;
        }
        if start == savable.len() {
            return; // nothing new
        }

        let conversation_id = tab.chat_saved_id.unwrap_or_else(uuid::Uuid::new_v4);
        let pane = tab.active();
        // A quick-connect host lives in an in-memory store that is gone
        // next launch, so only a saved connection is worth referencing;
        // everything else (local shell, cloud exec) records as hostless.
        let connection_id = match pane.origin {
            crate::state::PaneOrigin::Host(id) => Some(id),
            _ => None,
        };
        let session_log_id = pane.session_log_id;
        // The tab's own name, not the OSC title: a saved conversation wants
        // the host it happened on, not whichever program was running when
        // the last turn landed.
        let label = tab
            .custom_name
            .clone()
            .unwrap_or_else(|| tab.label.clone());
        let provider = self.ai.provider.clone();
        let model = self.ai.model.clone();
        let total = savable.len();

        let Some(vault) = self.vault.as_ref() else { return };
        if restart {
            // Replace the turns; the conversation keeps its identity.
            let _ = vault.delete_chat_conversation(&conversation_id);
        }
        if vault
            .upsert_chat_conversation(
                &conversation_id,
                connection_id.as_ref(),
                session_log_id.as_ref(),
                &label,
                &provider,
                &model,
            )
            .is_err()
        {
            return;
        }
        for turn in &savable[start..] {
            let _ = vault.append_chat_message(
                &conversation_id,
                turn.role,
                &turn.content,
                turn.tool_json.as_deref(),
            );
        }

        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.chat_saved_id = Some(conversation_id);
            tab.chat_persisted = total;
        }
    }

    /// Forget the saved conversation for a tab whose chat was just reset,
    /// so the next exchange starts a fresh row instead of appending to the
    /// conversation the user cleared.
    ///
    /// The stored rows are deliberately LEFT ALONE: "reset" clears the live
    /// context sent to the model, and silently deleting a saved
    /// conversation is a different, destructive act the user did not ask
    /// for. The History screen keeps its own delete.
    pub(crate) fn detach_saved_chat(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.chat_saved_id = None;
            tab.chat_persisted = 0;
        }
    }
}

/// One turn lifted out of the live history, owned so the write phase does
/// not hold a borrow of `self.tabs`.
struct SavedTurn {
    role: &'static str,
    content: String,
    tool_json: Option<String>,
}

impl From<&ChatMessage> for SavedTurn {
    fn from(m: &ChatMessage) -> Self {
        Self {
            role: role_name(&m.role),
            content: m.content.clone(),
            // The tool exchange rides as JSON so the saved view can show
            // the command and its output the way the live bubble does.
            tool_json: m.tool.as_ref().map(|t| {
                serde_json::json!({
                    "command": t.command,
                    "risk": t.risk,
                    "output": t.output,
                })
                .to_string()
            }),
        }
    }
}

/// Turns worth reading back later.
///
/// `PendingTool` is a question, not something that happened; an empty
/// `Assistant` bubble is the placeholder a stream is about to fill. Errors
/// ARE kept: "it failed here" is exactly what someone re-reading a session
/// wants to see.
fn is_savable(m: &ChatMessage) -> bool {
    match m.role {
        ChatRole::PendingTool => false,
        ChatRole::Assistant => !m.content.is_empty(),
        _ => true,
    }
}

/// Stable on-disk name for a role. Spelled out rather than derived from
/// `Debug` so renaming the enum cannot silently change stored data.
fn role_name(role: &ChatRole) -> &'static str {
    match role {
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::System => "system",
        ChatRole::Error => "error",
        ChatRole::PendingTool => "pending",
        ChatRole::Tool => "tool",
    }
}
