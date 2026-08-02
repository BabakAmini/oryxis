//! `Oryxis::handle_sftp_transfers`, match arms for the SFTP transfer
//! pipeline: single + batch + folder uploads/downloads/duplicates,
//! conflict resolution, OS-level file drop, queue lifecycle (slots,
//! retry, error reporting, cancel). Pulled out of `dispatch_sftp.rs`
//! since the queue runner is genuinely a different subsystem from the
//! navigation/listing arms.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{SftpMessage, Message, Oryxis};
use crate::sftp_helpers::{
    apply_overwrite_for_download_item, apply_overwrite_for_item, build_client_pool,
    do_download_item, do_local_duplicate_item,
    destinations_are_one_directory, do_relay_item, do_upload_item, parent_path,
    relay_target_is_inside_source, remote_cp, remote_join, remove_moved_sources,
    transfer_item_label, unique_name_in_local_dir,
    unique_name_in_remote_dir, walk_local_for_duplicate, walk_local_for_upload,
    walk_remote_for_download, walk_remote_for_relay, TransferStepOutcome, UploadOutcome,
};
use crate::state::SftpPaneSide;

impl Oryxis {
    /// Folder picker for a download destination, seeded at the local
    /// pane's current directory. Returns the task that asks and then
    /// replays `then`; `None` means "no need to ask, go ahead".
    ///
    /// Called at the top of every download entry point, gated on the
    /// `sftp_ask_download_dir` setting. The `download_dest_override`
    /// check is what stops the replay from asking a second time: the
    /// picker sets it, the handler consumes it.
    pub(crate) fn sftp_ask_download_dir(
        &self,
        then: SftpMessage,
    ) -> Option<Task<Message>> {
        if !self.setting_sftp_ask_download_dir || self.sftp.download_dest_override.is_some() {
            return None;
        }
        Some(self.sftp_pick_download_dir(then))
    }

    /// Unconditional version, behind the row menu's "Download to...".
    pub(crate) fn sftp_pick_download_dir(&self, then: SftpMessage) -> Task<Message> {
        let start = self
            .sftp
            .local_side()
            .map(|s| self.sftp.pane(s).local_path.clone());
        Task::perform(
            async move {
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_title(crate::i18n::t("sftp_download_to"));
                if let Some(dir) = start {
                    dialog = dialog.set_directory(dir);
                }
                dialog.pick_folder().await.map(|f| f.path().to_path_buf())
            },
            move |dir| {
                Message::Sftp(SftpMessage::SftpDownloadDestPicked(
                    dir,
                    Box::new(then.clone()),
                ))
            },
        )
    }

