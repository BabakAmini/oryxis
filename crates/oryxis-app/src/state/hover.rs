//! Which card, row or chip the cursor is over.
//!
//! Every one of these exists for the same reason: per-row action icons
//! are floating and hover-revealed by convention (CLAUDE.md), never
//! inline, so each list that has actions needs to know which of its rows
//! the pointer is on. They were twenty `hovered_*` fields on `Oryxis`,
//! which is twenty declarations, twenty boot initializers and twenty
//! chances to leave one behind when a surface is removed.
//!
//! Grouping them buys one more thing: a surface that should drop its
//! highlight can say so in one place (`clear()`), instead of every
//! caller remembering which field belongs to the view it just left.

use uuid::Uuid;

/// The hover cursor for every card / row list in the app.
///
/// Index-based where the list is positional (it is rebuilt per frame in
/// render order, so an index is only ever read on the frame that
/// recorded it), id-based where the row survives a re-sort.
#[derive(Debug, Clone, Default)]
pub(crate) struct HoverState {
    /// Tab strip chip (terminal side).
    pub(crate) tab: Option<usize>,
    /// SFTP tab chip, which is a separate list in the same strip.
    pub(crate) sftp_tab: Option<usize>,
    /// The Settings chip, which is one entry rather than a list.
    pub(crate) settings_tab: bool,

    /// Host card on the dashboard.
    pub(crate) card: Option<usize>,
    /// Group / folder card, keyed by id: folders re-sort on rename.
    pub(crate) folder_card: Option<Uuid>,
    pub(crate) session_group_card: Option<usize>,

    pub(crate) key_card: Option<usize>,
    pub(crate) identity_card: Option<usize>,
    pub(crate) snippet_card: Option<usize>,
    pub(crate) port_forward_card: Option<usize>,
    pub(crate) local_terminal_card: Option<usize>,

    /// Cloud account card and the dynamic-group card beside it, both
    /// keyed by id for the same reason folders are.
    pub(crate) cloud_card: Option<Uuid>,
    pub(crate) dynamic_group_card: Option<Uuid>,

    /// Terminal theme cards: the user's own, and the built-ins (whose
    /// only hover action is Clone).
    pub(crate) theme_card: Option<usize>,
    pub(crate) builtin_theme_card: Option<usize>,
    /// The same pair for app / UI themes.
    pub(crate) ui_theme_card: Option<usize>,
    pub(crate) builtin_ui_theme_card: Option<usize>,

    /// History screen: the recording card, and the session-log row keyed
    /// by log id.
    pub(crate) history_card: Option<usize>,
    pub(crate) log_row: Option<Uuid>,

    /// Files sidebar row.
    pub(crate) files_row: Option<usize>,
}

impl HoverState {
    /// Drop every highlight.
    ///
    /// A hover is only true while the pointer is where it was, and
    /// anything that replaces the surface underneath (a view change, a
    /// modal opening over it) makes every one of these a lie at once.
    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Drop `slot` ONLY if the item the cursor left is still the one
    /// holding it. Every `leave_*` below is this, named after its field.
    ///
    /// Crossing from one item to the next fires both events in the SAME
    /// frame, and their order is the list's build order, not the order the
    /// cursor visited them: a `Row` / `Column` updates its children by
    /// index, so moving RIGHT TO LEFT (or BOTTOM TO TOP) publishes the
    /// arriving item's `on_enter` first and the departing item's `on_exit`
    /// second. An unconditional clear then wipes the hover it had just
    /// gained, and the floating actions never appear. Testing the key
    /// makes the pair order-independent.
    ///
    /// A gap between items only hides it: the guard runs per pointer
    /// event, so any move long enough to skip the gap in one frame (an
    /// ordinary flick of the wrist) delivers the pair back to back. The
    /// tab strip put 1 px between chips and broke every time; the SFTP
    /// rows have none and broke "maybe one time in ten" (a drag that
    /// never armed, see `SftpRowExit`).
    fn leave<T: PartialEq>(slot: &mut Option<T>, left: T) {
        if slot.as_ref() == Some(&left) {
            *slot = None;
        }
    }

    /// Terminal tab chip, and the SFTP chips beside it in the same strip.
    pub(crate) fn leave_tab(&mut self, idx: usize) {
        Self::leave(&mut self.tab, idx);
    }

    pub(crate) fn leave_sftp_tab(&mut self, idx: usize) {
        Self::leave(&mut self.sftp_tab, idx);
    }

    /// Dashboard: host cards, folder cards, session-group cards.
    pub(crate) fn leave_card(&mut self, idx: usize) {
        Self::leave(&mut self.card, idx);
    }

    pub(crate) fn leave_folder_card(&mut self, gid: Uuid) {
        Self::leave(&mut self.folder_card, gid);
    }

    pub(crate) fn leave_session_group_card(&mut self, idx: usize) {
        Self::leave(&mut self.session_group_card, idx);
    }

    /// Keychain: key cards and identity cards, two grids on one screen.
    pub(crate) fn leave_key_card(&mut self, idx: usize) {
        Self::leave(&mut self.key_card, idx);
    }

