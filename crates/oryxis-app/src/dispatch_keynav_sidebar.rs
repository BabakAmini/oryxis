//! Keyboard router for the terminal-sidebar list tabs (Snippets /
//! History), iteration 3 of the focus-zone framework.
//!
//! Unlike modals and side panels, these lists coexist with a live
//! terminal that owns every plain key, so the ring is strictly
//! opt-in:
//!
//! - The FocusSidebarList hotkey opens the sidebar (when closed),
//!   lands on a list tab and rings the first row; pressed again it
//!   cycles Snippets <-> History.
//! - Up / Down while the mouse cursor is over the sidebar engage the
//!   ring directly (those keys were already swallowed there, never
//!   reaching the PTY, so this upgrades a dead key into navigation).
//!
//! While engaged: Up/Down/Home/End move (wrapping), Enter pastes the
//! row (its click action), Shift+Enter runs it (+ Enter), Delete
//! removes it, Esc disengages. Everything else, typing included,
//! keeps its normal routing, so the terminal (or the list's search
//! field, when focused) still receives text while the ring is up;
//! the selection is tagged by sidebar tab and clamped against each
//! frame's recording, so filtering while ringed just clamps.

use iced::keyboard;
use iced::Task;

use crate::app::{Message, Oryxis};
use crate::keynav::movement::index_move;
use crate::state::TerminalSidebarTab;

impl Oryxis {
    /// The sidebar tab actually shown for the active terminal tab
    /// (Chat falls back to Snippets while AI is off), or `None` when
    /// no terminal tab is active or its sidebar is closed. Mirrors
    /// the resolution in `view_terminal_sidebar`.
    pub(crate) fn effective_sidebar_tab(&self) -> Option<TerminalSidebarTab> {
        let tab = self.active_tab.and_then(|i| self.tabs.get(i))?;
        if !tab.chat_visible {
            return None;
        }
        let active = if self.terminal_sidebar_tab == TerminalSidebarTab::Chat && !self.ai.enabled
        {
            TerminalSidebarTab::Snippets
        } else {
            self.terminal_sidebar_tab
        };
        Some(active)
    }

    /// The sidebar tab whose row list the ring can drive right now.
    fn sidebar_list_tab(&self) -> Option<TerminalSidebarTab> {
        self.effective_sidebar_tab().filter(|t| {
            matches!(t, TerminalSidebarTab::Snippets | TerminalSidebarTab::History)
        })
    }

    /// Keep the ringed row visible; same best-effort relative snap as
    /// the side-panel router (iced exposes no row bounds). Both list
    /// tabs give their scrollable the shared id (only one renders).
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

    /// Entry point, called from the `KeyboardEvent` arm right after
    /// the vault-area router. Returns `Some(task)` when consumed.
    pub(crate) fn handle_sidebar_nav_key(
        &mut self,
        event: &keyboard::Event,
    ) -> Option<Task<Message>> {
        let list_tab = self.sidebar_list_tab()?;
        if self.any_modal_blocks_input() || self.show_host_panel {
            return None;
        }
        let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            return None;
        };
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        let len = self.keynav.sidebar_items.borrow().len();
        // Selection engaged on the visible list, clamped against this
        // frame's recording (a search filter can shrink it).
        let engaged = match self.keynav.sidebar_selected {
            Some((tag, idx)) if tag == list_tab && len > 0 => Some(idx.min(len - 1)),
            _ => None,
        };
        let cursor_over_sidebar = self
            .active_tab
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.chat_visible)
            .unwrap_or(false)
            && self.mouse_position.x > (self.window_size.width - self.chat_sidebar_width);

        let keyboard::Key::Named(named) = key else {
            return None;
        };
        use keyboard::key::Named;
        match named {
            Named::ArrowUp | Named::ArrowDown => {
                if modifiers.shift() || len == 0 {
                    return None;
                }
                let forward = matches!(named, Named::ArrowDown);
                let next = match engaged {
                    Some(cur) => index_move(len, Some(cur), forward)?,
                    // Not engaged: only the hover gate turns a dead
                    // (already swallowed) arrow into an entry point.
                    None if cursor_over_sidebar => {
                        if forward {
                            0
                        } else {
                            len - 1
                        }
                    }
                    None => return None,
                };
                self.keynav.sidebar_selected = Some((list_tab, next));
                Some(self.sidebar_nav_scroll(next))
            }
            Named::Home | Named::End => {
                engaged?;
                if len == 0 {
                    return Some(Task::none());
                }
                let idx = if matches!(named, Named::Home) { 0 } else { len - 1 };
                self.keynav.sidebar_selected = Some((list_tab, idx));
                Some(self.sidebar_nav_scroll(idx))
            }
            Named::Enter => {
                let idx = engaged?;
                let row = self.keynav.sidebar_items.borrow().get(idx).cloned()?;
                // Shift+Enter = run (+ Enter); plain Enter = paste, the
                // row's click action. A row without a run verb (the
                // sudo helper) falls back to its primary either way.
                let msg = if modifiers.shift() {
                    row.run.unwrap_or(row.paste)
                } else {
                    row.paste
                };
                Some(self.update(msg))
            }
            Named::Delete => {
                let idx = engaged?;
                let row = self.keynav.sidebar_items.borrow().get(idx).cloned()?;
                let msg = row.delete?;
                // The recording shrinks next frame; the selection is
                // clamped on the next key, so the ring lands on the
                // neighbor instead of vanishing.
                Some(self.update(msg))
            }
            Named::Escape => {
                if engaged.is_some() {
                    self.keynav.sidebar_selected = None;
                    return Some(Task::none());
                }
                None
            }
            _ => None,
        }
    }

    /// FocusSidebarList hotkey: bring the keyboard to the sidebar.
    /// Opens it when closed (landing on the tab it already shows);
    /// pressed again it cycles EVERY visible tab, Chat (AI on) ->
    /// Snippets -> History -> HostConfig -> wrap. Landing on a list
    /// rings its first row; landing on Chat focuses the message
    /// editor. No-op outside a terminal tab.
    pub(crate) fn focus_sidebar_list(&mut self) -> Task<Message> {
        let Some(idx) = self.active_tab else {
            return Task::none();
        };
        let mut order: Vec<TerminalSidebarTab> = Vec::with_capacity(4);
        if self.ai.enabled {
            order.push(TerminalSidebarTab::Chat);
        }
        order.extend([
            TerminalSidebarTab::Snippets,
            TerminalSidebarTab::History,
            TerminalSidebarTab::HostConfig,
        ]);

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
        self.terminal_sidebar_tab = target;
        match target {
            TerminalSidebarTab::Chat => {
                self.keynav.sidebar_selected = None;
                iced::widget::operation::focus(iced::widget::Id::new("chat-input"))
            }
            TerminalSidebarTab::Snippets | TerminalSidebarTab::History => {
                if target == TerminalSidebarTab::History {
                    self.refresh_command_history();
                }
                self.keynav.sidebar_selected = Some((target, 0));
                self.sidebar_nav_scroll(0)
            }
            TerminalSidebarTab::HostConfig => {
                // No recorded rows yet (its selects/toggles join the
                // keyboard layer with the Tab-walk iteration); reaching
                // the tab is the win for now.
                self.keynav.sidebar_selected = None;
                Task::none()
            }
        }
    }
}
