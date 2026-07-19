//! The full `Message` enum, every event the iced runtime can dispatch
//! to `Oryxis::update`. Pulled out of `app.rs` so the message-loop file
//! is shorter; re-exported via `pub use` at the bottom of `app.rs` so
//! call sites continue to write `crate::app::Message::Foo`.

use std::sync::Arc;

use iced::widget::text_editor;
use uuid::Uuid;

use oryxis_ssh::SshSession;

use crate::state::SettingsSection;

mod ai;
pub use ai::AiMessage;
mod onboarding;
pub use onboarding::OnboardingMessage;
mod tabs;
pub use tabs::TabsMessage;
mod editor;
pub use editor::EditorMessage;
mod keys;
pub use keys::KeysMessage;
mod sidebar_files;
pub use sidebar_files::SidebarFilesMessage;
mod terminal;
pub use terminal::TerminalMessage;
mod ssh;
pub use ssh::SshMessage;
mod cloud;
pub use cloud::CloudMessage;
mod history;
pub use history::HistoryMessage;
mod mcp;
pub use mcp::McpMessage;
mod navigation;
pub use navigation::NavigationMessage;
mod command_history;
pub use command_history::CommandHistoryMessage;
mod update;
pub use update::UpdateMessage;
mod proxy_identity;
pub use proxy_identity::ProxyIdentityMessage;
mod plugin;
pub use plugin::PluginMessage;
mod agent;
pub use agent::AgentMessage;
mod zmodem;
pub use zmodem::ZmodemMessage;
mod known_host;
pub use known_host::KnownHostMessage;
mod remote_desktop;
pub use remote_desktop::RemoteDesktopMessage;
mod tray;
pub use tray::TrayMessage;
mod player;
pub use player::PlayerMessage;
mod vault;
pub use vault::VaultMessage;
mod session_group;
pub use session_group::SessionGroupMessage;
mod port_forward;
pub use port_forward::PortForwardMessage;
mod snippet;
pub use snippet::SnippetMessage;
mod share;
pub use share::ShareMessage;
mod sync;
pub use sync::SyncMessage;

/// The four per-class Privacy Mode gates (issue #78 block 1), each
/// mirroring a `privacy_mask_*` setting. The usernames class covers
/// both the shape heuristics (`user@host`, home dirs) and the
/// saved-connection usernames inside the terms list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyMaskClass {
    PublicIps,
    PrivateIps,
    Usernames,
    Hostnames,
}

/// SFTP async-completion messages that ride the `SftpFor` owner-routing
/// envelope (`route_sftp_async`). Grouped into their own enum so
/// `SftpFor` can carry `Box<SftpMessage>` instead of `Box<Message>`,
/// making it a compile error to route a non-SFTP message through the
/// buffer-owner swap path. This is the first `Message` sub-enum (the
/// pilot for splitting the god-enum); new message-heavy areas should be
/// born as their own sub-enum rather than flat `Message` variants.
///
/// Reached through [`Message::Sftp`]: the dispatcher unwraps the
/// envelope in `route_sftp_async` and re-dispatches as `Message::Sftp`,
/// which the SFTP handler chain matches.
#[derive(Debug, Clone)]
pub enum SftpMessage {
    /// Initial mount finished: the live session + SFTP channel, the
    /// session home and the first listing for the picked pane.
    HostMounted(
        crate::state::SftpPaneSide,
        String,
        Arc<SshSession>,
        oryxis_ssh::SftpClient,
        String,
        Vec<oryxis_ssh::SftpEntry>,
    ),
    /// A remote pane operation (mount / listing) failed; `SftpPaneSide`
    /// names the pane whose error banner shows the message.
    RemoteError(crate::state::SftpPaneSide, String),
    /// Central directory parsed (archive real path, mount token
    /// captured at spawn, payload or error). A token that no longer
    /// matches the pane means the pane was remounted (or switched back
    /// to Local) while the index was read: the result is dropped.
    ZipIndexed(
        crate::state::SftpPaneSide,
        String,
        crate::state::ArchiveOpToken,
        Result<crate::state::ZipIndexedPayload, String>,
    ),
    /// Archive operation finished: log label or error. The payload
    /// carries which pane the op changed (refresh / error target) and
    /// which pane it marked busy, each with the mount token captured at
    /// spawn, so completions clear / apply exactly what this op touched
    /// and stale (post-remount) results are dropped.
    ArchiveDone(crate::state::ArchiveDone),
}

#[derive(Debug, Clone)]
pub enum Message {
    // Vault
    // Vault lock / password / biometric (handle_vault)
    Vault(VaultMessage),

    // First-run welcome / onboarding carousel (rendered off
    // `VaultState::NeedSetup`).
    Onboarding(OnboardingMessage),

    // Navigation
    // Navigation (handle_navigation)
    Navigation(NavigationMessage),

    // Tabs
    // Tabs (handle_tabs)
    Tabs(TabsMessage),
    // ── Command palette (C4) ────────────────────────────────────────
    // Absorb-click sink, used by modal bodies to stop clicks from falling
    // through to the backdrop underneath. Handler is a no-op.
    NoOp,

