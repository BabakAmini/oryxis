//! Vault export / import and share-to-peer messages, wrapped by
//! [`crate::messages::Message::Share`]. Handled by `Oryxis::handle_share`.

#[derive(Debug, Clone)]
pub enum ShareMessage {
    ExportVault,
    ExportPasswordChanged(super::Redacted),
    ExportToggleKeys,
    /// Toggle one category checkbox in the export dialog.
    ExportToggleCategory(oryxis_vault::ExportCategory),
    ExportConfirm,
    ExportCompleted(Result<String, String>),
    /// Export the host list as a secrets-free CSV, the round-trip mate
    /// of the hub's CSV importer. No dialog step: there is no password
    /// to ask for, so the save dialog is the whole interaction.
    ExportHostsCsv,
    /// The CSV write finished; `Ok` carries the written path.
    ExportHostsCsvCompleted(Result<String, String>),
    ImportVault,
    /// Pick `~/.ssh/config` (or any file the user chooses) and read it.
    /// The parsed Host blocks land in a preview modal where the user
    /// ticks which to import.
    ImportSshConfig,
    /// Open the one-entry Import hub modal.
    ShowImportHub,
    /// Close it.
    ImportHubDismiss,
    /// The hub's "Choose file" button: any-file picker; the format is
    /// detected from the content.
    ImportHubPick,
    /// The picked file's bytes plus its file stem (the per-session
    /// formats have no name field; the file name IS the label).
    ImportHubLoaded(Result<(Vec<u8>, String), String>),
    /// The hub's "Choose folder" button: scan a session directory
    /// (Xshell / SecureCRT / FinalShell keep one file per host).
    ImportHubPickFolder,
    /// A folder scan finished, merged into one batch.
    ImportHubFolderScanned(Box<crate::importers::DirectImport>),
    /// File password typed in the hub (a protected confCons.xml).
    ImportHubPasswordChanged(super::Redacted),
    /// Retry the held protected file with the typed password.
    ImportHubUnlock,
    /// A confCons.xml parse finished off-thread (PBKDF2 key stretching
    /// runs there so a hostile KdfIterations can't freeze the UI). The
    /// flag records whether a real password was supplied, so a second
    /// miss reads as a wrong password rather than the silent default
    /// try.
    ImportHubMrngParsed(Box<crate::importers::mremoteng::MrngParse>, bool),
    /// File contents picked + read by the background task spawned from
    /// `ImportSshConfig`; the handler parses and opens the preview.
    SshConfigFileLoaded(Result<String, String>),
    /// Toggle one parsed host's inclusion in the SSH-config import.
    SshImportToggle(usize),
    /// Tick or untick every parsed host at once.
    SshImportSelectAll(bool),
    /// Save the ticked hosts and close the preview, emitting a toast.
    SshImportConfirm,
    /// Discard the SSH-config import preview without saving.
    SshImportDismiss,
    ImportFileLoaded(Vec<u8>),
    ImportPasswordChanged(super::Redacted),
    /// Decrypt the picked file with the entered password and reveal its
    /// per-category contents, the first step of the two-phase import.
    ImportInspect,
    /// Toggle one category checkbox in the import dialog (only the
    /// categories the file actually contains are interactive).
    ImportToggleCategory(oryxis_vault::ExportCategory),
    ImportConfirm,
    ImportCompleted(Result<String, String>),
    /// Destination chosen in the async Share save dialog; the handler
    /// encrypts the filtered export and writes it there.
    SharePathChosen(std::path::PathBuf),
    ExportImportDismiss,
    /// Open the SFTP backup-target picker from the export dialog. Routes
    /// the same encrypted blob to a remote host + path instead of a
    /// local file (validates the export password first).
    ExportToSftp,
    /// Open the SFTP picker to read an export blob back from a remote
    /// host + path, feeding the regular inspect/confirm import flow.
    ImportFromSftp,
    /// A host was selected in the SFTP backup picker (index into
    /// `connections`).
    SftpBackupHostSelected(usize),
    SftpBackupPathChanged(String),
    /// Connect to the chosen host and write (export) or read (import)
    /// the encrypted blob at the chosen remote path.
    SftpBackupConfirm,
    SftpBackupCancel,
    /// Result of an SFTP export: the human-readable status line.
    SftpBackupExportDone(Result<String, String>),
    /// Result of an SFTP import: the raw encrypted bytes, validated as an
    /// Oryxis export, ready to hand to the inspect/confirm flow.
    SftpBackupImportDone(Result<Vec<u8>, String>),
    ShareConnection(usize),
    /// Open the unified "Export hosts…" dialog. `Some(gid)` pre-ticks
    /// that folder (and its subfolders); `None` (root) pre-ticks every
    /// group plus the ungrouped hosts. The effective `Hosts(...)` filter
    /// is computed from the ticked folders on confirm.
    ShowExportHosts(Option<uuid::Uuid>),
    /// Toggle one folder's inclusion in the group-mode export.
    ShareToggleGroup(uuid::Uuid),
    /// Toggle inclusion of ungrouped (root) hosts in the export.
    ShareToggleUngrouped,
    SharePasswordChanged(super::Redacted),
    ShareToggleKeys,
    ShareConfirm,
    ShareDismiss,
}
