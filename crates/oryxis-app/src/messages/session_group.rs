//! Saved split-panel session-group entity: CRUD, the editor form and
//! card menu, wrapped by [`crate::messages::Message::SessionGroup`].

use iced::widget::text_editor;

#[derive(Debug, Clone)]
pub enum SessionGroupMessage {
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
    SessionGroupCardUnhovered(usize),
}
