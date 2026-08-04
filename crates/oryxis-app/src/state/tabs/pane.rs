//! A pane: one live session inside a tab.
//!
//! What it is connected to (`PaneOrigin`, `TerminalTransport`), what the
//! shell has told us about itself (`PromptState` and the capture state
//! command history reads), the Files sidebar mounted beside it, and the
//! transfer surfaces that ride on it (ZMODEM, OS drops).

use super::super::*;

/// What a pane reconnects to, so a saved session group can reference it.
/// This is an explicit discriminator rather than inferring "local" from a
/// missing connection id: cloud/SSM/ECS panes also lack a saved
/// `Connection`, so `None`-means-local would mis-save them. `Ephemeral`
/// covers those (and any pane we can't reference by id); they are pruned
/// when a tab is saved as a session group.
#[derive(Debug, Clone)]
pub(crate) enum PaneOrigin {
    /// Live reference to a saved Connection by id.
    Host(Uuid),
    /// Quick-connect host: the id points into `Oryxis.quick_connects`, an
    /// in-memory store that is never persisted. Kept apart from `Host` so
    /// vault-backed features (edit in place, session groups, pin restore)
    /// opt in deliberately instead of dereferencing a dangling vault id.
    QuickHost(Uuid),
    /// A local terminal; the spec is captured so the same shell is restored.
    Local(LocalShellSpec),
    /// Cloud/SSM/ECS or otherwise non-referenceable pane.
    Ephemeral,
}

/// Where a pane's remote shell stands in the OSC 133 prompt cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptState {
    /// No OSC 133 mark seen yet: the host has no shell integration and the
    /// command-history capture falls back to the echo heuristic.
    NoIntegration,
    /// `PromptEnd` (B) seen: the shell is reading a command line that starts
    /// at `col` of absolute grid row `abs_line`.
    AtPrompt { abs_line: i64, col: u16 },
    /// A command is running or the prompt is being redrawn; input is a
    /// program's stdin and must never be recorded.
    Busy,
}

/// A command submitted while `AtPrompt` whose echo had not reached the grid
/// yet (a paste with a trailing newline arrives before the round trip). The
/// echoed line is read back from these coordinates when `OutputStart`
/// confirms the shell accepted a command.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingCapture {
    pub b_abs: i64,
    pub b_col: u16,
}

/// In-band command capture: what the shell itself reported it parsed, via
/// `OSC 633 ; E`. This is the only capture path that survives a multiplexer
/// (under tmux the app's grid is tmux's repaint of every pane, so reading
/// the command back from it would splice the neighbouring pane's row into
/// the text), and the only one that can't mistake a keystroke for a command:
/// the text comes from the shell, not from the screen.
#[derive(Debug, Default, Clone)]
pub(crate) struct InbandCapture {
    /// True once this pane saw its first `E`. From then on the grid-reading
    /// and heuristic paths are off for this pane: keeping them alongside
    /// would double-record every command, and under tmux the outer grid's
    /// prompt belongs to whichever pane tmux drew there last.
    pub seen: bool,
    /// The reported command line, held until the `OutputStart` that confirms
    /// the shell actually ran it (a bare Enter or a Ctrl+C never reaches one).
    pub pending: Option<String>,
}

/// The live remote transport feeding a terminal pane. SSH and Telnet
/// expose the same session surface (write / resize / senders /
/// is_alive / close), so every generic pane path calls through this
/// enum; features that need the SSH machinery underneath (SFTP mounts,
/// OS detection, exec channels) reach it via [`TerminalTransport::ssh`]
/// and simply don't exist for Telnet panes. An enum rather than a
/// trait object because only the pane path branches, and the SSH arm
/// must keep handing out its concrete `Arc<SshSession>`.
#[derive(Debug, Clone)]
pub(crate) enum TerminalTransport {
    Ssh(Arc<SshSession>),
    Telnet(Arc<oryxis_telnet::TelnetSession>),
    Serial(Arc<oryxis_serial::SerialSession>),
}

