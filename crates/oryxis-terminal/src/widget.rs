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
mod selection;
mod state;

pub use clipboard::wrap_paste;
pub use selection::Selection;
pub use state::TerminalState;

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
pub use highlight::looks_like_ipv6;
pub(crate) use perf::*;
pub(crate) use selection::{next_click_count, union_selection, SelectGranularity};

// ---------------------------------------------------------------------------
// Canvas widget state (per-instance, managed by Iced)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct TerminalWidgetState {
    selecting: bool,
    selection: Option<Selection>,
    /// Lines scrolled back (0 = bottom). A `Cell` so the immutable-`&self`
    /// draw can reset it to the live edge on new output (PuTTY's "reset
    /// scrollback on display activity"); every other mutation is in
    /// `update` under `&mut State`, where `Cell` is equally fine.
    scroll_offset: std::cell::Cell<i32>,
    /// `render_epoch` observed by the last draw, so the next draw can
    /// tell whether new terminal activity landed (drives the
    /// reset-on-output behavior). `None` before the first draw.
    last_draw_epoch: std::cell::Cell<Option<u64>>,
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
    /// Cell extent `(visible_row, start_col, end_col)` of the OSC 8 hyperlink
    /// currently hovered, used to underline it. Regex URLs derive their extent
    /// from the per-frame highlight scan, but an explicit OSC 8 link isn't in
    /// that scan (its label need not look like a URL), so its run is captured
    /// here at hover time while the grid lock is held.
    hovered_osc8: Option<(u16, u16, u16)>,
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
    /// Hovered URL quantized to its cell, so sliding along one URL doesn't
    /// rebuild every pixel. `None` when not over a detected URL.
    hovered_url_cell: Option<(u16, u16)>,
    hovered_osc8: Option<(u16, u16, u16)>,
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
    /// Order-independent digest of the click-pinned privacy set (0 when off).
    pinned_privacy_hash: u64,
    font: Font,
    font_size: f32,
    cell_w: f32,
    cell_h: f32,
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

pub struct TerminalView<Message = ()> {
    state: Arc<Mutex<TerminalState>>,
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
    /// X11-style middle-click paste (the xterm / PuTTY tradition). Its
    /// own gesture, so it is NOT gated on `copy_on_select`; when the
    /// remote app holds mouse tracking, the report path wins (Shift
    /// bypasses, as everywhere).
    middle_click_paste: bool,
    /// What a right-click does (PuTTY's three schemes). The single
    /// authority for the gesture; see [`RightClickAction`].
    right_click_action: RightClickAction,
    /// Jump back to the live edge when the user presses a key that goes
    /// to the terminal (PuTTY's "reset scrollback on keypress").
    reset_scroll_on_keypress: bool,
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
    /// Draws the per-phase timing / fps HUD in the top-right of the pane.
    /// ORed with the `ORYXIS_TERM_PERF` env var at draw time.
    perf_overlay: bool,
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
    /// When true, cells whose foreground and background end up
    /// perceptually too close (e.g. PowerShell's `$PSStyle.FileInfo
    /// .Directory` blue-on-blue, LS_COLORS' `ow` green-on-green) get
    /// their foreground swapped for a high-contrast alternative so
    /// the text stays legible. Off paints the cell exactly as the
    /// emulator asked, which a few colour-precise tools rely on.
    smart_contrast: bool,
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
    /// When true, paint a brief translucent overlay over the whole pane this
    /// frame, the visual bell (bell mode = Flash). Driven by `Pane.bell_flash`,
    /// which a short timer clears.
    bell_flash: bool,
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
/// True for the platform's terminal clipboard chord (copy / select-all):
/// Ctrl+Shift everywhere, plus Cmd (logo) alone on macOS. Paste lives in
/// the app dispatcher (it must reach the SSH session), but copy and
/// select-all stay in the widget because it owns the selection state.
fn is_clipboard_chord(m: &keyboard::Modifiers) -> bool {
    (m.control() && m.shift())
        || (cfg!(target_os = "macos") && m.logo() && !m.control() && !m.alt())
}

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
        // Match the canvas `Text` default (Basic) so the measured advance
        // equals what `flush_run`'s `fill_text` renders.
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

impl<Message> TerminalView<Message> {
    pub fn new(state: Arc<Mutex<TerminalState>>) -> Self {
        let font_size = 14.0;
        Self {
            state,
            font_size,
            cell_width: cell_advance(Font::MONOSPACE, font_size),
            cell_height: font_size * 1.15,
            font: Font::MONOSPACE,
            copy_on_select: true,
            right_click_copy: false,
            middle_click_paste: true,
            right_click_action: RightClickAction::default(),
            reset_scroll_on_keypress: false,
            reset_scroll_on_output: false,
            bold_is_bright: true,
            keyword_highlight: true,
            performance: false,
            perf_overlay: false,
            privacy: false,
            privacy_terms: Vec::new(),
            smart_contrast: true,
            word_delimiters: crate::backend::DEFAULT_WORD_DELIMITERS.to_string(),
            on_font_size_increase: None,
            on_font_size_decrease: None,
            on_paste_request: None,
            on_context_menu: None,
            on_terminal_input: None,
            on_mouse_capture_hint: None,
            on_link_click_hint: None,
            on_link_opened: None,
            focused: true,
            bell_flash: false,
        }
    }

    /// Mark whether this pane is focused. Only the focused pane emits
    /// mouse-tracking reports (see the `focused` field).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Show the visual-bell flash overlay this frame.
    pub fn with_bell_flash(mut self, on: bool) -> Self {
        self.bell_flash = on;
        self
    }

    pub fn with_font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        // Recompute from the current font so the result is correct regardless
        // of whether `with_font_name` ran before or after this setter.
        self.cell_width = cell_advance(self.font, size);
        self.cell_height = size * 1.15;
        self
    }

    pub fn with_copy_on_select(mut self, on: bool) -> Self {
        self.copy_on_select = on;
        self
    }

    /// When on (and `copy_on_select` is also on), the selection waits for a
    /// right-click to copy instead of copying on release. No-op while
    /// `copy_on_select` is off.
    pub fn with_right_click_copy(mut self, on: bool) -> Self {
        self.right_click_copy = on;
        self
    }

    /// X11-style middle-click paste (independent of `copy_on_select`).
    pub fn with_middle_click_paste(mut self, on: bool) -> Self {
        self.middle_click_paste = on;
        self
    }

    /// Set the right-click scheme (Menu / Paste / Extend).
    pub fn with_right_click_action(mut self, action: RightClickAction) -> Self {
        self.right_click_action = action;
        self
    }

    /// PuTTY "reset scrollback on keypress": jump to the live edge when
    /// a key is sent to the terminal.
    pub fn with_reset_scroll_on_keypress(mut self, on: bool) -> Self {
        self.reset_scroll_on_keypress = on;
        self
    }

    /// PuTTY "reset scrollback on display activity": jump to the live
    /// edge on new terminal output.
    pub fn with_reset_scroll_on_output(mut self, on: bool) -> Self {
        self.reset_scroll_on_output = on;
        self
    }

    /// Wire the context-menu request (fired on right-click when the
    /// scheme is `Menu`). `f` receives window-absolute (x, y) and the
    /// live selection's text (`None` when there is no selection).
    pub fn on_context_menu(
        mut self,
        f: impl Fn(f32, f32, Option<String>) -> Message + 'static,
    ) -> Self {
        self.on_context_menu = Some(Box::new(f));
        self
    }

    pub fn with_bold_is_bright(mut self, on: bool) -> Self {
        self.bold_is_bright = on;
        self
    }

    pub fn with_smart_contrast(mut self, on: bool) -> Self {
        self.smart_contrast = on;
        self
    }

    pub fn with_privacy(mut self, on: bool) -> Self {
        self.privacy = on;
        self
    }

