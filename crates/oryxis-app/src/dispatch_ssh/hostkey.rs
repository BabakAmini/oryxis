//! Host-key verification, split out of `dispatch_ssh`: the shared
//! verify / reject / continue / accept-and-save modal answers, plus
//! the no-common-algorithm detection and the legacy-algorithm
//! fallback dialog (cancel / accept-and-expand). Called from
//! `handle_ssh`.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{Message, Oryxis};

impl Oryxis {
    pub(super) fn handle_ssh_hostkey(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::SshHostKeyVerify(query) => {
                self.pending_host_key = Some(query);
                // Bind the responder to this prompt by cloning the staging
                // slot. A connect that starts later overwrites only the
                // staging slot, so the answer the user gives here still goes
                // back to the connect whose host they actually saw. Clone
                // (not take): one connect's bridge is a loop that can raise
                // several host-key prompts (e.g. a jump chain with two
                // unknown hops), and every answer rides the same sender.
                self.active_host_key_tx = self.host_key_response_tx.clone();
            }
            Message::SshHostKeyReject => {
                self.pending_host_key = None;
                if let Some(tx) = self.active_host_key_tx.take() {
                    let _ = tx.try_send(false);
                }
            }
            Message::SshHostKeyContinue => {
                // Accept for this session but don't save to known hosts
                self.pending_host_key = None;
                if let Some(tx) = self.active_host_key_tx.take() {
                    let _ = tx.try_send(true);
                }
            }
            Message::SshHostKeyAcceptAndSave => {
                // Accept and save to known hosts
                if let (Some(query), Some(vault)) = (&self.pending_host_key, &self.vault) {
                    let kh = oryxis_core::models::known_host::KnownHost::new(
                        &query.hostname, query.port, &query.key_type, &query.fingerprint,
                    );
                    let _ = vault.save_known_host(&kh);
                    self.known_hosts = vault.list_known_hosts().unwrap_or_default();
                }
                self.pending_host_key = None;
                if let Some(tx) = self.active_host_key_tx.take() {
                    let _ = tx.try_send(true);
                }
            }
            Message::SshNoCommonAlgo { conn_id, category, server_offers, retry } => {
                // Only offer the fallback when the failed category is still
                // Auto. If it's already pinned (manually, or by a prior
                // accept that expanded everything) and STILL has no match,
                // the server wants an algorithm russh can't provide, so show
                // a plain error instead of looping the dialog.
                let already_pinned = self
                    .connections
                    .iter()
                    .find(|c| c.id == conn_id)
                    .or_else(|| self.quick_connects.get(&conn_id).map(|e| &e.conn))
                    .map(|c| match category {
                        oryxis_ssh::NegCategory::Cipher => c.ciphers.is_some(),
                        oryxis_ssh::NegCategory::Kex => c.kex.is_some(),
                        oryxis_ssh::NegCategory::Mac => c.macs.is_some(),
                        oryxis_ssh::NegCategory::HostKey => c.host_key_algorithms.is_some(),
                    })
                    .unwrap_or(false);
                if already_pinned {
                    if let Some(ref mut progress) = self.connecting {
                        progress.failed = true;
                        progress.logs.push((
                            progress.step,
                            crate::i18n::t("legacy_algo_unsupported").into(),
                        ));
                    }
                } else {
                    // Drop any "busy" backup spinner while the dialog is up so
                    // its retry (SftpBackupConfirm) isn't blocked by the guard.
                    self.sftp_backup.busy = false;
                    self.pending_legacy_algo = Some(crate::state::PendingLegacyAlgo {
                        conn_id,
                        category,
                        server_offers,
                        retry,
                    });
                }
            }
            Message::LegacyAlgoCancel => {
                self.pending_legacy_algo = None;
                let msg = crate::i18n::t("legacy_algo_cancelled");
                if let Some(ref mut progress) = self.connecting {
                    progress.failed = true;
                    progress.logs.push((progress.step, msg.into()));
                }
                // Clear the other paths' transient connecting state so
                // cancelling the dialog never leaves a stuck "busy" backup or
                // a spinning SFTP pane.
                self.sftp_backup.busy = false;
                if self.sftp_backup.open {
                    self.sftp_backup.status = Some(Err(msg.to_string()));
                }
                for side in [
                    crate::state::SftpPaneSide::Left,
                    crate::state::SftpPaneSide::Right,
                ] {
                    let pane = self.sftp.pane_mut(side);
                    if pane.remote_loading {
                        pane.remote_loading = false;
                        pane.error = Some(msg.to_string());
                    }
                }
            }
            Message::LegacyAlgoAccept { remember } => {
                let Some(pending) = self.pending_legacy_algo.take() else {
                    return Ok(Task::none());
                };
                // Expand every category to the full supported set (secure
                // names stay first, so a modern server still negotiates
                // securely). One retry then covers all legacy categories.
                let to_full = |names: Vec<&'static str>| -> Option<Vec<String>> {
                    Some(names.into_iter().map(|s| s.to_string()).collect())
                };
                let expand = |conn: &mut oryxis_core::models::Connection| {
                    // Secure-first order: the default safe set, then the
                    // legacy entries appended. Pinning raw `supported_*`
                    // here would demote chacha/gcm below 3des/cbc.
                    conn.ciphers = to_full(oryxis_ssh::algorithms::expanded_ciphers());
                    conn.kex = to_full(oryxis_ssh::algorithms::expanded_kex());
                    conn.macs = to_full(oryxis_ssh::algorithms::expanded_macs());
                    conn.host_key_algorithms =
                        to_full(oryxis_ssh::algorithms::expanded_host_keys());
                };
                if let Some(idx) =
                    self.connections.iter().position(|c| c.id == pending.conn_id)
                {
                    expand(&mut self.connections[idx]);
                    if remember && let Some(vault) = &self.vault {
                        let _ = vault.save_connection(&self.connections[idx], None);
                    }
                } else if let Some(entry) = self.quick_connects.get_mut(&pending.conn_id) {
                    // Ad-hoc host: expand the in-memory entry only. There
                    // is nothing to remember (the dialog hides the
                    // checkbox for quick connects); the QuickConnect retry
                    // below reuses this mutated entry by id.
                    expand(&mut entry.conn);
                } else {
                    return Ok(Task::none());
                }
                // Re-run the originating connect (terminal / SFTP / forward /
                // backup) now that the in-memory connection carries the
                // expanded algorithm set.
                self.pending_legacy_algo = None;
                return Ok(self.update(*pending.retry));
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
