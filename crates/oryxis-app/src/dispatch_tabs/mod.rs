//! `Oryxis::handle_tabs`, match arms for the tab strip + tab modals
//! (new-tab picker, tab-jump, icon picker), card hover/menu, folder
//! actions, window chrome (drag/resize/min/max/close).

#![allow(clippy::result_large_err)]

mod hybrid;
mod icon_picker;
mod lifecycle;
mod ordering;
mod window;

use iced::Task;

use crate::app::{SettingsMessage, TabsMessage, TerminalMessage, SshMessage, CloudMessage, NavigationMessage, Message, Oryxis};
use crate::state::{OverlayContent, OverlayState, View};

/// Smallest gap between two `WindowDrag` / `WindowResizeDrag`
/// presses we'll accept. iced's `MouseArea` re-fires `on_press` on
/// the second click of a double-click before `on_double_click` lands;
/// honouring that second drag races our `toggle_maximize` /
/// `WindowExpand*` follow-up. `300ms` is wider than any realistic
/// double-click and short enough that a deliberate two-quick-clicks-
/// to-drag still feels responsive.
const WINDOW_PRESS_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

impl Oryxis {
    /// Returns `true` when this press should be forwarded to the OS.
    /// Returns `false` when the previous press was within
    /// [`WINDOW_PRESS_DEBOUNCE`], swallowing the spurious second
    /// `on_press` that a double-click emits.
    pub(crate) fn consume_window_press(&mut self) -> bool {
        let now = std::time::Instant::now();
        let allow = self
            .last_window_press_at
            .is_none_or(|prev| now.duration_since(prev) >= WINDOW_PRESS_DEBOUNCE);
        if allow {
            self.last_window_press_at = Some(now);
        }
        allow
    }

