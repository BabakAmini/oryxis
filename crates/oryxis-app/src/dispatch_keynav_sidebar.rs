//! Keyboard router for the terminal-sidebar tabs (Chat / Snippets /
//! History / Host config), iteration 3 of the focus-zone framework.
//!
//! Unlike modals and side panels, this surface coexists with a live
//! terminal that owns every plain key, so the layer is strictly
//! opt-in:
//!
//! - The FocusSidebarList hotkey opens the sidebar (when closed) and
//!   cycles every visible tab; landing engages the tab's rows.
//! - Up / Down while the mouse cursor is over a LIST tab engage the
//!   ring directly (those keys were already swallowed there, never
//!   reaching the PTY, so this upgrades a dead key into navigation).
//! - Tab / Shift+Tab walk EVERY recorded row (panel contract: input
//!   rows get real iced focus, the rest get the ring) while the ring
//!   is engaged OR the cursor is over the sidebar. With the cursor
//!   over the terminal and no ring, Tab stays a PTY `\t`.
//!
//! While engaged: Up/Down/Home/End move over non-input rows
//! (wrapping), Enter or Space activates (list rows RUN their
//! command), Shift+Enter pastes without the newline, Left/Right
//! cycle picker rows (the font-size stepper, the chat mode chips)
//! and otherwise move the ring too (the header buttons sit side by
//! side, owner QA), Delete removes (through the row's confirm), Esc
//! disengages. Everything else, typing included, keeps its normal
//! routing, so the terminal (or a focused search field) still
//! receives text while the ring is up; the selection is tagged by
//! sidebar tab and clamped against each frame's recording, so
//! filtering while ringed just clamps.

use iced::keyboard;
use iced::Task;

use crate::app::{AiMessage, Message, Oryxis};
use crate::keynav::movement::index_move;
use crate::keynav::SidebarRow;
use crate::state::TerminalSidebarTab;

/// Blur every focusable (focusing a nonexistent id): moving the ring
/// onto a non-input row takes the keyboard away from whatever input
/// had focus (same trick as the side-panel router).
fn blur_task() -> Task<Message> {
    iced::widget::operation::focus(iced::widget::Id::new("__keynav_blur__"))
}

impl Oryxis {
    /// The sidebar tab actually shown for the active terminal tab
    /// (Chat falls back to Snippets while AI is off), or `None` when
    /// no terminal tab is active or its sidebar is closed. Mirrors
    /// the resolution in `view_terminal_sidebar`.
    pub(crate) fn effective_sidebar_tab(&self) -> Option<TerminalSidebarTab> {
        let tab = self.active_tab.and_then(|i| self.tabs.get(i))?;
        // A hybrid tab in Files mode replaces the whole tab content
        // (sidebar included), so no sidebar tab is effective there.
        if !tab.chat_visible || tab.files_mode {
            return None;
        }
        let files_available = self.sftp_enabled
            && tab.active().session.as_ref().and_then(|s| s.ssh()).is_some();
        let active = if (self.terminal_sidebar_tab == TerminalSidebarTab::Chat
            && !self.ai.enabled)
            || (self.terminal_sidebar_tab == TerminalSidebarTab::Files && !files_available)
            || (self.terminal_sidebar_tab == TerminalSidebarTab::Monitor
                && (!self.setting_host_monitoring
                    || tab.active().session.as_ref().and_then(|s| s.ssh()).is_none()))
        {
            TerminalSidebarTab::Snippets
        } else {
            self.terminal_sidebar_tab
        };
        Some(active)
    }

