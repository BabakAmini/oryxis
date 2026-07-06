//! Terminal tabs and panes (split out of `state.rs`).

use super::*;

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
    /// Last terminal geometry written to the recording; a change at
    /// flush time appends a resize event (`kind='r'`).
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
            session_log_id: None,
            session_log_buf: Vec::new(),
            session_log_t0: None,
            session_log_marks: Vec::new(),
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
            progress: None,
            running_cmd: None,
            last_submitted: None,
            attention: None,
            last_output: None,
            zmodem_detector: oryxis_zmodem::ZmodemDetector::new(),
            zmodem: None,
            mouse_hint_shown: false,
            link_hint_shown: false,
            files: PaneFiles::default(),
            osc7_injected: false,
        }
    }
}

/// The force-OSC7 setup line: a `PROMPT_COMMAND` that emits a
/// BEL-terminated OSC 7 (`file://host/cwd`) on every prompt, prepended
/// to any existing `PROMPT_COMMAND` so the user's own hook still runs.
/// bash/zsh; a shell without `PROMPT_COMMAND` ignores the assignment.
/// One line echoes on send (documented in Settings). `${HOSTNAME:-…}`
/// covers shells that don't export HOSTNAME.
pub(crate) const OSC7_PROMPT_INJECT: &str =
    "PROMPT_COMMAND='printf \"\\033]7;file://%s%s\\007\" \
     \"${HOSTNAME:-$(hostname 2>/dev/null)}\" \"$PWD\"'\"${PROMPT_COMMAND:+;$PROMPT_COMMAND}\"\n";

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
    /// Bytes moved so far, and the advertised total when known.
    pub transferred: u64,
    pub total: Option<u64>,
}

