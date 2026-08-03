//! Starting a session from a pick: a saved host from the list, or a
//! typed `user@host` that was never saved.
//!
//! Both first check whether the pick is filling a pending split, in
//! which case the per-pane path runs instead of opening a new tab.
//! The dial itself is `connect::start_ssh_tab`.

use super::*;

impl Oryxis {
    pub(super) fn handle_ssh_launch(&mut self, message: SshMessage) -> Task<Message> {
        match message {
            // -- SSH connection --
            SshMessage::ConnectSsh(idx) => {
                self.card_context_menu = None;
                self.overlay = None;
                // Close the new-tab picker if the connection was picked there.
                self.panels.new_tab_picker = false;
                // If this pick is filling a split pane (not a new tab),
                // route to the per-pane connect path instead.
                if let Some((tab_id, target, axis)) = self.pending_pane_split.take() {
                    // The tab the split was aimed at may be gone (closed,
                    // or merged into another). Falling through to the
                    // normal path opens the session as its own tab, which
                    // is what the user asked for minus the placement.
                    if let Some(tab_idx) = self.tab_index_by_id(tab_id) {
                        return self.connect_ssh_into_pane(idx, tab_idx, target, axis);
                    }
                }
                if let Some(conn) = self.connections.get(idx).cloned() {
                    return 
                        self.start_ssh_tab(conn, crate::state::ProgressOrigin::Saved(idx))
                    ;
                }
            }
            SshMessage::QuickConnect(entry) => {
                self.card_context_menu = None;
                self.overlay = None;
                self.panels.new_tab_picker = false;
                // Reuse an existing entry for the same id: a retry after the
                // legacy-algorithm dialog must see its in-place mutations.
                // First connects insert the incoming entry.
                let id = entry.conn.id;
                let conn = self
                    .quick_connects
                    .entry(id)
                    .or_insert_with(|| *entry)
                    .conn
                    .clone();
                if let Some((tab_id, target, axis)) = self.pending_pane_split.take()
                    && let Some(tab_idx) = self.tab_index_by_id(tab_id)
                {
                    return self.quick_connect_into_pane(id, tab_idx, target, axis);
                }
                return self.start_ssh_tab(conn, crate::state::ProgressOrigin::Quick(id));
            }
            // The router sends only this family here, so anything
            // else is a grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
