use super::*;

/// The OSC 8 hyperlink the pointer is currently over, surfaced to the app so
/// it can render a target-reveal chip (anti-spoofing: the visible label of an
/// OSC 8 link need not match its target). `allowed` is the scheme-allowlist
/// verdict (see `highlight::osc8_scheme_allowed`): when `false` the app shows
/// a "link type not allowed" chip instead of the target, and the widget
/// suppresses the pointer / underline / open affordance entirely. Written by
/// the widget under the render lock at hover time; read by the app in `view()`
/// via a non-blocking `try_lock`.
#[derive(Clone, Debug, PartialEq)]
pub struct HoveredLink {
    pub target: String,
    pub allowed: bool,
}

pub struct TerminalState {
    pub backend: TerminalBackend,
    pub pty: Option<PtyHandle>,
    pub palette: TerminalPalette,
    /// When this state is attached to an SSH session, resize events are
    /// forwarded here so the remote shell sees `window-change` and apps
    /// like `top`/`vim` re-layout instead of wrapping into our local grid.
    remote_resize_tx: Option<mpsc::UnboundedSender<(u16, u16)>>,
    /// Monotonic revision of anything that changes what the terminal would
    /// render (PTY output applied, synchronized-update flush, palette
    /// swap). The canvas widget folds this into its `RenderKey` so a draw
    /// triggered by unrelated UI churn (a hover elsewhere, a tab-title
    /// update, a toast) hits the geometry cache instead of re-tessellating
    /// the whole grid. Resizes are intentionally NOT counted here: a grid
    /// resize only happens on a bounds or font change, both of which the
    /// canvas cache already invalidates on directly.
    render_epoch: u64,
    /// Scrollback search (C1). `Some` while the find bar is open over
    /// this pane; `None` otherwise. Held here so the draw pass can read
    /// the match highlights under the same lock, and so a step / rebuild
    /// survives across frames.
    pub search: Option<crate::widget::search::BufferSearch>,
    /// A scroll-back offset the widget should snap to on the next draw
    /// (C1: center the active search match). `Cell` so the immutable
    /// draw pass can consume it, mirroring `reset_scroll_on_output`.
    pub pending_scroll: std::cell::Cell<Option<i32>>,
    /// The OSC 8 hyperlink under the pointer (C3), for the app's reveal
    /// chip. `None` when the pointer is over no explicit link. Updated by
    /// the widget's hover handler under the render lock.
    pub hovered_link: Option<HoveredLink>,
}

impl TerminalState {
    pub fn new(
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
    ) -> TerminalResult<(Self, mpsc::UnboundedReceiver<Vec<u8>>)>
    {
        let backend = TerminalBackend::new(cols, rows);
        let (pty, rx) =
            PtyHandle::spawn_command(cols, rows, None, &[], cwd, &backend.event_proxy)?;
        let palette = TerminalPalette::default();
        Ok((Self { backend, pty: Some(pty), palette, remote_resize_tx: None, render_epoch: 0, search: None, pending_scroll: std::cell::Cell::new(None), hovered_link: None }, rx))
    }

    /// Like `new` but spawns an explicit program (e.g. PowerShell or
    /// `wsl.exe -d Ubuntu`) instead of the OS default shell. Used by
    /// the Local Shell picker on Windows.
    pub fn new_with_command(
        cols: u16,
        rows: u16,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
    ) -> TerminalResult<(Self, mpsc::UnboundedReceiver<Vec<u8>>)>
    {
        let backend = TerminalBackend::new(cols, rows);
        let (pty, rx) = PtyHandle::spawn_command(
            cols, rows, Some(program), args, cwd, &backend.event_proxy,
        )?;
        let palette = TerminalPalette::default();
        Ok((Self { backend, pty: Some(pty), palette, remote_resize_tx: None, render_epoch: 0, search: None, pending_scroll: std::cell::Cell::new(None), hovered_link: None }, rx))
    }

