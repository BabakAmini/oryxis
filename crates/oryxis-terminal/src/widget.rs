use crate::backend::TerminalBackend;
use crate::colors::TerminalPalette;
use crate::mouse::{self as mouse_report, Mods as ReportMods, MouseButton as ReportButton, MouseEventKind};
use crate::pty::PtyHandle;

/// Common result type for terminal operations.
pub type TerminalResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::CursorShape;

use iced::alignment;
use iced::widget::canvas::{self, Action as CanvasAction, Frame, Geometry, Text as CanvasText};
use iced::{keyboard, mouse, Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme};

use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Bundled glyph-fallback font for the Unicode Private Use Area
/// (Powerline / Font Awesome / Devicons / Octicons / Codicons /
/// Material). Points at Symbols Nerd Font (loaded into the fontdb
/// in `main.rs` via `include_bytes!`) rather than SauceCodePro Nerd
/// Font: cosmic-text's canvas `font:` parameter is a hard pick, not
/// a fallback chain, so any PUA codepoint SauceCodePro happens to
/// miss (Material Design Icons + some Codicons in certain patched
/// builds) would render as tofu instead of falling through. Symbols
/// Nerd Font is the official NF "symbols-only" drop-in built for
/// universal PUA coverage, so we route every PUA codepoint to it.
const NERD_FONT: Font = Font::new("Symbols Nerd Font");

mod clipboard;
mod highlight;
mod perf;
pub mod search;
mod selection;
mod state;
mod builder;
mod draw;
mod events;

pub use clipboard::wrap_paste;
pub use selection::Selection;
pub use state::{HoveredLink, TerminalState};

/// Callback for a terminal context-menu request: `(x, y, selection)` ->
/// app message, where `selection` is the live selection's text (`None`
/// when empty). Captured here because the selection lives in the
/// widget's internal state, out of the app's reach. Aliased so the
/// boxed closure doesn't trip clippy's complex-type lint at the field.
type ContextMenuFn<Message> = Box<dyn Fn(f32, f32, Option<String>) -> Message>;

/// What a right-click does in the terminal, the three PuTTY schemes.
/// The single authority for the gesture: `right_click_copy` (the
/// copy-on-select "copy on right-click" sub-option) is honoured only
/// under [`Paste`](RightClickAction::Paste).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightClickAction {
    /// Open a context menu (Windows Terminal / iTerm default).
    Menu,
    /// Paste the clipboard, the current Oryxis default and PuTTY's
    /// X11-compromise scheme. Also the only mode where
    /// `right_click_copy` applies (copy-over-selection).
    #[default]
    Paste,
    /// Extend the current selection to the click point (xterm), moving
    /// its nearer boundary, then copy.
    Extend,
}

pub(crate) use clipboard::{open_url, set_clipboard_text};
pub(crate) use highlight::*;
// Shared with the app-side session-log redaction so both sides agree on
// what is IPv6-shaped.
pub use highlight::{
    ipv4_is_private_or_loopback, ipv6_is_local, looks_like_ipv6,
    PrivacyClasses,
};
pub(crate) use perf::*;
pub(crate) use selection::{next_click_count, union_selection, SelectGranularity};

// ---------------------------------------------------------------------------
// Canvas widget state (per-instance, managed by Iced)
// ---------------------------------------------------------------------------

/// Link-quality readout for the perf HUD's `net` row, a plain-data
/// mirror of the SSH engine's probe snapshot so this crate stays
/// transport-agnostic. All figures are milliseconds from the rolling
/// RTT probe window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NetHud {
    /// Most recent successful round trip.
    pub rtt_ms: Option<f32>,
    /// Mean round trip over the window.
    pub avg_rtt_ms: Option<f32>,
    /// Worst round trip in the window (TCP loss shows up here as
    /// spikes; raw loss is invisible above TCP).
    pub peak_rtt_ms: Option<f32>,
    /// Mean absolute difference between consecutive round trips.
    pub jitter_ms: Option<f32>,
    /// Probes that went unanswered in the window.
    pub lost: usize,
    /// Seconds since the server last answered, present only while the
    /// link is currently unresponsive (drives the "no reply" banner).
    pub silent_for_secs: Option<f32>,
}

/// Set by the draw pass whenever at least one privacy redaction bar was
/// actually drawn this frame (issue #78). The app swaps it on its update
/// loop to fire the one-shot "hover to peek, click to pin" hint toast; a
/// process-wide flag because the draw path has no message channel (same
/// spirit as the bounds-reporter slots).
static PRIVACY_MASK_DRAWN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Swap-read the first-mask signal (see [`PRIVACY_MASK_DRAWN`]).
pub fn take_privacy_mask_drawn() -> bool {
    PRIVACY_MASK_DRAWN.swap(false, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Default)]
