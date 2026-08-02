//! Overlay (floating context menus) (split out of `state.rs`).

use super::*;

#[derive(Debug, Clone)]
pub(crate) enum OverlayContent {
    HostActions(usize),
    /// Kebab / right-click menu on a session-group card. Items: Open, Edit,
    /// Duplicate, Delete.
    SessionGroupActions(usize),
    KeyActions(usize),
    IdentityActions(usize),
    /// Kebab menu on a snippet card. Items: Edit and Delete.
    SnippetActions(usize),
    /// Kebab menu on a port-forward rule card. Items: Edit and Delete.
    PortForwardActions(usize),
    KeychainAdd,
    TabActions(usize),
    /// Right-click menu on an SFTP browser tab. Items: New SFTP tab,
    /// Pin/Unpin, Close. `usize` is the `sftp_tabs` index.
    SftpTabActions(usize),
    /// Right-click menu on a sidebar Files row. Carries the entry's
    /// full remote path + kind; items: Open (dirs), Open SFTP session
    /// here, Copy path, Copy name (files).
    SidebarFilesRow { path: String, is_dir: bool },
    /// Right-click on the sidebar Files list's empty area: directory
    /// actions for the current folder.
    SidebarFilesBackground { dir: String },
    /// Hover popover under the `+` tab button: New Tab + Split actions for
    /// the active terminal tab.
    SplitMenu,
    FolderActions(Uuid),
    CloudProfileActions(Uuid),
    /// Kebab menu on a dynamic-group card (ECS / K8s service folder).
    /// Items: Edit (template) and Delete.
    DynamicGroupActions(Uuid),
    /// Dropdown menu rendered next to "+ Host", lists every
    /// configured cloud profile so the user can launch discovery
    /// directly from the Hosts view. Only opened when at least one
    /// profile is configured (otherwise the chevron is hidden).
    CloudProviderPicker,
    /// Dropdown next to the Snippets sort button: multi-select
    /// snippet-tag filter, mirroring `HostTagFilter`.
    SnippetTagFilter,
    /// Kebab menu on a History session row: Export .cast, Export
    /// transcript, Delete. `usize` is the `session_logs` index.
    SessionLogActions(usize),
    /// Kebab menu of a saved AI conversation row, by index into
    /// `Oryxis::chat_conversations`.
    ChatConversationActions(usize),
    /// Same session-log actions, opened from the viewer's header `...`
    /// button: the viewer has a dedicated Play button, so this variant
    /// renders the menu without the Play row.
    SessionLogViewerActions(usize),
    /// Dropdown next to the dashboard sort button: pick a host tag to
    /// filter the grid by (or clear the filter).
    HostTagFilter,
    /// Dropdown under the History-toolbar tag button: multi-select
    /// host-tag filter over the timeline rows, mirroring
    /// `HostTagFilter`.
    HistoryTagFilter,
    /// Floating context menu for the Discover import modal's
    /// "Import into" combo. Carries a search input + the full list
    /// of user groups. Rendered through the modal's local Stack
    /// (the global overlay path is short-circuited by the modal's
    /// early return).
    CloudDiscoverGroupPicker,
    /// Shared group-picker popover for side-panel Parent Group
    /// inputs. The target enum tells the dispatch which form field
    /// the picked value flows into so the same overlay machinery
    /// (search + list) serves both the host editor and the dynamic
    /// group editor without duplicate state.
    GroupPicker(GroupPickerTarget),
    /// Sort dropdown anchored to the toolbar sort button in one of
    /// the card-grid views (Hosts / Keychain / Snippets).
    SortMenu(SortMenuKind),
    /// Floating search field popped from the toolbar's search icon when
    /// the window is too narrow for an inline search box. Carries no
    /// payload: the field (id, value, on_input) is resolved from the
    /// active view, exactly like the inline `vault_search_field`.
    ToolbarSearch,
    /// Overflow `…` menu folding the active view's secondary toolbar
    /// actions (sort, view toggle, history pagination) when even the
    /// icon-collapsed search can't free enough room for them inline.
    ToolbarOverflow,
    /// Right-click context menu over a terminal pane (right-click scheme
    /// = Menu). Items: Copy (when a selection was live), Copy All, Paste,
    /// Clear Scrollback. Carries the pane id (so actions target the
    /// clicked pane) and the selection text captured by the widget at
    /// right-click (the app can't reach the widget's live selection).
    /// Position lives in `OverlayState.x/y` (window-absolute).
    TerminalContextMenu(Uuid, Option<String>),
    /// Right-click context menu over the session-log transcript viewer
    /// (issue #90, right-click scheme = Menu). Read-only, so the only
    /// items are Copy (when a selection was live) and Copy All; there is
    /// a single viewer at a time, so no id is carried. Selection text is
    /// captured by the widget at right-click. Position lives in
    /// `OverlayState.x/y` (window-absolute).
    SessionLogViewerContext(Option<String>),
    /// Kebab menu on a Plugins-panel row. Carries the provider id.
    /// Items depend on the row's status: check for updates, the
    /// auto-update override toggle, uninstall / remove downloads.
    PluginActions(String),
    /// Right-click menu on a Monitor-tab listening-port row (issue
    /// #96). Items: Forward this port locally (TCP only, since SSH has
    /// no UDP forwarding), Kill process, Force kill. Carries the whole
    /// socket row so the confirmation describes the socket that was
    /// pointed at, not whatever a later sample holds.
    MonitorPortActions(Box<crate::monitor::model::PortStat>),
}

/// Which side-panel input the shared group picker is currently
/// driving. Each panel carries its own combo bounds cell so the
/// popover anchors precisely under the right chevron.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupPickerTarget {
    DynamicFormParent,
    SessionGroupFolder,
    /// Parent combo in the manual group editor side panel. The list
    /// excludes the edited group's own subtree (no cycles).
    GroupEditParent,
}

/// Host editor's startup-command source. `None` runs nothing; `Snippet`
/// seeds the command from a saved snippet (snapshotted into the command
/// text on save); `Custom` is the free-text editor. On reopen the choice
/// is recovered by matching the stored command against snippet bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupChoice {
    None,
    Custom,
    Snippet(uuid::Uuid),
}

/// Which list the open sort menu controls. Drives both the dispatched
/// `Set*Sort` message and the icon shown on the trigger button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortMenuKind {
    Hosts,
    Keys,
    Snippets,
}

#[derive(Debug, Clone)]
pub(crate) struct OverlayState {
    pub content: OverlayContent,
    pub x: f32,
    pub y: f32,
}
