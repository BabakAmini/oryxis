//! Blocking-modal registry (capstone of the god-struct refactor).
//!
//! The app's blocking modals (pickers, editors, confirm dialogs) used to
//! be tracked as ~19 independent `show_*: bool` / `Option<_>` fields on
//! `Oryxis`, with two hand-maintained functions in `shortcuts.rs`
//! (`any_modal_blocks_input`, `close_topmost_modal`) that had to be edited
//! by hand for every new modal, a documented footgun: a forgotten entry
//! leaks keystrokes into the PTY behind the modal, or makes a modal
//! un-dismissable by Esc.
//!
//! This enum makes those two functions exhaustive `match`es the compiler
//! enforces. The per-modal `show_*` flag / `Option<_>` data field stays as
//! the single source of truth for "is this modal open" (so render sites
//! and the ~50 scattered open/close sites are unchanged); the enum is a
//! key into them. `Oryxis::is_modal_open` and `Oryxis::close_modal`
//! (`shortcuts.rs`) are `match`es over every variant, so a new modal
//! cannot compile without being handled. The only manual lists are
//! [`Modal::ALL`] and [`Modal::ESC_ORDER`]; a unit test guards `ALL`
//! against a forgotten variant.

/// One blocking modal. Each maps to a `show_*` / `Option<_>` field on
/// `Oryxis` via `is_modal_open` / `close_modal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Modal {
    NewTabPicker,
    TabJump,
    IconPicker,
    ThemePicker,
    ChainEditor,
    SessionGroupPanel,
    FolderRename,
    FolderDelete,
    /// Transient tab rename (terminal or SFTP tab, addressed by `TabRef`).
    TabRename,
    /// Careful-paste confirmation (multi-line clipboard paste parked in
    /// `pending_paste`, waiting for the user to confirm or cancel).
    CarefulPaste,
    /// Snippet-variables prompt (`{name}` placeholders parked in
    /// `pending_snippet_vars`, filled before the send).
    SnippetVars,
    /// Keyboard-interactive (2FA / OTP) prompt. Blocks input but owns its
    /// own dismissal, so it is intentionally absent from `ESC_ORDER`.
    KbiPrompt,
    /// Host-key verification prompt (`pending_host_key`) for a backgrounded
    /// connect (split pane / manual port forward / RDP launcher) with no
    /// connect-progress screen. A security prompt, so it MUST block input
    /// (Enter must never fall through to the PTY behind it) and Esc rejects
    /// the key (the safe default: never accept an unknown / changed key by
    /// a stray keystroke).
    HostKey,
    ThemeEditor,
    ThemeImport,
    UiThemeEditor,
    ShareDialog,
    CloudImportConfirm,
    /// Shared error / single-action confirm dialog (`error_dialog`),
    /// also the confirm step for known-host and session-log deletes.
    ErrorDialog,
    /// "Clear all history" confirmation.
    ClearHistoryConfirm,
    /// SSH-config import host-selection dialog.
    SshImport,
    SftpRename,
    SftpNewEntry,
    SftpProperties,
    SftpOverwrite,
    SftpPicker,
    /// The ssh-agent per-signature confirm prompt (`agent.pending_confirm`,
    /// B1). A blocking security prompt like `HostKey`: it MUST block input
    /// (Enter must never fall through to the PTY behind it) and Esc denies
    /// the signature (the safe default). In `ESC_ORDER` next to `HostKey`
    /// so the Esc router and the modal-keynav router both reach it.
    AgentConfirm,
    /// Read-only viewer for a key's attached OpenSSH certificate
    /// (`cert_viewer`, B2). Carries no secret (public cert material), so
    /// Esc simply closes it; it is in `ESC_ORDER` in the lightweight
    /// group next to the other dismissible info dialogs.
    CertificateViewer,
}