    // Icon picker (custom host icon/color)
    // Per-host terminal theme picker (modal opened from the host
    // editor). The form field updates immediately on select; the
    // change is committed on EditorSave like every other form field.
    // Editor (handle_editor)
    Editor(EditorMessage),
    // ── C5 per-host legacy keyboard modes + feature toggles ──────────

    // SFTP browser. Most pane operations are side-addressed: the
    // `SftpPaneSide` says *which* pane (Left / Right), and the handler
    // branches on that pane's `is_remote` flag to pick filesystem vs
    // SFTP behaviour.
    /// Wrapper for the SFTP async-completion sub-enum ([`SftpMessage`]).
    /// The owner-routing envelope (`SftpFor`) carries these, and
    /// `route_sftp_async` re-dispatches unowned ones through here.
    Sftp(SftpMessage),
    SftpPickHost(usize),
    SftpRemoteLoaded(crate::state::SftpPaneSide, u64, String, Vec<oryxis_ssh::SftpEntry>),
    /// Owner-routing envelope for SFTP async completions whose payload has no
    /// owner stamp of its own (the mount pipeline: `SftpMessage::HostMounted` /
    /// `SftpMessage::RemoteError`). Carries the id of the tab (standalone SFTP tab or
    /// hybrid terminal tab) that owned the live buffer at kickoff time, so
    /// `route_sftp_async` swaps that owner's state in before the inner
    /// message runs, or drops it when the owner is gone. Without it, a
    /// park/hoist swap between kickoff and completion would land the result
    /// in whichever buffer happens to be live. Built via `Message::sftp_owned`.
    SftpFor(Uuid, Box<SftpMessage>),
    /// Navigate a *remote* pane to a POSIX path.
    SftpNavigateRemote(crate::state::SftpPaneSide, String),
    /// Navigate a *local* pane to a filesystem path.
    SftpNavigateLocal(crate::state::SftpPaneSide, std::path::PathBuf),
    /// Go up one directory in the given pane (local or remote).
    SftpUp(crate::state::SftpPaneSide),
    /// Refresh a local pane's listing from its current path.
    SftpRefreshLocal(crate::state::SftpPaneSide),
    /// Open the host picker, choosing for the given pane.
    SftpOpenPicker(crate::state::SftpPaneSide),
    /// Pick "Local" for the left pane (only offered there).
    SftpPickLocal,
    SftpClosePicker,
    /// Focus the SFTP tab at this `sftp_tabs` index (swap its state into the
    /// active buffer and switch the surface to it).
    SelectSftpTab(usize),
    /// Close the SFTP tab at this index. Guards against an in-flight transfer
    /// / unsaved edit-session via a confirmation modal.
    CloseSftpTab(usize),
    /// Open a fresh, empty SFTP tab (host picker) and focus it.
    NewSftpTab,
    /// Proceed with closing the SFTP tab pending confirmation (after the
    /// in-flight-transfer / unsaved-edit warning).
    ConfirmCloseSftpTab,
    /// Dismiss the SFTP close-guard modal without closing.
    CancelCloseSftpTab,
    /// Toggle the pinned state of the SFTP tab at this index.
    ToggleSftpTabPin(usize),
    /// Open the right-click context menu for the SFTP tab at this index.
    ShowSftpTabMenu(usize),
    /// Close every SFTP tab except the one at this index.
    CloseOtherSftpTabs(usize),
    /// Mount connection `usize` into a specific pane side (regardless of the
    /// picker target). Used to re-mount a restored pinned SFTP tab's pane(s).
    SftpRemountPane(crate::state::SftpPaneSide, usize),
    /// Cursor entered the SFTP tab at this index (hover + live-slide target).
    SftpTabHovered(usize),
    /// Cursor left the SFTP tab strip.
    SftpTabUnhovered,
    SftpPickerSearch(String),
    SftpToggleHidden(crate::state::SftpPaneSide),
    SftpFilter(crate::state::SftpPaneSide, String),
    SftpToggleActions(crate::state::SftpPaneSide),
    SftpToggleDrives(crate::state::SftpPaneSide),
    SftpCloseMenus,
    /// Toggle visibility of an optional file-list column (Size / Modified /
    /// Type / Permissions / Owner) for one pane. Per-pane; also updates the
    /// persisted template.
    SftpToggleColumn(crate::state::SftpPaneSide, crate::state::SftpColumn),
    /// Begin dragging a column's right-edge resize handle.
    SftpColResizeStart(crate::state::SftpPaneSide, crate::state::SftpColumn),
    /// Double-click a column's resize handle: auto-fit the column to the
    /// widest value across every row (visible or not).
    SftpColAutoFit(crate::state::SftpPaneSide, crate::state::SftpColumn),
    /// Press on a column header: arms a reorder drag (promoted to active on
    /// move; a release without movement falls through to the sort click).
    SftpColDragStart(crate::state::SftpPaneSide, crate::state::SftpColumn),
    /// Cursor entered / left a column header (reorder drop target).
    SftpColHovered(crate::state::SftpPaneSide, crate::state::SftpColumn),
    SftpColUnhovered,
    /// Toggle this pane's collapsed filter popover (narrow layout).
    SftpToggleFilterSearch(crate::state::SftpPaneSide),
    /// Toggle the FileZilla-style message-log panel at the bottom of the view.
    SftpToggleLog,
    /// Begin dragging the horizontal divider above the message-log panel to
    /// resize its height.
    SftpLogResizeStart,
    /// Begin dragging the center divider between the two SFTP panes.
    SftpSplitResizeStart,
    /// Open a new SFTP tab mounted on the saved connection at this index
    /// (host-card context menu). Reuses a live SSH session if one is open,
    /// otherwise connects.
    OpenSftpForConnection(usize),
    SftpStartEditPath(crate::state::SftpPaneSide),
    SftpEditPath(crate::state::SftpPaneSide, String),
    SftpCommitPath(crate::state::SftpPaneSide),
    #[allow(dead_code)] // wired by upcoming Esc handler
    SftpCancelEditPath,
    SftpSort(crate::state::SftpPaneSide, crate::state::SftpSortColumn),

