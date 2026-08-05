//! What is open on top of the vault surface.
//!
//! Twenty-three booleans on `Oryxis`, each a panel, modal, dropdown or
//! gallery answering the same question: am I showing? Grouped rather
//! than collapsed into one "current overlay" enum on purpose, because
//! several of these legitimately stack (a picker over a panel) and the
//! enum would be a lie the first nested surface exposes.

/// Nothing is open on a fresh boot, which is exactly what the
/// derive says: every one of these is `false`.
#[derive(Debug, Default)]
pub(crate) struct PanelsOpen {
    pub(crate) new_tab_picker: bool,
    /// Termius-style "Jump to" modal, lists all open tabs (plus Quick
    /// connect entries) for direct navigation when the bar runs out of
    /// horizontal room. Triggered by the `⋯` button in the tab bar or
    /// Ctrl+J anywhere.
    pub(crate) tab_jump: bool,
    /// Top-left burger menu visibility. Mirrors Termius's `☰` strip at
    /// the start of the tab bar: Settings / Updates / About / Exit.
    /// Toggled via the burger button or by pressing the same button
    /// again to dismiss.
    pub(crate) burger_menu: bool,
    /// Vault sub-nav overflow ("…") menu: open when the pill strip
    /// can't fit every destination and the user clicked the cue.
    pub(crate) subnav_overflow: bool,
    // Icon/color picker (from the host editor's icon box).
    pub(crate) icon_picker: bool,
    /// Whether the per-host terminal theme picker modal is open.
    /// Drawn on top of the host editor; the form's
    /// `terminal_theme` field is updated as soon as the user picks
    /// a card.
    pub(crate) theme_picker: bool,
    /// Whether the jump host picker modal is open. Opened from the
    /// Chain editor (Termius-style multi-hop jump-host editor), opened
    /// from the "Host Chaining" row in the host editor. `adding` flips
    /// the modal into "pick a host to append" mode; the search filters
    /// that list by label, hostname, group, or username.
    pub(crate) chain_editor: bool,
    // Connection editor
    pub(crate) host_panel: bool,
    // Session group editor (save / edit a split arrangement)
    pub(crate) session_group_panel: bool,
    pub(crate) key_panel: bool,
    /// Whether the generation panel is open (mutually exclusive with
    /// the import/identity panels in the keys view).
    pub(crate) key_generate_panel: bool,
    pub(crate) identity_panel: bool,
    pub(crate) keychain_add_menu: bool,
    /// Import-theme modal (paste an iTerm / Windows Terminal / base16
    /// scheme). On import the parsed colors open in the editor for review.
    pub(crate) theme_import: bool,
    pub(crate) ui_theme_import: bool,
    pub(crate) snippet_panel: bool,
    pub(crate) port_forward_panel: bool,
    /// Global terminal-theme gallery (Settings > Terminal) is open.
    pub(crate) terminal_theme_gallery: bool,
    /// The app-theme gallery is open (Settings > Interface). Same reason
    /// as its terminal sibling: the grid was the tallest thing on the
    /// page and buried every group under it.
    pub(crate) ui_theme_gallery: bool,
    // Export/Import
    pub(crate) export_dialog: bool,
    pub(crate) import_dialog: bool,
    // Share. The dialog-open flag stays at the top level; its transient
    // editor state is grouped in `share`.
    pub(crate) share_dialog: bool,
    pub(crate) ssh_import_dialog: bool,
    /// The one-entry Import hub: explains the supported formats and
    /// opens a picker whose file is format-detected automatically.
    pub(crate) import_hub: bool,
}

