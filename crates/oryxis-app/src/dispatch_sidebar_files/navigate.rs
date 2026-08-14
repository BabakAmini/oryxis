//! Moving around: hover, the two view toggles, refresh, and the
//! three ways out of the current directory (walk into it, promote
//! the browser to a full SFTP surface, follow the shell).
//!
//! `SidebarFilesNavigate` is the one place that unpins the OSC 7
//! follow, so manual navigation wins until the pin is re-enabled.

use std::time::Duration;

use super::*;

/// Double-click window, matching the SFTP pane's constant.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

impl Oryxis {
    pub(super) fn handle_sidebar_files_navigate(
        &mut self,
        message: SidebarFilesMessage,
    ) -> Task<Message> {
        match message {
            SidebarFilesMessage::SidebarFilesRowHovered(idx) => {
                self.hover.files_row = Some(idx);
            }
            SidebarFilesMessage::SidebarFilesRowUnhovered(idx) => {
                self.hover.leave_files_row(idx);
            }
            SidebarFilesMessage::SidebarFilesSelectRow(path, is_dir) => {
                // Single-click selects the row (highlight); double-click
                // on a directory enters it, matching the SFTP pane's rule.
                // The click hands the cursor to the mouse: a keynav ring
                // engaged elsewhere is dropped so Enter can't act on a row
                // the user just clicked away from (the selection anchors
                // the next arrow entry right here instead).
                self.keynav.sidebar_selected = None;
                let Some(pane) = self.active_pane_mut() else {
                    return Task::none();
                };
                // Clicking a row is also the path editor's blur, same
                // rule as Navigate: close the edit so the header snaps
                // back to the label + actions.
                pane.files.path_editing = None;
                pane.files.path_history_open = false;
                let now = std::time::Instant::now();
                let is_double = pane.files.last_click.as_ref().is_some_and(
                    |(t, p)| p == &path && now.duration_since(*t) < DOUBLE_CLICK_WINDOW,
                );
                pane.files.last_click = Some((now, path.clone()));
                pane.files.selected = Some(path.clone());
                if is_double && is_dir {
                    pane.files.last_click = None;
                    pane.files.selected = None;
                    return self.update(Message::SidebarFiles(
                        SidebarFilesMessage::SidebarFilesNavigate(path),
                    ));
                }
                // Arm a drag-out (issue #167) on FILE rows where a
                // backend can serve it: crossing the movement threshold
                // while the button is still down turns this press into
                // an OS drag (see the check in `dispatch.rs`); a plain
                // click stays exactly the select it always was. The
                // payload is resolved NOW, while the pane is in hand;
                // the press anchor was synced at the top of `update`.
                let arm = (!is_dir && crate::drag_out::supported())
                    .then(|| {
                        let name = files_basename(&path);
                        let size = pane
                            .files
                            .entries
                            .iter()
                            .find(|e| e.name == name)
                            .map(|e| e.size)
                            .unwrap_or(0);
                        match pane.files.client.as_ref()? {
                            crate::local_files::FilesClient::Local(_) => {
                                Some(crate::drag_out::DragOutPayload::Local(vec![
                                    std::path::PathBuf::from(&path),
                                ]))
                            }
                            crate::local_files::FilesClient::Sftp(client) => {
                                Some(crate::drag_out::DragOutPayload::Remote {
                                    client: client.clone(),
                                    files: vec![crate::drag_out::RemoteDragFile {
                                        path: path.clone(),
                                        name,
                                        size,
                                    }],
                                })
                            }
                        }
                    })
                    .flatten();
                self.drag_out_arm = arm.map(|payload| crate::drag_out::DragOutArm {
                    press: self.mouse_position,
                    payload,
                });
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
                // The selection is deliberately kept: the rows stay on
                // screen while the re-list is in flight, and the prune
                // on arrival keeps it only if its entry survived.
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
                // Also fired from the row context menu and the
                // ".." row; dismiss the overlay and clear selection.
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
                pane.files.selected = None;
                pane.files.last_click = None;
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
            // The parent routed us here, so anything else is a
            // grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
