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
use crate::messages::Message;
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

impl Oryxis {
    pub(crate) fn handle_sidebar_files(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::SidebarFilesRowHovered(idx) => {
                self.hovered_files_row = Some(idx);
            }
            Message::SidebarFilesRowUnhovered => {
                self.hovered_files_row = None;
            }
            Message::SidebarFilesToggleFollow => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.files.follow_disabled = !pane.files.follow_disabled;
                }
                // Re-enabling the pin snaps the browser back to the
                // shell's directory right away.
                return Ok(self.sidebar_files_sync());
            }
            Message::SidebarFilesToggleHidden => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.files.show_hidden = !pane.files.show_hidden;
                }
            }
            Message::SidebarFilesRefresh => {
                let Some(pane) = self.active_pane_mut() else {
                    return Ok(Task::none());
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
                        return Ok(list_dir_task(client, path, pane_id, seq));
                    }
                    // Not mounted (or a failed mount): retry from scratch.
                    _ => return Ok(self.sidebar_files_sync()),
                }
            }
            Message::SidebarFilesNavigate(path) => {
                let Some(pane) = self.active_pane_mut() else {
                    return Ok(Task::none());
                };
                let Some(client) = pane.files.client.clone() else {
                    return Ok(Task::none());
                };
                // A manual navigation away from the shell's cwd would be
                // undone by the next follow sync, so browsing by hand
                // implies unpinning; the toggle re-enables it.
                if pane.files.follow() && pane.cwd.as_deref() != Some(path.as_str()) {
                    pane.files.follow_disabled = true;
                }
                let pane_id = pane.id;
                pane.files.loading = true;
                pane.files.error = None;
                // Rapid clicks race their listings; the stamp makes the
                // LATEST navigation win regardless of completion order.
                let seq = pane.files.next_req();
                return Ok(list_dir_task(client, path, pane_id, seq));
            }
            Message::SidebarFilesExpand => {
                // Promote to a full SFTP tab at the browser's directory.
                // The hint is consumed by the SFTP mount pipeline
                // (`initial_remote_listing`), falling back to the home
                // directory if the path stopped existing.
                let Some(tab_idx) = self.active_tab else {
                    return Ok(Task::none());
                };
                let path = self
                    .tabs
                    .get(tab_idx)
                    .map(|t| t.active().files.path.clone())
                    .filter(|p| !p.is_empty());
                self.sftp_open_at_path = path;
                return Ok(self.update(Message::OpenSftpForTab(tab_idx)));
            }
            Message::SidebarFilesMounted(pane_id, seq, client, path, mut entries) => {
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return Ok(Task::none());
                };
                // Superseded (a newer request, or a disconnect reset that
                // bumped the stamp): the channel may ride a dead handle,
                // drop it instead of installing a client that can only
                // error. Also guards the reconnect race where the pane
                // has a NEW session by the time the old mount lands.
                if pane.files.req_seq != seq {
                    return Ok(Task::none());
                }
                if pane.session.as_ref().and_then(|s| s.ssh()).is_none() {
                    return Ok(Task::none());
                }
                sort_entries(&mut entries);
                pane.files.client = Some(client);
                pane.files.mounting = false;
                pane.files.loading = false;
                pane.files.error = None;
                pane.files.path = path;
                pane.files.entries = entries;
            }
            Message::SidebarFilesListed(pane_id, seq, path, mut entries) => {
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return Ok(Task::none());
                };
                // Out-of-order completion of a superseded listing: drop,
                // the newer request's result is the one that must win.
                if pane.files.req_seq != seq {
                    return Ok(Task::none());
                }
                sort_entries(&mut entries);
                pane.files.loading = false;
                pane.files.error = None;
                pane.files.path = path;
                pane.files.entries = entries;
                // The shell may have moved again while this listing was
                // in flight; chase it so follow never sticks one step
                // behind a fast `cd a && cd b`.
                return Ok(self.sidebar_files_sync());
            }
            Message::SidebarFilesError(pane_id, seq, e) => {
                let Some(pane) = self.pane_by_id_any_tab(pane_id) else {
                    return Ok(Task::none());
                };
                // A stale error must not clear the flags (or paint the
                // banner) of a newer in-flight request.
                if pane.files.req_seq != seq {
                    return Ok(Task::none());
                }
                pane.files.mounting = false;
                pane.files.loading = false;
                pane.files.error = Some(e);
            }
            other => return Err(other),
        }
        Ok(Task::none())
    }

    /// The active tab's focused pane, mutably. `None` outside a
    /// terminal tab.
    fn active_pane_mut(&mut self) -> Option<&mut crate::state::Pane> {
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
            let hint = if pane.files.follow() { pane.cwd.clone() } else { None };
            let seq = pane.files.next_req();
            return Task::perform(
                async move {
                    let client = ssh.open_sftp().await.map_err(|e| e.to_string())?;
                    let (path, entries) =
                        crate::dispatch_sftp::initial_remote_listing(&client, hint).await?;
                    Ok::<_, String>((client, path, entries))
                },
                move |result| match result {
                    Ok((client, path, entries)) => {
                        Message::SidebarFilesMounted(pane_id, seq, client, path, entries)
                    }
                    Err(e) => Message::SidebarFilesError(pane_id, seq, e),
                },
            );
        }

        // Mounted: follow the shell if it moved.
        if pane.files.follow()
            && !pane.files.loading
            && let Some(cwd) = pane.cwd.clone()
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

/// One directory listing on the sidebar browser's channel. `seq` is the
/// request stamp compared on completion (latest request wins).
fn list_dir_task(
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
            Ok((path, entries)) => Message::SidebarFilesListed(pane_id, seq, path, entries),
            Err(e) => Message::SidebarFilesError(pane_id, seq, e),
        },
    )
}