pub struct TerminalWidgetState {
    selecting: bool,
    selection: Option<Selection>,
    /// X11 PRIMARY selection: the text of the last completed selection,
    /// remembered independently of whether `selection` is still
    /// highlighted. Set by selecting (not by any setting, and not by the
    /// clipboard), so it outlives the highlight that clear-on-keypress
    /// drops. Read by middle-click paste and the paste-selection action.
    /// xterm exposes the same idea to users as the `keepSelection`
    /// resource.
    primary_selection: Option<String>,
    /// Where `primary_selection` was captured: the selection range plus
    /// the grid column count at capture. Drawn as a faint "ghost" band
    /// once the live highlight is gone, illustrating what a PRIMARY
    /// paste will insert; a new selection replaces it. The column count
    /// guards a resize: reflow moves lines, so a stale range would band
    /// unrelated cells. The line total guards grid ROTATION for the same
    /// reason: our line coordinates are raw, so once output pushes the
    /// screen (total_lines grows) the range points at different content.
    /// Typing on the current line rotates nothing, so the
    /// select-type-paste flow keeps its ghost. Never drawn in alt-screen
    /// (the region belongs to the main grid, which the alt app is
    /// covering). Drawn in BOTH clipboard modes: under `copy_on_select`
    /// the last selection IS the clipboard, so the band stays an honest
    /// cue for the paste gestures either way.
    primary_ghost: Option<(Selection, u16, usize)>,
    /// Lines scrolled back (0 = bottom). A `Cell` so the immutable-`&self`
    /// draw can reset it to the live edge on new output (PuTTY's "reset
    /// scrollback on display activity"); every other mutation is in
    /// `update` under `&mut State`, where `Cell` is equally fine.
    scroll_offset: std::cell::Cell<i32>,
    /// `render_epoch` observed by the last draw, so the next draw can
    /// tell whether new terminal activity landed (drives the
    /// reset-on-output behavior). `None` before the first draw.
    last_draw_epoch: std::cell::Cell<Option<u64>>,
    /// Sub-cell pixel remainder carried across wheel events that arrive
    /// as `ScrollDelta::Pixels` (Windows precision touchpads / high-res
    /// wheels deliver a few pixels per notch). Truncating each event to
    /// whole cells floored every sub-cell delta to zero, so scrollback
    /// never moved on those devices (issue #91: the live pane hid it
    /// behind reset-on-output, the transcript viewer had no output to
    /// snap it back). Accumulate the pixels, emit whole cells, keep the
    /// remainder. Reset when the sign flips so a direction change is
    /// responsive rather than fighting a stale opposite-sign residual.
    scroll_px_residual: std::cell::Cell<f32>,
    /// True while the cursor is somewhere over the terminal canvas. Drives
    /// the scrollbar's hover-to-reveal visibility.
    hover: bool,
    /// `Some((cursor_y_at_press, scroll_offset_at_press))` while the user
    /// is dragging the scrollbar thumb.
    scrollbar_drag: Option<(f32, i32)>,
    /// Latest known modifier mask, refreshed on every keyboard event.
    /// Drives the Ctrl+Click-to-open-link UX (Termius-style: plain
    /// clicks select, Ctrl+Click follows the URL).
    modifiers: iced::keyboard::Modifiers,
    /// Currently hovered URL + the cursor pixel position. Used by the
    /// canvas to underline only the hovered URL (not all of them) and
    /// to show the pointer cursor over it.
    hovered_url: Option<(String, iced::Point)>,
    /// Per-row cell extents `(visible_row, start_col, end_col)` of the OSC 8
    /// hyperlink currently hovered, used to underline it. One entry per grid
    /// row the link wraps onto (empty when not over a link). Regex URLs derive
    /// their extent from the per-frame highlight scan, but an explicit OSC 8
    /// link isn't in that scan (its label need not look like a URL), so its
    /// run is captured here at hover time while the grid lock is held.
    hovered_osc8: Vec<(u16, u16, u16)>,
    /// Last `(col, row)` the URL hover detection ran for. Used to skip
    /// the lock + per-cell scan on sub-cell mouse moves, at typical
    /// font sizes the cursor crosses many pixels per cell, and running
    /// the full URL scan on every pixel contends with `state.process`
    /// when the SSH echo lands at the same time, showing up as typing
    /// lag.
    hovered_cell: Option<(u16, u16)>,
    /// Button currently held down while the remote app has mouse
    /// tracking on. Drives drag-motion reports (which carry the held
    /// button) and the matching release report. `None` when no button
    /// is down or the app isn't tracking the mouse.
    report_button: Option<ReportButton>,
    /// Last `(col, row)` reported to the remote app, used to suppress
    /// duplicate motion reports while the cursor stays inside one cell.
    report_cell: Option<(u16, u16)>,
    /// Per-drag guard: set once the "mouse tracking is swallowing your
    /// selection" hint has fired during the current drag, so the many
    /// motion events of one gesture emit a single hint. Reset on each
    /// button press (start of a new drag). Cross-drag / per-pane
    /// suppression lives in app state (`Pane::mouse_hint_shown` +
    /// `HintMode`), which unwires the callback entirely once retired.
    mouse_hint_emitted: bool,
    /// Previous left-click as `(time, position, count)`, used to classify
    /// the next press as single / double / triple / quad (300 ms / 6 px
    /// window). Rolled here rather than via `iced`'s `mouse::Click` because
    /// that caps at triple and we need a fourth count for paragraph select.
    last_click: Option<(std::time::Instant, Point, u8)>,
    /// `Some((granularity, anchor_cell))` while a double/triple-click
    /// selection is active, so a drag extends by whole words/lines
    /// instead of by cell. `None` for a plain single-click drag.
    select_anchor: Option<(SelectGranularity, (u16, i32))>,
    /// Last grid cell the word/line drag recomputed against. Throttles
    /// the union recompute to one per cell crossing (the recompute locks
    /// the mutex + runs two semantic searches; running it per pixel
    /// would contend with the SSH echo path, see the URL-hover note).
    last_extend_cell: Option<(u16, i32)>,
    /// Time of the last edge auto-scroll step. Rate-limits the scroll so
    /// its speed is tied to wall-clock, not the (very high) mouse-move
    /// event rate, which otherwise made the buffer rocket past the edge.
    last_autoscroll: Option<std::time::Instant>,
    /// Privacy-span values the user click-pinned visible. A plain click
    /// on a masked span toggles its value here; every occurrence of a
    /// pinned value renders unmasked until clicked again. Keyed by the
    /// span text (not its cells) so the reveal survives scrolling and
    /// re-prints of the same value.
    pinned_privacy: std::collections::HashSet<String>,
    /// Panel rect (widget-local coords) the perf HUD occupied on the last
    /// draw. Used by `update` to hit-test the click that toggles compact /
    /// full-name metric labels and by `mouse_interaction` for the pointer
    /// cursor. `None` until the HUD has drawn once. A `Cell` because it is
    /// written from the immutable-`&self` draw path.
    hud_rect: std::cell::Cell<Option<Rectangle>>,
    /// Whether the cursor sat over a privacy span on the last draw
    /// (issue #78). Read by `mouse_interaction` for the pointer cursor,
    /// the same "click does something here" affordance links get
    /// (clicking pins the reveal). A `Cell` because it is written from
    /// the immutable-`&self` draw path, like `hud_rect`.
    hovered_privacy: std::cell::Cell<bool>,
    /// True between a left press that landed on the perf HUD and its
    /// release, so the release can't fall through to the selection /
    /// privacy-pin handling for the cells underneath the panel.
    hud_pressed: bool,
    /// Tessellated grid geometry from the last miss, kept across frames.
    /// A draw whose [`RenderKey`] matches `last_render_key` returns this
    /// cached geometry without re-running the (expensive) snapshot + glyph
    /// build. Uses interior mutability, so a `&self` draw can still refill
    /// it. Invalidated by an explicit `clear()` on any key change.
    geometry_cache: canvas::Cache,
    /// The `RenderKey` the cached geometry was built for, or `None` before
    /// the first draw. Stored in a `Cell` so the immutable-`&State` draw can
    /// update it. `RenderKey` is `Copy`, so no allocation on the hot path.
    last_render_key: std::cell::Cell<Option<RenderKey>>,
}

