//! The snippet editor's fields.
//!
//! A form like the ones in `forms.rs`, and the last one still spread
//! across `Oryxis` as loose fields. `editing_id` travels with it
//! because it is what makes this an edit rather than a create.

use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct SnippetForm {
    pub(crate) label: String,
    pub(crate) command: iced::widget::text_editor::Content,
    /// Snippet editor: free-form group name (empty = ungrouped).
    pub(crate) group: String,
    /// Options state for the snippet Group combo (same type-ahead
    /// combo_box as the host editor's Parent Group): filters the
    /// existing snippet groups as you type, still accepts a new name.
    /// Rebuilt whenever the editor opens.
    pub(crate) group_combo: iced::widget::combo_box::State<String>,
    /// Snippet editor: comma-separated tags as typed; parsed
    /// (trim/dedup/drop-empty) on save. Chips are display-only, so
    /// the field stays fully keyboard-editable.
    pub(crate) tags_input: String,
    pub(crate) editing_id: Option<Uuid>,
    /// Snippet editor: the custom run hotkey being edited (parsed form
    /// of `Snippet.hotkey`).
    pub(crate) hotkey: Option<crate::hotkeys::HotkeyBinding>,
    /// True while the snippet editor's shortcut recorder waits for a
    /// key combo (next chord becomes the binding, Esc cancels).
    pub(crate) hotkey_capturing: bool,
    /// "Install script" category toggle (issue #147).
    pub(crate) install: bool,
    pub(crate) error: Option<String>,
}

impl Default for SnippetForm {
    fn default() -> Self {
        Self {
            label: String::new(),
            command: iced::widget::text_editor::Content::new(),
            group: String::new(),
            group_combo: iced::widget::combo_box::State::new(Vec::new()),
            tags_input: String::new(),
            editing_id: None,
            hotkey: None,
            hotkey_capturing: false,
            install: false,
            error: None,
        }
    }
}
