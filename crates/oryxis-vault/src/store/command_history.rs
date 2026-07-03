use super::*;

/// Per-host cap. On every record the host's history is pruned back to this
/// many rows, dropping the least-recently-used first. Generous enough that a
/// heavy user never notices, small enough that the table stays trivial.
const MAX_PER_HOST: usize = 200;

impl VaultStore {
    // -----------------------------------------------------------------------
    // Command history (terminal sidebar History tab). Local-only: no sync,
    // no portable export, cascaded away when the host is deleted.
    // -----------------------------------------------------------------------

    /// Record one execution of `command` on a host: bumps the frequency
    /// counter of the existing row or inserts a fresh one, then prunes the
    /// host back under the per-host cap.
    pub fn record_command(&self, connection_id: &Uuid, command: &str) -> Result<(), VaultError> {
        let now = Utc::now().to_rfc3339();
        let updated = self.db.execute(
            "UPDATE command_history SET use_count = use_count + 1, last_used_at = ?3
             WHERE connection_id = ?1 AND command = ?2",
            params![connection_id.to_string(), command, now],
        )?;
        if updated == 0 {
            self.db.execute(
                "INSERT INTO command_history (id, connection_id, command, use_count, last_used_at, created_at)
                 VALUES (?1, ?2, ?3, 1, ?4, ?4)",
                params![Uuid::new_v4().to_string(), connection_id.to_string(), command, now],
            )?;
            self.db.execute(
                "DELETE FROM command_history WHERE connection_id = ?1 AND id NOT IN (
                     SELECT id FROM command_history WHERE connection_id = ?1
                     ORDER BY last_used_at DESC LIMIT ?2
                 )",
                params![connection_id.to_string(), MAX_PER_HOST as i64],
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
        let mut stmt = self.db.prepare(
            "SELECT id, connection_id, command, use_count, last_used_at
             FROM command_history WHERE connection_id = ?1
             ORDER BY last_used_at DESC",
        )?;
        let rows = stmt.query_map(params![connection_id.to_string()], |row| {
            Ok(CommandHistoryEntry {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_default(),
                connection_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                command: row.get(2)?,
                use_count: row.get(3)?,
                last_used_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
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
