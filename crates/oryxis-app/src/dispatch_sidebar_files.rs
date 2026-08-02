//! Sidebar Files tab: a per-pane SFTP browser multiplexed on the pane's
//! live SSH session, with follow-cwd driven by the OSC 7 the terminal
//! already captures. Mounting is lazy (first time the tab shows) and
//! every async result routes by the pane's stable `Uuid`, so pane / tab
//! switches mid-flight can't land a listing on the wrong browser.

// The `Err(message)` pass-through of the try_handler! chain carries the full
// Message enum by design; same allowance as the sibling dispatch modules.
#![allow(clippy::result_large_err)]

use iced::Task;
use uuid::Uuid;

use crate::app::Oryxis;
use crate::messages::{Message, SidebarFilesMessage, TabsMessage, SftpMessage};
use crate::state::TerminalSidebarTab;

/// Dirs first, then case-insensitive by name, the sidebar's fixed sort
/// (the full SFTP pane has sortable columns; this browser does not).
fn sort_entries(entries: &mut [oryxis_ssh::SftpEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Parent of an absolute POSIX path, `None` at the root.
pub(crate) fn files_parent_dir(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let idx = trimmed.rfind('/')?;
    Some(if idx == 0 { "/".to_string() } else { trimmed[..idx].to_string() })
}

/// Join an entry name onto the browser's current directory.
pub(crate) fn files_join(path: &str, name: &str) -> String {
    if path == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", path.trim_end_matches('/'))
    }
}

/// Working-directory fallback for shells without OSC 7 integration:
/// the stock Debian/Ubuntu/Fedora PS1 titles the window `\u@\h: \w`,
/// so an OSC 0/2 title like `root@web: /var/www` carries the cwd.
/// Extracts the trailing path (absolute or `~`-relative); anything
/// else (`vim main.rs`, plain program names) yields `None`.
pub(crate) fn title_cwd(title: &str) -> Option<&str> {
    // Preferred: the "\u@\h: \w" form (Debian/Ubuntu default, colon +
    // space). Fallback: the no-space "\u@\h:\w" some PS1s use, taken
    // only when the head looks like `user@host` so a stray "note:foo"
    // title can't masquerade as a cwd.
    let tail = title
        .rsplit_once(": ")
        .map(|(_, t)| t)
        .or_else(|| {
            let (head, t) = title.rsplit_once(':')?;
            head.contains('@').then_some(t)
        })
        .unwrap_or(title);
    let tail = tail.trim();
    (tail.starts_with('/') || tail == "~" || tail.starts_with("~/")).then_some(tail)
}

/// Expand a possibly `~`-relative cwd (the title fallback) against the
/// session's home directory. Absolute paths pass through; `~` without
/// a known home resolves to `None` (can't follow yet, the mount's
/// canonicalize will supply the home).
fn expand_cwd(cwd: &str, home: Option<&str>) -> Option<String> {
    if cwd.starts_with('/') {
        return Some(cwd.to_string());
    }
    let home = home?.trim_end_matches('/');
    if cwd == "~" {
        return Some(home.to_string());
    }
    cwd.strip_prefix("~/").map(|rest| format!("{home}/{rest}"))
}

/// Cap per host, matching the per-pane dropdown's own cap: the list is
/// there to be scanned, not archived.
const FILES_RECENT_CAP: usize = 20;

impl Oryxis {
    /// Record a visited folder in the persistent, host-keyed history and
    /// write it back. No-op for panes with no saved host (quick-connect,
    /// local, cloud), which have no stable key to file it under.
    fn record_files_recent(&mut self, pane_id: uuid::Uuid, path: &str) {
        if path.is_empty() {
            return;
        }
        let host = self
            .tabs
            .iter()
            .flat_map(|t| t.pane_grid.panes.values())
            .find(|p| p.id == pane_id)
            .and_then(|p| match p.origin {
                crate::state::PaneOrigin::Host(id) => Some(id),
                _ => None,
            });
        let Some(host) = host else {
            return;
        };
        let list = self.files_recent_folders.entry(host).or_default();
        if list.first().is_some_and(|p| p == path) {
            // Already on top: the optimistic navigate records the same
            // path twice (mount then listing), so this is the common case
            // and it must not cost a vault write.
            return;
        }
        list.retain(|p| p != path);
        list.insert(0, path.to_string());
        list.truncate(FILES_RECENT_CAP);
        if let Ok(json) = serde_json::to_string(&self.files_recent_folders) {
            self.persist_setting("files_recent_folders", &json);
        }
    }

    /// Refill a pane's dropdown from the stored history for its host.
    /// Called when the Files sidebar mounts, which is what makes the
    /// disconnect-time wipe harmless.
    fn hydrate_files_recent(&mut self, pane_id: uuid::Uuid) {
        let host = self
            .tabs
            .iter()
            .flat_map(|t| t.pane_grid.panes.values())
            .find(|p| p.id == pane_id)
            .and_then(|p| match p.origin {
                crate::state::PaneOrigin::Host(id) => Some(id),
                _ => None,
            });
        let Some(stored) = host.and_then(|h| self.files_recent_folders.get(&h)).cloned() else {
            return;
        };
        if let Some(pane) = self.pane_by_id_any_tab(pane_id)
            && pane.files.path_history.is_empty()
        {
            pane.files.path_history = stored;
        }
    }

    pub(crate) fn handle_sidebar_files(
        &mut self,
        message: SidebarFilesMessage,
    ) -> Task<Message> {
        match message {
            SidebarFilesMessage::SidebarFilesRowHovered(idx) => {
                self.hovered_files_row = Some(idx);
            }
            SidebarFilesMessage::SidebarFilesRowUnhovered => {
                self.hovered_files_row = None;
            }
            SidebarFilesMessage::SidebarFilesToggleFollow => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.files.follow_disabled = !pane.files.follow_disabled;
                }
                // Re-enabling the pin snaps the browser back to the
                // shell's directory right away.
                return self.sidebar_files_sync();
            }
            SidebarFilesMessage::SidebarFilesToggleHidden => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.files.show_hidden = !pane.files.show_hidden;
                }
            }
            SidebarFilesMessage::SidebarFilesRefresh => {
                // Also fired from the background context menu.
                self.overlay = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                pane.files.error = None;
                match (&pane.files.client, pane.files.path.is_empty()) {
                    // Mounted: re-list the current directory.
                    (Some(client), false) => {
                        let client = client.clone();
                        let path = pane.files.path.clone();
                        let pane_id = pane.id;
                        pane.files.loading = true;
                        let seq = pane.files.next_req();
                        return list_dir_task(client, path, pane_id, seq);
                    }
                    // Not mounted (or a failed mount): retry from scratch.
                    _ => return self.sidebar_files_sync(),
                }
            }
            SidebarFilesMessage::SidebarFilesNavigate(path) => {
                // Also fired from the row context menu; dismiss it.
                self.overlay = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                // Clicking a row while the path input is open is its
                // blur: close the edit (its buffer is stale the moment
                // the listing changes) so the header snaps back to the
                // label + actions.
                pane.files.path_editing = None;
                // A manual navigation away from the shell's cwd would be
                // undone by the next follow sync, so browsing by hand
                // implies unpinning; the toggle re-enables it. The toast
                // makes the silent state flip visible (owner QA ask).
                let mut unpinned = false;
                if pane.files.follow()
                    && pane
                        .cwd
                        .as_deref()
                        .and_then(|c| expand_cwd(c, pane.files.home.as_deref()))
                        .as_deref()
                        != Some(path.as_str())
                {
                    pane.files.follow_disabled = true;
                    unpinned = true;
                }
                let pane_id = pane.id;
                // Optimistic UI: adopt the target path on screen NOW and
                // clear the old listing, so the click answers instantly
                // (the roundtrip used to look like a freeze, owner QA).
                // Clearing is also correctness: keeping the OLD rows
                // visible under the NEW path would let a rapid second
                // click join a stale entry name onto the wrong base.
                // The ".." row derives from the optimistic path, so
                // navigating up mid-load stays coherent; the listing
                // that lands (stamp-guarded) replaces everything.
                pane.files.path = path.clone();
                pane.files.entries.clear();
                pane.files.loading = true;
                pane.files.error = None;
                // Rapid clicks race their listings; the stamp makes the
                // LATEST navigation win regardless of completion order.
                let seq = pane.files.next_req();
                let list = list_dir_task(client, path, pane_id, seq);
                if unpinned {
                    self.set_toast(crate::i18n::t("files_follow_paused").to_string());
                    return Task::batch([
                        list,
                        Task::perform(
                            async {
                                tokio::time::sleep(std::time::Duration::from_millis(
                                    2500,
                                ))
                                .await;
                            },
                            |_| Message::ToastClear,
                        ),
                    ]);
                }
                return list;
            }
            SidebarFilesMessage::SidebarFilesExpand => {
                // Expand = this tab's SFTP session (the hybrid Files
                // mode) at the browser's current directory. Owner QA
                // 2026-07-05: expanding must NOT open a standalone tab.
                let path = self
                    .active_pane_mut()
                    .map(|p| p.files.path.clone())
                    .unwrap_or_default();
                return self.update(Message::SidebarFiles(SidebarFilesMessage::SidebarFilesOpenSftpAt(path)));
            }
            SidebarFilesMessage::SidebarFilesOpenSftpAt(path) => {
                // Flip the active tab into its SFTP session at `path`.
                // The one-shot hint is consumed by the toggle's mount
                // (or by a navigate when the session already exists),
                // with home fallback if the path stopped existing.
                self.overlay = None;
                let Some(tab_idx) = self.active_tab else {
                    return Task::none();
                };
                self.sftp_open_at_path = (!path.is_empty()).then_some(path);
                return self.update(Message::Tabs(TabsMessage::ToggleTabFilesMode(tab_idx)));
            }
            SidebarFilesMessage::ShowSidebarFilesRowMenu(path, is_dir) => {
                let anchor = self.keynav_take_menu_anchor();
                self.overlay = Some(crate::state::OverlayState {
                    content: crate::state::OverlayContent::SidebarFilesRow { path, is_dir },
                    x: anchor.0,
                    y: anchor.1,
                });
            }
            SidebarFilesMessage::ShowSidebarFilesBackgroundMenu => {
                // Directory-level menu for the current folder; only once
                // mounted (an unmounted browser has nothing to act on).
                let Some(dir) = self
                    .active_pane_mut()
                    .filter(|p| p.files.client.is_some() && !p.files.path.is_empty())
                    .map(|p| p.files.path.clone())
                else {
                    return Task::none();
                };
                let anchor = self.keynav_take_menu_anchor();
                self.overlay = Some(crate::state::OverlayState {
                    content: crate::state::OverlayContent::SidebarFilesBackground { dir },
                    x: anchor.0,
                    y: anchor.1,
                });
            }
            SidebarFilesMessage::SidebarFilesStartRename(path) => {
                self.overlay = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                let basename = path
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or(&path)
                    .to_string();
                pane.files.new_entry = None;
                pane.files.rename = Some((path, basename));
                return iced::widget::operation::focus(iced::widget::Id::new(
                    "sidebar-files-rename",
                ));
            }
            SidebarFilesMessage::SidebarFilesRenameInput(s) => {
                if let Some(pane) = self.active_pane_mut()
                    && let Some((_, input)) = pane.files.rename.as_mut()
                {
                    *input = s;
                }
            }
            SidebarFilesMessage::SidebarFilesRenameCommit => {
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                let Some((original, input)) = pane.files.rename.take() else {
                    return Task::none();
                };
                let name = input.trim();
                // Same guard as the SFTP pane's rename: one plain path
                // component (rejects empty, ".", ".." and separators).
                if !crate::sftp_helpers::is_safe_remote_entry_name(name) {
                    return Task::none();
                }
                let parent = files_parent_dir(&original).unwrap_or_else(|| "/".to_string());
                let target = files_join(&parent, name);
                if target == original {
                    return Task::none();
                }
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                let list_path = pane.files.path.clone();
                let pane_id = pane.id;
                pane.files.loading = true;
                pane.files.error = None;
                let seq = pane.files.next_req();
                return op_then_list(
                    client.clone(),
                    list_path,
                    pane_id,
                    seq,
                    async move { client.rename(&original, &target).await },
                );
            }
            SidebarFilesMessage::SidebarFilesStartNewEntry(kind) => {
                self.overlay = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                if pane.files.client.is_none() {
                    return Task::none();
                }
                pane.files.rename = None;
                pane.files.new_entry = Some((kind, String::new()));
                return iced::widget::operation::focus(iced::widget::Id::new(
                    "sidebar-files-new",
                ));
            }
            SidebarFilesMessage::SidebarFilesNewEntryInput(s) => {
                if let Some(pane) = self.active_pane_mut()
                    && let Some((_, input)) = pane.files.new_entry.as_mut()
                {
                    *input = s;
                }
            }
            SidebarFilesMessage::SidebarFilesNewEntryCommit => {
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                let Some((kind, input)) = pane.files.new_entry.take() else {
                    return Task::none();
                };
                let name = input.trim();
                // One plain path component (rejects empty, ".", ".." and
                // separators), same guard as the rename commit.
                if !crate::sftp_helpers::is_safe_remote_entry_name(name) {
                    return Task::none();
                }
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                let target = files_join(&pane.files.path, name);
                let list_path = pane.files.path.clone();
                let pane_id = pane.id;
                pane.files.loading = true;
                pane.files.error = None;
                let seq = pane.files.next_req();
                let exists_msg = crate::i18n::t("files_entry_exists")
                    .replacen("{name}", name, 1);
                return op_then_list(client.clone(), list_path, pane_id, seq, async move {
                    match kind {
                        crate::state::SftpEntryKind::Folder => client.create_dir(&target).await,
                        // Exclusive create: colliding with an existing name
                        // must error out, not silently truncate it to zero
                        // bytes (which a plain CREATE|TRUNCATE write does).
                        // The stat pre-check turns the server's opaque
                        // EXCL failure into a readable message; EXCL still
                        // closes the check-to-create race.
                        crate::state::SftpEntryKind::File => {
                            if client.stat(&target).await.is_ok() {
                                return Err(oryxis_ssh::SshError::Channel(exists_msg));
                            }
                            client.create_file_exclusive(&target).await
                        }
                    }
                });
            }
            SidebarFilesMessage::SidebarFilesDelete(path, is_dir) => {
                self.overlay = None;
                let name = path
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or(&path)
                    .to_string();
                // Shared destructive-confirm dialog (Enter confirms via
                // the modal keynav router).
                self.confirm_remove(
                    name,
                    Message::SidebarFiles(SidebarFilesMessage::SidebarFilesDeleteConfirmed(path, is_dir)),
                );
            }
            SidebarFilesMessage::SidebarFilesDeleteConfirmed(path, is_dir) => {
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                let list_path = pane.files.path.clone();
                let pane_id = pane.id;
                pane.files.loading = true;
                pane.files.error = None;
                let seq = pane.files.next_req();
                return op_then_list(client.clone(), list_path, pane_id, seq, async move {
                    if is_dir {
                        client.remove_dir_recursive(&path).await
                    } else {
                        client.remove_file(&path).await
                    }
                });
            }
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
                return Task::perform(
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
                );
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
                return Task::perform(
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
                );
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
                return Task::perform(
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
                );
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
                return Task::batch([
                    toast_task,
                    list_dir_task(client, path, pane_id, seq),
                ]);
            }
            SidebarFilesMessage::SidebarFilesOpToast(text) => {
                self.set_toast(text);
                return Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                    },
                    |_| Message::ToastClear,
                );
            }
            SidebarFilesMessage::SidebarFilesShowProperties(path, is_dir) => {
                self.overlay = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                let stat_client = client.clone();
                let target = path.clone();
                return Task::perform(
                    async move { stat_client.stat(&target).await.map_err(|e| e.to_string()) },
                    move |result| match result {
                        Ok(stat) => {
                            let mode = stat.permissions.unwrap_or(0o644);
                            Message::Sftp(SftpMessage::SftpPropertiesLoaded(crate::state::PropertiesView {
                                // `side` is unused when a client override
                                // is present; Right is a stable filler.
                                side: crate::state::SftpPaneSide::Right,
                                client_override: Some(client.clone()),
                                from_sidebar: true,
                                path: path.clone(),
                                is_dir,
                                size: stat.size,
                                mtime: stat.mtime,
                                owner_uid: stat.uid,
                                owner_gid: stat.gid,
                                original_mode: mode,
                                bits: crate::state::PermBits::from_mode(mode),
                                mode_input: format!("{:03o}", mode & 0o777),
                                applying: false,
                                error: None,
                            }))
                        }
                        // One-shot op, not a listing: its failure surfaces
                        // as a toast (like download/upload) instead of a
                        // SidebarFilesError, whose un-bumped stamp would
                        // alias an in-flight listing's and paint an error
                        // over it / clear its loading flag.
                        Err(e) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesOpToast(e)),
                    },
                );
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
                return self.start_edit_watch(
                    crate::state::SftpPaneSide::Right,
                    path,
                    crate::state::SftpEditOpener::OsDefault,
                    Some((client, host)),
                    true,
                );
            }
            SidebarFilesMessage::SidebarFilesStartEditPath => {
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                if pane.files.client.is_none() {
                    return Task::none();
                }
                pane.files.path_editing = Some(pane.files.path.clone());
                // Drop the keyboard straight into the input (mirrors the
                // SFTP pane's path editing).
                return iced::widget::operation::focus(iced::widget::Id::new(
                    "sidebar-files-path",
                ));
            }
            SidebarFilesMessage::SidebarFilesEditPath(s) => {
                if let Some(pane) = self.active_pane_mut()
                    && pane.files.path_editing.is_some()
                {
                    pane.files.path_editing = Some(s);
                }
            }
            SidebarFilesMessage::SidebarFilesEditBlur => {
                if let Some(pane) = self.active_pane_mut() {
                    let files = &mut pane.files;
                    files.path_editing = None;
                    files.rename = None;
                    files.new_entry = None;
                    files.path_history_open = false;
                }
            }
            SidebarFilesMessage::SidebarFilesPathHistoryToggle => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.files.path_history_open = !pane.files.path_history_open;
                }
            }
            SidebarFilesMessage::SidebarFilesPathHistoryClose => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.files.path_history_open = false;
                }
            }
            SidebarFilesMessage::SidebarFilesPathHistoryPick(path) => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.files.path_history_open = false;
                }
                // Navigate also closes a pending path edit, so picking
                // from the dropdown mid-edit leaves a clean header.
                return Task::done(Message::SidebarFiles(
                    SidebarFilesMessage::SidebarFilesNavigate(path),
                ));
            }
            SidebarFilesMessage::SidebarFilesCommitPath => {
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                let Some(input) = pane.files.path_editing.take() else {
                    return Task::none();
                };
                let input = input.trim().to_string();
                if input.is_empty() {
                    return Task::none();
                }
                // `~`-relative input expands against the session home;
                // anything else goes to the server's canonicalize (which
                // resolves relative segments) before listing.
                let target = expand_cwd(&input, pane.files.home.as_deref())
                    .unwrap_or(input);
                let Some(client) = pane.files.client.clone() else {
                    return Task::none();
                };
                // Typing a path away from the shell's cwd unpins follow,
                // same rule as clicking into a folder.
                if pane.files.follow() {
                    pane.files.follow_disabled = true;
                }
                let pane_id = pane.id;
                // Same optimistic adoption as SidebarFilesNavigate; the
                // canonicalized path from the listing replaces this
                // best-effort value when it lands.
                pane.files.path = target.clone();
                pane.files.entries.clear();
                pane.files.loading = true;
                pane.files.error = None;
                let seq = pane.files.next_req();
                return Task::perform(
                    async move {
                        let path = client
                            .canonicalize(&target)
                            .await
                            .map_err(|e| e.to_string())?;
                        let entries =
                            client.list_dir(&path).await.map_err(|e| e.to_string())?;
                        Ok::<_, String>((path, entries))
                    },
                    move |result| match result {
                        Ok((path, entries)) => {
                            Message::SidebarFiles(SidebarFilesMessage::SidebarFilesListed(pane_id, seq, path, entries))
                        }
                        Err(e) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesError(pane_id, seq, e)),
                    },
                );
            }
            SidebarFilesMessage::SidebarFilesMounted(pane_id, seq, client, home, path, mut entries) => {
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return Task::none();
                };
                // Superseded (a newer request, or a disconnect reset that
                // bumped the stamp): the channel may ride a dead handle,
                // drop it instead of installing a client that can only
                // error. Also guards the reconnect race where the pane
                // has a NEW session by the time the old mount lands.
                if pane.files.req_seq != seq {
                    return Task::none();
                }
                if pane.session.as_ref().and_then(|s| s.ssh()).is_none() {
                    return Task::none();
                }
                sort_entries(&mut entries);
                pane.files.client = Some(client);
                pane.files.home = home;
                pane.files.mounting = false;
                pane.files.loading = false;
                pane.files.error = None;
                // Adopted-directory history (issue #85): recorded only
                // here and in Listed, i.e. once a path proved listable.
                // Unconditional: the optimistic navigate already set
                // `files.path`, so an equality guard would skip exactly
                // the visits that matter (the dedupe makes re-recording
                // the current directory a no-op).
                let previous = std::mem::take(&mut pane.files.path);
                pane.files.push_path_history(path.clone());
                pane.files.path = path.clone();
                if previous != path {
                    pane.files.push_nav(previous);
                }
                pane.files.entries = entries;
                // Mount is where the stored, host-keyed history comes back
                // (the per-pane list is wiped on disconnect on purpose),
                // and where this visit joins it.
                self.hydrate_files_recent(pane_id);
                self.record_files_recent(pane_id, &path);
                // The title-fallback cwd may be `~`-relative and only
                // expandable now that the home is known; chase it.
                return self.sidebar_files_sync();
            }
            SidebarFilesMessage::SidebarFilesListed(pane_id, seq, path, mut entries) => {
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return Task::none();
                };
                // Out-of-order completion of a superseded listing: drop,
                // the newer request's result is the one that must win.
                if pane.files.req_seq != seq {
                    return Task::none();
                }
                sort_entries(&mut entries);
                pane.files.loading = false;
                pane.files.error = None;
                // Unconditional for the same optimistic-path reason as
                // the Mounted arm above.
                let previous = std::mem::take(&mut pane.files.path);
                pane.files.push_path_history(path.clone());
                pane.files.path = path.clone();
                if previous != path {
                    pane.files.push_nav(previous);
                }
                pane.files.entries = entries;
                self.record_files_recent(pane_id, &path);
                // The shell may have moved again while this listing was
                // in flight; chase it so follow never sticks one step
                // behind a fast `cd a && cd b`.
                return self.sidebar_files_sync();
            }
            SidebarFilesMessage::SidebarFilesError(pane_id, seq, e) => {
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return Task::none();
                };
                // A stale error must not clear the flags (or paint the
                // banner) of a newer in-flight request.
                if pane.files.req_seq != seq {
                    return Task::none();
                }
                pane.files.mounting = false;
                pane.files.loading = false;
                pane.files.error = Some(e);
            }
        }
        Task::none()
    }

    /// The active tab's focused pane, mutably. `None` outside a
    /// terminal tab.
    pub(crate) fn active_pane_mut(&mut self) -> Option<&mut crate::state::Pane> {
        let idx = self.active_tab?;
        Some(self.tabs.get_mut(idx)?.active_mut())
    }

    /// Find a pane by its stable id across every tab (async results
    /// arrive after the user may have switched tabs / panes).
    fn pane_by_id_any_tab(&mut self, pane_id: Uuid) -> Option<&mut crate::state::Pane> {
        self.tabs
            .iter_mut()
            .flat_map(|t| t.pane_grid.panes.values_mut())
            .find(|p| p.id == pane_id)
    }

    /// Bring the visible Files browser in line with its pane: mount the
    /// SFTP channel if the tab just opened, or chase the shell's OSC 7
    /// cwd when follow is on. Idempotent and cheap when nothing needs
    /// doing, so every entry point (tab select, sidebar open, pane
    /// focus, cwd change) just calls it.
    pub(crate) fn sidebar_files_sync(&mut self) -> Task<Message> {
        // Only the visible browser drives SFTP traffic; a background
        // pane's cwd changes are picked up when its tab shows again.
        if self.effective_sidebar_tab() != Some(TerminalSidebarTab::Files) {
            return Task::none();
        }
        let Some(pane) = self.active_pane_mut() else {
            return Task::none();
        };
        let Some(ssh) = pane.session.as_ref().and_then(|s| s.ssh()).cloned() else {
            return Task::none();
        };
        if !ssh.is_alive() {
            return Task::none();
        }
        let pane_id = pane.id;

        // Not mounted yet: open the channel and land on the shell's cwd
        // (when following) or the home directory.
        if pane.files.client.is_none() {
            if pane.files.mounting {
                return Task::none();
            }
            pane.files.mounting = true;
            pane.files.error = None;
            // The pre-mount hint can only use an absolute cwd (a
            // `~`-relative title fallback has no home to expand against
            // yet; the mount lands on the home anyway and the post-mount
            // chase in SidebarFilesMounted finishes the job).
            let hint = if pane.files.follow() {
                pane.cwd.as_deref().and_then(|c| expand_cwd(c, None))
            } else {
                None
            };
            let seq = pane.files.next_req();
            return Task::perform(
                async move {
                    let client = ssh.open_sftp().await.map_err(|e| e.to_string())?;
                    // Session home, resolved once: expands `~`-relative
                    // cwds from the title fallback.
                    let home = client.canonicalize(".").await.ok();
                    let (path, entries) =
                        crate::dispatch_sftp::initial_remote_listing(&client, hint).await?;
                    Ok::<_, String>((client, home, path, entries))
                },
                move |result| match result {
                    Ok((client, home, path, entries)) => {
                        Message::SidebarFiles(SidebarFilesMessage::SidebarFilesMounted(pane_id, seq, client, home, path, entries))
                    }
                    Err(e) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesError(pane_id, seq, e)),
                },
            );
        }

        // Mounted: follow the shell if it moved.
        if pane.files.follow()
            && !pane.files.loading
            && let Some(cwd) = pane
                .cwd
                .as_deref()
                .and_then(|c| expand_cwd(c, pane.files.home.as_deref()))
            && cwd != pane.files.path
        {
            let client = pane.files.client.clone().expect("checked above");
            pane.files.loading = true;
            let seq = pane.files.next_req();
            return list_dir_task(client, cwd, pane_id, seq);
        }
        Task::none()
    }
}

