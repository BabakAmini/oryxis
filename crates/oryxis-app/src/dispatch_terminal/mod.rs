//! `Oryxis::handle_terminal`, match arms for terminal I/O: PTY bytes
//! coming back, keyboard events routed to the active tab, split-pane
//! management, the scrollback find-bar, broadcast input, the paste
//! paths and the terminal context menu. The router fans `Message`
//! variants out to per-area submodules:
//!
//! - `output`  : the `PtyOutput` firehose + the batched session-log
//!   flush machinery (and its timing/alignment helpers).
//! - `keyboard`: the `KeyboardEvent` chord resolver and PTY key
//!   routing.
//!
//! The small arms (pane focus/split/close, search bar, broadcast
//! toggles, paste/copy, context menu, IME commit) stay here.

#![allow(clippy::result_large_err)]

mod keyboard;
mod output;

use iced::Task;

use crate::app::{TabsMessage, TerminalMessage, Message, Oryxis};

impl Oryxis {
    /// Tear down every remote session (SSH or Telnet) in a tab.
    /// Dropping the pane alone is not enough: the connect stream task
    /// holds its own Arc to the session, so without an explicit
    /// close() the engine tasks, the channel, and any per-connection
    /// port-forward listeners keep running (and generating UI
    /// messages) forever.
    pub(crate) fn close_tab_sessions(tab: &crate::state::TerminalTab) {
        for pane in tab.pane_grid.panes.values() {
            if let Some(session) = &pane.session {
                session.close();
            }
        }
    }

    /// Paste `text` into the active tab's session. Careful-paste gate:
    /// when the setting is on (default) and the text contains a line
    /// break, the paste is parked in `pending_paste` and a confirmation
    /// dialog (line count + preview) takes over, so a hidden trailing
    /// newline can't auto-run a command. Single-line pastes, and every
    /// paste when the guard is off, go straight to the session.
    pub(crate) fn paste_text_into_active(&mut self, text: &str) {
        if self.active_tab.is_none() {
            return;
        }
        // Two independent gates (owner call: each has its own setting):
        // careful paste parks multi-line text; the paste guard parks
        // suspicious CONTENT (bidi/invisible chars, raw control bytes,
        // curl|sh one-liners, homograph tokens) even on one line.
        if (self.setting_careful_paste
            && (text.contains('\n') || text.contains('\r')))
            || (self.setting_paste_guard
                && !crate::paste_guard::paste_warnings(text).is_empty())
        {
            self.pending_paste = Some(text.to_string());
            return;
        }
        self.write_paste_to_active(text);
    }

    /// Write `text` into the active tab's session, wrapping it for
    /// bracketed-paste when the focused app enabled it (`\e[?2004h`).
    /// Routes to the SSH session when one is attached, otherwise the
    /// local PTY. Shared by the clipboard (right-click / Ctrl+Shift+V)
    /// paste paths and the careful-paste confirmation.
    pub(crate) fn write_paste_to_active(&mut self, text: &str) {
        if let Some(tab_idx) = self.active_tab {
            let Some(tab) = self.tabs.get(tab_idx) else {
                return;
            };
            let bracketed = tab
                .active()
                .terminal
                .lock()
                .map(|s| s.bracketed_paste_enabled())
                .unwrap_or(false);
            let payload = oryxis_terminal::wrap_paste(text, bracketed);
            self.write_input_to_tab(tab_idx, &payload);
        }
    }

    /// Read the system clipboard and paste it into the active session.
    /// Shared by the Ctrl+V, Ctrl+Shift+V and Cmd+V (macOS) key paths so
    /// the bracketed-paste handling lives in exactly one place.
    pub(crate) fn paste_clipboard_into_active(&mut self) {
        if let Ok(mut clip) = arboard::Clipboard::new()
            && let Ok(text) = clip.get_text()
        {
            self.paste_text_into_active(&text);
        }
    }

