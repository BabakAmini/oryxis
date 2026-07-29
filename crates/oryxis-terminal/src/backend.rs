use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Process-wide scrollback (lines of history) applied to every terminal
/// created afterwards. The app sets this from the user's `scrollback_rows`
/// setting at boot and whenever it changes; terminals already open keep
/// their current buffer. Defaults to 10,000 to match the historical
/// hard-coded value, so behavior is unchanged until the app overrides it.
static DEFAULT_SCROLLBACK: AtomicUsize = AtomicUsize::new(10_000);

/// Set the scrollback used by terminals created after this call.
pub fn set_default_scrollback(lines: usize) {
    DEFAULT_SCROLLBACK.store(lines, Ordering::Relaxed);
}

fn default_scrollback() -> usize {
    DEFAULT_SCROLLBACK.load(Ordering::Relaxed)
}

/// OSC 52 clipboard access gates, process-wide so the per-terminal
/// `EventProxy` can read them without threading the setting through every
/// constructor (mirrors `DEFAULT_SCROLLBACK`). Write defaults on (the common,
/// low-risk direction, tmux/vim yank-to-clipboard); read defaults off (a
/// remote app reading the local clipboard is a privacy risk).
static OSC52_WRITE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static OSC52_READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set the OSC 52 clipboard access policy (write = apps may set the system
/// clipboard; read = apps may query it). Called by the app from its setting.
pub fn set_clipboard_access(write: bool, read: bool) {
    OSC52_WRITE.store(write, Ordering::Relaxed);
    OSC52_READ.store(read, Ordering::Relaxed);
}

/// Default set of characters that terminate a word for double-click
/// selection (the "word delimiters" / semantic-escape set). Matches
/// alacritty's own default minus the literal tab: terminal cells never
/// hold a raw `\t` (the emulator expands tabs into cursor moves and
/// spaces), so the tab delimiter is behaviorally inert and only made
/// the Settings text field awkward to edit. Space is kept since it is
/// the most common word boundary.
pub const DEFAULT_WORD_DELIMITERS: &str = ",│`|:\"' ()[]{}<>";

/// Event proxy that collects terminal events.
#[derive(Clone)]
pub struct EventProxy {
    /// Pending title from the shell.
    pub title: Arc<Mutex<Option<String>>>,
    /// Set when the shell rings the bell (BEL / `\a`). The app drains it each
    /// output batch and turns it into the user's chosen bell action
    /// (audible beep / visual flash / nothing).
    pub bell: Arc<std::sync::atomic::AtomicBool>,
    /// Sender wired to the PTY writer thread. The terminal emulator
    /// uses this to write replies back into the PTY for queries that
    /// the host (e.g. ConPTY's `\x1b[6n` cursor-position request)
    /// blocks on. Without it cmd.exe / wsl.exe stall after a few
    /// startup bytes and never paint a banner.
    pty_write_tx: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
    /// Per-instance OSC 52 clipboard override (C5 per-host quirk):
    /// `-1` = inherit the global policy, `0` = force off, `1` = force on.
    /// Checked before the global statics. Read is only ever forced OFF
    /// per-host (a host can tighten read, never grant it).
    osc52_write: Arc<std::sync::atomic::AtomicI8>,
    osc52_read: Arc<std::sync::atomic::AtomicI8>,
}

impl Default for EventProxy {
    fn default() -> Self {
        Self::new()
    }
}

impl EventProxy {
    pub fn new() -> Self {
        Self {
            title: Arc::new(Mutex::new(None)),
            bell: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pty_write_tx: Arc::new(Mutex::new(None)),
            osc52_write: Arc::new(std::sync::atomic::AtomicI8::new(-1)),
            osc52_read: Arc::new(std::sync::atomic::AtomicI8::new(-1)),
        }
    }

    /// Wires the back-channel from the terminal emulator to the PTY
    /// writer. Called by `PtyHandle::spawn_command` once the writer
    /// thread is running.
    pub fn set_pty_write_tx(&self, tx: mpsc::UnboundedSender<Vec<u8>>) {
        if let Ok(mut slot) = self.pty_write_tx.lock() {
            *slot = Some(tx);
        }
    }

    /// Set the per-instance OSC 52 clipboard overrides (C5). `None`
    /// inherits the global policy for that direction; `Some(bool)` forces
    /// it. Read is only ever forced OFF per-host (a host can tighten read,
    /// never grant it).
    pub fn set_osc52_override(&self, write: Option<bool>, read: Option<bool>) {
        let enc = |o: Option<bool>| match o {
            None => -1,
            Some(false) => 0,
            Some(true) => 1,
        };
        self.osc52_write.store(enc(write), Ordering::Relaxed);
        self.osc52_read.store(enc(read), Ordering::Relaxed);
    }

    /// Effective OSC 52 write policy: the per-instance override when set,
    /// else the global `OSC52_WRITE`.
    fn osc52_write_allowed(&self) -> bool {
        match self.osc52_write.load(Ordering::Relaxed) {
            0 => false,
            1 => true,
            _ => OSC52_WRITE.load(Ordering::Relaxed),
        }
    }

    /// Effective OSC 52 read policy: the per-instance override (only ever
    /// force-off) when set, else the global `OSC52_READ`.
    fn osc52_read_allowed(&self) -> bool {
        match self.osc52_read.load(Ordering::Relaxed) {
            0 => false,
            1 => true,
            _ => OSC52_READ.load(Ordering::Relaxed),
        }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                if let Ok(mut t) = self.title.lock() {
                    *t = Some(title);
                }
            }
            // OSC ResetTitle: surface as an empty string so the app drops the
            // custom title and falls back to its connection label.
            Event::ResetTitle => {
                if let Ok(mut t) = self.title.lock() {
                    *t = Some(String::new());
                }
            }
            Event::PtyWrite(s) => {
                if let Ok(slot) = self.pty_write_tx.lock()
                    && let Some(tx) = slot.as_ref()
                {
                    let _ = tx.send(s.into_bytes());
                }
            }
            Event::Wakeup => {}
            Event::Bell => {
                self.bell.store(true, Ordering::Relaxed);
            }
            // OSC 52: an app sets the system clipboard. Gated, so a remote
            // session can't silently overwrite the clipboard when disabled.
            Event::ClipboardStore(_ty, text) if self.osc52_write_allowed() => {
                crate::widget::set_clipboard_text(&text);
            }
            // OSC 52: an app reads the system clipboard. Off by default (a
            // remote reading your clipboard is a privacy risk). When enabled,
            // the formatter builds the reply, sent back through the PTY
            // back-channel (the same one cursor-position replies use).
            Event::ClipboardLoad(_ty, formatter) if self.osc52_read_allowed() => {
                let current = arboard::Clipboard::new()
                    .ok()
                    .and_then(|mut c| c.get_text().ok())
                    .unwrap_or_default();
                let reply = formatter(&current);
                if let Ok(slot) = self.pty_write_tx.lock()
                    && let Some(tx) = slot.as_ref()
                {
                    let _ = tx.send(reply.into_bytes());
                }
            }
            _ => {}
        }
    }
}

