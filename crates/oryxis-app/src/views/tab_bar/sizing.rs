//! Tab bar: sizing. Split out of views/tab_bar/mod.rs.

use super::*;

/// Where the tab strip docks (Settings -> Interface -> Tab bar
/// position). `Top` / `Bottom` are the horizontal strips; `Left` /
/// `Right` dock a vertical tab list on that side of the window
/// (issue #87). Left / right are PHYSICAL sides, not logical ones:
/// the user picked an explicit edge, so RTL must not flip it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TabBarPos {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

impl TabBarPos {
    /// Parse the `tab_bar_position` setting value; anything
    /// unrecognized falls back to `Top`, mirroring the dispatch
    /// normalization.
    pub(crate) fn from_setting(v: &str) -> Self {
        match v {
            "bottom" => Self::Bottom,
            "left" => Self::Left,
            "right" => Self::Right,
            _ => Self::Top,
        }
    }

    /// A vertical (left / right docked) strip.
    pub(crate) fn is_side(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// Process-wide tab-strip dock position gate, mirroring the
/// `AUTO_TITLE` gate in `state/tabs.rs`: `active_tab_bg` is a free fn
/// called from every tab/chip renderer, so threading the setting through
/// each signature would touch the whole family for one gradient flip.
/// Set from boot + the Settings dispatch; read at render time.
static TAB_BAR_POS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub(crate) fn set_tab_bar_pos(pos: TabBarPos) {
    let v = match pos {
        TabBarPos::Top => 0,
        TabBarPos::Bottom => 1,
        TabBarPos::Left => 2,
        TabBarPos::Right => 3,
    };
    TAB_BAR_POS.store(v, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn tab_bar_pos() -> TabBarPos {
    match TAB_BAR_POS.load(std::sync::atomic::Ordering::Relaxed) {
        1 => TabBarPos::Bottom,
        2 => TabBarPos::Left,
        3 => TabBarPos::Right,
        _ => TabBarPos::Top,
    }
}
/// Decide how much horizontal space each tab gets. Returns
/// `(active_width, inactive_width)`. The active tab claims its natural
/// width when it fits; inactives split whatever's left, clamped to the
/// minimum so they don't disappear.
pub(crate) fn allocate_tab_widths(n: usize, available: f32) -> (f32, f32) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let n_f = n as f32;
    let total_spacing = TAB_SPACING * (n_f - 1.0).max(0.0);
    let usable = (available - total_spacing).max(0.0);
    if n == 1 {
        let tab_width = usable.clamp(TAB_MIN_WIDTH, TAB_NATURAL_WIDTH);
        return (tab_width, tab_width);
    }
    // Try natural for active + share rest among inactives.
    let active_target = TAB_NATURAL_WIDTH.min(usable);
    let remaining = (usable - active_target).max(0.0);
    let inactive = (remaining / (n_f - 1.0)).clamp(TAB_MIN_WIDTH, TAB_NATURAL_WIDTH);
    // If the inactives end up wider than the active (because total fits
    // generously), level them up so everything reads at the same width.
    let active = active_target.max(inactive);
    (active, inactive)
}

/// Width an inactive tab needs to show its full label without
/// truncation, clamped to `[TAB_MIN_WIDTH, TAB_NATURAL_WIDTH]`. The
/// reserved portion mirrors `truncate_label`'s (icon slot + gaps +
/// button padding) plus a couple px of slack so a content-sized tab
/// never ellipsizes its own label, with the trailing close slot and
/// the split-count chip added when those variants are present.
pub(crate) fn tab_content_width(label: &str, close_on_right: bool, has_count_chip: bool) -> f32 {
    let base = label.trim_end_matches(" (disconnected)");
    let chars = base.chars().count() as f32;
    // 29 = TAB_ICON_SLOT + 5 (gap) + 4 + 4 (truncate_label's reserve);
    // +6 slack so the last glyph isn't flush against the edge.
    let mut reserved = TAB_ICON_SLOT + 5.0 + 4.0 + 4.0 + 6.0;
    if close_on_right {
        // Trailing close slot reserves its own width (see session_tab).
        reserved += TAB_ICON_SLOT + 4.0;
    }
    if has_count_chip {
        // Split pane-count pill (COUNT_DISC) + its leading gap.
        reserved += 15.0 + 4.0;
    }
    (reserved + chars * TAB_CHAR_WIDTH).clamp(TAB_MIN_WIDTH, TAB_NATURAL_WIDTH)
}

/// Truncate a label to fit visually within `width` px at the tab font
/// size. Falls back to a single character + ellipsis on extreme shrink
/// so the user still sees something.
pub(crate) fn truncate_label(label: &str, width: f32) -> String {
    let reserved = TAB_ICON_SLOT + 5.0 + 4.0 + 4.0; // icon + gap + padding
    let usable = (width - reserved).max(0.0);
    let max_chars = (usable / TAB_CHAR_WIDTH).floor() as usize;
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = label.chars().collect();
    if chars.len() <= max_chars {
        return label.to_string();
    }
    let cut: String = chars
        .iter()
        .take(max_chars.saturating_sub(1))
        .collect();
    format!("{}…", cut)
}

/// Session tab: icon badge (host icon by default, X on hover) + label.
/// Width is fixed by the caller so the row layout adapts to overflow.
///
/// `close_on_right`: when true the close X gets its own slot at the
/// trailing edge of the tab and the OS badge always stays on the
/// leading edge. When false (the default, Termius-style), the X
/// replaces the OS badge in the leading slot on hover/active.
///
/// `status_dot`: when Some, a small filled circle of that color is
/// stacked over the OS badge's bottom-right corner. None hides the
/// dot entirely (local-shell tabs and users who disabled the setting).
///
/// `host_accent`: per-host accent color resolved from `Connection.color`.
/// When Some, the active-tab fill and label adopt this color instead of
/// the global accent, so each tab "breathes" the color of its host.
///
/// `host_icon_style`: shape the OS badge takes in this tab. Resolved
/// from the per-host override or the global `default_host_icon`
/// setting; defaults to Square here (back-compat with the previous
/// fixed shape) when the caller passes nothing custom.
/// Area tab: navigation entry (Hosts, SFTP, ...) rendered into the
/// top tab strip in Workspace mode. Same height + bg as a session
/// tab so the strip reads as one continuous row, but with a leading
/// glyph instead of a host badge and no close affordance (areas
/// can't be closed). Dispatches `ChangeView` so the existing
/// navigation handler picks it up.
/// Background for an active tab or area chip. By default it paints the
/// "lit from above" vertical accent gradient (a saturated tint at the top
/// fading to near-transparent at the bottom). When `solid_fill` is set
/// (Settings -> Interface -> Tab fill style = Solid color), it paints a
/// single flat accent tint instead, so the active tab reads as a uniform
/// chip. Shared by every tab/chip renderer (and the Settings preview) so
/// the choice stays consistent and the preview can't drift from the strip.
pub(crate) fn active_tab_bg(accent: Color, solid_fill: bool) -> Background {
    if solid_fill {
        return Background::Color(Color { a: 0.16, ..accent });
    }
    let hi = Color { a: 0.28, ..accent };
    let lo = Color { a: 0.04, ..accent };
    // The saturated edge always hugs the window edge the strip docks
    // against ("lit from the frame"), fading toward the content: top
    // strip lights from above, bottom from below, and the side-docked
    // vertical strips light from their outer edge.
    let (angle, start, end) = match tab_bar_pos() {
        TabBarPos::Top => (std::f32::consts::PI, hi, lo),
        TabBarPos::Bottom => (std::f32::consts::PI, lo, hi),
        TabBarPos::Left => (std::f32::consts::FRAC_PI_2, hi, lo),
        TabBarPos::Right => (std::f32::consts::FRAC_PI_2, lo, hi),
    };
    Background::Gradient(iced::Gradient::Linear(
        iced::gradient::Linear::new(iced::Radians(angle))
            .add_stop(0.0, start)
            .add_stop(1.0, end),
    ))
}