/// Everything a single grid geometry depends on, other than the content
/// revision that [`TerminalState::render_epoch`] tracks. Two draws with an
/// equal key produce byte-identical grid geometry, so the canvas cache can
/// be reused. Kept `Copy` (hashes stand in for the variable-length privacy
/// sets) so it lives in a `Cell` with no per-frame allocation.
///
/// Deliberately excluded: the visual-bell flash and the perf HUD, both of
/// which are drawn as their own always-fresh layers on top of the cached
/// grid, so toggling either never invalidates the grid tessellation.
#[derive(Clone, Copy, PartialEq)]
struct RenderKey {
    /// `TerminalState::render_epoch` snapshot: covers grid content, cursor
    /// position/shape, alt-screen mode, scrollback size and palette.
    epoch: u64,
    /// Raw (unclamped) scrollback offset; combined with `epoch` this fixes
    /// the clamped value the draw actually uses.
    scroll_offset: i32,
    selection: Option<Selection>,
    /// The PRIMARY ghost band's range when it is eligible to draw (no
    /// live selection, copy_on_select off). Alt-screen and resize
    /// transitions arrive with an epoch bump, so eligibility here can
    /// skip those two checks; folding the range in makes the demote
    /// (selection cleared with no output, e.g. a click on blank space)
    /// repaint the band without waiting for output.
    ghost: Option<Selection>,
    /// Hovered URL quantized to its cell, so sliding along one URL doesn't
    /// rebuild every pixel. `None` when not over a detected URL.
    hovered_url_cell: Option<(u16, u16)>,
    /// Digest of the per-row OSC 8 underline extents (0 when not over an
    /// allowed link), so a wrapped-link hover invalidates the cached grid.
    hovered_osc8: u64,
    /// Only folded in under Privacy Mode (the sole draw-time consumer), so a
    /// bare hover move doesn't invalidate the grid when privacy is off.
    hovered_cell: Option<(u16, u16)>,
    /// Scrollbar visibility inputs (it only shows while hovering / dragging /
    /// selecting *this* canvas, so these never fire on unrelated UI churn).
    hover: bool,
    scrollbar_dragging: bool,
    selecting: bool,
    privacy: bool,
    keyword_highlight: bool,
    performance: bool,
    smart_contrast: bool,
    bold_is_bright: bool,
    /// Order-independent digest of `privacy_terms` (0 when privacy is off).
    privacy_terms_hash: u64,
    /// Per-class privacy gates (issue #78): flipping a class in
    /// Settings must invalidate the cached geometry or stale masks
    /// linger until the next content epoch.
    privacy_classes: PrivacyClasses,
    /// Order-independent digest of the click-pinned privacy set (0 when off).
    pinned_privacy_hash: u64,
    /// `BufferSearch::generation` (0 when no search is open): bumped on every
    /// query change / step / rebuild / open / close so the match overlay
    /// invalidates the cached grid geometry.
    search_generation: u64,
    font: Font,
    font_size: f32,
    cell_w: f32,
    cell_h: f32,
}