/// A terminal tab. Its panes live in an iced `pane_grid::State`, which owns
/// the split layout (N-way horizontal / vertical splits) and resizing. A
/// fresh tab has exactly one pane; the user can split it.
pub(crate) struct TerminalTab {
    pub _id: Uuid,
    pub label: String,
    /// User-set tab name from "Rename tab". Transient by design: it lives
    /// for this tab's lifetime only, is never written to the host or the
    /// pin spec, and wins over every automatic label source (session
    /// group, OSC title, pane label) in `display_label`. `None` = auto.
    pub custom_name: Option<String>,
    /// The pane tree (1+ panes). `pane_grid` owns the geometry.
    pub pane_grid: pane_grid::State<Pane>,
    /// Handle of the currently focused pane. Kept valid by the split /
    /// close / focus handlers; `active()` falls back to the first pane if
    /// it ever goes stale so we never index a closed pane.
    pub focused: pane_grid::Pane,
    /// AI chat history for this terminal session.
    pub chat_history: Vec<ChatMessage>,
    /// Whether the terminal sidebar is visible (Chat / Snippets / History
    /// tabs share this flag; the active tab is `Oryxis::terminal_sidebar_tab`).
    pub chat_visible: bool,
    /// First-token allow-list for AI tool execution. Populated when the
    /// user clicks "ALWAYS RUN" on a confirmation prompt, future tool
    /// calls whose first whitespace-delimited token matches an entry
    /// here skip the prompt and run immediately. Per-tab so an
    /// "always run rm" decision on one host doesn't leak to others.
    pub chat_always_run_commands: Vec<String>,
    /// Commands auto-executed by the AI (judge-approved or allow-listed)
    /// since the last user message. A proposed command already in this
    /// list is refused auto-execution and surfaced for explicit approval
    /// instead, the guard that stops the model re-running the same
    /// command (e.g. `docker --version`) forever. Cleared whenever the
    /// user retakes control (new message, reset, or an explicit approval).
    pub chat_auto_run_history: Vec<String>,
    /// Count of consecutive AI-auto-executed commands since the last user
    /// message. A backstop for the "many different commands" runaway that
    /// exact-repeat detection can't catch: once it passes
    /// `CHAT_AUTO_RUN_STREAK_MAX` further auto-exec is refused and the
    /// command is surfaced for explicit approval. Reset alongside
    /// `chat_auto_run_history`.
    pub chat_auto_run_streak: usize,
    /// True while a chat stream (assistant reply or a tool-followup
    /// pipeline) is in flight for THIS tab. Per-tab, not global: a chat on
    /// one tab keeps streaming while the user works in another, and the
    /// "Thinking..."/Stop affordances read the active tab's flag.
    pub chat_loading: bool,
    /// Abort handle for this tab's in-flight chat stream (reply + any
    /// detached tool-followup it feeds). Aborting drops the receiver so the
    /// detached tokio task's `tx.send` fails and it stops too. Per-tab so
    /// Stop / close / reset target the right conversation and starting a
    /// chat on one tab never cancels another's. `None` when idle.
    pub chat_task: Option<iced::task::Handle>,
    /// How this tab's assistant gates tool calls: `Auto` (allow-list +
    /// judge auto-exec safe commands), `Ask` (every command needs explicit
    /// approval), or `Plan` (read-only investigation only, writes blocked).
    /// Per-tab so it travels with the conversation; seeded from the global
    /// `ai_default_mode` setting when the tab is created.
    pub chat_mode: crate::state::ChatMode,
    /// Last time the streaming markdown re-parse ran for this tab. Throttles
    /// the O(content) parse to ~10/s during streaming. Per-tab (not a single
    /// global) because two tabs can stream at once now: a shared static would
    /// see alternating tab ids and never throttle, re-parsing every chunk.
    pub chat_last_md_parse: Option<std::time::Instant>,
    /// True for cloud SSM / ECS-Exec tabs (a `session-manager-plugin`
    /// PTY). These talk SSM over a websocket whose idle timer kills the
    /// session after ~20 min of inactivity, so they get the
    /// resize-based keepalive while the window is unfocused. Plain SSH /
    /// local tabs leave this `false`.
    pub ssm_keepalive: bool,
    /// Message that re-creates this session, for "Duplicate Tab". Set
    /// only for cloud tabs that have no saved `Connection` to look up
    /// by label (ECS Exec, kubectl pod). SSH / InstanceConnect / SSM
    /// tabs are connection-backed and duplicate via label lookup
    /// instead, so they leave this `None`.
    pub relaunch: Option<Box<crate::messages::Message>>,
    /// Set when this tab was opened from a saved session group (or just
    /// saved as one). Drives the tab context menu label ("Save group" vs
    /// "Edit group") and lets the editor update the existing group in place.
    pub session_group_id: Option<Uuid>,
    /// Pinned tabs render first in the strip (compact icon chip or a
    /// bordered tab, per the `pinned_tab_style` setting) and are restored on
    /// the next launch. Toggled from the tab context menu.
    pub pinned: bool,
    /// Set on a *dormant* pinned tab recreated at boot: the tab shows in the
    /// strip but isn't connected. The first time it's selected, this spec
    /// reopens it (connect host / spawn local shell), then clears. `None` on
    /// a live tab.
    pub pending_reopen: Option<PinnedTabSpec>,
    /// Hybrid tab state (issue #61): when set, this SSH tab shows its
    /// host's files (the full dual-pane SFTP surface) instead of the
    /// terminal. The PTY keeps running underneath; the tab glyph /
    /// status-bar segment / hotkey toggle it back.
    pub files_mode: bool,
    /// Parked SFTP browsing state for `files_mode`, hoisted into the
    /// live `Oryxis::sftp` buffer while this tab owns the surface
    /// (`hybrid_sftp_owner`), same swap-on-focus invariant as the
    /// standalone `SftpTab::state`. Boxed: most tabs never browse.
    pub files_state: Box<SftpState>,
}

/// Reference to an open tab in the unified strip. Terminal and SFTP tabs
/// share one reorderable, pinnable row; identity is by `Uuid` (stable
/// across reorder / close) rather than a vec index. Reserved for the full
/// cross-type interleave / drag-reorder (deferred): SFTP tabs render grouped
/// after terminal tabs today, so `Terminal` is not yet constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum TabRef {
    Terminal(Uuid),
    Sftp(Uuid),
}

