//! History timeline + session-log recordings: paging, view/menu, delete, and .cast/transcript/GIF exports, wrapped by [`crate::messages::Message::History`]. Handled by `Oryxis::handle_history`.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum HistoryMessage {
    RequestClearHistory,
    CancelClearHistory,
    ClearLogs,
    LogsPageNext,
    LogsPagePrev,
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
    /// Right-click context menu over the transcript viewer body (scheme
    /// = Menu): window-absolute x/y and the selection captured by the
    /// widget at right-click. Read-only, so it only offers copy actions.
    ShowSessionViewerContextMenu(f32, f32, Option<String>),
    /// Copy the whole transcript from the viewer's emulator to the
    /// clipboard (the "Copy All" item on that context menu).
    SessionViewerCopyAll,
    /// Toggle the viewer-header `...` menu (session-log actions minus
    /// Play, which the viewer offers as its own header button).
    ShowSessionLogViewerMenu(usize),
    /// Ask for confirmation before deleting one recording; the
    /// dialog's action carries `DeleteSessionLog`.
    RequestDeleteSessionLog(usize),
    DeleteSessionLog(usize),
    /// Hover tracking for clickable session rows in the Logs view.
    LogRowHovered(Uuid),
    LogRowUnhovered,
    #[allow(dead_code)]
    ClearSessionLogs,
    #[allow(dead_code)]
    SessionLogsPageNext,
    #[allow(dead_code)]
    SessionLogsPagePrev,
    /// Copy the canonical ssh:// URL of the host at this index (card
    /// context-menu action).
    CopyHostSshUrl(usize),
}
