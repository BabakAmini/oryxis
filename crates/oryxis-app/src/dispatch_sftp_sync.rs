//! `impl Oryxis` for the SFTP sync transport: one reconcile round against
//! the shared snapshot file (download, LWW-merge into the vault, rebuild,
//! upload). The merge core lives in `oryxis-sync`
//! (`merge_snapshot` / `build_full_snapshot`); this glue resolves
//! credentials, opens the SFTP channel, and moves the bytes. See
//! `SFTP_SYNC_SPEC.md`.

use std::sync::{Arc, Mutex};

use iced::Task;

use oryxis_ssh::SshEngine;
use oryxis_vault::VaultStore;

use crate::app::{SyncMessage, Message, Oryxis};
use crate::i18n::t;

/// Default snapshot filename appended when the user gives only a directory
/// (a remote path ending in `/`). Keeping it fixed means every device in a
/// group lands on the same file from the same directory.
const DEFAULT_SNAPSHOT_NAME: &str = "oryxis-sync.bin";

/// Split a remote path into `(parent_dir, basename)`. A path with no `/`
/// lives in the SFTP session's default directory, represented as `"."`
/// (every server resolves it to the login home). Used to probe existence
/// via a parent listing rather than a typed not-found error, which
/// russh-sftp collapses into an opaque string.
fn split_remote(remote: &str) -> (String, String) {
    match remote.rsplit_once('/') {
        Some((dir, base)) => {
            let dir = if dir.is_empty() { "/" } else { dir };
            (dir.to_string(), base.to_string())
        }
        None => (".".to_string(), remote.to_string()),
    }
}