/// Ordered digest of the OSC 8 underline extents (0 when empty), so the render
/// key changes as the hovered link's wrapped rows change. Empty maps to 0 so a
/// no-link frame matches a no-link frame without a hash round trip.
fn hash_osc8(segments: &[(u16, u16, u16)]) -> u64 {
    use std::hash::{Hash, Hasher};
    if segments.is_empty() {
        return 0;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    segments.hash(&mut h);
    h.finish()
}

/// Deterministic digest of an ordered string list (used for `privacy_terms`).
fn hash_terms(terms: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    terms.len().hash(&mut h);
    for t in terms {
        t.hash(&mut h);
    }
    h.finish()
}

/// Order-independent digest of a string set: XOR of each element's hash,
/// mixed with the count. `HashSet` iteration order is non-deterministic, so
/// a per-element XOR (which the ordering can't perturb) is what keeps the
/// key stable frame to frame while still changing on any add/remove.
fn hash_pinned(set: &std::collections::HashSet<String>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut acc = set.len() as u64;
    for s in set {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        acc ^= h.finish();
    }
    acc
}

// ---------------------------------------------------------------------------
// Terminal View
// ---------------------------------------------------------------------------

/// A terminal gesture the widget performs itself because it owns the
/// state involved: the selection and the scroll offset both live in
/// this widget's canvas state, out of reach of the app's dispatcher.
///
/// Paste is deliberately absent. It stays in the app, which is the only
/// layer that can reach an SSH session; a widget-side paste would write
/// to a local PTY only, and silently do nothing on a remote host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalChordAction {
    Copy,
    /// Paste the X11 PRIMARY selection (the remembered text of the last
    /// completed selection) into this pane: the keyboard twin of
    /// middle-click. Performed app-side through `on_paste_selection`,
    /// which is the only layer that reaches an SSH session. Never touches
    /// the system clipboard and leaves the highlight in place.
    PasteSelection,
    SelectAll,
    ScrollPageUp,
    ScrollPageDown,
}

/// Resolves a key event to a [`TerminalChordAction`], or `None` when the
/// event isn't one of the widget's chords.
///
/// The app owns the binding model (chords are user-editable), so it
/// hands the matcher down as a closure rather than teaching this crate
/// about actions and bindings. That keeps ONE implementation of chord
/// matching: a copy of it here would drift from the editor's.
pub type ChordResolver = Box<dyn Fn(&keyboard::Key, &keyboard::Modifiers) -> Option<TerminalChordAction>>;