/// Wraps alacritty_terminal's Term + ansi Processor.
pub struct TerminalBackend {
    pub term: Term<EventProxy>,
    processor: ansi::Processor,
    pub event_proxy: EventProxy,
    cols: u16,
    rows: u16,
    /// Kept so `set_word_delimiters` can hand a full `Config` back to
    /// `Term::set_options` (alacritty has no narrower setter exposed).
    config: TermConfig,
    /// Sniffs OSC 7/133/9 out of the byte stream (alacritty doesn't surface
    /// those as events).
    pub osc: crate::osc::OscSniffer,
    /// Strips screen's `ESC k … ST` window-title sequences before the
    /// emulator can print them as text (issue #88). Runs first, so the
    /// OSC sniffer's byte offsets refer to the filtered stream.
    screen_title: crate::screen_title::ScreenTitleFilter,
    /// OSC 133 shell-integration marks captured by `process`, each stamped
    /// with the cursor position at the moment the emulator reached the mark.
    /// Drained by `take_marks`; bounded so an undrained pane can't grow it.
    /// A deque so evicting the oldest mark at the cap is O(1): `process`
    /// runs on the UI thread, and a mark flood paying a 4096-element
    /// shift per mark is exactly the silent per-batch cost class #104
    /// hunts.
    marks: std::collections::VecDeque<crate::osc::PositionedShellMark>,
}