/// An SFTP browser tab. Unlike terminal tabs, the **active** SFTP tab's
/// live state lives in `Oryxis::sftp` (a working buffer); this struct's
/// `state` field is a default placeholder while this tab is focused, and
/// holds the parked state while it is not. See the swap-on-focus invariant
/// in `SFTP_TABS_PLAN.md`: never read the active tab's state from the vec,
/// route by id through `Oryxis::route_sftp_async`.
pub(crate) struct SftpTab {
    pub id: Uuid,
    pub label: String,
    /// User-set tab name from "Rename tab". Transient, mirrors
    /// `TerminalTab::custom_name`: display-only, never persisted.
    pub custom_name: Option<String>,
    /// Pinned SFTP tabs render first in the strip.
    pub pinned: bool,
    /// Set on a dormant pinned SFTP tab recreated at boot: reopens (re-mounts
    /// its panes) the first time it's selected, then clears. Reserved for
    /// pin-restore-on-boot (deferred); not read yet.
    #[allow(dead_code)]
    pub pending_reopen: Option<PinnedTabSpec>,
    /// Parked state while this tab is not focused; a default placeholder while
    /// it IS the active tab (live state hoisted to `Oryxis::sftp`).
    pub state: SftpState,
}

impl SftpTab {
    pub(crate) fn new(label: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            label,
            custom_name: None,
            pinned: false,
            pending_reopen: None,
            state: SftpState::default(),
        }
    }

    /// Label to show in the tab strip: the user's transient rename when
    /// set, else the mount label. Lookups (host colour, detected OS)
    /// must keep using `label`, the custom name is display-only.
    pub(crate) fn display_label(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.label)
    }
}

/// Persisted restore spec for a pinned tab. Stored as JSON in the
/// `pinned_tabs` setting; on boot each becomes a dormant pinned tab that
/// reopens lazily on first select. Cloud / ephemeral tabs have no spec and
/// aren't persisted.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum PinnedTabSpec {
    /// A saved host, reopened with `ConnectSsh` (id resolved to an index
    /// fresh at reopen time, so it survives connection reordering).
    Host { id: Uuid, label: String },
    /// A local shell, reopened with the captured program / args.
    LocalShell { program: String, args: Vec<String>, label: String },
    /// An ECS Exec session, reopened with `ConnectEcsExecTask` (same
    /// mechanism the in-session reconnect uses; the task id may have
    /// recycled, in which case the reconnect re-resolves the group).
    EcsExec {
        group_id: Uuid,
        task_id: String,
        task_label: String,
        container: String,
        label: String,
    },
    /// A kubectl exec session, reopened with `ConnectKubectlExecPod`.
    KubectlExec {
        group_id: Uuid,
        namespace: String,
        pod: String,
        container: String,
        label: String,
    },
    /// A pinned SFTP browser tab. Captures both panes (Local vs which
    /// connection); reopened dormant and re-mounts its remote pane(s) on first
    /// focus.
    Sftp {
        left: SftpPaneSpec,
        right: SftpPaneSpec,
        label: String,
    },
}

/// Restore spec for one SFTP pane: Local browsing, or a remote host by saved
/// connection id (resolved fresh at reopen so it survives reordering).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum SftpPaneSpec {
    Local,
    Remote(Uuid),
}

/// In-progress drag of a tab in the strip, for reordering. Started on press
/// (`SelectTab`), promoted to `active` once the cursor moves past a small
/// threshold (so a plain click isn't a drag), committed on mouse release
/// onto the hovered tab. Reorder is restricted to within the same group
/// (pinned among pinned, normal among normal).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TabDrag {
    /// The tab being dragged, by id so it survives any reindexing (a tab
    /// closing mid-drag) and resolves to the right source at drop time.
    pub from_id: Uuid,
    /// Cursor position at press, for the move threshold.
    pub start: iced::Point,
    /// Promoted past the threshold (a real drag, not a click).
    pub active: bool,
}

impl PinnedTabSpec {
    pub fn label(&self) -> &str {
        match self {
            PinnedTabSpec::Host { label, .. } => label,
            PinnedTabSpec::LocalShell { label, .. } => label,
            PinnedTabSpec::EcsExec { label, .. } => label,
            PinnedTabSpec::KubectlExec { label, .. } => label,
            PinnedTabSpec::Sftp { label, .. } => label,
        }
    }

