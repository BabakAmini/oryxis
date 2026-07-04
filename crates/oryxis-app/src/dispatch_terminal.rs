//! `Oryxis::handle_terminal`, match arms for terminal I/O: PTY bytes
//! coming back, keyboard events routed to the active tab, mouse moves
//! tracked for hit-testing, window resize/drag/min/max/close.

#![allow(clippy::result_large_err)]

use iced::keyboard;
use iced::Task;

use crate::app::{Message, Oryxis};
use crate::util::{ctrl_key_bytes, key_to_named_bytes};

/// Flush a pane's recorded-output buffer to the vault once it reaches
/// this size, so a burst (e.g. an `apt upgrade` dump) doesn't sit in
/// RAM unbounded between the periodic flush ticks.
const SESSION_LOG_FLUSH_BYTES: usize = 64 * 1024;

/// Minimum gap between arrival marks for the flush to cut a separate
/// timed chunk (asciicast replay step). Bursty output (a compile, a
/// `find /`) coalesces into a few chunks per second instead of one
/// vault row per PTY read; interactive pauses stay visible.
const SESSION_LOG_SEGMENT_MS: i64 = 250;

/// Split a flushed buffer into timed replay segments: `(byte offset,
/// offset_ms)` per chunk, driven by the arrival marks. Cuts happen
/// ONLY at line boundaries, either the byte before the mark is `\n` /
/// `\r`, or the mark itself sits on a `\r` (a CR-prefixed progress-bar
/// redraw, wget style). Secret runs never contain `\n` or `\r`, so a
/// cut there can't split one across redaction chunks. And only marks
/// whose arrival gap is worth a replay step cut; bursty output
/// coalesces instead of producing one vault row per PTY read.
fn session_log_segments(head: &[u8], marks: &[(usize, i64)]) -> Vec<(usize, i64)> {
    let mut segs: Vec<(usize, i64)> = Vec::new();
    for &(pos, ms) in marks {
        match segs.last() {
            None => segs.push((0, ms)),
            Some(&(_, prev_ms)) => {
                let line_bounded = pos > 0
                    && pos < head.len()
                    && (matches!(head[pos - 1], b'\n' | b'\r') || head[pos] == b'\r');
                if line_bounded && ms - prev_ms >= SESSION_LOG_SEGMENT_MS {
                    segs.push((pos, ms));
                }
            }
        }
    }
    if segs.is_empty() {
        segs.push((0, 0));
    }
    segs
}

/// Largest cut `<= take` that doesn't split a trailing multi-byte
/// UTF-8 sequence (its continuation bytes may not have arrived yet).
/// Walks back over at most 3 continuation bytes; anything that doesn't
/// parse as UTF-8 (raw binary output) keeps the original cut, this is
/// a best-effort alignment, not a validator.
fn utf8_aligned(buf: &[u8], take: usize) -> usize {
    let mut cut = take;
    let floor = take.saturating_sub(3);
    while cut > floor && buf[cut - 1] & 0xC0 == 0x80 {
        cut -= 1;
    }
    if cut == 0 {
        return take;
    }
    let lead = buf[cut - 1];
    let need = match lead {
        b if b & 0x80 == 0x00 => 1,
        b if b & 0xE0 == 0xC0 => 2,
        b if b & 0xF0 == 0xE0 => 3,
        b if b & 0xF8 == 0xF0 => 4,
        // Stray continuation or invalid lead: not UTF-8, cut as-is.
        _ => return take,
    };
    if need > take - (cut - 1) {
        // The sequence is incomplete at the cut: flush up to its start.
        cut - 1
    } else {
        take
    }
}

impl Oryxis {
    /// Drain every pane's recorded-output buffer into the vault, one
    /// append per pane. Driven by the size threshold, the flush tick,
    /// disconnect, and window close, so the vault sees batched writes
    /// instead of one per SSH chunk (the old per-chunk path rewrote the
    /// whole growing blob and hammered the disk).
    ///
    /// Secrets/PII are scrubbed per flushed chunk (`session_redact`).
    /// Patterns can't match across chunk boundaries, so the periodic
    /// (non-final) flush holds back everything after the buffer's last
    /// line boundary (`\n` or `\r`; secret runs contain neither, so a
    /// cut there can't split one); the partial line rides along to the
    /// next flush unless the buffer is oversized anyway.
    pub(crate) fn flush_session_logs(&mut self) {
        self.flush_session_logs_inner(false);
    }

    /// Flush including trailing partial lines. Use when the pane, tab,
    /// session, or window is going away (or the log is about to be
    /// read), so the recorded tail isn't lost.
    pub(crate) fn flush_session_logs_final(&mut self) {
        self.flush_session_logs_inner(true);
    }

    /// Tear down every remote session (SSH or Telnet) in a tab.
    /// Dropping the pane alone is not enough: the connect stream task
    /// holds its own Arc to the session, so without an explicit
    /// close() the engine tasks, the channel, and any per-connection
    /// port-forward listeners keep running (and generating UI
    /// messages) forever.
    pub(crate) fn close_tab_sessions(tab: &crate::state::TerminalTab) {
        for pane in tab.pane_grid.panes.values() {
            if let Some(session) = &pane.session {
                session.close();
            }
        }
    }