impl TerminalTransport {
    /// The inner SSH session, for the SSH-only feature paths.
    pub fn ssh(&self) -> Option<&Arc<SshSession>> {
        match self {
            TerminalTransport::Ssh(s) => Some(s),
            TerminalTransport::Telnet(_) | TerminalTransport::Serial(_) => None,
        }
    }

    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        match self {
            TerminalTransport::Ssh(s) => s.write(data).map_err(|e| e.to_string()),
            TerminalTransport::Telnet(s) => s.write(data).map_err(|e| e.to_string()),
            TerminalTransport::Serial(s) => s.write(data).map_err(|e| e.to_string()),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        match self {
            TerminalTransport::Ssh(s) => s.resize(cols, rows),
            TerminalTransport::Telnet(s) => s.resize(cols, rows),
            // A serial line has no window size; resize is a no-op.
            TerminalTransport::Serial(_) => {}
        }
    }

    /// Clone of the resize sender (SSH window-change / Telnet NAWS) so
    /// the terminal state forwards viewport changes directly. `None`
    /// for serial, which has no viewport concept.
    pub fn resize_sender(&self) -> Option<tokio::sync::mpsc::UnboundedSender<(u16, u16)>> {
        match self {
            TerminalTransport::Ssh(s) => Some(s.resize_sender()),
            TerminalTransport::Telnet(s) => Some(s.resize_sender()),
            TerminalTransport::Serial(_) => None,
        }
    }

    /// Clone of the input sender for in-band query replies (cursor
    /// position report, DECRQM, ...), which remote programs block on.
    pub fn write_sender(&self) -> tokio::sync::mpsc::UnboundedSender<Vec<u8>> {
        match self {
            TerminalTransport::Ssh(s) => s.write_sender(),
            TerminalTransport::Telnet(s) => s.write_sender(),
            TerminalTransport::Serial(s) => s.write_sender(),
        }
    }

    pub fn is_alive(&self) -> bool {
        match self {
            TerminalTransport::Ssh(s) => s.is_alive(),
            TerminalTransport::Telnet(s) => s.is_alive(),
            TerminalTransport::Serial(s) => s.is_alive(),
        }
    }

    /// Tear the session down (idempotent on every arm).
    pub fn close(&self) {
        match self {
            TerminalTransport::Ssh(s) => s.close(),
            TerminalTransport::Telnet(s) => s.close(),
            TerminalTransport::Serial(s) => s.close(),
        }
    }
}

/// Sidebar Files tab state, one instance per pane: an SFTP channel
/// multiplexed on this pane's SSH session plus the browsing state.
/// The channel dies with the session, so `SshDisconnected` resets the
/// whole struct (keeping only the user's follow / hidden preferences).
#[derive(Default)]
pub(crate) struct PaneFiles {
    /// The SFTP channel on this pane's live `client::Handle`. `None`
    /// until the Files tab is first opened (mounted lazily so panes
    /// that never browse pay nothing).
    pub client: Option<SftpClient>,
    /// True while the initial mount (open channel + first listing) is
    /// in flight, the guard against double-mounting on rapid clicks.
    pub mounting: bool,
    /// Current directory (absolute remote POSIX path). Empty until the
    /// first listing lands.
    pub path: String,
    /// The session's home directory, resolved at mount. Expands the
    /// `~`-relative cwd the OSC 0/2 title fallback produces.
    pub home: Option<String>,
    /// In-progress manual path edit (the header path is clickable,
    /// mirroring the SFTP pane's path editing); `None` = display mode.
    pub path_editing: Option<String>,
    /// In-progress inline rename: `(full remote path, edited name)`.
    pub rename: Option<(String, String)>,
    /// In-progress inline create: `(kind, typed name)`, rendered as an
    /// input row at the top of the list.
    pub new_entry: Option<(SftpEntryKind, String)>,
    /// Entries of `path`, sorted dirs-first / name-insensitive.
    pub entries: Vec<SftpEntry>,
    /// True while a `list_dir` (navigation or cwd follow) is in flight.
    pub loading: bool,
    /// Monotonic request stamp: every mount / list task carries the
    /// value at dispatch time and its completion is dropped unless it
    /// still matches (latest request wins). Bumped by
    /// `reset_for_disconnect` too, so a mount racing a reconnect can't
    /// install a client whose channel rode the dead session.
    pub req_seq: u64,
    pub error: Option<String>,
    /// Whether the browser follows the shell's OSC 7 cwd. `true` for a
    /// fresh pane; the pin toggle flips it.
    pub follow_disabled: bool,
    pub show_hidden: bool,
    /// Directories this browser actually adopted, most recent first,
    /// deduped, capped (issue #85; the SFTP pane's `path_history`
    /// sibling). Feeds the path combo-box dropdown. In-memory and
    /// host-scoped: `reset_for_disconnect` clears it.
    pub path_history: Vec<String>,
    /// Back / forward stacks; see `PaneState`'s for why they are not the
    /// recency list.
    pub nav_back: Vec<String>,
    pub nav_fwd: Vec<String>,
    pub nav_replay: bool,
    /// Whether the path combo-box dropdown is open.
    pub path_history_open: bool,
    /// Full remote path of the currently selected row, if any.
    /// Cleared on navigation / mount / disconnect.
    pub selected: Option<String>,
    /// Timestamp + path of the last single click, for double-click
    /// detection (matching the SFTP pane's rule).
    pub last_click: Option<(std::time::Instant, String)>,
}