impl Oryxis {
    /// Run one SFTP-sync round. Validates config from state, resolves
    /// credentials on the main thread, then performs connect + transfer +
    /// merge off-thread. Returns `Task::none()` (with an inline status) on
    /// any precondition failure, so the caller can fire it blindly.
    pub(crate) fn run_sftp_sync_round(&mut self) -> Task<Message> {
        if self.sync.sftp.in_progress {
            return Task::none();
        }
        // A locked vault has no master key to decrypt with; the
        // manual buttons must not rely on the lock screen hiding them.
        if !self.sync_round_allowed() {
            return Task::none();
        }

        let Some(host_id) = self.sync.sftp.host_id else {
            self.sync.sftp.status = Some(Err(t("sftp_sync_no_host").to_string()));
            return Task::none();
        };
        let Some(mut conn) = self.connections.iter().find(|c| c.id == host_id).cloned() else {
            self.sync.sftp.status = Some(Err(t("sftp_sync_no_host").to_string()));
            return Task::none();
        };
        // Same working copy every connect path dials: group inheritance
        // (D4) and the effective proxy, so the sync host authenticates
        // exactly like a terminal tab to it would.
        self.apply_group_inheritance(&mut conn);
        let remote_input = self.sync.sftp.remote_path.trim();
        if remote_input.is_empty() {
            self.sync.sftp.status = Some(Err(t("sftp_sync_no_path").to_string()));
            return Task::none();
        }
        // Resolve the snapshot FILE path. The user may give just a
        // directory (the common case, ending in `/`): append the default
        // filename so we never read/write the directory itself. A path
        // that already names a file is used as-is (custom filename).
        let remote = if remote_input.ends_with('/') {
            format!("{remote_input}{DEFAULT_SNAPSHOT_NAME}")
        } else {
            remote_input.to_string()
        };
        let Some(vault) = &self.vault else {
            return Task::none();
        };
        // Group key from STORAGE, not the form field: the four snapshot
        // transports share one `sync_sftp_passphrase` row, and a sibling
        // transport's edit can leave this form stale. Sealing with a
        // stale value pushes a snapshot the next session cannot decrypt
        // (see `run_git_sync_round`).
        let passphrase = match vault.get_sync_sftp_passphrase() {
            Ok(Some(p)) => p,
            Ok(None) => {
                self.sync.sftp.status = Some(Err(t("sftp_sync_no_passphrase").to_string()));
                return Task::none();
            }
            Err(e) => {
                self.sync.sftp.status = Some(Err(e.to_string()));
                return Task::none();
            }
        };

        // Round-scoped dedicated vault handle: the snapshot fns need an
        // `Arc<Mutex<VaultStore>>` but the app holds a plain handle. Same
        // pattern as `sync_runtime`; opened inside the task on the same
        // SQLite file (WAL makes concurrent handles safe).
        let db_path = vault.db_path().to_path_buf();
        let master_password = self.master_password.clone();

        // Per-device temp-file tag. The upload writes a temp then renames
        // it over the shared snapshot; if two devices used the SAME temp
        // name their concurrent writes would interleave into one corrupt
        // blob, and every device would then fail to merge it and wedge the
        // sync permanently. A device-unique tag makes each rename install a
        // complete valid snapshot instead (last writer wins, missing edits
        // self-heal next round). The tag is generated once and kept local:
        // it is NOT a synced entity and NOT in the portable-settings
        // whitelist, so it can't collide via a connection that was synced
        // earlier over P2P.
        let device_tag = match vault
            .get_setting("sftp_sync_device_tag")
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
        {
            Some(tag) => tag,
            None => {
                let tag = uuid::Uuid::new_v4().to_string();
                let _ = vault.set_setting("sftp_sync_device_tag", &tag);
                tag
            }
        };

        // Credentials + connect parameters, resolved while we hold `&self`.
        let (password, private_key, certificate) = self.resolve_credentials(&conn);
        // Agent-auth pin (B3), same rule as the tab connect.
        let pinned_agent = self.pinned_agent_public(&conn);
        let totp_secret = self
            .vault
            .as_ref()
            .and_then(|v| v.get_connection_totp_secret(&conn.id).ok().flatten());
        let resolver = self.make_jump_resolver(&conn);
        let host_key_check = self.make_host_key_check();
        let keepalive = self.effective_keepalive(&conn);
        let connect_to = self.sftp_connect_timeout();
        let auth_to = self.sftp_auth_timeout();
        let session_to = self.sftp_session_timeout();
        let op_to = self.sftp_op_timeout();

        // Reuse a live session if a terminal tab already points at this
        // host (skips a second auth + host-key dance). Mirrors the SFTP
        // backup path.
        let existing = self.tabs.iter().find_map(|tb| {
            let base = tb.label.trim_end_matches(" (disconnected)");
            if base == conn.label {
                // SSH handles only: a Telnet tab can't carry SFTP.
                tb.active().session.as_ref().and_then(|s| s.ssh()).cloned()
            } else {
                None
            }
        });

        self.sync.sftp.in_progress = true;
        self.sync.sftp.status = Some(Ok(t("sftp_sync_running").to_string()));

        Task::perform(
            async move {
                // The group-secret derivation first: Argon2id, off the
                // UI thread (it used to run synchronously in the
                // handler, freezing the app for every round).
                let secret = tokio::task::spawn_blocking(move || {
                    oryxis_vault::derive_sync_secret(&passphrase).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("join: {e}")))?;
                // 1. Obtain an SFTP client: reuse an open session, else a
                //    fresh SFTP-only connect with a STRICT host-key check
                //    (no ask channel -> the engine rejects unknown/changed
                //    keys instead of prompting in a background flow).
                let client = if let Some(session) = existing {
                    session.open_sftp().await.map_err(|e| e.to_string())?
                } else {
                    let engine = SshEngine::new()
                        .with_host_key_check(host_key_check)
                        .with_strict_host_key(true)
                        .with_totp_secret(totp_secret.as_deref())
                        .with_keepalive(keepalive)
                        .with_address_family(conn.address_family)
                        .with_rekey_limit_mb(conn.rekey_limit_mb)
                        .with_pinned_agent_key(pinned_agent.as_deref())
                        .with_algorithm_overrides(
                            conn.ciphers.clone(),
                            conn.kex.clone(),
                            conn.macs.clone(),
                            conn.host_key_algorithms.clone(),
                        )
                        .with_connect_timeout(connect_to)
                        .with_auth_timeout(auth_to)
                        .with_session_timeout(session_to);
                    let (session, _rx) = engine
                        .connect_with_resolver(
                            &conn,
                            password.as_deref(),
                            private_key
                                .as_deref()
                                .map(|pem| oryxis_ssh::KeyMaterial::new(pem, certificate.as_deref())),
                            80,
                            24,
                            resolver.as_ref(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    Arc::new(session)
                        .open_sftp()
                        .await
                        .map_err(|e| e.to_string())?
                };
                // Apply the user's configured per-op timeout so a slow
                // listing/transfer follows the same budget as the SFTP
                // browser (the client otherwise defaults to 30s).
                client.set_op_timeout(op_to);

                // 2. Distinguish "file absent" (first run) from a real
                //    error. A successful parent listing proves the link
                //    works; if the basename isn't there, the snapshot is
                //    genuinely absent. A listing error aborts the round so
                //    we never overwrite the remote on a transient failure.
                let (parent, base) = split_remote(&remote);
                let listing = client.list_dir(&parent).await.map_err(|e| e.to_string())?;
                let exists = listing.iter().any(|e| e.name == base);

                // 3. Round-scoped vault handle.
                let mut v = VaultStore::open(&db_path).map_err(|e| e.to_string())?;
                match master_password.as_deref() {
                    Some(pw) => v.unlock(pw).map_err(|e| e.to_string())?,
                    None => v.open_without_password().map_err(|e| e.to_string())?,
                }
                let v = Arc::new(Mutex::new(v));

                // 4. Merge the remote snapshot (if any) into the vault.
                let mut pulled = 0usize;
                if exists {
                    let blob = client.read_file(&remote).await.map_err(|e| e.to_string())?;
                    pulled = oryxis_sync::merge_snapshot(&v, &blob, &secret)
                        .map_err(|e| e.to_string())?;
                }

                // 5. Rebuild the full snapshot from the merged vault and
                //    upload it atomically: write a temp, then replace the
                //    target in one step. Prefer the posix-rename extension
                //    (atomic overwrite). If the server lacks it, fall back
                //    to remove + plain rename; the brief gap falls under
                //    the same read-modify-write self-heal as any sync race
                //    (delay, not loss).
                let snapshot = oryxis_sync::build_full_snapshot(&v, &secret)
                    .map_err(|e| e.to_string())?;
                let tmp = format!("{remote}.tmp.{device_tag}");
                client
                    .write_file(&tmp, &snapshot)
                    .await
                    .map_err(|e| e.to_string())?;
                if client.posix_rename(&tmp, &remote).await.is_err() {
                    let _ = client.remove_file(&remote).await;
                    client
                        .rename(&tmp, &remote)
                        .await
                        .map_err(|e| e.to_string())?;
                }

                Ok::<usize, String>(pulled)
            },
            |res| match res {
                Ok(pulled) => Message::Sync(SyncMessage::SftpDone(Ok(t("sftp_sync_done")
                    .replace("{n}", &pulled.to_string())))),
                Err(e) => Message::Sync(SyncMessage::SftpDone(Err(e))),
            },
        )
    }
}