    /// Identity key for de-duplicating pins. Ephemeral resource ids
    /// (ECS task, K8s pod) are excluded on purpose: a recycled task
    /// produces a spec with a different task_id but it is still the
    /// same pin, and keeping both is how duplicate chips appear.
    pub fn dedupe_key(&self) -> String {
        match self {
            PinnedTabSpec::Host { id, .. } => format!("host:{id}"),
            PinnedTabSpec::LocalShell { program, args, label } => {
                format!("local:{program}:{}:{label}", args.join("\u{1f}"))
            }
            PinnedTabSpec::EcsExec { group_id, container, .. } => {
                format!("ecs:{group_id}:{container}")
            }
            PinnedTabSpec::KubectlExec { group_id, namespace, container, .. } => {
                format!("k8s:{group_id}:{namespace}:{container}")
            }
            PinnedTabSpec::Sftp { left, right, .. } => {
                let key = |p: &SftpPaneSpec| match p {
                    SftpPaneSpec::Local => "local".to_string(),
                    SftpPaneSpec::Remote(id) => format!("remote:{id}"),
                };
                format!("sftp:{}:{}", key(left), key(right))
            }
        }
    }
}

impl TerminalTab {
    /// Build a new tab with a single pane. Split it later via
    /// `pane_grid.split(...)`.
    pub fn new_single(label: String, terminal: Arc<Mutex<TerminalState>>) -> Self {
        let (pane_grid, focused) = pane_grid::State::new(Pane::new(label.clone(), terminal));
        Self {
            _id: Uuid::new_v4(),
            label,
            custom_name: None,
            pane_grid,
            focused,
            chat_history: Vec::new(),
            chat_visible: false,
            chat_always_run_commands: Vec::new(),
            chat_auto_run_history: Vec::new(),
            chat_auto_run_streak: 0,
            chat_loading: false,
            chat_task: None,
            chat_mode: default_chat_mode(),
            chat_last_md_parse: None,
            ssm_keepalive: false,
            relaunch: None,
            session_group_id: None,
            pinned: false,
            pending_reopen: None,
            files_mode: false,
            files_state: Box::default(),
        }
    }

    /// A dormant pinned tab recreated at boot: shows in the strip with the
    /// saved label but holds no live session. The placeholder pane carries a
    /// hint; selecting the tab the first time fires `spec` to reopen it.
    pub fn new_dormant_pinned(label: String, spec: PinnedTabSpec) -> Self {
        let mut term = TerminalState::new_no_pty(80, 24).unwrap();
        let hint = format!("\x1b[2m  {}\x1b[0m\r\n", crate::i18n::t("pinned_tab_dormant_hint"));
        term.process(hint.as_bytes());
        let mut tab = Self::new_single(label, Arc::new(Mutex::new(term)));
        tab.pinned = true;
        tab.pending_reopen = Some(spec);
        tab
    }

    /// Restore spec for persisting this pinned tab, or `None` if it can't be
    /// reopened (cloud / ephemeral pane with no stable reference). A dormant
    /// tab keeps the spec it was created with; a live tab derives one from
    /// its focused pane's origin.
    pub fn pin_spec(&self) -> Option<PinnedTabSpec> {
        if let Some(spec) = &self.pending_reopen {
            return Some(spec.clone());
        }
        let base = self.label.trim_end_matches(" (disconnected)").to_string();
        match &self.active().origin {
            PaneOrigin::Host(id) => Some(PinnedTabSpec::Host { id: *id, label: base }),
            // Quick-connect hosts have no stable reference to restore from
            // (the entry dies with the app), so the pin is session-only,
            // like SSM tabs.
            PaneOrigin::QuickHost(_) => None,
            PaneOrigin::Local(spec) => Some(PinnedTabSpec::LocalShell {
                program: spec.program.clone(),
                args: spec.args.clone(),
                label: spec.label.clone(),
            }),
            // Cloud exec tabs have no saved Connection, but carry the
            // relaunch message that recreates them; mirror it into a
            // serializable spec. SSM (relaunch None) and anything else stay
            // unpersisted.
            PaneOrigin::Ephemeral => match self.relaunch.as_deref() {
                Some(crate::messages::Message::ConnectEcsExecTask {
                    group_id,
                    task_id,
                    task_label,
                    container,
                }) => Some(PinnedTabSpec::EcsExec {
                    group_id: *group_id,
                    task_id: task_id.clone(),
                    task_label: task_label.clone(),
                    container: container.clone(),
                    label: base,
                }),
                Some(crate::messages::Message::ConnectKubectlExecPod {
                    group_id,
                    namespace,
                    pod,
                    container,
                }) => Some(PinnedTabSpec::KubectlExec {
                    group_id: *group_id,
                    namespace: namespace.clone(),
                    pod: pod.clone(),
                    container: container.clone(),
                    label: base,
                }),
                _ => None,
            },
        }
    }