    /// Whether the mouse cursor currently sits over the (open)
    /// sidebar of the active terminal tab. Keys there never reach
    /// the PTY (the chat-sidebar swallow gate), so promoting them to
    /// navigation costs nothing.
    pub(crate) fn cursor_over_sidebar(&self) -> bool {
        let visible = self
            .active_tab
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.chat_visible)
            .unwrap_or(false);
        if !visible {
            return false;
        }
        // The sidebar hugs whichever edge it is docked on (issue #85),
        // shifted inward by a side-docked tab strip (issue #87): with
        // both on the same edge the sidebar starts AFTER the strip, and
        // classifying the strip band as "sidebar" would let arrow keys
        // over the strip engage the ring while the sidebar's inner edge
        // leaked keys into the PTY.
        let strip_left = self.side_strip_left_offset();
        let strip_right = self.side_strip_reserve() - strip_left;
        if self.setting_terminal_sidebar_left {
            self.mouse_position.x > strip_left
                && self.mouse_position.x < strip_left + self.chat_sidebar_width
        } else {
            let right_edge = self.window_size.width - strip_right;
            self.mouse_position.x > right_edge - self.chat_sidebar_width
                && self.mouse_position.x < right_edge
        }
    }

    /// Close a pending Files path edit and its history dropdown, if
    /// any. Moving the keyboard (or the mouse, or the sidebar tab) off
    /// the path is its blur, and while editing the header hides the
    /// action icons so the input can take the whole width; the blur
    /// must snap that back (owner ask). No-op on every other state.
    pub(crate) fn close_files_path_edit(&mut self) {
        if let Some(idx) = self.active_tab
            && let Some(tab) = self.tabs.get_mut(idx)
        {
            let files = &mut tab.active_mut().files;
            files.path_editing = None;
            files.path_history_open = false;
        }
    }

    /// Whether the recorded row at `idx` is an input row (Tab focuses
    /// it instead of ringing it).
    fn sidebar_row_is_input(&self, idx: usize) -> bool {
        self.keynav
            .sidebar_items
            .borrow()
            .get(idx)
            .is_some_and(|r| r.action.focus.is_some())
    }

    /// Keep the selected row visible; same best-effort relative snap
    /// as the side-panel router (iced exposes no row bounds). Both
    /// list tabs give their scrollable the shared id (only one
    /// renders); tabs without that scrollable no-op.
    fn sidebar_nav_scroll(&self, idx: usize) -> Task<Message> {
        let len = self.keynav.sidebar_items.borrow().len();
        let denom = len.saturating_sub(1).max(1);
        iced::widget::operation::snap_to(
            iced::widget::Id::new("sidebar-list-scroll"),
            iced::widget::operation::RelativeOffset {
                x: None,
                y: Some(idx as f32 / denom as f32),
            },
        )
    }

    /// Tab / Shift+Tab over the recorded sidebar rows (panel
    /// contract): input rows receive real iced focus, non-input rows
    /// show the ring and blur whatever input had the keyboard.
    fn sidebar_nav_tab(&mut self, tab: TerminalSidebarTab, forward: bool) -> Option<Task<Message>> {
        let len = self.keynav.sidebar_items.borrow().len();
        if len == 0 {
            return None;
        }
        let cur = match self.keynav.sidebar_selected {
            Some((tag, idx)) if tag == tab => Some(idx.min(len - 1)),
            _ => None,
        };
        let next = index_move(len, cur, forward)?;
        self.keynav.sidebar_selected = Some((tab, next));
        let action = self.keynav.sidebar_items.borrow().get(next)?.action.clone();
        let step = match action.focus {
            Some(id) => iced::widget::operation::focus(id),
            None => {
                // Walking onto a non-input row blurs whatever input had
                // the keyboard; that blur also closes a pending Files
                // path edit (which was holding the whole header width).
                self.close_files_path_edit();
                blur_task()
            }
        };
        Some(Task::batch([step, self.sidebar_nav_scroll(next)]))
    }

    /// Entry point, called from the `KeyboardEvent` arm right after
    /// the vault-area router. Returns `Some(task)` when consumed.
    pub(crate) fn handle_sidebar_nav_key(
        &mut self,
        event: &keyboard::Event,
    ) -> Option<Task<Message>> {
        let tab = self.effective_sidebar_tab()?;
        if self.any_modal_blocks_input() || self.show_host_panel {
            return None;
        }
        let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            return None;
        };
        let len = self.keynav.sidebar_items.borrow().len();
        // Selection engaged on the visible tab, clamped against this
        // frame's recording (a search filter can shrink it).
        let selected = match self.keynav.sidebar_selected {
            Some((tag, idx)) if tag == tab && len > 0 => Some(idx.min(len - 1)),
            _ => None,
        };
        // Ctrl+F with the keyboard in the sidebar (owner ask): open /
        // focus the active tab's search field instead of sending the
        // readline forward-char to the PTY. Same ownership gate as the
        // Tab walk; tabs without a search decline so nothing surprising
        // gets consumed.
        if modifiers.control()
            && !modifiers.alt()
            && !modifiers.logo()
            && !modifiers.shift()
            && matches!(key, keyboard::Key::Character(c) if c.as_str().eq_ignore_ascii_case("f"))
            && (selected.is_some() || self.cursor_over_sidebar())
        {
            match tab {
                TerminalSidebarTab::Snippets => {
                    // The input's focused border takes over as the
                    // keyboard affordance; a ring left behind on some
                    // other row would read as stuck.
                    self.keynav.sidebar_selected = None;
                    if self.sidebar_search_open {
                        return Some(iced::widget::operation::focus(iced::widget::Id::new(
                            "sidebar-snippet-search",
                        )));
                    }
                    // Opens the field and focuses it (the handler does).
                    return Some(self.update(Message::Ai(AiMessage::ToggleSidebarSearch)));
                }
                TerminalSidebarTab::History => {
                    self.keynav.sidebar_selected = None;
                    return Some(iced::widget::operation::focus(iced::widget::Id::new(
                        "sidebar-history-search",
                    )));
                }
                // No search on Chat / Host config.
                _ => return None,
            }
        }
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        // The ring is "active" only on non-input rows; while the
        // selection points at an input row the real focus owns the
        // keys (typing, caret, the chat editor's own Enter binding).
        let ring = selected.filter(|&i| !self.sidebar_row_is_input(i));

        // Space presses the ringed control, matching the desktop
        // convention (owner ask). Only while ringed: an idle Space
        // belongs to the PTY (and typing there drops the ring anyway).
        let is_space = matches!(key, keyboard::Key::Named(keyboard::key::Named::Space))
            || matches!(key, keyboard::Key::Character(c) if c.as_str() == " ");
        if is_space {
            let idx = ring?;
            let row: SidebarRow = self.keynav.sidebar_items.borrow().get(idx).cloned()?;
            let msg = row.action.activate.or(row.paste)?;
            return Some(self.update(msg));
        }

        let keyboard::Key::Named(named) = key else {
            return None;
        };
        use keyboard::key::Named;
        match named {
            // Tab walks the rows while the sidebar owns the keyboard
            // (ring engaged or cursor over it); otherwise the PTY
            // keeps its literal \t.
            Named::Tab => {
                if selected.is_none() && !self.cursor_over_sidebar() {
                    return None;
                }
                self.sidebar_nav_tab(tab, !modifiers.shift())
            }
            Named::ArrowUp | Named::ArrowDown => {
                if modifiers.shift() || len == 0 {
                    return None;
                }
                let forward = matches!(named, Named::ArrowDown);
                let cur = match selected {
                    Some(cur) => {
                        // Arrows only move from a ringed row; a focused
                        // input keeps its native caret/history keys.
                        ring?;
                        cur
                    }
                    // Not engaged: only the hover gate over a LIST tab
                    // (Snippets / History / Files) turns a dead (already
                    // swallowed) arrow into an entry point; Chat / Host
                    // config need the hotkey or Tab.
                    None if self.cursor_over_sidebar()
                        && matches!(
                            tab,
                            TerminalSidebarTab::Snippets
                                | TerminalSidebarTab::History
                                | TerminalSidebarTab::Files
                        ) =>
                    {
                        // Land straight on the LIST body: first list row
                        // going down, last going up. Header chrome (path,
                        // search, sort, Close) stays reachable by walking
                        // on from there, but entry must never ring it: a
                        // ring popping up on the Files path row reads as a
                        // plain focused text input, not as navigation
                        // (live QA), and ringing Close would put Enter one
                        // keypress away from closing the sidebar.
                        let target = {
                            let items = self.keynav.sidebar_items.borrow();
                            if forward {
                                items.iter().position(|r| r.list)
                            } else {
                                items.iter().rposition(|r| r.list)
                            }
                        };
                        if let Some(t) = target {
                            self.keynav.sidebar_selected = Some((tab, t));
                            self.close_files_path_edit();
                            return Some(Task::batch([
                                blur_task(),
                                self.sidebar_nav_scroll(t),
                            ]));
                        }
                        // No list rows this frame (empty dir / group view):
                        // start on Close so the move below lands on the
                        // first body row going down / wraps to the last
                        // going up.
                        0
                    }
                    None => return None,
                };
                // Hop over input rows (panel contract): arrows are the
                // quick jump between actionable rows, Tab is the full
                // walk.
                let mut next = cur;
                for _ in 0..len {
                    next = index_move(len, Some(next), forward)?;
                    if !self.sidebar_row_is_input(next) {
                        break;
                    }
                }
                if self.sidebar_row_is_input(next) {
                    return Some(Task::none());
                }
                self.keynav.sidebar_selected = Some((tab, next));
                self.close_files_path_edit();
                Some(Task::batch([blur_task(), self.sidebar_nav_scroll(next)]))
            }
            Named::Home | Named::End => {
                ring?;
                if len == 0 {
                    return Some(Task::none());
                }
                let mut idx = if matches!(named, Named::Home) { 0 } else { len - 1 };
                let step_forward = matches!(named, Named::Home);
                for _ in 0..len {
                    if !self.sidebar_row_is_input(idx) {
                        break;
                    }
                    idx = index_move(len, Some(idx), step_forward)?;
                }
                if self.sidebar_row_is_input(idx) {
                    return Some(Task::none());
                }
                self.keynav.sidebar_selected = Some((tab, idx));
                Some(self.sidebar_nav_scroll(idx))
            }
            Named::Enter => {
                let idx = ring?;
                let row: SidebarRow = self.keynav.sidebar_items.borrow().get(idx).cloned()?;
                // Shift+Enter = paste without the newline; plain Enter
                // = the row's primary (list rows RUN their command,
                // buttons/toggles/cards activate). Picker rows consume
                // as a no-op so the key can't leak into the PTY while
                // ringed.
                let msg = if modifiers.shift() {
                    row.paste.or(row.action.activate)
                } else {
                    row.action.activate.or(row.paste)
                };
                Some(match msg {
                    Some(msg) => self.update(msg),
                    None => Task::none(),
                })
            }
            Named::ArrowLeft | Named::ArrowRight => {
                let idx = ring?;
                let action = self.keynav.sidebar_items.borrow().get(idx)?.action.clone();
                let rtl = crate::i18n::is_rtl_layout();
                let forward = matches!(named, Named::ArrowRight) != rtl;
                if action.prev.is_some() || action.next.is_some() {
                    // Picker row: cycle the value in place.
                    let msg = if forward { action.next } else { action.prev };
                    return Some(match msg {
                        Some(msg) => self.update(msg),
                        None => Task::none(),
                    });
                }
                // Non-picker row: move the ring along the recording,
                // hopping over inputs. Owner QA: the sort / search
                // header buttons sit side by side, so switching
                // between them must answer to the horizontal arrows
                // too, not only Up/Down.
                let mut next = idx;
                for _ in 0..len {
                    next = index_move(len, Some(next), forward)?;
                    if !self.sidebar_row_is_input(next) {
                        break;
                    }
                }
                if self.sidebar_row_is_input(next) {
                    return Some(Task::none());
                }
                self.keynav.sidebar_selected = Some((tab, next));
                self.close_files_path_edit();
                Some(Task::batch([blur_task(), self.sidebar_nav_scroll(next)]))
            }
            Named::Delete => {
                let idx = ring?;
                let row = self.keynav.sidebar_items.borrow().get(idx).cloned()?;
                let msg = row.delete?;
                // The recording shrinks next frame; the selection is
                // clamped on the next key, so the ring lands on the
                // neighbor instead of vanishing.
                Some(self.update(msg))
            }
            Named::Escape => {
                // Esc is the "give me the terminal back" key: drop the
                // ring AND blur whatever sidebar input the walk focused
                // (the terminal never holds iced focus, so no focused
                // input means keys route to the PTY again). Also fires
                // with the cursor over the sidebar, where Esc was
                // previously swallowed by the hover gate and did
                // nothing at all.
                if selected.is_some() || self.cursor_over_sidebar() {
                    self.keynav.sidebar_selected = None;
                    // A half-typed Files edit (path / rename / new
                    // entry) cancels with the disengage (mirrors the
                    // SFTP pane's Esc).
                    if let Some(idx) = self.active_tab
                        && let Some(tab) = self.tabs.get_mut(idx)
                    {
                        let files = &mut tab.active_mut().files;
                        files.path_editing = None;
                        files.rename = None;
                        files.new_entry = None;
                        files.path_history_open = false;
                    }
                    return Some(blur_task());
                }
                None
            }
            _ => None,
        }
    }

    /// FocusSidebarList hotkey: bring the keyboard to the sidebar.
    /// Opens it when closed (landing on the tab it already shows);
    /// pressed again it cycles EVERY visible tab, Chat (AI on) ->
    /// Snippets -> History -> HostConfig -> wrap. Landing focuses the
    /// tab's natural entry point: Chat the message editor, History
    /// its search field, Snippets/HostConfig their first row. No-op
    /// outside a terminal tab.
    pub(crate) fn focus_sidebar_list(&mut self) -> Task<Message> {
        let Some(idx) = self.active_tab else {
            return Task::none();
        };
        let mut order: Vec<TerminalSidebarTab> = Vec::with_capacity(5);
        if self.ai.enabled {
            order.push(TerminalSidebarTab::Chat);
        }
        order.extend([TerminalSidebarTab::Snippets, TerminalSidebarTab::History]);
        // Files only exists for an SSH pane with the SFTP feature on
        // (mirrors the strip).
        if self.sftp_enabled
            && self
                .tabs
                .get(idx)
                .map(|t| t.active().session.as_ref().and_then(|s| s.ssh()).is_some())
                .unwrap_or(false)
        {
            order.push(TerminalSidebarTab::Files);
        }
        // Monitor joins the cycle only when the feature is on and an SSH
        // session is live (its per-host opt-in is offered inside the tab),
        // next to Files.
        if self.setting_host_monitoring
            && self
                .active_tab
                .and_then(|idx| self.tabs.get(idx))
                .map(|t| t.active().session.as_ref().and_then(|s| s.ssh()).is_some())
                .unwrap_or(false)
        {
            order.push(TerminalSidebarTab::Monitor);
        }
        order.push(TerminalSidebarTab::HostConfig);

        let was_open = self
            .tabs
            .get(idx)
            .map(|t| t.chat_visible)
            .unwrap_or(false);
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.chat_visible = true;
        }
        let current = self
            .effective_sidebar_tab()
            .unwrap_or(TerminalSidebarTab::Snippets);
        // First press lands on what's already showing; repeats advance.
        let target = if was_open {
            let cur_pos = order.iter().position(|t| *t == current).unwrap_or(0);
            order[(cur_pos + 1) % order.len()]
        } else {
            current
        };
        tracing::debug!(?current, ?target, was_open, "FocusSidebarList");
        self.terminal_sidebar_tab = target;
        match target {
            TerminalSidebarTab::Chat => {
                self.keynav.sidebar_selected = None;
                iced::widget::operation::focus(iced::widget::Id::new("chat-input"))
            }
            TerminalSidebarTab::History => {
                self.refresh_command_history();
                // Owner call: entering History goes straight to its
                // search field (real focus; Tab walks on from there).
                self.keynav.sidebar_selected = None;
                iced::widget::operation::focus(iced::widget::Id::new(
                    "sidebar-history-search",
                ))
            }
            TerminalSidebarTab::Files => {
                // Same first-body-row landing as Snippets/HostConfig,
                // batched with the mount / follow sync so the browser
                // is live (or catching up to the shell) by the time
                // the ring shows.
                self.keynav.sidebar_selected = Some((target, 1));
                Task::batch([self.sidebar_nav_scroll(1), self.sidebar_files_sync()])
            }
            TerminalSidebarTab::Monitor => {
                // Gauges are informational; the only navigable row is the
                // opt-in button (when the host hasn't enabled monitoring
                // yet), which the body records first.
                self.keynav.sidebar_selected = Some((target, 1));
                self.sidebar_nav_scroll(1)
            }
            TerminalSidebarTab::Snippets | TerminalSidebarTab::HostConfig => {
                // Land on the first row of the tab BODY. Index 0 is the
                // header's Close button (the strip records first, on
                // these tabs exactly one action; Chat's extra Reset
                // never applies here), and landing there would put
                // Enter one keypress away from closing the sidebar.
                // Next frame's recording; the slot only draws the ring
                // on non-input rows, and Enter/Tab dive into inputs.
                self.keynav.sidebar_selected = Some((target, 1));
                self.sidebar_nav_scroll(1)
            }
        }
    }
}