impl PaneFiles {
    /// Follow-cwd is stored inverted so `Default` gives "on".
    pub fn follow(&self) -> bool {
        !self.follow_disabled
    }

    /// Drop everything tied to the dead SSH session, keeping only the
    /// user's preferences (follow / hidden) for the reconnect. The
    /// request stamp bumps so any in-flight mount / listing on the old
    /// session is dropped when it completes.
    pub fn reset_for_disconnect(&mut self) {
        self.client = None;
        self.mounting = false;
        self.path.clear();
        self.home = None;
        self.path_editing = None;
        self.rename = None;
        self.new_entry = None;
        self.entries.clear();
        self.loading = false;
        self.req_seq += 1;
        self.error = None;
        // Host-scoped: a reconnect may land on another host's tree, so the
        // in-memory list goes. It is no longer a loss: the history is
        // persisted per host and `hydrate_files_recent` refills this on the
        // next mount (issue #114). Before that, closing the host was the
        // end of it.
        self.path_history.clear();
        self.path_history_open = false;
        self.selected = None;
        self.last_click = None;
    }

    /// Record an adopted directory in the combo-box history: most
    /// recent first, a revisit moves the entry to the top, capped so
    /// the dropdown stays scannable (issue #85, the SFTP pane's rule).
    /// Record leaving `previous` for a new directory. Clears the forward
    /// stack, because branching off mid-history is a new future.
    pub fn push_nav(&mut self, previous: String) {
        const NAV_CAP: usize = 100;
        // A back / forward step consumes the flag instead of recording:
        // its arrival is the history being replayed, not a new visit.
        if std::mem::take(&mut self.nav_replay) {
            return;
        }
        if previous.is_empty() {
            return;
        }
        self.nav_back.push(previous);
        self.nav_fwd.clear();
        if self.nav_back.len() > NAV_CAP {
            self.nav_back.remove(0);
        }
    }

    /// Pop the previous directory, remembering `current` so Forward can
    /// come back to it. `None` when there is nowhere to go.
    pub fn nav_go_back(&mut self, current: String) -> Option<String> {
        let target = self.nav_back.pop()?;
        self.nav_fwd.push(current);
        self.nav_replay = true;
        Some(target)
    }

    /// The mirror of [`Self::nav_go_back`].
    pub fn nav_go_forward(&mut self, current: String) -> Option<String> {
        let target = self.nav_fwd.pop()?;
        self.nav_back.push(current);
        self.nav_replay = true;
        Some(target)
    }

    pub fn push_path_history(&mut self, path: String) {
        const PATH_HISTORY_CAP: usize = 20;
        if path.is_empty() {
            return;
        }
        self.path_history.retain(|p| p != &path);
        self.path_history.insert(0, path);
        self.path_history.truncate(PATH_HISTORY_CAP);
    }

