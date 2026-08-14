//! Sidebar Files tab (per-pane SFTP mini-browser) messages.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum SidebarFilesMessage {
    SidebarFilesNavigate(String),
    SidebarFilesRefresh,
    SidebarFilesToggleFollow,
    SidebarFilesToggleHidden,
    /// Promote the sidebar browser to a full SFTP tab at its current
    /// directory.
    SidebarFilesExpand,
    /// Initial mount finished: the backend (SFTP channel, or the local
    /// filesystem for a local shell), the session home (for
    /// `~`-relative cwd expansion) plus the first listing. The `u64` is
    /// the request stamp (`PaneFiles::req_seq`) captured at dispatch; a
    /// mismatch on arrival means a newer request (or a disconnect
    /// reset) superseded this one and it is dropped.
    SidebarFilesMounted(
        Uuid,
        u64,
        crate::local_files::FilesClient,
        Option<String>,
        String,
        Vec<oryxis_ssh::SftpEntry>,
    ),
    /// A navigation / follow / refresh listing landed (same stamp rule).
    SidebarFilesListed(Uuid, u64, String, Vec<oryxis_ssh::SftpEntry>),
    SidebarFilesError(Uuid, u64, String),
    SidebarFilesRowHovered(usize),
    SidebarFilesRowUnhovered(usize),
    /// Left-click on a row: single-click selects it (highlight); a quick
    /// double-click on a directory enters it.
    SidebarFilesSelectRow(String, bool),
    /// Right-click on a sidebar Files row: open its context menu
    /// (full path + is_dir), anchored at the cursor.
    ShowSidebarFilesRowMenu(String, bool),
    /// The header path is clickable (mirrors the SFTP pane's path
    /// editing): start / live-edit / commit typing a directory.
    SidebarFilesStartEditPath,
    SidebarFilesEditPath(String),
    SidebarFilesCommitPath,
    /// Path combo-box (issue #85): toggle / close the visited-directory
    /// dropdown, or jump straight to a picked entry.
    SidebarFilesPathHistoryToggle,
    SidebarFilesPathHistoryClose,
    SidebarFilesPathHistoryPick(String),
    /// A left click landed on the Files tab's dead space (no row, no
    /// input, no button captured it): blur = cancel any inline edit
    /// (path / rename / new entry) and close the history dropdown.
    SidebarFilesEditBlur,
    /// Open (or reveal) this tab's SFTP session at the given remote
    /// directory: the sidebar ⛶, the row context menu and the expand
    /// affordances all funnel here.
    SidebarFilesOpenSftpAt(String),
    /// Right-click on the list's empty area: directory-level menu
    /// (New file / New folder / Upload here / Refresh / Copy path).
    ShowSidebarFilesBackgroundMenu,
    /// Inline rename of a sidebar row: start (full path) / live input /
    /// commit. Esc via the sidebar router cancels.
    SidebarFilesStartRename(String),
    SidebarFilesRenameInput(String),
    SidebarFilesRenameCommit,
    /// Inline create (file or folder) at the top of the list.
    SidebarFilesStartNewEntry(crate::state::SftpEntryKind),
    SidebarFilesNewEntryInput(String),
    SidebarFilesNewEntryCommit,
    /// Delete an entry: ask (routes through the shared confirm dialog),
    /// then the confirmed op (recursive for directories).
    SidebarFilesDelete(String, bool),
    SidebarFilesDeleteConfirmed(String, bool),
    /// Download a file to a local destination picked via the OS dialog.
    SidebarFilesDownload(String),
    /// One-shot op finished (download / upload): toast the outcome.
    SidebarFilesOpToast(String),
    /// Upload local file(s) picked via the OS dialog into a directory.
    /// Only opens the dialog; a cancelled dialog ends the flow with no
    /// state touched (in particular no request-stamp bump, which would
    /// strand an in-flight listing's completion).
    SidebarFilesUploadInto(String),
    /// The upload dialog returned actual picks: run the uploads on the
    /// pane's channel. Payload: pane id, destination directory, local
    /// paths.
    SidebarFilesUploadPicked(Uuid, String, Vec<std::path::PathBuf>),
    /// Open the shared Properties (permissions) modal for a sidebar
    /// entry, chmod-ing through the sidebar's own client.
    SidebarFilesShowProperties(String, bool),
    /// Edit-in-place for a sidebar file (temp download + OS editor +
    /// auto-upload), through the sidebar's own client.
    SidebarFilesEdit(String),
}