/// What a bound mouse button does when it is pressed over the canvas.
///
/// The split mirrors the keyboard side: gestures that need canvas state
/// (selection, scroll offset) are performed here, everything else is a
/// message for the app, which is the only layer that reaches an SSH
/// session.
pub enum MouseGesture<Message> {
    /// One of the widget's own gestures, run in place.
    Widget(TerminalChordAction),
    /// Hand this message to the app.
    Publish(Message),
}

/// Resolves a mouse press to a [`MouseGesture`], or `None` when no
/// binding claims that button with those modifiers.
///
/// Same contract as [`ChordResolver`]: the app owns the (user-editable)
/// binding model and hands a matcher down, so there is exactly ONE
/// implementation of binding matching. Left and Right are never
/// resolved: they are the canvas's own select / right-click-scheme
/// gestures, so the binding editor refuses them.
pub type MouseResolver<Message> =
    Box<dyn Fn(mouse::Button, &keyboard::Modifiers) -> Option<MouseGesture<Message>>>;

pub struct TerminalView<Message = ()> {
    state: Arc<Mutex<TerminalState>>,
    /// User-bound chords for the gestures this widget performs. `None`
    /// falls back to no chords at all (the harness and any other caller
    /// that doesn't wire the binding table).
    chords: Option<ChordResolver>,
    font_size: f32,
    cell_width: f32,
    cell_height: f32,
    font: Font,
    /// When true, completing a mouse selection auto-copies it to the
    /// system clipboard, same UX as XTerm / iTerm "copy on select".
    copy_on_select: bool,
    /// Only consulted when `copy_on_select` is on. When true the selection
    /// no longer auto-copies on release; instead a right-click over a live
    /// selection copies it (the Windows console "QuickEdit" model), and a
    /// right-click with no selection still pastes.
    right_click_copy: bool,
    /// User-bound mouse buttons (middle-click paste out of the box).
    /// `None` = no mouse gestures at all, same fallback as `chords` for
    /// callers that don't wire the binding table.
    mouse_bindings: Option<MouseResolver<Message>>,
    /// What a right-click does (PuTTY's three schemes). The single
    /// authority for the gesture; see [`RightClickAction`].
    right_click_action: RightClickAction,
    /// Jump back to the live edge on new terminal output (PuTTY's "reset
    /// scrollback on display activity").
    reset_scroll_on_output: bool,
    /// When true, ANSI bold flag promotes the named foreground color to
    /// its bright variant (red → bright red, etc).
    bold_is_bright: bool,
    /// When true, the terminal scans visible rows for URLs / IPs / paths
    /// and tints them. Disable to recover frame time in dense UIs.
    keyword_highlight: bool,
    /// Performance mode: skip the per-frame highlight scan (keyword
    /// tinting plus URL / IP / path detection) to save CPU on weak or
    /// software render paths. The scan still runs when
    /// [`privacy`](Self::privacy) is on, because Privacy Mode masks the
    /// spans that same scan produces.
    performance: bool,
    /// Draws the per-phase timing HUD in the top-right of the pane.
    /// ORed with the `ORYXIS_TERM_PERF` env var at draw time.
    perf_overlay: bool,
    /// Link-quality figures for the HUD's `net` row, provided by the app
    /// from the SSH session's RTT probe window. `None` for panes without
    /// a probed transport (local shell, telnet, serial), which simply
    /// omits the row.
    net_hud: Option<NetHud>,
    /// Privacy Mode: when true, detected IP addresses and `user@host`
    /// prompt tokens are masked with muted block glyphs and revealed only
    /// when the cursor hovers their span. Runs independently of
    /// `keyword_highlight` (detection happens even when tinting is off).
    privacy: bool,
    /// Saved-connection hostnames masked literally under Privacy Mode
    /// (lowercase, set via [`TerminalView::with_privacy_terms`]). Plain
    /// DNS names have no detectable shape, so the known values are
    /// matched exactly instead of guessed.
    privacy_terms: Vec<String>,
    /// Per-class Privacy Mode gates (issue #78): which detector
    /// classes may mask (public IPs / private IPs / username shapes).
    /// The terms list above is class-filtered app-side, so it carries
    /// no flag here. All on by default; irrelevant while `privacy` is
    /// off.
    privacy_classes: PrivacyClasses,
    /// When true, cells whose foreground and background end up
    /// perceptually too close (e.g. PowerShell's `$PSStyle.FileInfo
    /// .Directory` blue-on-blue, LS_COLORS' `ow` green-on-green) get
    /// their foreground swapped for a high-contrast alternative so
    /// the text stays legible. Off paints the cell exactly as the
    /// emulator asked, which a few colour-precise tools rely on.
    smart_contrast: bool,
    /// Whether this pane honours remote mouse-tracking requests (C5). When
    /// false (a host with `disable_mouse_reporting`), clicks always
    /// select / paste locally even while the remote enabled tracking, so a
    /// broken or hostile remote can't hijack the mouse. Default true.
    mouse_reporting: bool,
    /// Characters that terminate a word for double-click selection
    /// (the semantic-escape / "word delimiters" set). Threaded from the
    /// user's Terminal setting each frame and synced into the backend on
    /// the next word-select. Defaults to [`crate::backend::DEFAULT_WORD_DELIMITERS`].
    word_delimiters: String,
    /// Optional callback messages for Ctrl+Wheel font zoom. When unset,
    /// Ctrl+Wheel still gets captured but produces no state change.
    on_font_size_increase: Option<Message>,
    on_font_size_decrease: Option<Message>,
    /// Optional callback for right-click paste. When set, the widget
    /// emits this message instead of writing the clipboard text directly
    /// to the local PTY, so the app dispatcher can route to the SSH
    /// session (mirroring the Ctrl+Shift+V path).
    on_paste_request: Option<Message>,
    /// Emitted with the PRIMARY selection's text by middle-click and by
    /// the paste-selection chord. Carries the text because PRIMARY lives
    /// in this widget's state, while the paste has to happen app-side:
    /// only the dispatcher reaches an SSH session, and only it owns the
    /// careful-paste / paste-guard gates. Not wired = both gestures fall
    /// back (middle-click to a clipboard paste, the chord to nothing).
    on_paste_selection: Option<Box<dyn Fn(String) -> Message>>,
    /// Emitted (with window-absolute x, y and whether a selection is
    /// live) when a right-click should open the context menu
    /// (`right_click_action == Menu`). The app renders + drives the menu
    /// through its overlay pipeline.
    on_context_menu: Option<ContextMenuFn<Message>>,
    /// Optional callback for raw input bytes the widget synthesizes
    /// (mouse-tracking reports, wheel-to-arrow translation). Like
    /// `on_paste_request`, this routes the bytes through the dispatcher
    /// so they reach the active SSH session; without it the widget
    /// falls back to a local-PTY write, which is dead on SSH tabs.
    on_terminal_input: Option<Box<dyn Fn(Vec<u8>) -> Message>>,
    /// Optional callback fired the first time the user left-drags inside a
    /// pane whose remote app has mouse tracking on (so the drag is being
    /// reported instead of selecting text). Lets the app surface the
    /// "hold Shift to select" hint at the exact moment selection is being
    /// swallowed, rather than at TUI launch. Fires at most once per pane.
    on_mouse_capture_hint: Option<Box<dyn Fn() -> Message>>,
    /// Optional callback fired when a plain (no Ctrl) click lands on a
    /// URL: the user likely expected the link to open, so the app can
    /// show a "hold Ctrl and click" toast at the exact moment the
    /// gesture missed. Mirrors `on_mouse_capture_hint`; the app stops
    /// wiring it once the hint has been taught for the pane.
    on_link_click_hint: Option<Box<dyn Fn() -> Message>>,
    /// Emitted after a Ctrl+Click successfully opens a URL, so the app
    /// can persist "the user knows the gesture" and drop the hint.
    on_link_opened: Option<Message>,
    /// Whether this pane currently has focus. Only the focused pane emits
    /// mouse-tracking reports, so a click that merely focuses an inactive
    /// split pane (e.g. one running htop, which leaves mouse mode on)
    /// doesn't inject a stray report into that shell. Defaults to `true`
    /// so the single-pane path is unchanged.
    focused: bool,
    /// Per-edge strip, in pixels, that this pane hands back to whatever
    /// contains it: `(top, right, bottom, left)`.
    ///
    /// A `pane_grid` divider is grabbable within `spacing + leeway` of the
    /// split line, but the canvas fills its pane and gets the press first
    /// (the grid forwards to its child unconditionally), so outside the
    /// spacing itself a drag on the divider also starts a text selection.
    /// With no spacing at all the divider would be unreachable. Declining
    /// presses in these strips is what lets the panes sit flush and still
    /// be resizable: the host sets a non-zero margin only on edges that
    /// actually border another pane, so the outermost edges keep their
    /// full selectable area.
    resize_margins: (f32, f32, f32, f32),
    /// When true, paint a brief translucent overlay over the whole pane this
    /// frame, the visual bell (bell mode = Flash). Driven by `Pane.bell_flash`,
    /// which a short timer clears.
    bell_flash: bool,
    /// When true, the grid keeps whatever geometry the backend holds
    /// instead of auto-fitting the canvas bounds each draw. Replay
    /// surfaces (the session player) pin the grid to the recording's
    /// geometry; the host sizes the canvas with [`grid_pixel_size`] so
    /// bounds and grid agree. Default false (live panes auto-fit).
    fixed_grid: bool,
}

