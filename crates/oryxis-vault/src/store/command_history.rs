use super::*;

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Per-host cap. On every record the host's history is pruned back to this
/// many rows, dropping the least-recently-used first. Generous enough that a
/// heavy user never notices, small enough that the table stays trivial.
const MAX_PER_HOST: usize = 200;

/// Domain-separation context for the dedup hash, so the content key's use
/// as an HMAC key here can never collide with another derivation.
const DEDUP_CONTEXT: &[u8] = b"oryxis.command_history.dedup.v1";

/// Keyed dedup token for a (host, command) pair: HMAC-SHA256 under the
/// content key, hex-encoded. Keyed so an attacker holding only the vault
/// file cannot confirm guesses of common commands offline; scoped to the
/// connection so the same command on two hosts yields unrelated tokens.
fn command_hash(key: &[u8; KEY_LEN], connection_id: &str, command: &str) -> String {
    // HMAC accepts any key length, so new_from_slice can't fail. The
    // fully-qualified call disambiguates from the `chacha20poly1305`
    // KeyInit blanket impl that `use super::*` drags into scope.
    let mut mac =
        <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(DEDUP_CONTEXT);
    mac.update(&[0x1f]);
    mac.update(connection_id.as_bytes());
    mac.update(&[0x1f]);
    mac.update(command.as_bytes());
    let tag = mac.finalize().into_bytes();
    let mut out = String::with_capacity(tag.len() * 2);
    for b in tag {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        out.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap());
    }
    out
}

impl VaultStore {
    // -----------------------------------------------------------------------
    // Command history (terminal sidebar History tab). Local-only: no sync,
    // no portable export, cascaded away when the host is deleted.
    //
    // At-rest layout: the command text is sealed with the content key in
    // `command_enc` (same treatment as session-recording chunks, and for
    // the same reason: an ECHOED inline secret passes the capture gates
    // by definition, so the text is credential-adjacent). The plaintext
    // `command` column carries the keyed dedup hash instead, which keeps
    // the unique index and the bump-in-place UPDATE working.
    // -----------------------------------------------------------------------

    /// Record one execution of `command` on a host: bumps the frequency
    /// counter of the existing row or inserts a fresh sealed one, then
    /// prunes the host back under the per-host cap. Requires the vault to
    /// be unlocked (the text is encrypted); callers treat
    /// [`VaultError::Locked`] as "recording paused", not a failure.
    pub fn record_command(&self, connection_id: &Uuid, command: &str) -> Result<(), VaultError> {
        let key = self.session_log_key()?;
        self.migrate_plaintext_command_history(&key)?;
        let conn_str = connection_id.to_string();
        let hash = command_hash(&key, &conn_str, command);
        let now = Utc::now().to_rfc3339();
        let updated = self.db.execute(
            "UPDATE command_history SET use_count = use_count + 1, last_used_at = ?3
             WHERE connection_id = ?1 AND command = ?2",
            params![conn_str, hash, now],
        )?;
        if updated == 0 {
            let sealed = self.seal_chunk(command.as_bytes())?;
            self.db.execute(
                "INSERT INTO command_history
                     (id, connection_id, command, command_enc, use_count, last_used_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
                params![Uuid::new_v4().to_string(), conn_str, hash, sealed, now],
            )?;
            self.db.execute(
                "DELETE FROM command_history WHERE connection_id = ?1 AND id NOT IN (
                     SELECT id FROM command_history WHERE connection_id = ?1
                     ORDER BY last_used_at DESC LIMIT ?2
                 )",
                params![conn_str, MAX_PER_HOST as i64],
            )?;
        }
        Ok(())
    }