    // Row interactions
    SftpRowRightClick(crate::state::SftpPaneSide, String, bool),
    /// Right-click on the empty area of a pane (not a row). Opens the
    /// directory-level context menu anchored at the cursor.
    SftpBackgroundRightClick(crate::state::SftpPaneSide),
    SftpRowMenuClose,
    /// Copy a full path (row entry or the pane's current directory) to
    /// the clipboard. The string arrives already side-formatted (POSIX
    /// for remote entries, OS-native for local ones).
    SftpCopyPath(String),

    // Sidebar Files tab (the per-pane SFTP browser next to Chat /
    // Snippets / History). Navigation targets the ACTIVE pane; async
    // results carry the pane's stable `Uuid` so a pane/tab switch
    // mid-flight can't land a listing on the wrong browser.
    // SidebarFiles (handle_sidebar_files)
    SidebarFiles(SidebarFilesMessage),
    /// Copy every selected path in the given pane, one per line.
    SftpCopySelectionPaths(crate::state::SftpPaneSide),
    SftpStartRename(crate::state::SftpPaneSide, String),
    SftpRenameInput(String),
    SftpRenameCommit,
    /// A remote rename succeeded: `(side, dir to reload, new basename)`.
    /// Logs the rename, then re-lists the directory.
    SftpRenamed(crate::state::SftpPaneSide, String, String),
    SftpAskDelete(crate::state::SftpPaneSide, String, bool),
    SftpAskDeleteSelection,
    SftpConfirmDelete,
    SftpCancelDelete,
    /// Remote delete succeeded: drop these (full) paths from the given
    /// pane's listing in place, no re-list.
    SftpEntriesRemoved(crate::state::SftpPaneSide, Vec<String>),
    /// Toggle the per-file progress panel that drops down from the
    /// transfer status strip.
    SftpToggleTransferPanel,
    /// Periodic tick while a transfer runs: forces a redraw so the live
    /// byte-progress bar advances (it reads a shared atomic counter).
    SftpTransferTick,
    /// Deferred type-ahead search fire. Carries the generation it was
    /// scheduled for; runs only if no newer keystroke superseded it
    /// (debounce, so fast typing searches once with the full buffer).
    SftpTypeAheadFire(u64),
    /// A pane's file list scrolled: carries the side, the new absolute
    /// vertical offset (px) and the visible viewport height (px). Stored so
    /// keyboard navigation only scrolls when the cursor reaches an edge.
    SftpListScrolled(crate::state::SftpPaneSide, f32, f32),
    SftpStartNewEntry(crate::state::SftpPaneSide, crate::state::SftpEntryKind),
    SftpNewEntryInput(String),
    SftpNewEntryCommit,
    SftpNewEntryCancel,
    SftpUpload(std::path::PathBuf),
    SftpDownload(String),
    SftpDuplicate(crate::state::SftpPaneSide, String),
    SftpFileHovered,
    SftpFilesHoveredLeft,
    SftpFileDropped(std::path::PathBuf),
    SftpDropFlush,
    /// Async local-directory listing landed (side, pane listing seq,
    /// listed path, rows or error). Emitted by `spawn_local_listing`;
    /// stale seqs are dropped.
    SftpLocalListed(
        crate::state::SftpPaneSide,
        u64,
        std::path::PathBuf,
        Result<Vec<crate::state::LocalEntry>, String>,
    ),
    SftpRowEnter(crate::state::SftpPaneSide, String, bool),
    SftpRowExit,
    SftpMouseLeftPressed,
    SftpUploadFolder(std::path::PathBuf),
    SftpDownloadFolder(String),
    SftpDuplicateFolder(crate::state::SftpPaneSide, String),
    SftpSelectRow(crate::state::SftpPaneSide, String, bool),
    SftpStartEdit(String),
    /// Open a local file in the OS default app, no temp copy, no
    /// mtime watch. Edits land on the file directly.
    SftpOpenLocal(std::path::PathBuf),
    /// Reveal a local file/folder in the OS file manager (local pane
    /// only). Folders open in place; files open their folder selected.
    /// Carries the absolute path and whether it's a directory.
    SftpRevealInExplorer(std::path::PathBuf, bool),
    /// Open an arbitrary URL in the user's default browser.
    /// Used by clickable links in the About panel.
    OpenUrl(String),
    /// Copy a string to the system clipboard. Used by the Copy
    /// affordance on chat bubbles and code blocks (text-selection
    /// isn't supported by iced's `text` / markdown widgets in 0.14).
    CopyToClipboard(String),
    /// Dismiss the transient toast chip (`Oryxis.toast`). Fired by a
    /// `Task::perform` sleep scheduled when a toast is shown.
    /// Deadline-guarded clear: clears the toast only if `toast_deadline`
    /// has passed. Fired by the `ToastTick`-style subscription and by any
    /// legacy scheduled sleep-timer, so a superseded timer can never wipe
    /// a newer toast.
    ToastClear,
    /// Immediate dismissal (clicking the chip), regardless of deadline.
    ToastDismiss,
    /// Dismiss the blocking error dialog (`Oryxis.error_dialog`). Fired
    /// by the OK button or by clicking the scrim.
    ErrorDialogDismiss,
    /// Fire the dialog's optional recovery action: dismisses the
    /// dialog and dispatches the message it carries.
    ErrorDialogRunAction,
    SftpEditReady(crate::state::EditSession),
    SftpEditSave,
    SftpEditDiscard,
    SftpEditWatchTick,
    SftpCancelRemoteLoad(crate::state::SftpPaneSide),
    /// Retry the last failed remote action, either re-list the
    /// current path (if a session is still mounted) or re-run the
    /// full host-pick flow (if the connect itself failed).
    SftpRetryRemote(crate::state::SftpPaneSide),
    SftpShowProperties(crate::state::SftpPaneSide, String, bool),
    SftpPropertiesLoaded(crate::state::PropertiesView),
    SftpPropertiesToggleBit(crate::state::PermBit),
    SftpPropertiesModeInput(String),
    SftpPropertiesApply,
    SftpPropertiesDone(Result<(), String>),
    SftpPropertiesClose,
    SftpAskOverwrite(crate::state::OverwritePrompt),
    SftpResolveOverwrite(crate::state::OverwriteAction),
    SftpToggleApplyToAll,
    SftpUploadBatch(Vec<std::path::PathBuf>),
    SftpUploadSelection,
    SftpDownloadSelection,
    SftpDuplicateSelection,
    // Archive operations (extract / compress / virtual zip browse).
    // Async completions ride the `SftpFor` owner envelope like the
    // transfer queue does.
    /// Open a zip archive (real full path) for virtual browsing.
    SftpZipOpen(crate::state::SftpPaneSide, String),
    /// Navigate to a directory INSIDE the browsed archive ("" = root).
    SftpZipNavigate(crate::state::SftpPaneSide, String),
    /// Leave virtual browsing, restoring the pane's real directory.
    SftpZipClose(crate::state::SftpPaneSide),
    /// Copy an entry (inner path, is_dir) out of the browsed archive
    /// into the OTHER pane's current directory.
    SftpZipCopyOut(crate::state::SftpPaneSide, String, bool),
    /// Extract an archive (real full path) next to itself.
    SftpArchiveExtract(crate::state::SftpPaneSide, String),
    /// Compress the clicked path (or the selection containing it) into
    /// an archive of the given kind, in the pane's current directory.
    SftpArchiveCompress(
        crate::state::SftpPaneSide,
        oryxis_archive::names::ArchiveKind,
        String,
    ),
    /// Archive operation finished: log label or error. The payload
    /// Once-per-mount remote tool probe result. The token is the mount
    /// generation the probe was spawned for; a stale one is dropped.
    SftpToolsProbed(
        crate::state::SftpPaneSide,
        crate::state::ArchiveOpToken,
        oryxis_archive::remote::RemoteShell,
        oryxis_archive::remote::ArchiveTools,
    ),
    // The leading `Uuid` on the transfer-queue continuation messages is the
    // owning SFTP tab. These arrive after async work, by which point the user
    // may have focused another SFTP tab; the dispatcher swaps the owning tab's
    // state into `self.sftp` for the duration so the handler routes to the
    // right tab. See `Message::sftp_async_owner` + `route_sftp_async`.
    SftpTransferConflict(Uuid, crate::state::OverwritePrompt, crate::state::TransferItem, u8),
    SftpTransferQueueReady(Uuid, crate::state::TransferState),
    /// Pop one item and dispatch to whichever slot is free. The Next
    /// handler picks the slot itself instead of carrying it in the
    /// message, that way pause/resume can spawn fresh chains without
    /// having to remember which slot was on which client. The `Uuid` is the
    /// owning SFTP tab.
    SftpTransferNext(Uuid),
    /// Slot freed up after a queue item completed successfully.
    SftpTransferItemDone(Uuid, u8),
    SftpTransferError(Uuid, String, u8),
    SftpCancelTransfer,
    /// Operation result for a remote pane. `SftpPaneSide` names the pane
    /// whose error banner should show the message on failure.
    SftpOpResult(crate::state::SftpPaneSide, String, bool),
    /// Relay a single remote file from the `from` side's host to the
    /// other side's host (server-to-server). `from` is the source pane.
    SftpRelay(crate::state::SftpPaneSide, String),
    /// Relay a remote folder tree from the `from` side's host to the
    /// other side's host.
    SftpRelayFolder(crate::state::SftpPaneSide, String),