    /// Currently focused pane. Falls back to the first pane if `focused`
    /// is stale (e.g. just after a close), so this never panics.
    pub fn active(&self) -> &Pane {
        self.pane_grid
            .get(self.focused)
            .or_else(|| self.pane_grid.panes.values().next())
            .expect("a tab always has at least one pane")
    }

    pub fn active_mut(&mut self) -> &mut Pane {
        // Resolve a valid key first (repairing `focused` if it went
        // stale), then take the mutable borrow.
        let key = if self.pane_grid.panes.contains_key(&self.focused) {
            self.focused
        } else {
            let k = *self
                .pane_grid
                .panes
                .keys()
                .next()
                .expect("a tab always has at least one pane");
            self.focused = k;
            k
        };
        self.pane_grid.get_mut(key).expect("valid pane key")
    }

    /// Look up a pane by its stable `Uuid` (for routing PTY output /
    /// session events).
    pub fn pane_by_id_mut(&mut self, id: Uuid) -> Option<&mut Pane> {
        self.pane_grid.panes.values_mut().find(|p| p.id == id)
    }

    /// Number of panes in this tab. `> 1` means the tab is split.
    pub fn pane_count(&self) -> usize {
        self.pane_grid.panes.len()
    }

    /// Label to show in the tab strip. A tab opened from (or saved as) a
    /// session group shows the group's name. Otherwise a split tab follows
    /// the *focused* pane (so a tab split across two hosts reads as whichever
    /// pane you're in); a single-pane tab uses the tab's own label, which
    /// carries the "(disconnected)" suffix the focused-pane label doesn't.
    /// Label to show in the tab strip. `auto_title` is the effective per-tab
    /// auto-title decision (resolved by the caller from the focused host's
    /// override and the global default), kept as a parameter because a
    /// `TerminalTab` can't reach the connection list to resolve it itself.
    pub fn display_label(&self, auto_title: bool) -> &str {
        // An explicit rename wins over every automatic source: the user
        // asked for this exact name, so neither the group name nor a
        // shell-set OSC title may overwrite it.
        if let Some(name) = self.custom_name.as_deref() {
            return name;
        }
        self.auto_label(auto_title)
    }

    /// The automatic label, ignoring any user rename. This is what
    /// lookups (host accent, detected-OS badge) key on: a custom name is
    /// display-only and must never leak into a `Connection`-by-label
    /// match.
    pub fn auto_label(&self, auto_title: bool) -> &str {
        // A session group keeps its own name; OSC titles never override it.
        if self.session_group_id.is_some() {
            return &self.label;
        }
        // The focused pane's shell-set title wins when auto-title is on, so
        // the tab reads as the running program / remote prompt.
        if auto_title
            && let Some(t) = self.active().osc_title.as_deref()
            && !t.is_empty()
        {
            return t;
        }
        if self.pane_count() > 1 {
            &self.active().label
        } else {
            &self.label
        }
    }
}


#[cfg(test)]
mod terminal_tab_tests {
    use super::*;

    fn dummy_terminal() -> Arc<Mutex<TerminalState>> {
        Arc::new(Mutex::new(TerminalState::new_no_pty(80, 24).unwrap()))
    }