    pub(crate) fn leave_identity_card(&mut self, idx: usize) {
        Self::leave(&mut self.identity_card, idx);
    }

    /// Snippets, from both the full screen and the terminal sidebar.
    pub(crate) fn leave_snippet_card(&mut self, idx: usize) {
        Self::leave(&mut self.snippet_card, idx);
    }

    pub(crate) fn leave_port_forward_card(&mut self, idx: usize) {
        Self::leave(&mut self.port_forward_card, idx);
    }

    pub(crate) fn leave_local_terminal_card(&mut self, idx: usize) {
        Self::leave(&mut self.local_terminal_card, idx);
    }

    /// Cloud accounts and the dynamic-group cards on the dashboard.
    pub(crate) fn leave_cloud_card(&mut self, id: Uuid) {
        Self::leave(&mut self.cloud_card, id);
    }

    pub(crate) fn leave_dynamic_group_card(&mut self, id: Uuid) {
        Self::leave(&mut self.dynamic_group_card, id);
    }

    /// Theme galleries: the user's own and the built-ins, terminal and UI.
    pub(crate) fn leave_theme_card(&mut self, idx: usize) {
        Self::leave(&mut self.theme_card, idx);
    }

    pub(crate) fn leave_builtin_theme_card(&mut self, idx: usize) {
        Self::leave(&mut self.builtin_theme_card, idx);
    }

    pub(crate) fn leave_ui_theme_card(&mut self, idx: usize) {
        Self::leave(&mut self.ui_theme_card, idx);
    }

    pub(crate) fn leave_builtin_ui_theme_card(&mut self, idx: usize) {
        Self::leave(&mut self.builtin_ui_theme_card, idx);
    }

    /// History: the command-history cards and the session-log rows.
    pub(crate) fn leave_history_card(&mut self, idx: usize) {
        Self::leave(&mut self.history_card, idx);
    }

    pub(crate) fn leave_log_row(&mut self, id: Uuid) {
        Self::leave(&mut self.log_row, id);
    }

    /// Files sidebar row.
    pub(crate) fn leave_files_row(&mut self, idx: usize) {
        Self::leave(&mut self.files_row, idx);
    }
}

#[cfg(test)]
mod tests {
    use super::{HoverState, Uuid};

    /// The regression: the cursor moves from tab 1 onto tab 0, so the
    /// enter lands before the exit. The stale exit must not take the new
    /// hover with it.
    #[test]
    fn exit_from_the_tab_left_behind_keeps_the_new_hover() {
        let mut hover = HoverState { tab: Some(1), ..Default::default() };

        // Same frame, strip order: tab 0 enters, then tab 1 leaves.
        hover.tab = Some(0);
        hover.leave_tab(1);

        assert_eq!(hover.tab, Some(0));
    }

    /// Leaving the strip for good (or moving left to right, where the exit
    /// arrives first) still clears.
    #[test]
    fn exit_from_the_hovered_tab_clears_it() {
        let mut hover = HoverState { tab: Some(2), ..Default::default() };
        hover.leave_tab(2);
        assert_eq!(hover.tab, None);
    }

    /// The two chip lists share the strip, so the same pair of events can
    /// cross between them; each only ever drops its own.
    #[test]
    fn sftp_exit_does_not_touch_the_terminal_hover() {
        let mut hover = HoverState { sftp_tab: Some(0), ..Default::default() };

        // Cursor crosses from the SFTP chip onto a terminal tab: entering
        // the terminal tab is what clears the SFTP hover, and the SFTP
        // chip's own exit arrives after it.
        hover.tab = Some(3);
        hover.sftp_tab = None;
        hover.leave_sftp_tab(0);

        assert_eq!(hover.tab, Some(3));
        assert_eq!(hover.sftp_tab, None);
    }

    /// The id-keyed lists (folder / cloud / dynamic-group cards, log rows)
    /// re-sort under the cursor, so they carry a `Uuid` instead of a
    /// position. The guard is the same compare, and it has to hold for a
    /// key that is not `Copy`-cheap ordering.
    #[test]
    fn an_id_keyed_list_guards_the_same_way() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let mut hover = HoverState { log_row: Some(b), ..Default::default() };

        // Walking UP the list: row `a` enters, then row `b` leaves.
        hover.log_row = Some(a);
        hover.leave_log_row(b);
        assert_eq!(hover.log_row, Some(a));

        hover.leave_log_row(a);
        assert_eq!(hover.log_row, None);
    }

    /// Every list gets its own field, so an exit from one can never reach
    /// into another even when both are on screen (the Keychain shows the
    /// key grid and the identity grid at once).
    #[test]
    fn lists_on_the_same_screen_do_not_clear_each_other() {
        let mut hover = HoverState {
            key_card: Some(0),
            identity_card: Some(4),
            ..Default::default()
        };

        hover.leave_key_card(0);

        assert_eq!(hover.key_card, None);
        assert_eq!(hover.identity_card, Some(4));
    }
}
