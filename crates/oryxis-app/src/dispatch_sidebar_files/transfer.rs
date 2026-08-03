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
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                let basename = path
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or(&path)
                    .to_string();
                // One-shot transfer with a toast on completion. Heavier
                // moves (progress, queues, retries) live in the full SFTP
                // session one context-menu entry away.
                Task::perform(
                    async move {
                        let dest = rfd::AsyncFileDialog::new()
                            .set_file_name(&basename)
                            .save_file()
                            .await?
                            .path()
                            .to_path_buf();
                        Some(match client.download_to(&path, &dest, None).await {
                            Ok(()) => crate::i18n::t("files_download_done")
                                .replacen("{name}", &basename, 1),
                            Err(e) => format!("{basename}: {e}"),
                        })
                    },
                    |msg| match msg {
                        Some(m) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesOpToast(m)),
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
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let mut failed: Vec<String> = Vec::new();
                        let mut ok = 0usize;
                        for local in paths {
                            let name = local
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "file".to_string());
                            let remote = files_join(&dir, &name);
                            match client.upload_from(&local, &remote).await {
                                Ok(()) => ok += 1,
                                Err(e) => failed.push(format!("{name}: {e}")),
                            }
                        }
                        if failed.is_empty() {
                            crate::i18n::t("files_upload_done")
                                .replacen("{n}", &ok.to_string(), 1)
                        } else {
                            failed.join(" · ")
                        }
                    },
                    move |toast| Message::SidebarFiles(SidebarFilesMessage::SidebarFilesUploadFinished(pane_id, toast)),
                )
            }
            SidebarFilesMessage::SidebarFilesUploadFinished(pane_id, toast) => {
                let toast_task = self.update(Message::SidebarFiles(SidebarFilesMessage::SidebarFilesOpToast(toast)));
                // Refresh the pane's CURRENT listing through the normal
                // stamped pipeline: the bump happens synchronously with
                // `loading = true`, and the completion (listed or error)
                // always resolves it, so no stuck states are possible.
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return toast_task;
                };
                let Some(client) = pane.files.client.clone() else {
                    return toast_task;
                };
                let path = pane.files.path.clone();
                pane.files.loading = true;
                pane.files.error = None;
                let seq = pane.files.next_req();
                Task::batch([
                    toast_task,
                    list_dir_task(client, path, pane_id, seq),
                ])
            }
            SidebarFilesMessage::SidebarFilesEdit(path) => {
                self.overlay = None;
                let Some(client) = self.active_pane_mut().and_then(|p| p.files.client.clone())
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