    pub(crate) fn handle_sftp_transfers(
        &mut self,
        message: SftpMessage,
    ) -> Result<Task<Message>, SftpMessage> {
        // The remote destination/source side for upload/download. Both
        // paths only run with exactly one remote pane, so this resolves
        // unambiguously. Default to Right for state mutations when no
        // remote pane exists (the early returns below short-circuit).
        let remote_side = self.sftp.remote_side().unwrap_or(SftpPaneSide::Right);
        let local_side = self.sftp.local_side().unwrap_or(SftpPaneSide::Left);
        // Owning SFTP tab for any continuation message this handler emits. For
        // a user-initiated transfer this is the focused tab; for a routed
        // continuation (`route_sftp_async`) it is the originating tab. Captured
        // by the async result closures so the chain stays pinned to one tab.
        // No active SFTP tab means none of this handler's messages apply, so
        // DECLINE (Err) to pass the message down the dispatch chain. Returning
        // Ok here would swallow every message (this is the first handler in the
        // chain), freezing the whole app whenever no SFTP tab is open.
        let Some(owner) = self.current_sftp_owner() else {
            return Err(message);
        };
        match message {
            SftpMessage::SftpUpload(local_path) => {
                self.sftp.row_menu = None;
                if self.sftp_upload_blocked_by_zip(remote_side) {
                    return Ok(Task::none());
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                };
                let remote_dir = self
                    .sftp
                    .upload_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(remote_side).remote_path.clone());
                return Ok(Task::perform(
                    async move {
                        let basename = local_path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .ok_or_else(|| "invalid filename".to_string())?
                            .to_string();
                        let entries = client
                            .list_dir(&remote_dir)
                            .await
                            .map_err(|e| e.to_string())?;
                        // Existence check up front: hand back to the
                        // user via overwrite modal if the name is taken,
                        // otherwise stream the file and finish silently.
                        let conflict = entries.iter().find(|e| e.name == basename);
                        if let Some(existing) = conflict {
                            let src_size = tokio::fs::metadata(&local_path)
                                .await
                                .map(|m| m.len())
                                .unwrap_or(0);
                            return Ok::<UploadOutcome, String>(UploadOutcome::Conflict(
                                crate::state::OverwritePrompt {
                                    src: local_path.to_string_lossy().into_owned(),
                                    dst_dir: remote_dir,
                                    basename,
                                    src_size,
                                    dst_size: existing.size,
                                    direction: crate::state::OverwriteDirection::Upload,
                                    multi: false,
                                    apply_to_all: false,
                                },
                            ));
                        }
                        let target = remote_join(&remote_dir, &basename);
                        client
                            .upload_from(&local_path, &target)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(UploadOutcome::Done(remote_dir))
                    },
                    move |result| match result {
                        Ok(UploadOutcome::Done(reload)) => {
                            Message::Sftp(SftpMessage::SftpNavigateRemote(remote_side, reload))
                        }
                        Ok(UploadOutcome::Conflict(prompt)) => Message::Sftp(SftpMessage::SftpAskOverwrite(prompt)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ));
            }
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
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                };
                // Pull a parked transfer item if this prompt fired from
                // inside a queue runner. Two distinct flows hang off
                // here: standalone single-file conflict, and in-transfer
                // multi-file conflict with sticky decisions.
                let (pending_item, pending_slot, slot_count) =
                    self.sftp.transfer.as_mut().map_or(
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
                            && let Some(t) = self.sftp.transfer.as_mut()
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
                        .sftp
                        .transfer
                        .as_ref()
                        .and_then(|t| t.clients.get(slot as usize).cloned())
                        .unwrap_or(client);
                    if let Some(t) = self.sftp.transfer.as_mut()
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
            SftpMessage::SftpDownload(remote_path) => {
                self.sftp.row_menu = None;
                if let Some(ask) = self.sftp_ask_download_dir(SftpMessage::SftpDownload(
                    remote_path.clone(),
                )) {
                    return Ok(ask);
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                };
                let local_dir = self
                    .sftp
                    .download_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(local_side).local_path.clone());
                return Ok(Task::perform(
                    async move {
                        let basename = remote_path
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or(&remote_path)
                            .to_string();
                        let target = local_dir.join(&basename);
                        // A name already taken is the user's call, not
                        // ours: same four answers the upload side offers,
                        // including Duplicate, which is what this path
                        // used to do silently.
                        if let Ok(existing) = tokio::fs::metadata(&target).await {
                            let src_size = client
                                .stat(&remote_path)
                                .await
                                .map(|s| s.size)
                                .unwrap_or(0);
                            return Ok::<_, String>(Some(crate::state::OverwritePrompt {
                                src: remote_path,
                                dst_dir: local_dir.to_string_lossy().into_owned(),
                                basename,
                                src_size,
                                dst_size: existing.len(),
                                direction: crate::state::OverwriteDirection::Download,
                                multi: false,
                                apply_to_all: false,
                            }));
                        }
                        client
                            // Single file: one extra stat is negligible.
                            .download_to(&remote_path, &target, None)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(None)
                    },
                    move |result| match result {
                        Ok(None) => Message::Sftp(SftpMessage::SftpRefreshLocal(local_side)),
                        Ok(Some(prompt)) => Message::Sftp(SftpMessage::SftpAskOverwrite(prompt)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ));
            }
            SftpMessage::SftpDownloadTo(then) => {
                self.sftp.row_menu = None;
                // Explicit ask, so an override left over from a drop onto
                // a folder must not short-circuit it.
                self.sftp.download_dest_override = None;
                return Ok(self.sftp_pick_download_dir(*then));
            }
            SftpMessage::SftpDownloadDestPicked(dir, then) => {
                let Some(dir) = dir else {
                    // Cancelled: nothing was touched, in particular the
                    // override stays unset so the next download still asks.
                    return Ok(Task::none());
                };
                self.sftp.download_dest_override = Some(dir);
                return Ok(Task::done(Message::Sftp(*then)));
            }
            SftpMessage::SftpDuplicate(side, path) => {
                self.sftp.row_menu = None;
                if !self.sftp.pane(side).is_remote {
                        let src = std::path::PathBuf::from(&path);
                        let parent = match src.parent() {
                            Some(p) => p.to_path_buf(),
                            None => {
                                self.sftp.pane_mut(side).error = Some("Cannot duplicate root".into());
                                return Ok(Task::none());
                            }
                        };
                        let basename = src
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("untitled")
                            .to_string();
                        let unique = unique_name_in_local_dir(&parent, &basename);
                        let dest = parent.join(&unique);
                        // The copy can be multi-GB; run it off the event
                        // loop instead of freezing update() for the
                        // duration, mirroring the remote branch below.
                        return Ok(Task::perform(
                            tokio::task::spawn_blocking(move || std::fs::copy(&src, &dest)),
                            move |res| match res {
                                Ok(Ok(_)) => Message::Sftp(SftpMessage::SftpRefreshLocal(side)),
                                Ok(Err(e)) => Message::Sftp(SftpMessage::SftpOpResult(side, format!("copy: {e}"), true)),
                                Err(e) => Message::Sftp(SftpMessage::SftpOpResult(side, format!("copy: {e}"), true)),
                            },
                        ));
                } else {
                        let Some(client) = self.sftp.pane(side).client.clone() else {
                            return Ok(Task::none());
                        };
                        let parent = parent_path(&path);
                        let basename = path
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or(&path)
                            .to_string();
                        let reload = self.sftp.pane(side).remote_path.clone();
                        let src = path.clone();
                        return Ok(Task::perform(
                            async move {
                                let unique =
                                    unique_name_in_remote_dir(&client, &parent, &basename)
                                        .await?;
                                let dest = remote_join(&parent, &unique);
                                // `cp -- src dst`, same exec channel trick
                                // we used for `rm -rf`. Using -- prevents
                                // dashes in names from being parsed as flags.
                                remote_cp(&client, &src, &dest, false).await?;
                                Ok::<String, String>(reload)
                            },
                            move |result| match result {
                                Ok(reload) => Message::Sftp(SftpMessage::SftpNavigateRemote(side, reload)),
                                Err(e) => Message::Sftp(SftpMessage::SftpOpResult(side, e, true)),
                            },
                        ));
                }
            }
            SftpMessage::SftpFileHovered => {
                self.sftp.drop_active = true;
            }
            SftpMessage::SftpFilesHoveredLeft => {
                self.sftp.drop_active = false;
            }
            SftpMessage::SftpFileDropped(path) => {
                // OS drops only land in a remote folder when the
                // hovered row is on the remote pane AND a folder.
                let target_folder = self
                    .sftp
                    .hovered_row
                    .as_ref()
                    .filter(|(s, _, is_dir)| *s == remote_side && *is_dir)
                    .map(|(_, p, _)| p.clone());
                self.sftp.drop_active = false;
                // Deliberately NOT gated on `drop_active`: a FileDropped
                // only ever arrives from a genuine OS drop, and requiring
                // the hover flag broke real gestures twice over. A
                // multi-file drop delivers one FileDropped per file, so
                // the first file consumed the flag and the rest were
                // silently ignored; and a missed/late FileHovered
                // (observed on Windows after a previous drop) killed the
                // whole next gesture. The flag now only powers the drop
                // highlight.
                if !self.sftp_surface_visible() {
                    // Not an SFTP drop at all: an SFTP tab exists (the
                    // owner gate above passed) but a terminal is what's
                    // on screen. Hand the file to the terminal drop
                    // router (#106) instead of swallowing it.
                    return Ok(self.buffer_terminal_drop(path));
                }
                let in_remote_pane =
                    target_folder.is_some() || self.is_cursor_over_remote_pane();
                if !in_remote_pane {
                    return Ok(Task::none());
                }
                if self.sftp.pane(remote_side).client.is_none() {
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                }
                // A multi-select drop arrives as one FileDropped per
                // file. Collect the burst and flush once, so it becomes
                // a single batch transfer instead of N transfers racing
                // for the queue UI. The first file of the gesture pins
                // the destination (folder row vs pane dir) for them all.
                if self.sftp.pending_drops.is_empty() {
                    self.sftp.upload_dest_override = target_folder;
                    self.sftp.pending_drops.push(path);
                    return Ok(Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                150,
                            ))
                            .await;
                        },
                        |_| Message::Sftp(SftpMessage::SftpDropFlush),
                    ));
                }
                self.sftp.pending_drops.push(path);
            }
            SftpMessage::SftpDropFlush => {
                let mut paths = std::mem::take(&mut self.sftp.pending_drops);
                // The upload handlers below consume `upload_dest_override`
                // (set when the burst started) before falling back to the
                // pane's remote dir.
                return Ok(match paths.len() {
                    0 => Task::none(),
                    1 => {
                        let p = paths.remove(0);
                        if p.is_dir() {
                            Task::done(Message::Sftp(SftpMessage::SftpUploadFolder(p)))
                        } else {
                            Task::done(Message::Sftp(SftpMessage::SftpUpload(p)))
                        }
                    }
                    _ => Task::done(Message::Sftp(SftpMessage::SftpUploadBatch(paths))),
                });
            }
            SftpMessage::SftpUploadFolder(local_root) => {
                self.sftp.row_menu = None;
                if self.sftp_upload_blocked_by_zip(remote_side) {
                    return Ok(Task::none());
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                };
                let remote_dir = self
                    .sftp
                    .upload_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(remote_side).remote_path.clone());
                let concurrency = self.sftp_concurrency();
                return Ok(Task::perform(
                    async move {
                        let basename = local_root
                            .file_name()
                            .and_then(|s| s.to_str())
                            .ok_or_else(|| "invalid folder name".to_string())?
                            .to_string();
                        let unique =
                            unique_name_in_remote_dir(&client, &remote_dir, &basename).await?;
                        let target_root = remote_join(&remote_dir, &unique);
                        let mut queue = std::collections::VecDeque::new();
                        queue.push_back(crate::state::TransferItem {
                            src: local_root.to_string_lossy().into_owned(),
                            dst: target_root.clone(),
                            is_dir: true,
                            size: None,
                        });
                        walk_local_for_upload(&local_root, &target_root, &mut queue)
                            .map_err(|e| e.to_string())?;
                        let clients = build_client_pool(client, concurrency).await?;
                        Ok::<crate::state::TransferState, String>(crate::state::TransferState::new(
                            crate::state::TransferKind::Upload,
                            unique,
                            queue,
                            clients,
                            None,
                            None,
                            concurrency,
                        ))
                    },
                    move |result| match result {
                        Ok(state) => Message::Sftp(SftpMessage::SftpTransferQueueReady(owner, state)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ));
            }
            SftpMessage::SftpDownloadFolder(remote_root) => {
                self.sftp.row_menu = None;
                if let Some(ask) = self.sftp_ask_download_dir(SftpMessage::SftpDownloadFolder(
                    remote_root.clone(),
                )) {
                    return Ok(ask);
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                };
                let local_dir = self
                    .sftp
                    .download_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(local_side).local_path.clone());
                let concurrency = self.sftp_concurrency();
                return Ok(Task::perform(
                    async move {
                        let basename = remote_root
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or(&remote_root)
                            .to_string();
                        // Pick a non-colliding local name.
                        let unique = unique_name_in_local_dir(&local_dir, &basename);
                        let target_root = local_dir.join(&unique);
                        let mut queue = std::collections::VecDeque::new();
                        queue.push_back(crate::state::TransferItem {
                            src: remote_root.clone(),
                            dst: target_root.to_string_lossy().into_owned(),
                            is_dir: true,
                            size: None,
                        });
                        walk_remote_for_download(&client, &remote_root, &target_root, &mut queue)
                            .await?;
                        let clients = build_client_pool(client, concurrency).await?;
                        Ok::<crate::state::TransferState, String>(crate::state::TransferState::new(
                            crate::state::TransferKind::Download,
                            unique,
                            queue,
                            clients,
                            None,
                            None,
                            concurrency,
                        ))
                    },
                    move |result| match result {
                        Ok(state) => Message::Sftp(SftpMessage::SftpTransferQueueReady(owner, state)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ));
            }
            SftpMessage::SftpDuplicateFolder(side, path) => {
                self.sftp.row_menu = None;
                if !self.sftp.pane(side).is_remote {
                        let src = std::path::PathBuf::from(&path);
                        let parent = match src.parent() {
                            Some(p) => p.to_path_buf(),
                            None => {
                                self.sftp.pane_mut(side).error = Some("Cannot duplicate root".into());
                                return Ok(Task::none());
                            }
                        };
                        let basename = src
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("untitled")
                            .to_string();
                        let unique = unique_name_in_local_dir(&parent, &basename);
                        let target_root = parent.join(&unique);
                        // Build the queue synchronously, no client needed
                        // for a local-only walk + copy.
                        let mut queue = std::collections::VecDeque::new();
                        queue.push_back(crate::state::TransferItem {
                            src: src.to_string_lossy().into_owned(),
                            dst: target_root.to_string_lossy().into_owned(),
                            is_dir: true,
                            size: None,
                        });
                        if let Err(e) = walk_local_for_duplicate(&src, &target_root, &mut queue) {
                            self.sftp.pane_mut(side).error = Some(e);
                            return Ok(Task::none());
                        }
                        // Local duplicate uses sync std::fs::copy in
                        // the queue runner, no SFTP channels needed,
                        // so the client pool stays empty. Concurrency
                        // is fixed at 1 for the same reason: spawning
                        // multiple sync workers wouldn't help (they'd
                        // hammer the OS file cache from the same
                        // thread).
                        let state = crate::state::TransferState::new(
                            crate::state::TransferKind::DuplicateLocal,
                            unique,
                            queue,
                            Vec::new(),
                            None,
                            None,
                            1,
                        );
                        return Ok(Task::done(Message::Sftp(SftpMessage::SftpTransferQueueReady(owner, state))));
                } else {
                        let Some(client) = self.sftp.pane(side).client.clone() else {
                            return Ok(Task::none());
                        };
                        let parent = parent_path(&path);
                        let basename = path
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or(&path)
                            .to_string();
                        let reload = self.sftp.pane(side).remote_path.clone();
                        let src = path.clone();
                        // `cp -r --`, single fast call, no progress bar
                        // needed since the user can't usefully observe
                        // partial recursive copy progress over SSH anyway.
                        return Ok(Task::perform(
                            async move {
                                let unique =
                                    unique_name_in_remote_dir(&client, &parent, &basename)
                                        .await?;
                                let dest = remote_join(&parent, &unique);
                                remote_cp(&client, &src, &dest, true).await?;
                                Ok::<String, String>(reload)
                            },
                            move |result| match result {
                                Ok(reload) => Message::Sftp(SftpMessage::SftpNavigateRemote(side, reload)),
                                Err(e) => Message::Sftp(SftpMessage::SftpOpResult(side, e, true)),
                            },
                        ));
                }
            }
            SftpMessage::SftpTransferQueueReady(_, state) => {
                let slot_count = state.busy_slots.len().max(1);
                // Fresh transfer: reset the per-file panel log + collapse it.
                self.sftp.transfer_done_log.clear();
                self.sftp.transfer_panel_open = false;
                let verb_key = match state.kind {
                    crate::state::TransferKind::Upload => "sftp_log_uploading",
                    crate::state::TransferKind::Download => "sftp_log_downloading",
                    crate::state::TransferKind::DuplicateLocal => "sftp_log_duplicating",
                    crate::state::TransferKind::Relay => "sftp_log_relaying",
                };
                self.push_sftp_log(
                    crate::state::SftpLogLevel::Info,
                    format!(
                        "{} {} ({} {})",
                        crate::i18n::t(verb_key),
                        state.root_label,
                        state.total,
                        crate::i18n::t("sftp_log_items"),
                    ),
                );
                // Live byte progress: total = sum of known item sizes (0 if
                // unknown, bar falls back to item counts). Use a *fresh*
                // counter rather than resetting the old one, so a lingering
                // worker from a previous/cancelled transfer (whose task may
                // still be draining) can't keep incrementing this transfer's
                // counter and spike the bar to 100% before its first byte.
                self.sftp.transfer_bytes_total =
                    state.queue.iter().filter_map(|i| i.size).sum();
                self.sftp.transfer_bytes_done =
                    std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                self.sftp.transfer = Some(state);
                // Kick off one Next per slot so the worker pool fills
                // up immediately. Each completion will dispatch its
                // own Next to keep the chain going.
                let initial: Vec<Task<Message>> = (0..slot_count)
                    .map(|_| Task::done(Message::Sftp(SftpMessage::SftpTransferNext(owner))))
                    .collect();
                return Ok(Task::batch(initial));
            }
            SftpMessage::SftpTransferNext(_) => {
                let Some(transfer) = self.sftp.transfer.as_mut() else {
                    return Ok(Task::none());
                };
                if transfer.paused {
                    // Modal is up, workers idle until the user picks
                    // an action. Resolve will re-dispatch Next for
                    // each slot then.
                    return Ok(Task::none());
                }
                if transfer.dir_slot.is_some() {
                    // A directory item is in flight. It's an ordering
                    // barrier (see `TransferState::dir_slot`): nothing
                    // queued behind it may start until it exists. Its
                    // ItemDone refills the pool.
                    return Ok(Task::none());
                }
                if transfer.queue.front().is_some_and(|i| i.is_dir)
                    && transfer.busy_slots.iter().any(|b| *b)
                {
                    // Next up is a directory: drain the in-flight items
                    // first so everything queued before it (its own
                    // parent dir included) has finished. The pending
                    // ItemDones re-dispatch Next.
                    return Ok(Task::none());
                }
                let Some(slot) = transfer
                    .busy_slots
                    .iter()
                    .position(|b| !b)
                    .map(|i| i as u8)
                else {
                    // All slots busy, Next dispatch by ItemDone is
                    // ahead of an already-busy slot. Drop it; the
                    // next ItemDone will free a slot.
                    return Ok(Task::none());
                };
                let Some(item) = transfer.queue.pop_front() else {
                    // Queue exhausted. If every slot is idle, finalize
                    // and refresh; otherwise wait for in-flight slots
                    // to drain.
                    if transfer.busy_slots.iter().all(|b| !b) {
                        let kind = transfer.kind;
                        // Relay refreshes its actual destination pane,
                        // which may be the left pane (right-to-left relay),
                        // not the canonical remote (`remote_side`).
                        let relay_dest = transfer.dest_side;
                        let root_label = transfer.root_label.clone();
                        // A MOVE removes its sources here and nowhere
                        // else. Reaching this arm is the proof the copy
                        // succeeded: every item was popped, every worker
                        // reported done, and any single failure would
                        // have cleared `transfer` from the error arm long
                        // before the queue could drain (issue #115).
                        let move_sources = transfer.move_sources.take();
                        let move_client = transfer.clients.first().cloned();
                        self.sftp.transfer = None;
                        self.push_sftp_log(
                            crate::state::SftpLogLevel::Ok,
                            format!("{} {}", crate::i18n::t("sftp_log_transfer_done"), root_label),
                        );
                        if let (Some(sources), Some(client)) = (move_sources, move_client) {
                            let src_side = if relay_dest == Some(SftpPaneSide::Left) {
                                SftpPaneSide::Right
                            } else {
                                SftpPaneSide::Left
                            };
                            let dst_side = relay_dest.unwrap_or(remote_side);
                            let src_path = self.sftp.pane(src_side).remote_path.clone();
                            let dst_path = self.sftp.pane(dst_side).remote_path.clone();
                            return Ok(Task::perform(
                                remove_moved_sources(client, sources),
                                move |r| match r {
                                    // Both panes changed: the source lost
                                    // the tree, the destination gained it.
                                    Ok(_) => Message::Sftp(SftpMessage::SftpNavigateRemote(
                                        src_side,
                                        src_path.clone(),
                                    )),
                                    // The copy is safe on the other host;
                                    // only the cleanup failed, so say that
                                    // and leave the source alone.
                                    Err(e) => Message::Sftp(SftpMessage::SftpOpResult(
                                        src_side, e, true,
                                    )),
                                },
                            )
                            .chain(Task::done(Message::Sftp(
                                SftpMessage::SftpNavigateRemote(dst_side, dst_path),
                            ))));
                        }
                        return Ok(match kind {
                            crate::state::TransferKind::Relay => {
                                let dst = relay_dest.unwrap_or(remote_side);
                                Task::done(Message::Sftp(SftpMessage::SftpNavigateRemote(
                                    dst,
                                    self.sftp.pane(dst).remote_path.clone(),
                                )))
                            }
                            crate::state::TransferKind::Upload => Task::done(
                                Message::Sftp(SftpMessage::SftpNavigateRemote(
                                    remote_side,
                                    self.sftp.pane(remote_side).remote_path.clone(),
                                )),
                            ),
                            crate::state::TransferKind::Download
                            | crate::state::TransferKind::DuplicateLocal => {
                                self.refresh_sftp_local(local_side);
                                Task::none()
                            }
                        });
                    }
                    return Ok(Task::none());
                };
                transfer.busy_slots[slot as usize] = true;
                if item.is_dir {
                    transfer.dir_slot = Some(slot);
                }
                transfer.current = Some(transfer_item_label(&item));
                let kind = transfer.kind;
                let overwrite_default = transfer.overwrite_default;
                let multi = transfer.total > 1;
                // Shared live-byte counter the worker increments as chunks
                // move; the tick subscription polls it for the bar.
                let bytes_done = self.sftp.transfer_bytes_done.clone();
                match kind {
                    crate::state::TransferKind::Upload => {
                        let Some(client) = transfer.clients.get(slot as usize).cloned() else {
                            return Ok(Task::done(Message::Sftp(SftpMessage::SftpTransferError(
                                owner,
                                "transfer: slot has no client".into(),
                                slot,
                            ))));
                        };
                        return Ok(Task::perform(
                            do_upload_item(client, item, overwrite_default, multi, Some(bytes_done)),
                            move |r| match r {
                                Ok(TransferStepOutcome::Done) => {
                                    Message::Sftp(SftpMessage::SftpTransferItemDone(owner, slot))
                                }
                                Ok(TransferStepOutcome::Conflict { prompt, item }) => {
                                    Message::Sftp(SftpMessage::SftpTransferConflict(owner, prompt, item, slot))
                                }
                                Err(e) => Message::Sftp(SftpMessage::SftpTransferError(owner, e, slot)),
                            },
                        ));
                    }
                    crate::state::TransferKind::Download => {
                        let Some(client) = transfer.clients.get(slot as usize).cloned() else {
                            return Ok(Task::done(Message::Sftp(SftpMessage::SftpTransferError(
                                owner,
                                "transfer: slot has no client".into(),
                                slot,
                            ))));
                        };
                        return Ok(Task::perform(
                            do_download_item(client, item, overwrite_default, multi, Some(bytes_done)),
                            move |r| match r {
                                Ok(TransferStepOutcome::Done) => {
                                    Message::Sftp(SftpMessage::SftpTransferItemDone(owner, slot))
                                }
                                Ok(TransferStepOutcome::Conflict { prompt, item }) => {
                                    Message::Sftp(SftpMessage::SftpTransferConflict(owner, prompt, item, slot))
                                }
                                Err(e) => Message::Sftp(SftpMessage::SftpTransferError(owner, e, slot)),
                            },
                        ));
                    }
                    crate::state::TransferKind::Relay => {
                        // Source client for the slot, plus the single
                        // dest-host client (relay runs at concurrency 1).
                        let Some(src_client) = transfer.clients.get(slot as usize).cloned() else {
                            return Ok(Task::done(Message::Sftp(SftpMessage::SftpTransferError(
                                owner,
                                "transfer: slot has no client".into(),
                                slot,
                            ))));
                        };
                        let Some(dst_client) = transfer.dest_client.clone() else {
                            return Ok(Task::done(Message::Sftp(SftpMessage::SftpTransferError(
                                owner,
                                "relay: missing destination client".into(),
                                slot,
                            ))));
                        };
                        // A move verifies every file landed at the right
                        // size before anything is removed later; a copy
                        // does not pay for that round trip.
                        let verify = transfer.move_sources.is_some();
                        return Ok(Task::perform(
                            do_relay_item(src_client, dst_client, item, Some(bytes_done), verify),
                            move |r| match r {
                                Ok(()) => Message::Sftp(SftpMessage::SftpTransferItemDone(owner, slot)),
                                Err(e) => Message::Sftp(SftpMessage::SftpTransferError(owner, e, slot)),
                            },
                        ));
                    }
                    crate::state::TransferKind::DuplicateLocal => {
                        // Sync, no need for an async task.
                        return Ok(match do_local_duplicate_item(&item) {
                            Ok(()) => Task::done(Message::Sftp(SftpMessage::SftpTransferItemDone(owner, slot))),
                            Err(e) => Task::done(Message::Sftp(SftpMessage::SftpTransferError(owner, e, slot))),
                        });
                    }
                }
            }
            SftpMessage::SftpTransferItemDone(_, slot) => {
                // Record the finished item's label for the per-file panel.
                // `current` is the label set when this item was dispatched
                // (exact at the relay's concurrency of 1; an approximation
                // at higher concurrency, good enough for a status list).
                let finished = self.sftp.transfer.as_ref().and_then(|t| t.current.clone());
                let mut refill = 1usize;
                if let Some(transfer) = self.sftp.transfer.as_mut() {
                    transfer.completed += 1;
                    transfer.current = None;
                    if (slot as usize) < transfer.busy_slots.len() {
                        transfer.busy_slots[slot as usize] = false;
                    }
                    if transfer.dir_slot == Some(slot) {
                        // Barrier lifted: the dir exists now, so refill
                        // the whole pool (Next dispatched nothing while
                        // it was in flight; extra Nexts drop harmlessly
                        // on the all-busy guard).
                        transfer.dir_slot = None;
                        refill = transfer.busy_slots.len().max(1);
                    }
                }
                if let Some(label) = finished {
                    self.sftp.transfer_done_log.push(label);
                }
                let next: Vec<Task<Message>> = (0..refill)
                    .map(|_| Task::done(Message::Sftp(SftpMessage::SftpTransferNext(owner))))
                    .collect();
                return Ok(Task::batch(next));
            }
            SftpMessage::SftpToggleTransferPanel => {
                self.sftp.transfer_panel_open = !self.sftp.transfer_panel_open;
            }
            // No-op: the redraw it triggers is the point (the bar reads the
            // shared byte counter during view()).
            SftpMessage::SftpTransferTick => {}
            SftpMessage::SftpTransferConflict(_, prompt, item, slot) => {
                // Park the popped item alongside the prompt so the
                // resolve handler knows which destination the user is
                // about to act on. The queue stays stalled here until
                // the modal is answered.
                if let Some(transfer) = self.sftp.transfer.as_mut() {
                    transfer.pending_conflict_item = Some(item);
                    transfer.pending_conflict_slot = Some(slot);
                    transfer.paused = true;
                    if (slot as usize) < transfer.busy_slots.len() {
                        transfer.busy_slots[slot as usize] = false;
                    }
                }
                self.sftp.overwrite_prompt = Some(prompt);
            }
            SftpMessage::SftpUploadBatch(paths) => {
                self.sftp.row_menu = None;
                if self.sftp_upload_blocked_by_zip(remote_side) {
                    return Ok(Task::none());
                }
                if paths.is_empty() {
                    return Ok(Task::none());
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                };
                let remote_dir = self
                    .sftp
                    .upload_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(remote_side).remote_path.clone());
                let concurrency = self.sftp_concurrency();
                return Ok(Task::perform(
                    async move {
                        let mut queue = std::collections::VecDeque::new();
                        // Each top-level path goes in as-is; folders
                        // expand recursively. Names aren't pre-uniqued
                        //, the per-item conflict check at the queue
                        // runner handles that with user input.
                        for path in &paths {
                            let basename = path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("file")
                                .to_string();
                            let target = if remote_dir == "/" {
                                format!("/{}", basename)
                            } else {
                                format!(
                                    "{}/{}",
                                    remote_dir.trim_end_matches('/'),
                                    basename
                                )
                            };
                            if path.is_dir() {
                                queue.push_back(crate::state::TransferItem {
                                    src: path.to_string_lossy().into_owned(),
                                    dst: target.clone(),
                                    is_dir: true,
                                    size: None,
                                });
                                walk_local_for_upload(path, &target, &mut queue)
                                    .map_err(|e| e.to_string())?;
                            } else {
                                queue.push_back(crate::state::TransferItem {
                                    src: path.to_string_lossy().into_owned(),
                                    dst: target,
                                    is_dir: false,
                                    // Byte size up front so the total is known
                                    // and the bar advances by bytes.
                                    size: path.metadata().map(|m| m.len()).ok(),
                                });
                            }
                        }
                        let label = if paths.len() == 1 {
                            paths[0]
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("upload")
                                .to_string()
                        } else {
                            format!("{} items", paths.len())
                        };
                        let clients = build_client_pool(client, concurrency).await?;
                        Ok::<crate::state::TransferState, String>(crate::state::TransferState::new(
                            crate::state::TransferKind::Upload,
                            label,
                            queue,
                            clients,
                            None,
                            None,
                            concurrency,
                        ))
                    },
                    move |result| match result {
                        Ok(state) => Message::Sftp(SftpMessage::SftpTransferQueueReady(owner, state)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ));
            }
            SftpMessage::SftpUploadSelection => {
                self.sftp.row_menu = None;
                let paths: Vec<std::path::PathBuf> = self
                    .sftp
                    .selected_rows
                    .iter()
                    .filter(|(s, _)| !self.sftp.pane(*s).is_remote)
                    .map(|(_, p)| std::path::PathBuf::from(p))
                    .collect();
                if paths.is_empty() {
                    return Ok(Task::none());
                }
                return Ok(Task::done(Message::Sftp(SftpMessage::SftpUploadBatch(paths))));
            }
            SftpMessage::SftpDownloadSelection => {
                self.sftp.row_menu = None;
                if let Some(ask) = self.sftp_ask_download_dir(SftpMessage::SftpDownloadSelection) {
                    return Ok(ask);
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some("Not connected to a host".into());
                    return Ok(Task::none());
                };
                let remote_items: Vec<(String, bool)> = self
                    .sftp
                    .selected_rows
                    .iter()
                    .filter(|(s, _)| self.sftp.pane(*s).is_remote)
                    .map(|(s, p)| (p.clone(), self.row_is_dir_in_pane(*s, p)))
                    .collect();
                if remote_items.is_empty() {
                    return Ok(Task::none());
                }
                let local_dir = self
                    .sftp
                    .download_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(local_side).local_path.clone());
                let concurrency = self.sftp_concurrency();
                return Ok(Task::perform(
                    async move {
                        let mut queue = std::collections::VecDeque::new();
                        for (remote_path, is_dir) in &remote_items {
                            let basename = remote_path
                                .rsplit('/')
                                .find(|s| !s.is_empty())
                                .unwrap_or(remote_path)
                                .to_string();
                            let target = local_dir.join(&basename);
                            if *is_dir {
                                queue.push_back(crate::state::TransferItem {
                                    src: remote_path.clone(),
                                    dst: target.to_string_lossy().into_owned(),
                                    is_dir: true,
                                    size: None,
                                });
                                walk_remote_for_download(
                                    &client,
                                    remote_path,
                                    &target,
                                    &mut queue,
                                )
                                .await?;
                            } else {
                                queue.push_back(crate::state::TransferItem {
                                    src: remote_path.clone(),
                                    dst: target.to_string_lossy().into_owned(),
                                    is_dir: false,
                                    size: None,
                                });
                            }
                        }
                        let label = if remote_items.len() == 1 {
                            remote_items[0]
                                .0
                                .rsplit('/')
                                .find(|s| !s.is_empty())
                                .unwrap_or(&remote_items[0].0)
                                .to_string()
                        } else {
                            format!("{} items", remote_items.len())
                        };
                        let clients = build_client_pool(client, concurrency).await?;
                        Ok::<crate::state::TransferState, String>(crate::state::TransferState::new(
                            crate::state::TransferKind::Download,
                            label,
                            queue,
                            clients,
                            None,
                            None,
                            concurrency,
                        ))
                    },
                    move |result| match result {
                        Ok(state) => Message::Sftp(SftpMessage::SftpTransferQueueReady(owner, state)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ));
            }
            SftpMessage::SftpDuplicateSelection => {
                self.sftp.row_menu = None;
                // Fan out per-item duplicate. They run sequentially
                // anyway because the SFTP connection serializes; for
                // local-side they're independent fs::copy calls.
                let items: Vec<(crate::state::SftpPaneSide, String, bool)> = self
                    .sftp
                    .selected_rows
                    .iter()
                    .map(|(side, path)| (*side, path.clone(), self.row_is_dir_in_pane(*side, path)))
                    .collect();
                if items.is_empty() {
                    return Ok(Task::none());
                }
                let mut tasks = Vec::with_capacity(items.len());
                for (side, path, is_dir) in items {
                    tasks.push(Task::done(if is_dir {
                        Message::Sftp(SftpMessage::SftpDuplicateFolder(side, path))
                    } else {
                        Message::Sftp(SftpMessage::SftpDuplicate(side, path))
                    }));
                }
                self.sftp.selected_rows.clear();
                return Ok(Task::batch(tasks));
            }
            SftpMessage::SftpTransferError(_, e, _slot) => {
                // Errors abort the whole transfer, the in-flight item
                // failed and we don't try to be clever about retrying
                // siblings (a network blip is likely to nuke them all).
                let kind = self.sftp.transfer.as_ref().map(|t| t.kind);
                let relay_dest = self.sftp.transfer.as_ref().and_then(|t| t.dest_side);
                self.sftp.transfer = None;
                match kind {
                    Some(crate::state::TransferKind::DuplicateLocal) => {
                        self.sftp.pane_mut(local_side).error = Some(e);
                        self.refresh_sftp_local(local_side);
                    }
                    Some(crate::state::TransferKind::Relay) => {
                        let dst = relay_dest.unwrap_or(remote_side);
                        self.sftp.pane_mut(dst).error = Some(e);
                    }
                    _ => {
                        self.sftp.pane_mut(remote_side).error = Some(e);
                    }
                }
            }
            SftpMessage::SftpCancelTransfer => {
                let kind = self.sftp.transfer.as_ref().map(|t| t.kind);
                let relay_dest = self.sftp.transfer.as_ref().and_then(|t| t.dest_side);
                self.sftp.transfer = None;
                // The in-flight item can't be aborted mid-byte (russh-sftp
                // doesn't expose a cancel token), but no further items
                // will run, and the user can refresh to see the partial
                // result.
                match kind {
                    Some(crate::state::TransferKind::Relay) => {
                        let dst = relay_dest.unwrap_or(remote_side);
                        return Ok(Task::done(Message::Sftp(SftpMessage::SftpNavigateRemote(
                            dst,
                            self.sftp.pane(dst).remote_path.clone(),
                        ))));
                    }
                    Some(crate::state::TransferKind::Upload) => {
                        return Ok(Task::done(Message::Sftp(SftpMessage::SftpNavigateRemote(
                            remote_side,
                            self.sftp.pane(remote_side).remote_path.clone(),
                        ))));
                    }
                    Some(_) => {
                        self.refresh_sftp_local(local_side);
                    }
                    None => {}
                }
            }
            SftpMessage::SftpRelay(from, src_path) => {
                return Ok(self.start_relay(owner, from, src_path, false, false));
            }
            SftpMessage::SftpRelayFolder(from, src_root) => {
                return Ok(self.start_relay(owner, from, src_root, true, false));
            }
            SftpMessage::SftpRelayMove(from, src_path) => {
                return Ok(self.start_relay(owner, from, src_path, false, true));
            }
            SftpMessage::SftpRelayMoveFolder(from, src_root) => {
                return Ok(self.start_relay(owner, from, src_root, true, true));
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }

    /// Build and start a server-to-server transfer: the file or tree at
    /// `src_path` on the `from` pane's host, onto the other pane's host.
    ///
    /// One builder for all four entry points, because a move IS a relay
    /// plus a removal at the end. Splitting them would give the move its
    /// own copy of the destination-naming and tree-walking logic, which
    /// is exactly the logic that must not drift between the two.
    ///
    /// `move_source` only attaches the removal list; nothing here
    /// deletes anything. The removal runs from the finalize arm, which
    /// is unreachable unless every item copied AND verified.
    fn start_relay(
        &mut self,
        owner: uuid::Uuid,
        from: SftpPaneSide,
        src_path: String,
        is_dir: bool,
        move_source: bool,
    ) -> Task<Message> {
        self.sftp.row_menu = None;
        let dest_side = if from == SftpPaneSide::Left {
            SftpPaneSide::Right
        } else {
            SftpPaneSide::Left
        };
        let (Some(src_client), Some(dst_client)) = (
            self.sftp.pane(from).client.clone(),
            self.sftp.pane(dest_side).client.clone(),
        ) else {
            self.sftp.pane_mut(from).error = Some("Both panes must be connected".into());
            return Task::none();
        };
        // Same machine? Two conservative signals: a shared SSH session is
        // exact (every client from one session holds that session's own
        // handle), and equal host labels mean the panes were mounted from
        // the same vault entry. Neither can claim "same host" for two
        // genuinely different machines, so the containment guard below
        // can only ever fire on paths that really do share a filesystem.
        // The converse is allowed to be wrong: missing a case leaves
        // today's behaviour, while a false positive would refuse a
        // legitimate transfer.
        let same_host = src_client.shares_session_with(&dst_client) || {
            let a = self.sftp.pane(from).host_label.as_ref();
            let b = self.sftp.pane(dest_side).host_label.as_ref();
            a.is_some() && a == b
        };
        let dest_dir = self
            .sftp
            .upload_dest_override
            .take()
            .unwrap_or_else(|| self.sftp.pane(dest_side).remote_path.clone());
        // A move within one SSH session is a rename: instant, atomic, and
        // it keeps ownership, permissions and timestamps that a copy plus
        // delete would rebuild. Known synchronously, so the task can be
        // shaped for it: the rename path finishes without a queue, and
        // both panes need refreshing when it does.
        let try_rename = move_source && src_client.shares_session_with(&dst_client);
        let src_refresh = self.sftp.pane(from).remote_path.clone();
        let dst_refresh = self.sftp.pane(dest_side).remote_path.clone();
        let build = Task::perform(
            async move {
                let basename = src_path
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or(&src_path)
                    .to_string();
                if move_source && same_host {
                    // Moving into the folder the item already sits in is a
                    // no-op, and the unique-name step below would quietly
                    // turn it into a RENAME instead: the name is taken by
                    // the source itself, so the copy lands beside it as
                    // "x (1)" and the original is then removed. Refuse,
                    // rather than silently renaming someone's file
                    // (issue #115).
                    if destinations_are_one_directory(
                        &src_client,
                        &parent_path(&src_path),
                        &dst_client,
                        &dest_dir,
                    )
                    .await
                    {
                        return Err(crate::i18n::t("sftp_move_same_directory").to_string());
                    }
                }
                // Pick a non-colliding name on the destination so a
                // transfer never silently clobbers an existing file with
                // the same name.
                let unique = unique_name_in_remote_dir(&dst_client, &dest_dir, &basename).await?;
                let target = remote_join(&dest_dir, &unique);
                if same_host && relay_target_is_inside_source(&src_path, &target) {
                    return Err(crate::i18n::t("sftp_relay_into_itself").to_string());
                }
                if try_rename {
                    // Falls through to copy plus delete on failure, which
                    // is what a cross-filesystem move on one host needs
                    // (`/home` and `/mnt/data` are one session but two
                    // devices, and rename cannot span them).
                    match src_client.rename(&src_path, &target).await {
                        Ok(()) => return Ok(None),
                        Err(e) => tracing::debug!(
                            "move: rename {src_path} -> {target} failed, \
                             falling back to copy and delete: {e}"
                        ),
                    }
                }
                let mut queue = std::collections::VecDeque::new();
                if is_dir {
                    queue.push_back(crate::state::TransferItem {
                        src: src_path.clone(),
                        dst: target.clone(),
                        is_dir: true,
                        size: None,
                    });
                    // Walk the SOURCE remote tree, mapping each entry onto
                    // a destination POSIX path under the target root.
                    walk_remote_for_relay(&src_client, &src_path, &target, &mut queue).await?;
                } else {
                    queue.push_back(crate::state::TransferItem {
                        src: src_path.clone(),
                        dst: target,
                        is_dir: false,
                        size: None,
                    });
                }
                let label = if is_dir { unique } else { basename };
                // Relay runs at concurrency 1: one source client slot plus
                // the single dest client.
                let state = crate::state::TransferState::new(
                    crate::state::TransferKind::Relay,
                    label,
                    queue,
                    vec![src_client],
                    Some(dst_client),
                    Some(dest_side),
                    1,
                );
                Ok::<Option<crate::state::TransferState>, String>(Some(if move_source {
                    // The removal list is the queue itself: same paths,
                    // same walk, so it cannot describe anything that was
                    // not copied.
                    let sources: Vec<crate::state::TransferItem> =
                        state.queue.iter().cloned().collect();
                    state.moving(sources)
                } else {
                    state
                }))
            },
            move |result| match result {
                Ok(Some(state)) => Message::Sftp(SftpMessage::SftpTransferQueueReady(owner, state)),
                // Renamed: no queue ever existed, so the source pane is
                // refreshed here and the destination by the chain below.
                Ok(None) => {
                    Message::Sftp(SftpMessage::SftpNavigateRemote(from, src_refresh.clone()))
                }
                Err(e) => Message::Sftp(SftpMessage::SftpOpResult(from, e, true)),
            },
        );
        if try_rename {
            build.chain(Task::done(Message::Sftp(SftpMessage::SftpNavigateRemote(
                dest_side,
                dst_refresh,
            ))))
        } else {
            build
        }
    }
}