/// Horizontal padding around the terminal content (left/right).
/// Termius uses ~8 px so the first column doesn't kiss the window
/// border, matched here.
const TERM_PAD: f32 = 8.0;
/// Vertical padding above the first row. Mirrors `TERM_PAD` so
/// horizontal and vertical breathing are symmetric, again matching
/// the Termius spacing. If the canvas still looks padded above the
/// first row of output, the gap isn't coming from here; likely the
/// remote session emits a leading clear / cursor-move sequence that
/// blanks the top rows.
const TERM_PAD_TOP: f32 = 8.0;

/// Screen-space rectangle for the OS IME candidate window, anchored at the
/// terminal caret. `bounds` is the widget's on-screen rect, `font_size` the
/// configured terminal font size, `cell` the cursor cell from
/// [`TerminalState::cursor_cell`]. Mirrors the cursor-rendering math in
/// `draw` so the candidate window lines up with the block cursor.
pub fn ime_caret_rect(
    bounds: Rectangle,
    font_size: f32,
    font_name: Option<&str>,
    cell: (u16, u16),
) -> Rectangle {
    let font = match font_name {
        Some(name) => Font::new(intern_font_name(name)),
        None => Font::MONOSPACE,
    };
    let cell_w = cell_advance(font, font_size);
    let cell_h = font_size * 1.15;
    let (col, row) = cell;
    let x = bounds.x + col as f32 * cell_w + TERM_PAD;
    let y = bounds.y + row as f32 * cell_h + TERM_PAD_TOP;
    Rectangle::new(Point::new(x, y), Size::new(cell_w.max(1.0), cell_h))
}