    // Folder (group) actions

    // Terminal I/O
    // Terminal (handle_terminal)
    Terminal(TerminalMessage),
    // Zmodem (handle_zmodem)
    Zmodem(ZmodemMessage),
    // Cloud (handle_cloud)
    Cloud(CloudMessage),
    /// Settings → Shortcuts: enter capture mode for an action. The
    /// next non-Esc, non-pure-modifier `KeyPressed` becomes the new
    /// binding (see `shortcuts::handle_hotkey_capture`).
    StartEditingHotkey(crate::hotkeys::HotkeyAction, crate::hotkeys::HotkeySlot),
    /// Settings → Shortcuts: drop a single action's user override and
    /// fall back to the factory default.
    ResetHotkey(crate::hotkeys::HotkeyAction),
    /// Settings → Shortcuts: drop every user override.
    ResetAllHotkeys,

    // Overlay

    // Card interactions
    // CommandHistory (handle_command_history)
    CommandHistory(CommandHistoryMessage),

    // Connection editor
    // Serial line params (reduced Serial form). Each carries the typed
    // value; the handler materializes `SerialParams` defaults first.
    // Remote desktop (RDP/VNC) editor rows: kind picker + the SSH host
    // to tunnel through (`None` = direct). The desktop endpoint + login
    // reuse the normal hostname/port/username/password fields.
    // Chain editor (Termius-style multi-hop jump-host editor). Opens
    // from the "Host Chaining" row in the host editor; edits the
    // ordered `editor_form.jump_chain`.

