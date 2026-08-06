//! Hybrid terminal+SFTP tab handlers (issue #61) split out of
//! `dispatch_tabs`: toggle Files mode, detach the SFTP session to a
//! standalone tab, close just the SFTP session, and open a terminal
//! for a standalone SFTP tab's host. Called from `handle_tabs`.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{TabsMessage, SshMessage, Message, Oryxis, SftpMessage};

impl Oryxis {
    pub(super) fn handle_toggle_tab_files_mode(&mut self, idx: usize) -> Task<Message> {
        // Fired from the tab context menu (among others):
        // dismiss it so it doesn't linger over the new surface.
        self.overlay = None;
        // Hybrid tab (issue #61): flip this SSH tab between its
        // terminal and its host's files (the full dual-pane SFTP
        // surface). The PTY keeps running underneath; the SFTP
        // state parks in `TerminalTab::files_state` when hidden.
        let Some(tab) = self.tabs.get(idx) else {
            return Task::none();
        };
        let tab_id = tab._id;
        // Clicking the glyph on a background tab also brings the
        // tab to front, whichever direction it flips.
        let select = if self.active_tab != Some(idx) {
            self.update(Message::Tabs(TabsMessage::SelectTab(idx)))
        } else {
            Task::none()
        };
        if self.tabs[idx].files_mode {
            // Back to the terminal: the browsing state goes home.
            // A stray one-shot directory hint dies here too.
            self.sftp_open_at_path = None;
            self.tabs[idx].files_mode = false;
            self.park_hybrid_sftp();
            return select;
        }
        // Turning ON requires the SFTP feature (optional, hidden
        // when off; this guards the hotkey path which bypasses
        // the gated UI). Turning OFF above is always allowed.
        if !self.sftp_enabled {
            self.sftp_open_at_path = None;
            return select;
        }
        // Files mode needs a live SSH session (SFTP is an SSH
        // subsystem; local / Telnet / serial tabs never show the
        // glyph, this guards the hotkey path).
        let Some(session) = self.tabs[idx]
            .active()
            .session
            .as_ref()
            .and_then(|s| s.ssh())
            .cloned()
        else {
            self.sftp_open_at_path = None;
            return select;
        };
        // Resolve by the FOCUSED pane (a split tab can host two
        // different servers; the tab label only names the first):
        // its label for the ad-hoc mount, its origin id for the
        // saved-connection lookup (immune to renames).
        let base = self.tabs[idx]
            .active()
            .label
            .trim_end_matches(" (disconnected)")
            .to_string();
        let origin_conn = match &self.tabs[idx].active().origin {
            crate::state::PaneOrigin::Host(hid) => Some(*hid),
            _ => None,
        };
        self.tabs[idx].files_mode = true;
        self.hoist_hybrid_sftp(tab_id);
        // Already mounted from an earlier visit: just show it,
        // navigating to the one-shot directory hint when an
        // expand/context-menu affordance carried one. Only a mount
        // whose session is still alive qualifies; a dead one (the tab
        // reconnected while Files was parked and the automatic remount
        // didn't land, issue #63) falls through to the mount pipeline
        // below, which reuses this tab's fresh session.
        if self.sftp.right.is_remote && self.sftp.right.host_label.is_some() {
            if self
                .sftp
                .right
                .session
                .as_ref()
                .is_some_and(|s| s.is_alive())
            {
                if let Some(p) = self.sftp_open_at_path.take() {
                    let nav = self.update(Message::Sftp(SftpMessage::SftpNavigateRemote(
                        crate::state::SftpPaneSide::Right,
                        p,
                    )));
                    return Task::batch([select, nav]);
                }
                return select;
            }
            // Land the remount at the previous directory (home
            // fallback); an explicit pending hint keeps priority.
            if self.sftp_open_at_path.is_none() {
                self.sftp_open_at_path = Some(self.sftp.right.remote_path.clone())
                    .filter(|p| !p.is_empty());
            }
        }
        // First open: seed the Local pane like a fresh SFTP tab,
        // then mount the host into the right pane.
        if self.sftp.left.local_path.as_os_str().is_empty() {
            self.sftp.left.local_path = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/"));
            self.sftp.left.columns = self.sftp_chrome.columns_template.clone();
            self.sftp.right.columns = self.sftp_chrome.columns_template.clone();
        }
        self.refresh_sftp_local(crate::state::SftpPaneSide::Left);
        // Saved host: the shared mount pipeline (reuse-or-connect)
        // finds this tab's live session by label and multiplexes
        // an SFTP channel on it, no second dial. Origin id wins
        // over the label match (rename-proof).
        if let Some(ci) = self
            .connections
            .iter()
            .position(|c| {
                origin_conn == Some(c.id)
                    && c.protocol
                        == oryxis_core::models::connection::ConnectionProtocol::Ssh
            })
            .or_else(|| {
                self.connections.iter().position(|c| {
                    c.label == base
                        && c.protocol
                            == oryxis_core::models::connection::ConnectionProtocol::Ssh
                })
            })
        {
            let mount = self.update(Message::Sftp(SftpMessage::SftpRemountPane(
                crate::state::SftpPaneSide::Right,
                ci,
            )));
            return Task::batch([select, mount]);
        }
        // Ad-hoc host (quick connect / cloud): mount the live
        // session directly, mirroring OpenSftpForTab's fallback.
        {
            let pane = self.sftp.pane_mut(crate::state::SftpPaneSide::Right);
            pane.is_remote = true;
            pane.host_label = Some(base.clone());
            pane.remote_loading = true;
            pane.error = None;
            pane.remote_entries.clear();
        }
        let target = crate::state::SftpPaneSide::Right;
        let session_for_task = session.clone();
        let label = base;
        // One-shot directory hint from the expand affordances.
        let initial_hint = self.sftp_open_at_path.take();
        let mount = Task::perform(
            async move {
                let client = session_for_task
                    .open_sftp()
                    .await
                    .map_err(|e| e.to_string())?;
                let (initial, entries) =
                    crate::dispatch_sftp::initial_remote_listing(
                        &client,
                        initial_hint,
                    )
                    .await?;
                Ok::<_, String>((client, initial, entries))
            },
            // Completion stamped with THIS hybrid tab (hoisted just
            // above): a park/hoist swap while the mount is in
            // flight must not land the result in whichever buffer
            // is live by then. `route_sftp_async` swaps the owner's
            // state back in, or drops the result if the tab closed.
            move |result| match result {
                Ok((client, path, entries)) => Message::sftp_owned(
                    Some(tab_id),
                    SftpMessage::HostMounted(
                        target,
                        label.clone(),
                        session.clone(),
                        client,
                        path,
                        entries,
                    ),
                ),
                Err(e) => Message::sftp_owned(
                    Some(tab_id),
                    SftpMessage::RemoteError(target, e),
                ),
            },
        );
        Task::batch([select, mount])
    }