    /// Extra strings Privacy Mode must mask wherever they appear, on top
    /// of the shape-based IP / `user@host` / home-dir detection. The app
    /// passes the vault's saved hostnames so plain DNS names are hidden
    /// too. Stored lowercase (matching is case-insensitive and
    /// token-bounded); very short terms are dropped, masking every "web"
    /// or "db1" in sight would be noise, not privacy. No-op while
    /// privacy is off.
    pub fn with_privacy_terms(mut self, terms: &[String]) -> Self {
        self.privacy_terms = terms
            .iter()
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| t.len() >= 4)
            .collect();
        self
    }

    pub fn with_keyword_highlight(mut self, on: bool) -> Self {
        self.keyword_highlight = on;
        self
    }

    /// Performance mode. See [`TerminalView::performance`].
    pub fn with_performance(mut self, on: bool) -> Self {
        self.performance = on;
        self
    }

    /// Show the per-pane perf HUD (also forced by `ORYXIS_TERM_PERF`).
    pub fn with_perf_overlay(mut self, on: bool) -> Self {
        self.perf_overlay = on;
        self
    }

    /// Set the word-delimiter set used for double-click word selection.
    /// Empty means no character terminates a word (double-click then
    /// grabs the whole logical line, like triple-click).
    pub fn with_word_delimiters(mut self, delimiters: &str) -> Self {
        self.word_delimiters = delimiters.to_string();
        self
    }

    /// Wire a message that fires when the user does Ctrl+Wheel-up over
    /// the terminal canvas.
    pub fn on_font_size_increase(mut self, msg: Message) -> Self {
        self.on_font_size_increase = Some(msg);
        self
    }

    /// Wire a message that fires when the user does Ctrl+Wheel-down over
    /// the terminal canvas.
    pub fn on_font_size_decrease(mut self, msg: Message) -> Self {
        self.on_font_size_decrease = Some(msg);
        self
    }

    /// Wire a message that fires on right-click over the terminal. The
    /// app dispatcher should read the clipboard and write the text to
    /// the active SSH session (or local PTY as fallback), the same path
    /// Ctrl+Shift+V takes. Without this hook, the widget falls back to
    /// writing the clipboard text directly to the local PTY, which only
    /// works for local-shell tabs.
    /// Wire the "that link needs Ctrl + Click" hint. The callback fires
    /// when a plain click (no Ctrl, no drag) lands on a URL, so the app
    /// can show a transient toast teaching the gesture at the moment it
    /// missed (one-time onboarding, see `on_link_opened`).
    pub fn on_link_click_hint(mut self, f: impl Fn() -> Message + 'static) -> Self {
        self.on_link_click_hint = Some(Box::new(f));
        self
    }

    /// Message emitted after a Ctrl+Click opens a URL.
    pub fn on_link_opened(mut self, msg: Message) -> Self {
        self.on_link_opened = Some(msg);
        self
    }

    pub fn on_paste_request(mut self, msg: Message) -> Self {
        self.on_paste_request = Some(msg);
        self
    }

    /// Wire a callback for synthesized input bytes (mouse-tracking
    /// reports and wheel-to-arrow translation). The dispatcher should
    /// route the bytes to the active SSH session, falling back to the
    /// local PTY, exactly like the keyboard / paste paths. Without this
    /// hook the widget writes to the local PTY directly, which is a
    /// no-op on SSH tabs (their `TerminalState` has no PTY).
    pub fn on_terminal_input(
        mut self,
        f: impl Fn(Vec<u8>) -> Message + 'static,
    ) -> Self {
        self.on_terminal_input = Some(Box::new(f));
        self
    }

    /// Wire the "mouse tracking is swallowing your selection" hint. The
    /// callback fires once per pane, on the first left-drag while the
    /// remote app holds the mouse, so the app can show a transient
    /// "hold Shift to select" toast at the moment it's relevant.
    pub fn on_mouse_capture_hint(mut self, f: impl Fn() -> Message + 'static) -> Self {
        self.on_mouse_capture_hint = Some(Box::new(f));
        self
    }

    /// Override the font used for cell rendering. If the font can't be resolved
    /// by cosmic-text, it falls back to the system default monospace.
    pub fn with_font_name(mut self, name: &str) -> Self {
        self.font = Font::new(intern_font_name(name));
        // The cell width depends on the font's advance; recompute it now that
        // the family changed (the width comes from the real metric, not a
        // fixed ratio, so a different font means a different cell width).
        self.cell_width = cell_advance(self.font, self.font_size);
        self
    }

    /// Grid dimensions (cols, rows) that fit the given canvas size at this
    /// view's measured cell metrics. Uses the real per-font cell width so the
    /// column count matches the glyphs actually drawn (a font wider than the
    /// old `0.6` ratio would otherwise be told it has more columns than fit,
    /// and wrap early).
    fn grid_size(&self, width: f32, height: f32) -> (u16, u16) {
        let cell_width = self.cell_width.max(1.0);
        let cell_height = self.cell_height.max(1.0);
        let usable_w = (width - TERM_PAD * 2.0).max(cell_width);
        let usable_h = (height - TERM_PAD_TOP - TERM_PAD).max(cell_height);
        let cols = (usable_w / cell_width).floor().max(1.0) as u16;
        let rows = (usable_h / cell_height).floor().max(1.0) as u16;
        (cols, rows)
    }

    fn pixel_to_cell(&self, pos: Point) -> (u16, u16) {
        let col = ((pos.x - TERM_PAD) / self.cell_width).floor().max(0.0) as u16;
        let row = ((pos.y - TERM_PAD_TOP) / self.cell_height).floor().max(0.0) as u16;
        (col, row)
    }

    /// Convert a visible-row index to the alacritty grid-line index, given
    /// the current scroll offset. Visible row 0 is the top of the canvas.
    fn visible_row_to_line(visible_row: u16, scroll_offset: i32) -> i32 {
        visible_row as i32 - scroll_offset
    }

    /// Compute a word- or line-granularity selection around `cell` using
    /// alacritty's native semantic / line search. `cell` is `(col, line)`
    /// in grid-line coordinates (negative line = scrollback). The current
    /// delimiter set is synced into the backend first (a cheap no-op when
    /// unchanged).
    fn semantic_selection(
        &self,
        backend: &mut TerminalBackend,
        cell: (u16, i32),
        gran: SelectGranularity,
    ) -> Selection {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point as TermPoint};
        backend.set_word_delimiters(&self.word_delimiters);
        let term = &backend.term;
        let grid = term.grid();
        // Clamp into the grid before building the point: the semantic /
        // line search routines index `grid[point]` up front and only
        // clamp the lower line bound, so an edge click (col >= cols or a
        // line past the last row, neither of which `pixel_to_cell`
        // clamps high) would panic.
        let line = cell.1.clamp(grid.topmost_line().0, grid.bottommost_line().0);
        let col = (cell.0 as usize).min(grid.columns().saturating_sub(1));
        let point = TermPoint::new(Line(line), Column(col));
        let (l, r) = match gran {
            SelectGranularity::Word => {
                (term.semantic_search_left(point), term.semantic_search_right(point))
            }
            SelectGranularity::Line => {
                (term.line_search_left(point), term.line_search_right(point))
            }
            SelectGranularity::Paragraph => {
                // Expand to the run of non-blank lines around the click,
                // bounded by blank rows (all spaces / NUL). Full width.
                let last_col = grid.columns().saturating_sub(1) as u16;
                let top_lim = grid.topmost_line().0;
                let bot_lim = grid.bottommost_line().0;
                let is_blank = |li: i32| {
                    let r = &grid[Line(li)];
                    (0..grid.columns()).all(|c| matches!(r[Column(c)].c, ' ' | '\0'))
                };
                let mut top = line;
                while top > top_lim && !is_blank(top - 1) {
                    top -= 1;
                }
                let mut bot = line;
                while bot < bot_lim && !is_blank(bot + 1) {
                    bot += 1;
                }
                return Selection {
                    start: (0, top),
                    end: (last_col, bot),
                    block: false,
                };
            }
        };
        Selection {
            start: (l.column.0 as u16, l.line.0),
            end: (r.column.0 as u16, r.line.0),
            block: false,
        }
    }

    /// Map an iced mouse button to its mouse-report button, or `None`
    /// for buttons the xterm protocol doesn't encode (Back / Forward /
    /// Other).
    fn iced_to_report_button(btn: mouse::Button) -> Option<ReportButton> {
        match btn {
            mouse::Button::Left => Some(ReportButton::Left),
            mouse::Button::Middle => Some(ReportButton::Middle),
            mouse::Button::Right => Some(ReportButton::Right),
            _ => None,
        }
    }

    /// Send synthesized input bytes (mouse reports, wheel-to-arrow) to the
    /// dispatcher so they reach the active SSH session. Falls back to a
    /// direct local-PTY write when no callback is wired (local-shell
    /// tabs). Always captures the originating event.
    fn emit_input(&self, bytes: Vec<u8>) -> CanvasAction<Message> {
        if let Some(cb) = &self.on_terminal_input {
            CanvasAction::publish(cb(bytes)).and_capture()
        } else {
            if let Ok(mut state) = self.state.lock() {
                state.write(&bytes);
            }
            CanvasAction::capture()
        }
    }

    /// Translate a pointer event into a mouse-tracking report for the
    /// remote app. Returns `Some(action)` when the event was consumed,
    /// `None` to let the normal local handlers run. The caller guarantees
    /// the app has mouse tracking on and Shift isn't held.
    #[allow(clippy::too_many_arguments)]
    fn handle_mouse_report(
        &self,
        widget_state: &mut TerminalWidgetState,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        mode: alacritty_terminal::term::TermMode,
        grid_cols: u16,
        grid_rows: u16,
    ) -> Option<CanvasAction<Message>> {
        use alacritty_terminal::term::TermMode;
        let kbd = widget_state.modifiers;
        let ctrl = kbd.control();
        // Shift is the local-selection bypass, so the caller only reaches
        // here with it released; never fold it into the report.
        let mods = ReportMods { shift: false, alt: kbd.alt(), ctrl };

        // Resolve a pixel position to a clamped, zero-based cell.
        let cell = |pos: Point| -> (u16, u16) {
            let (c, r) = self.pixel_to_cell(pos);
            (
                c.min(grid_cols.saturating_sub(1)),
                r.min(grid_rows.saturating_sub(1)),
            )
        };

        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(btn)) => {
                let pos = cursor.position_in(bounds)?;
                let rb = Self::iced_to_report_button(*btn)?;
                let (col, row) = cell(pos);
                widget_state.report_button = Some(rb);
                widget_state.report_cell = Some((col, row));
                // New drag: re-arm the per-drag hint guard so Always mode
                // can fire once for this gesture too.
                widget_state.mouse_hint_emitted = false;
                let bytes =
                    mouse_report::encode(mode, MouseEventKind::Press, rb, col, row, mods)?;
                Some(self.emit_input(bytes))
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(btn)) => {
                let rb = Self::iced_to_report_button(*btn)?;
                // Only report the release of a press WE reported. A release
                // whose press never reached the app (it landed on a sibling
                // widget) must stay local: this arm captures what it
                // consumes, and sibling `button`s fire on release, so
                // reporting unconditionally made every sidebar click dead
                // while a full-screen app (mc, htop) held mouse tracking,
                // forcing the Shift bypass for plain UI clicks.
                if widget_state.report_button != Some(rb) {
                    return None;
                }
                // A drag can end with the pointer off the canvas; fall back
                // to the last reported cell so the release still lands.
                let (col, row) = match cursor.position_in(bounds) {
                    Some(pos) => cell(pos),
                    None => widget_state.report_cell.unwrap_or((0, 0)),
                };
                widget_state.report_button = None;
                let bytes =
                    mouse_report::encode(mode, MouseEventKind::Release, rb, col, row, mods)?;
                Some(self.emit_input(bytes))
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let pos = cursor.position_in(bounds)?;
                let (col, row) = cell(pos);
                // Suppress repeats while the cursor stays inside one cell.
                if widget_state.report_cell == Some((col, row)) {
                    return None;
                }
                // Drag tracking (1002) reports motion only while a button is
                // held; any-motion tracking (1003) reports bare motion via
                // the "no button" sentinel.
                let btn = match widget_state.report_button {
                    Some(b) => b,
                    None if mode.contains(TermMode::MOUSE_MOTION) => ReportButton::None,
                    None => return None,
                };
                // A left-button drag while the app holds the mouse is the
                // user trying to select text that mouse tracking is
                // swallowing. Surface the Shift bypass once per pane, on
                // the first such drag. Dropping this single motion report
                // (we return before encoding) is harmless: the next move
                // reports the new cell.
                if !widget_state.mouse_hint_emitted
                    && widget_state.report_button == Some(ReportButton::Left)
                    && let Some(cb) = &self.on_mouse_capture_hint
                {
                    widget_state.mouse_hint_emitted = true;
                    widget_state.report_cell = Some((col, row));
                    return Some(CanvasAction::publish(cb()).and_capture());
                }
                let bytes =
                    mouse_report::encode(mode, MouseEventKind::Motion, btn, col, row, mods)?;
                widget_state.report_cell = Some((col, row));
                Some(self.emit_input(bytes))
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                // Ctrl+wheel stays a local font-zoom affordance; let it
                // reach the dedicated handler instead of reporting it.
                if ctrl {
                    return None;
                }
                let pos = cursor.position_in(bounds)?;
                let (col, row) = cell(pos);
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / self.cell_height,
                };
                if dy == 0.0 {
                    return None;
                }
                let btn = if dy > 0.0 {
                    ReportButton::WheelUp
                } else {
                    ReportButton::WheelDown
                };
                // One report per notch, capped so a fast flick can't flood
                // the session, concatenated into a single write.
                let notches = (dy.abs().ceil() as u32).clamp(1, 5);
                let mut bytes = Vec::new();
                for _ in 0..notches {
                    if let Some(seq) =
                        mouse_report::encode(mode, MouseEventKind::Press, btn, col, row, mods)
                    {
                        bytes.extend_from_slice(&seq);
                    }
                }
                if bytes.is_empty() {
                    return None;
                }
                Some(self.emit_input(bytes))
            }
            _ => None,
        }
    }

    fn is_in_selection(sel: &Selection, col: u16, line: i32) -> bool {
        if sel.block {
            let (c0, c1, l0, l1) = sel.block_bounds();
            return line >= l0 && line <= l1 && col >= c0 && col <= c1;
        }
        let (start, end) = sel.ordered();
        if start.1 == end.1 {
            line == start.1 && col >= start.0 && col <= end.0
        } else if line == start.1 {
            col >= start.0
        } else if line == end.1 {
            col <= end.0
        } else {
            line > start.1 && line < end.1
        }
    }
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
        // Refresh hover state for every event we see, drives the
        // scrollbar's reveal-on-hover behaviour. Done before the match so
        // we don't have to repeat it in every arm.
        let new_hover = cursor.position_in(bounds).is_some();
        let hover_changed = widget_state.hover != new_hover;
        widget_state.hover = new_hover;

        // When the remote app has mouse tracking on (tmux `mouse on`,
        // vim `mouse=a`, htop, ...) pointer events are reported to it
        // instead of driving local selection / scrollback. We snapshot
        // the relevant `TermMode` + grid size once per mouse event (the
        // lock is a cheap flag read; skipped for keyboard events so the
        // typing path never contends on it). Holding Shift bypasses
        // reporting and restores local selection, the universal escape
        // hatch every terminal honours.
        // Only the focused pane reports mouse events to its app. Otherwise
        // a click that just focuses an inactive split pane (one still in
        // mouse mode, e.g. running htop) would inject a stray SGR report
        // like `\x1b[<0;1;1m` into that shell.
        let report_ctx = if self.focused && matches!(event, iced::Event::Mouse(_)) {
            self.state.lock().ok().and_then(|s| {
                let mode = *s.backend.term.mode();
                mode.intersects(alacritty_terminal::term::TermMode::MOUSE_MODE)
                    .then(|| (mode, s.cols(), s.rows()))
            })
        } else {
            None
        };
        if let Some((mode, grid_cols, grid_rows)) = report_ctx
            && !widget_state.modifiers.shift()
            && let Some(action) =
                self.handle_mouse_report(widget_state, event, bounds, cursor, mode, grid_cols, grid_rows)
        {
            return Some(action);
        }

        match event {
            // Mouse press, scrollbar interaction takes priority, then
            // URL open, then text selection.
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(pos) = cursor.position_in(bounds) {
                    // Scrollbar: thumb drag start, or page-up/down on the
                    // empty track area. Only meaningful when there's
                    // actual scrollback.
                    if let Ok(state) = self.state.lock() {
                        let grid = state.backend.term.grid();
                        if let Some(sb) = scrollbar_geom(
                            bounds,
                            grid.total_lines(),
                            grid.screen_lines(),
                            widget_state.scroll_offset.get(),
                        ) && pos.x >= sb.track_x - 2.0
                            && pos.x <= sb.track_x + sb.track_w + 2.0
                            && pos.y >= sb.track_y
                            && pos.y <= sb.track_y + sb.track_h
                        {
                            let page = grid.screen_lines() as i32;
                            if pos.y >= sb.thumb_y && pos.y <= sb.thumb_y + sb.thumb_h {
                                widget_state.scrollbar_drag =
                                    Some((pos.y, widget_state.scroll_offset.get()));
                            } else if pos.y < sb.thumb_y {
                                widget_state.scroll_offset
                                    .set((widget_state.scroll_offset.get() + page).min(sb.history_size));
                            } else {
                                widget_state.scroll_offset
                                    .set((widget_state.scroll_offset.get() - page).max(0));
                            }
                            return Some(CanvasAction::request_redraw().and_capture());
                        }
                    }
                    let (col, vrow) = self.pixel_to_cell(pos);
                    let line = Self::visible_row_to_line(vrow, widget_state.scroll_offset.get());
                    // Only follow URLs on Ctrl+Click, plain clicks
                    // start a selection, matching Termius. Without
                    // the modifier gate, every click on a logged URL
                    // would lose the selection start.
                    if widget_state.modifiers.control()
                        && let Ok(state) = self.state.lock()
                        // An explicit OSC 8 hyperlink wins over a scraped URL,
                        // its target URI can differ from the visible label.
                        && let Some(url) = osc8_link_at_cell(&state.backend.term, line, col)
                            .map(|(uri, _, _)| uri)
                            .or_else(|| url_at_cell(&state.backend.term, line, col))
                    {
                        drop(state);
                        open_url(&url);
                        // Tell the app the gesture landed so the
                        // one-time hover hint can retire itself.
                        if let Some(msg) = self.on_link_opened.clone() {
                            return Some(CanvasAction::publish(msg).and_capture());
                        }
                        return Some(CanvasAction::capture());
                    }
                    // Shift+Click extends the current selection from its
                    // existing anchor instead of starting a new one (xterm
                    // behaviour). Handled before click-kind classification so
                    // a quick shift+click can't be misread as a double-click
                    // word grab. Block-ness carries over.
                    if widget_state.modifiers.shift()
                        && let Some(prev) = widget_state.selection
                    {
                        widget_state.select_anchor = None;
                        widget_state.selecting = true;
                        widget_state.last_extend_cell = Some((col, line));
                        widget_state.selection = Some(Selection {
                            start: prev.start,
                            end: (col, line),
                            block: prev.block,
                        });
                        return Some(CanvasAction::request_redraw().and_capture());
                    }
                    // Classify the press as single / double / triple / quad
                    // (300 ms / 6 px window). 1=cell (Alt=block), 2=word
                    // (smart-select on URL/IP/path), 3=line, 4=paragraph.
                    let now = std::time::Instant::now();
                    let consecutive = widget_state
                        .last_click
                        .map(|(t, p, _)| {
                            now.duration_since(t) <= std::time::Duration::from_millis(300)
                                && p.distance(pos) < 6.0
                        })
                        .unwrap_or(false);
                    let count = next_click_count(
                        widget_state.last_click.map(|(_, _, c)| c),
                        consecutive,
                    );
                    widget_state.last_click = Some((now, pos, count));
                    widget_state.selecting = true;
                    widget_state.last_extend_cell = Some((col, line));
                    match count {
                        1 => {
                            widget_state.select_anchor = None;
                            // Alt+drag starts a rectangular (column) selection.
                            widget_state.selection = Some(Selection {
                                start: (col, line),
                                end: (col, line),
                                block: widget_state.modifiers.alt(),
                            });
                        }
                        2 => {
                            if let Ok(mut state) = self.state.lock() {
                                // Smart-select: a double-click inside a URL /
                                // IP / path grabs the whole token instead of
                                // the delimiter word. Falls back to word.
                                if let Some((c0, c1)) = smart_span_at(
                                    &state.backend.term,
                                    &state.palette,
                                    line,
                                    col,
                                ) {
                                    widget_state.select_anchor = None;
                                    widget_state.selection = Some(Selection {
                                        start: (c0, line),
                                        end: (c1, line),
                                        block: false,
                                    });
                                } else {
                                    widget_state.select_anchor =
                                        Some((SelectGranularity::Word, (col, line)));
                                    widget_state.selection = Some(self.semantic_selection(
                                        &mut state.backend,
                                        (col, line),
                                        SelectGranularity::Word,
                                    ));
                                }
                            }
                        }
                        3 => {
                            widget_state.select_anchor =
                                Some((SelectGranularity::Line, (col, line)));
                            if let Ok(mut state) = self.state.lock() {
                                widget_state.selection = Some(self.semantic_selection(
                                    &mut state.backend,
                                    (col, line),
                                    SelectGranularity::Line,
                                ));
                            }
                        }
                        // 4 (and the cycle restarts after): paragraph.
                        _ => {
                            widget_state.select_anchor =
                                Some((SelectGranularity::Paragraph, (col, line)));
                            if let Ok(mut state) = self.state.lock() {
                                widget_state.selection = Some(self.semantic_selection(
                                    &mut state.backend,
                                    (col, line),
                                    SelectGranularity::Paragraph,
                                ));
                            }
                        }
                    }
                    return Some(CanvasAction::request_redraw().and_capture());
                }
            }
            // Mouse move, drag scrollbar thumb or extend selection.
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some((start_y, start_offset)) = widget_state.scrollbar_drag
                    && let Some(pos) = cursor.position_in(bounds)
                    && let Ok(state) = self.state.lock()
                {
                    let grid = state.backend.term.grid();
                    if let Some(sb) = scrollbar_geom(
                        bounds,
                        grid.total_lines(),
                        grid.screen_lines(),
                        start_offset,
                    ) {
                        let dy = pos.y - start_y;
                        let track_range = (sb.track_h - sb.thumb_h).max(1.0);
                        let dprogress = dy / track_range;
                        let doffset = (dprogress * sb.history_size as f32) as i32;
                        // Thumb moves down → progress decreases → offset decreases.
                        widget_state.scroll_offset
                            .set((start_offset - doffset).clamp(0, sb.history_size));
                        return Some(CanvasAction::request_redraw().and_capture());
                    }
                }
                if widget_state.selecting
                    && let Some(abs) = cursor.position() {
                        // Use the absolute cursor position (not
                        // `position_in`, which is `None` outside the widget)
                        // so a drag that leaves the widget but stays in the
                        // window still extends + auto-scrolls, matching other
                        // terminals. Once the pointer leaves the window the OS
                        // stops sending events, which we can't work around
                        // without a pointer grab iced doesn't expose.
                        let rel = Point::new(abs.x - bounds.x, abs.y - bounds.y);
                        // Auto-scroll when the drag passes the top/bottom
                        // edge so the selection extends into scrollback. The
                        // step grows with how far past the edge the cursor is
                        // (deliberately aggressive: 2 lines per overshoot
                        // cell). Events only fire on motion, so this follows
                        // the mouse rather than ticking while held still.
                        let top_edge = TERM_PAD_TOP;
                        let bot_edge = (bounds.height - TERM_PAD).max(top_edge);
                        // Rate-limit to one step per ~40 ms so the scroll
                        // speed tracks wall-clock instead of the mouse-move
                        // event rate (dozens per second at the edge), which
                        // is what made it feel like it rocketed.
                        let now = std::time::Instant::now();
                        let due = widget_state
                            .last_autoscroll
                            .map(|t| {
                                now.duration_since(t)
                                    >= std::time::Duration::from_millis(40)
                            })
                            .unwrap_or(true);
                        if (rel.y < top_edge || rel.y > bot_edge)
                            && due
                            && let Ok(state) = self.state.lock()
                        {
                            use alacritty_terminal::grid::Dimensions;
                            let grid = state.backend.term.grid();
                            let history = (grid
                                .total_lines()
                                .saturating_sub(grid.screen_lines()))
                                as i32;
                            let past = if rel.y < top_edge {
                                top_edge - rel.y
                            } else {
                                rel.y - bot_edge
                            };
                            // 1 line per tick at the edge, +1 per cell of
                            // overshoot, capped so a far pointer stays sane.
                            let step =
                                ((past / self.cell_height).floor() as i32 + 1).clamp(1, 4);
                            widget_state.last_autoscroll = Some(now);
                            if rel.y < top_edge {
                                widget_state.scroll_offset
                                    .set((widget_state.scroll_offset.get() + step).min(history));
                            } else {
                                widget_state.scroll_offset
                                    .set((widget_state.scroll_offset.get() - step).max(0));
                            }
                        }
                        // Clamp back into the widget for cell mapping (the
                        // pointer may be outside the bounds now).
                        let clamped = Point::new(
                            rel.x.clamp(0.0, bounds.width),
                            rel.y.clamp(0.0, bounds.height),
                        );
                        let (col, vrow) = self.pixel_to_cell(clamped);
                        let line = Self::visible_row_to_line(vrow, widget_state.scroll_offset.get());
                        if let Some((gran, anchor)) = widget_state.select_anchor {
                            // Word/line drag: extend by unioning the anchor's
                            // word/line with the cursor's. Throttle to one
                            // recompute per cell crossing, it locks the mutex
                            // and runs two semantic searches, which must not
                            // happen per pixel (same reasoning as the URL
                            // hover throttle below).
                            if widget_state.last_extend_cell != Some((col, line)) {
                                widget_state.last_extend_cell = Some((col, line));
                                if let Ok(mut state) = self.state.lock() {
                                    let head = self.semantic_selection(
                                        &mut state.backend, anchor, gran,
                                    );
                                    let tail = self.semantic_selection(
                                        &mut state.backend, (col, line), gran,
                                    );
                                    drop(state);
                                    widget_state.selection =
                                        Some(union_selection(head, tail));
                                }
                            }
                        } else if let Some(ref mut sel) = widget_state.selection {
                            sel.end = (col, line);
                        }
                        return Some(CanvasAction::request_redraw().and_capture());
                    }
                // URL hover detection. Skip the lock + grid scan when
                // the cursor is still over the same cell, at typical
                // font sizes a single cell spans many pixels and
                // running the scan on every pixel contended with
                // `state.process` (the SSH echo path), showing up as
                // typing lag.
                let cell_changed;
                let new_hover_url = if let Some(pos) = cursor.position_in(bounds) {
                    let (col, vrow) = self.pixel_to_cell(pos);
                    let same_cell = widget_state.hovered_cell == Some((col, vrow));
                    cell_changed = !same_cell;
                    widget_state.hovered_cell = Some((col, vrow));
                    if same_cell {
                        widget_state
                            .hovered_url
                            .as_ref()
                            .map(|(u, _)| (u.clone(), pos))
                    } else if let Ok(state) = self.state.lock() {
                        let line = Self::visible_row_to_line(vrow, widget_state.scroll_offset.get());
                        // Explicit OSC 8 link first (label may not look like a
                        // URL); capture its run for the underline. Else fall
                        // back to a scraped URL.
                        if let Some((uri, sc, ec)) =
                            osc8_link_at_cell(&state.backend.term, line, col)
                        {
                            widget_state.hovered_osc8 = Some((vrow, sc, ec));
                            Some((uri, pos))
                        } else {
                            widget_state.hovered_osc8 = None;
                            url_at_cell(&state.backend.term, line, col).map(|u| (u, pos))
                        }
                    } else {
                        None
                    }
                } else {
                    // Cursor left the canvas: a revealed privacy span must
                    // re-mask, so flag a cell change when one was tracked.
                    cell_changed = widget_state.hovered_cell.is_some();
                    widget_state.hovered_cell = None;
                    widget_state.hovered_osc8 = None;
                    None
                };
                let url_changed = match (&widget_state.hovered_url, &new_hover_url) {
                    (Some((a, _)), Some((b, _))) => a != b,
                    (None, None) => false,
                    _ => true,
                };
                widget_state.hovered_url = new_hover_url;
                // Under Privacy Mode a cell change can move the revealed
                // span even when no URL is involved, so repaint on any cell
                // change too (otherwise hovering an IP wouldn't reveal it).
                if hover_changed || url_changed || (self.privacy && cell_changed) {
                    return Some(CanvasAction::request_redraw());
                }
            }
            // Mouse release, end selection or scrollbar drag.
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was_dragging = widget_state.scrollbar_drag.is_some();
                widget_state.scrollbar_drag = None;
                let was_selecting = widget_state.selecting;
                // A double/triple-click selection is intentional even when
                // it lands on a single cell (a one-character word), so it
                // must still auto-copy despite `is_empty()`.
                let was_semantic = widget_state.select_anchor.is_some();
                widget_state.selecting = false;
                widget_state.select_anchor = None;
                widget_state.last_extend_cell = None;
                // Auto-copy the just-finished selection when the setting is
                // enabled (XTerm / iTerm behaviour). Skip degenerate
                // selections that didn't move (single click). When
                // `right_click_copy` is on the copy is deferred to a
                // right-click instead, so skip the auto-copy here.
                if was_selecting
                    && self.copy_on_select
                    && !self.right_click_copy
                    && let Some(ref sel) = widget_state.selection
                    && (!sel.is_empty() || was_semantic)
                    && let Ok(state) = self.state.lock()
                {
                    let text = state.get_selection_text(sel);
                    drop(state);
                    if !text.is_empty() {
                        set_clipboard_text(&text);
                    }
                }
                if was_dragging {
                    return Some(CanvasAction::request_redraw().and_capture());
                }
                // Plain click (no drag, no word/line select) on a masked
                // privacy span toggles a pinned reveal for its value: the
                // mask is undone for every occurrence of that value until
                // it's clicked again. Keyed by the span text, not its
                // cells, so the reveal survives scrolling and re-prints.
                if self.privacy
                    && was_selecting
                    && !was_semantic
                    && widget_state.selection.as_ref().is_some_and(|s| s.is_empty())
                    && let Some(pos) = cursor.position_in(bounds)
                {
                    let (col, vrow) = self.pixel_to_cell(pos);
                    let line = Self::visible_row_to_line(vrow, widget_state.scroll_offset.get());
                    let value = self.state.lock().ok().and_then(|state| {
                        privacy_value_at_cell(
                            &state.backend.term,
                            &state.palette,
                            &self.privacy_terms,
                            line,
                            col,
                        )
                    });
                    if let Some(value) = value {
                        if !widget_state.pinned_privacy.remove(&value) {
                            widget_state.pinned_privacy.insert(value);
                        }
                        return Some(CanvasAction::request_redraw().and_capture());
                    }
                }
                // Plain click (no Ctrl, no drag, no word/line select) on a
                // URL: the user likely expected the link to open, but plain
                // clicks select (Termius-style, see the press handler). Let
                // the app surface the "hold Ctrl and click" toast at the
                // exact moment the gesture missed. Ctrl+Click never reaches
                // here as a click: the press handler opens the URL without
                // starting a selection, so `was_selecting` is false.
                if !widget_state.modifiers.control()
                    && was_selecting
                    && !was_semantic
                    && widget_state.selection.as_ref().is_some_and(|s| s.is_empty())
                    && let Some(cb) = &self.on_link_click_hint
                    && let Some(pos) = cursor.position_in(bounds)
                {
                    let (col, vrow) = self.pixel_to_cell(pos);
                    let line = Self::visible_row_to_line(vrow, widget_state.scroll_offset.get());
                    let on_url = self.state.lock().is_ok_and(|state| {
                        osc8_link_at_cell(&state.backend.term, line, col).is_some()
                            || url_at_cell(&state.backend.term, line, col).is_some()
                    });
                    if on_url {
                        return Some(CanvasAction::publish(cb()).and_capture());
                    }
                }
                // Only swallow the release when it belongs to this terminal:
                // a finishing selection, or a release physically over the
                // canvas. A stray release that lands on a sibling widget
                // (e.g. a button in the terminal sidebar) must pass through,
                // otherwise that widget never sees its release and its
                // `on_press` never fires (iced buttons act on release).
                if was_selecting || was_semantic || cursor.position_in(bounds).is_some() {
                    return Some(CanvasAction::capture());
                }
                return None;
            }
            // Right-click, paste from clipboard. When the host wired an
            // X11-style middle-click paste (xterm / PuTTY tradition). Its
            // own gesture, so it isn't gated on `copy_on_select`; when
            // the remote app holds mouse tracking, the report path above
            // already consumed the press (Shift bypasses, as everywhere).
            // Same delegation as right-click below: `on_paste_request`
            // routes through the dispatcher (paste guard + SSH routing),
            // with a local-PTY fallback for callers without the hook.
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle))
                if cursor.position_in(bounds).is_some() && self.middle_click_paste =>
            {
                if let Some(msg) = self.on_paste_request.clone() {
                    return Some(CanvasAction::publish(msg).and_capture());
                }
                if let Ok(mut clip) = arboard::Clipboard::new()
                    && let Ok(text) = clip.get_text()
                    && let Ok(mut state) = self.state.lock()
                {
                    let bracketed = state.bracketed_paste_enabled();
                    state.write(&crate::wrap_paste(&text, bracketed));
                }
                return Some(CanvasAction::capture());
            }
            // `on_paste_request` callback we delegate the actual paste to
            // the app dispatcher so it can target the SSH session (the
            // local-PTY write below only reaches local-shell tabs). The
            // fallback covers callers that don't set the hook. Gated on
            // `copy_on_select`: that setting bundles "select to copy & right
            // click to paste", so right-click does nothing when it's off.
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
                if cursor.position_in(bounds).is_some() =>
            {
                // The right-click scheme (PuTTY's Menu / Paste / Extend) is
                // the single authority for this gesture. Unlike the old
                // path it is NOT gated on `copy_on_select`: an explicit
                // "Paste" scheme that silently did nothing with copy-on-
                // select off would be a surprise.
                match self.right_click_action {
                    RightClickAction::Menu => {
                        if let Some(cb) = &self.on_context_menu {
                            // Window-absolute position for the app's overlay
                            // (same coordinate space as every other menu
                            // anchor). `position()` is the viewport point.
                            let abs = cursor.position().unwrap_or_default();
                            // Capture the live selection's text now, so the
                            // app-rendered "Copy" row can offer it (the
                            // selection state is unreachable from the app).
                            let sel_text = widget_state
                                .selection
                                .as_ref()
                                .filter(|s| !s.is_empty())
                                .and_then(|sel| {
                                    self.state.lock().ok().and_then(|state| {
                                        let t = state.get_selection_text(sel);
                                        (!t.is_empty()).then_some(t)
                                    })
                                });
                            return Some(
                                CanvasAction::publish(cb(abs.x, abs.y, sel_text)).and_capture(),
                            );
                        }
                        return Some(CanvasAction::capture());
                    }
                    RightClickAction::Extend => {
                        // xterm extend: move the selection's NEARER boundary
                        // to the click point, keeping the far anchor fixed,
                        // then copy. A no-op when there is nothing to extend
                        // (or when the live selection is a block).
                        if let Some(pos) = cursor.position_in(bounds) {
                            let (col, vrow) = self.pixel_to_cell(pos);
                            let line =
                                Self::visible_row_to_line(vrow, widget_state.scroll_offset.get());
                            if let Some(sel) = widget_state.selection.as_ref().filter(|s| !s.block)
                            {
                                let extended = sel.extended_to((col, line));
                                widget_state.selection = Some(extended);
                                if let Ok(state) = self.state.lock() {
                                    let text = state.get_selection_text(&extended);
                                    drop(state);
                                    if !text.is_empty() {
                                        set_clipboard_text(&text);
                                    }
                                }
                                return Some(CanvasAction::request_redraw().and_capture());
                            }
                        }
                        return Some(CanvasAction::capture());
                    }
                    RightClickAction::Paste => {
                        // copy_on_select + right_click_copy: a right-click
                        // over a live selection copies it instead of pasting,
                        // then clears the selection so the next right-click
                        // pastes. The copy is written straight to the
                        // clipboard here (mirroring Ctrl+Shift+C), not via
                        // `on_paste_request` (the paste hook).
                        if self.copy_on_select
                            && self.right_click_copy
                            && let Some(sel) = widget_state.selection
                            && !sel.is_empty()
                        {
                            if let Ok(state) = self.state.lock() {
                                let text = state.get_selection_text(&sel);
                                drop(state);
                                if !text.is_empty() {
                                    set_clipboard_text(&text);
                                }
                            }
                            widget_state.selection = None;
                            return Some(CanvasAction::request_redraw().and_capture());
                        }
                        if let Some(msg) = self.on_paste_request.clone() {
                            return Some(CanvasAction::publish(msg).and_capture());
                        }
                        if let Ok(mut clip) = arboard::Clipboard::new()
                            && let Ok(text) = clip.get_text()
                            && let Ok(mut state) = self.state.lock()
                        {
                            let bracketed = state.bracketed_paste_enabled();
                            state.write(&crate::wrap_paste(&text, bracketed));
                        }
                        return Some(CanvasAction::capture());
                    }
                }
            }
            // Ctrl + wheel, adjust terminal font size in the standard
            // alacritty / kitty / gnome-terminal way. Captured before the
            // scrollback handler so it doesn't double-up with paging.
            // The TUI inside the session never sees the wheel event in
            // this branch, so htop / less / vim mouse modes aren't
            // disturbed.
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.position_in(bounds).is_some()
                    && widget_state.modifiers.control() =>
            {
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                if dy > 0.0
                    && let Some(msg) = self.on_font_size_increase.clone()
                {
                    return Some(CanvasAction::publish(msg).and_capture());
                }
                if dy < 0.0
                    && let Some(msg) = self.on_font_size_decrease.clone()
                {
                    return Some(CanvasAction::publish(msg).and_capture());
                }
                return Some(CanvasAction::capture());
            }
            // Mouse wheel, scrollback in the OS-natural direction:
            // wheel up shows older content (scroll_offset increases),
            // wheel down returns to the live edge (scroll_offset → 0).
            // Only consume when the cursor is actually over the terminal
            // canvas, otherwise the wheel bleeds into the AI sidebar.
            //
            // When the remote app has switched to the alternate screen
            // (top, vim, less, htop, …) we forward the wheel as cursor
            // arrows so paging works inside those apps, instead of
            // adding to our scrollback buffer (which is empty in alt
            // screen mode anyway).
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.position_in(bounds).is_some() =>
            {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y as i32 * 3,
                    mouse::ScrollDelta::Pixels { y, .. } => (*y / self.cell_height) as i32,
                };
                // One lock for both the alt-screen test and the scroll
                // clamp, this handler fires for every wheel tick and
                // locking twice doubled the contention with `process()`.
                let (in_alt_screen, max_scroll) = match self.state.lock() {
                    Ok(s) => {
                        let in_alt = s
                            .backend
                            .term
                            .mode()
                            .contains(alacritty_terminal::term::TermMode::ALT_SCREEN);
                        let grid = s.backend.term.grid();
                        (in_alt, grid.total_lines().saturating_sub(grid.screen_lines()) as i32)
                    }
                    Err(_) => (false, i32::MAX),
                };
                if in_alt_screen {
                    // Translate wheel into arrow-key bytes for the remote
                    // app, `top`/`vim`/`less` all listen for these. Routed
                    // through `emit_input` so it reaches the SSH session,
                    // a direct `state.write` only hits the local PTY and is
                    // a no-op on SSH tabs (this used to silently do nothing
                    // when scrolling vim / less over SSH).
                    let arrow: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
                    let count = lines.unsigned_abs().min(10) as usize;
                    let mut bytes = Vec::with_capacity(arrow.len() * count);
                    for _ in 0..count {
                        bytes.extend_from_slice(arrow);
                    }
                    return Some(self.emit_input(bytes));
                }
                widget_state.scroll_offset
                    .set((widget_state.scroll_offset.get() + lines).max(0).min(max_scroll));
                return Some(CanvasAction::request_redraw().and_capture());
            }
            // Modifier tracking for the URL Ctrl+Click gate. iced
            // doesn't pass the current modifier mask on mouse events,
            // so we mirror it from the dedicated change event.
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                widget_state.modifiers = *m;
            }
            // Keyboard, copy (paste is handled in app.rs so it can reach the
            // SSH session; widget.state.write only targets a local PTY). The
            // chord is Ctrl+Shift+C everywhere, plus Cmd+C on macOS, matching
            // the platform's native terminal convention.
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Character(c),
                modifiers,
                ..
            }) if is_clipboard_chord(modifiers) && matches!(c.as_str(), "C" | "c") => {
                if let Some(ref sel) = widget_state.selection
                    && !sel.is_empty()
                    && let Ok(state) = self.state.lock()
                {
                    let text = state.get_selection_text(sel);
                    if !text.is_empty() {
                        set_clipboard_text(&text);
                    }
                }
                return Some(CanvasAction::capture());
            }
            // Keyboard, select-all (Ctrl+Shift+A, plus Cmd+A on macOS).
            // Selects the entire buffer (scrollback + screen); copy stays a
            // separate gesture (the copy chord or copy-on-select on the next
            // release).
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Character(c),
                modifiers,
                ..
            }) if is_clipboard_chord(modifiers) && matches!(c.as_str(), "A" | "a") => {
                if let Ok(state) = self.state.lock() {
                    use alacritty_terminal::grid::Dimensions;
                    let grid = state.backend.term.grid();
                    let top = grid.topmost_line().0;
                    let bot = grid.bottommost_line().0;
                    let last_col = grid.columns().saturating_sub(1) as u16;
                    widget_state.selection = Some(Selection {
                        start: (0, top),
                        end: (last_col, bot),
                        block: false,
                    });
                    widget_state.select_anchor = None;
                }
                return Some(CanvasAction::request_redraw().and_capture());
            }
            // Any other key press dismisses a live selection, matching
            // xterm / iTerm where typing or navigating clears the highlight
            // (otherwise a stale selection lingers as a tinted band, e.g.
            // over a full-screen TUI like mc that took over the screen after
            // the selection was made), and, when enabled, jumps back to the
            // live edge (PuTTY's "reset scrollback on keypress"). The
            // keystroke is NOT captured: it must still reach the PTY through
            // the global key subscription (an independent path), so we only
            // drop the selection / reset the scroll and redraw. Bare modifier
            // presses (Ctrl / Shift / Alt / Super) must NOT trigger either,
            // otherwise the first key of a copy chord (Ctrl, then Shift+C)
            // wipes the selection before the copy fires. The copy / select-
            // all chords are handled by earlier arms that return first, so a
            // copy is never treated as a terminal keystroke here.
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. })
                if !matches!(
                    key,
                    keyboard::Key::Named(
                        keyboard::key::Named::Control
                            | keyboard::key::Named::Shift
                            | keyboard::key::Named::Alt
                            | keyboard::key::Named::Super
                            | keyboard::key::Named::Hyper
                            | keyboard::key::Named::Meta
                    )
                ) && (widget_state.selection.is_some()
                    || widget_state.select_anchor.is_some()
                    || (self.reset_scroll_on_keypress
                        && widget_state.scroll_offset.get() != 0)) =>
            {
                widget_state.selection = None;
                widget_state.select_anchor = None;
                widget_state.selecting = false;
                if self.reset_scroll_on_keypress {
                    widget_state.scroll_offset.set(0);
                }
                return Some(CanvasAction::request_redraw());
            }
            _ => {}
        }
        None
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if !cursor.is_over(bounds) {
            return mouse::Interaction::default();
        }
        // Pointer cursor over a URL, same as the browser hover affordance
        // and clear visual cue that "click does something different here".
        // Only when Ctrl is held does the click actually open the link.
        if state.hovered_url.is_some() {
            return mouse::Interaction::Pointer;
        }
        mouse::Interaction::Text
    }

    fn draw(
        &self,
        widget_state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

        let perf_on = self.perf_overlay || perf_overlay_enabled();
        let draw_start = perf_on.then(std::time::Instant::now);

        let cell_w = self.cell_width;
        let cell_h = self.cell_height;

        // --- Cheap RenderKey gate: decide hit/miss before any snapshot ---
        // The key is built from the content epoch (one very short lock) plus
        // the view/widget flags that change what a grid draws. On a match we
        // skip the whole snapshot + glyph build and reuse the cached
        // geometry; on a miss we clear the cache so `Cache::draw` below
        // re-runs the closure. The perf HUD and the visual-bell flash are
        // NOT part of the key: both are drawn as their own fresh top layers.
        let content_epoch = match self.state.lock() {
            Ok(s) => s.render_epoch(),
            Err(p) => p.into_inner().render_epoch(),
        };
        // PuTTY "reset scrollback on display activity": the render epoch
        // advances only on terminal output (process / sync-flush / palette),
        // never on scroll or cursor blink, so an epoch change since the last
        // draw means new activity. Jump to the live edge before the render
        // key is built so this frame draws at the bottom. Draw is `&self`,
        // hence the `Cell`s. Once-per-epoch by construction (the guard
        // updates `last_draw_epoch`), so the user can still scroll back
        // between two output batches and it sticks until the next one.
        if self.reset_scroll_on_output {
            let changed = widget_state
                .last_draw_epoch
                .get()
                .is_some_and(|e| e != content_epoch);
            if changed && widget_state.scroll_offset.get() != 0 {
                widget_state.scroll_offset.set(0);
            }
        }
        widget_state.last_draw_epoch.set(Some(content_epoch));
        let render_key = RenderKey {
            epoch: content_epoch,
            scroll_offset: widget_state.scroll_offset.get(),
            selection: widget_state.selection,
            hovered_url_cell: widget_state.hovered_url.as_ref().map(|(_, pos)| {
                (
                    ((pos.x - TERM_PAD) / cell_w).max(0.0) as u16,
                    ((pos.y - TERM_PAD_TOP) / cell_h).max(0.0) as u16,
                )
            }),
            hovered_osc8: widget_state.hovered_osc8,
            hovered_cell: if self.privacy { widget_state.hovered_cell } else { None },
            hover: widget_state.hover,
            scrollbar_dragging: widget_state.scrollbar_drag.is_some(),
            selecting: widget_state.selecting,
            privacy: self.privacy,
            keyword_highlight: self.keyword_highlight,
            performance: self.performance,
            smart_contrast: self.smart_contrast,
            bold_is_bright: self.bold_is_bright,
            privacy_terms_hash: if self.privacy { hash_terms(&self.privacy_terms) } else { 0 },
            pinned_privacy_hash: if self.privacy {
                hash_pinned(&widget_state.pinned_privacy)
            } else {
                0
            },
            font: self.font,
            font_size: self.font_size,
            cell_w,
            cell_h,
        };
        if widget_state.last_render_key.get() != Some(render_key) {
            widget_state.last_render_key.set(Some(render_key));
            widget_state.geometry_cache.clear();
        }

        // Per-phase timings, written from inside the (possibly skipped)
        // closure so the perf HUD layer below can read them. They stay ZERO
        // on a cache hit, which is precisely the signal that the hit avoided
        // the snapshot + build work.
        let mut lock_dur = std::time::Duration::ZERO;
        let mut cells_dur = std::time::Duration::ZERO;
        let mut highlights_dur = std::time::Duration::ZERO;
        // Set from inside the closure, so it reflects whether the geometry
        // was actually rebuilt. This is truer than comparing keys alone: the
        // fork's `Cache::draw` also re-runs the closure whenever `bounds`
        // changed (a resize), even when our key matched, so the HUD verdict
        // tracks the real work done rather than just our gate decision.
        let mut built = false;

        let grid_geometry = widget_state.geometry_cache.draw(
            renderer,
            bounds.size(),
            |frame| {
                built = true;
                let selection = &widget_state.selection;

                let mut cells: Vec<CellData> = DRAW_CELLS.take();
                cells.clear();
                let mut row_chars: Vec<(u16, Vec<(u16, char)>)> = Vec::new();

                // --- Snapshot phase, the only part that holds the state mutex ---
                // Everything draw needs (resolved cells, cursor, sizes, palette)
                // is copied out here and the lock is dropped before any text /
                // quad geometry is built, so drawing doesn't contend with
                // `process()` on the output path (see the typing-lag note on
                // `hovered_cell`).
                let lock_start = perf_on.then(std::time::Instant::now);
                let (
                    palette,
                    term_cursor,
                    screen_lines,
                    total_lines,
                    in_alt_screen,
                    scroll_offset,
                ) = {
                    let mut state = match self.state.lock() {
                        Ok(s) => s,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    lock_dur = lock_start.map(|t| t.elapsed()).unwrap_or_default();

                    // Auto-resize
                    let (new_cols, new_rows) = self.grid_size(bounds.width, bounds.height);
                    state.resize(new_cols, new_rows);

                    // Alt-screen apps (top, vim, less, htop, …) own the entire
                    // viewport with cursor positioning, there's no scrollback to
                    // page through. Force scroll_offset=0 so the user can't get
                    // stuck looking at stale history while the app keeps redrawing.
                    let in_alt_screen = state
                        .backend
                        .term
                        .mode()
                        .contains(alacritty_terminal::term::TermMode::ALT_SCREEN);

                    // Clamp scroll offset against the current grid bounds, resizes
                    // between frames can shrink history, so the offset stored in
                    // widget_state may exceed the new max.
                    let scroll_offset = if in_alt_screen {
                        0
                    } else {
                        let grid = state.backend.term.grid();
                        let max_scroll = grid.total_lines().saturating_sub(grid.screen_lines()) as i32;
                        widget_state.scroll_offset.get().clamp(0, max_scroll)
                    };

                    let term = &state.backend.term;
                    let palette = &state.palette;
                    let colors = term.colors();

                    let term_cursor = term.renderable_content().cursor;
                    let grid = term.grid();
                    let screen_lines = grid.screen_lines();
                    let cols_count = grid.columns();
                    let total_lines = grid.total_lines();
                    let topmost = grid.topmost_line();
                    let bottommost = grid.bottommost_line();

                    // --- Pass 1: collect cell data and build row character map ---
                    // Iterate the grid manually using `scroll_offset` as a row offset
                    // instead of mutating alacritty's `display_offset` via
                    // `scroll_display`. The previous approach yielded `display_iter`
                    // entries with negative `point.line.0` for scrollback rows, which
                    // when cast to `u16` wrapped to enormous numbers, those cells
                    // ended up rendered far off-screen, leaving blank rows in their
                    // place. Manual indexing keeps the math sane.
                    let cells_start = perf_on.then(std::time::Instant::now);
                    cells.reserve(screen_lines * cols_count);
                    row_chars.reserve(screen_lines);

                    // Flags that keep an otherwise blank default cell visible:
                    // INVERSE swaps the background in, underlines / strikeout
                    // paint rules over it.
                    let blank_visible_flags =
                        CellFlags::INVERSE | CellFlags::ALL_UNDERLINES | CellFlags::STRIKEOUT;

                    for visible_row in 0..screen_lines {
                        let line =
                            alacritty_terminal::index::Line(visible_row as i32 - scroll_offset);
                        if line < topmost || line > bottommost {
                            continue;
                        }
                        let row_data = &grid[line];
                        let mut chars: Vec<(u16, char)> = Vec::new();
                        for col_i in 0..cols_count {
                            let cell = &row_data[alacritty_terminal::index::Column(col_i)];

                            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                                continue;
                            }

                            let col = col_i as u16;
                            let row = visible_row as u16;
                            let c = cell.c;

                            // Skip cells that produce zero geometry: a blank glyph
                            // on the default background with no visible flags and
                            // no selection overlap. On a mostly empty screen this
                            // is the vast majority of the grid. (The cursor is
                            // painted independently of the cell snapshot, so a
                            // blank cell under it can be skipped too.)
                            if (c == ' ' || c == '\0')
                                && cell.bg == AnsiColor::Named(NamedColor::Background)
                                && !cell.flags.intersects(blank_visible_flags)
                                && !selection
                                    .as_ref()
                                    .is_some_and(|s| Self::is_in_selection(s, col, line.0))
                            {
                                continue;
                            }

                            let effective_fg =
                                if cell.flags.contains(CellFlags::BOLD) && self.bold_is_bright {
                                    brighten_named(&cell.fg)
                                } else {
                                    cell.fg
                                };
                            let fg = palette.resolve(&effective_fg, colors);
                            let bg = palette.resolve(&cell.bg, colors);

                            if c != ' ' && c != '\0' {
                                chars.push((col, c));
                            }

                            cells.push(CellData {
                                col,
                                row,
                                c,
                                fg,
                                bg,
                                flags: cell.flags,
                                link: cell.hyperlink().is_some(),
                            });
                        }
                        if !chars.is_empty() {
                            row_chars.push((visible_row as u16, chars));
                        }
                    }

                    cells_dur = cells_start.map(|t| t.elapsed()).unwrap_or_default();

                    (
                        state.palette.clone(),
                        term_cursor,
                        screen_lines,
                        total_lines,
                        in_alt_screen,
                        scroll_offset,
                    )
                };
                let palette = &palette;

                frame.fill_rectangle(Point::ORIGIN, bounds.size(), palette.background);

                // --- Detect syntax highlights ---
                // Runs when keyword tinting OR Privacy Mode is on; the latter needs
                // the IP / user@host spans to mask even when tinting is off.
                // Performance mode suppresses the tinting scan, but NOT when
                // privacy is active: killing the scan there would unmask every
                // IP / user@host, so privacy always wins over the perf skip.
                let highlights_start = perf_on.then(std::time::Instant::now);
                let scan_for_tint = self.keyword_highlight && !self.performance;
                let highlights = if scan_for_tint || self.privacy {
                    detect_highlights(&row_chars, palette, self.privacy, &self.privacy_terms)
                } else {
                    Vec::new()
                };
                highlights_dur = highlights_start.map(|t| t.elapsed()).unwrap_or_default();

                // Privacy Mode: the IP / user@host span the cursor is over right
                // now (from the last hovered cell), revealed while the rest stay
                // masked. Mirrors `hovered_url_extent` but keyed off `hovered_cell`
                // so it works without the cursor being over a clickable link.
                let hovered_privacy_extent: Option<(u16, u16, u16)> = if self.privacy {
                    widget_state
                        .hovered_cell
                        .and_then(|(col, vrow)| privacy_span_at(&highlights, vrow, col))
                } else {
                    None
                };

                // Spans whose value the user click-pinned visible, resolved per
                // frame against the pinned set so every occurrence of the value
                // (including re-prints and scrolled copies) stays revealed until
                // clicked again.
                let pinned_extents: Vec<(u16, u16, u16)> =
                    if self.privacy && !widget_state.pinned_privacy.is_empty() {
                        privacy_spans_with_text(&highlights, &row_chars)
                            .into_iter()
                            .filter(|(_, text)| widget_state.pinned_privacy.contains(text))
                            .map(|(ext, _)| ext)
                            .collect()
                    } else {
                        Vec::new()
                    };

                // Resolve which URL (if any) the cursor is over right now,
                // re-derived from the hovered cursor pixel position. We can't
                // trust the column we cached on hover because the grid may
                // have re-flowed since (resize, scroll). Drives the
                // "underline only the hovered URL" rule.
                let hovered_url_extent: Option<(u16, u16, u16)> = if let Some((_, pos)) =
                    widget_state.hovered_url
                {
                    let col = ((pos.x - TERM_PAD) / cell_w).max(0.0) as u16;
                    let row = ((pos.y - TERM_PAD_TOP) / cell_h).max(0.0) as u16;
                    hovered_url_range(&highlights, row, col)
                } else {
                    None
                };
                // An OSC 8 link's run was captured at hover time (it isn't in the
                // regex highlight scan); underline it the same way.
                let hovered_osc8 = widget_state.hovered_osc8;

                // --- Pass 2: draw cells with highlight overrides ---
                // Consecutive plain ASCII glyphs in a row that share the same
                // foreground (and the base font) are merged into one fill_text
                // run, one String + one shaping pass per run instead of per
                // glyph. This leans on the monospace advance matching the cell
                // width; runs are kept short and re-anchored to the grid so a
                // font whose advance is off by a hair can only drift
                // sub-pixel within one run. Wide chars, PUA symbols and
                // non-ASCII glyphs keep per-cell positioning because their
                // glyphs (often from a fallback font) need not advance by one
                // cell.
                struct GlyphRun {
                    row: u16,
                    start_col: u16,
                    next_col: u16,
                    fg: Color,
                    content: String,
                }
                // Re-anchor at most every 32 cells; bounds intra-run drift.
                const MAX_RUN_LEN: usize = 32;
                // Bridge small gaps (skipped blank cells) with spaces so a row
                // of short tokens still coalesces into few runs.
                const MAX_RUN_GAP: u16 = 4;
                let mut run: Option<GlyphRun> = None;
                let font_size = self.font_size;
                let base_font = self.font;
                let flush_run = |frame: &mut Frame, run: GlyphRun| {
                    frame.fill_text(CanvasText {
                        content: run.content,
                        position: Point::new(
                            run.start_col as f32 * cell_w + TERM_PAD,
                            run.row as f32 * cell_h + TERM_PAD_TOP,
                        ),
                        color: run.fg,
                        size: Pixels(font_size),
                        font: base_font,
                        align_x: alignment::Horizontal::Left.into(),
                        align_y: alignment::Vertical::Top,
                        ..Default::default()
                    });
                };
                for cd in &cells {
                    let x = cd.col as f32 * cell_w + TERM_PAD;
                    let y = cd.row as f32 * cell_h + TERM_PAD_TOP;

                    let mut fg = cd.fg;
                    let mut bg = cd.bg;
                    // The glyph actually drawn for this cell. Privacy Mode swaps it
                    // for a block below; everything else draws the real character.
                    let mut glyph = cd.c;

                    if cd.flags.contains(CellFlags::INVERSE) {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    if cd.flags.contains(CellFlags::DIM) {
                        fg = Color::from_rgba(fg.r * 0.66, fg.g * 0.66, fg.b * 0.66, fg.a);
                    }

                    // Syntax highlight override (only when text has default/foreground
                    // color). Gated on `keyword_highlight` so Privacy Mode, which also
                    // populates `highlights`, doesn't tint tokens when tinting is off.
                    if self.keyword_highlight
                        && let Some(hl_color) = highlight_color_at(&highlights, cd.row, cd.col)
                    {
                        // Only override if the cell isn't already colored by the application
                        let fg_is_default =
                            (fg.r - palette.foreground.r).abs() < 0.02
                            && (fg.g - palette.foreground.g).abs() < 0.02
                            && (fg.b - palette.foreground.b).abs() < 0.02;
                        if fg_is_default {
                            fg = hl_color;
                        }
                    }

                    // Explicit OSC 8 hyperlink: tint with the URL color (ansi blue),
                    // same as a detected URL, but only when the app left the text at
                    // the default foreground (don't fight an app that colored its own
                    // link). Persistent, the hover underline is added separately.
                    if cd.link {
                        let fg_is_default = (fg.r - palette.foreground.r).abs() < 0.02
                            && (fg.g - palette.foreground.g).abs() < 0.02
                            && (fg.b - palette.foreground.b).abs() < 0.02;
                        if fg_is_default {
                            fg = palette.ansi[4];
                        }
                    }

                    // Selection highlight, convert visible row to grid-line so
                    // the selection follows scrolled content instead of staying
                    // glued to viewport coordinates.
                    let cell_line = Self::visible_row_to_line(cd.row, scroll_offset);
                    let is_selected = selection
                        .as_ref()
                        .map(|s| Self::is_in_selection(s, cd.col, cell_line))
                        .unwrap_or(false);

                    if is_selected {
                        bg = Color::from_rgba(0.133, 0.60, 0.569, 0.35);
                        fg = Color::WHITE;
                    }

                    // Smart contrast, when an app picks a colour pair that
                    // renders too close to disappear (PowerShell's
                    // `$PSStyle.FileInfo.Directory` blue-on-blue, LS_COLORS'
                    // `ow` green-on-green over a green palette), swap the
                    // foreground for white or near-black depending on the
                    // background's luminance. Only kicks in when the cell
                    // actually has a non-default background, preserves
                    // colour-precise output everywhere else.
                    if self.smart_contrast && !is_selected {
                        let bg_overrides_default = (bg.r - palette.background.r).abs() >= 0.01
                            || (bg.g - palette.background.g).abs() >= 0.01
                            || (bg.b - palette.background.b).abs() >= 0.01;
                        if bg_overrides_default && contrast_ratio(fg, bg) < 2.5 {
                            fg = if relative_luminance(bg) >= 0.4 {
                                Color::from_rgb(0.05, 0.06, 0.07)
                            } else {
                                Color::WHITE
                            };
                        }
                    }

                    // Privacy Mode masking: cells inside a privacy span (IP,
                    // user@host, home-dir username, saved hostname) draw an inset
                    // filled bar (after the background, below) instead of their
                    // glyph. Every cell of the span is masked, separators
                    // included: a visible `.` / `@` / `:` would reveal the
                    // value's shape (octet count, username length), so the whole
                    // token reads as one solid block. The vertical inset keeps
                    // stacked masked lines from merging into a wall. The span the
                    // cursor hovers is revealed (same hover-reveal as links), and
                    // click-pinned values stay revealed.
                    let mut mask_bar = false;
                    if self.privacy && is_privacy_cell(&highlights, cd.row, cd.col) {
                        let in_extent = |&(r, sc, ec): &(u16, u16, u16)| {
                            cd.row == r && cd.col >= sc && cd.col <= ec
                        };
                        let revealed = hovered_privacy_extent.as_ref().is_some_and(in_extent)
                            || pinned_extents.iter().any(in_extent);
                        if !revealed {
                            // Opaque tone blended toward the background, then
                            // desaturated to neutral grey: keeping the theme hue
                            // makes the mask mimic legitimate reverse-video
                            // content (on a teal theme it reads as a highlight
                            // banner, not a censor mark). Brightness is kept by
                            // re-encoding the blend's linear luminance to sRGB.
                            let blend = Color {
                                r: palette.foreground.r * 0.45 + palette.background.r * 0.55,
                                g: palette.foreground.g * 0.45 + palette.background.g * 0.55,
                                b: palette.foreground.b * 0.45 + palette.background.b * 0.55,
                                a: 1.0,
                            };
                            let lum = relative_luminance(blend);
                            let grey = if lum <= 0.003_130_8 {
                                lum * 12.92
                            } else {
                                1.055 * lum.powf(1.0 / 2.4) - 0.055
                            };
                            fg = Color { r: grey, g: grey, b: grey, a: 1.0 };
                            mask_bar = true;
                            glyph = ' ';
                        }
                    }

                    // Draw background
                    let is_default_bg = !is_selected
                        && (bg.r - palette.background.r).abs() < 0.01
                        && (bg.g - palette.background.g).abs() < 0.01
                        && (bg.b - palette.background.b).abs() < 0.01;

                    if !is_default_bg {
                        let width = if cd.flags.contains(CellFlags::WIDE_CHAR) { cell_w * 2.0 } else { cell_w };
                        frame.fill_rectangle(Point::new(x, y), Size::new(width, cell_h), bg);
                    }

                    // Privacy redaction bar: an inset filled rect (drawn over the
                    // background) for a masked alphanumeric cell. The vertical inset
                    // is what keeps stacked masked lines from merging into a wall.
                    if mask_bar {
                        let width = if cd.flags.contains(CellFlags::WIDE_CHAR) { cell_w * 2.0 } else { cell_w };
                        let inset = (cell_h * 0.12).clamp(1.0, 3.0);
                        frame.fill_rectangle(
                            Point::new(x, y + inset),
                            Size::new(width, (cell_h - inset * 2.0).max(1.0)),
                            fg,
                        );
                    }

                    // Draw character. Codepoints in the Unicode Private Use
                    // Areas are forced through the bundled SauceCodePro Nerd
                    // Font: cosmic-text's auto-fallback tends to pick CJK
                    // fonts (which use the PUA for user-defined chars) before
                    // our Nerd Font for the F0xx range, so prompts with
                    // Powerline / Font Awesome / Devicons would render as
                    // tofu or wrong-script glyphs. Forcing the symbol font
                    // here is what alacritty/wezterm call a "symbol_map",
                    // hard-coded to the bundled family since we ship it in
                    // the binary.
                    //
                    // `\t` is a marker the emulator parks at the *start* of a
                    // tab span (see alacritty's `put_tab` in `term/mod.rs`)
                    // so clipboard copy can recover the original TAB. It's
                    // not a glyph: GNU `ls` in TTY column mode pads with tabs,
                    // so rendering it would tofu after every filename.
                    if glyph != ' ' && glyph != '\0' && glyph != '\t' {
                        let cp = glyph as u32;
                        // Both Private Use Areas: BMP PUA covers Powerline,
                        // Font Awesome, Devicons, Octicons, Codicons and the
                        // rest of the legacy Nerd Font ranges; SMP PUA is
                        // where Nerd Font v3+ stuffed the Material Design
                        // Icons. Regular fonts don't use either area, so we
                        // can safely force the bundled Nerd Font across both.
                        let is_pua =
                            (0xE000..=0xF8FF).contains(&cp) || (0xF0000..=0xFFFFD).contains(&cp);
                        let is_wide = cd.flags.contains(CellFlags::WIDE_CHAR);
                        if !is_pua && !is_wide && glyph.is_ascii_graphic() {
                            // Batchable glyph: extend the open run when it lines
                            // up (same row, same color, contiguous or within a
                            // short bridgeable gap), otherwise start a new one.
                            let fits = run.as_ref().is_some_and(|r| {
                                r.row == cd.row
                                    && r.fg == fg
                                    && cd.col >= r.next_col
                                    && cd.col - r.next_col <= MAX_RUN_GAP
                                    && r.content.len() < MAX_RUN_LEN
                            });
                            if fits {
                                let r = run.as_mut().expect("checked by fits");
                                for _ in r.next_col..cd.col {
                                    r.content.push(' ');
                                }
                                r.content.push(glyph);
                                r.next_col = cd.col + 1;
                            } else {
                                if let Some(r) = run.take() {
                                    flush_run(frame, r);
                                }
                                run = Some(GlyphRun {
                                    row: cd.row,
                                    start_col: cd.col,
                                    next_col: cd.col + 1,
                                    fg,
                                    content: glyph.to_string(),
                                });
                            }
                        } else {
                            if let Some(r) = run.take() {
                                flush_run(frame, r);
                            }
                            let font = if is_pua { NERD_FONT } else { self.font };
                            frame.fill_text(CanvasText {
                                content: glyph.to_string(),
                                position: Point::new(x, y),
                                color: fg,
                                size: Pixels(self.font_size),
                                font,
                                align_x: alignment::Horizontal::Left.into(),
                                align_y: alignment::Vertical::Top,
                                ..Default::default()
                            });
                        }
                    }

                    // Underline, from explicit ANSI SGR flags, or for URL
                    // cells that the cursor is currently hovering over (the
                    // visual cue paired with the Pointer cursor).
                    // Other URLs in the viewport stay un-underlined to avoid
                    // looking like every link is independently clickable.
                    let is_hovered_url = hovered_url_extent.is_some_and(|(r, sc, ec)| {
                        cd.row == r && cd.col >= sc && cd.col <= ec
                    }) || hovered_osc8.is_some_and(|(r, sc, ec)| {
                        cd.row == r && cd.col >= sc && cd.col <= ec
                    });
                    if cd.flags.intersects(CellFlags::ALL_UNDERLINES) || is_hovered_url {
                        let width = if cd.flags.contains(CellFlags::WIDE_CHAR) { cell_w * 2.0 } else { cell_w };
                        frame.fill_rectangle(Point::new(x, y + cell_h - 2.0), Size::new(width, 1.0), fg);
                    }

                    // Strikethrough
                    if cd.flags.contains(CellFlags::STRIKEOUT) {
                        let width = if cd.flags.contains(CellFlags::WIDE_CHAR) { cell_w * 2.0 } else { cell_w };
                        frame.fill_rectangle(Point::new(x, y + cell_h / 2.0), Size::new(width, 1.0), fg);
                    }
                }
                if let Some(r) = run.take() {
                    flush_run(frame, r);
                }

                // Hand the cell snapshot buffer back so its capacity is reused
                // by the next frame.
                DRAW_CELLS.set(cells);

                // Cursor, only render when its visible row falls inside the
                // viewport. When the user scrolls into history, the cursor sits
                // below the visible area and shouldn't be drawn.
                let cursor = term_cursor;
                let visible_cursor_row = cursor.point.line.0 + scroll_offset;
                if (0..screen_lines as i32).contains(&visible_cursor_row) {
                    let cx = cursor.point.column.0 as f32 * cell_w + TERM_PAD;
                    let cy = visible_cursor_row as f32 * cell_h + TERM_PAD_TOP;
                    match cursor.shape {
                        CursorShape::Block => {
                            frame.fill_rectangle(
                                Point::new(cx, cy),
                                Size::new(cell_w, cell_h),
                                Color { a: 0.7, ..palette.cursor },
                            );
                        }
                        CursorShape::Beam => {
                            frame.fill_rectangle(Point::new(cx, cy), Size::new(2.0, cell_h), palette.cursor);
                        }
                        CursorShape::Underline => {
                            frame.fill_rectangle(
                                Point::new(cx, cy + cell_h - 2.0),
                                Size::new(cell_w, 2.0),
                                palette.cursor,
                            );
                        }
                        _ => {
                            frame.fill_rectangle(
                                Point::new(cx, cy),
                                Size::new(cell_w, cell_h),
                                Color { a: 0.5, ..palette.cursor },
                            );
                        }
                    }
                }

                // Scrollbar, only painted while the cursor is over the canvas
                // (or actively dragging), there's actual history to scroll, and
                // we're not in alt-screen mode (no scrollback there).
                // Keep the scrollbar visible during an active text-selection drag
                // too, even if the cursor leaves the widget (hover goes false), so
                // it doesn't blink out while auto-scrolling at the edge.
                let visible_scrollbar = !in_alt_screen
                    && (widget_state.hover
                        || widget_state.scrollbar_drag.is_some()
                        || widget_state.selecting);
                if visible_scrollbar
                    && let Some(sb) = scrollbar_geom(
                        bounds,
                        total_lines,
                        screen_lines,
                        scroll_offset,
                    )
                {
                    // Track, faint background gutter so the user has a visible
                    // hit target when clicking above/below the thumb.
                    frame.fill_rectangle(
                        Point::new(sb.track_x, sb.track_y),
                        Size::new(sb.track_w, sb.track_h),
                        Color { a: 0.08, ..palette.foreground },
                    );
                    // Thumb, pops out a little when dragging.
                    let thumb_alpha = if widget_state.scrollbar_drag.is_some() { 0.55 } else { 0.35 };
                    frame.fill_rectangle(
                        Point::new(sb.track_x, sb.thumb_y),
                        Size::new(sb.track_w, sb.thumb_h),
                        Color { a: thumb_alpha, ..palette.foreground },
                    );
                }
            },
        );

        let mut geometries = vec![grid_geometry];

        // Perf HUD as its own always-fresh top layer (never cached), so the
        // fps / phase numbers and the cache hit/miss verdict update every
        // frame even while the grid below is served from the cache. Drawn
        // above the grid, its opaque panel covers the glyphs beneath it, so
        // the grid pass no longer needs to reserve those cells.
        if let Some(start) = draw_start {
            let total = start.elapsed();
            let now = std::time::Instant::now();

            let (fps, max_lock, max_cells, max_hl, max_total) = {
                let mut stats = perf_stats().lock().unwrap();
                let frame_gap = stats
                    .last_draw_at
                    .map(|prev| now - prev)
                    .unwrap_or_default();
                stats.last_draw_at = Some(now);
                stats.samples.push_back(PerfSample {
                    frame_gap,
                    lock: lock_dur,
                    cells: cells_dur,
                    highlights: highlights_dur,
                    total,
                });
                while stats.samples.len() > PERF_WINDOW {
                    stats.samples.pop_front();
                }
                (
                    stats.fps(),
                    stats.max_lock(),
                    stats.max_cells(),
                    stats.max_highlights(),
                    stats.max_total(),
                )
            };

            // The grid palette lived inside the cache closure and isn't in
            // scope here; grab just the two colors the HUD paints with.
            let (hud_bg, hud_fg) = match self.state.lock() {
                Ok(s) => (s.palette.background, s.palette.foreground),
                Err(p) => {
                    let s = p.into_inner();
                    (s.palette.background, s.palette.foreground)
                }
            };

            // Two-line HUD pinned top-right. Line 1 shows the current frame
            // plus the cache verdict (a hit reads C0.0 / H0.0, the signal
            // that it skipped the snapshot + build); line 2 shows the rolling
            // **max** over the last `PERF_WINDOW` frames so transient spikes,
            // the kind that read as typing lag, stay visible long enough to
            // spot. Fractional ms because most healthy draws are sub-1 ms.
            let ms = |d: std::time::Duration| d.as_secs_f32() * 1000.0;
            let line1 = format!(
                "{:>4.0} fps   T{:>5.1}  L{:>4.1}  C{:>4.1}  H{:>4.1}   {}",
                fps,
                ms(total),
                ms(lock_dur),
                ms(cells_dur),
                ms(highlights_dur),
                if built { "cache:miss" } else { "cache:hit" },
            );
            let line2 = format!(
                "  peak     T{:>5.1}  L{:>4.1}  C{:>4.1}  H{:>4.1}",
                ms(max_total),
                ms(max_lock),
                ms(max_cells),
                ms(max_hl),
            );

            let panel_w = 300.0;
            let panel_h = 38.0;
            let panel = Rectangle::new(
                Point::new((bounds.width - panel_w - 8.0).max(0.0), 6.0),
                Size::new(panel_w, panel_h),
            );
            let border = Color { a: 0.5, ..hud_fg };
            let mut hud = Frame::new(renderer, bounds.size());
            hud.fill_rectangle(
                Point::new(panel.x, panel.y),
                Size::new(panel.width, panel.height),
                hud_bg,
            );
            hud.fill_rectangle(Point::new(panel.x, panel.y), Size::new(panel.width, 1.0), border);
            hud.fill_rectangle(
                Point::new(panel.x, panel.y + panel.height - 1.0),
                Size::new(panel.width, 1.0),
                border,
            );
            hud.fill_rectangle(Point::new(panel.x, panel.y), Size::new(1.0, panel.height), border);
            hud.fill_rectangle(
                Point::new(panel.x + panel.width - 1.0, panel.y),
                Size::new(1.0, panel.height),
                border,
            );
            for (i, content) in [line1, line2].into_iter().enumerate() {
                hud.fill_text(CanvasText {
                    content,
                    position: Point::new(panel.x + 8.0, panel.y + 6.0 + i as f32 * 13.0),
                    color: hud_fg,
                    size: Pixels(10.0),
                    font: self.font,
                    align_x: alignment::Horizontal::Left.into(),
                    align_y: alignment::Vertical::Top,
                    ..Default::default()
                });
            }
            geometries.push(hud.into_geometry());
        }

        // Visual bell: a brief translucent wash over the whole pane, its own
        // top layer so it sits above every glyph. A short timer in the app
        // clears `bell_flash`, ending the flash on the next frame.
        if self.bell_flash {
            // The grid foreground (used for the flash tint) lived inside the
            // cache closure; fetch it directly here.
            let flash_color = match self.state.lock() {
                Ok(s) => s.palette.foreground,
                Err(p) => p.into_inner().palette.foreground,
            };
            let mut flash = Frame::new(renderer, bounds.size());
            flash.fill_rectangle(
                Point::new(0.0, 0.0),
                bounds.size(),
                Color { a: 0.18, ..flash_color },
            );
            geometries.push(flash.into_geometry());
        }
        geometries
    }
}