/// Visual layout of the scrollbar gutter for a given grid state.
struct ScrollbarGeom {
    track_x: f32,
    track_y: f32,
    track_w: f32,
    track_h: f32,
    thumb_y: f32,
    thumb_h: f32,
    history_size: i32,
}

/// Compute the scrollbar geometry for the given canvas bounds and current
/// grid + scroll state. Returns `None` when there's no history to scroll.
fn scrollbar_geom(
    bounds: Rectangle,
    total_lines: usize,
    screen_lines: usize,
    scroll_offset: i32,
) -> Option<ScrollbarGeom> {
    let history_size = (total_lines.saturating_sub(screen_lines)) as i32;
    if history_size <= 0 {
        return None;
    }
    let track_x = bounds.width - 8.0;
    let track_w = 6.0;
    let track_y = TERM_PAD_TOP;
    let track_h = (bounds.height - TERM_PAD_TOP - TERM_PAD).max(0.0);
    let total = total_lines as f32;
    let visible = screen_lines as f32;
    let thumb_h = (track_h * (visible / total)).max(24.0).min(track_h);
    let progress = scroll_offset as f32 / history_size as f32;
    let thumb_y = track_y + (track_h - thumb_h) * (1.0 - progress);
    Some(ScrollbarGeom {
        track_x,
        track_y,
        track_w,
        track_h,
        thumb_y,
        thumb_h,
        history_size,
    })
}

