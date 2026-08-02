//! Terminal pane / PTY / input / search messages.

use iced::keyboard;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum TerminalMessage {
    PtyOutput(Uuid, Vec<u8>),  // (pane_id, bytes)
    /// One-shot wake-up that force-flushes a stalled DEC `?2026`
    /// synchronized update on the given pane (`pane_id`). Armed by the
    /// `PtyOutput` handler when output stops mid-update; without it an app
    /// that opens a sync update and blocks on input freezes the screen.
    TerminalSyncFlush(Uuid),
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
    /// Close a pane (closes its tab if it was the tab's last one).
    /// `Some(pane_id)` targets that exact pane, re-resolved at dispatch
    /// time (the context-menu row: focus and the active tab can change
    /// via hotkeys while the menu overlay is open, and the pane may be
    /// gone entirely, a safe no-op). `None` closes the focused pane of
    /// the active tab (the hotkey path).
    ClosePane(Option<Uuid>),
    /// Move focus to the adjacent pane in a direction (keyboard nav).
    FocusPaneDir(iced::widget::pane_grid::Direction),
    /// Expand the focused pane to the whole tab, and back. `None` targets
    /// the active tab (hotkey); `Some(idx)` a specific one (tab menu).
    ToggleMaximizePane(Option<usize>),
    /// Periodic flush of buffered session-log output to the vault.
    SessionLogFlushTick,
    /// Emitted by the terminal widget when the user right-clicks. The
    /// dispatcher reads the clipboard and routes the text to the SSH
    /// session (if active) or the local PTY, mirroring Ctrl+Shift+V.
    TerminalPasteFromClipboard,
    /// Clipboard text handed back by the runtime for a pending paste, with
    /// the tab index the paste was requested FROM (`None` text = empty or
    /// unavailable). Every paste path funnels through here: the runtime is
    /// the only clipboard reader in the process, so a second concurrent read
    /// can't corrupt the heap (see `oryxis_terminal::host_clipboard`). The
    /// tab rides along because the read resolves later and the user may have
    /// switched tabs in between.
    TerminalPasteResolved(usize, Option<String>),
    /// Careful-paste confirmation: send the multi-line text held in
    /// `pending_paste` to the tab it was captured for (not the currently
    /// active one, which may have changed since).
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
    /// Open the terminal context menu for a pane at a window-absolute
    /// point (right-click scheme = Menu). `(pane_id, x, y, selection)`,
    /// where `selection` is the live selection's text captured by the
    /// widget (`None` when empty), so the menu can offer "Copy".
    ShowTerminalContextMenu(Uuid, f32, f32, Option<String>),
    /// Copy the captured selection text to the clipboard (context-menu
    /// "Copy").
    TerminalCopySelection(String),
    /// Paste the X11 PRIMARY selection into a pane: middle-click, the
    /// paste-selection action, or the context-menu row. `(pane_id, text)`;
    /// the pane is explicit because every sender knows it (the widget
    /// captures its own at build time, the menu carries the right-clicked
    /// one) and the focused pane can change before this is handled. Never
    /// touches the system clipboard.
    TerminalPasteSelection(Uuid, String),
    /// Flush the buffered OS drop (a multi-file drop arrives as one
    /// FileDropped per file): resolve the target pane and route the
    /// batch to its transport. Fired by a short debounce after the
    /// first file of the gesture.
    TerminalDropFlush,
    /// The OS-drop SFTP upload task streamed a progress event for a
    /// pane. Terminal events (Done / Failed / Cancelled) clear the
    /// pane's card and toast the outcome.
    TerminalDropProgress(Uuid, crate::state::DropProgress),
    /// User asked to cancel the pane's in-flight OS-drop upload.
    TerminalDropCancel(Uuid),
    /// Copy the whole buffer (scrollback + screen) of a pane to the
    /// clipboard (context-menu "Copy All"). `pane_id`.
    TerminalCopyAll(Uuid),
    /// Drop a pane's scrollback history (context-menu "Clear
    /// Scrollback"). `pane_id`.
    TerminalClearScrollback(Uuid),
    /// Clear a pane's visual-bell flash after its short display window.
    TerminalBellFlashEnd(Uuid),
}
