//! Snippet CRUD, tag filters, variable prompts and the built-in
//! sudo-password snippet, wrapped by [`crate::messages::Message::Snippet`].

use iced::widget::text_editor;

#[derive(Debug, Clone)]
pub enum SnippetMessage {
    ShowSnippetPanel,
    HideSnippetPanel,
    SnippetLabelChanged(String),
    /// Snippet editor: free-form group name field.
    SnippetGroupChanged(String),
    /// Snippet editor: comma-separated tags field.
    SnippetTagsChanged(String),
    /// Snippet editor: flip the install-script category (issue #147).
    ToggleSnippetInstall,
    /// Snippets sidebar: toggle "only snippets tagged like this host".
    ToggleSnippetTagFilter,
    /// Vault Snippets: open/close the tag-filter dropdown.
    ShowSnippetTagFilterMenu,
    /// Vault Snippets: toggle one tag in the multi-select filter.
    ToggleSnippetTagFilterTag(String),
    /// Vault Snippets: clear the tag filter.
    ClearSnippetTagFilter,
    /// Vault Snippets: open a snippet group as a folder.
    OpenSnippetGroup(String),
    /// Vault Snippets: back to the root (group cards + ungrouped).
    CloseSnippetGroup,
    /// Sidebar Snippets tab: open a group folder row.
    OpenSidebarSnippetGroup(String),
    /// Sidebar Snippets tab: back to the folder list.
    CloseSidebarSnippetGroup,
    /// Snippet-variables modal: edit the Nth placeholder's value.
    SnippetVarChanged(usize, String),
    /// Substitute the filled values and send the parked snippet.
    ConfirmSnippetVars,
    /// Drop the parked snippet without sending.
    CancelSnippetVars,
    /// Snippet editor: arm the shortcut recorder (next chord binds).
    SnippetHotkeyCaptureStart,
    /// Snippet editor: remove the custom shortcut.
    SnippetHotkeyClear,
    SnippetCommandAction(text_editor::Action),
    SaveSnippet,
    /// Open the kebab (⋮) context menu on a snippet card (Edit / Delete).
    ShowSnippetMenu(usize),
    EditSnippet(usize),
    /// Ask for confirmation before removing a snippet.
    RequestDeleteSnippet(usize),
    DeleteSnippet(usize),
    RunSnippet(usize),
    /// Inject a snippet's command into the active terminal WITHOUT a
    /// trailing newline (the user presses Enter), unlike `RunSnippet`.
    PasteSnippet(usize),
    /// Built-in "global snippet": type the active host's stored password
    /// then Enter into the terminal (e.g. to answer a sudo prompt). No-op
    /// with a toast when the host has no stored password.
    ApplySudoPassword,
}
