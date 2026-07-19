//! The full `Message` enum, every event the iced runtime can dispatch
//! to `Oryxis::update`. Pulled out of `app.rs` so the message-loop file
//! is shorter; re-exported via `pub use` at the bottom of `app.rs` so
//! call sites continue to write `crate::app::Message::Foo`.

use std::sync::Arc;

use iced::keyboard;
use iced::widget::text_editor;
use iced::Point;
use uuid::Uuid;

use oryxis_ssh::{ForwardSession, SshSession};
use oryxis_core::models::port_forward_rule::ForwardKind;

use crate::state::{ConnectionStep, SettingsSection, View};

mod ai;
pub use ai::AiMessage;
mod onboarding;
pub use onboarding::OnboardingMessage;
mod player;
pub use player::PlayerMessage;
mod vault;
pub use vault::VaultMessage;
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
    ChangeView(View),
    QuickHostInput(String),
    QuickHostContinue,
    OpenGroup(Uuid),
    HostSearchChanged(String),

    // Tabs
    SelectTab(usize),
    CloseTab(usize),
    TabHovered(usize),
    TabUnhovered,
    /// Cursor entered the trailing drop zone (the `+` button area) during an
    /// active tab-reorder drag: slide the dragged tab to the end of its
    /// partition, the one slot the live-slide can't otherwise reach.
    TabDragToEnd,
    ShowNewTabPicker,
    HideNewTabPicker,
    NewTabPickerSearchChanged(String),
    /// Enter pressed in the picker search: quick-connect when the input
    /// parses as `user@host[:port]`, otherwise a no-op.
    NewTabPickerSubmit,
    /// Drill into a group in the new-tab picker. For a cloud-query group
    /// this also kicks off (or refreshes) the resolve so the ECS tasks /
    /// K8s pods load. `Uuid` is the group id.
    NewTabPickerOpenGroup(Uuid),
    /// Step back out of a drilled-into group to the top-level picker list.
    NewTabPickerBack,
    ShowTabJump,
    HideTabJump,
    TabJumpSearchChanged(String),
    /// Translate a vertical mouse-wheel delta over the tab bar into a
    /// horizontal scroll on the tab strip. Carries the y-pixel delta;
    /// sign flips for natural-feeling navigation (wheel-down moves
    /// later tabs into view).
    TabBarWheel(f32),
    /// Two-step dispatch: close the modal first, then fire the inner
    /// message (SelectTab, OpenLocalShell, etc). Boxed to keep the enum
    /// variant size from blowing up.
    TabJumpSelect(Box<Message>),
    // ── Command palette (C4) ────────────────────────────────────────
    /// Open the command palette (`Ctrl+Shift+P`): resets the query and
    /// focuses the search input. Refused while the vault is locked.
    ShowCommandPalette,
    HideCommandPalette,
    PaletteQueryChanged(String),
    /// Two-step dispatch like `TabJumpSelect`: close the palette, then
    /// fire the row's real message (carried, not re-derived by index).
    PaletteActivate(Box<Message>),
    /// Replay a hotkey action from a palette row (reuses the per-action
    /// context gating in `dispatch_hotkey_action`).
    RunHotkeyAction(crate::hotkeys::HotkeyAction),
    /// Navigate to a Settings section from anywhere: switches to the
    /// Settings view AND selects the section (`ChangeSettingsSection`
    /// alone only sets the section, assuming the view is already open).
    OpenSettingsSection(crate::state::SettingsSection),
    // Absorb-click sink, used by modal bodies to stop clicks from falling
    // through to the backdrop underneath. Handler is a no-op.
    NoOp,

    // Icon picker (custom host icon/color)
    ShowIconPicker(Uuid),
    HideIconPicker,
    IconPickerSelectIcon(String),
    IconPickerSelectColor(String),
    IconPickerHexInputChanged(String),
    IconPickerIconSearchChanged(String),
    /// Open the HSV color popover, anchored at the current cursor.
    IconPickerOpenColorPopover,
    /// Dismiss the HSV color popover (click outside / pick done).
    IconPickerCloseColorPopover,
    IconPickerSave,
    IconPickerResetAuto,
    // Per-host terminal theme picker (modal opened from the host
    // editor). The form field updates immediately on select; the
    // change is committed on EditorSave like every other form field.
    EditorOpenThemePicker,
    EditorCloseThemePicker,
    /// Empty string == "inherit the global theme".
    EditorTerminalThemeChanged(String),
    /// Cloud transport pick (only meaningful when editing a cloud-imported host).
    EditorCloudTransportChanged(oryxis_core::models::cloud::TransportKind),
    /// Per-host initial command, sent as keystrokes after the shell
    /// opens. Empty = none. Useful for hosts that drop into `/bin/sh`
    /// when you really want `bash`.
    EditorInitialCommandChanged(text_editor::Action),
    /// Set the per-host icon shape override. Empty string clears the
    /// override (falls back to the global `default_host_icon`).
    EditorIconStyleChanged(String),
    EditorEncodingChanged(String),
    /// Per-host TERM name picked in the host editor.
    EditorTerminalTypeChanged(String),
    /// Empty string == "inherit the global keepalive setting".
    /// "0" == explicitly disabled on this host; any positive integer
    /// is the per-host override in seconds. Sanitized to digits-only.
    EditorKeepaliveChanged(String),
    /// Per-host auto-title (OSC 0/2) selection from the host editor pick:
    /// the localized "Default / Show / Hide" label.
    EditorAutoTitleChanged(String),
    /// Per-host Privacy Mode selection from the host editor pick: the
    /// localized "Default / On / Off" label.
    EditorPrivacyModeChanged(String),
    // ── C5 per-host legacy keyboard modes + feature toggles ──────────
    /// Backspace mode pick (localized "Control-? (127)" / "Control-H (8)").
    EditorQuirkBackspaceChanged(String),
    /// Home/End mode pick (localized "Standard" / "rxvt").
    EditorQuirkHomeEndChanged(String),
    /// Function-key mode pick (localized Xterm / Linux / VT400 / rxvt).
    EditorQuirkFnKeysChanged(String),
    /// "Report mouse to remote" toggle (off = `disable_mouse_reporting`).
    EditorQuirkMouseReportingChanged(bool),
    /// "Allow remote title changes" toggle (off = `disable_title_change`).
    EditorQuirkTitleChangeChanged(bool),
    /// OSC 52 clipboard-write override pick (localized Default / On / Off).
    EditorQuirkOsc52Changed(String),
    /// macOS Option-as-Meta pick (localized Off / Left / Right / Both;
    /// issue #80: the default composes characters like every macOS
    /// terminal, Meta is the readline/emacs opt-in).
    EditorQuirkOptionAsMetaChanged(String),
    /// Per-host SSH rekey limit (MB) text input.
    EditorQuirkRekeyChanged(String),
    /// Toggle a per-host SSH algorithm category between Auto (None) and a
    /// custom pinned list (seeded from the safe defaults).
    EditorAlgoSetAuto(crate::state::AlgoCategory, bool),
    /// Add/remove one algorithm name in a category's pinned list.
    EditorAlgoToggle(crate::state::AlgoCategory, String),
    ShowTabMenu(usize),
    ReconnectTab(usize),
    DuplicateTab(usize),
    DuplicateInNewWindow(usize),
    /// Pin / unpin a tab (from its context menu). Pinned tabs render first
    /// and are restored on the next launch.
    ToggleTabPin(usize),
    /// Open the rename dialog for a terminal tab (from its context menu).
    /// The name is transient: it lives for the tab's lifetime only and is
    /// never written back to the host or the pin spec.
    StartRenameTab(usize),
    /// Open the rename dialog for an SFTP tab (same transient semantics).
    StartRenameSftpTab(usize),
    TabRenameInput(String),
    /// Commit the rename dialog. An empty (or whitespace-only) name clears
    /// the custom name, restoring the automatic label.
    ConfirmTabRename,
    CancelTabRename,

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
    SidebarFilesNavigate(String),
    SidebarFilesRefresh,
    SidebarFilesToggleFollow,
    SidebarFilesToggleHidden,
    /// Promote the sidebar browser to a full SFTP tab at its current
    /// directory.
    SidebarFilesExpand,
    /// Initial mount finished: the SFTP channel, the session home (for
    /// `~`-relative cwd expansion) plus the first listing. The `u64` is
    /// the request stamp (`PaneFiles::req_seq`) captured at dispatch; a
    /// mismatch on arrival means a newer request (or a disconnect
    /// reset) superseded this one and it is dropped.
    SidebarFilesMounted(
        Uuid,
        u64,
        oryxis_ssh::SftpClient,
        Option<String>,
        String,
        Vec<oryxis_ssh::SftpEntry>,
    ),
    /// A navigation / follow / refresh listing landed (same stamp rule).
    SidebarFilesListed(Uuid, u64, String, Vec<oryxis_ssh::SftpEntry>),
    SidebarFilesError(Uuid, u64, String),
    SidebarFilesRowHovered(usize),
    SidebarFilesRowUnhovered,
    /// Right-click on a sidebar Files row: open its context menu
    /// (full path + is_dir), anchored at the cursor.
    ShowSidebarFilesRowMenu(String, bool),
    /// The header path is clickable (mirrors the SFTP pane's path
    /// editing): start / live-edit / commit typing a directory.
    SidebarFilesStartEditPath,
    SidebarFilesEditPath(String),
    SidebarFilesCommitPath,
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
    /// Uploads finished: toast the outcome, then refresh the pane's
    /// current listing through the normal stamped pipeline (the handler
    /// bumps the request stamp synchronously, so the refresh always
    /// resolves `loading` no matter how it completes).
    SidebarFilesUploadFinished(Uuid, String),
    /// Hybrid tab (issue #61): flip the terminal tab at this index
    /// between its Terminal and Files-full (dual-pane SFTP) states.
    /// Fired by the tab's mode glyph, the status-bar segment, the tab
    /// context menu and the hotkey.
    ToggleTabFilesMode(usize),
    /// Promote the terminal tab's SFTP session to a standalone SFTP tab
    /// (the server-to-server surface); the hybrid state moves out.
    DetachTabSftp(usize),
    /// Close ONLY the terminal tab's SFTP session (back to a plain
    /// terminal tab): drops the browsing state + channel, the mode
    /// glyph disappears. The terminal keeps running.
    CloseTabSftpSession(usize),
    /// From an SFTP tab's context menu: focus a live terminal tab on
    /// the mounted host, or connect one.
    OpenTerminalForSftpTab(usize),
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
    ShowFolderActions(Uuid),
    StartRenameFolder(Uuid),
    FolderRenameInput(String),
    ConfirmRenameFolder,
    CancelFolderModal,
    /// Open the manual host-group editor side panel for this group.
    EditGroup(Uuid),
    GroupEditLabelChanged(String),
    /// Open the icon/color picker routed to the group editor.
    ShowGroupEditIconPicker,
    SaveGroupEdit,
    CancelGroupEdit,
    StartDeleteFolder(Uuid),
    DeleteFolderKeepHosts,
    DeleteFolderWithHosts,
    CloseOtherTabs(usize),
    CloseAllTabs,

    // Terminal I/O
    PtyOutput(Uuid, Vec<u8>),  // (pane_id, bytes)
    /// A ZMODEM transfer streamed a progress / outcome event for a pane.
    /// Terminal states (Completed / Aborted / Error) clear the pane's
    /// transfer and resume the terminal.
    ZmodemProgress(Uuid, oryxis_zmodem::Progress),  // (pane_id, progress)
    /// User asked to cancel the pane's in-flight ZMODEM transfer.
    ZmodemCancel(Uuid),  // (pane_id)
    /// One-shot wake-up that force-flushes a stalled DEC `?2026`
    /// synchronized update on the given pane (`pane_id`). Armed by the
    /// `PtyOutput` handler when output stops mid-update; without it an app
    /// that opens a sync update and blocks on input freezes the screen.
    TerminalSyncFlush(Uuid),
    /// A cloud plugin PTY stream ended (session-manager-plugin /
    /// kubectl exited). Marks the tab disconnected, prints an in-pane
    /// notice and re-arms `pending_reopen` so selecting the tab again
    /// reconnects (the pane previously just went silently dead).
    PluginSessionEnded(Uuid),
    /// Scrollback find-bar (C1). All four act on the ACTIVE pane of the
    /// active terminal tab (the bar only ever shows there).
    /// Open the find-bar (Ctrl+F over the terminal) and focus its input.
    TerminalSearchOpen,
    /// The find-bar needle changed: rebuild matches and scroll the first
    /// hit into view.
    TerminalSearchInput(String),
    /// Step the active match forward (`true`, Enter) or backward
    /// (`false`, Shift+Enter), wrapping, and scroll it into view.
    TerminalSearchStep(bool),
    /// Close the find-bar (Esc) and drop the match set; the terminal keeps
    /// focus.
    TerminalSearchClose,
    /// Broadcast input (C2): arm / disarm fan-out of keystrokes, pastes and
    /// snippets to every pane of the tab at `usize`. Toggled by the status
    /// segment, the tab context menu and the `ToggleBroadcastInput` hotkey.
    /// Arming requires a split tab: on a single-pane tab the handler
    /// refuses with a hint toast (the segment and menu entry are not
    /// rendered there, so only the hotkey / palette reach it).
    ToggleTabBroadcast(usize),
    /// Broadcast input (C2): flip whether the pane at `Uuid` participates in
    /// its tab's broadcast (the per-pane observer opt-out).
    TogglePaneBroadcastOptOut(Uuid),
    KeyboardEvent(keyboard::Event),
    /// Text committed by the OS IME (e.g. a composed CJK character).
    /// Arrives separately from `KeyboardEvent`; forwarded to the active
    /// PTY in `dispatch_terminal` behind the same focus guards.
    TerminalImeCommit(String),
    MouseMoved(Point),
    WindowResized(iced::Size),
    /// OS window moved; carries the new outer position in logical
    /// desktop coordinates (negative on monitors left of / above the
    /// primary). Feeds the persisted window geometry so the next launch
    /// reopens on the same monitor at the same spot.
    WindowMoved(Point),
    /// Post-boot sanity check for the restored window position: if the
    /// saved coordinates landed on a monitor that is no longer there,
    /// move the window back onto the current monitor.
    WindowEnsureOnScreen,
    /// OS window gained (`true`) or lost (`false`) focus. Gates the
    /// cloud SSM/ECS keepalive ticker: it only runs while unfocused.
    WindowFocusChanged(bool),
    /// Periodic tick (mounted only while the window is unfocused and at
    /// least one SSM/ECS tab is open) that nudges those tabs' terminal
    /// size so the SSM idle timer resets and a long alt-tab away doesn't
    /// drop the session.
    SsmKeepaliveTick,
    WindowDrag,
    WindowResizeDrag(iced::window::Direction),
    /// Double-click on a N/S edge, fill the full monitor height while
    /// keeping horizontal position and width.
    WindowExpandVertical,
    WindowMinimize,
    WindowMaximizeToggle,
    WindowFullscreenToggle,
    /// Clears the "Press F11 to exit fullscreen" banner. Fired by a
    /// timed `Task::perform` 3 s after entering fullscreen.
    FullscreenHintHide,
    /// Settings → Shortcuts: enter capture mode for an action. The
    /// next non-Esc, non-pure-modifier `KeyPressed` becomes the new
    /// binding (see `shortcuts::handle_hotkey_capture`).
    StartEditingHotkey(crate::hotkeys::HotkeyAction, crate::hotkeys::HotkeySlot),
    /// Settings → Shortcuts: drop a single action's user override and
    /// fall back to the factory default.
    ResetHotkey(crate::hotkeys::HotkeyAction),
    /// Settings → Shortcuts: drop every user override.
    ResetAllHotkeys,
    WindowClose,
    /// Spawn a fresh top-level Oryxis window without binding to any
    /// existing tab. Triggered by Ctrl+Shift+N and the burger menu's
    /// "New Window" entry. Inherits the vault master password the
    /// same way `DuplicateInNewWindow` does.
    SpawnNewWindow,
    /// Focus the current view's primary search/filter input. Triggered
    /// by Ctrl+F outside the terminal. No-op when the active view has
    /// no search field (Snippets, Settings, History).
    FocusViewSearch,
    /// Activate the Nth slot of the visual tab strip (0-indexed). In
    /// Workspace mode slot 0 is Hosts, slot 1 is SFTP (when enabled),
    /// followed by terminal tabs. In Classic mode the strip only
    /// holds terminal tabs. Out-of-range slots are no-ops.
    ActivateStripSlot(usize),

    // Overlay
    HideOverlayMenu,

    // Card interactions
    CardHovered(usize),
    CardUnhovered,
    FolderCardHovered(Uuid),
    FolderCardUnhovered,
    KeyCardHovered(usize),
    KeyCardUnhovered,
    IdentityCardHovered(usize),
    SnippetCardHovered(usize),
    SnippetCardUnhovered,
    HistoryCardHovered(usize),
    HistoryCardUnhovered,
    IdentityCardUnhovered,
    ShowCardMenu(usize),
    #[allow(dead_code)]
    HideCardMenu,

    // Connection editor
    /// Continuation of a side-panel Tab press: `focused` is the widget
    /// iced actually has focused (resolved via `find_focused`), so the
    /// ring index can sync to a mouse-clicked field before walking to the
    /// next row. `None` = nothing focused (ring authoritative).
    PanelNavTabResolved {
        forward: bool,
        focused: Option<iced::widget::Id>,
    },
    ShowNewConnection,
    /// Open the host editor seeded as a RemoteDesktop host ("Add remote
    /// desktop" in the + Host menu; only shown when the feature toggle is on).
    ShowNewRemoteDesktop,
    EditConnection(usize),
    EditorLabelChanged(String),
    /// Host editor: comma-separated tags field.
    EditorTagsChanged(String),
    EditorHostnameChanged(String),
    /// Host editor: the wire-protocol picker (SSH / Telnet). Switching
    /// swaps the reduced form and, when the port still holds the old
    /// protocol's default, retargets it (22 <-> 23).
    EditorProtocolChanged(oryxis_core::models::connection::ConnectionProtocol),
    // Serial line params (reduced Serial form). Each carries the typed
    // value; the handler materializes `SerialParams` defaults first.
    EditorSerialBaudChanged(u32),
    EditorSerialDataBitsChanged(u8),
    EditorSerialParityChanged(oryxis_core::models::serial::SerialParity),
    EditorSerialStopBitsChanged(oryxis_core::models::serial::SerialStopBits),
    EditorSerialFlowChanged(oryxis_core::models::serial::SerialFlowControl),
    EditorSerialLineEndingChanged(oryxis_core::models::serial::SerialLineEnding),
    EditorSerialLocalEchoToggled,
    // Remote desktop (RDP/VNC) editor rows: kind picker + the SSH host
    // to tunnel through (`None` = direct). The desktop endpoint + login
    // reuse the normal hostname/port/username/password fields.
    EditorRdKindChanged(oryxis_core::models::remote_desktop::RemoteDesktopKind),
    EditorRdGatewayChanged(Option<uuid::Uuid>),
    /// Address-family preference picked in the host editor (SSH > Network).
    EditorAddressFamilyChanged(oryxis_core::models::connection::AddressFamily),
    EditorPortChanged(String),
    EditorUsernameChanged(String),
    EditorPasswordChanged(String),
    EditorAuthMethodChanged(String),
    EditorGroupChanged(String),
    EditorKeyChanged(String),
    // Chain editor (Termius-style multi-hop jump-host editor). Opens
    // from the "Host Chaining" row in the host editor; edits the
    // ordered `editor_form.jump_chain`.
    OpenChainEditor,
    CloseChainEditor,
    /// Switch the chain editor into "add a hop" mode (host picker).
    ChainEditorStartAdd,
    /// Back out of "add a hop" mode to the chain list.
    ChainEditorCancelAdd,
    ChainEditorSearchChanged(String),
    /// Append the selected connection as the next hop.
    ChainEditorAddHop(Uuid),
    ChainEditorRemoveHop(usize),
    ChainEditorMoveHopUp(usize),
    ChainEditorMoveHopDown(usize),
    EditorProxyKindChanged(crate::state::ProxyKind),
    EditorProxyHostChanged(String),
    EditorProxyPortChanged(String),
    EditorProxyUsernameChanged(String),
    EditorProxyPasswordChanged(String),
    EditorProxyCommandChanged(String),
    EditorTogglePasswordVisibility,
    /// TOTP secret (2FA) field: value edit + eye toggle. Tri-state save
    /// mirrors the password field (untouched preserves the stored secret).
    EditorTotpChanged(String),
    EditorToggleTotpVisibility,
    EditorSave,
    /// Connect using the current editor form WITHOUT persisting anything:
    /// builds an ephemeral quick-connect entry (typed credentials ride in
    /// memory) and dispatches `QuickConnect`. New-host flow only.
    EditorConnectWithoutSaving,
    EditorCancel,
    /// Ask for confirmation before removing a host. Confirming dispatches
    /// `DeleteConnection`. Destructive removals are routed through a confirm
    /// dialog so a stray click can't silently drop a host.
    RequestDeleteConnection(usize),
    DeleteConnection(usize),
    DuplicateConnection(usize),

    // Session groups (saved split-panel arrangements)
    /// Open the editor to save / edit the arrangement of tab `idx`.
    ShowSaveSessionGroup(usize),
    /// Open the editor for an existing saved group (index into session_groups).
    EditSessionGroup(usize),
    /// Open the saved group (index into session_groups) into a new split tab.
    OpenSessionGroup(usize),
    /// Save a copy of the group (new id, "… copy" label).
    DuplicateSessionGroup(usize),
    /// Ask for confirmation before removing a session group.
    RequestDeleteSessionGroup(usize),
    DeleteSessionGroup(usize),
    /// Open the card context menu (dots / right-click) for a session group.
    ShowSessionGroupMenu(usize),
    SessionGroupFormLabelChanged(String),
    SessionGroupFormGroupChanged(String),
    /// Multi-line edit on the currently-shown pane's startup script.
    SessionGroupScriptAction(text_editor::Action),
    /// Step the visible pane in the editor; `true` = next, `false` = previous.
    SessionGroupPaneNav(bool),
    SessionGroupFormSave,
    SessionGroupFormCancel,
    /// Open the shared icon/color picker targeting the session-group form.
    ShowSessionGroupIconPicker,
    SessionGroupCardHovered(usize),
    SessionGroupCardUnhovered,

    // SSH
    ConnectSsh(usize),
    /// Connect an ad-hoc quick-connect host (never persisted). The entry
    /// is inserted into `quick_connects` keyed by its connection id; a
    /// retry for an id already present reuses the stored entry so
    /// in-place mutations (expanded legacy algorithms) survive.
    QuickConnect(Box<crate::state::QuickConnectEntry>),
    /// Open the host editor prefilled from the quick-connect entry so the
    /// user can persist it as a regular host.
    SaveQuickHost(Uuid),
    /// Same prefill, but as the temporary-host edit flow (from the
    /// connect progress screen): Connect (without saving) is the primary
    /// footer action, Save the secondary.
    EditQuickHost(Uuid),
    SshProgress(ConnectionStep, String),
    /// Pre-auth banner (RFC 4252 §5.4) for the connect in progress:
    /// shown on the progress card and written to the tab's terminal.
    SshBanner(String),
    /// Pre-auth banner for a split-pane connect (no progress card):
    /// written straight to that pane's terminal.
    SshPaneBanner(Uuid, String),
    SshConnected(Uuid, crate::state::TerminalTransport),  // (pane_id, transport)
    SshDisconnected(Uuid),  // (pane_id)
    SshError(String),
    /// Handshake hit "no common algorithm". Prompts the legacy-fallback
    /// dialog for `conn_id` (the failed category + what the server offered).
    SshNoCommonAlgo {
        conn_id: uuid::Uuid,
        category: oryxis_ssh::NegCategory,
        server_offers: Vec<String>,
        /// Action to re-run after the user enables legacy algorithms (the
        /// originating connect: terminal / SFTP / port-forward / backup).
        retry: Box<Message>,
    },
    /// Accept the legacy fallback: enable the legacy algorithms on the
    /// pending host and reconnect. `remember` persists the change.
    LegacyAlgoAccept { remember: bool },
    LegacyAlgoCancel,
    SshHostKeyVerify(oryxis_ssh::HostKeyQuery),
    SshHostKeyReject,
    SshHostKeyContinue,
    SshHostKeyAcceptAndSave,
    /// A keyboard-interactive challenge round arrived from the engine.
    /// The `Option<Uuid>` is the quick-connect entry id when the prompt
    /// belongs to an ad-hoc connect (it unlocks the saved identity / key
    /// selector in the modal); `None` for saved hosts.
    SshKbiPrompt(Option<Uuid>, oryxis_ssh::KbiQuery),
    /// User edited the answer for prompt `usize` in the current round.
    SshKbiInput(usize, String),
    /// User submitted all answers for the current round.
    SshKbiSubmit,
    /// User cancelled the interactive auth.
    SshKbiCancel,
    /// User picked a saved identity / key for a quick-connect host (from
    /// the interactive-prompt modal or the failed-connect screen). Mutates
    /// the ephemeral entry and retries the connect with it.
    QuickAuthSwitch(Uuid, crate::state::QuickAuthChoice),
    SshCloseProgress,
    SshEditFromProgress,
    SshRetry,

    // Snippets
    // Snippets (handle_snippets)
    Snippet(SnippetMessage),
    /// Settings > Terminal: toggle the paste content heuristics.
    TogglePasteGuard,
    /// Dashboard: open/close the host tag-filter dropdown.
    ShowHostTagFilterMenu,
    /// Dashboard: toggle one tag in the multi-select filter (the
    /// dropdown stays open so several can be picked in one visit).
    ToggleHostTagFilterTag(String),
    /// Dashboard: clear the tag filter entirely.
    ClearHostTagFilter,

    // Command history (terminal sidebar History tab)
    /// Re-run a captured command in the active terminal (+ Enter).
    RunHistoryCommand(Uuid),
    /// Insert a captured command WITHOUT the trailing newline.
    PasteHistoryCommand(Uuid),
    /// Ask before removing a captured command (routes through the shared
    /// confirm dialog; a lone misclick on the hover trash silently wiped
    /// a host's only entry once, live QA 2026-07-03).
    RequestDeleteHistoryCommand(Uuid),
    /// Remove one captured command from the host's history (confirmed).
    DeleteHistoryCommand(Uuid),
    /// Filter text for the sidebar History tab's search field (distinct
    /// from `HistorySearchChanged`, which filters the session-logs view).
    CmdHistorySearchChanged(String),
    /// Save the focused host's captured commands to a plain-text file
    /// (save dialog; offline reference / support sharing).
    ExportCommandHistory,
    /// Outcome of the export: `Ok(path)` shows a toast, `Err` a warning.
    CommandHistoryExported(Result<String, String>),
    /// Settings > Terminal: enable/disable command-history capture.
    ToggleCommandHistory,
    /// Settings > Terminal: live-append captured commands to per-host
    /// text files.
    ToggleCommandHistoryFile,
    /// Pick the folder the per-host command logs are written into.
    PickCommandHistoryDir,
    /// Folder chosen (or dialog dismissed with `None`).
    CommandHistoryDirPicked(Option<String>),
    /// Pick the folder ZMODEM downloads are saved into.
    PickZmodemDownloadDir,
    /// ZMODEM download folder chosen (or dialog dismissed with `None`).
    ZmodemDownloadDirPicked(Option<String>),
    /// Reset the ZMODEM download folder to the OS default.
    ClearZmodemDownloadDir,

    // Split panes
    /// Focus a pane (click). Routes keyboard / snippets / paste to it.
    FocusPane(iced::widget::pane_grid::Pane),
    /// Drag a pane divider to resize.
    ResizePane(iced::widget::pane_grid::ResizeEvent),
    /// Split the focused pane of the active tab along an axis, opening the
    /// connection picker to fill the new pane.
    SplitPane(iced::widget::pane_grid::Axis),
    /// Like `SplitPane` but targets a specific tab (from its right-click
    /// menu), so it works even when that tab isn't the active one.
    SplitTabPane(usize, iced::widget::pane_grid::Axis),
    /// Hover entered the `+` button: reveal the New-Tab / Split popover.
    /// No-op unless a terminal tab is open.
    ShowSplitMenu,
    /// Cursor entered the popover itself (keeps it open across the bridge).
    SplitMenuEnter,
    /// Cursor left the `+` button or the popover: schedule a close.
    SplitMenuLeave,
    /// Delayed close: hide the popover unless the cursor came back to it.
    SplitMenuCloseIfIdle,
    /// Close the focused pane (closes the tab if it was the last one).
    ClosePane,
    /// Move focus to the adjacent pane in a direction (keyboard nav).
    FocusPaneDir(iced::widget::pane_grid::Direction),
    /// Picker "Local Shell" entry. Opens a local shell, into a split pane
    /// when `pending_pane_split` is set, otherwise a new tab.
    PickLocalShell,
    /// A pane's SSH connect failed; surface the error inside the pane.
    PaneConnectError(Uuid, String),

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
    ShowPortForwardPanel,
    HidePortForwardPanel,
    PfLabelChanged(String),
    PfKindChanged(ForwardKind),
    PfHostChanged(Uuid),
    PfListenHostChanged(String),
    PfListenPortChanged(String),
    PfTargetHostChanged(String),
    PfTargetPortChanged(String),
    PfAutoStartToggled(bool),
    SavePortForwardRule,
    EditPortForwardRule(usize),
    DeletePortForwardRule(usize),
    /// Toggle a rule on: opens a dedicated PTY-less SSH session.
    StartPortForward(Uuid),
    /// Toggle a rule off: drops its `ForwardSession` (cancels the tunnel).
    StopPortForward(Uuid),
    /// Result of a `StartPortForward` connect attempt.
    PortForwardStarted(Uuid, Result<Arc<ForwardSession>, String>),
    /// Periodic liveness sweep; drops forwards whose connection died.
    PortForwardLivenessTick,
    /// Periodic flush of buffered session-log output to the vault.
    SessionLogFlushTick,
    PortForwardCardHovered(usize),
    PortForwardCardUnhovered,
    PortForwardSearchChanged(String),
    CloudSearchChanged(String),
    ProxySearchChanged(String),

    // Terminal side panel (Chat / Snippets / Host config tabs)
    // AI settings + chat sidebar (handle_ai)
    Ai(AiMessage),
    /// Live per-host edits from the Host config sidebar tab. Each mutates
    /// the focused pane's connection, persists immediately, and (for the
    /// theme) repaints the running terminal for instant preview.
    HostConfigThemeChanged(String),
    HostConfigEncodingChanged(String),
    HostConfigTerminalTypeChanged(String),
    HostConfigAutoTitleChanged(String),
    /// Local/ephemeral panes have no saved host: pick a session-only theme
    /// for the open local terminals, or promote it to the global default.
    LocalConfigThemeChanged(String),
    LocalConfigSaveGlobal,

    // Known hosts
    /// Open the confirm dialog before deleting a single known host.
    RequestDeleteKnownHost(usize),
    DeleteKnownHost(usize),
    /// Open the confirm dialog before clearing every known host.
    RequestClearAllKnownHosts,
    ClearAllKnownHosts,

    // History
    RequestClearHistory,
    CancelClearHistory,
    ClearLogs,
    LogsPageNext,
    LogsPagePrev,

    // Session logs
    ViewSessionLog(Uuid),
    /// Open the kebab menu on a History session row.
    ShowSessionLogMenu(usize),
    /// Export a recorded session as an asciicast v2 `.cast` file
    /// (replayable in the asciinema player). Output-only by design.
    ExportSessionCast(Uuid),
    /// Export a recorded session as a plain-text transcript (ANSI
    /// resolved and stripped by the same renderer the viewer uses).
    ExportSessionTranscript(Uuid),
    /// Export only the commands typed during a recorded session (the
    /// 'c' chunks) as a plain-text file.
    ExportSessionCommands(Uuid),
    /// Render a recorded session to an animated GIF via the
    /// `oryxis-gif` plugin (downloaded on first use). Opens the plugin
    /// install modal when the binary isn't present yet and resumes the
    /// export after the install.
    ExportSessionGif(Uuid),
    /// Outcome of a GIF render: `None` = save dialog dismissed (no
    /// toast), `Some(Ok(path))` / `Some(Err(cause))` otherwise.
    GifExportFinished(Option<Result<String, String>>),
    CloseSessionLogView,
    /// Toggle the viewer-header `...` menu (session-log actions minus
    /// Play, which the viewer offers as its own header button).
    ShowSessionLogViewerMenu(usize),
    /// In-app session player (issue #71): a read-only playback surface
    /// on the History view.
    Player(PlayerMessage),
    /// Ask for confirmation before deleting one recording; the
    /// dialog's action carries `DeleteSessionLog`.
    RequestDeleteSessionLog(usize),
    DeleteSessionLog(usize),
    /// Hover tracking for clickable session rows in the Logs view.
    LogRowHovered(Uuid),
    LogRowUnhovered,
    // History was split in v0.6 (logs + session logs in two panes
    // with independent pagination); v0.7 merges both into one timeline
    // so the per-section Clear / Next / Prev controls don't render
    // anymore. Handlers stay wired so we can resurrect a dedicated
    // session-logs surface without re-introducing the messages.
    #[allow(dead_code)]
    ClearSessionLogs,
    #[allow(dead_code)]
    SessionLogsPageNext,
    #[allow(dead_code)]
    SessionLogsPagePrev,

    // Settings
    TerminalThemeChanged(String),
    /// Retention code picked in Settings ("off" / "1d" / ... / "90d");
    /// persists and prunes immediately.
    LogsRetentionChanged(&'static str),
    AppThemeChanged(String),
    TerminalFontSizeIncrease,
    TerminalFontSizeDecrease,
    TerminalFontChanged(String),
    /// Emitted by the terminal widget when the user right-clicks. The
    /// dispatcher reads the clipboard and routes the text to the SSH
    /// session (if active) or the local PTY, mirroring Ctrl+Shift+V.
    TerminalPasteFromClipboard,
    /// Careful-paste confirmation: send the multi-line text held in
    /// `pending_paste` to the active session.
    ConfirmPendingPaste,
    /// Careful-paste confirmation dismissed: drop the held text.
    CancelPendingPaste,
    /// Raw input bytes synthesized by the terminal widget (mouse-tracking
    /// reports, wheel-to-arrow translation). Routed to the active SSH
    /// session, falling back to the local PTY.
    TerminalInput(Vec<u8>),
    /// The user left-dragged in a pane whose remote app has mouse tracking
    /// on, so the drag is being reported instead of selecting text. Shows
    /// the "hold Shift to select" toast. Fires at most once per pane.
    TerminalMouseCaptureHint,
    /// The user plain-clicked (no Ctrl) a link in the terminal, so it
    /// selected instead of opening. Shows the "hold Ctrl and click to
    /// open" toast; under `HintMode::Once` it fires at most once per pane.
    TerminalLinkClickHint,
    /// The user ctrl-clicked a link in the terminal: the gesture landed,
    /// so under `HintMode::Once` retire the link toast for the focused pane.
    TerminalLinkOpened,
    /// Settings: terminal hint mode picker changed. Carries the localized
    /// option label; the dispatch handler maps it back to a `HintMode`.
    HintModeChanged(String),
    /// Flip the reveal/eye state of a secret input field.
    ToggleSecretVisibility(crate::state::SecretField),
    /// Settings: switch the auto-update release channel (stable/nightly).
    SettingUpdateChannelChanged(crate::update::UpdateChannel),
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
    /// Open the terminal context menu for a pane at a window-absolute
    /// point (right-click scheme = Menu). `(pane_id, x, y, selection)`,
    /// where `selection` is the live selection's text captured by the
    /// widget (`None` when empty), so the menu can offer "Copy".
    ShowTerminalContextMenu(Uuid, f32, f32, Option<String>),
    /// Copy the captured selection text to the clipboard (context-menu
    /// "Copy").
    TerminalCopySelection(String),
    /// Copy the whole buffer (scrollback + screen) of a pane to the
    /// clipboard (context-menu "Copy All"). `pane_id`.
    TerminalCopyAll(Uuid),
    /// Drop a pane's scrollback history (context-menu "Clear
    /// Scrollback"). `pane_id`.
    TerminalClearScrollback(Uuid),
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
    /// Clear a pane's visual-bell flash after its short display window.
    TerminalBellFlashEnd(Uuid),
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
    /// Host editor startup-command source changed (the picker label:
    /// the None sentinel, the Custom sentinel, or a snippet label).
    EditorStartupChoiceChanged(String),
    /// The Initial Command / Snippet combo gained focus; clears its
    /// typed value so the dropdown opens on the full list.
    EditorStartupComboOpened,
    /// The SSH Key combo gained focus; clears its typed value so the
    /// dropdown opens on the full list.
    EditorKeyComboOpened,
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
    /// A tray menu item was clicked (carries the raw menu id). Delivered
    /// by the event-driven tray subscription so a click wakes the UI
    /// only when it happens, instead of the old 100 ms poll that
    /// re-rendered the whole app 10x/s on Windows. Only constructed on
    /// Windows (the subscription is only mounted there).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    TrayMenuEvent(String),
    /// The tray icon was double-clicked (restore the window). Same
    /// event-driven delivery as [`TrayMenuEvent`].
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    TrayIconDoubleClick,
    SettingTabCloseButtonSideChanged(String),
    SettingPinnedTabStyleChanged(String),
    SettingTabFillStyleChanged(String),
    SettingTabAccentColorChanged(String),
    /// Dock the tab strip at the top (default) or the bottom of the
    /// window ("top" / "bottom"). The window chrome (burger, drag area,
    /// minimize / maximize / close) stays in a slim top bar either way.
    SettingTabBarPositionChanged(String),
    SettingToggleShowTabStatusDot,
    /// Show/hide the top-left burger menu (Settings / Updates / About /
    /// Exit). Mirrors Termius's `☰` strip at the start of the tab bar.
    ToggleBurgerMenu,
    /// Show/hide the vault sub-nav overflow ("…") menu listing the
    /// destinations that didn't fit in the pill strip.
    ToggleSubnavOverflow,
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
    /// Tunnel + client-spawn result: `Ok((session, local_port))` keeps
    /// the managed forward alive; `Err` is a ready-to-toast message. The
    /// `u64` is the launch generation, so a stale result from a superseded
    /// launch can't clobber a newer tunnel for the same host.
    RemoteDesktopReady(
        Uuid,
        u64,
        Result<(std::sync::Arc<oryxis_ssh::ForwardSession>, u16), String>,
    ),
    /// The ephemeral RDP/VNC tunnel closed on its own (desktop client
    /// disconnected and it went idle). Carries the launch generation so
    /// only the matching map entry is dropped.
    RemoteDesktopClientClosed(Uuid, u64),
    /// Tear down the RDP/VNC tunnel for this host connection id.
    StopRemoteDesktop(Uuid),
    /// Copy the canonical ssh:// URL of the host at this index (card
    /// context-menu action).
    CopyHostSshUrl(usize),
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
    OsDetected(Uuid, Option<String>),
    SettingToggleAutoCheckUpdates,

    // Auto-update
    CheckForUpdate,
    CheckForUpdateManual,
    UpdateCheckResult(Option<crate::update::UpdateInfo>),
    /// Manual update check failed (network / HTTP / parse); carries the
    /// concise cause for the Settings > About status line + toast.
    UpdateCheckFailed(String),
    UpdateSkipVersion,
    UpdateLater,
    UpdateStartDownload,
    UpdateDownloadProgress(f32),
    UpdateDownloadComplete(Result<std::path::PathBuf, String>),
    UpdateOpenRelease,
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
    ShowKeyPanel,
    HideKeyPanel,
    /// Keychain > ADD > Generate key: open the generation panel.
    ShowKeyGeneratePanel,
    HideKeyGeneratePanel,
    KeyGenLabelChanged(String),
    KeyGenCommentChanged(String),
    KeyGenAlgoSelected(crate::state::KeyGenAlgo),
    KeyGenBitsSelected(oryxis_vault::RsaBits),
    KeyGenCurveSelected(oryxis_vault::EcdsaCurveChoice),
    /// Kick the generation task (RSA runs seconds; spawn_blocking).
    GenerateKey,
    /// Generation finished; Ok saves to the vault and shows the
    /// result screen.
    KeyGenerated(Result<std::sync::Arc<oryxis_vault::GeneratedKey>, String>),
    CopyGeneratedPublicKey,
    SaveGeneratedPublicKeyFile,
    KeyGenExportPassphraseChanged(String),
    KeyGenExportPassphraseConfirmChanged(String),
    KeyGenExportPassphraseToggleVisibility,
    KeyGenExportPassphraseConfirmToggleVisibility,
    /// Export the generated private key to a file, passphrase-
    /// encrypted when the pair fields are non-empty.
    ExportGeneratedPrivateKey,

    // ── SSH-agent server (B1) ──
    /// Settings > Features & Plugins: turn the agent on/off.
    AgentServerToggled(bool),
    /// Toggle the per-signature confirm prompt.
    AgentConfirmToggled(bool),
    /// Toggle accepting external ADD/REMOVE (KeePassXC et al) into the
    /// in-memory roster.
    AgentAllowAddToggled(bool),
    /// Toggle also serving the standard OpenSSH agent pipe name when
    /// free (Windows only).
    AgentOpensshPipeToggled(bool),
    /// A per-key "expose via agent" toggle on the keychain card.
    KeyExposeViaAgentToggled(uuid::Uuid),
    /// A confirm prompt arrived from the agent runtime (carries the
    /// oneshot responder inside an Arc<Mutex<>> so Message stays Clone).
    AgentConfirmAsk(crate::state::AgentConfirmCard),
    /// The user's confirm decision; `always` grants the key for the
    /// session.
    AgentConfirmDecision { allow: bool, always: bool },
    /// Toggle the confirm card's "remember this session" checkbox.
    AgentConfirmToggleAlways,
    /// The on-screen confirm prompt tagged with this seq has sat
    /// unanswered past its deadline: deny it and drop the dead modal.
    AgentConfirmTimedOut(u64),
    /// Copy the socket path / a setup snippet from the settings block.
    CopyAgentPath,
    CopyAgentSnippet(crate::state::AgentSnippetKind),
    KeyImportLabelChanged(String),
    KeyContentAction(text_editor::Action),
    BrowseKeyFile,
    // (filename, content, auto-probed `<file>.pub` and `<file>-cert.pub`
    // if present and parseable)
    KeyFileLoaded(String, String, Option<String>, Option<String>),
    KeyFileBrowseError(String),
    KeyImportPassphraseChanged(String),
    KeyImportPassphraseToggleVisibility,
    /// Edit action on the public-key textarea (B2.1).
    KeyImportPublicAction(text_editor::Action),
    /// Edit action on the attached-certificate textarea (B2).
    KeyImportCertAction(text_editor::Action),
    /// Open the key import panel with the certificate field focused
    /// (the keychain ADD menu's "Certificate" entry, B2.1).
    ShowKeyPanelCertFocus,
    /// Open the key import panel with the public-key field focused
    /// (the keychain ADD menu's "Import public key" entry, B3).
    ShowKeyPanelPublicFocus,
    /// Pick a `.pub` certificate file for the key import form.
    BrowseCertFile,
    /// A certificate file was read; its contents fill the paste field.
    CertFileLoaded(String),
    /// Open the read-only certificate viewer for key at index.
    ViewKeyCertificate(usize),
    CloseCertViewer,
    /// Ask for confirmation before detaching a key's certificate.
    RequestRemoveKeyCertificate(usize),
    /// Detach the certificate from key at index (post-confirm).
    RemoveKeyCertificate(usize),
    ImportKey,
    /// Ask for confirmation before removing a key.
    RequestDeleteKey(usize),
    DeleteKey(usize),
    ShowKeyMenu(usize),
    #[allow(dead_code)]
    HideKeyMenu,
    EditKey(usize),
    KeySearchChanged(String),
    /// Workspace sub-nav search input wired to Snippets view.
    SnippetSearchChanged(String),
    /// Workspace sub-nav search input wired to History view.
    HistorySearchChanged(String),

    // Identities
    ShowIdentityPanel,
    HideIdentityPanel,
    IdentityLabelChanged(String),
    IdentityUsernameChanged(String),
    IdentityPasswordChanged(String),
    IdentityKeyChanged(String),
    IdentityTogglePasswordVisibility,
    SaveIdentity,
    EditIdentity(usize),
    /// Ask for confirmation before removing an identity.
    RequestDeleteIdentity(usize),
    DeleteIdentity(usize),
    ShowIdentityMenu(usize),
    ToggleKeychainAddMenu,

    // Per-list sort menus (Hosts / Keychain / Snippets toolbars).
    // The Toggle* messages open/close the dropdown anchored to the
    // toolbar sort button; the Set* messages pick a sort mode and
    // persist it via the matching `*_sort` settings key.
    ToggleSortMenu(crate::state::SortMenuKind),
    SetListSort(crate::state::SortMenuKind, crate::state::ListSort),

    // Responsive toolbar collapse (narrow windows). `ToggleToolbarSearch`
    // pops/dismisses the floating search field when the inline box has
    // collapsed to an icon; `ToggleToolbarOverflow` pops/dismisses the
    // `…` menu folding the view's secondary toolbar actions.
    ToggleToolbarSearch,
    ToggleToolbarOverflow,

    // Keyboard navigation, modal layer: pointer hover moved onto a
    // recorded row, so the keyboard ring follows it (index into the
    // per-frame `keynav.modal.items` recording).
    ModalNavHover(usize),
    // A pick_list dropdown opened (true) or closed (false); keeps
    // `keynav.pick_open` in sync so app-side key routing yields to
    // the widget while its menu is up.
    PickOpenChanged(bool),

    // Proxy Identities (Settings → Proxies)
    ShowProxyIdentityForm(Option<Uuid>),
    HideProxyIdentityForm,
    ProxyIdentityFormLabelChanged(String),
    ProxyIdentityFormKindChanged(crate::state::ProxyKind),
    ProxyIdentityFormHostChanged(String),
    ProxyIdentityFormPortChanged(String),
    ProxyIdentityFormUsernameChanged(String),
    ProxyIdentityFormPasswordChanged(String),
    ProxyIdentityFormPasswordToggleVisibility,
    SaveProxyIdentity,
    DeleteProxyIdentity(Uuid),

    // Cloud Accounts
    ShowCloudForm(Option<Uuid>),
    HideCloudForm,
    CloudFormLabelChanged(String),
    CloudFormProviderChanged(crate::state::CloudProviderChoice),
    CloudFormAuthKindChanged(crate::state::CloudAuthChoice),
    CloudFormAwsProfileNameChanged(String),
    CloudFormAwsRegionDraftChanged(String),
    /// Commit the current draft to the regions chip list. Supports
    /// comma or whitespace separated input so paste-multiple works.
    CloudFormAwsRegionAdd,
    CloudFormAwsRegionRemove(usize),
    CloudFormAwsAccessKeyIdChanged(String),
    CloudFormAwsAccessKeySecretChanged(String),
    CloudFormAwsAccessKeySessionTokenChanged(String),
    // Wired to a future "show password" eye icon next to the secret
    // input, `text_input.secure(false)` flips when this fires.
    #[allow(dead_code)]
    CloudFormAwsAccessKeySecretToggleVisibility,
    CloudFormAwsSsoStartUrlChanged(String),
    CloudFormAwsSsoRegionChanged(String),
    CloudFormAwsSsoAccountIdChanged(String),
    CloudFormAwsSsoRoleNameChanged(String),
    /// Kubernetes (Kubeconfig) auth fields.
    CloudFormKubeconfigPathChanged(String),
    CloudFormContextChanged(String),
    /// GCP project id field in the cloud wizard.
    CloudFormGcpProjectChanged(String),
    CloudFormAzureSubscriptionChanged(String),
    /// Kicks off a `test_credentials` round-trip via the registered
    /// provider. The result lands as `CloudFormTestResult`.
    CloudFormTestCredentials,
    CloudFormTestResult(Result<(), String>),
    SaveCloudProfile,
    DeleteCloudProfile(Uuid),
    /// Open the kebab context menu on a cloud account card. Anchored
    /// to the cursor like the host-card menu.
    ShowCloudCardMenu(Uuid),
    CloudCardHovered(Uuid),
    CloudCardUnhovered,
    /// Open the cloud-provider picker dropdown next to the "+ Host"
    /// button (only when at least one cloud profile is configured).
    ShowCloudProviderPicker,

    // Cloud Discovery & Import
    ShowCloudDiscover(Uuid),
    HideCloudDiscover,
    CloudDiscoverRefresh,
    /// Result of `provider.discover()`, payload boxed because
    /// `DiscoveryResult` carries collections per resource family and
    /// clippy yells about the variant size otherwise.
    CloudDiscoverResult(Result<Box<oryxis_cloud::DiscoveryResult>, String>),
    CloudDiscoverToggleEc2(String),
    /// Toggle an ECS service entry in the discovery panel. Carries
    /// the `cluster/service/container` key.
    CloudDiscoverToggleEcs(String),
    /// Toggle a discovered K8s workload (`namespace/kind/name`).
    CloudDiscoverToggleK8s(String),
    CloudDiscoverImport,
    /// Triggered from the transport-confirmation modal: actually run
    /// the import using the picked default transport.
    CloudDiscoverImportConfirmed,
    /// Close the transport-confirmation modal without importing.
    CloudDiscoverImportCancelled,
    CloudDiscoverFilterChanged(String),
    /// Toggle expanded/collapsed state of a section header in the
    /// discovery panel. Carries the section key (e.g. `"ec2"`).
    CloudDiscoverToggleSection(String),
    /// Add a discovered GKE cluster: fetch its kubeconfig
    /// (get-credentials) and create a Kubernetes account pointed at the
    /// resulting context.
    CloudDiscoverAddGke { cluster: String, location: String },
    /// get-credentials succeeded: `(label, context)` for the new K8s
    /// account to create.
    CloudDiscoverGkeCredentials(String, String),
    /// Result of the GKE add: `Ok(())` created the k8s account (refresh),
    /// `Err(msg)` surfaces on the discovery panel.
    CloudDiscoverGkeAdded(Result<(), String>),
    /// Add a discovered AKS cluster: fetch its kubeconfig
    /// (get-credentials) and create a Kubernetes account pointed at the
    /// resulting context.
    CloudDiscoverAddAks { cluster: String, resource_group: String },
    /// get-credentials succeeded: `(label, context)` for the new K8s
    /// account to create.
    CloudDiscoverAksCredentials(String, String),
    /// Result of the AKS add: `Ok(())` created the k8s account (refresh),
    /// `Err(msg)` surfaces on the discovery panel.
    CloudDiscoverAksAdded(Result<(), String>),
    CloudDiscoverDefaultTransportChanged(oryxis_core::models::cloud::TransportKind),
    CloudDiscoverDefaultGroupNameChanged(String),
    CloudDiscoverDefaultGroupPick(String),
    /// Open / close the shared group picker for a side-panel parent
    /// group input. Anchors the popover at the matching combo's
    /// measured bounds (`dynamic_form_parent_combo_bounds` or
    /// `session_group_folder_combo_bounds`).
    ToggleGroupPicker(crate::state::GroupPickerTarget),
    /// Live filter for the shared group-picker popover.
    GroupPickerSearchChanged(String),
    /// Route a pick into the matching form field and close the
    /// popover. Existing field-change messages (`EditorGroupChanged`,
    /// `DynamicGroupFormParentChanged`) still drive the write.
    GroupPickerPick(crate::state::GroupPickerTarget, String),
    /// Toggle the floating group-picker overlay rendered at the top
    /// of the Discover import modal. Independent of the global
    /// OverlayState so it can sit on top of the modal scrim.
    ToggleCloudDiscoverGroupPicker,
    /// Live filter typed inside the group-picker overlay's own
    /// search field. Doesn't affect the main "Import into" input.
    CloudDiscoverDefaultGroupPickerSearchChanged(String),
    /// Apply / clear the dashboard cloud-profile filter. Passing None
    /// clears it; passing Some(pid) restricts the grid to items whose
    /// cloud origin matches that profile.
    HostFilterByCloudProfile(Option<Uuid>),
    /// Manual sync of a cloud profile, re-runs discovery and updates
    /// every already-imported host whose `cloud_ref.profile_id` matches.
    /// Fields the user has flagged in `customized_fields` are preserved.
    /// Hosts not in the upstream result get their `cloud_ref.orphaned_at`
    /// set; hosts that come back get it cleared.
    CloudProfileSync(Uuid),
    CloudProfileSyncResult(Uuid, Result<Box<oryxis_cloud::DiscoveryResult>, String>),
    SettingCloudAutoRefreshToggle,
    SettingCloudAutoRefreshIntervalChanged(String),
    SettingCloudAutoArchiveToggle,
    SettingCloudOrphanArchiveDaysChanged(String),
    /// Fired by the iced subscription when the auto-refresh interval
    /// elapses. Iterates every cloud profile and dispatches a
    /// `CloudProfileSync(pid)` for each.
    CloudAutoRefreshTick,
    DynamicGroupFormLabelChanged(String),
    DynamicGroupFormParentChanged(String),
    DynamicGroupFormClusterChanged(String),
    DynamicGroupFormServiceChanged(String),
    DynamicGroupFormContainerChanged(String),
    /// K8s dynamic-group source fields (context / namespace / selector
    /// kind + value).
    DynamicGroupFormK8sContextChanged(String),
    DynamicGroupFormNamespaceChanged(String),
    DynamicGroupFormK8sSelectorKindChanged(crate::state::K8sSelectorKind),
    DynamicGroupFormK8sSelectorValueChanged(String),
    /// Open the shared icon + color picker pre-filled with the current
    /// dynamic-group form values. On Save the picker writes back to the
    /// form (not directly to the vault) so the deferred Save button on
    /// the form panel still controls when the group is persisted.
    ShowIconPickerForDynamicGroupForm,
    /// Kick off `provider.resolve_query()` for a dynamic group. The
    /// async result lands as `DynamicGroupResolved`. Idempotent
    /// safe to dispatch even if a resolve is already running for the
    /// same group; the dashboard handler dedupes.
    DynamicGroupResolve(Uuid),
    /// User clicked a task row inside an open dynamic group. Carries
    /// the group id (so we can find the cloud_query) and the task's
    /// `resource_id` (the task ARN suffix). Triggers ECS Exec.
    /// Connect to whichever task of the dynamic group is currently
    /// running, re-resolving first when the cached listing is stale.
    /// Used by pinned-tab reopen (the stored task id is ephemeral by
    /// nature) and by the "connect to current task" recovery button
    /// after an exec failure. `fallback_task_id` wins when it still
    /// exists; otherwise the first RUNNING task is picked.
    EcsExecConnectFreshTask {
        group_id: Uuid,
        container: String,
        fallback_task_id: String,
    },
    ConnectEcsExecTask {
        group_id: Uuid,
        task_id: String,
        task_label: String,
        /// Specific container to exec into. Required because under
        /// wildcard queries (empty `container` in `cloud_query`) the
        /// row knows which container the user actually clicked while
        /// the query itself doesn't pin one. Always populated from
        /// the row's `DiscoveredHost.container_name`.
        container: String,
    },
    /// Open an interactive shell in a Kubernetes pod by spawning
    /// `kubectl exec -it` in a local PTY. No provider round-trip; the
    /// dispatch builds the kubectl args from the group's profile + query.
    ConnectKubectlExecPod {
        group_id: Uuid,
        namespace: String,
        pod: String,
        /// Container to exec into, empty = the pod's default (kubectl
        /// picks the first container).
        container: String,
    },
    /// Result of `ecs:ExecuteCommand` + plugin invocation prep. On
    /// success the dispatch spawns the plugin and opens a tab; on
    /// error it's surfaced in the UI.
    EcsExecSessionReady {
        /// Group the task belongs to. Carried so the error arm can
        /// re-resolve the dynamic group's list: a failed connect on a
        /// recycled task means the cached list is stale, refreshing it
        /// surfaces the live task without a manual Refresh click.
        group_id: Uuid,
        task_label: String,
        /// Task id + container the session targets. Carried so the
        /// spawn handler can rebuild a `ConnectEcsExecTask` and stash
        /// it on the tab as its relaunch message (used by Duplicate Tab,
        /// ECS tabs have no saved `Connection` to look up by label).
        task_id: String,
        container: String,
        result: Result<Box<oryxis_cloud::SessionPayload>, String>,
    },
    /// SSM Session result, same plugin payload shape as ECS Exec, so
    /// we reuse the spawn path. Carries the host's display label so
    /// the spawned tab gets a useful title.
    SsmSessionReady {
        host_label: String,
        result: Result<Box<oryxis_cloud::SessionPayload>, String>,
    },
    DynamicGroupResolved(Uuid, Result<Vec<oryxis_cloud::DiscoveredHost>, String>),

    // Plugins panel, cloud-provider plugin install / update lifecycle.
    /// Global auto-update toggle (applies to every plugin without an
    /// explicit per-plugin override).
    PluginToggleGlobalAutoUpdate(bool),
    /// Per-plugin auto-update override.
    PluginToggleAutoUpdate(String, bool),
    /// Fetch the hosted manifest for a provider and compare against
    /// the installed version.
    PluginCheckUpdates(String),
    /// Header action: run the update check for every installed plugin.
    PluginCheckAllUpdates,
    /// Toggle the kebab menu on a plugin row (secondary actions:
    /// check for updates, auto-update override, uninstall).
    ShowPluginMenu(String),
    /// Manifest fetch finished, `Ok` carries the parsed manifest.
    PluginManifestFetched(String, Result<Box<crate::plugins::PluginManifest>, String>),
    /// Open / close the first-use install opt-in modal for a provider.
    ShowPluginInstallModal(String),
    HidePluginInstallModal,
    /// Begin downloading + installing the best compatible version.
    PluginInstall(String),
    /// Install finished, `Ok` carries the installed version string.
    PluginInstallDone(String, Result<String, String>),
    /// Remove a provider's cached binaries.
    PluginUninstall(String),
    /// Confirmed from the uninstall dialog: actually remove the
    /// cached binaries (and the MCP launcher copy for `mcp`).
    PluginUninstallConfirmed(String),

    /// A CJK font (Korean / Chinese / Japanese) finished downloading or
    /// was read from cache; `Ok` carries the font bytes to hand to
    /// `iced::font::load`. Carries the language code so the in-memory
    /// "already loaded" guard can be cleared on failure for a retry.
    CjkFontReady(String, Result<Vec<u8>, String>),

    // Edit dynamic group panel, sets template fields (key, identity,
    // transport, initial command) on a `Group.cloud_query`.
    EditDynamicGroup(Uuid),
    HideDynamicGroupForm,
    DynamicGroupFormUsernameChanged(String),
    DynamicGroupFormInitialCommandChanged(String),
    DynamicGroupFormTransportChanged(oryxis_core::models::cloud::TransportKind),
    DynamicGroupFormKeyChanged(String),
    DynamicGroupFormIdentityChanged(String),
    SaveDynamicGroup,
    DeleteDynamicGroup(Uuid),
    /// ⋮ menu on a dynamic-group card.
    ShowDynamicGroupCardMenu(Uuid),
    DynamicGroupCardHovered(Uuid),
    DynamicGroupCardUnhovered,

    // Connection identity
    EditorIdentityChanged(String),

    // AI settings

    // Vault password management

    // AI chat sidebar

    // Port forwarding
    EditorAddPortForward,
    EditorRemovePortForward(usize),
    EditorPortFwdLocalPortChanged(usize, String),
    EditorPortFwdRemoteHostChanged(usize, String),
    EditorPortFwdRemotePortChanged(usize, String),
    EditorAddEnvVar,
    EditorRemoveEnvVar(usize),
    EditorEnvVarKeyChanged(usize, String),
    EditorEnvVarValueChanged(usize, String),

    // SSH agent forwarding (per-host opt-in)
    EditorToggleAgentForwarding,

    // MCP
    EditorToggleMcpEnabled,
    /// Cycle the per-host session-recording override: Default -> On -> Off.
    EditorCycleSessionLogging,
    ToggleMcpServer,
    ShowMcpInfo,
    HideMcpInfo,
    CopyMcpConfig,
    InstallMcpConfig,
    InstallMcpConfigResult(Result<String, String>),
    /// Pick which client the snippet, Copy, and Install target: the
    /// native client (`false`) or one running inside WSL (`true`).
    /// Only the Windows build renders the toggle that emits this, so
    /// elsewhere the variant is constructed nowhere.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    SetMcpTarget(bool),
    /// Generate a fresh random MCP server token and persist it. Wipes
    /// the previous value, every existing MCP config will need to be
    /// reissued (re-copy / re-install) with the new token.
    RegenerateMcpToken,
    /// Show / hide the MCP token in plain text on the settings panel.
    /// Default masked.
    ToggleMcpTokenVisibility,
    /// Put the active MCP token on the clipboard. Logs nothing, the
    /// toast tells the user it was copied.
    CopyMcpToken,
    /// Open the master-password confirm row in the MCP setup panel
    /// (consent gate for embedding `ORYXIS_VAULT_PASSWORD` in the
    /// client config).
    McpVaultPwPromptOpen,
    /// Close the confirm row, discarding whatever was typed.
    McpVaultPwPromptCancel,
    /// Keystrokes into the master-password confirm input.
    McpVaultPwInput(String),
    /// Verify the typed master password; on success persist the
    /// consent flag so the snippet / Copy / Install embed the vault
    /// password from then on.
    McpVaultPwConfirm,
    /// Withdraw the consent: the snippet / Copy / Install stop
    /// embedding the vault password; the config(s) on disk are scrubbed
    /// in place in the background.
    McpVaultPwRemove,
    /// Outcome of that background scrub: `Ok(())` when every config that
    /// carried the password was rewritten without it, `Err(msg)` on a
    /// rewrite failure.
    McpVaultPwStripResult(Result<(), String>),

    // Sync
    Sync(SyncMessage),

    // Export / Import
    // Export / import / share (handle_share)
    Share(ShareMessage),

    // System tray (Windows only at runtime; messages compile on
    // every platform so dispatch.rs and subscription.rs stay cfg-
    // free).
    /// 100 ms ticker emitted by the iced subscription. The handler
    /// drains the tray-icon crate's crossbeam event channels and
    /// re-emits real `TrayShow / TrayHide / TrayQuit` messages.
    /// Polling here is acceptable noise (~10 ticks/sec, each a
    /// non-blocking `try_recv`) and avoids wiring a custom
    /// Subscription stream that bridges crossbeam into iced.
    /// The ticker only mounts on Windows (the tray lives there),
    /// hence the cfg'd allow.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    TrayPoll,
    /// User clicked "Show Oryxis" in the tray menu, or left-clicked
    /// the tray icon. Bring the main window back from hidden state
    /// and pull it to the foreground.
    TrayShow,
    /// User clicked "Hide to tray". Hide the main window (true
    /// hide via Win32 ShowWindow, not just minimize) and leave
    /// only the tray icon present.
    TrayHide,
    /// User clicked "Quit" in the tray menu. Tear down the tray
    /// icon and exit the process.
    TrayQuit,
    /// User clicked an entry in the tray menu's "Active sessions"
    /// section. Payload is the tab index from `Oryxis::tabs`. The
    /// handler shows the window (in case it was hidden) and selects
    /// the tab.
    TrayActivateSession(usize),
    /// User clicked an entry in the tray menu's "Recent hosts"
    /// section. Payload is the connection UUID. The handler shows
    /// the window and opens a new tab against that connection.
    TrayOpenHost(uuid::Uuid),

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
