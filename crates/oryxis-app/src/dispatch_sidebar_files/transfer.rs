//! Bytes crossing the wire: download, upload, and the open-in-
//! editor round trip that is a download plus a watched upload.
//!
//! Every path here resolves the pane by id rather than by position,
//! because the user is free to switch tabs while a transfer runs.

use super::*;

impl Oryxis {
    pub(super) fn handle_sidebar_files_transfer(
        &mut self,
        message: SidebarFilesMessage,
    ) -> Task<Message> {
        match message {
            SidebarFilesMessage::SidebarFilesDownload(path) => {
                self.overlay = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                // Transfers only mean something over a session; a local
                // browser's row menu never offers them (issue #145).
                let Some(client) = pane.files.client.as_ref().and_then(|c| c.sftp().cloned())
                else {
                    return Task::none();
                };
                let pane_id = pane.id;
                let basename = path
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or(&path)
                    .to_string();
                // This used to be a one-shot `download_to` with a toast:
                // no progress, no cancel, and an error that vanished after
                // three seconds. A 3 GB file took all three defects at
                // once. It now enqueues on the SAME runner the dual-pane
                // surface uses, owned by this pane.
                let size = pane
                    .files
                    .entries
                    .iter()
                    .find(|e| !e.is_dir && path.ends_with(&e.name))
                    .map(|e| e.size);
                let concurrency = self.sftp_concurrency();
                Task::perform(
                    async move {
                        let dest = rfd::AsyncFileDialog::new()
                            .set_file_name(&basename)
                            .save_file()
                            .await?
                            .path()
                            .to_path_buf();
                        let mut queue = std::collections::VecDeque::new();
                        queue.push_back(crate::state::TransferItem {
                            src: path.clone(),
                            dst: dest.to_string_lossy().into_owned(),
                            is_dir: false,
                            size,
                        });
                        let clients =
                            crate::sftp_helpers::build_client_pool(client, concurrency)
                                .await
                                .ok()?;
                        Some(crate::state::TransferState::new(
                            crate::state::TransferKind::Download,
                            basename.clone(),
                            queue,
                            clients,
                            None,
                            None,
                            concurrency,
                        ))
                    },
                    move |state| match state {
                        // The envelope is what makes the pane the owner:
                        // it stamps `routing_sftp`, so the runner's
                        // accessors resolve to this pane's slot instead of
                        // whatever SFTP surface happens to be focused, and
                        // the whole chain stays pinned to it.
                        Some(state) => Message::SftpFor(
                            pane_id,
                            Box::new(SftpMessage::SftpTransferQueueReady(pane_id, state)),
                        ),
                        None => Message::NoOp,
                    },
                )
            }
            SidebarFilesMessage::SidebarFilesUploadInto(dir) => {
                self.overlay = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                if pane.files.client.is_none() {
                    return Task::none();
                }
                let pane_id = pane.id;
                // The dialog is cancellable, so NOTHING is touched here (in
                // particular the request stamp: bumping it now would leave a
                // superseded in-flight listing's completion dropped with
                // `loading` stuck on, killing follow-cwd for good). The
                // uploads themselves start in SidebarFilesUploadPicked.
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new().pick_files().await.map(|files| {
                            files
                                .iter()
                                .map(|f| f.path().to_path_buf())
                                .collect::<Vec<_>>()
                        })
                    },
                    move |picked| match picked {
                        Some(paths) if !paths.is_empty() => {
                            Message::SidebarFiles(SidebarFilesMessage::SidebarFilesUploadPicked(pane_id, dir.clone(), paths))
                        }
                        _ => Message::NoOp,
                    },
                )
            }
            SidebarFilesMessage::SidebarFilesUploadPicked(pane_id, dir, paths) => {
                // The dialog returned real picks; the pane may have changed
                // hands meanwhile, so resolve it by id like any completion.
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return Task::none();
                };
                let Some(client) = pane.files.client.as_ref().and_then(|c| c.sftp().cloned())
                else {
                    return Task::none();
                };
                // Same reversal as the download side: the serial
                // `upload_from` loop had no per-file progress, no
                // concurrency and no cancel. The runner has all three.
                let concurrency = self.sftp_concurrency();
                let label = paths
                    .first()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Task::perform(
                    async move {
                        let mut queue = std::collections::VecDeque::new();
                        for local in paths {
                            let name = local
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "file".to_string());
                            let size = tokio::fs::metadata(&local).await.ok().map(|m| m.len());
                            queue.push_back(crate::state::TransferItem {
                                src: local.to_string_lossy().into_owned(),
                                dst: files_join(&dir, &name),
                                is_dir: false,
                                size,
                            });
                        }
                        let clients =
                            crate::sftp_helpers::build_client_pool(client, concurrency)
                                .await
                                .ok()?;
                        Some(crate::state::TransferState::new(
                            crate::state::TransferKind::Upload,
                            label.clone(),
                            queue,
                            clients,
                            None,
                            None,
                            concurrency,
                        ))
                    },
                    move |state| match state {
                        Some(state) => Message::SftpFor(
                            pane_id,
                            Box::new(SftpMessage::SftpTransferQueueReady(pane_id, state)),
                        ),
                        None => Message::NoOp,
                    },
                )
            }
            SidebarFilesMessage::SidebarFilesEdit(path) => {
                self.overlay = None;
                let Some(client) = self
                    .active_pane_mut()
                    .and_then(|p| p.files.client.as_ref().and_then(|c| c.sftp().cloned()))
                else {
                    return Task::none();
                };
                // Same background watch as the SFTP pane's Open / Edit, with
                // the sidebar's own channel: the browser stays usable while
                // the editor runs, and each save asks before uploading.
                // Host identity for the watch, which is what tells two hosts'
                // same-path files apart. Resolved through the pane's saved
                // connection so it matches the label an SFTP pane records
                // (`conn.label`): the same file opened from both surfaces
                // must be recognised as already being edited, not watched
                // twice. Falls back to the pane's own label for panes with
                // no vault connection (quick-connect, cloud, local).
                let host = match self.active_pane_mut().map(|p| p.origin.clone()) {
                    Some(crate::state::PaneOrigin::Host(id)) => self
                        .connections
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| c.label.clone()),
                    _ => None,
                }
                .or_else(|| self.active_pane_mut().map(|p| p.label.clone()))
                .unwrap_or_default();
                self.start_edit_watch(
                    crate::state::SftpPaneSide::Right,
                    path,
                    crate::state::SftpEditOpener::OsDefault,
                    Some((client, host)),
                    true,
                )
            }
            SidebarFilesMessage::SidebarFilesDragOutReady(result) => {
                // The payload is ready; hop onto the UI thread and run
                // the OS drag there (issue #167). `DoDragDrop` blocks
                // that closure through the gesture; see `drag_out`.
                // The button may already be up by now (a very quick
                // flick): the drop source then ends the drag on its
                // first QueryContinueDrag, which is the right outcome.
                match result {
                    Ok(prepared) => {
                        // `window::run` takes an `Fn` closure, so the
                        // one-shot payload travels in a take-once slot.
                        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(prepared)));
                        iced::window::oldest()
                            .and_then(move |id| {
                                let slot = slot.clone();
                                iced::window::run(id, move |window| {
                                    let Some(prepared) =
                                        slot.lock().ok().and_then(|mut s| s.take())
                                    else {
                                        return;
                                    };
                                    if let Err(e) = crate::drag_out::start(window, prepared)
                                    {
                                        tracing::warn!("drag-out failed to start: {e}");
                                    }
                                })
                            })
                            .discard()
                    }
                    Err(e) => {
                        // The open failed (file vanished, channel
                        // died): the toast is the same surface every
                        // other one-shot op reports through.
                        self.set_toast(e);
                        Task::perform(
                            async {
                                tokio::time::sleep(std::time::Duration::from_millis(3000))
                                    .await;
                            },
                            |_| Message::ToastClear,
                        )
                    }
                }
            }
            SidebarFilesMessage::SidebarFilesOpToast(text) => {
                self.set_toast(text);
                Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                    },
                    |_| Message::ToastClear,
                )
            }
            // The parent routed us here, so anything else is a
            // grouping mistake, not a runtime case.
            m => crate::dispatch::unrouted(m),
        }
    }
}
