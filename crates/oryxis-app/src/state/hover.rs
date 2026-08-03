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
}