    pub(super) fn handle_detach_tab_sftp(&mut self, idx: usize) -> Task<Message> {
        // Promote the tab's SFTP session to a standalone SFTP tab
        // (the dual-remote / server-to-server surface). The hybrid
        // state moves out wholesale: live channel, panes, log,
        // any in-flight transfer keeps running under the new
        // owner id via route_sftp_async.
        self.overlay = None;
        let Some(tab) = self.tabs.get(idx) else {
            return Task::none();
        };
        let tab_id = tab._id;
        if !self.tab_has_sftp_session(tab) {
            return Task::none();
        }
        // An in-flight transfer's continuations are stamped with
        // THIS tab's id; moving the state under a new SftpTab id
        // would orphan them mid-run. Decline until it finishes.
        {
            let st: &crate::state::SftpState =
                if self.hybrid_sftp_owner == Some(tab_id) {
                    &self.sftp
                } else {
                    &tab.files_state
                };
            if st.transfer.state.is_some() {
                self.set_toast(
                    crate::i18n::t("tab_detach_sftp_busy").to_string(),
                );
                return Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            2500,
                        ))
                        .await;
                    },
                    |_| Message::ToastClear,
                );
            }
        }
        // The state must be home (parked) before it can move.
        if self.hybrid_sftp_owner == Some(tab_id) {
            self.park_hybrid_sftp();
        }
        let Some(tab) = self.tabs.get_mut(idx) else {
            return Task::none();
        };
        tab.files_mode = false;
        let state = std::mem::take(&mut *tab.files_state);
        let label = state
            .right
            .host_label
            .clone()
            .or_else(|| state.left.host_label.clone())
            .unwrap_or_else(|| crate::i18n::t("sftp").to_string());
        let mut stab = crate::state::SftpTab::new(label);
        stab.state = state;
        let sid = stab.id;
        self.sftp_tabs.push(stab);
        self.tab_order.push(crate::state::TabRef::Sftp(sid));
        let new_idx = self.sftp_tabs.len() - 1;
        self.focus_sftp_tab(new_idx);
        self.active_tab = None;
        self.active_view = crate::state::View::Sftp;
        Task::none()
    }

    pub(super) fn handle_close_tab_sftp_session(&mut self, idx: usize) -> Task<Message> {
        // Close ONLY the hybrid tab's SFTP session: drop the
        // browsing state + channel, back to a plain terminal
        // tab (the mode glyph disappears with the session). The
        // terminal keeps running untouched.
        self.overlay = None;
        let Some(tab) = self.tabs.get(idx) else {
            return Task::none();
        };
        let tab_id = tab._id;
        if !self.tab_has_sftp_session(tab) {
            return Task::none();
        }
        // An in-flight transfer would be killed by dropping the
        // state mid-run, and its continuations are stamped with
        // this still-existing tab id (so they would land on the
        // freshly-reset state); decline until it finishes (same
        // guard as the detach path).
        {
            let st: &crate::state::SftpState =
                if self.hybrid_sftp_owner == Some(tab_id) {
                    &self.sftp
                } else {
                    &tab.files_state
                };
            if st.transfer.state.is_some() {
                self.set_toast(crate::i18n::t("tab_detach_sftp_busy").to_string());
                return Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            2500,
                        ))
                        .await;
                    },
                    |_| Message::ToastClear,
                );
            }
            // Unsaved work beyond the transfer (a dirty external
            // edit whose upload hasn't landed): route through the
            // same confirm modal as the standalone tab close
            // instead of silently discarding the pending save.
            if crate::sftp_methods::sftp_state_has_unsaved(st) {
                self.pending_sftp_close = Some(
                    crate::state::PendingSftpClose::HybridSession(tab_id),
                );
                return Task::none();
            }
        }
        self.close_tab_sftp_session(tab_id)
    }

    pub(super) fn handle_open_terminal_for_sftp_tab(&mut self, idx: usize) -> Task<Message> {
        // From an SFTP tab's menu: the way back to a shell on the
        // mounted host. Focus a live terminal pane on that host,
        // else connect a new tab (the saved-host pipeline).
        self.overlay = None;
        let Some(stab) = self.sftp_tabs.get(idx) else {
            return Task::none();
        };
        let st: &crate::state::SftpState = if self.active_sftp == Some(idx) {
            &self.sftp
        } else {
            &stab.state
        };
        let Some(host) = st
            .right
            .host_label
            .clone()
            .or_else(|| st.left.host_label.clone())
        else {
            return Task::none();
        };
        // Live pane on that host wins (any pane, split included).
        if let Some(t_idx) = self.tabs.iter().position(|t| {
            t.pane_grid.panes.values().any(|p| {
                p.label.trim_end_matches(" (disconnected)") == host
                    && p.session.as_ref().and_then(|s| s.ssh()).is_some()
            })
        }) {
            return self.update(Message::Tabs(TabsMessage::SelectTab(t_idx)));
        }
        if let Some(ci) = self.connections.iter().position(|c| {
            c.label == host
                && c.protocol
                    == oryxis_core::models::connection::ConnectionProtocol::Ssh
        }) {
            return self.update(Message::Ssh(SshMessage::ConnectSsh(ci)));
        }
        Task::none()
    }

}
