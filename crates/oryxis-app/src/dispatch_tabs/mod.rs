//! `Oryxis::handle_tabs`, match arms for the tab strip + tab modals
//! (new-tab picker, tab-jump, icon picker), card hover/menu, folder
//! actions, window chrome (drag/resize/min/max/close).

#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]

mod hybrid;
mod icon_picker;
mod lifecycle;
mod merge;
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
        message: TabsMessage,
    ) -> Task<Message> {
        match message {
            // -- Card interactions --
            TabsMessage::CardHovered(idx) => {
                self.hovered_card = Some(idx);
            }
            TabsMessage::CardUnhovered => {
                self.hovered_card = None;
            }
            TabsMessage::FolderCardHovered(gid) => {
                self.hovered_folder_card = Some(gid);
            }
            TabsMessage::FolderCardUnhovered => {
                self.hovered_folder_card = None;
            }
            TabsMessage::KeyCardHovered(idx) => {
                self.hovered_key_card = Some(idx);
            }
            TabsMessage::KeyCardUnhovered => {
                self.hovered_key_card = None;
            }
            TabsMessage::IdentityCardHovered(idx) => {
                self.hovered_identity_card = Some(idx);
            }
            TabsMessage::SnippetCardHovered(idx) => {
                self.hovered_snippet_card = Some(idx);
            }
            TabsMessage::SnippetCardUnhovered => {
                self.hovered_snippet_card = None;
            }
            TabsMessage::IdentityCardUnhovered => {
                self.hovered_identity_card = None;
            }
            TabsMessage::MouseMoved(pos) => return self.handle_mouse_moved(pos),
            TabsMessage::WindowResized(size) => return self.handle_window_resized(size),
            TabsMessage::WindowMoved(pos) => {
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
            TabsMessage::WindowEnsureOnScreen => return self.handle_window_ensure_on_screen(),
            TabsMessage::WindowFocusChanged(focused) => return self.handle_window_focus_changed(focused),
            TabsMessage::SsmKeepaliveTick => {
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
            TabsMessage::SettingsTabHovered => {
                self.hovered_settings_tab = true;
            }
            TabsMessage::SettingsTabUnhovered => {
                self.hovered_settings_tab = false;
            }
            TabsMessage::CloseSettingsTab => {
                return self.close_settings_tab();
            }
            TabsMessage::WindowDrag => {
                if !self.consume_window_press() {
                    return Task::none();
                }
                return iced::window::latest().then(|id_opt| match id_opt {
                    Some(id) => iced::window::drag(id),
                    None => Task::none(),
                });
            }
            TabsMessage::WindowResizeDrag(direction) => {
                // Ignore resize requests while maximized, the window has no
                // borders to grab and the OS will reject/misbehave on WinIt.
                if self.window_maximized {
                    return Task::none();
                }
                if !self.consume_window_press() {
                    return Task::none();
                }
                return iced::window::latest().then(move |id_opt| match id_opt {
                    Some(id) => iced::window::drag_resize(id, direction),
                    None => Task::none(),
                });
            }
            TabsMessage::WindowExpandVertical => return self.handle_window_expand_vertical(),
            TabsMessage::WindowMinimize => return self.handle_window_minimize(),
            TabsMessage::WindowMaximizeToggle => {
                self.window_maximized = !self.window_maximized;
                // Cheap write, and it keeps the restored state accurate
                // even when the process later dies without reaching an
                // exit path (OS shutdown, kill).
                self.persist_window_geometry();
                return iced::window::latest().then(|id_opt| match id_opt {
                    Some(id) => iced::window::toggle_maximize(id),
                    None => Task::none(),
                });
            }
            TabsMessage::WindowClose => return self.handle_window_close(),
            TabsMessage::WindowFullscreenToggle => return self.handle_window_fullscreen_toggle(),
            TabsMessage::FullscreenHintHide => {
                self.fullscreen_hint_visible = false;
            }
            TabsMessage::SpawnNewWindow => {
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
            TabsMessage::ActivateStripSlot(slot) => {
                if let Some(msg) = self.strip_slot_target(slot) {
                    return Task::done(msg);
                }
            }
            TabsMessage::FocusViewSearch => {
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
                    return self.update(Message::Terminal(TerminalMessage::TerminalSearchOpen));
                }
                if let Some(id) = self.active_view_search_id() {
                    return iced::widget::operation::focus(id);
                }
            }
            TabsMessage::HideOverlayMenu => {
                self.overlay = None;
                self.card_context_menu = None;
                self.snippet_context_menu = None;
                self.key_context_menu = None;
                self.identity_context_menu = None;
                self.port_forward_context_menu = None;
                self.show_keychain_add_menu = false;
            }
            TabsMessage::ShowCardMenu(idx) => {
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
            TabsMessage::HideCardMenu => {
                self.card_context_menu = None;
                self.overlay = None;
            }

            // -- Tabs --
            TabsMessage::SelectTab(idx) => return self.handle_select_tab(idx),
            TabsMessage::ToggleTabFilesMode(idx) => return self.handle_toggle_tab_files_mode(idx),
            TabsMessage::DetachTabSftp(idx) => return self.handle_detach_tab_sftp(idx),
            TabsMessage::CloseTabSftpSession(idx) => return self.handle_close_tab_sftp_session(idx),
            TabsMessage::OpenTerminalForSftpTab(idx) => return self.handle_open_terminal_for_sftp_tab(idx),
            TabsMessage::CopyTabAddress(idx) => {
                self.overlay = None;
                // Resolve through the focused pane's origin, not the tab
                // label: a split tab can hold panes on different hosts and
                // the label may be renamed. `CopyToClipboard` owns the write
                // (one clipboard access per process) and toasts only once the
                // runtime confirms it landed.
                let address = self
                    .tabs
                    .get(idx)
                    .map(|t| t.active().id)
                    .and_then(|pane_id| self.pane_origin_connection(pane_id))
                    .map(|c| c.hostname.clone());
                if let Some(address) = address {
                    return self.update(Message::CopyToClipboard(address));
                }
            }
            TabsMessage::TabHovered(idx) => {
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
            TabsMessage::TabUnhovered => {
                self.hovered_tab = None;
            }
            TabsMessage::TabDragToEnd => {
                // Trailing drop zone: the live-slide only ever moves the
                // dragged tab to *before* a hovered tab, so the slot after the
                // last tab is unreachable by hovering. Entering the `+` area
                // during an active drag fills that gap.
                if let Some(drag) = self.tab_drag.filter(|d| d.active) {
                    self.slide_tab_to_partition_end(drag.from_id);
                }
            }
            TabsMessage::ShowNewTabPicker => {
                // Opening the picker from the `+` button always targets a new
                // tab, never a split (only SplitPane sets that).
                self.overlay = None; // dismiss the `+` hover popover if open
                self.pending_pane_split = None;
                self.show_new_tab_picker = true;
                self.new_tab_picker_search.clear();
                self.new_tab_picker_group = None;
                // Land focus on the search so the picker is
                // type-to-filter from the first keystroke.
                return iced::widget::operation::focus(iced::widget::Id::new(
                    crate::state::NEW_TAB_PICKER_SEARCH_ID,
                ));
            }
            TabsMessage::HideNewTabPicker => {
                self.show_new_tab_picker = false;
                self.pending_pane_split = None;
                self.new_tab_picker_group = None;
            }
            TabsMessage::NewTabPickerOpenGroup(gid) => {
                // Drill into the group; the search box now filters this
                // group's members instead of the top-level list, so clear
                // the leftover top-level needle.
                self.new_tab_picker_group = Some(gid);
                self.new_tab_picker_search.clear();
                // Cloud-query group: kick off (or refresh) the resolve so
                // the ECS tasks / K8s pods load. Reuses the same TTL gate
                // as the dashboard's OpenGroup so we don't hammer the API.
                if self.dynamic_group_needs_resolve(gid) {
                    return self.handle_cloud(CloudMessage::DynamicGroupResolve(gid));
                }
            }
            TabsMessage::NewTabPickerBack => {
                self.new_tab_picker_group = None;
                self.new_tab_picker_search.clear();
            }
            TabsMessage::PickLocalShell => {
                self.show_new_tab_picker = false;
                // Both destinations (a pending split pane and a new tab)
                // take the same route: the local-shell decision applies the
                // user's curated list / "always open X" default and raises
                // the shell picker when there is a real choice to make. The
                // split target stays pending across that picker and is
                // consumed by `open_local_shell_resolved` once a shell is
                // actually chosen. Splitting used to jump straight to the
                // OS default shell instead (issue #108).
                return self.update(Message::Settings(SettingsMessage::OpenLocalShell));
            }
            TabsMessage::ShowTabJump => {
                self.show_tab_jump = true;
                self.tab_jump_search.clear();
                // Land focus on the search so the modal is
                // type-to-filter from the first keystroke, matching the
                // new-tab picker and the command palette. The modal's
                // Up/Down/Enter navigation arrives via the global key
                // subscription, so the focused input never blocks it.
                return iced::widget::operation::focus(iced::widget::Id::new(
                    crate::state::TAB_JUMP_SEARCH_ID,
                ));
            }
            TabsMessage::ToggleBurgerMenu => {
                self.show_burger_menu = !self.show_burger_menu;
            }
            TabsMessage::ToggleSubnavOverflow => {
                self.show_subnav_overflow = !self.show_subnav_overflow;
            }
            TabsMessage::HideTabJump => {
                self.show_tab_jump = false;
            }
            TabsMessage::TabJumpSearchChanged(v) => {
                self.tab_jump_search = v;
            }
            TabsMessage::TabBarWheel(dy) => {
                // Vertical wheel over the tab bar scrolls horizontally
                // iced's horizontal-only scrollable ignores y deltas, so
                // we translate them via scroll_by here. Sign flip so
                // wheel-down brings later tabs into view (matches the
                // direction Chrome/VS Code use).
                return iced::widget::operation::scroll_by(
                    iced::widget::Id::new("tab-scroll"),
                    iced::widget::scrollable::AbsoluteOffset { x: -dy, y: 0.0 },
                );
            }
            TabsMessage::TabJumpSelect(inner) => {
                self.show_tab_jump = false;
                return Task::done(*inner);
            }
            TabsMessage::ShowCommandPalette => {
                // The palette assumes an unlocked vault (its actions do).
                // The hotkey path already gates on this; guard here too so
                // no other producer can open it over the lock screen.
                if self.vault_ui.state != crate::state::VaultState::Unlocked {
                    return Task::none();
                }
                self.palette.open = true;
                self.palette.query.clear();
                // Focus the query input so the user types immediately.
                return iced::widget::operation::focus(
                    iced::widget::Id::new(crate::palette::PALETTE_INPUT_ID),
                );
            }
            TabsMessage::HideCommandPalette => {
                self.palette.open = false;
                self.palette.query.clear();
            }
            TabsMessage::PaletteQueryChanged(v) => {
                self.palette.query = v;
            }
            TabsMessage::PaletteActivate(inner) => {
                // Two-step dispatch like TabJumpSelect: close first, then
                // fire the row's real message (it may open another modal).
                self.palette.open = false;
                self.palette.query.clear();
                return Task::done(*inner);
            }
            TabsMessage::RunHotkeyAction(action) => {
                return self.dispatch_hotkey_action(
                    action,
                    crate::hotkeys::FamilyMatch::Plain,
                );
            }
            TabsMessage::OpenSettingsSection(section) => {
                // Switch to Settings AND select the section:
                // ChangeSettingsSection alone assumes the view is open.
                let t1 = self.update(Message::Navigation(NavigationMessage::ChangeView(View::Settings)));
                let t2 = self.update(Message::Settings(SettingsMessage::ChangeSettingsSection(section)));
                return Task::batch([t1, t2]);
            }

            TabsMessage::NewTabPickerSearchChanged(v) => {
                self.new_tab_picker_search = v;
            }
            TabsMessage::NewTabPickerSubmit => {
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
                        return self.update(msg);
                    }
                }
                if let Some(conn) = self.quick_connect_target(&self.new_tab_picker_search)
                {
                    return self.update(Message::Ssh(SshMessage::QuickConnect(Box::new(
                        crate::state::QuickConnectEntry::bare(conn),
                    ))));
                }
                let top = self.keynav.modal.items.borrow().first().cloned();
                if let Some(msg) = top.and_then(|a| a.activate) {
                    return self.update(msg);
                }
            }
            TabsMessage::ShowIconPicker(conn_id) => {
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
            TabsMessage::HideIconPicker => {
                self.show_icon_picker = false;
                self.icon_picker.for_id = None;
                self.icon_picker.for_group_form = false;
                self.icon_picker.for_session_group = false;
                self.icon_picker.for_group_edit = false;
                self.icon_picker.for_local_terminal = false;
                self.icon_picker.icon_search.clear();
                self.icon_color_popover = None;
            }
            TabsMessage::IconPickerSelectIcon(name) => {
                self.icon_picker.icon = Some(name);
            }
            TabsMessage::IconPickerIconSearchChanged(q) => {
                self.icon_picker.icon_search = q;
            }
            TabsMessage::IconPickerOpenColorPopover => {
                self.icon_color_popover = Some(self.mouse_position);
            }
            TabsMessage::IconPickerCloseColorPopover => {
                self.icon_color_popover = None;
            }
            TabsMessage::IconPickerSelectColor(hex) => {
                self.icon_picker.hex_input = hex.clone();
                self.icon_picker.color = Some(hex);
            }
            TabsMessage::IconPickerHexInputChanged(v) => {
                self.icon_picker.hex_input = v.clone();
                // Validate + commit only on well-formed #RRGGBB.
                let trimmed = v.trim().trim_start_matches('#');
                if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                    self.icon_picker.color = Some(format!("#{}", trimmed.to_uppercase()));
                }
            }
            TabsMessage::IconPickerSave => return self.handle_icon_picker_save(),
            TabsMessage::IconPickerResetAuto => return self.handle_icon_picker_reset_auto(),
            TabsMessage::CloseTab(idx) => return self.handle_close_tab(idx),
            TabsMessage::ConfirmCloseGroupedTab(idx) => return self.close_tab_now(idx),
            TabsMessage::ShowTabMenu(idx) => {
                let anchor = self.keynav_take_menu_anchor();
                self.overlay = Some(OverlayState {
                    content: OverlayContent::TabActions(idx),
                    x: anchor.0,
                    y: anchor.1,
                });
            }
            TabsMessage::ShowSplitMenu => {
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
            TabsMessage::SplitMenuEnter => {
                self.split_menu_hovered = true;
            }
            TabsMessage::SplitMenuLeave => {
                // Left the `+` button or the popover. Defer the close briefly
                // so moving from the button INTO the menu (which re-enters
                // via `SplitMenuEnter`) doesn't flap it shut.
                self.split_menu_hovered = false;
                return Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(180)).await;
                    },
                    |_| Message::Tabs(TabsMessage::SplitMenuCloseIfIdle),
                );
            }
            TabsMessage::SplitMenuCloseIfIdle => {
                if !self.split_menu_hovered
                    && matches!(
                        self.overlay.as_ref().map(|o| &o.content),
                        Some(OverlayContent::SplitMenu)
                    )
                {
                    self.overlay = None;
                }
            }
            TabsMessage::ToggleTabPin(idx) => {
                self.overlay = None;
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.pinned = !tab.pinned;
                }
                self.persist_pinned_tabs();
            }
            TabsMessage::ReconnectTab(idx) => return self.handle_reconnect_tab(idx),
            TabsMessage::DuplicateTab(idx) => return self.handle_duplicate_tab(idx),
            TabsMessage::DuplicateInNewWindow(idx) => {
                self.overlay = None;
                self.spawn_oryxis_child(Some(idx));
            }
            TabsMessage::ShowFolderActions(gid) => {
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
            TabsMessage::StartRenameFolder(gid) => {
                self.overlay = None;
                let current = self
                    .groups
                    .iter()
                    .find(|g| g.id == gid)
                    .map(|g| g.label.clone())
                    .unwrap_or_default();
                self.folder_rename = Some((gid, current));
            }
            TabsMessage::FolderRenameInput(val) => {
                if let Some((_, ref mut buf)) = self.folder_rename {
                    *buf = val;
                }
            }
            TabsMessage::ConfirmRenameFolder => {
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
            TabsMessage::CancelFolderModal => {
                self.folder_rename = None;
                self.close_modal(crate::state::Modal::FolderDelete);
            }
            // -- Tab rename (transient custom name) --
            TabsMessage::StartRenameTab(idx) => {
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
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        crate::views::layout::TAB_RENAME_INPUT_ID,
                    ));
                }
            }
            TabsMessage::StartRenameSftpTab(idx) => {
                self.overlay = None;
                if let Some(tab) = self.sftp_tabs.get(idx) {
                    let current = tab.display_label().to_string();
                    self.tab_rename = Some((crate::state::TabRef::Sftp(tab.id), current));
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        crate::views::layout::TAB_RENAME_INPUT_ID,
                    ));
                }
            }
            TabsMessage::TabRenameInput(val) => {
                if let Some((_, ref mut buf)) = self.tab_rename {
                    *buf = val;
                }
            }
            TabsMessage::ConfirmTabRename => {
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
                        // Not renameable: it has no per-tab identity to
                        // name, and the rename entry is never offered for
                        // it. Reachable only if some future surface starts
                        // a rename on it, which should do nothing.
                        crate::state::TabRef::Settings => {}
                    }
                }
            }
            TabsMessage::CancelTabRename => {
                self.tab_rename = None;
            }
            TabsMessage::EditGroup(gid) => {
                self.overlay = None;
                if let Some(group) = self.groups.iter().find(|g| g.id == gid) {
                    self.group_edit.id = Some(gid);
                    self.group_edit.label = group.label.clone();
                    self.group_edit.icon = group.icon.clone().unwrap_or_default();
                    self.group_edit.color = group.color.clone().unwrap_or_default();
                    // Resolve the stored parent id back to its full
                    // breadcrumb path for the combo (what the picker
                    // displays); a dangling id (deleted parent) shows
                    // as root, matching how the grid renders it.
                    self.group_edit.parent_label = group
                        .parent_id
                        .filter(|pid| self.groups.iter().any(|g| g.id == *pid))
                        .map(|pid| oryxis_core::models::Group::path_of(&self.groups, pid))
                        .unwrap_or_default();
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
            TabsMessage::NewSubgroup(gid) => {
                self.overlay = None;
                // Only a manual folder can contain manual children
                // (dynamic groups derive their contents from the query).
                let parent_label = self
                    .groups
                    .iter()
                    .any(|g| g.id == gid && g.cloud_query.is_none())
                    .then(|| oryxis_core::models::Group::path_of(&self.groups, gid));
                if let Some(parent_label) = parent_label {
                    self.group_edit = crate::state::GroupEditForm {
                        visible: true,
                        id: None,
                        label: String::new(),
                        icon: String::new(),
                        color: String::new(),
                        parent_label,
                    };
                    // Mutually exclusive with the other right-hand panels.
                    self.show_host_panel = false;
                    self.panel_nav_clear();
                    self.show_session_group_panel = false;
                    self.cloud_form.visible = false;
                    self.cloud_dynamic_form.visible = false;
                    self.cloud_discover_visible = false;
                }
            }
            TabsMessage::NewGroup => {
                self.overlay = None;
                // Create a fresh top-level folder: empty parent = root.
                // Symmetric counterpart to "New subgroup", so an empty
                // group can be born from the add menu instead of only by
                // typing a new name in the host editor's group combo.
                self.group_edit = crate::state::GroupEditForm {
                    visible: true,
                    id: None,
                    label: String::new(),
                    icon: String::new(),
                    color: String::new(),
                    parent_label: String::new(),
                };
                // Mutually exclusive with the other right-hand panels.
                self.show_host_panel = false;
                self.panel_nav_clear();
                self.show_session_group_panel = false;
                self.cloud_form.visible = false;
                self.cloud_dynamic_form.visible = false;
                self.cloud_discover_visible = false;
            }
            TabsMessage::GroupEditLabelChanged(v) => {
                self.group_edit.label = v;
            }
            TabsMessage::GroupEditParentChanged(v) => {
                self.group_edit.parent_label = v;
            }
            TabsMessage::ShowGroupEditIconPicker => {
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
            TabsMessage::SaveGroupEdit => {
                let trimmed = self.group_edit.label.trim().to_string();
                if !trimmed.is_empty() {
                    // Resolve the parent combo by label, mirroring the
                    // dynamic-group editor: empty / unmatched = root and
                    // only a manual folder qualifies as a container. The
                    // edited group's own subtree is excluded so a save
                    // can never mint a parent cycle (nesting a folder
                    // under its own descendant would orphan the subtree).
                    let excluded = self
                        .group_edit
                        .id
                        .map(|gid| oryxis_core::models::Group::subtree_ids(&self.groups, gid))
                        .unwrap_or_default();
                    // Full-path match first (what the picker fills in),
                    // bare label as the typed-by-hand fallback.
                    let parent_id = oryxis_core::models::Group::resolve_path_or_label(
                        &self.groups,
                        &self.group_edit.parent_label,
                        &excluded,
                    );
                    let icon = if self.group_edit.icon.is_empty() {
                        None
                    } else {
                        Some(self.group_edit.icon.clone())
                    };
                    let color = if self.group_edit.color.is_empty() {
                        None
                    } else {
                        Some(self.group_edit.color.clone())
                    };
                    if let Some(gid) = self.group_edit.id {
                        if let Some(group) = self.groups.iter_mut().find(|g| g.id == gid) {
                            group.label = trimmed;
                            group.icon = icon;
                            group.color = color;
                            group.parent_id = parent_id;
                            group.updated_at = chrono::Utc::now();
                            if let Some(vault) = &self.vault {
                                let _ = vault.save_group(group);
                            }
                        }
                    } else {
                        // Create mode (the folder kebab's "New subgroup").
                        // Reuse an existing manual folder with the same
                        // (label, parent) instead of minting a
                        // byte-identical duplicate: two "New subgroup"s
                        // with the same name under one parent would
                        // otherwise produce two groups with identical
                        // breadcrumb paths, the second unselectable
                        // (combos resolve first-match-wins) and an
                        // indistinguishable duplicate card. Reuse mirrors
                        // the host editor's find-or-create semantics; the
                        // user's icon / colour edits are folded onto the
                        // existing folder so the save isn't a silent
                        // no-op. Navigation is intentionally left alone,
                        // matching the fresh-create branch below.
                        let dup = self
                            .groups
                            .iter()
                            .find(|g| {
                                g.cloud_query.is_none()
                                    && g.parent_id == parent_id
                                    && g.label == trimmed
                            })
                            .map(|g| g.id);
                        if let Some(gid) = dup {
                            if let Some(group) =
                                self.groups.iter_mut().find(|g| g.id == gid)
                            {
                                group.icon = icon;
                                group.color = color;
                                group.updated_at = chrono::Utc::now();
                                if let Some(vault) = &self.vault {
                                    let _ = vault.save_group(group);
                                }
                            }
                        } else {
                            let mut group = oryxis_core::models::Group::new(trimmed);
                            group.icon = icon;
                            group.color = color;
                            group.parent_id = parent_id;
                            if let Some(vault) = &self.vault {
                                let _ = vault.save_group(&group);
                            }
                            self.groups.push(group);
                        }
                    }
                }
                self.group_edit.visible = false;
                self.group_edit.id = None;
            }
            TabsMessage::CancelGroupEdit => {
                self.group_edit.visible = false;
                self.group_edit.id = None;
            }
            TabsMessage::StartDeleteFolder(gid) => {
                self.overlay = None;
                self.folder_delete = Some(gid);
            }
            TabsMessage::DeleteFolderKeepHosts => {
                if let Some(gid) = self.folder_delete {
                    // Deleting a folder promotes its contents one level
                    // up (to the deleted folder's own parent; root for a
                    // top-level folder, which preserves the pre-subgroup
                    // behavior).
                    let new_parent = self
                        .groups
                        .iter()
                        .find(|g| g.id == gid)
                        .and_then(|g| g.parent_id);
                    // Track any vault write failure across the re-home
                    // passes. A silently dropped Result here could leave
                    // a host / subgroup pointing at a group we then
                    // tombstone, stranding it (renders nowhere at root).
                    // So we surface failures and skip the final delete
                    // unless every child was re-homed successfully.
                    let mut write_failed = false;
                    for conn in self.connections.iter_mut() {
                        if conn.group_id == Some(gid) {
                            conn.group_id = new_parent;
                            conn.updated_at = chrono::Utc::now();
                            if let Some(vault) = &self.vault
                                && let Err(e) = vault.save_connection(conn, None)
                            {
                                tracing::error!(
                                    "delete folder {gid}: failed to re-home host {}: {e}",
                                    conn.id
                                );
                                write_failed = true;
                            }
                        }
                    }
                    // Re-home nested sub-groups (manual subgroups and
                    // ECS / K8s dynamic groups alike), so they don't
                    // dangle off the deleted parent and vanish from
                    // every view.
                    for g in self.groups.iter_mut() {
                        if g.parent_id == Some(gid) {
                            g.parent_id = new_parent;
                            g.updated_at = chrono::Utc::now();
                            if let Some(vault) = &self.vault
                                && let Err(e) = vault.save_group(g)
                            {
                                tracing::error!(
                                    "delete folder {gid}: failed to re-home subgroup {}: {e}",
                                    g.id
                                );
                                write_failed = true;
                            }
                        }
                    }
                    // Only tombstone the folder once every child was
                    // re-homed. On failure, abort with a toast and leave
                    // the folder in place; the user can retry rather than
                    // be left with orphaned hosts.
                    let mut removed = false;
                    if write_failed {
                        self.set_toast(crate::i18n::t("folder_delete_failed").to_string());
                    } else if let Some(vault) = &self.vault {
                        if let Err(e) = vault.delete_group(&gid) {
                            tracing::error!("delete folder {gid}: failed to delete group: {e}");
                            self.set_toast(
                                crate::i18n::t("folder_delete_failed").to_string(),
                            );
                        } else {
                            removed = true;
                        }
                    } else {
                        // No vault (should not happen for a saved
                        // folder), keep the in-memory removal consistent.
                        removed = true;
                    }
                    if removed {
                        self.groups.retain(|g| g.id != gid);
                        if self.active_group == Some(gid) {
                            self.active_group = new_parent;
                        }
                        // Don't leave the editor panel open on a deleted row.
                        if self.group_edit.id == Some(gid) {
                            self.group_edit.visible = false;
                            self.group_edit.id = None;
                        }
                    }
                    self.close_modal(crate::state::Modal::FolderDelete);
                }
            }
            TabsMessage::DeleteFolderWithHosts => {
                if let Some(gid) = self.folder_delete {
                    // Drop every host inside the folder, then the folder.
                    let to_drop: Vec<_> = self
                        .connections
                        .iter()
                        .filter(|c| c.group_id == Some(gid))
                        .map(|c| c.id)
                        .collect();
                    // Nested sub-groups (manual subgroups and dynamic
                    // ECS / K8s groups) aren't "hosts": promote them to
                    // the deleted folder's own parent rather than
                    // deleting them with the folder, so an import isn't
                    // silently lost and nothing dangles off the removed
                    // parent.
                    let new_parent = self
                        .groups
                        .iter()
                        .find(|g| g.id == gid)
                        .and_then(|g| g.parent_id);
                    // Track vault write failures across the re-home and
                    // host-drop passes. A silently dropped Result could
                    // leave a subgroup (re-home failed) or a still-live
                    // host (delete failed) pointing at a group we then
                    // tombstone, stranding it at root. Skip the final
                    // group delete unless every write succeeded.
                    let mut write_failed = false;
                    for g in self.groups.iter_mut() {
                        if g.parent_id == Some(gid) {
                            g.parent_id = new_parent;
                            g.updated_at = chrono::Utc::now();
                            if let Some(vault) = &self.vault
                                && let Err(e) = vault.save_group(g)
                            {
                                tracing::error!(
                                    "delete folder {gid}: failed to re-home subgroup {}: {e}",
                                    g.id
                                );
                                write_failed = true;
                            }
                        }
                    }
                    let mut dropped: Vec<uuid::Uuid> = Vec::new();
                    if let Some(vault) = &self.vault {
                        for cid in &to_drop {
                            if let Err(e) = vault.delete_connection(cid) {
                                tracing::error!(
                                    "delete folder {gid}: failed to delete host {cid}: {e}"
                                );
                                write_failed = true;
                            } else {
                                // Saved AI conversations reference the host
                                // by id; sweep them with it.
                                let _ = vault.delete_chat_conversations_for_connection(cid);
                                dropped.push(*cid);
                            }
                        }
                    } else {
                        dropped = to_drop.clone();
                    }
                    // Drop from memory only the hosts actually removed
                    // from the vault, so a failed delete doesn't vanish
                    // the row while its record survives on disk.
                    self.connections.retain(|c| !dropped.contains(&c.id));
                    // Only tombstone the folder once every child write
                    // landed. On failure, abort with a toast and keep the
                    // folder so nothing is stranded; the user can retry.
                    let mut removed = false;
                    if write_failed {
                        self.set_toast(crate::i18n::t("folder_delete_failed").to_string());
                    } else if let Some(vault) = &self.vault {
                        if let Err(e) = vault.delete_group(&gid) {
                            tracing::error!("delete folder {gid}: failed to delete group: {e}");
                            self.set_toast(
                                crate::i18n::t("folder_delete_failed").to_string(),
                            );
                        } else {
                            removed = true;
                        }
                    } else {
                        removed = true;
                    }
                    if removed {
                        self.groups.retain(|g| g.id != gid);
                        if self.active_group == Some(gid) {
                            self.active_group = new_parent;
                        }
                        // Don't leave the editor panel open on a deleted row.
                        if self.group_edit.id == Some(gid) {
                            self.group_edit.visible = false;
                            self.group_edit.id = None;
                        }
                    }
                    self.close_modal(crate::state::Modal::FolderDelete);
                }
            }
            TabsMessage::CloseOtherTabs(idx) => {
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
            TabsMessage::CloseAllTabs => {
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
        }
        Task::none()
    }
}
