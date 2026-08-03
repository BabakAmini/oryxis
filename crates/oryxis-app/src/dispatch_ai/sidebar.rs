//! The terminal sidebar's chrome: which tab is showing, its search and
//! sort affordances, and the drag that resizes it.
//!
//! Not chat-specific, despite living with the AI dispatch: the same
//! panel hosts Snippets, History and Files, and switching between them
//! is the same message.

use iced::Task;
use crate::app::{SftpMessage, AiMessage, Message, Oryxis};


impl Oryxis {
    pub(super) fn handle_ai_sidebar(&mut self, message: AiMessage) -> Task<Message> {
        match message {
            AiMessage::ToggleChatSidebar => {
                let toggled_to = if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                {
                    tab.chat_visible = !tab.chat_visible;
                    Some(tab.chat_visible)
                } else {
                    None
                };
                if toggled_to == Some(true) {
                    // Opening: land on the configured default tab
                    // (issue #85), resolved against what the pane offers
                    // so a gated default (Files/Monitor with no SSH, Chat
                    // with AI off) never opens an empty panel. "Last
                    // opened" (`None`) keeps the remembered tab, only
                    // applying the legacy Chat->Snippets fallback so the
                    // remembered tab survives a temporary loss of its gate.
                    use crate::state::TerminalSidebarTab;
                    if let Some(default) = self.prefs.sidebar_default_tab {
                        self.terminal_sidebar_tab =
                            self.resolve_available_sidebar_tab(default);
                    } else if !self.ai.enabled
                        && self.terminal_sidebar_tab == TerminalSidebarTab::Chat
                    {
                        self.terminal_sidebar_tab = TerminalSidebarTab::Snippets;
                    }
                    // Opening onto the Files tab: mount / catch up to the
                    // shell's cwd (no-op on every other tab).
                    return self.sidebar_files_sync();
                }
                if toggled_to == Some(false) {
                    // Closing the panel is the user's "stop it" gesture (the
                    // reported bug: a runaway tool loop kept running after the
                    // sidebar was closed). Cancel any live chat work so it
                    // doesn't keep executing commands in the background.
                    self.abort_active_chat_task();
                    // A closed sidebar can't keep a keynav ring: it would
                    // silently swallow Enter/arrows meant for the terminal.
                    // Same for the dropdown gate: a HostConfig pick_list
                    // open at close time unmounts without on_close.
                    self.keynav.sidebar_selected = None;
                    self.keynav.pick_open = false;
                }
            }
            AiMessage::SelectTerminalSidebarTab(tab) => {
                // A HostConfig dropdown open when the sidebar tab swaps
                // unmounts without on_close; drop the gate with it.
                self.keynav.pick_open = false;
                // Leaving the Files tab is a blur for its path edit; a
                // stale full-width input waiting behind the tab switch
                // would read as broken on return.
                self.close_files_path_edit();
                self.terminal_sidebar_tab = tab;
                if tab == crate::state::TerminalSidebarTab::History {
                    self.refresh_command_history();
                    // Owner call: entering History lands the keyboard in
                    // its search field. No-op on the empty state, whose
                    // frame renders no such input.
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "sidebar-history-search",
                    ));
                }
                if tab == crate::state::TerminalSidebarTab::Files {
                    // Mount the pane's SFTP channel (first open) or catch
                    // up to the shell's cwd.
                    return self.sidebar_files_sync();
                }
            }
            AiMessage::SidebarSnippetSearchChanged(v) => {
                self.sidebar_snippet_search = v;
            }
            AiMessage::ToggleSidebarSort => {
                self.sidebar_sort_open = !self.sidebar_sort_open;
                if self.sidebar_sort_open {
                    self.sidebar_search_open = false;
                }
            }
            AiMessage::ToggleSidebarSearch => {
                self.sidebar_search_open = !self.sidebar_search_open;
                self.sidebar_sort_open = false;
                if self.sidebar_search_open {
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "sidebar-snippet-search",
                    ));
                }
                // Collapsing clears the needle so the list shows everything.
                self.sidebar_snippet_search.clear();
            }
            AiMessage::ChatSidebarResizeStart => {
                // Capture cursor x and current width, the MouseMoved
                // handler computes the delta against these.
                self.chat_ui.sidebar_drag = Some((self.mouse_position.x, self.chat_ui.sidebar_width));
            }
            AiMessage::ChatSidebarResizeStop => {
                self.chat_ui.sidebar_drag = None;
                // The same global Left-release ends an SFTP divider drag;
                // persist the final ratio so it survives a relaunch.
                if self.sftp_chrome.split_drag.take().is_some() {
                    self.persist_setting(
                        "sftp_split_ratio",
                        &format!("{:.4}", self.sftp_chrome.split_ratio),
                    );
                }
                // Same Left-release ends a log-panel resize; persist the
                // final height so it survives a relaunch.
                if self.sftp_chrome.log_drag.take().is_some() {
                    self.persist_setting(
                        "sftp_log_height",
                        &format!("{:.0}", self.sftp.log_height),
                    );
                }
                // End a column resize: the width was updated live, so just
                // re-seed the template and persist.
                if let Some((side, _, _, _)) = self.sftp_chrome.col_resize.take() {
                    self.sftp_chrome.columns_template = self.sftp.pane(side).columns.clone();
                    self.persist_sftp_columns();
                }
                // A tab released over the content area merges into the
                // tab showing there instead of reordering (issue #112).
                // Runs first and consumes the drag on success, so the
                // reorder path below sees nothing left to do. Nothing is
                // proposed unless the cursor sits on a split anchor, so
                // an ordinary reorder release falls straight through.
                self.merge_dragged_tab_if_proposed();
                // Ends a tab reorder drag. The live-slide already moved
                // the tab into place during the drag (see TabHovered); on
                // drop we just persist the new pinned order (if the dragged
                // tab is pinned) and clear. A plain click (never promoted to
                // `active`) clears with no persist. Runs BEFORE any early
                // return below: a release that also finished a column
                // sort / SFTP drag / armed a rename used to skip this,
                // leaving the ghost chip stuck on screen (field report).
                if let Some(drag) = self.tab_drag.take()
                    && drag.active
                {
                    // Persist when the dragged tab (terminal or SFTP) is pinned,
                    // so the rearranged pinned order survives a relaunch.
                    let pinned = self
                        .tabs
                        .iter()
                        .find(|t| t._id == drag.from_id)
                        .map(|t| t.pinned)
                        .or_else(|| {
                            self.sftp_tabs
                                .iter()
                                .find(|t| t.id == drag.from_id)
                                .map(|t| t.pinned)
                        })
                        .unwrap_or(false);
                    if pinned {
                        self.persist_pinned_tabs();
                    }
                }
                // End a column reorder. If the drag went active, move the
                // dragged column before whichever header the cursor is over;
                // a release without movement is a plain click that sorts.
                if let Some(drag) = self.sftp_chrome.col_drag.take() {
                    let hovered = self.sftp_chrome.hovered_col;
                    self.sftp_chrome.hovered_col = None;
                    if drag.active {
                        // Name is never a drop target: nothing can be dropped
                        // onto/before it (so it shows no drop effect and keeps
                        // its slot). It can still be dragged elsewhere itself.
                        if let Some((hside, hcol)) = hovered
                            && hside == drag.side
                            && hcol != drag.col
                            && hcol != crate::state::SftpColumn::Name
                        {
                            self.sftp.pane_mut(drag.side).columns.reorder(drag.col, hcol);
                            self.sftp_chrome.columns_template =
                                self.sftp.pane(drag.side).columns.clone();
                            self.persist_sftp_columns();
                        }
                    } else if let Some(sort_col) = drag.col.sort_column() {
                        return Task::done(Message::Sftp(SftpMessage::SftpSort(drag.side, sort_col)));
                    }
                }
                // Same global Left-release event also ends an internal
                // SFTP drag. If the drag was active, dispatch the transfer;
                // otherwise it was a plain click, which may have armed a
                // slow-click rename (set on the press in SftpSelectRow).
                if let Some(drag) = self.sftp.drag.take()
                    && drag.active
                {
                    self.sftp.pending_rename = None;
                    return self.handle_internal_drag_drop(drag);
                }
                if self.sftp.pending_rename.is_some() {
                    return self.defer_slow_rename();
                }
            }
            // Routed here by `handle_ai`; anything else is a
            // grouping mistake rather than a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