impl Modal {
    /// Every variant. Drives `any_modal_blocks_input`. Kept in sync with
    /// the enum by `tests::all_covers_every_variant`.
    pub(crate) const ALL: &'static [Modal] = &[
        Modal::NewTabPicker,
        Modal::TabJump,
        Modal::IconPicker,
        Modal::ThemePicker,
        Modal::ChainEditor,
        Modal::SessionGroupPanel,
        Modal::FolderRename,
        Modal::FolderDelete,
        Modal::TabRename,
        Modal::CarefulPaste,
        Modal::SnippetVars,
        Modal::KbiPrompt,
        Modal::HostKey,
        Modal::ThemeEditor,
        Modal::ThemeImport,
        Modal::UiThemeEditor,
        Modal::ShareDialog,
        Modal::CloudImportConfirm,
        Modal::ErrorDialog,
        Modal::ClearHistoryConfirm,
        Modal::SshImport,
        Modal::SftpRename,
        Modal::SftpNewEntry,
        Modal::SftpProperties,
        Modal::SftpOverwrite,
        Modal::SftpPicker,
        Modal::AgentConfirm,
        Modal::CertificateViewer,
    ];

    /// Modals Esc dismisses, in topmost-first priority order (the order
    /// `close_topmost_modal` walks). Modals absent here own their own
    /// dismissal and are not Esc-closeable: the kbi prompt and the SFTP
    /// rename / new-entry / properties / overwrite dialogs.
    pub(crate) const ESC_ORDER: &'static [Modal] = &[
        Modal::NewTabPicker,
        Modal::TabJump,
        Modal::IconPicker,
        Modal::ThemePicker,
        Modal::ChainEditor,
        Modal::FolderRename,
        Modal::FolderDelete,
        Modal::TabRename,
        Modal::CarefulPaste,
        Modal::SnippetVars,
        // A security prompt: Esc rejects the host key (safe default).
        Modal::HostKey,
        // Sibling security prompt: Esc denies the signature (safe default).
        Modal::AgentConfirm,
        // The error dialog can pop over another flow, so it dismisses
        // before the heavier editors below; the two confirm dialogs
        // follow in the same lightweight-confirm group.
        Modal::ErrorDialog,
        Modal::ClearHistoryConfirm,
        Modal::SshImport,
        Modal::SessionGroupPanel,
        Modal::ThemeEditor,
        Modal::UiThemeEditor,
        Modal::ThemeImport,
        Modal::ShareDialog,
        Modal::CloudImportConfirm,
        Modal::SftpPicker,
        // Read-only info dialog; Esc just closes it.
        Modal::CertificateViewer,
    ];

    /// Whether this modal captures keyboard input, so keystrokes must not
    /// fall through to the terminal behind it. Every current modal does;
    /// the method exists so a future non-capturing overlay is a compiler-
    /// visible decision, not a silent omission.
    pub(crate) fn blocks_input(self) -> bool {
        match self {
            Modal::NewTabPicker
            | Modal::TabJump
            | Modal::IconPicker
            | Modal::ThemePicker
            | Modal::ChainEditor
            | Modal::SessionGroupPanel
            | Modal::FolderRename
            | Modal::FolderDelete
            | Modal::TabRename
            | Modal::CarefulPaste
            | Modal::SnippetVars
            | Modal::KbiPrompt
            | Modal::HostKey
            | Modal::ThemeEditor
            | Modal::ThemeImport
            | Modal::UiThemeEditor
            | Modal::ShareDialog
            | Modal::CloudImportConfirm
            | Modal::ErrorDialog
            | Modal::ClearHistoryConfirm
            | Modal::SshImport
            | Modal::SftpRename
            | Modal::SftpNewEntry
            | Modal::SftpProperties
            | Modal::SftpOverwrite
            | Modal::SftpPicker
            | Modal::AgentConfirm
            | Modal::CertificateViewer => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Modal;

    #[test]
    fn all_covers_every_variant() {
        // The exhaustive match means a new variant fails to compile here
        // until it is named; the assert then forces it into `ALL` too.
        for &m in Modal::ALL {
            match m {
                Modal::NewTabPicker
                | Modal::TabJump
                | Modal::IconPicker
                | Modal::ThemePicker
                | Modal::ChainEditor
                | Modal::SessionGroupPanel
                | Modal::FolderRename
                | Modal::FolderDelete
                | Modal::TabRename
                | Modal::CarefulPaste
                | Modal::SnippetVars
                | Modal::KbiPrompt
                | Modal::HostKey
                | Modal::ThemeEditor
                | Modal::ThemeImport
                | Modal::UiThemeEditor
                | Modal::ShareDialog
                | Modal::CloudImportConfirm
                | Modal::ErrorDialog
                | Modal::ClearHistoryConfirm
                | Modal::SshImport
                | Modal::SftpRename
                | Modal::SftpNewEntry
                | Modal::SftpProperties
                | Modal::SftpOverwrite
                | Modal::SftpPicker
                | Modal::AgentConfirm
                | Modal::CertificateViewer => {}
            }
        }
        assert_eq!(Modal::ALL.len(), 28, "add the new variant to Modal::ALL");
        // Every Esc-closeable modal must also be a known modal.
        for m in Modal::ESC_ORDER {
            assert!(Modal::ALL.contains(m));
        }
    }
}
