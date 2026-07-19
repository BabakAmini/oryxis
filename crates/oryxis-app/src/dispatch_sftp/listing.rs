//! Navigation + listing arms split out of `dispatch_sftp`: remote and
//! local navigation (including ".."), listing application with stale
//! guards, refresh, the path-bar edit flow, sort / filter / hidden
//! toggles and list scroll tracking. Called from `handle_sftp`.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{Message, Oryxis, SftpMessage};
use crate::sftp_helpers::{parent_path, sort_local_entries, sort_remote_entries};

impl Oryxis {
    pub(super) fn handle_sftp_listing(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::Sftp(SftpMessage::SftpNavigateRemote(side, path)) => {
                // Also dismiss any open menu (Refresh routes here).
                self.sftp.close_menus();
                // Zip-browse interception: a synthetic `<archive>!/...`
                // target relists from the cached index (no I/O); any
                // real path leaves browse mode and navigates normally.
                if let Some(zip) = &self.sftp.pane(side).zip {
                    if let Some(inner) = zip.inner_from_synthetic(&path) {
                        return Ok(Task::done(Message::Sftp(SftpMessage::SftpZipNavigate(side, inner))));
                    }
                    self.sftp.pane_mut(side).zip = None;
                }
                let client = match self.sftp.pane(side).client.clone() {
                    Some(c) => c,
                    None => {
                        // No client to load from: drop any cursor target
                        // queued for this side so a later successful load
                        // doesn't consume a stale one.
                        if matches!(&self.sftp.pending_focus, Some((s, _)) if *s == side) {
                            self.sftp.pending_focus = None;
                        }
                        return Ok(Task::none());
                    }
                };
                // Stamp a fresh listing seq (shared global counter) so a
                // slower earlier listing can't overwrite this navigation
                // when it lands, and a listing can't land on the wrong
                // surface's pane after a hybrid park/hoist swap.
                let seq = crate::sftp_methods::next_list_seq();
                {
                    let pane = self.sftp.pane_mut(side);
                    pane.remote_loading = true;
                    pane.remote_list_seq = seq;
                    pane.error = None;
                }
                let target = path.clone();
                return Ok(Task::perform(
                    async move { client.list_dir(&target).await.map_err(|e| e.to_string()) },
                    move |result| match result {
                        Ok(entries) => Message::Sftp(SftpMessage::SftpRemoteLoaded(side, seq, path.clone(), entries)),
                        Err(e) => Message::Sftp(SftpMessage::RemoteError(side, e)),
                    },
                ));
            }
            Message::Sftp(SftpMessage::SftpRemoteLoaded(side, seq, path, entries)) => {
                // Drop a stale listing: only the most recently spawned
                // navigation for this pane may apply (mirrors the local
                // path). A global seq also means a listing from another
                // surface, swapped out by park/hoist, can never match.
                if self.sftp.pane(side).remote_list_seq != seq {
                    return Ok(Task::none());
                }
                let sort = self.sftp.pane(side).sort;
                let mut entries = entries;
                sort_remote_entries(&mut entries, sort);
                let entry_count = entries.len();
                let path_for_log = path.clone();
                let pane = self.sftp.pane_mut(side);
                // Only a genuine directory change resets the scroll. A
                // same-path reload (Refresh, post-op reload) keeps the
                // scrollable's id, so iced preserves the visual scroll;
                // zeroing list_scroll_y there would desync our tracked
                // offset from the widget and break edge-based scrolling.
                let changed_dir = pane.remote_path != path;
                pane.remote_path = path;
                pane.remote_entries = entries;
                pane.remote_loading = false;
                if changed_dir {
                    pane.list_scroll_y = 0.0;
                }
                // Selection is path-keyed; navigation invalidates it.
                self.sftp.selected_rows.clear();
                self.sftp.selection_anchor = None;
                self.sftp.parent_cursor = false;
                self.push_sftp_log(
                    crate::state::SftpLogLevel::Info,
                    format!(
                        "{} {} ({} {})",
                        crate::i18n::t("sftp_log_listed"),
                        path_for_log,
                        entry_count,
                        crate::i18n::t("sftp_log_items"),
                    ),
                );
                // Folder descent / back-navigation: now the listing is in,
                // drop the keyboard cursor where the move queued it.
                if let Some(task) = self.sftp_take_pending_focus(side) {
                    return Ok(task);
                }
            }
            Message::Sftp(SftpMessage::SftpUp(side)) => {
                // Inside a browsed archive, ".." climbs the virtual tree
                // and leaves the archive at its root.
                if let Some(zip) = &self.sftp.pane(side).zip {
                    if zip.inner.is_empty() {
                        return Ok(Task::done(Message::Sftp(SftpMessage::SftpZipClose(side))));
                    }
                    let parent = match zip.inner.rsplit_once('/') {
                        Some((head, _)) => head.to_string(),
                        None => String::new(),
                    };
                    return Ok(Task::done(Message::Sftp(SftpMessage::SftpZipNavigate(side, parent))));
                }
                if self.sftp.pane(side).is_remote {
                    let cur = self.sftp.pane(side).remote_path.clone();
                    // Land the cursor on the folder we're leaving once the
                    // parent loads (its full path in the parent listing).
                    let child = cur.trim_end_matches('/').to_string();
                    if !child.is_empty() {
                        self.sftp.pending_focus =
                            Some((side, crate::state::SftpPendingFocus::Path(child)));
                    }
                    let parent = parent_path(&cur);
                    return Ok(Task::done(Message::Sftp(SftpMessage::SftpNavigateRemote(side, parent))));
                }
                if let Some(p) = self.sftp.pane(side).local_path.parent() {
                    let p = p.to_path_buf();
                    // The folder we're leaving, as it'll appear in the parent.
                    let child = self
                        .sftp
                        .pane(side)
                        .local_path
                        .to_string_lossy()
                        .into_owned();
                    {
                        let pane = self.sftp.pane_mut(side);
                        pane.local_path = p;
                        // New directory -> fresh scrollable starts at the top.
                        pane.list_scroll_y = 0.0;
                    }
                    self.sftp.selected_rows.clear();
                    self.sftp.selection_anchor = None;
                    self.sftp.parent_cursor = false;
                    self.refresh_sftp_local(side);
                    // Local listing is synchronous: focus the folder we left.
                    return Ok(self.sftp_apply_pending_focus(
                        side,
                        crate::state::SftpPendingFocus::Path(child),
                    ));
                }
            }
            Message::Sftp(SftpMessage::SftpNavigateLocal(side, path)) => {
                // Zip-browse interception, mirroring SftpNavigateRemote.
                if let Some(zip) = &self.sftp.pane(side).zip {
                    if let Some(inner) = zip.inner_from_synthetic(&path.to_string_lossy()) {
                        return Ok(Task::done(Message::Sftp(SftpMessage::SftpZipNavigate(side, inner))));
                    }
                    self.sftp.pane_mut(side).zip = None;
                }
                {
                    let pane = self.sftp.pane_mut(side);
                    // Only a real directory change resets the scroll (see
                    // SftpRemoteLoaded): a same-path navigate keeps the
                    // scrollable id and its preserved scroll position.
                    let changed_dir = pane.local_path != path;
                    pane.local_path = path.clone();
                    pane.local_entries.clear();
                    pane.error = None;
                    pane.drives_open = false;
                    pane.actions_open = false;
                    if changed_dir {
                        pane.list_scroll_y = 0.0;
                    }
                }
                self.sftp.left.actions_open = false;
                self.sftp.right.actions_open = false;
                self.sftp.selected_rows.clear();
                self.sftp.selection_anchor = None;
                self.sftp.parent_cursor = false;
                // Listing runs off-thread: a cold path (network drive,
                // spun-down disk) can block read_dir for seconds and a
                // synchronous call here froze the whole UI (Windows then
                // offers to kill the "not responding" window, which read
                // as a crash in the field). SftpLocalListed applies the
                // rows and lands any queued folder-descent focus.
                return Ok(self.spawn_local_listing(side, path));
            }
            Message::Sftp(SftpMessage::SftpLocalListed(side, seq, path, result)) => {
                // Stale guard: only the most recently spawned listing
                // for this pane may apply; anything older is a leftover
                // from a navigation the user already moved past.
                if self.sftp.pane(side).local_list_seq != seq {
                    return Ok(Task::none());
                }
                match result {
                    Ok(mut entries) => {
                        let sort = self.sftp.pane(side).sort;
                        crate::sftp_helpers::sort_local_entries(&mut entries, sort);
                        let pane = self.sftp.pane_mut(side);
                        if pane.is_remote {
                            return Ok(Task::none());
                        }
                        if pane.local_path != path {
                            // Typed/pasted path commit: adopt it now that
                            // it's proven listable.
                            pane.local_path = path;
                            pane.list_scroll_y = 0.0;
                        }
                        pane.local_entries = entries;
                        pane.error = None;
                        if let Some(task) = self.sftp_take_pending_focus(side) {
                            return Ok(task);
                        }
                    }
                    Err(e) => {
                        let pane = self.sftp.pane_mut(side);
                        if pane.is_remote {
                            return Ok(Task::none());
                        }
                        // Navigate case (path already adopted): show the
                        // error in place, the ".." row remains the way
                        // out. Commit case (path not adopted): keep the
                        // current listing and surface the error.
                        pane.error = Some(e);
                    }
                }
            }
            Message::Sftp(SftpMessage::SftpRefreshLocal(side)) => {
                self.sftp.close_menus();
                self.refresh_sftp_local(side);
            }
            Message::Sftp(SftpMessage::SftpToggleHidden(side)) => {
                self.sftp.close_menus();
                let pane = self.sftp.pane_mut(side);
                pane.show_hidden = !pane.show_hidden;
            }
            Message::Sftp(SftpMessage::SftpFilter(side, s)) => {
                self.sftp.pane_mut(side).filter = s;
            }
            Message::Sftp(SftpMessage::SftpStartEditPath(side)) => {
                let value = if self.sftp.pane(side).is_remote {
                    self.sftp.pane(side).remote_path.clone()
                } else {
                    self.sftp.pane(side).local_path.display().to_string()
                };
                self.sftp.pane_mut(side).path_editing = Some(value);
            }
            Message::Sftp(SftpMessage::SftpEditPath(side, s)) => {
                if self.sftp.pane(side).path_editing.is_some() {
                    self.sftp.pane_mut(side).path_editing = Some(s);
                }
            }
            Message::Sftp(SftpMessage::SftpCommitPath(side)) => {
                let Some(input) = self.sftp.pane_mut(side).path_editing.take() else {
                    return Ok(Task::none());
                };
                // Pasted paths arrive decorated: Explorer's "Copy as path"
                // wraps them in double quotes and stray whitespace rides
                // along from terminals / chat. Strip both so a paste lands
                // on the directory instead of a "Not a directory" error.
                let input = input.trim();
                let input = input
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(input)
                    .to_string();
                if input.is_empty() {
                    return Ok(Task::none());
                }
                if self.sftp.pane(side).is_remote {
                    return Ok(Task::done(Message::Sftp(SftpMessage::SftpNavigateRemote(side, input))));
                }
                // Probe + list off-thread; the path is only adopted by
                // SftpLocalListed once it proves listable. A synchronous
                // is_dir()/read_dir here froze the UI on cold paths
                // (network drives, spun-down disks) long enough for
                // Windows to kill the window.
                return Ok(
                    self.spawn_local_listing(side, std::path::PathBuf::from(input))
                );
            }
            Message::Sftp(SftpMessage::SftpCancelEditPath) => {
                self.sftp.left.path_editing = None;
                self.sftp.right.path_editing = None;
            }
            Message::Sftp(SftpMessage::SftpSort(side, col)) => {
                {
                    let pane = self.sftp.pane_mut(side);
                    if pane.sort.column == col {
                        pane.sort.ascending = !pane.sort.ascending;
                    } else {
                        pane.sort.column = col;
                        pane.sort.ascending = true;
                    }
                }
                let sort = self.sftp.pane(side).sort;
                if self.sftp.pane(side).is_remote {
                    sort_remote_entries(&mut self.sftp.pane_mut(side).remote_entries, sort);
                } else {
                    sort_local_entries(&mut self.sftp.pane_mut(side).local_entries, sort);
                }
            }
            Message::Sftp(SftpMessage::SftpListScrolled(side, offset_y, viewport_h)) => {
                let pane = self.sftp.pane_mut(side);
                pane.list_scroll_y = offset_y;
                pane.list_viewport_h = viewport_h;
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }
}