    /// Stamp a new outgoing request (mount or listing) and return its
    /// sequence value for the completion message to carry.
    pub fn next_req(&mut self) -> u64 {
        self.req_seq += 1;
        self.req_seq
    }
}

/// One terminal pane, owns its alacritty grid and (optionally) the
/// remote session feeding it. A `TerminalTab` holds one or more panes
/// in a `pane_grid::State`, which owns their split layout.
pub(crate) struct Pane {
    /// Stable identity used to route PTY output / session events to the
    /// right pane (the `pane_grid::Pane` handle is only unique within a
    /// tab's grid, this `Uuid` is unique across all tabs).
    pub id: Uuid,
    /// This pane's own connection label ("user@host", "Local Shell", ...).
    /// The tab bar shows the *focused* pane's label + icon, so a tab split
    /// across two hosts reads as whichever pane you're in.
    pub label: String,
    pub terminal: Arc<Mutex<TerminalState>>,
    /// Remote transport handle (SSH or Telnet; None for local shell).
    pub session: Option<TerminalTransport>,
    /// True while an in-place reconnect dial for this pane is in
    /// flight, making a repeat `ReconnectTab` a no-op (a held chord or
    /// an auto-reconnect tick racing a manual click must not stack a
    /// second dial). Set when the reconnect spawns the dial; cleared by
    /// every completion (`SshConnected` attach, `SshDisconnected`,
    /// `PaneConnectError`).
    pub connecting: bool,
    /// Session log ID for terminal recording.
    pub session_log_id: Option<Uuid>,
    /// Recorded bytes not yet flushed to the vault. PTY output appends
    /// here; `Oryxis::flush_session_logs` drains it (size threshold, a
    /// periodic tick, disconnect, or window close). Batching keeps the
    /// vault from taking one write per SSH chunk.
    pub session_log_buf: Vec<u8>,
    /// Recording clock zero: set on the first recorded output batch, so
    /// chunk offsets (asciicast timing) count from the session's first
    /// byte rather than the connect handshake.
    pub session_log_t0: Option<std::time::Instant>,
    /// Arrival marks into `session_log_buf`: (byte position, ms since
    /// `session_log_t0`), one per PTY output batch. The flush splits
    /// the drained bytes at newline-aligned marks so the stored chunks
    /// carry real replay timing without extra writes mid-session.
    pub session_log_marks: Vec<(usize, i64)>,
    /// Resize marks into `session_log_buf`: (byte position, ms since
    /// `session_log_t0`, cols, rows), recorded when an output batch is
    /// processed on a grid whose size differs from the last recorded
    /// one. The flush interleaves these as `kind='r'` rows between the
    /// output chunks, so replay resizes at the same stream position the
    /// live grid did (the first batch records the initial geometry).
    pub session_log_resizes: Vec<(usize, i64, u16, u16)>,
    /// Last terminal geometry written to the recording; a change
    /// appends a resize mark (output-batch path, or the flush-cadence
    /// fallback for a resize with no output after it).
    pub session_log_last_size: Option<(u16, u16)>,
    /// What this pane reconnects to when restored from a saved session group.
    /// Defaults to `Ephemeral`; the creating site overrides it to `Host` or
    /// `Local` when the pane is referenceable.
    pub origin: PaneOrigin,
    /// True while a one-shot `TerminalSyncFlush` timer is armed for this
    /// pane. A DEC `?2026` synchronized update buffers output in vte until
    /// the matching ESU, a 2 MiB overflow, or a host-driven flush; an app
    /// that opens one and then blocks on input (docker compose's `(y/N)`
    /// prompt) would otherwise freeze the screen on the pre-update frame.
    /// The flag is the rising-edge guard so a long sync burst (one
    /// `PtyOutput` per coalesced batch) arms a single timer, not one each.
    pub sync_flush_scheduled: bool,
    /// Latest window title the shell set via OSC 0/2 (`None` once an OSC
    /// ResetTitle, or never set). When auto-title is on, the tab strip shows
    /// this instead of the connection label so a tab reads as the running
    /// program / remote prompt, like every other terminal.
    pub osc_title: Option<String>,
    /// True while the visual bell flash is showing on this pane (bell mode =
    /// Flash). Set when the shell rings, cleared by a short
    /// `TerminalBellFlashEnd` timer; drives a brief overlay in the widget.
    pub bell_flash: bool,
    /// Working directory the shell last reported via OSC 7, or (fallback)
    /// parsed from the OSC 0/2 title when the shell has no OSC 7
    /// integration (default Debian/Ubuntu PS1 titles `\u@\h: \w`, so the
    /// title carries the cwd, possibly `~`-relative). Used by the sidebar
    /// Files follow and so a new local shell can open in the focused
    /// pane's directory.
    pub cwd: Option<String>,
    /// True once a real OSC 7 report arrived; from then on the title
    /// fallback is ignored (OSC 7 is exact, titles are a heuristic).
    pub cwd_from_osc7: bool,
    /// Where the remote shell stands in the OSC 133 prompt cycle, driven by
    /// the marks drained per output batch. Gates the command-history capture:
    /// only input submitted while `AtPrompt` can be a command; everything
    /// else is a running program's stdin (sudo passwords, editor keystrokes)
    /// and is never recorded.
    pub prompt: PromptState,
    /// Mirror of the remote line editor, fed with every byte of user input
    /// so the capture knows what was on the command line at Enter.
    pub input_tracker: oryxis_terminal::InputTracker,
    /// A command submitted at the prompt whose echo had not reached the grid
    /// yet (paste with a trailing newline). Resolved when `OutputStart`
    /// arrives, at which point the echoed line is read back from the grid.
    pub pending_capture: Option<PendingCapture>,
    /// In-band command capture (`OSC 633 ; E`) for this pane.
    pub inband: InbandCapture,
    /// Latest OSC 9;4 progress the shell reported, drawn as a growing border
    /// around the tab. `None` (or state 0) means no active progress.
    pub progress: Option<oryxis_terminal::Progress>,
    /// Smart tabs: the command currently running here, stamped at the OSC
    /// 133 `OutputStart` mark and resolved at `CommandEnd` / next prompt.
    /// Only integrated hosts ever set one. Cleared on disconnect (a dead
    /// transport voids any in-flight timing).
    pub running_cmd: Option<crate::smart_tabs::CommandRun>,
    /// Smart tabs: the last command line the input capture saw submitted,
    /// consumed by the next `OutputStart` to label `running_cmd`.
    pub last_submitted: Option<String>,
    /// Smart tabs: why this pane's tab wants the user's eye (attention
    /// dot on the tab strip); the tab shows its panes' highest-priority
    /// cause. Cleared when the tab is viewed.
    pub attention: Option<crate::smart_tabs::TabAttention>,
    /// Instant of the last PTY output batch, driving the quiet-period
    /// (output-after-silence) detection.
    pub last_output: Option<std::time::Instant>,
    /// ZMODEM initiation sniffer, fed every output batch while NOT already
    /// transferring. Cheap (a few bytes of held-back state); it flags a
    /// `sz` / `rz` on the remote and hands over the byte stream.
    pub zmodem_detector: oryxis_zmodem::ZmodemDetector,
    /// `Some` while a ZMODEM transfer owns this pane's byte stream: output
    /// is diverted to the driver (not the emulator) and input is frozen.
    /// Cleared when the transfer ends, which resumes the terminal.
    pub zmodem: Option<ZmodemPane>,
    /// `Some` while an OS drag-and-drop upload runs over SFTP on this
    /// pane's session (`drop.rs`). Unlike `zmodem` this does NOT divert
    /// the byte stream: the upload rides its own subsystem channel, so
    /// the terminal stays fully interactive. Drives the same overlay
    /// card the ZMODEM transfers use.
    pub drop_upload: Option<DropUploadPane>,
    /// Files from an OS drop waiting for the ZMODEM detector: the app
    /// typed `rz -y` and, when the detector sees the remote receiver
    /// start, `begin_zmodem_transfer` consumes these instead of opening
    /// the file picker. Cleared by the detect-timeout (remote has no
    /// lrzsz) and on disconnect.
    pub pending_drop_sources: Vec<std::path::PathBuf>,
    /// Screen rect of this pane's canvas as last drawn, written by a
    /// `bounds_reporter` wrapper each frame. Read by the OS-drop router
    /// to find the pane under the cursor: a split tab can hold panes on
    /// different hosts, so "the focused pane" is not always the pane the
    /// user dropped onto.
    pub bounds: crate::widgets::BoundsCell,
    /// `HintMode::Once` bookkeeping: set once the "hold Shift to select"
    /// mouse-capture toast has fired for this pane, so it retires here.
    /// In-memory only, a fresh pane (new tab / host) starts over.
    pub mouse_hint_shown: bool,
    /// `HintMode::Once` bookkeeping: set once the "hold Ctrl and click"
    /// link toast has fired for this pane, or once the user has
    /// ctrl-clicked a link here (either way the gesture is known),
    /// retiring the hint for the pane.
    pub link_hint_shown: bool,
    /// Sidebar Files tab: the SFTP browser multiplexed on this pane's
    /// SSH session. Lazily mounted; reset on disconnect.
    pub files: PaneFiles,
    /// True once the force-OSC7 PROMPT_COMMAND was injected into this
    /// pane's shell, so toggling the setting on (and reconnects) don't
    /// stack duplicate emitters. Reset on disconnect.
    pub osc7_injected: bool,
    /// Scrollback find-bar (C1): true while the overlay is shown. The match
    /// set + active index live on the widget's `TerminalState.search`; this
    /// flag and `search_query` are the app-owned UI mirror so the find-bar's
    /// `text_input` renders without locking the terminal mutex in `view()`.
    pub search_open: bool,
    /// Mirror of the find-bar needle (drives the `text_input` value).
    pub search_query: String,
    /// Broadcast input (C2): while its tab is armed (`TerminalTab.broadcast`),
    /// a pane with this set is excluded, staying an observer. Cleared when the
    /// tab disarms so a later re-arm starts clean.
    pub broadcast_opt_out: bool,
    /// Legacy keyboard modes + feature toggles (C5), RESOLVED for this pane's
    /// host at connect (a `None` on the connection resolves to
    /// `DEFAULT_QUIRKS`). Read on the hot key path (`key_to_named_bytes`) and
    /// by the widget (mouse / title / OSC 52 gates), so the vault is never
    /// consulted per keystroke. Local shells keep `DEFAULT_QUIRKS`.
    pub quirks: oryxis_core::models::terminal_quirks::TerminalQuirks,
}