    fn flush_session_logs_inner(&mut self, final_flush: bool) {
        // Full detail = timed segments + resize events (.cast export);
        // simple = one untimed chunk per flush, the plain log of old.
        let full = self.setting_session_log_full;
        let compress = self.setting_session_log_compress;
        // (log id, offset_ms, bytes) output chunks + pending resizes.
        let mut pending: Vec<(uuid::Uuid, Option<i64>, Vec<u8>)> = Vec::new();
        let mut resizes: Vec<(uuid::Uuid, i64, u16, u16)> = Vec::new();
        for tab in &mut self.tabs {
            for pane in tab.pane_grid.panes.values_mut() {
                if let Some(log_id) = pane.session_log_id
                    && !pane.session_log_buf.is_empty()
                {
                    let buf = &mut pane.session_log_buf;
                    let take = if final_flush {
                        buf.len()
                    } else if buf.len() >= SESSION_LOG_FLUSH_BYTES {
                        // Oversized burst: flush everything, but never
                        // split a multi-byte UTF-8 sequence across chunks
                        // (the .cast export decodes per chunk, so both
                        // halves of a split char would render as U+FFFD).
                        utf8_aligned(buf, buf.len())
                    } else {
                        // Hold back the partial trailing line so a secret
                        // mid-echo isn't split across redaction chunks.
                        // `\r` ends a line here too: progress-bar redraws
                        // are CR-delimited and would otherwise sit in RAM
                        // until a newline or the size threshold.
                        match buf.iter().rposition(|&b| b == b'\n' || b == b'\r') {
                            Some(pos) => pos + 1,
                            None => 0,
                        }
                    };
                    if take == 0 {
                        continue;
                    }
                    let tail = buf.split_off(take);
                    let head = std::mem::replace(buf, tail);
                    // Partition the arrival marks: the head's marks
                    // drive the timed segments below; the tail's are
                    // rebased, keeping a mark at 0 so carried-over
                    // bytes don't lose their arrival time.
                    let mut head_marks: Vec<(usize, i64)> = Vec::new();
                    let mut tail_marks: Vec<(usize, i64)> = Vec::new();
                    for (pos, ms) in pane.session_log_marks.drain(..) {
                        if pos < take {
                            head_marks.push((pos, ms));
                        } else {
                            tail_marks.push((pos - take, ms));
                        }
                    }
                    if !pane.session_log_buf.is_empty()
                        && tail_marks.first().is_none_or(|m| m.0 > 0)
                    {
                        let carry_ms = head_marks.last().map(|m| m.1).unwrap_or(0);
                        tail_marks.insert(0, (0, carry_ms));
                    }
                    pane.session_log_marks = tail_marks;
                    if full {
                        let segs = session_log_segments(&head, &head_marks);
                        for (i, &(start, ms)) in segs.iter().enumerate() {
                            let end = segs.get(i + 1).map(|s| s.0).unwrap_or(head.len());
                            pending.push((log_id, Some(ms), head[start..end].to_vec()));
                        }
                    } else {
                        // Simple mode: one untimed chunk, no replay
                        // metadata (NULL offset = the legacy shape).
                        pending.push((log_id, None, head));
                    }
                    // Geometry check rides the flush cadence: cheap, and
                    // a resize between flushes still lands close enough
                    // for replay. The first flush records the initial
                    // size (the asciicast header reads it back). Replay
                    // metadata, so full-detail mode only.
                    let size = full
                        .then(|| pane.terminal.lock().ok())
                        .flatten()
                        .map(|s| (s.cols(), s.rows()))
                        .filter(|&(c, r)| c > 0 && r > 0);
                    if let Some((cols, rows)) = size
                        && pane.session_log_last_size != Some((cols, rows))
                    {
                        pane.session_log_last_size = Some((cols, rows));
                        let now_ms = pane
                            .session_log_t0
                            .map(|t| t.elapsed().as_millis() as i64)
                            .unwrap_or(0);
                        resizes.push((log_id, now_ms, cols, rows));
                    }
                }
            }
        }
        if pending.is_empty() && resizes.is_empty() {
            return;
        }
        if let Some(vault) = &self.vault {
            for (log_id, offset_ms, bytes) in pending {
                let scrubbed = crate::session_redact::redact_secrets(&bytes);
                if let Err(e) =
                    vault.append_session_data(&log_id, &scrubbed, offset_ms, compress)
                {
                    tracing::warn!("session log append failed for {log_id}: {e}");
                }
            }
            for (log_id, offset_ms, cols, rows) in resizes {
                if let Err(e) = vault.append_session_resize(&log_id, offset_ms, cols, rows) {
                    tracing::warn!("session resize append failed for {log_id}: {e}");
                }
            }
        }
    }

    /// Paste `text` into the active tab's session. Careful-paste gate:
    /// when the setting is on (default) and the text contains a line
    /// break, the paste is parked in `pending_paste` and a confirmation
    /// dialog (line count + preview) takes over, so a hidden trailing
    /// newline can't auto-run a command. Single-line pastes, and every
    /// paste when the guard is off, go straight to the session.
    pub(crate) fn paste_text_into_active(&mut self, text: &str) {
        if self.active_tab.is_none() {
            return;
        }
        // Two independent gates (owner call: each has its own setting):
        // careful paste parks multi-line text; the paste guard parks
        // suspicious CONTENT (bidi/invisible chars, raw control bytes,
        // curl|sh one-liners, homograph tokens) even on one line.
        if (self.setting_careful_paste
            && (text.contains('\n') || text.contains('\r')))
            || (self.setting_paste_guard
                && !crate::paste_guard::paste_warnings(text).is_empty())
        {
            self.pending_paste = Some(text.to_string());
            return;
        }
        self.write_paste_to_active(text);
    }

    /// Write `text` into the active tab's session, wrapping it for
    /// bracketed-paste when the focused app enabled it (`\e[?2004h`).
    /// Routes to the SSH session when one is attached, otherwise the
    /// local PTY. Shared by the clipboard (right-click / Ctrl+Shift+V)
    /// paste paths and the careful-paste confirmation.
    pub(crate) fn write_paste_to_active(&mut self, text: &str) {
        if let Some(tab_idx) = self.active_tab {
            let Some(tab) = self.tabs.get(tab_idx) else {
                return;
            };
            let bracketed = tab
                .active()
                .terminal
                .lock()
                .map(|s| s.bracketed_paste_enabled())
                .unwrap_or(false);
            let payload = oryxis_terminal::wrap_paste(text, bracketed);
            self.write_input_to_tab(tab_idx, &payload);
        }
    }

    /// Read the system clipboard and paste it into the active session.
    /// Shared by the Ctrl+V, Ctrl+Shift+V and Cmd+V (macOS) key paths so
    /// the bracketed-paste handling lives in exactly one place.
    fn paste_clipboard_into_active(&mut self) {
        if let Ok(mut clip) = arboard::Clipboard::new()
            && let Ok(text) = clip.get_text()
        {
            self.paste_text_into_active(&text);
        }
    }