    /// A host's history ordered most-recently-used first. The caller derives
    /// the "frequent" shortlist by re-sorting a copy on `use_count`.
    pub fn list_command_history(
        &self,
        connection_id: &Uuid,
    ) -> Result<Vec<CommandHistoryEntry>, VaultError> {
        let key = self.session_log_key().ok();
        if let Some(k) = key.as_ref() {
            self.migrate_plaintext_command_history(k)?;
        }
        let mut stmt = self.db.prepare(
            "SELECT id, connection_id, command, command_enc, use_count, last_used_at
             FROM command_history WHERE connection_id = ?1
             ORDER BY last_used_at DESC",
        )?;
        let rows = stmt.query_map(params![connection_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let Ok((id, conn, cmd_col, enc, use_count, last_used)) = row else {
                continue;
            };
            let command = match &enc {
                // Sealed row: open with the content key. A row that
                // doesn't open (locked vault, foreign key) is skipped;
                // surfacing the dedup hash would be garbage, and the
                // sidebar list simply shows what is readable.
                Some(blob) => match key
                    .as_ref()
                    .and_then(|k| Self::unseal_chunk(k, blob))
                    .and_then(|plain| String::from_utf8(plain).ok())
                {
                    Some(text) => text,
                    None => continue,
                },
                // Pre-migration row: `command` still holds the plaintext.
                None => cmd_col,
            };
            entries.push(CommandHistoryEntry {
                id: Uuid::parse_str(&id).unwrap_or_default(),
                connection_id: Uuid::parse_str(&conn).unwrap_or_default(),
                command,
                use_count,
                last_used_at: chrono::DateTime::parse_from_rfc3339(&last_used)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(entries)
    }

    /// Search every host's command history for a case-insensitive
    /// substring. Returns at most one `(connection_id, command)` pair
    /// per host (its most recently used match), so the History content
    /// search can answer "which hosts ran this command" even for hosts
    /// with no recorded sessions. Bounded by the per-host cap, so the
    /// full-table decrypt stays trivial. Rows that don't open (locked
    /// vault) are skipped like in [`Self::list_command_history`].
    ///
    /// Case-folding note: this tier (and `search_session_commands`)
    /// matches with full Unicode `to_lowercase()`, while the app's
    /// output tier (`content_match_snippet`) folds 1:1 per char so
    /// its excerpt offsets stay aligned with the rendered text.
    /// Needles with multi-char foldings (İ, ß) can therefore match
    /// here but not there. Intentional divergence, documented at
    /// both sites.
    pub fn search_command_history(
        &self,
        needle: &str,
    ) -> Result<Vec<(Uuid, String)>, VaultError> {
        let needle = needle.to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let key = self.session_log_key().ok();
        if let Some(k) = key.as_ref() {
            self.migrate_plaintext_command_history(k)?;
        }
        let mut stmt = self.db.prepare(
            "SELECT connection_id, command, command_enc FROM command_history
             ORDER BY last_used_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?;
        let mut matches: Vec<(Uuid, String)> = Vec::new();
        let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for row in rows {
            let Ok((conn, cmd_col, enc)) = row else {
                continue;
            };
            let Ok(conn) = Uuid::parse_str(&conn) else {
                continue;
            };
            if seen.contains(&conn) {
                continue;
            }
            let command = match &enc {
                Some(blob) => match key
                    .as_ref()
                    .and_then(|k| Self::unseal_chunk(k, blob))
                    .and_then(|plain| String::from_utf8(plain).ok())
                {
                    Some(text) => text,
                    None => continue,
                },
                // Pre-migration row: `command` still holds the plaintext.
                None => cmd_col,
            };
            if command.to_lowercase().contains(&needle) {
                seen.insert(conn);
                matches.push((conn, command));
            }
        }
        Ok(matches)
    }

    /// One-shot upgrade of rows written before command encryption existed
    /// (plaintext `command`, NULL `command_enc`): seal the text with the
    /// content key and replace `command` with the keyed dedup hash, so
    /// the plaintext leaves the database. Runs before every record/list
    /// while unlocked; the `AtomicBool` caches "nothing pending" for this
    /// open, and a row that fails is logged and retried on the next open
    /// rather than blocking the write. Migration order matters: it runs
    /// before the dedup UPDATE in [`Self::record_command`], so a legacy
    /// plaintext row is hashed before the new record could duplicate it.
    fn migrate_plaintext_command_history(&self, key: &[u8; KEY_LEN]) -> Result<(), VaultError> {
        use std::sync::atomic::Ordering;
        if self.command_history_migrated.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut stmt = self.db.prepare(
            "SELECT id, connection_id, command FROM command_history WHERE command_enc IS NULL",
        )?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let migrated = !rows.is_empty();
        // Only cache "nothing pending" for this open once every row is
        // sealed. A row left behind (transient write failure) must be
        // retried on the next call, or its plaintext sits at rest forever.
        let mut all_sealed = true;
        for (id, conn, cmd) in rows {
            let hash = command_hash(key, &conn, &cmd);
            let sealed = self.seal_chunk(cmd.as_bytes())?;
            if let Err(e) = self.db.execute(
                "UPDATE command_history SET command = ?2, command_enc = ?3 WHERE id = ?1",
                params![id, hash, sealed],
            ) {
                // A sealed sibling already carries this command (its
                // `command` is the same keyed hash), so the UPDATE trips
                // UNIQUE(connection_id, command). The plaintext row is now
                // redundant: fold its use_count into the sibling and drop
                // it. Leaving it would keep the plaintext at rest AND
                // re-collide on every future open (a permanent leak).
                let folded = self
                    .db
                    .execute(
                        "UPDATE command_history SET use_count = use_count + \
                         (SELECT use_count FROM command_history WHERE id = ?1) \
                         WHERE connection_id = ?2 AND command = ?3 AND id != ?1",
                        params![id, conn, hash],
                    )
                    .unwrap_or(0);
                if folded > 0 {
                    let _ = self.db.execute(
                        "DELETE FROM command_history WHERE id = ?1",
                        params![id],
                    );
                } else {
                    tracing::warn!("command-history: migrating row {id} failed: {e}");
                    all_sealed = false;
                }
            }
        }
        if migrated && all_sealed {
            // The overwritten plaintext lingers on SQLite free pages
            // until they are reused; compact once so the migrated
            // secrets actually leave the file (same rationale as the
            // VACUUM in `destroy_and_recreate`). Best-effort: a busy
            // database (second sync-engine handle mid-write) just
            // skips it, the rows themselves are already sealed.
            if let Err(e) = self.db.execute_batch("VACUUM") {
                tracing::warn!("command-history: post-migration VACUUM skipped: {e}");
            }
            tracing::info!("command-history: legacy plaintext rows sealed");
        }
        if all_sealed {
            self.command_history_migrated.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Delete one history entry (the row's own id, not the host's).
    pub fn delete_command_history_entry(&self, id: &Uuid) -> Result<(), VaultError> {
        self.db.execute(
            "DELETE FROM command_history WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// Drop a host's entire history (host deletion cascade, privacy clear).
    pub fn clear_command_history(&self, connection_id: &Uuid) -> Result<(), VaultError> {
        self.db.execute(
            "DELETE FROM command_history WHERE connection_id = ?1",
            params![connection_id.to_string()],
        )?;
        Ok(())
    }
}