impl TerminalBackend {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::new_with_scrollback(cols, rows, default_scrollback())
    }

    /// Like [`new`](Self::new) but with an explicit scrollback line
    /// budget instead of the process-wide default. The session-log
    /// viewer uses this to hold a whole recording (which can exceed the
    /// user's live `scrollback_rows`) without truncating the oldest
    /// lines. alacritty grows the history lazily, so a high budget costs
    /// only what the content actually fills.
    pub fn new_with_scrollback(cols: u16, rows: u16, scrollback: usize) -> Self {
        let size = TermSize { cols, rows };
        let config = TermConfig {
            scrolling_history: scrollback,
            semantic_escape_chars: DEFAULT_WORD_DELIMITERS.to_string(),
            ..Default::default()
        };
        let event_proxy = EventProxy::new();
        let term = Term::new(config.clone(), &size, event_proxy.clone());
        let processor = ansi::Processor::new();

        Self {
            term,
            processor,
            event_proxy,
            cols,
            rows,
            config,
            osc: crate::osc::OscSniffer::default(),
            screen_title: crate::screen_title::ScreenTitleFilter::default(),
            marks: std::collections::VecDeque::new(),
        }
    }

    /// Update the word-delimiter set used by double-click semantic
    /// selection. No-op when unchanged so the per-click sync stays
    /// cheap (`set_options` marks the grid fully damaged, so we must
    /// not call it on every mouse event).
    pub fn set_word_delimiters(&mut self, delimiters: &str) {
        if self.config.semantic_escape_chars == delimiters {
            return;
        }
        self.config.semantic_escape_chars = delimiters.to_string();
        self.term.set_options(self.config.clone());
    }

    /// Feed raw bytes from PTY into the terminal emulator.
    pub fn process(&mut self, bytes: &[u8]) {
        // Strip screen's `ESC k … ST` window titles first (issue #88): the
        // emulator would print their payload as text, and everything below
        // (OSC offsets, mark positions) must see the same stream it does.
        let (filtered, screen_titles) = self.screen_title.filter(bytes);
        for title in screen_titles {
            if let Ok(mut slot) = self.event_proxy.title.lock() {
                *slot = Some(title);
            }
        }
        let bytes = filtered.as_ref();
        // Sniff OSC 7/133/9 before handing the bytes to the emulator (which
        // ignores those OSC numbers); a no-op for the common no-OSC chunk.
        let events = self.osc.feed(bytes);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if events.is_empty() {
                self.processor.advance(&mut self.term, bytes);
                return;
            }
            // OSC 133 marks in this batch: advance in mark-aligned segments
            // so each mark's cursor snapshot is taken exactly where the shell
            // emitted it. Advancing the whole batch first would sample the
            // end-of-batch cursor, which lies whenever the batch carries more
            // output after the mark (right-side prompts, command echo, ...).
            let mut start = 0;
            for ev in &events {
                self.processor.advance(&mut self.term, &bytes[start..ev.offset]);
                start = ev.offset;
                let point = self.term.grid().cursor.point;
                let abs_line =
                    self.term.grid().history_size() as i64 + i64::from(point.line.0);
                if self.marks.len() >= 4096 {
                    self.marks.pop_front();
                }
                self.marks.push_back(crate::osc::PositionedShellMark {
                    mark: ev.mark,
                    abs_line,
                    col: point.column.0 as u16,
                });
            }
            self.processor.advance(&mut self.term, &bytes[start..]);
        }));
        if result.is_err() {
            tracing::error!("Terminal processor panic on {} bytes (ignored)", bytes.len());
        }
    }

    /// Drain the OSC 133 marks captured since the last call.
    pub fn take_marks(&mut self) -> Vec<crate::osc::PositionedShellMark> {
        std::mem::take(&mut self.marks).into()
    }

    /// Deadline at which an open synchronized update (DEC `?2026`) must be
    /// force-flushed, or `None` when nothing is buffering. vte buffers every
    /// byte after a BSU (`ESC[?2026h`) and only applies it on the matching
    /// ESU (`ESC[?2026l`), a 2 MiB overflow, or an explicit `stop_sync`, it
    /// never expires the 150 ms timeout from inside `advance`. Driving that
    /// timeout is the host's job: without it an app that opens a sync update
    /// and then blocks on input (docker compose's `(y/N)` prompt) leaves the
    /// screen frozen on the frame before the update began. The caller
    /// schedules a wake-up at this instant and calls `flush_sync`.
    pub fn sync_timeout(&self) -> Option<std::time::Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    /// Force-end a buffered synchronized update, applying the buffered bytes
    /// to the grid. No-op when none is pending. Mirrors the 150 ms abort
    /// alacritty's own event loop performs so a never-closed update can't
    /// freeze the terminal indefinitely.
    pub fn flush_sync(&mut self) {
        if self.processor.sync_timeout().sync_timeout().is_some() {
            self.processor.stop_sync(&mut self.term);
        }
    }

    /// Resize the terminal grid.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let size = TermSize { cols, rows };
        self.term.resize(size);
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }
}

