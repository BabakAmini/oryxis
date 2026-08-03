//! The Keychain screen: the import and generate forms, the key material
//! pasted into them, and the list's own search and row menu.
//!
//! The three `iced::widget::text_editor::Content` buffers stay OUT of
//! `KeyImportForm` deliberately: that struct is cloned, and
//! `iced::widget::text_editor::Content` is not `Clone`.

#[derive(Debug)]
pub(crate) struct KeysUi {
    /// Live multi-line PEM editor buffer. Stays on `Oryxis` (not in
    /// `key_import_form`) because `iced::widget::text_editor::Content` is not `Clone`.
    pub(crate) import_content: iced::widget::text_editor::Content,
    /// Editor buffers for the public-key / certificate fields (multi-line
    /// like the PEM: OpenSSH lines are far wider than the panel, so a
    /// wrapping textarea beats a one-line input). The canonical values
    /// live in `key_import_form` (synced on every action) because
    /// `Content` is not `Clone`.
    pub(crate) import_public_content: iced::widget::text_editor::Content,
    pub(crate) import_cert_content: iced::widget::text_editor::Content,
    pub(crate) import_form: crate::state::KeyImportForm,
    /// Key-generation panel state (keychain > ADD > Generate key).
    pub(crate) generate_form: crate::state::KeyGenerateForm,
    pub(crate) error: Option<String>,
    pub(crate) success: Option<String>,
    pub(crate) context_menu: Option<usize>,
    pub(crate) search: String,
}

impl Default for KeysUi {
    fn default() -> Self {
        Self {
            import_content: iced::widget::text_editor::Content::new(),
            import_public_content: iced::widget::text_editor::Content::new(),
            import_cert_content: iced::widget::text_editor::Content::new(),
            import_form: crate::state::KeyImportForm::default(),
            generate_form: crate::state::KeyGenerateForm::default(),
            error: None,
            success: None,
            context_menu: None,
            search: String::new(),
        }
    }
}