/// Run a mutation (rename / create / delete) then re-list the current
/// directory, all on the sidebar browser's channel; the completion
/// carries the request stamp like any listing.
fn op_then_list(
    client: oryxis_ssh::SftpClient,
    list_path: String,
    pane_id: Uuid,
    seq: u64,
    op: impl std::future::Future<Output = Result<(), oryxis_ssh::SshError>> + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            op.await.map_err(|e| e.to_string())?;
            let entries = client
                .list_dir(&list_path)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((list_path, entries))
        },
        move |result| match result {
            Ok((path, entries)) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesListed(pane_id, seq, path, entries)),
            Err(e) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesError(pane_id, seq, e)),
        },
    )
}

/// One directory listing on the sidebar browser's channel. `seq` is the
/// request stamp compared on completion (latest request wins).
/// `pub(crate)`: the OS-drop upload refreshes the visible listing on
/// completion through this (`drop.rs`), pinned to its pane id rather
/// than riding `SidebarFilesRefresh`, whose "active pane" can have
/// changed during a long upload.
pub(crate) fn list_dir_task(
    client: oryxis_ssh::SftpClient,
    path: String,
    pane_id: Uuid,
    seq: u64,
) -> Task<Message> {
    Task::perform(
        async move {
            let entries = client.list_dir(&path).await.map_err(|e| e.to_string())?;
            Ok::<_, String>((path, entries))
        },
        move |result| match result {
            Ok((path, entries)) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesListed(pane_id, seq, path, entries)),
            Err(e) => Message::SidebarFiles(SidebarFilesMessage::SidebarFilesError(pane_id, seq, e)),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_cwd_extracts_stock_ps1_titles() {
        // Stock Debian/Ubuntu PS1: \u@\h: \w
        assert_eq!(title_cwd("root@web-01: /var/www"), Some("/var/www"));
        assert_eq!(title_cwd("root@web-01: ~"), Some("~"));
        assert_eq!(title_cwd("u@h: ~/projects/api"), Some("~/projects/api"));
        // Colons inside the path segment: the LAST ": " wins.
        assert_eq!(title_cwd("u@h: /data/a: b"), None); // "b" is not a path
        assert_eq!(title_cwd("note: see: /etc"), Some("/etc"));
        // No-space "\u@\h:\w" form (the head has an '@' so it's trusted).
        assert_eq!(title_cwd("root@web-01:/var/www"), Some("/var/www"));
        assert_eq!(title_cwd("root@web-01:~"), Some("~"));
        // A bare absolute path as the whole title.
        assert_eq!(title_cwd("/srv/app"), Some("/srv/app"));
    }

    #[test]
    fn title_cwd_rejects_non_path_titles() {
        assert_eq!(title_cwd("vim main.rs"), None);
        assert_eq!(title_cwd("htop"), None);
        assert_eq!(title_cwd(""), None);
        assert_eq!(title_cwd("root@web-01"), None);
        // Windows-style path in a title is not a POSIX cwd.
        assert_eq!(title_cwd(r"cmd: C:\Users\x"), None);
        // A bare "~x" user-home form is ambiguous; declined.
        assert_eq!(title_cwd("u@h: ~other"), None);
    }

    #[test]
    fn expand_cwd_handles_absolute_and_home_relative() {
        assert_eq!(expand_cwd("/var/www", None).as_deref(), Some("/var/www"));
        assert_eq!(expand_cwd("~", Some("/root")).as_deref(), Some("/root"));
        assert_eq!(
            expand_cwd("~/a/b", Some("/home/u/")).as_deref(),
            Some("/home/u/a/b")
        );
        // Home unknown: `~` can't expand yet.
        assert_eq!(expand_cwd("~", None), None);
        assert_eq!(expand_cwd("~/x", None), None);
    }

    #[test]
    fn files_join_and_parent_are_inverse_at_the_root() {
        assert_eq!(files_join("/", "etc"), "/etc");
        assert_eq!(files_join("/var/www", "html"), "/var/www/html");
        assert_eq!(files_parent_dir("/etc").as_deref(), Some("/"));
        assert_eq!(files_parent_dir("/var/www/html").as_deref(), Some("/var/www"));
        assert_eq!(files_parent_dir("/"), None);
    }
}