/// Process-wide auto-title gate (OSC 0/2). Mirrors the `LayoutDirection`
/// global: set once at boot and whenever the user toggles it, read at
/// display time by `display_label` so the per-pane `osc_title` capture stays
/// unconditional (toggling never loses the captured title, it just hides it).
///
/// Default OFF: Oryxis is connection-oriented (like PuTTY / Termius), so the
/// curated tab label ("Local Shell", the host name) is the better default than
/// the shell's `\u@\h: \w` title. Users who want emulator-style titles (the
/// running program in the tab) opt in via the Terminal setting.
static AUTO_TITLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Enable/disable showing the shell-set OSC title in the tab strip.
pub(crate) fn set_auto_title(on: bool) {
    AUTO_TITLE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the tab strip shows the shell-set OSC title (the user setting).
pub(crate) fn auto_title_enabled() -> bool {
    AUTO_TITLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Process-wide default AI chat mode for freshly created tabs. Mirrors the
/// `AUTO_TITLE` pattern: set once at boot and whenever the user changes the
/// "Default mode" setting, read in `TerminalTab::new_single` so every tab
/// starts on the user's chosen default without threading it through every
/// construction site. Stored as the `ChatMode` discriminant (0 = Plan,
/// 1 = Ask, 2 = Auto).
static DEFAULT_CHAT_MODE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(2);

/// Set the default chat mode applied to new tabs.
pub(crate) fn set_default_chat_mode(mode: crate::state::ChatMode) {
    let v = match mode {
        crate::state::ChatMode::Plan => 0,
        crate::state::ChatMode::Ask => 1,
        crate::state::ChatMode::Auto => 2,
    };
    DEFAULT_CHAT_MODE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The default chat mode for a new tab (the user's "Default mode" setting).
pub(crate) fn default_chat_mode() -> crate::state::ChatMode {
    match DEFAULT_CHAT_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        0 => crate::state::ChatMode::Plan,
        1 => crate::state::ChatMode::Ask,
        _ => crate::state::ChatMode::Auto,
    }
}

impl Pane {
    pub fn new(label: String, terminal: Arc<Mutex<TerminalState>>) -> Self {
        Self {
            id: Uuid::new_v4(),
            label,
            terminal,
            session: None,
            connecting: false,
            session_log_id: None,
            session_log_buf: Vec::new(),
            session_log_t0: None,
            session_log_marks: Vec::new(),
            session_log_resizes: Vec::new(),
            session_log_last_size: None,
            origin: PaneOrigin::Ephemeral,
            sync_flush_scheduled: false,
            osc_title: None,
            bell_flash: false,
            cwd: None,
            cwd_from_osc7: false,
            prompt: PromptState::NoIntegration,
            input_tracker: oryxis_terminal::InputTracker::new(),
            pending_capture: None,
            inband: InbandCapture::default(),
            progress: None,
            running_cmd: None,
            last_submitted: None,
            attention: None,
            last_output: None,
            zmodem_detector: oryxis_zmodem::ZmodemDetector::new(),
            zmodem: None,
            drop_upload: None,
            pending_drop_sources: Vec::new(),
            bounds: crate::widgets::new_bounds_cell(),
            mouse_hint_shown: false,
            link_hint_shown: false,
            files: PaneFiles::default(),
            osc7_injected: false,
            search_open: false,
            search_query: String::new(),
            broadcast_opt_out: false,
            // Xterm defaults until resolved for a real host at connect.
            quirks: oryxis_core::models::terminal_quirks::DEFAULT_QUIRKS,
        }
    }

    /// Attach a fresh session log to this pane, resetting the whole
    /// recording state. A reconnect reuses the pane, and a stale clock
    /// zero / last recorded geometry would leak the previous log's
    /// timeline into the new recording: offsets counting from the old
    /// session's first byte, and no initial resize row (the grid
    /// matches the "last recorded" size, so the change detector stays
    /// quiet and replay falls back to 80x24).
    pub fn start_session_log(&mut self, log_id: Uuid) {
        self.session_log_id = Some(log_id);
        self.session_log_buf.clear();
        self.session_log_t0 = None;
        self.session_log_marks.clear();
        self.session_log_resizes.clear();
        self.session_log_last_size = None;
    }
}

/// The force-OSC7 setup: defines a helper that emits a BEL-terminated
/// OSC 7 (`file://host/cwd`), then registers it as a pre-prompt hook in
/// BOTH shell families so the terminal Files sidebar can follow the exact
/// cwd. `${HOSTNAME:-…}` covers shells that don't export HOSTNAME.
///
/// Works on bash AND zsh with no shell detection, by registering through
/// each shell's own mechanism and letting the other one no-op. bash reads
/// `PROMPT_COMMAND` (we prepend the helper, keeping any existing value),
/// and its `precmd_functions+=(…)` just creates an array bash never reads.
/// zsh has no `PROMPT_COMMAND` (that assignment sets an unused var) and
/// runs `precmd_functions`, the array we append the helper to. So the same
/// line lights up cwd following on either shell, and neither mechanism
/// errors in the other.
///
/// It also cleans up its own echo instead of leaving the setup text on
/// screen. An interactive shell runs through readline (raw mode), so the
/// tty echoes what we send and no `stty` trick can suppress it; and we
/// can't send raw control bytes as input (readline would interpret them).
/// So we send two ordinary-text commands in one write. The first, `printf
/// '\x1b7'`, saves the cursor (DECSC) at the clean prompt baseline, before
/// the big line below echoes. The second defines + registers the helper,
/// then `printf '\x1b8\x1b[1A\x1b[J'` restores the cursor (DECRC), steps
/// over the tiny first line, and erases to the end of the screen. That
/// wipes the whole echoed block regardless of how many rows it wrapped to,
/// without touching the MOTD above it. Only literal backslash escapes are
/// sent; printf turns them into the real control bytes at run time, so the
/// remote line editor only ever sees plain text. The DECSC/DECRC bytes use
/// `\x1b` hex (bounded to two hex digits) rather than octal `\033`, because
/// the octal form would merge the trailing `7` of DECSC into the escape
/// (`\0337` parses as one octal byte, not ESC + `7`); `\x1b` is safe here
/// since the feature is bash/zsh-only and both printf builtins accept
/// `\xHH`.
pub(crate) const OSC7_PROMPT_INJECT: &str = "printf '\\x1b7'\n\
     __oryxis_o7(){ printf '\\033]7;file://%s%s\\007' \
     \"${HOSTNAME:-$(hostname 2>/dev/null)}\" \"$PWD\"; }; \
     PROMPT_COMMAND=\"__oryxis_o7${PROMPT_COMMAND:+;$PROMPT_COMMAND}\"; \
     precmd_functions+=(__oryxis_o7); printf '\\x1b8\\x1b[1A\\x1b[J'\n";

/// Live state of a ZMODEM transfer that has seized a pane's byte stream.
/// While present, `PtyOutput` for the pane is routed into `wire_tx`
/// (the driver's input) instead of the emulator, and keyboard input is
/// suppressed; the fields below drive the progress overlay.
pub(crate) struct ZmodemPane {
    pub direction: oryxis_zmodem::Direction,
    /// Feeds diverted terminal output into the transfer driver.
    pub wire_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    /// Set to request a cooperative cancel (drives a ZCAN).
    pub abort: Arc<std::sync::atomic::AtomicBool>,
    /// Current file name (once the peer advertises it).
    pub file_name: Option<String>,
    /// `(k, n)` on a multi-file upload; `None` for single files and
    /// downloads.
    pub batch: Option<(usize, usize)>,
    /// Bytes moved so far, and the advertised total when known.
    pub transferred: u64,
    pub total: Option<u64>,
    /// Output that arrived after the driver ended but before its
    /// terminal `Progress` cleared the divert (the `wire_tx` send
    /// fails once the driver drops its receiver). Replayed into the
    /// emulator at teardown so a fast prompt is never swallowed.
    pub late: Vec<u8>,
}

/// Live state of an OS-drop upload running over SFTP on a pane's
/// session. Same overlay card as [`ZmodemPane`], different plumbing: the
/// upload rides its own subsystem channel, so nothing is diverted and
/// the terminal stays interactive throughout.
pub(crate) struct DropUploadPane {
    /// Current top-level entry being sent (file or folder root).
    pub file_name: Option<String>,
    /// `(k, n)` position across the drop's top-level entries.
    pub batch: Option<(usize, usize)>,
    /// Bytes moved so far across the whole drop, and the pre-walked
    /// total (known up front, unlike ZMODEM's advertised sizes).
    pub transferred: u64,
    pub total: Option<u64>,
    /// Set by the overlay's Cancel; the upload task checks it on its
    /// progress tick, aborts the in-flight file and removes the partial.
    pub abort: Arc<std::sync::atomic::AtomicBool>,
    /// Remote directory the drop lands in. Read at `Done` to refresh
    /// the sidebar Files browser when it is showing this directory, so
    /// the uploaded entries appear without a manual refresh.
    pub dest_dir: String,
}

/// Progress events streamed by the OS-drop SFTP upload task
/// (`begin_drop_sftp_upload`) back to the update loop. One terminal
/// event (`Done` / `Failed` / `Cancelled`) is guaranteed; it clears
/// [`Pane::drop_upload`] and toasts the outcome.
#[derive(Debug, Clone)]
pub(crate) enum DropProgress {
    /// Emitted once after the local walk: the whole drop's byte total.
    Plan { total: u64 },
    /// A top-level entry started uploading. `(k, n)` across entries.
    Entry { name: String, index: usize, of: usize },
    /// Cumulative bytes moved across the whole drop.
    Advanced { transferred: u64 },
    Done,
    Failed(String),
    Cancelled,
}