struct TermSize {
    cols: u16,
    rows: u16,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line, Point};

    #[test]
    fn osc52_per_instance_override_beats_global_both_ways() {
        let proxy = EventProxy::new();
        // Inherit (default): the effective policy tracks the global.
        set_clipboard_access(true, false);
        assert!(proxy.osc52_write_allowed(), "inherit follows global-on");
        set_clipboard_access(false, false);
        assert!(!proxy.osc52_write_allowed(), "inherit follows global-off");
        // Write: force-on beats global-off; force-off beats global-on.
        proxy.set_osc52_override(Some(true), None);
        assert!(proxy.osc52_write_allowed(), "force-on beats global-off");
        set_clipboard_access(true, false);
        proxy.set_osc52_override(Some(false), None);
        assert!(!proxy.osc52_write_allowed(), "force-off beats global-on");
        // Read is only ever force-off: with global read ON, an "Off" host
        // (read forced off) still blocks read, while an inherit host reads.
        set_clipboard_access(true, true);
        proxy.set_osc52_override(Some(false), Some(false));
        assert!(!proxy.osc52_read_allowed(), "force-off read beats global-on");
        proxy.set_osc52_override(Some(true), None);
        assert!(proxy.osc52_read_allowed(), "inherit read follows global-on");
        // Back to inherit tracks the global again.
        proxy.set_osc52_override(None, None);
        assert!(proxy.osc52_write_allowed());
        // Restore the process default (write on, read off) for other tests.
        set_clipboard_access(true, false);
    }

    /// `set_word_delimiters` must actually drive alacritty's native
    /// semantic search: with the default set, `foo-bar` is one word
    /// (no `-` delimiter), but after adding `-` it splits at the dash.
    /// This is the behavior the double-click word selection rides on.
    #[test]
    fn word_delimiters_drive_semantic_search() {
        let mut backend = TerminalBackend::new(40, 5);
        backend.process(b"foo-bar baz");
        let origin = Point::new(Line(0), Column(0));

        // Default set has no `-`: the word spans the whole `foo-bar`.
        let right_default = backend.term.semantic_search_right(origin).column.0;
        assert_eq!(right_default, 6, "default should treat foo-bar as one word");

        // Adding `-` as a delimiter stops the word at `foo`.
        backend.set_word_delimiters("-");
        let right_dash = backend.term.semantic_search_right(origin).column.0;
        assert_eq!(right_dash, 2, "`-` delimiter should split foo|bar");
    }

    fn cell0(backend: &TerminalBackend) -> char {
        backend.term.grid()[Line(0)][Column(0)].c
    }

    /// An open DEC `?2026` synchronized update buffers output in vte: the
    /// glyph must not reach the grid, and a flush deadline must be armed.
    /// `flush_sync` (the host-driven 150 ms abort) then applies it. This is
    /// the freeze the host MUST break, vte never expires the timeout itself.
    #[test]
    fn synchronized_update_buffers_until_flush() {
        let mut backend = TerminalBackend::new(40, 5);
        backend.process(b"\x1b[?2026hX");
        assert_eq!(cell0(&backend), ' ', "buffered glyph must not reach the grid");
        assert!(backend.sync_timeout().is_some(), "an open update arms a deadline");

        backend.flush_sync();
        assert_eq!(cell0(&backend), 'X', "flush_sync must apply the buffered glyph");
        assert!(backend.sync_timeout().is_none(), "deadline clears after flush");
    }

    /// A complete BSU...ESU pair in one feed applies immediately and leaves
    /// no pending deadline, so the host arms no needless timer.
    #[test]
    fn closed_synchronized_update_needs_no_flush() {
        let mut backend = TerminalBackend::new(40, 5);
        backend.process(b"\x1b[?2026hY\x1b[?2026l");
        assert_eq!(cell0(&backend), 'Y', "closed update applies on its own");
        assert!(backend.sync_timeout().is_none(), "closed update leaves no deadline");
    }

    /// In-band terminal queries must be answered through the PtyWrite
    /// back-channel once a sender is wired (issue #48: docker compose's
    /// raw-mode prompt blocks forever on an unanswered query, freezing
    /// the session for the user). DSR `\x1b[6n` asks for the cursor
    /// position; the reply is `\x1b[{row};{col}R`, 1-based.
    #[test]
    fn dsr_query_reply_reaches_back_channel() {
        let mut backend = TerminalBackend::new(40, 5);
        let (tx, mut rx) = mpsc::unbounded_channel();
        backend.event_proxy.set_pty_write_tx(tx);
        backend.process(b"ab\x1b[6n");
        let reply = rx.try_recv().expect("DSR query must produce a reply");
        assert_eq!(reply, b"\x1b[1;3R", "cursor sits on row 1, column 3 after `ab`");
    }

    /// DECRQM private-mode queries get a report too; buildkit / docker
    /// compose probe `?2026` (synchronized output) this way before its
    /// prompt. `\x1b[?2026;2$y` = mode recognized, currently reset.
    #[test]
    fn decrqm_query_reply_reaches_back_channel() {
        let mut backend = TerminalBackend::new(40, 5);
        let (tx, mut rx) = mpsc::unbounded_channel();
        backend.event_proxy.set_pty_write_tx(tx);
        backend.process(b"\x1b[?2026$p");
        let reply = rx.try_recv().expect("DECRQM query must produce a reply");
        assert_eq!(reply, b"\x1b[?2026;2$y");
    }

    /// `flush_sync` with no update pending is a no-op (must not corrupt the
    /// grid or panic), since the timer can fire after a normal close.
    #[test]
    fn flush_sync_without_pending_update_is_noop() {
        let mut backend = TerminalBackend::new(40, 5);
        backend.process(b"Z");
        backend.flush_sync();
        assert_eq!(cell0(&backend), 'Z');
        assert!(backend.sync_timeout().is_none());
    }

    /// Read a rendered row back as text, trailing blanks trimmed. Asserting
    /// on the grid (not on the filter's output) is the point: it is the only
    /// way to prove the payload never became visible cells.
    fn line(backend: &TerminalBackend, row: usize) -> String {
        let grid = backend.term.grid();
        let cols = grid.columns();
        let mut s = String::with_capacity(cols);
        for col in 0..cols {
            s.push(grid[Line(row as i32)][Column(col)].c);
        }
        s.trim_end().to_string()
    }

    /// Issue #88 follow-up (Mazwak, CentOS 7). On a `screen*` TERM the stock
    /// `/etc/bashrc` sets
    /// `PROMPT_COMMAND='printf "\033k%s@%s:%s\033\\" ...'`, so every prompt is
    /// preceded by screen's window-title sequence. vte dispatches `ESC k` as an
    /// unhandled escape and PRINTS the payload, which is what rendered the
    /// prompt twice: `root@oldserver:~[root@oldserver ~]#`. The grid must carry
    /// the shell's prompt alone, and the title must arrive as a title.
    #[test]
    fn centos_screen_prompt_command_does_not_paint_a_second_prompt() {
        let mut backend = TerminalBackend::new(40, 5);
        // Byte for byte what bash emits on that host.
        backend.process(b"\x1bkroot@oldserver:~\x1b\\[root@oldserver ~]# ");
        assert_eq!(
            line(&backend, 0),
            "[root@oldserver ~]#",
            "the window title must not reach the grid as text"
        );
        let title = backend.event_proxy.title.lock().unwrap().clone();
        assert_eq!(title.as_deref(), Some("root@oldserver:~"), "title is surfaced");
    }

    /// The second half of the same report: with the payload occupying real
    /// columns, readline's Ctrl+R redraw (which returns to column 0 and
    /// overwrites) could not cover the stale prompt, leaving its tail visible
    /// (`(reverse-i-search)`':ot@oldserver ~]#`). With the sequence stripped
    /// the redraw covers the whole prompt, exactly as it does on xterm-256color.
    #[test]
    fn reverse_search_redraw_covers_the_whole_prompt() {
        let mut backend = TerminalBackend::new(60, 5);
        backend.process(b"\x1bkroot@oldserver:~\x1b\\[root@oldserver ~]# ");
        // Ctrl+R: bash returns to column 0 and paints the search prompt over
        // whatever was there.
        backend.process(b"\r(reverse-i-search)`': ");
        assert_eq!(
            line(&backend, 0),
            "(reverse-i-search)`':",
            "no tail of the old prompt may survive the redraw"
        );
    }
}