    pub(crate) fn handle_terminal(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // -- Split panes --
            Message::FocusPane(pane) => {
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.focused = pane;
                }
                // Clicking a terminal pane takes the keyboard back from the
                // sidebar ring (see write_input_to_tab for the rationale),
                // and drops the dropdown gate defensively: a click outside
                // an open pick_list normally fires on_close, but if the
                // widget unmounted first the stuck flag would swallow
                // Enter/Space/Esc/arrows forever.
                self.keynav.sidebar_selected = None;
                self.keynav.pick_open = false;
                // The History tab is per-host; follow the focused pane.
                if self.terminal_sidebar_tab == crate::state::TerminalSidebarTab::History {
                    self.refresh_command_history();
                }
            }
            Message::ResizePane(ev) => {
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.pane_grid.resize(ev.split, ev.ratio);
                }
            }
            Message::SplitPane(axis) => {
                // Open the connection picker to choose what fills the new
                // pane (a host, or a local shell). The selection routes into
                // a split via `pending_pane_split` instead of a new tab.
                self.overlay = None; // dismiss the `+` hover popover if open
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get(tab_idx)
                {
                    self.pending_pane_split = Some((tab_idx, tab.focused, axis));
                    self.show_new_tab_picker = true;
                    self.new_tab_picker_search.clear();
                    self.new_tab_picker_group = None;
                }
            }
            Message::SplitTabPane(tab_idx, axis) => {
                // From a tab's right-click menu: focus that tab first, then
                // open the picker to fill the new split pane.
                self.overlay = None;
                if let Some(tab) = self.tabs.get(tab_idx) {
                    let target = tab.focused;
                    self.active_tab = Some(tab_idx);
                    self.active_view = crate::state::View::Terminal;
                    self.remember_terminal_tab_focus(tab_idx);
                    self.pending_pane_split = Some((tab_idx, target, axis));
                    self.show_new_tab_picker = true;
                    self.new_tab_picker_search.clear();
                    self.new_tab_picker_group = None;
                }
            }
            Message::ClosePane => {
                let Some(tab_idx) = self.active_tab else {
                    return Ok(Task::none());
                };
                // Last pane in the tab: closing it closes the whole tab.
                if self.tabs[tab_idx].pane_grid.panes.len() <= 1 {
                    return Ok(self.update(Message::CloseTab(tab_idx)));
                }
                // Persist the closing pane's recorded output before it goes.
                self.flush_session_logs_final();
                let tab = &mut self.tabs[tab_idx];
                let target = tab.focused;
                // Tear down the pane's remote session (the connect stream
                // holds its own Arc; see close_tab_sessions).
                if let Some(pane) = tab.pane_grid.panes.get(&target)
                    && let Some(session) = &pane.session
                {
                    session.close();
                }
                if let Some((_closed, sibling)) = tab.pane_grid.close(target) {
                    tab.focused = sibling;
                }
                // Drop quick-connect entries (and their in-memory
                // credentials) that no pane references anymore.
                self.prune_quick_connects();
            }
            Message::FocusPaneDir(dir) => {
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                    && let Some(adj) = tab.pane_grid.adjacent(tab.focused, dir)
                {
                    tab.focused = adj;
                }
            }
            // -- Terminal I/O --
            Message::PtyOutput(pane_id, mut bytes) => {
                // ── ZMODEM interception (before any emulator processing) ──
                // While a transfer owns the pane, output is protocol wire:
                // hand it to the driver and stop. Otherwise the initiation
                // detector runs; a detected `sz`/`rz` starts a transfer and
                // the clean prefix still flows to the emulator below.
                let mut zmodem_start: Option<(oryxis_zmodem::Direction, Vec<u8>)> = None;
                if let Some(pane) = self.pane_by_id_mut(pane_id) {
                    if let Some(zm) = pane.zmodem.as_ref() {
                        let _ = zm.wire_tx.send(std::mem::take(&mut bytes));
                        return Ok(Task::none());
                    }
                    let scan = pane.zmodem_detector.feed(&bytes);
                    // Only divert when the pane has a transport to run the
                    // protocol on; a local shell keeps the bytes on screen.
                    if let Some(direction) = scan.detection
                        && pane.session.is_some()
                    {
                        zmodem_start = Some((direction, scan.wire));
                        bytes = scan.clean;
                    } else if scan.detection.is_some() {
                        bytes = scan.clean;
                        bytes.extend(scan.wire);
                    } else {
                        bytes = scan.clean;
                    }
                }
                // Route to the specific pane (a tab may have several, each
                // with its own PTY). Scan is trivial at these counts.
                let mut over_threshold = false;
                let mut schedule_flush: Option<std::time::Duration> = None;
                // Snapshot the (Copy) bell mode before borrowing self.tabs; the
                // bell action runs while the pane is borrowed.
                let bell_mode = self.setting_bell_mode;
                // Notification policy + focus snapshot before the tabs borrow.
                let notif_mode = self.setting_notification_mode;
                let win_focused = self.window_focused;
                let mut flash_pane: Option<uuid::Uuid> = None;
                let mut pending_notification: Option<String> = None;
                let capture_enabled = self.setting_command_history;
                let log_full = self.setting_session_log_full;
                let mut captured_cmds: Vec<(uuid::Uuid, String)> = Vec::new();
                if let Some(pane) = self
                    .tabs
                    .iter_mut()
                    .flat_map(|t| t.pane_grid.panes.values_mut())
                    .find(|p| p.id == pane_id)
                {
                    let mut sync_deadline = None;
                    let mut new_title = None;
                    let mut bell_rang = false;
                    let mut new_cwd = None;
                    let mut new_notification = None;
                    let mut new_progress = None;
                    if let Ok(mut state) = pane.terminal.lock() {
                        state.process(&bytes);
                        // A buffering DEC ?2026 update reports its abort
                        // deadline here; read it while still locked.
                        sync_deadline = state.sync_timeout();
                        // OSC 0/2 title set by the shell this batch (or an
                        // empty string for ResetTitle). Captured unconditionally;
                        // the auto-title setting only gates display.
                        new_title = state.take_title();
                        bell_rang = state.take_bell();
                        // OSC 7 working directory.
                        new_cwd = state.take_cwd();
                        // OSC 133 shell-integration marks drive the pane's
                        // prompt state (the command-history capture gate) and
                        // resolve captures deferred until the echo arrived.
                        // Applied inside the lock: the marks' grid positions
                        // refer to rows this very batch drew.
                        let new_marks = state.take_shell_marks();
                        if !new_marks.is_empty() {
                            let cmds = crate::command_capture::observe_output_marks(
                                &mut pane.prompt,
                                &mut pane.pending_capture,
                                &state,
                                &new_marks,
                            );
                            if capture_enabled
                                && let crate::state::PaneOrigin::Host(hid) = &pane.origin
                            {
                                captured_cmds.extend(cmds.into_iter().map(|c| (*hid, c)));
                            }
                        }
                        // OSC 9 notification text + OSC 9;4 progress.
                        new_notification = state.take_notification();
                        new_progress = state.progress();
                    }
                    // OSC 9;4 progress (state 0 = clear) drives the tab border.
                    pane.progress = new_progress.filter(|p| p.state != 0 && p.value > 0);
                    pending_notification = new_notification;
                    if let Some(cwd) = new_cwd {
                        pane.cwd = Some(cwd);
                    }
                    if let Some(title) = new_title {
                        // Stored raw: when auto-title is on it's opt-in emulator
                        // behavior, so the tab shows exactly what the shell set
                        // (`user@host: ~`, `vim file`, …), like gnome-terminal /
                        // iTerm / Windows Terminal do.
                        let trimmed = title.trim();
                        pane.osc_title = (!trimmed.is_empty()).then(|| trimmed.to_string());
                    }
                    if bell_rang {
                        match bell_mode {
                            crate::util::BellMode::Off => {}
                            crate::util::BellMode::Beep => crate::util::play_system_beep(),
                            crate::util::BellMode::Flash => {
                                pane.bell_flash = true;
                                flash_pane = Some(pane_id);
                            }
                        }
                    }
                    // Rising edge only: arm one flush timer per update, not
                    // one per coalesced output batch. The flag clears when the
                    // update closes normally (deadline gone) or when the
                    // `TerminalSyncFlush` handler fires.
                    match sync_deadline {
                        Some(deadline) if !pane.sync_flush_scheduled => {
                            pane.sync_flush_scheduled = true;
                            schedule_flush = Some(deadline.saturating_duration_since(
                                std::time::Instant::now(),
                            ));
                        }
                        None => pane.sync_flush_scheduled = false,
                        _ => {}
                    }
                    // Buffer the bytes; the vault write is batched (see
                    // `flush_session_logs`). Flush early once the buffer
                    // grows large so a burst doesn't balloon in RAM.
                    // Each batch leaves an arrival mark so the flush can
                    // stamp real replay timing onto the stored chunks.
                    if pane.session_log_id.is_some() {
                        // Arrival marks are replay metadata: full-detail
                        // recording only. Simple mode just buffers bytes
                        // (the flush stores one untimed chunk).
                        if log_full {
                            let t0 = *pane
                                .session_log_t0
                                .get_or_insert_with(std::time::Instant::now);
                            pane.session_log_marks.push((
                                pane.session_log_buf.len(),
                                t0.elapsed().as_millis() as i64,
                            ));
                        }
                        pane.session_log_buf.extend_from_slice(&bytes);
                        over_threshold =
                            pane.session_log_buf.len() >= SESSION_LOG_FLUSH_BYTES;
                    }
                }
                for (host, cmd) in captured_cmds {
                    self.record_command_history(host, cmd);
                }
                if over_threshold {
                    self.flush_session_logs();
                }
                // OSC 9 notification. The in-app toast is a gentle cue and
                // fires regardless of focus (also useful for a background tab);
                // the OS notification only fires while the window is unfocused
                // (a native popup for the thing you're already watching is just
                // noise) and falls back to a toast if the native call fails (no
                // daemon / no AppUserModelID on a non-installed Windows build).
                let mut toast_shown = false;
                if let Some(text) = pending_notification {
                    let body = text.trim();
                    if !body.is_empty() {
                        let show_toast = match notif_mode {
                            crate::util::NotificationMode::Off => false,
                            crate::util::NotificationMode::Toast => true,
                            crate::util::NotificationMode::Os => {
                                !win_focused
                                    && !crate::util::show_os_notification("Oryxis", body)
                            }
                        };
                        if show_toast {
                            self.toast = Some(body.to_string());
                            // Auto-dismiss on a timer only when the window is
                            // focused (you see it now). A toast raised while
                            // unfocused is left up and cleared shortly after you
                            // return (WindowFocusChanged), so it isn't gone
                            // before you look.
                            toast_shown = win_focused;
                        }
                    }
                }
                // Session-group per-pane startup script for LOCAL panes. SSH
                // panes inject on `SshConnected`, but a local shell has no
                // such ready event, so we gate on its first output (the
                // prompt) to be sure the shell is reading stdin.
                if self.pane_script_overrides.contains_key(&pane_id) {
                    let is_local = self
                        .tabs
                        .iter()
                        .flat_map(|t| t.pane_grid.panes.values())
                        .find(|p| p.id == pane_id)
                        .map(|p| matches!(p.origin, crate::state::PaneOrigin::Local(_)))
                        .unwrap_or(false);
                    if is_local
                        && let Some(script) = self.pane_script_overrides.remove(&pane_id)
                        && let Some(pane) = self
                            .tabs
                            .iter()
                            .flat_map(|t| t.pane_grid.panes.values())
                            .find(|p| p.id == pane_id)
                        && let Ok(mut state) = pane.terminal.lock()
                    {
                        state.write(format!("{script}\n").as_bytes());
                    }
                }
                // Arm the one-shot flush for a synchronized update that
                // stalled with output buffered. Fires `flush_sync` at the
                // 150 ms deadline so a never-closed `?2026` can't leave the
                // screen frozen (see `TerminalSyncFlush`).
                let mut tasks: Vec<iced::Task<Message>> = Vec::new();
                // A detected ZMODEM transfer starts after the clean prefix
                // has been drawn: it seizes the pane and streams progress.
                if let Some((direction, wire)) = zmodem_start {
                    tasks.push(self.begin_zmodem_transfer(pane_id, direction, wire));
                }
                if let Some(remaining) = schedule_flush {
                    tasks.push(Task::perform(
                        async move {
                            tokio::time::sleep(remaining).await;
                        },
                        move |_| Message::TerminalSyncFlush(pane_id),
                    ));
                }
                if let Some(fp) = flash_pane {
                    // Clear the visual-bell flash after a brief window.
                    tasks.push(Task::perform(
                        async move {
                            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                        },
                        move |_| Message::TerminalBellFlashEnd(fp),
                    ));
                }
                if toast_shown {
                    // Auto-dismiss the fallback notification toast.
                    tasks.push(Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                        },
                        |_| Message::ToastClear,
                    ));
                }
                if !tasks.is_empty() {
                    return Ok(Task::batch(tasks));
                }
            }
            Message::TerminalBellFlashEnd(pane_id) => {
                if let Some(pane) = self
                    .tabs
                    .iter_mut()
                    .flat_map(|t| t.pane_grid.panes.values_mut())
                    .find(|p| p.id == pane_id)
                {
                    pane.bell_flash = false;
                }
            }
            Message::TerminalSyncFlush(pane_id) => {
                if let Some(pane) = self
                    .tabs
                    .iter_mut()
                    .flat_map(|t| t.pane_grid.panes.values_mut())
                    .find(|p| p.id == pane_id)
                {
                    pane.sync_flush_scheduled = false;
                    let mut reschedule: Option<std::time::Duration> = None;
                    if let Ok(mut state) = pane.terminal.lock() {
                        match state.sync_timeout() {
                            // The app extended the update past our deadline
                            // (a fresh BSU reset vte's 150 ms timer): re-arm
                            // for the new deadline instead of flushing
                            // mid-update, matching alacritty's behavior.
                            Some(deadline) if deadline > std::time::Instant::now() => {
                                reschedule = Some(deadline.saturating_duration_since(
                                    std::time::Instant::now(),
                                ));
                            }
                            // Deadline reached, update still open: force the
                            // buffered frame onto the grid.
                            Some(_) => state.flush_sync(),
                            // Closed normally in the meantime; nothing to do.
                            None => {}
                        }
                    }
                    if let Some(remaining) = reschedule {
                        pane.sync_flush_scheduled = true;
                        return Ok(Task::perform(
                            async move {
                                tokio::time::sleep(remaining).await;
                            },
                            move |_| Message::TerminalSyncFlush(pane_id),
                        ));
                    }
                }
            }
            // Periodic batched write of recorded output. Only mounted by
            // the subscription while at least one pane is recording.
            Message::SessionLogFlushTick => {
                self.flush_session_logs();
            }
            // Right-click paste from the terminal widget. Mirrors the
            // Ctrl+Shift+V path below: SSH session if active, local PTY
            // otherwise. Without this, the widget's fallback write only
            // reached the local PTY and right-click looked broken on
            // every SSH tab.
            Message::TerminalPasteFromClipboard => {
                if let Ok(mut clip) = arboard::Clipboard::new()
                    && let Ok(text) = clip.get_text()
                {
                    self.paste_text_into_active(&text);
                }
            }
            // Careful-paste confirmation: release the parked multi-line
            // text into the session, or drop it.
            Message::ConfirmPendingPaste => {
                if let Some(text) = self.pending_paste.take() {
                    self.write_paste_to_active(&text);
                }
            }
            Message::CancelPendingPaste => {
                self.pending_paste = None;
            }
            // Synthesized input from the terminal widget: mouse-tracking
            // reports (tmux `mouse on`, vim `mouse=a`, htop, ...) and the
            // wheel-to-arrow translation in alt-screen. Same SSH-or-local
            // routing as keystrokes; without this the widget's local-PTY
            // fallback would never reach the remote session.
            Message::TerminalInput(bytes) => {
                if let Some(tab_idx) = self.active_tab {
                    self.write_input_to_tab(tab_idx, &bytes);
                }
            }
            Message::TerminalMouseCaptureHint => {
                // Mark the focused pane so HintMode::Once retires the hint
                // (harmless under Always, where the view ignores the flag).
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.active_mut().mouse_hint_shown = true;
                }
                // Longer dwell than the default toast: this one is a sentence
                // to read, not a one-word "Copied" confirmation.
                return Ok(self.show_toast_secs(crate::i18n::t("mouse_capture_hint").to_string(), 5));
            }
            Message::TerminalLinkClickHint => {
                // Plain click on a link without Ctrl: teach the gesture with
                // a toast at the moment it missed (replaces the old hover
                // tooltip). Mark the focused pane so HintMode::Once retires
                // it (harmless under Always, where the view ignores the flag).
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.active_mut().link_hint_shown = true;
                }
                // Same longer dwell as the mouse-capture hint: a sentence.
                return Ok(self.show_toast_secs(crate::i18n::t("terminal_link_hint").to_string(), 5));
            }
            Message::KeyboardEvent(event) => {
                // Track modifier state for downstream consumers (SFTP
                // ctrl/shift-click selection). Always update first so
                // every later branch in this handler sees fresh state.
                if let keyboard::Event::ModifiersChanged(m) = &event {
                    self.modifiers = *m;
                }
                // Key-gate tracing (debug log): Ctrl+Shift chords are
                // always app hotkeys, so snapshot the gate-relevant
                // state on arrival. Field report 2026-07-03: the
                // FocusSidebarList chord worked on a local tab but not
                // on an SSH tab of the same build, and no static
                // difference between those paths exists; this line is
                // how the next report pinpoints the consuming gate.
                if let keyboard::Event::KeyPressed { key, modifiers, .. } = &event
                    && modifiers.control()
                    && modifiers.shift()
                {
                    tracing::debug!(
                        ?key,
                        view = ?self.active_view,
                        tab = ?self.active_tab,
                        pick_open = self.keynav.pick_open,
                        modal = self.any_modal_blocks_input(),
                        panel = self.show_host_panel,
                        capture = self.editing_hotkey.is_some(),
                        "key-gate: ctrl+shift chord"
                    );
                }
                // PrintScreen -> open the Windows snip overlay (region
                // capture), matching the OS default. winit delivers the
                // key to the focused window without forwarding it to
                // DefWindowProc, so Windows' own PrintScreen handler never
                // fires while Oryxis is focused; we remap it explicitly.
                // VK_SNAPSHOT classically emits only WM_KEYUP, so accept a
                // press or a release, debounced so a paired press+release
                // doesn't launch the overlay twice. Handled before the
                // modal / chat / PTY gates so a screenshot always works.
                #[cfg(target_os = "windows")]
                {
                    let is_printscreen = matches!(
                        &event,
                        keyboard::Event::KeyPressed {
                            key: keyboard::Key::Named(keyboard::key::Named::PrintScreen),
                            ..
                        } | keyboard::Event::KeyReleased {
                            key: keyboard::Key::Named(keyboard::key::Named::PrintScreen),
                            ..
                        }
                    );
                    if is_printscreen {
                        let now = std::time::Instant::now();
                        let recent = self
                            .last_printscreen
                            .map(|t| {
                                now.duration_since(t) < std::time::Duration::from_millis(400)
                            })
                            .unwrap_or(false);
                        if !recent {
                            self.last_printscreen = Some(now);
                            crate::util::open_screenshot_tool();
                        }
                        return Ok(Task::none());
                    }
                }
                // An open pick_list dropdown owns the keyboard: the
                // widget itself handles Enter/Space (confirm), Up/Down
                // (move the hovered option) and Esc (close). The global
                // subscription still delivers those keys here, so
                // swallow them or Esc would ALSO close the panel/modal
                // behind the dropdown and Enter would double-dispatch.
                // Tab is left alone (the widget closes the menu without
                // capturing so the focus chain still runs).
                if self.keynav.pick_open
                    && let keyboard::Event::KeyPressed { key, .. } = &event
                    && (matches!(
                        key,
                        keyboard::Key::Named(
                            keyboard::key::Named::Enter
                                | keyboard::key::Named::Space
                                | keyboard::key::Named::Escape
                                | keyboard::key::Named::ArrowUp
                                | keyboard::key::Named::ArrowDown
                        )
                    ) || matches!(key, keyboard::Key::Character(c) if c.as_str() == " "))
                {
                    return Ok(Task::none());
                }
                // Host editor panel open -> Tab / Shift+Tab move focus
                // between form fields like a browser, instead of falling
                // through to the PTY (which would emit a literal \t) or a
                // hotkey binding. focus_next / focus_previous walk iced's
                // real focus chain, so click-then-Tab works too.
                if self.side_panel_open()
                    && !self.any_modal_blocks_input()
                    && let keyboard::Event::KeyPressed { key, modifiers, .. } = &event
                    && matches!(key, keyboard::Key::Named(keyboard::key::Named::Tab))
                    && !modifiers.control()
                {
                    // Tab walks the panel's recorded rows: inputs get
                    // real focus, selects/toggles/buttons get the ring
                    // (the raw focus chain skipped everything that
                    // isn't a text input). Panels that record nothing
                    // fall back to the old focus-chain walk. See
                    // dispatch_keynav_panel.rs.
                    if let Some(task) = self.panel_nav_tab(!modifiers.shift()) {
                        return Ok(task);
                    }
                    return Ok(if modifiers.shift() {
                        iced::widget::operation::focus_previous()
                    } else {
                        iced::widget::operation::focus_next()
                    });
                }
                // Settings -> Security password forms open -> Tab /
                // Shift+Tab walk the password fields like a browser form,
                // instead of leaking a literal \t. Same focus-chain
                // mechanism as the host editor above. Covers both the
                // set-password and change-password forms.
                if self.active_view == crate::state::View::Settings
                    && self.settings_section == crate::state::SettingsSection::Security
                    && (self.vault_ui.show_password_form || self.vault_ui.change_password_open)
                    && let keyboard::Event::KeyPressed { key, modifiers, .. } = &event
                    && matches!(key, keyboard::Key::Named(keyboard::key::Named::Tab))
                {
                    return Ok(if modifiers.shift() {
                        iced::widget::operation::focus_previous()
                    } else {
                        iced::widget::operation::focus_next()
                    });
                }
                // Ctrl+Tab / Ctrl+Shift+Tab: switch tabs by last use, like the
                // OS Alt+Tab. A single repeated press toggles the two most
                // recent tabs; holding Ctrl and pressing Tab several times
                // walks further back through the recency stack, committing the
                // choice when Ctrl is released. Covers open tabs only (Home
                // stays on Ctrl+1 / Alt+arrow). Handled here rather than via the
                // configurable hotkey table so it works from any surface.
                // Consumed unconditionally so the combo never leaks a literal
                // \t into the PTY. See `tab_cycle.rs` for the MRU mechanics;
                // the run is committed by `reconcile_tab_mru` (Ctrl-release) and
                // `WindowFocusChanged` (focus lost mid-hold).
                if let keyboard::Event::KeyPressed { key, modifiers, .. } = &event
                    && matches!(key, keyboard::Key::Named(keyboard::key::Named::Tab))
                    && modifiers.control()
                    && !self.show_host_panel
                    && !self.any_modal_blocks_input()
                {
                    return Ok(self.cycle_mru_step(!modifiers.shift()));
                }
                // Modal / overlay-menu keyboard layer: while a
                // navigable dialog, dropdown or the burger menu is
                // open it owns movement + activation keys (Esc stays
                // with close_topmost_modal further down). See
                // dispatch_keynav_modal.rs.
                if let Some(task) = self.handle_modal_nav_key(&event) {
                    return Ok(task);
                }
                // Vault-area focus-zone navigation (Tab between zones,
                // arrows within, Enter activates, Esc back to idle).
                // Plain keys only, so Ctrl/Alt combos still reach the
                // hotkey table below; anything the router declines
                // falls through to it naturally. See dispatch_keynav.rs.
                if let Some(task) = self.handle_keynav_key(&event) {
                    return Ok(task);
                }
                // Terminal-sidebar navigation (all four tabs). Opt-in:
                // engaged by the FocusSidebarList hotkey, by Up/Down
                // while the cursor is over a list tab, or by Tab while
                // the cursor is over the sidebar; declines everything
                // else so the PTY keeps the keyboard (a plain Tab over
                // the terminal is still a literal \t). See
                // dispatch_keynav_sidebar.rs.
                if let Some(task) = self.handle_sidebar_nav_key(&event) {
                    return Ok(task);
                }
                // Hotkey dispatch + capture mode live in `shortcuts.rs`
                // (`handle_hotkey_keypress`). Returns a Task when the
                // event was consumed by a binding (or by the Settings
                // editor's capture mode), `None` to fall through to
                // the legacy PTY-routing block below.
                if let keyboard::Event::KeyPressed { key, modifiers, .. } = &event
                    && let Some(task) = self.handle_hotkey_keypress(key, modifiers)
                {
                    return Ok(task);
                }
                // When the AI chat sidebar is open and the cursor is over
                // it, the user is interacting with the textarea, drop the
                // event so it doesn't double-dispatch into the terminal
                // session running underneath.
                let cursor_in_chat_sidebar = self
                    .active_tab
                    .and_then(|i| self.tabs.get(i))
                    .map(|t| t.chat_visible)
                    .unwrap_or(false)
                    && self.mouse_position.x
                        > (self.window_size.width - self.chat_sidebar_width);
                if cursor_in_chat_sidebar {
                    return Ok(Task::none());
                }
                // A global picker / modal (new-tab picker, tab jump,
                // icon / theme / jump-host pickers, folder rename or
                // delete) owns the keyboard while open. Its own search
                // field consumes the keystroke via iced focus; without
                // this gate the same press also falls through to the
                // PTY below, so typing in the picker echoes into the
                // terminal. Esc still closes the modal because that's
                // handled earlier in `handle_hotkey_keypress`.
                if self.any_modal_blocks_input() {
                    return Ok(Task::none());
                }

                if let Some(tab_idx) = self.active_tab
                    && self.connecting.is_none()
                    && let keyboard::Event::KeyPressed {
                        key,
                        modifiers,
                        text: text_opt,
                        location,
                        ..
                    } = event
                    {
                        // Application-cursor-keys mode (DECCKM) of the active
                        // pane decides whether arrows / Home / End go out in
                        // SS3 (`ESC O A`) or CSI (`ESC [ A`) form. Read once
                        // here; the same flag is tracked for local PTY and SSH
                        // panes alike.
                        let app_cursor = self
                            .tabs
                            .get(tab_idx)
                            .and_then(|t| {
                                t.active().terminal.lock().ok().map(|s| s.application_cursor_keys())
                            })
                            .unwrap_or(false);
                        // macOS: Command (Cmd) is the clipboard modifier, not
                        // Ctrl. Cmd+V pastes; Cmd+C / Cmd+A are copy /
                        // select-all owned by the terminal widget (it holds the
                        // selection state). The Canvas widget and this global
                        // key subscription are independent paths, so the widget
                        // copying does NOT stop the bare character from echoing
                        // here. Swallow every unregistered Cmd combo so it never
                        // leaks into the PTY as text. Registered Cmd shortcuts
                        // (Cmd+K, Cmd+T, ...) were already consumed upstream by
                        // `handle_hotkey_keypress`. Ctrl keeps its Unix meaning
                        // (Ctrl+C = SIGINT) on every platform, including macOS.
                        if cfg!(target_os = "macos")
                            && modifiers.logo()
                            && !modifiers.control()
                            && !modifiers.alt()
                        {
                            if let keyboard::Key::Character(ref c) = key
                                && c.as_str().eq_ignore_ascii_case("v")
                            {
                                self.paste_clipboard_into_active();
                            }
                            // Cmd+C / Cmd+A and any other Cmd combo fall out
                            // here without writing, so nothing echoes.
                        }
                        // Ctrl+V → paste from clipboard (not raw Ctrl+V byte)
                        else if modifiers.control() && !modifiers.shift() {
                            if let keyboard::Key::Character(ref c) = key {
                                if c.as_str().eq_ignore_ascii_case("v") {
                                    self.paste_clipboard_into_active();
                                    // Don't fall through to the normal key handler
                                } else if c.as_str().eq_ignore_ascii_case("c") {
                                    // Ctrl+C → send interrupt (byte 3)
                                    self.write_input_to_tab(tab_idx, &[3]);
                                } else if c.as_str().eq_ignore_ascii_case("d")
                                    && self
                                        .tabs
                                        .get(tab_idx)
                                        .is_some_and(|t| t.label.ends_with(" (disconnected)"))
                                {
                                    // A logged-out session (SSH / exec tab relabelled
                                    // "(disconnected)") has no shell left to receive EOF,
                                    // so Ctrl+D would be swallowed. Treat it as "close this
                                    // dead tab" instead, matching the muscle memory of
                                    // dismissing an exited shell. Only single-pane tabs
                                    // carry the suffix, so siblings are never nuked.
                                    return Ok(self.update(Message::CloseTab(tab_idx)));
                                } else if let Some(bytes) = ctrl_key_bytes(&key) {
                                    // Other Ctrl+key combinations
                                    self.write_input_to_tab(tab_idx, &bytes);
                                }
                            } else if let Some(bytes) = key_to_named_bytes(&key, &modifiers, app_cursor) {
                                // Ctrl + named key (e.g. Ctrl+Home)
                                self.write_input_to_tab(tab_idx, &bytes);
                            }
                        } else if modifiers.shift() && modifiers.control() {
                            // Ctrl+Shift+V → paste from clipboard into SSH or local PTY.
                            // Copy (Ctrl+Shift+C) stays in the terminal widget since it
                            // owns the selection state.
                            if let keyboard::Key::Character(ref c) = key
                                && c.as_str().eq_ignore_ascii_case("v")
                            {
                                self.paste_clipboard_into_active();
                            }
                        } else {
                            // Normal keys (no Ctrl).
                            // Iced's `key` is the key WITHOUT modifiers, so a numpad
                            // keypress with NumLock on still shows up as Named::Home /
                            // ArrowUp / etc. while the OS-produced `text` is "7" / "8".
                            // Prefer the text on numpad so NumLock-on sends digits.
                            let numpad_text = if location == keyboard::Location::Numpad {
                                text_opt.as_ref().filter(|t| !t.is_empty())
                                    .map(|t| t.as_bytes().to_vec())
                            } else {
                                None
                            };
                            let mut bytes = numpad_text
                                .or_else(|| key_to_named_bytes(&key, &modifiers, app_cursor))
                                .or_else(|| text_opt.as_ref().map(|t| t.as_bytes().to_vec()));

                            // Meta-sends-escape: Alt+<char> (without Ctrl, so
                            // AltGr, reported as Ctrl+Alt on Windows and not as
                            // Alt on Linux, still composes text) is the
                            // ESC-prefixed character, the form readline / bash /
                            // zsh / vim / emacs / tmux bind their Meta keymaps to
                            // (Alt+b = back one word, Alt+f = forward, Alt+. =
                            // last arg, …). Named keys already fold their
                            // modifier in via key_to_named_bytes, so this only
                            // touches literal characters.
                            if modifiers.alt()
                                && !modifiers.control()
                                && let keyboard::Key::Character(c) = &key
                            {
                                // Some platforms drop `text` while Alt is held;
                                // fall back to the key's base character so
                                // Alt+b still emits `ESC b`.
                                let ch = bytes
                                    .filter(|b| !b.is_empty())
                                    .unwrap_or_else(|| c.as_str().as_bytes().to_vec());
                                let mut esc = Vec::with_capacity(ch.len() + 1);
                                esc.push(0x1b);
                                esc.extend_from_slice(&ch);
                                bytes = Some(esc);
                            }

                            if let Some(bytes) = bytes
                                && !bytes.is_empty()
                            {
                                self.write_input_to_tab(tab_idx, &bytes);
                            }
                        }
                    }
            }
            // Text committed by the OS IME (composed CJK characters, etc.),
            // delivered by the global subscription separately from
            // KeyboardEvent. Forward to the PTY under the same conditions as
            // a keystroke: no host editor panel or modal stealing focus, and
            // the cursor not over the chat sidebar. Deliberately does NOT
            // gate on active_view: in workspace mode a focused terminal runs
            // under the Dashboard view, not a dedicated Terminal view, so the
            // KeyboardEvent path doesn't check it either. When a text_input is
            // focused it handles its own Commit and inserts the text itself;
            // the host-panel / modal guards keep that from also hitting the
            // session.
            Message::TerminalImeCommit(text) => {
                if text.is_empty() || self.show_host_panel || self.any_modal_blocks_input() {
                    return Ok(Task::none());
                }
                let cursor_in_chat_sidebar = self
                    .active_tab
                    .and_then(|i| self.tabs.get(i))
                    .map(|t| t.chat_visible)
                    .unwrap_or(false)
                    && self.mouse_position.x
                        > (self.window_size.width - self.chat_sidebar_width);
                if cursor_in_chat_sidebar {
                    return Ok(Task::none());
                }
                if let Some(tab_idx) = self.active_tab
                    && self.connecting.is_none()
                {
                    let bytes = text.into_bytes();
                    self.write_input_to_tab(tab_idx, &bytes);
                }
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }

    /// Index of the tab whose grid contains the pane with `pane_id`.
    /// Used to route per-pane session events (connect / disconnect).
    pub(crate) fn pane_tab_index(&self, pane_id: uuid::Uuid) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.pane_grid.panes.values().any(|p| p.id == pane_id))
    }

    /// Find a pane by its stable id across every tab (shared read).
    pub(crate) fn pane_by_id(&self, pane_id: uuid::Uuid) -> Option<&crate::state::Pane> {
        self.tabs
            .iter()
            .flat_map(|t| t.pane_grid.panes.values())
            .find(|p| p.id == pane_id)
    }

    /// Find a pane by its stable id across every tab (mutable).
    pub(crate) fn pane_by_id_mut(&mut self, pane_id: uuid::Uuid) -> Option<&mut crate::state::Pane> {
        self.tabs
            .iter_mut()
            .flat_map(|t| t.pane_grid.panes.values_mut())
            .find(|p| p.id == pane_id)
    }
}

