//! The AI chat surface outside the tab: the saved conversations the
//! History screen lists, the reader open over one of them, and the
//! sidebar's own editor and geometry.
//!
//! The turns themselves live on the tab (they belong to a session);
//! this is the chrome around them.


pub(crate) struct ChatUi {
    /// Saved AI conversations, newest first. Shares the History timeline
    /// with the recordings (see `TimelineKind::Chat`).
    pub(crate) conversations: Vec<oryxis_vault::ChatConversationEntry>,
    /// The conversation open in the reader, with its turns loaded.
    /// `None` when the reader is closed.
    pub(crate) viewer: Option<crate::state::ChatViewer>,
    // AI chat sidebar
    pub(crate) input: iced::widget::text_editor::Content,
    // Per-conversation stream state (`chat_loading` + `chat_task`) lives on
    // `TerminalTab` now, so a chat on one tab keeps running while the user
    // works in another and Stop / close / reset target the right tab.
    /// True when the user's scroll is anchored at (or very near) the bottom
    /// of the chat history, used to decide whether new assistant messages
    /// should auto-scroll. If the user has scrolled up to read older
    /// content, we leave them where they are.
    pub(crate) scroll_at_bottom: bool,
    /// User-resizable width of the chat sidebar in pixels.
    pub(crate) sidebar_width: f32,
    /// Some((cursor_x_at_drag_start, sidebar_width_at_drag_start)) while
    /// the user is dragging the resize handle on the sidebar's left edge.
    pub(crate) sidebar_drag: Option<(f32, f32)>,
}

impl Default for ChatUi {
    fn default() -> Self {
        Self {
            conversations: Vec::new(),
            viewer: None,
            input: iced::widget::text_editor::Content::new(),
            // A fresh chat is at the bottom by definition: there is
            // nothing above the first turn to have scrolled away from.
            scroll_at_bottom: true,
            sidebar_width: 350.0,
            sidebar_drag: None,
        }
    }
}