/// Process-wide font-name interner. `iced::Font::new` needs a
/// `&'static str`, so each unique family name is leaked exactly once
/// and the cached reference is handed back on every later call. The
/// previous approach leaked a fresh copy per view pass per pane, which
/// added up over a long session.
fn intern_font_name(name: &str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static FONT_NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut map = FONT_NAMES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(interned) = map.get(name) {
        return interned;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    map.insert(name.to_string(), leaked);
    leaked
}

/// Stable cache key for a font family: the family name, or a sentinel for
/// the generic families (the `\0` prefix can't collide with a real name).
fn font_family_key(font: Font) -> String {
    match font.family {
        iced::font::Family::Name(n) => n.to_string(),
        iced::font::Family::SansSerif => "\0sans-serif".to_string(),
        iced::font::Family::Serif => "\0serif".to_string(),
        iced::font::Family::Cursive => "\0cursive".to_string(),
        iced::font::Family::Fantasy => "\0fantasy".to_string(),
        iced::font::Family::Monospace => "\0monospace".to_string(),
    }
}

/// Measured per-glyph advance (cell width in px) for `font` at `font_size`,
/// cached per `(family, size)`.
///
/// The terminal positions every glyph at `col * cell_width`, so this value
/// must equal the font's real monospace advance, the old hard-coded
/// `font_size * 0.6` was a guess that only happened to fit the bundled
/// default; fonts with a different advance (Fira Code and friends) drew each
/// run a hair too narrow, so glyphs crept left and overlapped and the cursor
/// no longer sat behind the last character. We measure through the same
/// global cosmic-text font system the canvas renders with, so the advance we
/// cache is exactly what `fill_text` lays down. A long run of one ligature-
/// free glyph is measured and divided so `min_bounds` rounding washes out and
/// no ligature substitution can apply. Falls back to the old ratio if the
/// font can't be measured yet (font system not populated on the very first
/// frame); the next frame replaces it with the real value.
fn cell_advance(font: Font, font_size: f32) -> f32 {
    use iced::advanced::text::Paragraph as _;
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<(String, u32), f32>>> = OnceLock::new();
    let key = (font_family_key(font), font_size.to_bits());
    let mut map = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(advance) = map.get(&key) {
        return *advance;
    }
    const SAMPLES: usize = 40;
    let sample = "0".repeat(SAMPLES);
    let text = iced::advanced::text::Text {
        content: sample.as_str(),
        bounds: iced::Size::INFINITE,
        size: Pixels(font_size),
        line_height: iced::advanced::text::LineHeight::default(),
        font,
        align_x: iced::advanced::text::Alignment::Default,
        align_y: alignment::Vertical::Top,
        // Basic on purpose, and it does NOT need to mirror the canvas.
        // The canvas `Text` default is `Shaping::Auto` (we enable neither
        // `basic-shaping` nor `advanced-shaping`), and `to_shaping` maps
        // Auto to cosmic-text's Basic for ASCII and Advanced otherwise. The
        // sample below is ASCII, so Auto would resolve to Basic here anyway:
        // naming it keeps the measurement independent of that mapping.
        //
        // Do NOT copy this to the draw path. `fill_text` there must stay on
        // the Auto default: cosmic-text's Basic means "no font fallback", so
        // pinning it would tofu every glyph the terminal font lacks (CJK,
        // emoji), and it would buy nothing because ASCII runs already shape
        // as Basic.
        shaping: iced::advanced::text::Shaping::Basic,
        wrapping: iced::advanced::text::Wrapping::None,
        ellipsis: iced::advanced::text::Ellipsis::None,
        hint_factor: None,
    };
    let total = iced::advanced::graphics::text::Paragraph::with_text(text)
        .min_bounds()
        .width;
    let advance = if total > 0.0 {
        total / SAMPLES as f32
    } else {
        font_size * 0.6
    };
    map.insert(key, advance);
    advance
}

/// Pixel size of a canvas that shows exactly `cols` x `rows` cells of
/// `font_name` at `font_size`, including the widget's own padding.
/// Hosts of a fixed-grid view ([`TerminalView::with_fixed_grid`]) size
/// the canvas with this so the pinned grid and the bounds agree; the
/// metrics come from the same `cell_advance` cache the draw pass uses,
/// so the result matches the rendered glyphs exactly.
pub fn grid_pixel_size(
    font_name: &str,
    font_size: f32,
    cols: u16,
    rows: u16,
) -> (f32, f32) {
    let font = Font::new(intern_font_name(font_name));
    let cell_w = cell_advance(font, font_size);
    let cell_h = font_size * 1.15;
    (
        cols as f32 * cell_w + TERM_PAD * 2.0,
        rows as f32 * cell_h + TERM_PAD_TOP + TERM_PAD,
    )
}


/// Per-cell snapshot taken in `draw()` while the state mutex is held.
/// Pass 2 renders from these without touching the mutex, so geometry
/// building never contends with `process()` on the output path.
struct CellData {
    col: u16,
    row: u16,
    c: char,
    fg: Color,
    bg: Color,
    flags: CellFlags,
    /// Explicit underline color (SGR 58), already palette-resolved.
    /// `None` = underline in the glyph's foreground, the default.
    underline: Option<Color>,
    /// Cell carries an explicit OSC 8 hyperlink. Tinted like a detected URL so
    /// the link reads as clickable even when its label isn't URL-shaped.
    link: bool,
}

thread_local! {
    /// Reusable cell-snapshot buffer for `draw()` (which always runs on
    /// the renderer thread). Taken out for the duration of a frame and
    /// put back afterwards so its capacity survives across frames and
    /// panes instead of reallocating per draw.
    static DRAW_CELLS: std::cell::RefCell<Vec<CellData>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl<Message> canvas::Program<Message, Theme> for TerminalView<Message>
where
    Message: Clone,
{
    type State = TerminalWidgetState;

    fn update(
        &self,
        widget_state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<CanvasAction<Message>> {
        self.on_event(widget_state, event, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        self.mouse_interaction_impl(state, bounds, cursor)
    }

    fn draw(
        &self,
        widget_state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        self.render(widget_state, renderer, theme, bounds, cursor)
    }
}

#[cfg(test)]
#[path = "widget/tests.rs"]
mod mouse_report_tests;