    pub fn new_no_pty(
        cols: u16,
        rows: u16,
    ) -> TerminalResult<Self> {
        let backend = TerminalBackend::new(cols, rows);
        let palette = TerminalPalette::default();
        Ok(Self { backend, pty: None, palette, remote_resize_tx: None, render_epoch: 0, search: None, pending_scroll: std::cell::Cell::new(None), hovered_link: None })
    }

    /// Wire a remote resize sender, called from the app once an SSH
    /// session attaches to this state, so subsequent `resize()` calls
    /// also notify the server of the new viewport.
    pub fn set_remote_resize_sender(
        &mut self,
        tx: mpsc::UnboundedSender<(u16, u16)>,
    ) {
        self.remote_resize_tx = Some(tx);
    }

    /// Wire the emulator's query-reply back-channel to a remote session's
    /// input, called from the app alongside `set_remote_resize_sender`.
    /// The emulator answers in-band queries (DSR `\x1b[6n` cursor position,
    /// DA `\x1b[c`, DECRQM `\x1b[?..$p`, ...) by emitting `Event::PtyWrite`;
    /// local PTYs wire the same slot in `PtyHandle::spawn_command`. Remote
    /// programs (docker compose's raw-mode `[y/N]` prompt) block on these
    /// replies, so dropping them freezes the session for the user: raw mode
    /// means no echo and no Ctrl+C, and the blocked program prints nothing.
    pub fn set_remote_reply_sender(
        &mut self,
        tx: mpsc::UnboundedSender<Vec<u8>>,
    ) {
        self.backend.event_proxy.set_pty_write_tx(tx);
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.backend.process(bytes);
        // A batch reached the emulator: even a pure cursor move or a
        // query-only sequence can change what a frame would draw, so bump
        // unconditionally. The cost of an occasional needless rebuild is
        // negligible next to the win of skipping the rebuild entirely when
        // no output arrived at all.
        self.render_epoch = self.render_epoch.wrapping_add(1);
    }

    /// Current render revision. See [`TerminalState::render_epoch`] (field).
    pub fn render_epoch(&self) -> u64 {
        self.render_epoch
    }

    // ── Scrollback search (C1) ──

    /// Open the find bar over this pane (idempotent). Returns the search
    /// generation so the caller can invalidate any cached frame.
    pub fn search_open(&mut self) {
        if self.search.is_none() {
            self.search = Some(crate::widget::search::BufferSearch::default());
        }
    }

    /// Close the find bar and drop the match set.
    pub fn search_close(&mut self) {
        self.search = None;
    }

    /// Whether the find bar is open over this pane.
    pub fn search_active(&self) -> bool {
        self.search.is_some()
    }

    /// Set the find needle and rebuild matches. Auto-scrolls the active
    /// match into view. No-op when the bar isn't open.
    pub fn search_set_query(&mut self, query: &str) {
        let epoch = self.render_epoch;
        if let Some(search) = self.search.as_mut() {
            search.set_query(query, &self.backend.term, epoch);
            let target = search.active_match();
            self.queue_scroll_to_match(target);
        }
    }

    /// Step the active match forward / backward, scrolling it into view.
    pub fn search_step(&mut self, forward: bool) {
        // Re-scan first if new output landed since the last build, so
        // stepping never lands on a stale coordinate.
        let epoch = self.render_epoch;
        if let Some(search) = self.search.as_mut()
            && search.scanned_epoch != epoch
        {
            search.rebuild(&self.backend.term, epoch);
        }
        let target = self
            .search
            .as_mut()
            .and_then(|s| s.step(forward));
        self.queue_scroll_to_match(target);
    }

    /// `(current, total)` for the count label (`current` is 1-based, 0
    /// when there are no matches). `None` when the bar isn't open.
    pub fn search_count(&self) -> Option<(usize, usize)> {
        self.search.as_ref().map(|s| {
            let total = s.matches.len();
            let current = if total == 0 { 0 } else { s.active + 1 };
            (current, total)
        })
    }

