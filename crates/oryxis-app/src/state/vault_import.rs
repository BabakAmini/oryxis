//! The portable-import flow: the password typed for an encrypted
//! export, the file that was picked, what it turned out to contain, and
//! what the user chose to bring in.
//!
//! One modal's worth of state, and every field is dead between runs of
//! it, so grouping them also makes "forget the import" a single
//! assignment instead of five.


pub(crate) struct VaultImportState {
    pub(crate) password: String,
    pub(crate) file_data: Option<Vec<u8>>,
    /// Per-category record counts of the picked file, populated by the
    /// "Inspect" step (decrypt + count). `None` until inspected; the
    /// import checkboxes + confirm button only render once it's `Some`.
    pub(crate) summary: Option<oryxis_vault::ExportSummary>,
    /// Which of the inspected categories to apply on import. Defaults to
    /// every category the file actually contains.
    pub(crate) selection: oryxis_vault::ExportSelection,
    pub(crate) status: Option<Result<String, String>>,
}

impl Default for VaultImportState {
    fn default() -> Self {
        Self {
            password: String::new(),
            file_data: None,
            summary: None,
            // Everything is brought in unless the user narrows it, which
            // is what the modal shows on open.
            selection: oryxis_vault::ExportSelection::all(),
            status: None,
        }
    }
}