/// For bold text, promote standard ANSI colors (0-7) to their bright variant (8-15).
/// This makes bold text colorful like in other terminal emulators.
fn brighten_named(color: &alacritty_terminal::vte::ansi::Color) -> alacritty_terminal::vte::ansi::Color {
    use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
    match color {
        AnsiColor::Named(named) => {
            let bright = match named {
                NamedColor::Black => NamedColor::BrightBlack,
                NamedColor::Red => NamedColor::BrightRed,
                NamedColor::Green => NamedColor::BrightGreen,
                NamedColor::Yellow => NamedColor::BrightYellow,
                NamedColor::Blue => NamedColor::BrightBlue,
                NamedColor::Magenta => NamedColor::BrightMagenta,
                NamedColor::Cyan => NamedColor::BrightCyan,
                NamedColor::White => NamedColor::BrightWhite,
                other => *other, // already bright or special, keep as-is
            };
            AnsiColor::Named(bright)
        }
        AnsiColor::Indexed(idx) if *idx < 8 => AnsiColor::Indexed(idx + 8),
        other => *other,
    }
}

#[cfg(test)]
mod mouse_report_tests {
    use super::*;

    fn view_and_state() -> (TerminalView<()>, TerminalWidgetState) {
        let term = TerminalState::new_no_pty(80, 24).unwrap();
        let view = TerminalView::new(Arc::new(Mutex::new(term)));
        (view, TerminalWidgetState::default())
    }