    /// The search generation (bumped on every query change / step), for
    /// the widget's `RenderKey`. 0 when the bar isn't open.
    pub fn search_generation(&self) -> u64 {
        self.search.as_ref().map(|s| s.generation).unwrap_or(0)
    }

    /// Translate a match's start line into a scroll-back offset that
    /// centers it in the viewport, and queue it for the next draw. A
    /// match already on the visible screen (line >= 0) with the viewport
    /// at the bottom needs no scroll.
    fn queue_scroll_to_match(&self, m: Option<crate::widget::search::SearchMatch>) {
        let Some(m) = m else { return };
        let rows = self.backend.rows() as i32;
        // The draw maps grid line ← visible_row: `line = visible_row -
        // scroll_offset`, so a cell at grid line L shows at
        // `visible_row = L + scroll_offset`. To land the match's line near
        // the middle row we solve `rows/2 = L + scroll_offset`, i.e.
        // `scroll_offset = rows/2 - L` (L is negative in scrollback, so this
        // is a positive scroll-up). Clamped ≥ 0; a match already on the
        // visible screen with the viewport at the bottom needs no scroll.
        let desired = rows / 2 - m.start_line;
        self.pending_scroll.set(Some(desired.max(0)));
    }

    /// Swap the palette and bump the render epoch so the canvas cache
    /// re-tessellates with the new colors. Callers that mutate `palette`
    /// through this method (theme switch, per-connection palette resolve)
    /// keep the cache correct; a raw field assignment would leave a stale
    /// cached frame in the old theme.
    pub fn set_palette(&mut self, palette: TerminalPalette) {
        self.palette = palette;
        self.render_epoch = self.render_epoch.wrapping_add(1);
    }

    /// Deadline of a buffering DEC `?2026` synchronized update, if any.
    /// See `TerminalBackend::sync_timeout`.
    pub fn sync_timeout(&self) -> Option<std::time::Instant> {
        self.backend.sync_timeout()
    }

    /// Force-apply a stalled synchronized update to the grid.
    /// See `TerminalBackend::flush_sync`.
    pub fn flush_sync(&mut self) {
        self.backend.flush_sync();
        // Buffered bytes from a stalled synchronized update were just
        // applied to the grid, so the next frame's content differs.
        self.render_epoch = self.render_epoch.wrapping_add(1);
    }

    pub fn write(&mut self, data: &[u8]) {
        if let Some(ref pty) = self.pty
            && let Err(e) = pty.write(data) {
                tracing::error!("PTY write error: {}", e);
            }
    }

