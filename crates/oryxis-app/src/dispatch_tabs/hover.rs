//! Pointer enter / leave for every card list.
//!
//! One arm per surface, all of them writing the same `HoverState`. They
//! are here rather than with their views because the floating action
//! icons are a convention (CLAUDE.md), so the shape is identical
//! everywhere and worth reading in one place.

use super::*;

impl Oryxis {
    pub(super) fn handle_tabs_hover(&mut self, message: TabsMessage) -> Task<Message> {
        match message {
            TabsMessage::CardHovered(idx) => {
                self.hover.card = Some(idx);
            }
            TabsMessage::CardUnhovered(idx) => {
                self.hover.leave_card(idx);
            }
            TabsMessage::FolderCardHovered(gid) => {
                self.hover.folder_card = Some(gid);
            }
            TabsMessage::FolderCardUnhovered(gid) => {
                self.hover.leave_folder_card(gid);
            }
            TabsMessage::KeyCardHovered(idx) => {
                self.hover.key_card = Some(idx);
            }
            TabsMessage::KeyCardUnhovered(idx) => {
                self.hover.leave_key_card(idx);
            }
            TabsMessage::IdentityCardHovered(idx) => {
                self.hover.identity_card = Some(idx);
            }
            TabsMessage::IdentityCardUnhovered(idx) => {
                self.hover.leave_identity_card(idx);
            }
            TabsMessage::SnippetCardHovered(idx) => {
                self.hover.snippet_card = Some(idx);
            }
            TabsMessage::SnippetCardUnhovered(idx) => {
                self.hover.leave_snippet_card(idx);
            }
            TabsMessage::SettingsTabHovered => {
                self.hover.settings_tab = true;
            }
            TabsMessage::SettingsTabUnhovered => {
                self.hover.settings_tab = false;
            }
            TabsMessage::TabHovered(idx) => {
                self.hover.tab = Some(idx);
                // Terminal / SFTP hover are mutually exclusive (one cursor).
                self.hover.sftp_tab = None;
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
            TabsMessage::TabUnhovered(idx) => {
                self.hover.leave_tab(idx);
            }
            // Routed here by the parent; anything else is a
            // grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
