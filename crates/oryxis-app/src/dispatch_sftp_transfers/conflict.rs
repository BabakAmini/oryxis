//! Answering "that name is taken".
//!
//! Raised from either direction (upload and download both ask now), and
//! answered with Replace / Replace if different / Duplicate / Cancel,
//! optionally sticky for the rest of a batch. The sticky answer is what
//! makes this a state machine rather than a dialog: the runner consults
//! it on every later item instead of asking again.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{SftpMessage, Message, Oryxis};
use crate::sftp_helpers::{
    apply_overwrite_for_download_item, apply_overwrite_for_item, remote_join, unique_name_in_local_dir,
    unique_name_in_remote_dir,
};
use super::SftpSides;

impl Oryxis {
    pub(super) fn handle_sftp_conflict(
        &mut self,
        message: SftpMessage,
        sides: SftpSides,
    ) -> Result<Task<Message>, SftpMessage> {
        let SftpSides { remote: remote_side, local: local_side, owner } = sides;
        match message {
            SftpMessage::SftpAskOverwrite(prompt) => {
                self.sftp.overwrite_prompt = Some(prompt);
            }
            SftpMessage::SftpToggleApplyToAll => {
                if let Some(p) = self.sftp.overwrite_prompt.as_mut() {
                    p.apply_to_all = !p.apply_to_all;
                }
            }
            SftpMessage::SftpResolveOverwrite(action) => {
                let Some(prompt) = self.sftp.overwrite_prompt.take() else {
                    return Ok(Task::none());
                };
                let apply_to_all = prompt.apply_to_all;
                let downloading =
                    prompt.direction == crate::state::OverwriteDirection::Download;
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some(crate::i18n::t("sftp_not_connected").to_string());
                    return Ok(Task::none());
                };
                // Pull a parked transfer item if this prompt fired from
                // inside a queue runner. Two distinct flows hang off
                // here: standalone single-file conflict, and in-transfer
                // multi-file conflict with sticky decisions.
                let (pending_item, pending_slot, slot_count) =
                    self.transfer_slot_mut(owner).and_then(|s| s.state.as_mut()).map_or(
                        (None, None, 0usize),
                        |t| {
                            if apply_to_all {
                                t.overwrite_default = Some(action);
                            }
                            // Resume the worker pool, set paused false
                            // so the resume Next dispatches succeed.
                            t.paused = false;
                            (
                                t.pending_conflict_item.take(),
                                t.pending_conflict_slot.take(),
                                t.busy_slots.len(),
                            )
                        },
                    );
                if let Some(item) = pending_item {
                    if matches!(action, crate::state::OverwriteAction::Cancel) {
                        // Cancel skips this item; with apply-to-all it
                        // also drops the rest of the queue so the user
                        // doesn't keep getting prompted.
                        if apply_to_all
                            && let Some(t) = self.transfer_slot_mut(owner).and_then(|s| s.state.as_mut())
                        {
                            t.queue.clear();
                        }
                        let slot = pending_slot.unwrap_or(0);
                        // Free slot bookkeeping handled by ItemDone.
                        // Also kick a Next per other slot so the rest
                        // of the workers resume from pause.
                        let mut tasks =
                            vec![Task::done(Message::Sftp(SftpMessage::SftpTransferItemDone(owner, slot)))];
                        for _ in 1..slot_count {
                            tasks.push(Task::done(Message::Sftp(SftpMessage::SftpTransferNext(owner))));
                        }
                        return Ok(Task::batch(tasks));
                    }
                    let slot = pending_slot.unwrap_or(0);
                    // Use the slot's own SFTP client for the apply
                    // step; falls back to the original navigation
                    // client only if the slot index is somehow stale.
                    let client = self
                        .transfer_slot_mut(owner)
                        .and_then(|s| s.state.as_ref())
                        .and_then(|t| t.clients.get(slot as usize).cloned())
                        .unwrap_or(client);
                    if let Some(t) = self.transfer_slot_mut(owner).and_then(|s| s.state.as_mut())
                        && (slot as usize) < t.busy_slots.len()
                    {
                        t.busy_slots[slot as usize] = true;
                    }
                    // The apply step writes to whichever side the prompt
                    // came from: an upload lands on the remote host, a
                    // download on the local filesystem. Same continuation
                    // either way (it captures only Copy state, so it is
                    // itself Copy and both arms can use it).
                    let done = move |r: Result<(), String>| match r {
                        Ok(()) => Message::Sftp(SftpMessage::SftpTransferItemDone(owner, slot)),
                        Err(e) => Message::Sftp(SftpMessage::SftpTransferError(owner, e, slot)),
                    };
                    let mut tasks = vec![if downloading {
                        Task::perform(
                            apply_overwrite_for_download_item(client, item, action),
                            done,
                        )
                    } else {
                        Task::perform(apply_overwrite_for_item(client, item, action), done)
                    }];
                    // Resume the other slots that exited on pause.
                    for _ in 1..slot_count {
                        tasks.push(Task::done(Message::Sftp(SftpMessage::SftpTransferNext(owner))));
                    }
                    return Ok(Task::batch(tasks));
                }
                // Standalone (non-queue) conflict: the single-file upload
                // and download paths both land here. Same four answers,
                // applied to whichever side the prompt names.
                if matches!(action, crate::state::OverwriteAction::Cancel)
                    || (matches!(action, crate::state::OverwriteAction::ReplaceIfDifferent)
                        && prompt.src_size == prompt.dst_size)
                {
                    // Same size, assume identical, no-op. The user
                    // explicitly opted into this lazy comparison so we
                    // don't need to hash to be sure.
                    return Ok(Task::none());
                }
                if downloading {
                    let dst_dir = std::path::PathBuf::from(&prompt.dst_dir);
                    let duplicate =
                        matches!(action, crate::state::OverwriteAction::Duplicate);
                    return Ok(Task::perform(
                        async move {
                            let name = if duplicate {
                                unique_name_in_local_dir(&dst_dir, &prompt.basename)
                            } else {
                                prompt.basename.clone()
                            };
                            client
                                .download_to(&prompt.src, &dst_dir.join(name), None)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        move |r| match r {
                            Ok(()) => Message::Sftp(SftpMessage::SftpRefreshLocal(local_side)),
                            Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                        },
                    ));
                }
                let reload = prompt.dst_dir.clone();
                let duplicate = matches!(action, crate::state::OverwriteAction::Duplicate);
                return Ok(Task::perform(
                    async move {
                        let name = if duplicate {
                            unique_name_in_remote_dir(&client, &prompt.dst_dir, &prompt.basename)
                                .await?
                        } else {
                            prompt.basename.clone()
                        };
                        let target = remote_join(&prompt.dst_dir, &name);
                        client
                            .upload_from(std::path::Path::new(&prompt.src), &target)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok::<String, String>(reload)
                    },
                    move |r| match r {
                        Ok(reload) => {
                            Message::Sftp(SftpMessage::SftpNavigateRemote(remote_side, reload))
                        }
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ));
            }
            SftpMessage::SftpTransferConflict(_, prompt, item, slot) => {
                // Park the popped item alongside the prompt so the
                // resolve handler knows which destination the user is
                // about to act on. The queue stays stalled here until
                // the modal is answered.
                if let Some(transfer) = self.transfer_slot_mut(owner).and_then(|s| s.state.as_mut()) {
                    transfer.pending_conflict_item = Some(item);
                    transfer.pending_conflict_slot = Some(slot);
                    transfer.paused = true;
                    if (slot as usize) < transfer.busy_slots.len() {
                        transfer.busy_slots[slot as usize] = false;
                    }
                }
                self.sftp.overwrite_prompt = Some(prompt);
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }
}