    fn bounds() -> Rectangle {
        Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 480.0))
    }

    /// SGR click tracking (mc, htop). Regression for the "must hold
    /// Shift to click the sidebar" report: a release whose press was
    /// never reported (it landed on a sibling widget, so the cursor is
    /// outside the canvas and no press is tracked) must NOT be consumed
    /// by the report path; capturing it starves sibling `button`s,
    /// which fire on release.
    #[test]
    fn untracked_release_is_not_reported() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let ev = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        // Cursor over the sidebar (outside the canvas), no tracked press.
        let cursor = mouse::Cursor::Available(Point::new(2000.0, 100.0));
        assert!(ws.report_button.is_none());
        let action = view.handle_mouse_report(&mut ws, &ev, bounds(), cursor, mode, 80, 24);
        assert!(action.is_none(), "sidebar release must stay local");
    }

    /// The canvas-originated press → drag off-canvas → release flow must
    /// still report the release (apps need the button-up to end a drag),
    /// falling back to the last reported cell.
    #[test]
    fn tracked_release_still_reports_after_leaving_canvas() {
        use alacritty_terminal::term::TermMode;
        let (view, mut ws) = view_and_state();
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        let press = iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let inside = mouse::Cursor::Available(Point::new(40.0, 40.0));
        let action = view.handle_mouse_report(&mut ws, &press, bounds(), inside, mode, 80, 24);
        assert!(action.is_some(), "on-canvas press must be reported");
        assert_eq!(ws.report_button, Some(ReportButton::Left));

        let release = iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let outside = mouse::Cursor::Available(Point::new(2000.0, 100.0));
        let action = view.handle_mouse_report(&mut ws, &release, bounds(), outside, mode, 80, 24);
        assert!(action.is_some(), "release of a reported press must land");
        assert!(ws.report_button.is_none(), "press tracking cleared on release");
    }
}