#[cfg(test)]
mod tests {
    use super::{session_log_segments, utf8_aligned, SESSION_LOG_SEGMENT_MS};

    /// Arrival marks for consecutive batches: each batch starts where
    /// the previous one ended, mirroring how the PTY handler records.
    fn marks_for(batches: &[(&[u8], i64)]) -> (Vec<u8>, Vec<(usize, i64)>) {
        let mut head = Vec::new();
        let mut marks = Vec::new();
        for &(bytes, ms) in batches {
            marks.push((head.len(), ms));
            head.extend_from_slice(bytes);
        }
        (head, marks)
    }

    #[test]
    fn cr_prefixed_progress_updates_get_their_own_timed_segments() {
        // wget style: every redraw starts with `\r`. Before the CR fix
        // these coalesced into one untimed lump.
        let gap = SESSION_LOG_SEGMENT_MS;
        let (head, marks) = marks_for(&[
            (b"GET file HTTP/1.1\n", 0),
            (b"\r 10% [==>      ]", gap),
            (b"\r 55% [=====>   ]", gap * 2),
            (b"\r100% [=========]", gap * 3),
        ]);
        let segs = session_log_segments(&head, &marks);
        assert_eq!(segs.len(), 4, "each CR redraw is a replay step: {segs:?}");
        assert_eq!(segs[1], (18, gap));
        assert!(segs.iter().skip(1).all(|&(p, _)| head[p] == b'\r'));
    }