    // Session groups (saved split-panel arrangements)
    // Session groups (handle_session_group)
    SessionGroup(SessionGroupMessage),

    // SSH
    // Ssh (handle_ssh)
    Ssh(SshMessage),

    // Snippets
    // Snippets (handle_snippets)
    Snippet(SnippetMessage),
    /// Settings > Terminal: toggle the paste content heuristics.
    TogglePasteGuard,

    // Command history (terminal sidebar History tab)
    /// Settings > Terminal: enable/disable command-history capture.
    ToggleCommandHistory,

    // Split panes

    // Custom terminal themes (Settings -> Themes)
    /// Open the editor for a brand new custom theme.
    ThemeEditorNew,
    /// Open the editor for the custom theme at this index.
    ThemeEditorEdit(usize),
    /// Close the editor without saving.
    ThemeEditorClose,
    ThemeEditorNameChanged(String),
    /// A color slot's hex value changed (live).
    ThemeEditorColorChanged(crate::state::ThemeColorSlot, String),
    /// Save the in-progress theme (insert or update) + repaint.
    ThemeEditorSave,
    /// Delete the custom theme at this index.
    ThemeDelete(usize),
    /// Import-theme modal (paste an iTerm / Windows Terminal / base16 scheme).
    ThemeImportOpen,
    ThemeImportClose,
    ThemeImportContentAction(text_editor::Action),
    ThemeImportNameChanged(String),
    /// Parse the pasted scheme; on success open it in the editor for review.
    ThemeImportApply,

    // Custom UI (chrome) themes (Settings -> Interface). `usize` is the
    // color-field index into `theme::UI_COLOR_FIELDS`.
    UiThemeEditorNew,
    UiThemeEditorEdit(usize),
    UiThemeEditorClose,
    UiThemeEditorNameChanged(String),
    UiThemeColorChanged(usize, String),
    UiThemeEditorOpenPicker(usize),
    UiThemeEditorClosePicker,
    UiThemeEditorSave,
    UiThemeDelete(usize),
    UiThemeCardHovered(usize),
    UiThemeCardUnhovered,
    /// Hover tracking for the floating edit / delete icons on a custom
    /// theme card.
    ThemeCardHovered(usize),
    ThemeCardUnhovered,
    /// Open the compact color-picker popover for a slot (anchored at the
    /// cursor).
    ThemeEditorOpenPicker(crate::state::ThemeColorSlot),
    /// Close the color-picker popover.
    ThemeEditorClosePicker,

    // Port forwards (standalone entity)
    // Port forwards (handle_port_forwards)
    PortForward(PortForwardMessage),
    // ProxyIdentity (handle_proxy_identity)
    ProxyIdentity(ProxyIdentityMessage),

    // Terminal side panel (Chat / Snippets / Host config tabs)
    // AI settings + chat sidebar (handle_ai)
    Ai(AiMessage),
    /// Local/ephemeral panes have no saved host: pick a session-only theme
    /// for the open local terminals, or promote it to the global default.
    LocalConfigThemeChanged(String),
    LocalConfigSaveGlobal,

    // Known hosts
    // KnownHost (handle_known_host)
    KnownHost(KnownHostMessage),

    // History
    // History (handle_history)
    History(HistoryMessage),

    // Session logs
    /// In-app session player (issue #71): a read-only playback surface
    /// on the History view.
    Player(PlayerMessage),
    // History was split in v0.6 (logs + session logs in two panes
    // with independent pagination); v0.7 merges both into one timeline
    // so the per-section Clear / Next / Prev controls don't render
    // anymore. Handlers stay wired so we can resurrect a dedicated
    // session-logs surface without re-introducing the messages.