    pub(crate) fn handle_tabs(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // -- Card interactions --
            Message::Tabs(TabsMessage::CardHovered(idx)) => {
                self.hovered_card = Some(idx);
            }
            Message::Tabs(TabsMessage::CardUnhovered) => {
                self.hovered_card = None;
            }
            Message::Tabs(TabsMessage::FolderCardHovered(gid)) => {
                self.hovered_folder_card = Some(gid);
            }
            Message::Tabs(TabsMessage::FolderCardUnhovered) => {
                self.hovered_folder_card = None;
            }
            Message::Tabs(TabsMessage::KeyCardHovered(idx)) => {
                self.hovered_key_card = Some(idx);
            }
            Message::Tabs(TabsMessage::KeyCardUnhovered) => {
                self.hovered_key_card = None;
            }
            Message::Tabs(TabsMessage::IdentityCardHovered(idx)) => {
                self.hovered_identity_card = Some(idx);
            }
            Message::Tabs(TabsMessage::SnippetCardHovered(idx)) => {
                self.hovered_snippet_card = Some(idx);
            }
            Message::Tabs(TabsMessage::SnippetCardUnhovered) => {
                self.hovered_snippet_card = None;
            }
            Message::Tabs(TabsMessage::IdentityCardUnhovered) => {
                self.hovered_identity_card = None;
            }
            Message::Tabs(TabsMessage::MouseMoved(pos)) => return self.handle_mouse_moved(pos),
            Message::Tabs(TabsMessage::WindowResized(size)) => return self.handle_window_resized(size),
            Message::Tabs(TabsMessage::WindowMoved(pos)) => {
                // Same skip rule as the windowed-size tracking above:
                // maximize / fullscreen park the window at the monitor
                // origin, and the optimistic flags flip before that
                // Moved event arrives. The second filter drops the
                // (-32000, -32000) sentinel Windows reports for
                // minimized windows (scaled by DPI when converted to
                // logical, hence the generous threshold: no real
                // monitor layout puts a window beyond -8000 on both
                // axes at once).
                let minimized_sentinel = pos.x <= -8000.0 && pos.y <= -8000.0;
                if !self.window_maximized
                    && !self.window_fullscreen
                    && !minimized_sentinel
                {
                    self.window_windowed_pos = Some(pos);
                }
            }
            Message::Tabs(TabsMessage::WindowEnsureOnScreen) => return self.handle_window_ensure_on_screen(),
            Message::Tabs(TabsMessage::WindowFocusChanged(focused)) => return self.handle_window_focus_changed(focused),
            Message::Tabs(TabsMessage::SsmKeepaliveTick) => {
                // Toggle each SSM/ECS terminal between `base` and
                // `base - 1` rows. Every tick is therefore a genuine size
                // change, which fires a SIGWINCH the plugin forwards to
                // SSM as a resize event, and resize events reset the
                // server's idle timer. No base means we're focused (the
                // ticker shouldn't be mounted then), so it's a no-op.
                if let Some((base_cols, base_rows)) = self.ssm_keepalive_base {
                    let shrunk = base_rows.saturating_sub(1).max(2);
                    for tab in self.tabs.iter().filter(|t| t.ssm_keepalive) {
                        for pane in tab.pane_grid.panes.values() {
                            if let Ok(mut state) = pane.terminal.lock() {
                                let target = if state.rows() == base_rows {
                                    shrunk
                                } else {
                                    base_rows
                                };
                                state.resize(base_cols, target);
                            }
                        }
                    }
                }
            }
            Message::Tabs(TabsMessage::WindowDrag) => {
                if !self.consume_window_press() {
                    return Ok(Task::none());
                }
                return Ok(iced::window::latest().then(|id_opt| match id_opt {
                    Some(id) => iced::window::drag(id),
                    None => Task::none(),
                }));
            }
            Message::Tabs(TabsMessage::WindowResizeDrag(direction)) => {
                // Ignore resize requests while maximized, the window has no
                // borders to grab and the OS will reject/misbehave on WinIt.
                if self.window_maximized {
                    return Ok(Task::none());
                }
                if !self.consume_window_press() {
                    return Ok(Task::none());
                }
                return Ok(iced::window::latest().then(move |id_opt| match id_opt {
                    Some(id) => iced::window::drag_resize(id, direction),
                    None => Task::none(),
                }));
            }
            Message::Tabs(TabsMessage::WindowExpandVertical) => return self.handle_window_expand_vertical(),
            Message::Tabs(TabsMessage::WindowMinimize) => return self.handle_window_minimize(),
            Message::Tabs(TabsMessage::WindowMaximizeToggle) => {
                self.window_maximized = !self.window_maximized;
                // Cheap write, and it keeps the restored state accurate
                // even when the process later dies without reaching an
                // exit path (OS shutdown, kill).
                self.persist_window_geometry();
                return Ok(iced::window::latest().then(|id_opt| match id_opt {
                    Some(id) => iced::window::toggle_maximize(id),
                    None => Task::none(),
                }));
            }
            Message::Tabs(TabsMessage::WindowClose) => return self.handle_window_close(),
            Message::Tabs(TabsMessage::WindowFullscreenToggle) => return self.handle_window_fullscreen_toggle(),
            Message::Tabs(TabsMessage::FullscreenHintHide) => {
                self.fullscreen_hint_visible = false;
            }
            Message::Tabs(TabsMessage::SpawnNewWindow) => {
                // Burger menu fires this. Drop both the context-menu
                // overlay AND the burger panel itself so the menu
                // doesn't linger on top of the freshly-spawned window.
                // The burger lives in its own `show_burger_menu` flag
                // (not `OverlayState`), so clearing `self.overlay`
                // alone wasn't enough.
                self.overlay = None;
                self.show_burger_menu = false;
                self.spawn_oryxis_child(None);
            }
            Message::Tabs(TabsMessage::ActivateStripSlot(slot)) => {
                if let Some(msg) = self.strip_slot_target(slot) {
                    return Ok(Task::done(msg));
                }
            }
            Message::Tabs(TabsMessage::FocusViewSearch) => {
                // Ctrl+F always returns keynav to the canonical idle
                // state (search = "zone zero", `focus == None`).
                self.keynav.focus = None;
                // Over a focused terminal pane, Ctrl+F opens the scrollback
                // find-bar (C1). `active_tab` is cleared on navigation into
                // the vault / settings / SFTP surfaces, so its presence means
                // the terminal surface is the one on screen; the hybrid Files
                // (SFTP) mode is excluded, since it has its own remote filter,
                // reached through `active_view_search_id` below.
                if let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i))
                    && !tab.files_mode
                {
                    return Ok(self.update(Message::Terminal(TerminalMessage::TerminalSearchOpen)));
                }
                if let Some(id) = self.active_view_search_id() {
                    return Ok(iced::widget::operation::focus(id));
                }
            }
            Message::Tabs(TabsMessage::HideOverlayMenu) => {
                self.overlay = None;
                self.card_context_menu = None;
                self.snippet_context_menu = None;
                self.key_context_menu = None;
                self.identity_context_menu = None;
                self.show_keychain_add_menu = false;
            }
            Message::Tabs(TabsMessage::ShowCardMenu(idx)) => {
                if self.card_context_menu == Some(idx) {
                    self.card_context_menu = None;
                    self.overlay = None;
                } else {
                    self.card_context_menu = Some(idx);
                    let anchor = self.keynav_take_menu_anchor();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::HostActions(idx),
                        x: anchor.0,
                        y: anchor.1,
                    });
                }
            }
            Message::Tabs(TabsMessage::HideCardMenu) => {
                self.card_context_menu = None;
                self.overlay = None;
            }

            // -- Tabs --
            Message::Tabs(TabsMessage::SelectTab(idx)) => return self.handle_select_tab(idx),
            Message::Tabs(TabsMessage::ToggleTabFilesMode(idx)) => return self.handle_toggle_tab_files_mode(idx),
            Message::Tabs(TabsMessage::DetachTabSftp(idx)) => return self.handle_detach_tab_sftp(idx),
            Message::Tabs(TabsMessage::CloseTabSftpSession(idx)) => return self.handle_close_tab_sftp_session(idx),
            Message::Tabs(TabsMessage::OpenTerminalForSftpTab(idx)) => return self.handle_open_terminal_for_sftp_tab(idx),
            Message::Tabs(TabsMessage::TabHovered(idx)) => {
                self.hovered_tab = Some(idx);
                // Terminal / SFTP hover are mutually exclusive (one cursor).
                self.hovered_sftp_tab = None;
                // Live-slide: while a drag is active, entering another tab in
                // the same group slides the dragged tab into that slot right
                // away. Stable because after the move the dragged tab sits
                // under the cursor, so it won't re-trigger until the cursor
                // crosses into a genuinely different tab.
                if let Some(drag) = self.tab_drag.filter(|d| d.active)
                    && let Some(target) = self.tabs.get(idx).map(|t| t._id)
                    && drag.from_id != target
                {
                    // Reorders `tab_order` (display) only; storage vecs and the
                    // active pointers are untouched. Same-partition guard is in
                    // `slide_tab_in_order`.
                    self.slide_tab_in_order(drag.from_id, target);
                }
            }
            Message::Tabs(TabsMessage::TabUnhovered) => {
                self.hovered_tab = None;
            }
            Message::Tabs(TabsMessage::TabDragToEnd) => {
                // Trailing drop zone: the live-slide only ever moves the
                // dragged tab to *before* a hovered tab, so the slot after the
                // last tab is unreachable by hovering. Entering the `+` area
                // during an active drag fills that gap.
                if let Some(drag) = self.tab_drag.filter(|d| d.active) {
                    self.slide_tab_to_partition_end(drag.from_id);
                }
            }
            Message::Tabs(TabsMessage::ShowNewTabPicker) => {
                // Opening the picker from the `+` button always targets a new
                // tab, never a split (only SplitPane sets that).
                self.overlay = None; // dismiss the `+` hover popover if open
                self.pending_pane_split = None;
                self.show_new_tab_picker = true;
                self.new_tab_picker_search.clear();
                self.new_tab_picker_group = None;
                // Land focus on the search so the picker is
                // type-to-filter from the first keystroke.
                return Ok(iced::widget::operation::focus(iced::widget::Id::new(
                    crate::state::NEW_TAB_PICKER_SEARCH_ID,
                )));
            }
            Message::Tabs(TabsMessage::HideNewTabPicker) => {
                self.show_new_tab_picker = false;
                self.pending_pane_split = None;
                self.new_tab_picker_group = None;
            }
            Message::Tabs(TabsMessage::NewTabPickerOpenGroup(gid)) => {
                // Drill into the group; the search box now filters this
                // group's members instead of the top-level list, so clear
                // the leftover top-level needle.
                self.new_tab_picker_group = Some(gid);
                self.new_tab_picker_search.clear();
                // Cloud-query group: kick off (or refresh) the resolve so
                // the ECS tasks / K8s pods load. Reuses the same TTL gate
                // as the dashboard's OpenGroup so we don't hammer the API.
                if self.dynamic_group_needs_resolve(gid) {
                    return Ok(self
                        .handle_cloud(Message::Cloud(CloudMessage::DynamicGroupResolve(gid)))
                        .unwrap_or_else(|_| Task::none()));
                }
            }
            Message::Tabs(TabsMessage::NewTabPickerBack) => {
                self.new_tab_picker_group = None;
                self.new_tab_picker_search.clear();
            }
            Message::Tabs(TabsMessage::PickLocalShell) => {
                self.show_new_tab_picker = false;
                if let Some((tab_idx, target, axis)) = self.pending_pane_split.take() {
                    return Ok(self.local_shell_into_pane(tab_idx, target, axis));
                }
                // No split pending: open a local shell in a new tab.
                return Ok(self.update(Message::Settings(SettingsMessage::OpenLocalShell)));
            }
            Message::Tabs(TabsMessage::ShowTabJump) => {
                self.show_tab_jump = true;
                self.tab_jump_search.clear();
            }
            Message::Tabs(TabsMessage::ToggleBurgerMenu) => {
                self.show_burger_menu = !self.show_burger_menu;
            }
            Message::Tabs(TabsMessage::ToggleSubnavOverflow) => {
                self.show_subnav_overflow = !self.show_subnav_overflow;
            }
            Message::Tabs(TabsMessage::HideTabJump) => {
                self.show_tab_jump = false;
            }
            Message::Tabs(TabsMessage::TabJumpSearchChanged(v)) => {
                self.tab_jump_search = v;
            }
            Message::Tabs(TabsMessage::TabBarWheel(dy)) => {
                // Vertical wheel over the tab bar scrolls horizontally
                // iced's horizontal-only scrollable ignores y deltas, so
                // we translate them via scroll_by here. Sign flip so
                // wheel-down brings later tabs into view (matches the
                // direction Chrome/VS Code use).
                return Ok(iced::widget::operation::scroll_by(
                    iced::widget::Id::new("tab-scroll"),
                    iced::widget::scrollable::AbsoluteOffset { x: -dy, y: 0.0 },
                ));
            }
            Message::Tabs(TabsMessage::TabJumpSelect(inner)) => {
                self.show_tab_jump = false;
                return Ok(Task::done(*inner));
            }
            Message::Tabs(TabsMessage::ShowCommandPalette) => {
                // The palette assumes an unlocked vault (its actions do).
                // The hotkey path already gates on this; guard here too so
                // no other producer can open it over the lock screen.
                if self.vault_ui.state != crate::state::VaultState::Unlocked {
                    return Ok(Task::none());
                }
                self.palette.open = true;
                self.palette.query.clear();
                // Focus the query input so the user types immediately.
                return Ok(iced::widget::operation::focus(
                    iced::widget::Id::new(crate::palette::PALETTE_INPUT_ID),
                ));
            }
            Message::Tabs(TabsMessage::HideCommandPalette) => {
                self.palette.open = false;
                self.palette.query.clear();
            }
            Message::Tabs(TabsMessage::PaletteQueryChanged(v)) => {
                self.palette.query = v;
            }
            Message::Tabs(TabsMessage::PaletteActivate(inner)) => {
                // Two-step dispatch like TabJumpSelect: close first, then
                // fire the row's real message (it may open another modal).
                self.palette.open = false;
                self.palette.query.clear();
                return Ok(Task::done(*inner));
            }
            Message::Tabs(TabsMessage::RunHotkeyAction(action)) => {
                return Ok(self.dispatch_hotkey_action(
                    action,
                    crate::hotkeys::FamilyMatch::Plain,
                ));
            }
            Message::Tabs(TabsMessage::OpenSettingsSection(section)) => {
                // Switch to Settings AND select the section:
                // ChangeSettingsSection alone assumes the view is open.
                let t1 = self.update(Message::Navigation(NavigationMessage::ChangeView(View::Settings)));
                let t2 = self.update(Message::Settings(SettingsMessage::ChangeSettingsSection(section)));
                return Ok(Task::batch([t1, t2]));
            }

            Message::Tabs(TabsMessage::NewTabPickerSearchChanged(v)) => {
                self.new_tab_picker_search = v;
            }
            Message::Tabs(TabsMessage::NewTabPickerSubmit) => {
                // Enter in the picker. Owned by the search input's
                // on_submit (the modal key router declines Enter here
                // so the two paths can never double-fire). Priority:
                // the explicit keyboard selection, then the ad-hoc
                // quick-connect target, then the top row of the
                // filtered list.
                if let Some((surface, _)) = self.modal_nav_surface()
                    && let Some(idx) = self.modal_nav_effective(surface)
                {
                    let action = self.keynav.modal.items.borrow().get(idx).cloned();
                    if let Some(msg) = action.and_then(|a| a.activate) {
                        return Ok(self.update(msg));
                    }
                }
                if let Some(conn) = self.quick_connect_target(&self.new_tab_picker_search)
                {
                    return Ok(self.update(Message::Ssh(SshMessage::QuickConnect(Box::new(
                        crate::state::QuickConnectEntry::bare(conn),
                    )))));
                }
                let top = self.keynav.modal.items.borrow().first().cloned();
                if let Some(msg) = top.and_then(|a| a.activate) {
                    return Ok(self.update(msg));
                }
            }
            Message::Tabs(TabsMessage::ShowIconPicker(conn_id)) => {
                // Pre-fill the picker with the icon the user is
                // currently seeing on the host card: custom override
                // first, then auto-detected OS, then the generic
                // "server" fallback as last resort. Using just
                // `custom_icon || "server"` here was buggy: hosts
                // whose icon comes from `detected_os` (Ubuntu, etc.)
                // showed "server" highlighted in the picker, so a
                // user clicking Save (even just to change the color)
                // accidentally overrode the auto-detected icon with
                // the generic stack glyph.
                if let Some(conn) = self.connections.iter().find(|c| c.id == conn_id) {
                    self.icon_picker.icon = conn
                        .custom_icon
                        .clone()
                        .or_else(|| conn.detected_os.clone())
                        .or_else(|| Some("server".to_string()));
                    self.icon_picker.color = conn.custom_color.clone();
                    self.icon_picker.hex_input = conn.custom_color.clone().unwrap_or_default();
                }
                self.icon_picker.icon_search.clear();
                self.icon_color_popover = None;
                self.icon_picker.for_id = Some(conn_id);
                self.icon_picker.for_local_terminal = false;
                self.show_icon_picker = true;
            }
            Message::Tabs(TabsMessage::HideIconPicker) => {
                self.show_icon_picker = false;
                self.icon_picker.for_id = None;
                self.icon_picker.for_group_form = false;
                self.icon_picker.for_session_group = false;
                self.icon_picker.for_group_edit = false;
                self.icon_picker.for_local_terminal = false;
                self.icon_picker.icon_search.clear();
                self.icon_color_popover = None;
            }
            Message::Tabs(TabsMessage::IconPickerSelectIcon(name)) => {
                self.icon_picker.icon = Some(name);
            }
            Message::Tabs(TabsMessage::IconPickerIconSearchChanged(q)) => {
                self.icon_picker.icon_search = q;
            }
            Message::Tabs(TabsMessage::IconPickerOpenColorPopover) => {
                self.icon_color_popover = Some(self.mouse_position);
            }
            Message::Tabs(TabsMessage::IconPickerCloseColorPopover) => {
                self.icon_color_popover = None;
            }
            Message::Tabs(TabsMessage::IconPickerSelectColor(hex)) => {
                self.icon_picker.hex_input = hex.clone();
                self.icon_picker.color = Some(hex);
            }
            Message::Tabs(TabsMessage::IconPickerHexInputChanged(v)) => {
                self.icon_picker.hex_input = v.clone();
                // Validate + commit only on well-formed #RRGGBB.
                let trimmed = v.trim().trim_start_matches('#');
                if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                    self.icon_picker.color = Some(format!("#{}", trimmed.to_uppercase()));
                }
            }
            Message::Tabs(TabsMessage::IconPickerSave) => return self.handle_icon_picker_save(),
            Message::Tabs(TabsMessage::IconPickerResetAuto) => return self.handle_icon_picker_reset_auto(),
            Message::Tabs(TabsMessage::CloseTab(idx)) => return self.handle_close_tab(idx),
            Message::Tabs(TabsMessage::ShowTabMenu(idx)) => {
                let anchor = self.keynav_take_menu_anchor();
                self.overlay = Some(OverlayState {
                    content: OverlayContent::TabActions(idx),
                    x: anchor.0,
                    y: anchor.1,
                });
            }
            Message::Tabs(TabsMessage::ShowSplitMenu) => {
                // Hover popover under `+`. Only meaningful with a terminal
                // tab open (something to split); otherwise `+` just opens a
                // new tab on click. Anchored under the cursor (over `+`).
                if self.active_view == View::Terminal
                    && self.active_tab.is_some()
                    && !matches!(
                        self.overlay.as_ref().map(|o| &o.content),
                        Some(OverlayContent::SplitMenu)
                    )
                {
                    // Anchor under the `+` button at a fixed position (its
                    // reported bounds), not the cursor, so the popover lines
                    // up cleanly with the button.
                    let b = self.plus_btn_bounds.get();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::SplitMenu,
                        x: b.x,
                        y: b.y + b.height,
                    });
                }
            }
            Message::Tabs(TabsMessage::SplitMenuEnter) => {
                self.split_menu_hovered = true;
            }
            Message::Tabs(TabsMessage::SplitMenuLeave) => {
                // Left the `+` button or the popover. Defer the close briefly
                // so moving from the button INTO the menu (which re-enters
                // via `SplitMenuEnter`) doesn't flap it shut.
                self.split_menu_hovered = false;
                return Ok(Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
                    },
                    |_| Message::Tabs(TabsMessage::SplitMenuCloseIfIdle),
                ));
            }
            Message::Tabs(TabsMessage::SplitMenuCloseIfIdle) => {
                if !self.split_menu_hovered
                    && matches!(
                        self.overlay.as_ref().map(|o| &o.content),
                        Some(OverlayContent::SplitMenu)
                    )
                {
                    self.overlay = None;
                }
            }
            Message::Tabs(TabsMessage::ToggleTabPin(idx)) => {
                self.overlay = None;
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.pinned = !tab.pinned;
                }
                self.persist_pinned_tabs();
            }
            Message::Tabs(TabsMessage::ReconnectTab(idx)) => return self.handle_reconnect_tab(idx),
            Message::Tabs(TabsMessage::DuplicateTab(idx)) => return self.handle_duplicate_tab(idx),
            Message::Tabs(TabsMessage::DuplicateInNewWindow(idx)) => {
                self.overlay = None;
                self.spawn_oryxis_child(Some(idx));
            }
            Message::Tabs(TabsMessage::ShowFolderActions(gid)) => {
                // Anchor the menu to the cursor, matches the host-card
                // "..." pattern. The global MouseMoved subscription keeps
                // `mouse_position` fresh.
                let anchor = self.keynav_take_menu_anchor();
                self.overlay = Some(OverlayState {
                    content: OverlayContent::FolderActions(gid),
                    x: anchor.0,
                    y: anchor.1,
                });
            }
            Message::Tabs(TabsMessage::StartRenameFolder(gid)) => {
                self.overlay = None;
                let current = self
                    .groups
                    .iter()
                    .find(|g| g.id == gid)
                    .map(|g| g.label.clone())
                    .unwrap_or_default();
                self.folder_rename = Some((gid, current));
            }
            Message::Tabs(TabsMessage::FolderRenameInput(val)) => {
                if let Some((_, ref mut buf)) = self.folder_rename {
                    *buf = val;
                }
            }
            Message::Tabs(TabsMessage::ConfirmRenameFolder) => {
                if let Some((gid, new_label)) = self.folder_rename.take() {
                    let trimmed = new_label.trim();
                    if !trimmed.is_empty()
                        && let Some(group) = self.groups.iter_mut().find(|g| g.id == gid)
                    {
                        group.label = trimmed.to_string();
                        group.updated_at = chrono::Utc::now();
                        if let Some(vault) = &self.vault {
                            let _ = vault.save_group(group);
                        }
                    }
                }
            }
            Message::Tabs(TabsMessage::CancelFolderModal) => {
                self.folder_rename = None;
                self.close_modal(crate::state::Modal::FolderDelete);
            }
            // -- Tab rename (transient custom name) --
            Message::Tabs(TabsMessage::StartRenameTab(idx)) => {
                self.overlay = None;
                if let Some(tab) = self.tabs.get(idx) {
                    // Prefill with what the strip currently shows (custom
                    // name, group name or OSC title), minus the state
                    // suffix, so "rename" starts from the visible truth.
                    let auto = self.tab_auto_title(tab);
                    let current = tab
                        .display_label(auto)
                        .trim_end_matches(" (disconnected)")
                        .to_string();
                    self.tab_rename =
                        Some((crate::state::TabRef::Terminal(tab._id), current));
                    // Drop the keyboard straight into the input, mirroring
                    // the SFTP inline rename.
                    return Ok(iced::widget::operation::focus(iced::widget::Id::new(
                        crate::views::layout::TAB_RENAME_INPUT_ID,
                    )));
                }
            }
            Message::Tabs(TabsMessage::StartRenameSftpTab(idx)) => {
                self.overlay = None;
                if let Some(tab) = self.sftp_tabs.get(idx) {
                    let current = tab.display_label().to_string();
                    self.tab_rename = Some((crate::state::TabRef::Sftp(tab.id), current));
                    return Ok(iced::widget::operation::focus(iced::widget::Id::new(
                        crate::views::layout::TAB_RENAME_INPUT_ID,
                    )));
                }
            }
            Message::Tabs(TabsMessage::TabRenameInput(val)) => {
                if let Some((_, ref mut buf)) = self.tab_rename {
                    *buf = val;
                }
            }
            Message::Tabs(TabsMessage::ConfirmTabRename) => {
                if let Some((tab_ref, name)) = self.tab_rename.take() {
                    let trimmed = name.trim();
                    // Empty clears the custom name: the automatic label
                    // (host / group / OSC title) takes over again.
                    let new_name =
                        (!trimmed.is_empty()).then(|| trimmed.to_string());
                    match tab_ref {
                        crate::state::TabRef::Terminal(id) => {
                            if let Some(tab) =
                                self.tabs.iter_mut().find(|t| t._id == id)
                            {
                                tab.custom_name = new_name;
                            }
                        }
                        crate::state::TabRef::Sftp(id) => {
                            if let Some(tab) =
                                self.sftp_tabs.iter_mut().find(|t| t.id == id)
                            {
                                tab.custom_name = new_name;
                            }
                        }
                    }
                }
            }
            Message::Tabs(TabsMessage::CancelTabRename) => {
                self.tab_rename = None;
            }
            Message::Tabs(TabsMessage::EditGroup(gid)) => {
                self.overlay = None;
                if let Some(group) = self.groups.iter().find(|g| g.id == gid) {
                    self.group_edit.id = Some(gid);
                    self.group_edit.label = group.label.clone();
                    self.group_edit.icon = group.icon.clone().unwrap_or_default();
                    self.group_edit.color = group.color.clone().unwrap_or_default();
                    self.group_edit.visible = true;
                    // Mutually exclusive with the other right-hand panels.
                    self.show_host_panel = false;
                    self.panel_nav_clear();
                    self.show_session_group_panel = false;
                    self.cloud_form.visible = false;
                    self.cloud_dynamic_form.visible = false;
                    self.cloud_discover_visible = false;
                }
            }
            Message::Tabs(TabsMessage::GroupEditLabelChanged(v)) => {
                self.group_edit.label = v;
            }
            Message::Tabs(TabsMessage::ShowGroupEditIconPicker) => {
                self.icon_picker.icon = if self.group_edit.icon.is_empty() {
                    None
                } else {
                    Some(self.group_edit.icon.clone())
                };
                self.icon_picker.color = if self.group_edit.color.is_empty() {
                    None
                } else {
                    Some(self.group_edit.color.clone())
                };
                self.icon_picker.hex_input = self.group_edit.color.clone();
                self.icon_picker.for_id = None;
                self.icon_picker.for_group_form = false;
                self.icon_picker.for_session_group = false;
                self.icon_picker.for_group_edit = true;
                self.icon_picker.for_local_terminal = false;
                self.show_icon_picker = true;
            }
            Message::Tabs(TabsMessage::SaveGroupEdit) => {
                if let Some(gid) = self.group_edit.id {
                    let trimmed = self.group_edit.label.trim().to_string();
                    if !trimmed.is_empty()
                        && let Some(group) = self.groups.iter_mut().find(|g| g.id == gid)
                    {
                        group.label = trimmed;
                        group.icon = if self.group_edit.icon.is_empty() {
                            None
                        } else {
                            Some(self.group_edit.icon.clone())
                        };
                        group.color = if self.group_edit.color.is_empty() {
                            None
                        } else {
                            Some(self.group_edit.color.clone())
                        };
                        group.updated_at = chrono::Utc::now();
                        if let Some(vault) = &self.vault {
                            let _ = vault.save_group(group);
                        }
                    }
                }
                self.group_edit.visible = false;
                self.group_edit.id = None;
            }
            Message::Tabs(TabsMessage::CancelGroupEdit) => {
                self.group_edit.visible = false;
                self.group_edit.id = None;
            }
            Message::Tabs(TabsMessage::StartDeleteFolder(gid)) => {
                self.overlay = None;
                self.folder_delete = Some(gid);
            }
            Message::Tabs(TabsMessage::DeleteFolderKeepHosts) => {
                if let Some(gid) = self.folder_delete {
                    // Move every host inside the folder to the root.
                    for conn in self.connections.iter_mut() {
                        if conn.group_id == Some(gid) {
                            conn.group_id = None;
                            conn.updated_at = chrono::Utc::now();
                            if let Some(vault) = &self.vault {
                                let _ = vault.save_connection(conn, None);
                            }
                        }
                    }
                    // Re-home nested sub-groups (e.g. ECS / K8s dynamic
                    // groups) to root too, so they don't dangle off the
                    // deleted parent and vanish from every view.
                    for g in self.groups.iter_mut() {
                        if g.parent_id == Some(gid) {
                            g.parent_id = None;
                            g.updated_at = chrono::Utc::now();
                            if let Some(vault) = &self.vault {
                                let _ = vault.save_group(g);
                            }
                        }
                    }
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_group(&gid);
                    }
                    self.groups.retain(|g| g.id != gid);
                    if self.active_group == Some(gid) {
                        self.active_group = None;
                    }
                    self.close_modal(crate::state::Modal::FolderDelete);
                }
            }
            Message::Tabs(TabsMessage::DeleteFolderWithHosts) => {
                if let Some(gid) = self.folder_delete {
                    // Drop every host inside the folder, then the folder.
                    let to_drop: Vec<_> = self
                        .connections
                        .iter()
                        .filter(|c| c.group_id == Some(gid))
                        .map(|c| c.id)
                        .collect();
                    // Nested sub-groups (dynamic ECS / K8s groups) aren't
                    // "hosts": re-home them to root rather than deleting
                    // them with the folder, so an import isn't silently
                    // lost and nothing dangles off the removed parent.
                    for g in self.groups.iter_mut() {
                        if g.parent_id == Some(gid) {
                            g.parent_id = None;
                            g.updated_at = chrono::Utc::now();
                            if let Some(vault) = &self.vault {
                                let _ = vault.save_group(g);
                            }
                        }
                    }
                    if let Some(vault) = &self.vault {
                        for cid in &to_drop {
                            let _ = vault.delete_connection(cid);
                        }
                        let _ = vault.delete_group(&gid);
                    }
                    self.connections.retain(|c| !to_drop.contains(&c.id));
                    self.groups.retain(|g| g.id != gid);
                    if self.active_group == Some(gid) {
                        self.active_group = None;
                    }
                    self.close_modal(crate::state::Modal::FolderDelete);
                }
            }
            Message::Tabs(TabsMessage::CloseOtherTabs(idx)) => {
                self.overlay = None;
                if idx < self.tabs.len() {
                    // Keep the clicked tab and every pinned tab (pinned tabs
                    // survive "close others", like a browser).
                    let target_id = self.tabs[idx]._id;
                    // Capture the connecting tab's id before filtering, so the
                    // progress state can be re-anchored / dropped afterwards.
                    let connecting_id = self
                        .connecting
                        .as_ref()
                        .and_then(|p| self.tabs.get(p.tab_idx))
                        .map(|t| t._id);
                    self.tabs.retain(|t| t._id == target_id || t.pinned);
                    let new_active = self
                        .tabs
                        .iter()
                        .position(|t| t._id == target_id)
                        .unwrap_or(0);
                    self.active_tab = Some(new_active);
                    self.remember_terminal_tab_focus(new_active);
                    self.reanchor_connecting_after_filter(connecting_id);
                }
            }
            Message::Tabs(TabsMessage::CloseAllTabs) => {
                self.overlay = None;
                let connecting_id = self
                    .connecting
                    .as_ref()
                    .and_then(|p| self.tabs.get(p.tab_idx))
                    .map(|t| t._id);
                // Pinned tabs survive "close all".
                self.tabs.retain(|t| t.pinned);
                if self.tabs.is_empty() {
                    self.active_tab = None;
                    self.clear_terminal_tab_memory();
                    self.active_view = View::Dashboard;
                    self.connecting = None;
                } else {
                    self.active_tab = Some(0);
                    self.remember_terminal_tab_focus(0);
                    self.reanchor_connecting_after_filter(connecting_id);
                }
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