    /// Dispatch a terminal message: `PtyOutput` and `KeyboardEvent`
    /// route straight to the `output` / `keyboard` submodule handlers,
    /// the remaining small arms match inline. Exhaustive on purpose: a
    /// new `TerminalMessage` variant fails to compile until it gets an
    /// arm, so it can never be silently dropped.
    pub(crate) fn handle_terminal(
        &mut self,
        message: TerminalMessage,
    ) -> Task<Message> {
        match message {
            TerminalMessage::PtyOutput(..) => {
                return self
                    .handle_terminal_output(message)
                    .unwrap_or_else(crate::dispatch::unrouted);
            }
            TerminalMessage::KeyboardEvent(..) => {
                return self
                    .handle_terminal_keyboard(message)
                    .unwrap_or_else(crate::dispatch::unrouted);
            }
            // -- Split panes --
            TerminalMessage::FocusPane(pane) => {
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.focused = pane;
                }
                // Clicking a terminal pane takes the keyboard back from the
                // sidebar ring (see write_input_to_tab for the rationale),
                // and drops the dropdown gate defensively: a click outside
                // an open pick_list normally fires on_close, but if the
                // widget unmounted first the stuck flag would swallow
                // Enter/Space/Esc/arrows forever.
                self.keynav.sidebar_selected = None;
                self.keynav.pick_open = false;
                // The History tab is per-host; follow the focused pane.
                if self.terminal_sidebar_tab == crate::state::TerminalSidebarTab::History {
                    self.refresh_command_history();
                }
                // The Files browser is per-pane; the newly focused pane may
                // need a mount or a cwd catch-up (no-op otherwise).
                return self.sidebar_files_sync();
            }
            TerminalMessage::ResizePane(ev) => {
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.pane_grid.resize(ev.split, ev.ratio);
                }
            }
            TerminalMessage::SplitPane(axis) => {
                // Open the connection picker to choose what fills the new
                // pane (a host, or a local shell). The selection routes into
                // a split via `pending_pane_split` instead of a new tab.
                self.overlay = None; // dismiss the `+` hover popover if open
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get(tab_idx)
                {
                    self.pending_pane_split = Some((tab_idx, tab.focused, axis));
                    self.show_new_tab_picker = true;
                    self.new_tab_picker_search.clear();
                    self.new_tab_picker_group = None;
                    // Same focus-the-search behavior as ShowNewTabPicker.
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        crate::state::NEW_TAB_PICKER_SEARCH_ID,
                    ));
                }
            }
            TerminalMessage::SplitTabPane(tab_idx, axis) => {
                // From a tab's right-click menu: focus that tab first, then
                // open the picker to fill the new split pane.
                self.overlay = None;
                if let Some(tab) = self.tabs.get(tab_idx) {
                    let target = tab.focused;
                    self.active_tab = Some(tab_idx);
                    self.active_view = crate::state::View::Terminal;
                    self.remember_terminal_tab_focus(tab_idx);
                    self.pending_pane_split = Some((tab_idx, target, axis));
                    self.show_new_tab_picker = true;
                    self.new_tab_picker_search.clear();
                    self.new_tab_picker_group = None;
                    // Same focus-the-search behavior as ShowNewTabPicker.
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        crate::state::NEW_TAB_PICKER_SEARCH_ID,
                    ));
                }
            }
            TerminalMessage::ClosePane(target_id) => {
                // Dismiss the terminal context menu when its "Close
                // pane" row fired this (no-op on the hotkey path).
                self.overlay = None;
                // Resolve the target at dispatch time. The context-menu
                // row carries the right-clicked pane's id: focus and the
                // active tab can change via hotkeys while the menu is
                // open (the overlay is not a modal), so "the focused
                // pane of the active tab" may no longer be the pane the
                // user clicked. A pane that is gone entirely (its tab
                // closed under the menu) is a safe no-op.
                let resolved = match target_id {
                    Some(pane_id) => {
                        self.pane_tab_index(pane_id).and_then(|tab_idx| {
                            self.tabs[tab_idx]
                                .pane_grid
                                .panes
                                .iter()
                                .find(|(_, p)| p.id == pane_id)
                                .map(|(handle, _)| (tab_idx, *handle))
                        })
                    }
                    // The hotkey path acts on the focused pane of the
                    // active tab, as before.
                    None => self
                        .active_tab
                        .filter(|&i| i < self.tabs.len())
                        .map(|i| (i, self.tabs[i].focused)),
                };
                let Some((tab_idx, target)) = resolved else {
                    return Task::none();
                };
                // Last pane in the tab: closing it closes the whole tab.
                // `tab_idx` is the pane's OWN tab by construction above,
                // so a stale focus can never close an unrelated tab.
                if self.tabs[tab_idx].pane_grid.panes.len() <= 1 {
                    return self.update(Message::Tabs(TabsMessage::CloseTab(tab_idx)));
                }
                // Persist the closing pane's recorded output before it goes.
                self.flush_session_logs_final();
                let tab = &mut self.tabs[tab_idx];
                // Tear down the pane's remote session (the connect stream
                // holds its own Arc; see close_tab_sessions) and collect
                // the end-of-session bookkeeping targets. This must be
                // synchronous: the `SshDisconnected` the close provokes
                // lands after the pane is gone and resolves nothing, so
                // deferring to it would leave the vault log row open
                // forever and the monitor primed to diff the next
                // session against the dead pane's counters.
                let mut ended_log = None;
                let mut closed_host = None;
                if let Some(pane) = tab.pane_grid.panes.get_mut(&target) {
                    if let Some(session) = pane.session.take() {
                        session.close();
                    }
                    ended_log = pane.session_log_id.take();
                    closed_host = match pane.origin {
                        crate::state::PaneOrigin::Host(id) => Some(id),
                        _ => None,
                    };
                }
                if let Some((_closed, sibling)) = tab.pane_grid.close(target) {
                    // Only a close of the focused pane moves focus;
                    // closing a background pane from its context menu
                    // must not yank the keyboard to its sibling.
                    if tab.focused == target {
                        tab.focused = sibling;
                    }
                }
                // Back to a single pane: disarm broadcast (its control
                // surfaces are hidden for unsplit tabs, so a lingering
                // armed state would be invisible) and drop the survivor's
                // opt-out so a later re-arm starts clean.
                if tab.pane_grid.panes.len() < 2 && tab.broadcast {
                    tab.broadcast = false;
                    for pane in tab.pane_grid.panes.values_mut() {
                        pane.broadcast_opt_out = false;
                    }
                }
                // A collapsed split has to re-anchor the tab on the pane
                // that is left: the unsplit label comes from the TAB, which
                // was named after the pane that just closed (issue #108).
                tab.sync_label_to_sole_pane();
                if let Some(log_id) = ended_log
                    && let Some(vault) = &self.vault
                {
                    let _ = vault.end_session_log(&log_id);
                }
                // Same rule as CloseTab: drop the monitor series only
                // when the closed pane was the host's last live one
                // anywhere (the closed pane is already out of the grid).
                if let Some(host) = closed_host {
                    let still_open = self.tabs.iter().any(|t| {
                        t.pane_grid.panes.values().any(|p| {
                            matches!(p.origin, crate::state::PaneOrigin::Host(id) if id == host)
                        })
                    });
                    if !still_open {
                        self.monitor_reset_host(&host);
                    }
                }
                // Drop quick-connect entries (and their in-memory
                // credentials) that no pane references anymore.
                self.prune_quick_connects();
            }
            TerminalMessage::FocusPaneDir(dir) => {
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                    && let Some(adj) = tab.pane_grid.adjacent(tab.focused, dir)
                {
                    tab.focused = adj;
                }
            }
            TerminalMessage::TerminalBellFlashEnd(pane_id) => {
                if let Some(pane) = self
                    .tabs
                    .iter_mut()
                    .flat_map(|t| t.pane_grid.panes.values_mut())
                    .find(|p| p.id == pane_id)
                {
                    pane.bell_flash = false;
                }
            }
            TerminalMessage::TerminalSyncFlush(pane_id) => {
                if let Some(pane) = self
                    .tabs
                    .iter_mut()
                    .flat_map(|t| t.pane_grid.panes.values_mut())
                    .find(|p| p.id == pane_id)
                {
                    pane.sync_flush_scheduled = false;
                    let mut reschedule: Option<std::time::Duration> = None;
                    if let Ok(mut state) = pane.terminal.lock() {
                        match state.sync_timeout() {
                            // The app extended the update past our deadline
                            // (a fresh BSU reset vte's 150 ms timer): re-arm
                            // for the new deadline instead of flushing
                            // mid-update, matching alacritty's behavior.
                            Some(deadline) if deadline > std::time::Instant::now() => {
                                reschedule = Some(deadline.saturating_duration_since(
                                    std::time::Instant::now(),
                                ));
                            }
                            // Deadline reached, update still open: force the
                            // buffered frame onto the grid.
                            Some(_) => state.flush_sync(),
                            // Closed normally in the meantime; nothing to do.
                            None => {}
                        }
                    }
                    if let Some(remaining) = reschedule {
                        pane.sync_flush_scheduled = true;
                        return Task::perform(
                            async move {
                                tokio::time::sleep(remaining).await;
                            },
                            move |_| Message::Terminal(TerminalMessage::TerminalSyncFlush(pane_id)),
                        );
                    }
                }
            }
            // ── Scrollback find-bar (C1) ──
            TerminalMessage::TerminalSearchOpen => {
                if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                {
                    let pane = tab.active_mut();
                    pane.search_open = true;
                    if let Ok(mut state) = pane.terminal.lock() {
                        state.search_open();
                        // Re-scan for the current needle so re-opening on the
                        // same query lands on live matches immediately.
                        if !pane.search_query.is_empty() {
                            state.search_set_query(&pane.search_query);
                        }
                    }
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "terminal-buffer-search",
                    ));
                }
            }
            TerminalMessage::TerminalSearchInput(v) => {
                if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                {
                    let pane = tab.active_mut();
                    pane.search_query = v;
                    if let Ok(mut state) = pane.terminal.lock() {
                        state.search_set_query(&pane.search_query);
                    }
                }
            }
            TerminalMessage::TerminalSearchStep(forward) => {
                if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                    && let Ok(mut state) = tab.active_mut().terminal.lock()
                {
                    state.search_step(forward);
                }
            }
            TerminalMessage::TerminalSearchClose => {
                if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                {
                    let pane = tab.active_mut();
                    pane.search_open = false;
                    if let Ok(mut state) = pane.terminal.lock() {
                        state.search_close();
                    }
                }
            }
            // ── Broadcast input (C2) ──
            TerminalMessage::ToggleTabBroadcast(idx) => {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    // Broadcast only exists across split panes: an unsplit
                    // tab refuses to arm and says why. The status segment
                    // and menu entry are hidden there, so this path is only
                    // reachable via the hotkey / command palette. Disarming
                    // stays unconditional so no state can ever get stuck.
                    if !tab.broadcast && tab.pane_grid.panes.len() < 2 {
                        self.set_toast(crate::i18n::t("broadcast_needs_split_hint").to_string());
                        return crate::shortcuts::toast_clear_after_secs(4);
                    }
                    tab.broadcast = !tab.broadcast;
                    if !tab.broadcast {
                        // Disarm: clear every opt-out so a later re-arm
                        // starts clean (all panes participate).
                        for pane in tab.pane_grid.panes.values_mut() {
                            pane.broadcast_opt_out = false;
                        }
                    }
                }
            }
            TerminalMessage::TogglePaneBroadcastOptOut(pane_id) => {
                if let Some(pane) = self
                    .tabs
                    .iter_mut()
                    .flat_map(|t| t.pane_grid.panes.values_mut())
                    .find(|p| p.id == pane_id)
                {
                    pane.broadcast_opt_out = !pane.broadcast_opt_out;
                }
            }
            // Periodic batched write of recorded output. Only mounted by
            // the subscription while at least one pane is recording.
            TerminalMessage::SessionLogFlushTick => {
                self.flush_session_logs();
            }
            // Right-click paste from the terminal widget. Mirrors the
            // Ctrl+Shift+V path below: SSH session if active, local PTY
            // otherwise. Without this, the widget's fallback write only
            // reached the local PTY and right-click looked broken on
            // every SSH tab.
            TerminalMessage::TerminalPasteFromClipboard => {
                // Also the terminal context-menu Paste row: dismiss the
                // menu (its item sits over the backdrop, so the backdrop
                // never sees the click). Idempotent for the other callers
                // (widget paste hook, middle-click, keyboard), which run
                // with no overlay open.
                self.overlay = None;
                if let Ok(mut clip) = arboard::Clipboard::new()
                    && let Ok(text) = clip.get_text()
                {
                    self.paste_text_into_active(&text);
                }
            }
            TerminalMessage::ShowTerminalContextMenu(pane_id, x, y, selection) => {
                // Focus the right-clicked pane first (standard context-menu
                // behavior), so all rows act on the same pane: Copy All /
                // Clear Scrollback are pane-targeted by id, and Paste
                // routes through the focused pane.
                if let Some(tab_idx) = self.pane_tab_index(pane_id) {
                    self.active_tab = Some(tab_idx);
                    if let Some(tab) = self.tabs.get_mut(tab_idx)
                        && let Some(gp) = tab
                            .pane_grid
                            .panes
                            .iter()
                            .find(|(_, p)| p.id == pane_id)
                            .map(|(gp, _)| *gp)
                    {
                        tab.focused = gp;
                    }
                }
                // Right-click scheme = Menu: anchor the overlay at the
                // click point (window-absolute, same space as every menu).
                self.overlay = Some(crate::state::OverlayState {
                    content: crate::state::OverlayContent::TerminalContextMenu(pane_id, selection),
                    x,
                    y,
                });
            }
            TerminalMessage::TerminalCopySelection(text) => {
                self.overlay = None;
                if !text.is_empty()
                    && let Ok(mut clip) = arboard::Clipboard::new()
                {
                    let _ = clip.set_text(text);
                }
            }
            TerminalMessage::TerminalCopyAll(pane_id) => {
                self.overlay = None;
                if let Some(pane) = self.pane_by_id(pane_id)
                    && let Ok(state) = pane.terminal.lock()
                {
                    let text = state.all_text();
                    drop(state);
                    if !text.is_empty()
                        && let Ok(mut clip) = arboard::Clipboard::new()
                    {
                        let _ = clip.set_text(text);
                    }
                }
            }
            TerminalMessage::TerminalClearScrollback(pane_id) => {
                self.overlay = None;
                if let Some(pane) = self.pane_by_id(pane_id)
                    && let Ok(mut state) = pane.terminal.lock()
                {
                    state.clear_scrollback();
                }
            }
            // Careful-paste confirmation: release the parked multi-line
            // text into the session, or drop it.
            TerminalMessage::ConfirmPendingPaste => {
                if let Some(text) = self.pending_paste.take() {
                    self.write_paste_to_active(&text);
                }
            }
            TerminalMessage::CancelPendingPaste => {
                self.pending_paste = None;
            }
            // Synthesized input from the terminal widget: mouse-tracking
            // reports (tmux `mouse on`, vim `mouse=a`, htop, ...) and the
            // wheel-to-arrow translation in alt-screen. Same SSH-or-local
            // routing as keystrokes; without this the widget's local-PTY
            // fallback would never reach the remote session.
            TerminalMessage::TerminalInput(bytes) => {
                if let Some(tab_idx) = self.active_tab {
                    self.write_input_to_tab(tab_idx, &bytes);
                }
            }
            TerminalMessage::TerminalMouseCaptureHint => {
                // Mark the focused pane so HintMode::Once retires the hint
                // (harmless under Always, where the view ignores the flag).
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.active_mut().mouse_hint_shown = true;
                }
                // Longer dwell than the default toast: this one is a sentence
                // to read, not a one-word "Copied" confirmation.
                return self.show_toast_secs(crate::i18n::t("mouse_capture_hint").to_string(), 5);
            }
            TerminalMessage::TerminalLinkClickHint => {
                // Plain click on a link without Ctrl: teach the gesture with
                // a toast at the moment it missed (replaces the old hover
                // tooltip). Mark the focused pane so HintMode::Once retires
                // it (harmless under Always, where the view ignores the flag).
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.active_mut().link_hint_shown = true;
                }
                // Same longer dwell as the mouse-capture hint: a sentence.
                return self.show_toast_secs(crate::i18n::t("terminal_link_hint").to_string(), 5);
            }
            // Text committed by the OS IME (composed CJK characters, etc.),
            // delivered by the global subscription separately from
            // KeyboardEvent. Forward to the PTY under the same conditions as
            // a keystroke: no host editor panel or modal stealing focus, and
            // the cursor not over the chat sidebar. Deliberately does NOT
            // gate on active_view: in workspace mode a focused terminal runs
            // under the Dashboard view, not a dedicated Terminal view, so the
            // KeyboardEvent path doesn't check it either. When a text_input is
            // focused it handles its own Commit and inserts the text itself;
            // the host-panel / modal guards keep that from also hitting the
            // session.
            TerminalMessage::TerminalImeCommit(text) => {
                if text.is_empty() || self.show_host_panel || self.any_modal_blocks_input() {
                    return Task::none();
                }
                // `cursor_over_sidebar` honors the dock side (issue #85)
                // and the side tab strip (issue #87); the old inline
                // right-edge math leaked IME commits into the PTY when
                // the sidebar was docked left.
                if self.cursor_over_sidebar() {
                    return Task::none();
                }
                if let Some(tab_idx) = self.active_tab
                    && self.connecting.is_none()
                {
                    let bytes = text.into_bytes();
                    self.write_input_to_tab(tab_idx, &bytes);
                }
            }
        }
        Task::none()
    }

    /// Index of the tab whose grid contains the pane with `pane_id`.
    /// Used to route per-pane session events (connect / disconnect).
    pub(crate) fn pane_tab_index(&self, pane_id: uuid::Uuid) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.pane_grid.panes.values().any(|p| p.id == pane_id))
    }

    /// Find a pane by its stable id across every tab (shared read).
    pub(crate) fn pane_by_id(&self, pane_id: uuid::Uuid) -> Option<&crate::state::Pane> {
        self.tabs
            .iter()
            .flat_map(|t| t.pane_grid.panes.values())
            .find(|p| p.id == pane_id)
    }

    /// Find a pane by its stable id across every tab (mutable).
    pub(crate) fn pane_by_id_mut(&mut self, pane_id: uuid::Uuid) -> Option<&mut crate::state::Pane> {
        self.tabs
            .iter_mut()
            .flat_map(|t| t.pane_grid.panes.values_mut())
            .find(|p| p.id == pane_id)
    }
}