    /// True when the focused application has enabled bracketed paste mode
    /// (DECSET 2004, `ESC [ ? 2004 h`). Callers wrap pasted clipboard text
    /// in bracket markers so embedded newlines arrive as literal characters
    /// instead of one Enter per line. The backend tracks this even over SSH
    /// because remote output is fed through `process()` into the same term.
    pub fn bracketed_paste_enabled(&self) -> bool {
        use alacritty_terminal::term::TermMode;
        self.backend.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Take the pending window title set by the shell via OSC 0/2, draining
    /// the slot so each change is reported exactly once. An OSC ResetTitle
    /// surfaces as `Some("")` so the caller can fall back to its default
    /// label; `None` means nothing changed since the last call.
    pub fn take_title(&self) -> Option<String> {
        self.backend
            .event_proxy
            .title
            .lock()
            .ok()
            .and_then(|mut t| t.take())
    }

    /// Drain the pending bell flag, returning true at most once per ring.
    /// The app maps a true to the user's chosen bell action.
    pub fn take_bell(&self) -> bool {
        self.backend
            .event_proxy
            .bell
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Drain the latest working directory the shell reported via OSC 7.
    pub fn take_cwd(&mut self) -> Option<String> {
        self.backend.osc.take_cwd()
    }

    /// Drain the latest OSC 9 notification text, if any.
    pub fn take_notification(&mut self) -> Option<String> {
        self.backend.osc.take_notification()
    }

    /// Current OSC 9;4 progress report, if the app set one.
    pub fn progress(&self) -> Option<crate::osc::Progress> {
        self.backend.osc.progress()
    }

    /// Drain the OSC 133 shell-integration marks captured since the last
    /// call, each stamped with the cursor position at emission time.
    pub fn take_shell_marks(&mut self) -> Vec<crate::osc::PositionedShellMark> {
        self.backend.take_marks()
    }

    /// True while the alternate screen buffer is active (vim, htop, less...).
    /// The command-history capture ignores everything typed there.
    pub fn is_alt_screen(&self) -> bool {
        use alacritty_terminal::term::TermMode;
        self.backend.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Text of the logical (wrap-joined) line the cursor sits on, from
    /// column 0 of its first physical row (prompt included). Used by the
    /// command-history capture's heuristic path on hosts without OSC 133.
    /// Returns `None` on the alternate screen.
    pub fn cursor_logical_line(&self) -> Option<String> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags as CellFlags;
        if self.is_alt_screen() {
            return None;
        }
        let grid = self.backend.term.grid();
        let cols = grid.columns();
        let topmost = grid.topmost_line().0;
        let cursor_line = grid.cursor.point.line.0;

        // Walk up to the first row of the wrapped chain. Bounded so a
        // pathological full-width wrap chain can't stall the UI thread;
        // 64 rows of a wide terminal is far beyond any real command line.
        let mut first = cursor_line;
        let mut walked = 0;
        while first > topmost && walked < 64 {
            let prev = &grid[Line(first - 1)];
            if !prev[Column(cols - 1)].flags.contains(CellFlags::WRAPLINE) {
                break;
            }
            first -= 1;
            walked += 1;
        }
        Some(self.read_logical_line(first, 0))
    }

    /// Text of the logical (wrap-joined) line starting at physical row
    /// `abs_line` (absolute index: `history_size + visible line`, the
    /// coordinate space of [`crate::osc::PositionedShellMark`]) column
    /// `start_col`. Returns `None` on the alternate screen or when the row
    /// has left the addressable grid (scrollback ring saturated and rotated
    /// past it), so a stale mark can never read unrelated rows. This is how
    /// the capture reads the command the shell echoed after its OSC 133
    /// `PromptEnd` mark.
    pub fn logical_line_from_abs(&self, abs_line: i64, start_col: u16) -> Option<String> {
        use alacritty_terminal::grid::Dimensions;
        if self.is_alt_screen() {
            return None;
        }
        let grid = self.backend.term.grid();
        let rel = abs_line - grid.history_size() as i64;
        if rel < i64::from(grid.topmost_line().0) || rel > i64::from(grid.bottommost_line().0) {
            return None;
        }
        Some(self.read_logical_line(rel as i32, start_col as usize))
    }

    /// Join the soft-wrapped chain that starts at physical row `first`
    /// (grid-relative), reading from `start_col` on that first row and from
    /// column 0 on continuations. Wide-char spacers are skipped, trailing
    /// whitespace trimmed. When the result is a single physical row, a run
    /// of 8+ interior spaces truncates it: that gap is a zsh RPROMPT sitting
    /// on the right edge of the prompt row, not command text (a real command
    /// with 8 literal spaces inside is vanishingly rare next to how common
    /// right prompts are).
    fn read_logical_line(&self, first: i32, start_col: usize) -> String {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags as CellFlags;
        let grid = self.backend.term.grid();
        let cols = grid.columns();
        let mut text = String::new();
        let mut line = first;
        let mut rows = 0;
        loop {
            let row = &grid[Line(line)];
            let from = if line == first { start_col.min(cols) } else { 0 };
            for c in from..cols {
                let cell = &row[Column(c)];
                if cell.c != '\0' && !cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    text.push(cell.c);
                }
            }
            rows += 1;
            if !row[Column(cols - 1)].flags.contains(CellFlags::WRAPLINE)
                || line >= grid.bottommost_line().0
                || rows >= 64
            {
                break;
            }
            line += 1;
        }
        let mut text = text.trim_end().to_string();
        if rows == 1
            && let Some(gap) = text.find("        ")
        {
            text.truncate(gap);
        }
        text
    }

    /// True when the focused application has enabled application cursor keys
    /// mode (DECCKM, `ESC [ ? 1 h`, emitted by the terminfo `smkx`
    /// capability). In this mode the arrow and Home/End keys must be sent in
    /// their SS3 form (`ESC O A` …) instead of the default CSI form
    /// (`ESC [ A` …), which is what every full-screen TUI binds its
    /// navigation to (mc, vim, less, …). Tracked by the backend over both
    /// local PTY and SSH because remote output flows through the same
    /// `process()` into the term.
    pub fn application_cursor_keys(&self) -> bool {
        use alacritty_terminal::term::TermMode;
        self.backend.term.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> bool {
        if cols == self.backend.cols() && rows == self.backend.rows() {
            return false;
        }
        if cols < 2 || rows < 2 {
            return false;
        }
        self.backend.resize(cols, rows);
        if let Some(ref pty) = self.pty {
            let _ = pty.resize(cols, rows);
        }
        if let Some(ref tx) = self.remote_resize_tx {
            let _ = tx.send((cols, rows));
        }
        // A resize reflows the grid, so search matches at old coordinates
        // are stale: rebuild them against the new layout (C1).
        let epoch = self.render_epoch;
        if let Some(search) = self.search.as_mut() {
            search.rebuild(&self.backend.term, epoch);
        }
        true
    }

    pub fn cols(&self) -> u16 { self.backend.cols() }
    pub fn rows(&self) -> u16 { self.backend.rows() }

    /// The whole buffer (scrollback + screen) as text, trailing blank
    /// lines trimmed. Backs the "Copy All" context-menu action, which is
    /// app-driven and so can't reach the widget's live selection state.
    /// Reuses the selection extractor over a full-buffer range.
    pub fn all_text(&self) -> String {
        use alacritty_terminal::grid::Dimensions;
        let grid = self.backend.term.grid();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        let last_col = grid.columns().saturating_sub(1) as u16;
        let sel = Selection {
            start: (0, top),
            end: (last_col, bottom),
            block: false,
        };
        self.get_selection_text(&sel).trim_end().to_string()
    }

    /// Drop the scrollback history, keeping the visible screen (the PuTTY
    /// / Windows Terminal "Clear Scrollback" action). No-op when there is
    /// no history.
    pub fn clear_scrollback(&mut self) {
        self.backend.term.grid_mut().clear_history();
    }

    /// Visible cursor cell as `(column, line)`, 0-based from the top-left of
    /// the active screen. Used to anchor the OS IME candidate window near the
    /// caret. Ignores the widget's scrollback offset (during composition the
    /// view sits at the bottom), so it is exact while typing and only
    /// approximate if the user has scrolled into history.
    pub fn cursor_cell(&self) -> (u16, u16) {
        let p = self.backend.term.renderable_content().cursor.point;
        (p.column.0 as u16, p.line.0.max(0) as u16)
    }

    /// Extract text from a selection range.
    pub fn get_selection_text(&self, sel: &Selection) -> String {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags as CellFlags;
        let grid = self.backend.term.grid();
        let topmost = grid.topmost_line();
        let bottommost = grid.bottommost_line();
        let cols = grid.columns();
        let last_col = cols.saturating_sub(1) as u16;

        // Block (column) selection: every row takes the same column slice.
        // The slice is kept verbatim, including trailing spaces, so the
        // rectangle preserves its column alignment (trimming would ragged
        // a multi-column block, e.g. two columns of a table).
        if sel.block {
            let (c0, c1, l0, l1) = sel.block_bounds();
            let mut rows: Vec<String> = Vec::new();
            for line_idx in l0..=l1 {
                let line = Line(line_idx);
                if !(topmost..=bottommost).contains(&line) {
                    rows.push(String::new());
                    continue;
                }
                let row = &grid[line];
                let mut line_str = String::new();
                for c in c0..=c1.min(last_col) {
                    let cell = &row[Column(c as usize)];
                    // The trailing cell of a wide (CJK) glyph is a spacer
                    // whose `c` is a space; skip it so a double-width char
                    // doesn't copy out as "char + space".
                    if cell.c != '\0' && !cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                        line_str.push(cell.c);
                    }
                }
                rows.push(line_str);
            }
            return rows.join("\n");
        }

        let (start, end) = sel.ordered();
        // Iterate over the line range manually, selection lines are in
        // grid coordinates (negative for scrollback) which `display_iter`
        // alone wouldn't reach unless we mutated the display offset.
        // Each row is trimmed of trailing whitespace before joining, the
        // standard terminal behaviour so a wrapped/multi-line copy doesn't
        // carry the blank padding out to the right margin.
        let mut rows: Vec<String> = Vec::new();
        for line_idx in start.1..=end.1 {
            let line = Line(line_idx);
            if line < topmost || line > bottommost {
                continue;
            }
            let row = &grid[line];
            let (start_col, end_col) = if start.1 == end.1 {
                (start.0, end.0)
            } else if line_idx == start.1 {
                (start.0, last_col)
            } else if line_idx == end.1 {
                (0, end.0)
            } else {
                (0, last_col)
            };
            // Clamp to the last valid column: `pixel_to_cell` floors the
            // column low but not high, so a drag into the right padding can
            // push `end.0`/`start.0` to `cols`, which would panic on the
            // `row[Column(..)]` index below (the block branch above already
            // clamps with `c1.min(last_col)`).
            let (start_col, end_col) = (start_col.min(last_col), end_col.min(last_col));
            let mut line_str = String::new();
            for c in start_col..=end_col {
                let cell = &row[Column(c as usize)];
                // Skip wide-char spacer cells (the trailing half of a CJK
                // glyph), otherwise each one copies out as an extra space.
                if cell.c != '\0' && !cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    line_str.push(cell.c);
                }
            }
            rows.push(line_str.trim_end().to_string());
        }

        rows.join("\n")
    }