    // Settings
    TerminalThemeChanged(String),
    /// Retention code picked in Settings ("off" / "1d" / ... / "90d");
    /// persists and prunes immediately.
    LogsRetentionChanged(&'static str),
    AppThemeChanged(String),
    TerminalFontSizeIncrease,
    TerminalFontSizeDecrease,
    TerminalFontChanged(String),
    /// The user ctrl-clicked a link in the terminal: the gesture landed,
    /// so under `HintMode::Once` retire the link toast for the focused pane.
    TerminalLinkOpened,
    /// Settings: terminal hint mode picker changed. Carries the localized
    /// option label; the dispatch handler maps it back to a `HintMode`.
    HintModeChanged(String),
    /// Flip the reveal/eye state of a secret input field.
    ToggleSecretVisibility(crate::state::SecretField),
    // Update (handle_update)
    Update(UpdateMessage),
    ChangeSettingsSection(SettingsSection),
    /// Pick the renderer backend ("auto" / "opengl" / "software").
    /// Persisted to the vault; takes effect on the next launch (the
    /// backend is fixed at startup via WGPU_BACKEND / ICED_BACKEND).
    SettingRendererBackendChanged(String),
    /// Resolved graphics backend + adapter from the compositor, queried
    /// when the Interface settings section opens. `(backend, adapter)`.
    RendererInfoLoaded(String, String),
    ToggleCopyOnSelect,
    ToggleRightClickCopy,
    ToggleMiddleClickPaste,
    ToggleSftpForceOsc7,
    /// PuTTY "reset scrollback on keypress" toggled in Settings > Terminal.
    ToggleScrollbackResetKeypress,
    /// PuTTY "reset scrollback on display activity" toggled in Settings.
    ToggleScrollbackResetOutput,
    /// Right-click scheme changed from the settings pick (localized
    /// "Context menu / Paste / Extend selection" label).
    TerminalRightClickChanged(String),
    /// Flip the careful-paste guard (warn before multi-line paste).
    ToggleCarefulPaste,
    ToggleBoldIsBright,
    /// Toggle showing the shell-set window title (OSC 0/2) in the tab strip.
    ToggleTerminalAutoTitle,
    /// Terminal bell behavior changed from the settings pick (localized
    /// "Off / Flash / Beep" label).
    BellModeChanged(String),
    /// OSC 52 clipboard access policy changed from the settings pick
    /// (localized "Off / Write only / Read & write" label).
    ClipboardAccessChanged(String),
    /// OSC 9 notification surfacing changed from the settings pick
    /// (localized "Off / Toast / OS" label).
    NotificationModeChanged(String),
    /// Smart tabs (attention dots + long-command / activity
    /// notifications) toggled in Settings > Terminal.
    SettingToggleSmartTabs,
    /// Smart-tabs long-command threshold changed from the settings pick
    /// (display label; resolved via `smart_tabs::threshold_options`).
    SmartTabsThresholdChanged(String),
    ToggleKeywordHighlight,
    ToggleSmartContrast,
    SettingToggleShowStatusBar,
    /// Flip the host dashboard between the responsive card grid and a
    /// single-column list.
    ToggleHostListView,
    /// Flip the per-colour accent wash on dashboard cards (glass vs pure).
    ToggleCardAccentGlass,
    /// Flip showing of the `user@host:port` address on host cards.
    ToggleShowHostAddress,
    /// Flip the global Privacy Mode default (auto-hide sensitive data).
    TogglePrivacyMode,
    /// Privacy Mode session override (issue #78): press once to force
    /// the opposite of the configured global state (above per-host
    /// overrides too), press again to fall back to the settings.
    /// Volatile, never persisted. Driven by the Ctrl+Shift+M hotkey
    /// and the status-bar chip.
    TogglePrivacySessionOverride,
    /// Privacy Mode always-mask list edited (issue #78): literals
    /// masked wherever they appear, on top of the derived terms.
    SettingPrivacyAlwaysMaskChanged(String),
    /// Privacy Mode never-mask list edited (issue #78): words the
    /// derived terms must not include (generic usernames).
    SettingPrivacyNeverMaskChanged(String),
    /// Flip one per-class Privacy Mode gate (issue #78 block 1).
    TogglePrivacyMaskClass(PrivacyMaskClass),
    /// Flip the Settings > Advanced debug logging (tracing events also
    /// written to the exportable `~/.oryxis/oryxis-debug.log`).
    SettingToggleDebugLogging,
    /// Settings > Advanced: download-mirror picker changed
    /// ("auto" / "github" / "custom").
    DownloadMirrorPicked(String),
    /// Custom mirror URL field edited (live value).
    DownloadMirrorUrlEdited(String),
    /// Custom mirror URL committed (Enter / Save): validate + persist.
    DownloadMirrorUrlCommitted,
    /// Run the mirror reachability probe against the entered URL.
    DownloadMirrorTest,
    /// Probe outcome: latency in ms, or the failure cause.
    DownloadMirrorTestResult(Result<u64, String>),
    /// Reveal the debug log file in the OS file manager (falls back to
    /// the `~/.oryxis` folder while no log file exists yet).
    RevealDebugLog,
    /// Wipe the debug log file (truncated in place while logging is on,
    /// deleted otherwise).
    ClearDebugLog,
    /// Toggle the Logs view Privacy Mode reveal (show raw sensitive data
    /// in the timeline + session-log viewer until toggled back).
    TogglePrivacyReveal,
    SettingToggleCloseToTray,
    SettingToggleMinimizeToTray,
    SettingToggleTabAccentLine,
    SettingToggleTabAccentWash,
    SettingToggleTabAccentText,
    SettingTogglePerformanceMode,
    SettingTogglePerfOverlay,
    /// Toggle the opt-in "remote desktop" feature (`remote_desktop_enabled`).
    SettingToggleRemoteDesktop,
    /// Relaunch the app in place to apply a start-time-only setting (the
    /// graphics renderer). Fired from the renderer-change restart modal.
    RelaunchApp,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Tray(TrayMessage),
    SettingTabCloseButtonSideChanged(String),
    SettingPinnedTabStyleChanged(String),
    SettingTabFillStyleChanged(String),
    SettingTabAccentColorChanged(String),
    /// Dock the tab strip at the top (default) or the bottom of the
    /// window ("top" / "bottom"). The window chrome (burger, drag area,
    /// minimize / maximize / close) stays in a slim top bar either way.
    SettingTabBarPositionChanged(String),
    SettingToggleShowTabStatusDot,
    SettingToggleSftpEnabled,
    SettingNavOrientationChanged(String),
    /// Expand/collapse the vertical nav rail (labels vs icon-only).
    ToggleNavRailExpanded,
    SettingDefaultHostIconChanged(String),
    SettingKeepaliveChanged(String),
    /// New-connection defaults (pre-filled into a fresh host form).
    ToggleDefaultAgentForwarding,
    DefaultPortChanged(String),
    DefaultKeepaliveChanged(String),
    DefaultTerminalTypeChanged(String),
    /// Extended new-connection defaults (the default host profile).
    DefaultUsernameChanged(String),
    DefaultAuthMethodChanged(String),
    DefaultIdentityChanged(String),
    DefaultKeyChanged(String),
    DefaultGroupChanged(String),
    DefaultProxyChanged(String),
    ToggleDefaultMcpEnabled,
    DefaultEncodingChanged(String),
    DefaultAddEnvVar,
    DefaultRemoveEnvVar(usize),
    DefaultEnvVarKeyChanged(usize, String),
    DefaultEnvVarValueChanged(usize, String),
    /// Collapse / expand the "New connection defaults" card.
    ToggleDefaultsCollapsed,
    SettingScrollbackChanged(String),
    SettingWordDelimitersChanged(String),
    SettingResetWordDelimiters,
    SettingSftpConcurrencyChanged(String),
    SettingSftpConnectTimeoutChanged(String),
    SettingSftpAuthTimeoutChanged(String),
    SettingSftpSessionTimeoutChanged(String),
    SettingSftpOpTimeoutChanged(String),
    SettingToggleAutoReconnect,
    SettingMaxReconnectChanged(String),
    /// Vault auto-lock idle threshold, minutes as typed ("0" = off).
    SettingAutoLockChanged(String),
    /// Periodic idle check while the vault is unlocked and auto-lock is
    /// enabled; locks when the idle threshold is crossed.
    AutoLockTick,
    // RemoteDesktop (handle_remote_desktop)
    RemoteDesktop(RemoteDesktopMessage),
    SettingToggleOsDetection,
    /// Toggle the global "record terminal sessions" setting.
    SettingToggleSessionLogging,
    /// Toggle full-detail recording (timing + resizes, feeds the .cast
    /// export) vs the plain output log.
    SettingToggleSessionLogFull,
    /// Toggle deflate compression of recorded chunks at flush time.
    SettingToggleSessionLogCompress,
    /// Toggle the global "record connection events" (history) setting.
    SettingToggleConnectionHistory,

    // Auto-update
    AutoReconnectTick,
    ConnectAnimTick,

    // Language
    LanguageChanged(String),
    /// User picked a layout-direction option (Auto / LTR / RTL).
    /// The string is the localized label shown in the picker; the
    /// dispatch handler maps it back to a `LayoutDirection` value.
    LayoutDirectionChanged(String),
    FlattenHostsToggle,

    // Local shell
    OpenLocalShell,
    /// Show the Local Shell picker overlay (Windows: cmd / PowerShell
    /// / WSL distros). On non-Windows platforms `OpenLocalShell` skips
    /// this and spawns the default directly.
    ShowLocalShellPicker,
    /// Result of the async shell-detection probe, `where.exe pwsh` +
    /// `wsl --list --quiet`. Lands in the message loop so we don't
    /// stall the UI thread on a cold WSL host.
    LocalShellsDetected(Vec<crate::state::LocalShellSpec>),
    /// Dismiss the picker overlay (clicking outside or Escape).
    HideLocalShellPicker,
    /// Spawn a specific local shell, `(program, args, label)`
    /// produced by clicking a row in the picker.
    OpenLocalShellWith {
        program: String,
        args: Vec<String>,
        label: String,
    },

    // Local terminals management (Settings → Terminal card)
    /// Navigate from the picker's "+ terminal" footer to the management
    /// card; closes the picker overlay.
    OpenLocalTerminalsSettings,
    /// Re-run the auto-scan and merge new findings into the curated list
    /// (keeps everything already there; re-adds detected entries removed
    /// earlier, since it's an explicit user action).
    RescanLocalTerminals,
    /// Result of the async re-scan probe; merged + persisted on arrival.
    LocalTerminalsRescanned(Vec<crate::state::LocalShellSpec>),
    /// Remove one curated entry by its id.
    RemoveLocalTerminal(uuid::Uuid),
    /// Set the "always open X" default (the entry id), or `None` to
    /// restore "always ask (picker)".
    SetDefaultLocalTerminal(Option<uuid::Uuid>),
    /// Open the "add local terminal" modal (blank form).
    OpenLocalTerminalAddModal,
    /// Open the modal to edit an existing entry by id.
    OpenLocalTerminalEditModal(uuid::Uuid),
    CloseLocalTerminalAddModal,
    /// Open the host icon / color picker targeting the add-edit form.
    OpenLocalTerminalIconPicker,
    /// Add / edit form field edits.
    LocalTerminalFormLabelChanged(String),
    LocalTerminalFormProgramChanged(String),
    LocalTerminalFormArgsChanged(String),
    LocalTerminalFormTagsChanged(String),
    /// Commit the add / edit form into the curated list.
    AddLocalTerminalSubmit,
    /// Hover tracking for the per-card remove action.
    LocalTerminalCardHovered(usize),
    LocalTerminalCardUnhovered,

    // Keys
    // Keys (handle_keys)
    Keys(KeysMessage),

    // ── SSH-agent server (B1) ──
    // Agent (handle_agent)
    Agent(AgentMessage),
    // (filename, content, auto-probed `<file>.pub` and `<file>-cert.pub`
    // if present and parseable)

    // Identities

    // Per-list sort menus (Hosts / Keychain / Snippets toolbars).
    // The Toggle* messages open/close the dropdown anchored to the
    // toolbar sort button; the Set* messages pick a sort mode and
    // persist it via the matching `*_sort` settings key.

    // Responsive toolbar collapse (narrow windows). `ToggleToolbarSearch`
    // pops/dismisses the floating search field when the inline box has
    // collapsed to an icon; `ToggleToolbarOverflow` pops/dismisses the
    // `…` menu folding the view's secondary toolbar actions.

    // Keyboard navigation, modal layer: pointer hover moved onto a
    // recorded row, so the keyboard ring follows it (index into the
    // per-frame `keynav.modal.items` recording).
    // A pick_list dropdown opened (true) or closed (false); keeps
    // `keynav.pick_open` in sync so app-side key routing yields to
    // the widget while its menu is up.

    // Proxy Identities (Settings → Proxies)

    // Cloud Accounts
    // Wired to a future "show password" eye icon next to the secret
    // input, `text_input.secure(false)` flips when this fires.

    // Cloud Discovery & Import
    SettingCloudAutoRefreshToggle,
    SettingCloudAutoRefreshIntervalChanged(String),
    SettingCloudAutoArchiveToggle,
    SettingCloudOrphanArchiveDaysChanged(String),

    // Plugins panel, cloud-provider plugin install / update lifecycle.
    // Plugin (handle_plugin)
    Plugin(PluginMessage),

    /// A CJK font (Korean / Chinese / Japanese) finished downloading or
    /// was read from cache; `Ok` carries the font bytes to hand to
    /// `iced::font::load`. Carries the language code so the in-memory
    /// "already loaded" guard can be cleared on failure for a retry.
    CjkFontReady(String, Result<Vec<u8>, String>),

    // Edit dynamic group panel, sets template fields (key, identity,
    // transport, initial command) on a `Group.cloud_query`.

    // Connection identity

    // AI settings

    // Vault password management

    // AI chat sidebar

    // Port forwarding

    // SSH agent forwarding (per-host opt-in)

    // MCP
    // Mcp (handle_mcp)
    Mcp(McpMessage),

    // Sync
    Sync(SyncMessage),

    // Export / Import
    // Export / import / share (handle_share)
    Share(ShareMessage),

    // System tray (Windows only at runtime; messages compile on
    // every platform so dispatch.rs and subscription.rs stay cfg-
    // free).

    // Share
}

impl Message {
    /// For an SFTP async-continuation message that targets a specific tab,
    /// returns that tab's id. The dispatcher uses this to swap the owning
    /// tab's state into `self.sftp` for the duration so the handler routes
    /// to the right tab even after the user focused a different SFTP tab.
    pub(crate) fn sftp_async_owner(&self) -> Option<Uuid> {
        match self {
            Message::SftpTransferQueueReady(id, _)
            | Message::SftpTransferNext(id)
            | Message::SftpTransferItemDone(id, _)
            | Message::SftpTransferError(id, _, _)
            | Message::SftpTransferConflict(id, _, _, _)
            | Message::SftpFor(id, _) => Some(*id),
            _ => None,
        }
    }

    /// Wrap an SFTP async completion in the `SftpFor` owner-routing
    /// envelope when a buffer owner existed at kickoff time. `None`
    /// falls back to the unowned message (pre-envelope behavior: applied
    /// to whichever buffer is live on arrival), which only happens when
    /// no SFTP surface owned the buffer at all.
    pub(crate) fn sftp_owned(owner: Option<Uuid>, message: SftpMessage) -> Message {
        match owner {
            Some(id) => Message::SftpFor(id, Box::new(message)),
            None => Message::Sftp(message),
        }
    }
}