    fn split(tab: &mut TerminalTab, axis: pane_grid::Axis) -> pane_grid::Pane {
        let (handle, _) = tab
            .pane_grid
            .split(axis, tab.focused, Pane::new("p".into(), dummy_terminal()))
            .expect("split");
        tab.focused = handle;
        handle
    }

    #[test]
    fn split_then_close_keeps_focused_on_a_live_pane() {
        let mut tab = TerminalTab::new_single("t".into(), dummy_terminal());
        assert_eq!(tab.pane_grid.panes.len(), 1);
        split(&mut tab, pane_grid::Axis::Vertical);
        split(&mut tab, pane_grid::Axis::Horizontal);
        assert_eq!(tab.pane_grid.panes.len(), 3);

        // Close the focused pane the way `ClosePane` does, then point
        // `focused` at the sibling that took over.
        let (_, sibling) = tab.pane_grid.close(tab.focused).expect("close");
        tab.focused = sibling;
        assert_eq!(tab.pane_grid.panes.len(), 2);

        // `active()` must resolve to one of the surviving panes, never panic.
        let active_id = tab.active().id;
        assert!(tab.pane_grid.panes.values().any(|p| p.id == active_id));
    }

    #[test]
    fn active_falls_back_when_focused_is_stale() {
        let mut tab = TerminalTab::new_single("t".into(), dummy_terminal());
        let handle = split(&mut tab, pane_grid::Axis::Vertical);
        // Close the focused pane WITHOUT repairing `focused` (simulating a
        // missed update): `active()` must still return a live pane.
        tab.pane_grid.close(handle);
        let _ = tab.active().id; // must not panic
        // `active_mut()` repairs `focused` to a valid handle.
        let id = tab.active_mut().id;
        assert!(tab.pane_grid.panes.values().any(|p| p.id == id));
    }

    #[test]
    fn pane_by_id_mut_targets_the_right_pane() {
        let mut tab = TerminalTab::new_single("t".into(), dummy_terminal());
        let id1 = tab.active().id;
        let h2 = split(&mut tab, pane_grid::Axis::Vertical);
        let id2 = tab.pane_grid.get(h2).unwrap().id;
        assert_ne!(id1, id2);
        assert_eq!(tab.pane_by_id_mut(id1).map(|p| p.id), Some(id1));
        assert_eq!(tab.pane_by_id_mut(id2).map(|p| p.id), Some(id2));
        assert!(tab.pane_by_id_mut(Uuid::new_v4()).is_none());
    }

    #[test]
    fn custom_name_wins_over_every_automatic_label_source() {
        let mut tab = TerminalTab::new_single("host-a".into(), dummy_terminal());
        assert_eq!(tab.display_label(true), "host-a");

        // Custom name beats the plain label...
        tab.custom_name = Some("prod db".into());
        assert_eq!(tab.display_label(true), "prod db");
        // ...an OSC title with auto-title on...
        tab.active_mut().osc_title = Some("vim main.rs".into());
        assert_eq!(tab.display_label(true), "prod db");
        // ...and the session-group name.
        tab.session_group_id = Some(Uuid::new_v4());
        assert_eq!(tab.display_label(true), "prod db");

        // `auto_label` keeps ignoring the rename, so lookups (host
        // accent, OS badge) still key on the automatic label.
        assert_eq!(tab.auto_label(true), "host-a");

        // Clearing the name restores the automatic sources.
        tab.custom_name = None;
        tab.session_group_id = None;
        assert_eq!(tab.display_label(true), "vim main.rs");
        assert_eq!(tab.display_label(false), "host-a");
    }

    #[test]
    fn sftp_custom_name_is_display_only() {
        let mut tab = SftpTab::new("host-a".into());
        assert_eq!(tab.display_label(), "host-a");
        tab.custom_name = Some("files".into());
        assert_eq!(tab.display_label(), "files");
        assert_eq!(tab.label, "host-a");
        tab.custom_name = None;
        assert_eq!(tab.display_label(), "host-a");
    }
}