    /// Last `n_lines` rows of the terminal buffer as text, **including
    /// scrollback history** (not just the visible viewport). Each grid row is
    /// one line; wide-char spacer cells are dropped and trailing whitespace is
    /// trimmed, and the blank rows below the last output are skipped so the
    /// tail ends on real content. Internal blank lines (e.g. between blocks of
    /// output) are preserved. Used to feed recent terminal output to the AI
    /// assistant, which previously saw only the on-screen rows and silently
    /// lost anything that had scrolled off.
    pub fn tail_text(&self, n_lines: usize) -> Vec<String> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags as CellFlags;
        if n_lines == 0 {
            return Vec::new();
        }
        let grid = self.backend.term.grid();
        let cols = grid.columns();
        let top = grid.topmost_line().0;
        let bot = grid.bottommost_line().0;
        let line_text = |li: i32| -> String {
            let row = &grid[Line(li)];
            let mut s = String::new();
            for c in 0..cols {
                let cell = &row[Column(c)];
                // Skip wide-char spacer cells (the trailing half of a CJK
                // glyph); otherwise each copies out as an extra space.
                if cell.c != '\0' && !cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    s.push(cell.c);
                }
            }
            s.trim_end().to_string()
        };
        // Skip the blank rows below the last real output so the tail ends on
        // content, then take the last `n_lines` rows ending there (reaching
        // up into history when the viewport doesn't hold that many).
        let mut end = bot;
        while end > top && line_text(end).is_empty() {
            end -= 1;
        }
        let start = (end - (n_lines as i32 - 1)).max(top);
        (start..=end).map(line_text).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canvas geometry cache keys off `render_epoch`: a stale epoch
    /// after real output or a palette swap would leave the terminal showing
    /// last frame's grid. Guard both bumps structurally.
    #[test]
    fn render_epoch_advances_on_output_and_palette() {
        let mut state = TerminalState::new_no_pty(24, 80).expect("headless state");

        let e0 = state.render_epoch();
        state.process(b"hello");
        let e1 = state.render_epoch();
        assert!(e1 > e0, "process() must advance the render epoch");

        state.set_palette(TerminalPalette::default());
        assert!(
            state.render_epoch() > e1,
            "set_palette() must advance the render epoch"
        );
    }

    // ── Scrollback search (C1) ──

    /// A needle on the visible screen is found; the count is 1-based.
    #[test]
    fn search_finds_visible_match() {
        let mut state = TerminalState::new_no_pty(80, 24).expect("headless state");
        state.process(b"needle-alpha here\r\n");
        state.search_open();
        state.search_set_query("needle-alpha");
        assert_eq!(state.search_count(), Some((1, 1)));
        let m = state.search.as_ref().unwrap().matches[0];
        // On the visible screen, line 0 is the first row.
        assert_eq!(m.start_line, 0);
        assert_eq!(m.start_col, 0);
    }

    /// A match that scrolled off the top lives at a negative grid line.
    #[test]
    fn search_finds_scrollback_match() {
        let mut state = TerminalState::new_no_pty(80, 24).expect("headless state");
        // Print the needle, then push it well above the screen.
        state.process(b"needle-top\r\n");
        for i in 0..40 {
            state.process(format!("filler line {i}\r\n").as_bytes());
        }
        state.search_open();
        state.search_set_query("needle-top");
        assert_eq!(state.search_count(), Some((1, 1)));
        let m = state.search.as_ref().unwrap().matches[0];
        assert!(m.start_line < 0, "scrollback match must be a negative line, got {}", m.start_line);
        // The queued scroll must bring that scrollback line into the visible
        // window: with the draw's `visible_row = line + scroll_offset`, the
        // resulting row has to land inside [0, rows). Regression guard for a
        // sign slip that scrolled the match off the top.
        let offset = state.pending_scroll.get().expect("scroll queued");
        let visible_row = m.start_line + offset;
        assert!(
            (0..24).contains(&visible_row),
            "match at line {} + offset {} = row {} must be on screen",
            m.start_line,
            offset,
            visible_row,
        );
    }

    /// Literal search: the needle is escaped, so `a.b` does not match `axb`.
    #[test]
    fn search_is_literal_not_regex() {
        let mut state = TerminalState::new_no_pty(80, 24).expect("headless state");
        state.process(b"axb and a.b\r\n");
        state.search_open();
        state.search_set_query("a.b");
        // Only the literal `a.b` matches, not `axb`.
        assert_eq!(state.search_count(), Some((1, 1)));
    }

    /// Stepping wraps around and reports the right 1-based index.
    #[test]
    fn search_step_wraps() {
        let mut state = TerminalState::new_no_pty(80, 24).expect("headless state");
        state.process(b"x x x\r\n");
        state.search_open();
        state.search_set_query("x");
        assert_eq!(state.search_count(), Some((1, 3)));
        state.search_step(true);
        assert_eq!(state.search_count(), Some((2, 3)));
        state.search_step(true);
        assert_eq!(state.search_count(), Some((3, 3)));
        state.search_step(true); // wrap
        assert_eq!(state.search_count(), Some((1, 3)));
        state.search_step(false); // wrap backward
        assert_eq!(state.search_count(), Some((3, 3)));
    }

    /// An empty query clears the matches; closing drops the state.
    #[test]
    fn search_empty_and_close() {
        let mut state = TerminalState::new_no_pty(80, 24).expect("headless state");
        state.process(b"hello world\r\n");
        state.search_open();
        state.search_set_query("hello");
        assert_eq!(state.search_count(), Some((1, 1)));
        state.search_set_query("");
        assert_eq!(state.search_count(), Some((0, 0)));
        state.search_close();
        assert!(!state.search_active());
        assert_eq!(state.search_count(), None);
    }
}
