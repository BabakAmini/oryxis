use super::*;

/// Cap on saved conversations. Every save prunes the oldest beyond this,
/// so the table cannot grow without bound on a long-lived vault. Generous
/// enough that a heavy user never notices, in the spirit of
/// `command_history`'s per-host cap.
const MAX_CONVERSATIONS: usize = 500;

/// A saved AI conversation, as listed on the History screen. The turns
/// themselves are loaded separately by [`VaultStore::chat_messages`], so a
/// listing never pays to decrypt bodies it will not show.
#[derive(Debug, Clone)]
pub struct ChatConversationEntry {
    pub id: Uuid,
    /// The host this conversation was held next to, or `None` for a local
    /// shell (which has no saved connection).
    pub connection_id: Option<Uuid>,
    /// The recording of the same session, when one was being made. Purely
    /// a correlation: session logging is opt-in per host, so this is
    /// `None` whenever it was off, and the conversation is saved anyway.
    pub session_log_id: Option<Uuid>,
    pub label: String,
    pub provider: String,
    pub model: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Number of turns, so the listing can show a size without loading them.
    pub message_count: usize,
}

/// One saved turn. `content` and `tool_json` come back decrypted; a turn
/// whose payload cannot be opened (a vault restored without its content
/// key) is skipped rather than surfaced as garbage.
#[derive(Debug, Clone)]
pub struct ChatMessageEntry {
    pub role: String,
    pub content: String,
    /// The tool exchange as JSON, for turns that carried one.
    pub tool_json: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl VaultStore {
    /// Create (or refresh) a conversation row. Called when a chat first
    /// produces a turn worth keeping, and again whenever its metadata
    /// changes, so a renamed tab or a switched model stays accurate.
    pub fn upsert_chat_conversation(
        &self,
        id: &Uuid,
        connection_id: Option<&Uuid>,
        session_log_id: Option<&Uuid>,
        label: &str,
        provider: &str,
        model: &str,
    ) -> Result<(), VaultError> {
        let now = Utc::now().to_rfc3339();
        // Keep the original `started_at` on re-save: only the mutable
        // metadata and `updated_at` move.
        self.db.execute(
            "INSERT INTO chat_conversations
                 (id, connection_id, session_log_id, label, provider, model,
                  started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 connection_id  = excluded.connection_id,
                 session_log_id = excluded.session_log_id,
                 label          = excluded.label,
                 provider       = excluded.provider,
                 model          = excluded.model,
                 updated_at     = excluded.updated_at",
            params![
                id.to_string(),
                connection_id.map(|c| c.to_string()),
                session_log_id.map(|s| s.to_string()),
                label,
                provider,
                model,
                now,
            ],
        )?;
        self.prune_chat_conversations()?;
        Ok(())
    }

    /// Append one turn. Both payloads are sealed with the session-log
    /// content key: a chat turn quotes terminal output and command lines,
    /// which is the same secret-bearing material the recording protects.
    pub fn append_chat_message(
        &self,
        conversation_id: &Uuid,
        role: &str,
        content: &str,
        tool_json: Option<&str>,
    ) -> Result<(), VaultError> {
        let content_enc = self.seal_chunk(content.as_bytes())?;
        let tool_enc = match tool_json {
            Some(j) => Some(self.seal_chunk(j.as_bytes())?),
            None => None,
        };
        let now = Utc::now().to_rfc3339();
        self.db.execute(
            "INSERT INTO chat_messages
                 (conversation_id, role, content_enc, tool_enc, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![conversation_id.to_string(), role, content_enc, tool_enc, now],
        )?;
        // The conversation's recency follows its last turn, which is what
        // the listing sorts on.
        self.db.execute(
            "UPDATE chat_conversations SET updated_at = ?2 WHERE id = ?1",
            params![conversation_id.to_string(), now],
        )?;
        Ok(())
    }

    /// Saved conversations, most recently active first.
    pub fn list_chat_conversations(&self) -> Result<Vec<ChatConversationEntry>, VaultError> {
        let mut stmt = self.db.prepare(
            "SELECT c.id, c.connection_id, c.session_log_id, c.label, c.provider,
                    c.model, c.started_at, c.updated_at,
                    (SELECT COUNT(*) FROM chat_messages m
                      WHERE m.conversation_id = c.id)
             FROM chat_conversations c
             ORDER BY c.updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, conn, log, label, provider, model, started, updated, count) = row?;
            // A row whose ids no longer parse is corrupt, not fatal: skip it
            // rather than failing the whole listing.
            let Ok(id) = Uuid::parse_str(&id) else { continue };
            out.push(ChatConversationEntry {
                id,
                connection_id: conn.as_deref().and_then(|c| Uuid::parse_str(c).ok()),
                session_log_id: log.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
                label,
                provider,
                model,
                started_at: parse_ts(&started),
                updated_at: parse_ts(&updated),
                message_count: count.max(0) as usize,
            });
        }
        Ok(out)
    }

    /// The turns of one conversation, in the order they happened.
    pub fn chat_messages(
        &self,
        conversation_id: &Uuid,
    ) -> Result<Vec<ChatMessageEntry>, VaultError> {
        let key = self.session_log_key()?;
        let mut stmt = self.db.prepare(
            "SELECT role, content_enc, tool_enc, created_at
             FROM chat_messages WHERE conversation_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![conversation_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (role, content_enc, tool_enc, created) = row?;
            // A turn that will not open (vault restored without its content
            // key) is dropped rather than rendered as garbage.
            let Some(content) = Self::unseal_chunk(&key, &content_enc) else {
                continue;
            };
            let Ok(content) = String::from_utf8(content) else { continue };
            let tool_json = tool_enc
                .as_deref()
                .and_then(|b| Self::unseal_chunk(&key, b))
                .and_then(|b| String::from_utf8(b).ok());
            out.push(ChatMessageEntry {
                role,
                content,
                tool_json,
                created_at: parse_ts(&created),
            });
        }
        Ok(out)
    }

    /// Delete one conversation and its turns.
    pub fn delete_chat_conversation(&self, id: &Uuid) -> Result<(), VaultError> {
        self.db.execute(
            "DELETE FROM chat_messages WHERE conversation_id = ?1",
            params![id.to_string()],
        )?;
        self.db.execute(
            "DELETE FROM chat_conversations WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// Drop every saved conversation (the History screen's clear-all).
    pub fn clear_chat_conversations(&self) -> Result<(), VaultError> {
        self.db.execute_batch(
            "DELETE FROM chat_messages; DELETE FROM chat_conversations;",
        )?;
        Ok(())
    }

    /// Conversations belonging to a host, used when a connection is deleted
    /// so its chats go with it instead of dangling.
    pub fn delete_chat_conversations_for_connection(
        &self,
        connection_id: &Uuid,
    ) -> Result<(), VaultError> {
        self.db.execute(
            "DELETE FROM chat_messages WHERE conversation_id IN
                 (SELECT id FROM chat_conversations WHERE connection_id = ?1)",
            params![connection_id.to_string()],
        )?;
        self.db.execute(
            "DELETE FROM chat_conversations WHERE connection_id = ?1",
            params![connection_id.to_string()],
        )?;
        Ok(())
    }

    /// Keep the newest [`MAX_CONVERSATIONS`], dropping the least recently
    /// active first (and their turns with them).
    fn prune_chat_conversations(&self) -> Result<(), VaultError> {
        self.db.execute(
            "DELETE FROM chat_messages WHERE conversation_id IN (
                 SELECT id FROM chat_conversations
                 ORDER BY updated_at DESC LIMIT -1 OFFSET ?1)",
            params![MAX_CONVERSATIONS as i64],
        )?;
        self.db.execute(
            "DELETE FROM chat_conversations WHERE id IN (
                 SELECT id FROM chat_conversations
                 ORDER BY updated_at DESC LIMIT -1 OFFSET ?1)",
            params![MAX_CONVERSATIONS as i64],
        )?;
        Ok(())
    }
}

/// Parse a stored RFC3339 stamp, falling back to the epoch so one bad row
/// cannot fail a whole listing.
fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| DateTime::UNIX_EPOCH)
}