    #[test]
    fn cr_terminated_progress_updates_cut_after_the_cr() {
        // apt style: every redraw ends with `\r`, the next batch starts
        // with printable text right after it.
        let gap = SESSION_LOG_SEGMENT_MS;
        let (head, marks) = marks_for(&[
            (b"Reading... 10%\r", 0),
            (b"Reading... 55%\r", gap),
            (b"Done\n", gap * 2),
        ]);
        let segs = session_log_segments(&head, &marks);
        assert_eq!(segs.len(), 3, "{segs:?}");
        assert!(segs.iter().skip(1).all(|&(p, _)| head[p - 1] == b'\r'));
    }

    #[test]
    fn bursty_marks_coalesce_and_mid_line_marks_never_cut() {
        let gap = SESSION_LOG_SEGMENT_MS;
        // Marks inside one replay step (gap measured from the last CUT,
        // not the last mark) coalesce even at line boundaries.
        let (head, marks) =
            marks_for(&[(b"a\n", 0), (b"b\n", gap / 3), (b"c\n", gap - 1)]);
        assert_eq!(session_log_segments(&head, &marks).len(), 1);
        // A big gap mid-line (no `\n`/`\r` anywhere near) still can't
        // cut: chunk boundaries must stay line-bounded for redaction.
        let (head, marks) = marks_for(&[(b"export TOKEN=abc", 0), (b"def123\n", gap * 4)]);
        assert_eq!(session_log_segments(&head, &marks).len(), 1);
        // No marks at all: a single segment at t=0.
        assert_eq!(session_log_segments(b"x", &[]), vec![(0, 0)]);
    }

    #[test]
    fn utf8_aligned_backs_off_split_sequences_only() {
        // "é" = C3 A9. A cut between the bytes moves before the lead.
        let buf = b"abc\xC3\xA9";
        assert_eq!(utf8_aligned(buf, 4), 3);
        assert_eq!(utf8_aligned(buf, 5), 5);
        // 4-byte emoji (F0 9F 92 96) split at every interior position.
        let buf = b"ok\xF0\x9F\x92\x96";
        for cut in 3..6 {
            assert_eq!(utf8_aligned(buf, cut), 2, "cut at {cut}");
        }
        assert_eq!(utf8_aligned(buf, 6), 6);
        // Pure ASCII and raw binary keep the requested cut.
        assert_eq!(utf8_aligned(b"hello", 5), 5);
        assert_eq!(utf8_aligned(&[0xFF; 8], 8), 8);
        assert_eq!(utf8_aligned(&[0x80; 8], 8), 8);
    }
}
