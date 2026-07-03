//! Pure movement math for the focus-zone keyboard router. No iced
//! types on purpose: everything here is unit-testable without a UI
//! harness (there is none for iced).

use super::FocusZone;

/// A within-zone movement key, already normalized by the router
/// (modifiers stripped; Tab is not here because Tab cycles zones,
/// not items).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

/// 2-D cyclic movement over visual rows, ported verbatim from the old
/// dashboard-only handler: Left/Right wrap linearly across the flat
/// order, Up/Down move by a grid row clamping the column on ragged
/// last rows (or move linearly in single-column `list_mode`), and a
/// `None` current selection enters at the first item (Right/Down) or
/// the last (Left/Up).
///
/// `rtl` mirrors Left/Right before mapping: rows are recorded in
/// logical order but `dir_row` reverses them visually, so the visual
/// "right" neighbour is the logical previous item. Home/End keep
/// logical (reading-order) semantics in both directions.
pub(crate) fn grid_move<T: Copy + PartialEq>(
    rows: &[Vec<T>],
    cur: Option<T>,
    key: MoveKey,
    rtl: bool,
    list_mode: bool,
) -> Option<T> {
    // Defensive: a view should never record an empty row (chunks()
    // can't produce one), but an empty row must not panic the router.
    let rows: Vec<&Vec<T>> = rows.iter().filter(|r| !r.is_empty()).collect();
    let flat: Vec<T> = rows.iter().flat_map(|r| r.iter()).copied().collect();
    if flat.is_empty() {
        return None;
    }
    let key = if rtl {
        match key {
            MoveKey::Left => MoveKey::Right,
            MoveKey::Right => MoveKey::Left,
            k => k,
        }
    } else {
        key
    };
    let n = flat.len();
    let flat_pos = cur.and_then(|c| flat.iter().position(|&i| i == c));
    // Linear forward / backward with wrap-around.
    let fwd = flat[flat_pos.map_or(0, |p| (p + 1) % n)];
    let back = flat[flat_pos.map_or(n - 1, |p| (p + n - 1) % n)];
    // (row, col) of the current selection, for row-wise Up/Down.
    let cur_rc = cur.and_then(|c| {
        rows.iter()
            .enumerate()
            .find_map(|(r, row)| row.iter().position(|&i| i == c).map(|col| (r, col)))
    });
    let nrows = rows.len();
    match key {
        MoveKey::Right => Some(fwd),
        MoveKey::Left => Some(back),
        MoveKey::Down if list_mode => Some(fwd),
        MoveKey::Up if list_mode => Some(back),
        MoveKey::Down => Some(match cur_rc {
            Some((r, c)) => {
                let nr = (r + 1) % nrows;
                rows[nr][c.min(rows[nr].len() - 1)]
            }
            None => flat[0],
        }),
        MoveKey::Up => Some(match cur_rc {
            Some((r, c)) => {
                let nr = (r + nrows - 1) % nrows;
                rows[nr][c.min(rows[nr].len() - 1)]
            }
            None => *flat.last().unwrap(),
        }),
        MoveKey::Home => Some(match cur_rc {
            Some((r, _)) => rows[r][0],
            None => flat[0],
        }),
        MoveKey::End => Some(match cur_rc {
            Some((r, _)) => *rows[r].last().unwrap(),
            None => *flat.last().unwrap(),
        }),
    }
}

/// 1-D cyclic movement (sub-nav pills, toolbar cluster, section
/// hotkey). `forward` already has any RTL mirroring applied by the
/// caller (the router maps visual arrows to logical direction per
/// orientation). A `None` current selection enters at the first item
/// going forward, the last going backward.
pub(crate) fn linear_move<T: Copy + PartialEq>(
    items: &[T],
    cur: Option<T>,
    forward: bool,
) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let n = items.len();
    let pos = cur.and_then(|c| items.iter().position(|&i| i == c));
    Some(match (pos, forward) {
        (Some(p), true) => items[(p + 1) % n],
        (Some(p), false) => items[(p + n - 1) % n],
        (None, true) => items[0],
        (None, false) => items[n - 1],
    })
}

/// 1-D wrapping movement over an index-addressed list (the modal /
/// menu / settings row layers). `None` enters at the first index
/// going forward, at the last going backward; empty lists move
/// nowhere. A stale `cur` past the end is clamped before stepping.
pub(crate) fn index_move(len: usize, cur: Option<usize>, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (cur.map(|c| c.min(len - 1)), forward) {
        (Some(c), true) => (c + 1) % len,
        (Some(c), false) => (c + len - 1) % len,
        (None, true) => 0,
        (None, false) => len - 1,
    })
}

/// Clamp a possibly stale index to a list that may have shrunk since
/// the selection was made. `None` when the list is empty.
pub(crate) fn clamp_index(cur: usize, len: usize) -> Option<usize> {
    if len == 0 {
        None
    } else {
        Some(cur.min(len - 1))
    }
}

/// Tab-cycle order: Search -> Toolbar -> Content -> SubNav -> Search.
/// Visual reading order of the surface: the search field, the action
/// buttons beside it, the content grid/list (the router additionally
/// steps through content SECTIONS before leaving the zone), and the
/// section nav (sidebar rail / pills) last. `None` stands for the
/// Search zone (iced text_input focus). Zones with no recorded items
/// are skipped, as is Search when the current view has no search
/// field (e.g. Known Hosts). Returns the current zone unchanged when
/// nothing else is available.
pub(crate) fn cycle_zone(
    cur: Option<FocusZone>,
    forward: bool,
    has_search: bool,
    subnav_empty: bool,
    toolbar_empty: bool,
    content_empty: bool,
) -> Option<FocusZone> {
    const ORDER: [Option<FocusZone>; 4] = [
        None,
        Some(FocusZone::Toolbar),
        Some(FocusZone::Content),
        Some(FocusZone::SubNav),
    ];
    let available = |z: Option<FocusZone>| match z {
        None => has_search,
        Some(FocusZone::SubNav) => !subnav_empty,
        Some(FocusZone::Toolbar) => !toolbar_empty,
        Some(FocusZone::Content) => !content_empty,
    };
    let start = ORDER.iter().position(|&z| z == cur).unwrap_or(0);
    for step in 1..=ORDER.len() {
        // `start + 2*len - step` keeps the subtraction positive.
        let idx = if forward {
            (start + step) % ORDER.len()
        } else {
            (start + ORDER.len() * 2 - step) % ORDER.len()
        };
        if available(ORDER[idx]) {
            return ORDER[idx];
        }
    }
    cur
}
