//! Menus and the odd global affordance: the burger, the sub-nav
//! overflow, a card's kebab, the view's search field, and running a
//! hotkey action by name.

use super::*;

impl Oryxis {
    pub(super) fn handle_tabs_menus(&mut self, message: TabsMessage) -> Task<Message> {
        match message {
            TabsMessage::ToggleBurgerMenu => {
                self.panels.burger_menu = !self.panels.burger_menu;
            }
            TabsMessage::ToggleSubnavOverflow => {
                self.panels.subnav_overflow = !self.panels.subnav_overflow;
            }
            TabsMessage::HideOverlayMenu => {
                self.overlay = None;
                self.card_context_menu = None;
                self.snippet_context_menu = None;
                self.keys_ui.context_menu = None;
                self.identity_context_menu = None;
                self.port_forward_context_menu = None;
                self.panels.keychain_add_menu = false;
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
            // Routed here by the parent; anything else is a
            // grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
