//! Per-device approval of command proxies.
//!
//! A `ProxyType::Command` proxy is the one piece of connection data that
//! is config-as-code: the engine spawns it locally, with the user's
//! privileges, BEFORE the SSH handshake, so it runs whether or not the
//! target is reachable and whether or not its host key checks out. Every
//! other proxy kind is an address.
//!
//! That matters because connection data ARRIVES from places the local
//! user never typed: a sync peer's `apply_records`, a `.oryxis` import,
//! a group default resolved onto a host that has no proxy of its own.
//! All three write the vault verbatim, all three carry a peer-chosen
//! `updated_at` that wins last-writer-wins, and none of them is a
//! decision the person in front of this machine made. Replicating the
//! DATA is right; letting it EXECUTE on arrival is not.
//!
//! So the command travels like any other field, and the execution is
//! what asks. This table is that answer, and it is deliberately:
//!
//! - **Local only.** No `EntityType`, no portable-export category. An
//!   approval means "I, on this machine, accept running this"; a synced
//!   approval would carry the decision to a device whose owner never
//!   made it, which is the exact hole being closed.
//! - **Keyed by the command's fingerprint**, not by host or identity.
//!   The string is what runs (verbatim, no `%h`/`%p` substitution), so
//!   the string is what gets approved; the same line under a second host
//!   is the same process and needs no second prompt, while one edited
//!   character is a different process and needs a new one.
//! - **Never storing the line itself.** A command proxy can embed
//!   credentials, which is why the connect log prints only its type.
//!   `label` is the host the approval was granted from, enough for a
//!   revocation list to be meaningful without a second copy of the
//!   secret.

use super::*;
use oryxis_core::models::connection::proxy_command_fingerprint;

/// One granted approval, for the revocation list.
#[derive(Debug, Clone)]
pub struct TrustedProxyCommand {
    /// SHA-256 of the approved command line.
    pub fingerprint: String,
    /// Where the approval was granted (host label), for display.
    pub label: String,
    pub trusted_at: chrono::DateTime<chrono::Utc>,
}

impl VaultStore {
    /// Record that this device accepts running `command`.
    ///
    /// Called from the two places a local human actually decides: the
    /// consent modal raised by a dial, and the editors/importers where
    /// the user authored the line themselves (typing a ProxyCommand into
    /// the host editor IS the approval; prompting for it on the next
    /// dial would be theatre).
    pub fn trust_proxy_command(&self, command: &str, label: &str) -> Result<(), VaultError> {
        self.db.execute(
            "INSERT OR REPLACE INTO trusted_proxy_commands (fingerprint, label, trusted_at)
             VALUES (?1, ?2, ?3)",
            params![
                proxy_command_fingerprint(command),
                label,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Whether this device has approved running `command`.
    ///
    /// A read failure answers `false`. This gates a local process spawn,
    /// so the only safe direction to fail in is "not approved": a vault
    /// that cannot answer must not be read as consent.
    pub fn is_proxy_command_trusted(&self, command: &str) -> bool {
        let fingerprint = proxy_command_fingerprint(command);
        self.db
            .query_row(
                "SELECT 1 FROM trusted_proxy_commands WHERE fingerprint = ?1",
                params![fingerprint],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Withdraw one approval. The next dial that needs it asks again.
    pub fn forget_proxy_command(&self, fingerprint: &str) -> Result<(), VaultError> {
        self.db.execute(
            "DELETE FROM trusted_proxy_commands WHERE fingerprint = ?1",
            params![fingerprint],
        )?;
        Ok(())
    }

    /// Every approval this device has granted, newest first.
    pub fn list_trusted_proxy_commands(&self) -> Result<Vec<TrustedProxyCommand>, VaultError> {
        let mut stmt = self.db.prepare(
            "SELECT fingerprint, label, trusted_at FROM trusted_proxy_commands
             ORDER BY trusted_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TrustedProxyCommand {
                fingerprint: row.get(0)?,
                label: row.get(1)?,
                trusted_at: row
                    .get::<_, String>(2)
                    .ok()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(chrono::Utc::now),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
