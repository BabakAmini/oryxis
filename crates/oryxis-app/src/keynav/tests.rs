//! Unit tests for the pure keynav movement math.

use super::movement::{clamp_index, cycle_zone, grid_move, index_move, linear_move, MoveKey};
use super::FocusZone;

// A 2x3 grid plus a ragged last row of one item:
//   1 2 3
//   4 5 6
//   7
fn ragged() -> Vec<Vec<u32>> {
    vec![vec![1, 2, 3], vec![4, 5, 6], vec![7]]
}

#[test]
fn grid_right_wraps_last_to_first() {
    let rows = ragged();
    assert_eq!(grid_move(&rows, Some(7), MoveKey::Right, false, false), Some(1));
    assert_eq!(grid_move(&rows, Some(3), MoveKey::Right, false, false), Some(4));
}

#[test]
fn grid_left_wraps_first_to_last() {
    let rows = ragged();
    assert_eq!(grid_move(&rows, Some(1), MoveKey::Left, false, false), Some(7));
    assert_eq!(grid_move(&rows, Some(4), MoveKey::Left, false, false), Some(3));
}

#[test]
fn grid_down_clamps_column_on_ragged_row() {
    let rows = ragged();
    // Column 2 (item 6) moving down lands on the only item of row 3.
    assert_eq!(grid_move(&rows, Some(6), MoveKey::Down, false, false), Some(7));
    // And wraps back up to row 1 keeping the column.
    assert_eq!(grid_move(&rows, Some(7), MoveKey::Down, false, false), Some(1));
}

#[test]
fn grid_up_wraps_and_clamps() {
    let rows = ragged();
    assert_eq!(grid_move(&rows, Some(2), MoveKey::Up, false, false), Some(7));
    assert_eq!(grid_move(&rows, Some(7), MoveKey::Up, false, false), Some(4));
}

#[test]
fn grid_entry_with_no_selection() {
    let rows = ragged();
    // Right/Down enter at the first item, Left/Up at the last.
    assert_eq!(grid_move(&rows, None, MoveKey::Right, false, false), Some(1));
    assert_eq!(grid_move(&rows, None, MoveKey::Down, false, false), Some(1));
    assert_eq!(grid_move(&rows, None, MoveKey::Left, false, false), Some(7));
    assert_eq!(grid_move(&rows, None, MoveKey::Up, false, false), Some(7));
}

#[test]
fn grid_list_mode_is_linear() {
    let rows: Vec<Vec<u32>> = vec![vec![1], vec![2], vec![3]];
    assert_eq!(grid_move(&rows, Some(3), MoveKey::Down, false, true), Some(1));
    assert_eq!(grid_move(&rows, Some(1), MoveKey::Up, false, true), Some(3));
}

#[test]
fn grid_rtl_mirrors_horizontal_only() {
    let rows = ragged();
    // Visual right under RTL is the logical previous item.
    assert_eq!(grid_move(&rows, Some(2), MoveKey::Right, true, false), Some(1));
    assert_eq!(grid_move(&rows, Some(2), MoveKey::Left, true, false), Some(3));
    // Vertical movement is unaffected.
    assert_eq!(grid_move(&rows, Some(2), MoveKey::Down, true, false), Some(5));
    // Home/End keep logical (reading-order) semantics.
    assert_eq!(grid_move(&rows, Some(2), MoveKey::Home, true, false), Some(1));
    assert_eq!(grid_move(&rows, Some(2), MoveKey::End, true, false), Some(3));
}

#[test]
fn grid_home_end_act_within_current_row() {
    let rows = ragged();
    assert_eq!(grid_move(&rows, Some(5), MoveKey::Home, false, false), Some(4));
    assert_eq!(grid_move(&rows, Some(5), MoveKey::End, false, false), Some(6));
    // Without a selection they enter at the extremes of the whole grid.
    assert_eq!(grid_move(&rows, None, MoveKey::Home, false, false), Some(1));
    assert_eq!(grid_move(&rows, None, MoveKey::End, false, false), Some(7));
}

#[test]
fn grid_empty_and_stale_inputs() {
    let empty: Vec<Vec<u32>> = Vec::new();
    assert_eq!(grid_move(&empty, Some(1), MoveKey::Right, false, false), None);
    // Empty rows are skipped, not panicked on.
    let holey: Vec<Vec<u32>> = vec![vec![], vec![1, 2], vec![]];
    assert_eq!(grid_move(&holey, Some(2), MoveKey::Right, false, false), Some(1));
    // A stale selection (filtered out since the last render) re-enters
    // at the first item going forward.
    let rows = ragged();
    assert_eq!(grid_move(&rows, Some(99), MoveKey::Right, false, false), Some(1));
}

#[test]
fn linear_wraps_both_directions() {
    let items = [10, 20, 30];
    assert_eq!(linear_move(&items, Some(30), true), Some(10));
    assert_eq!(linear_move(&items, Some(10), false), Some(30));
    assert_eq!(linear_move(&items, Some(10), true), Some(20));
}

#[test]
fn linear_entry_and_edge_cases() {
    let items = [10, 20, 30];
    assert_eq!(linear_move(&items, None, true), Some(10));
    assert_eq!(linear_move(&items, None, false), Some(30));
    // Stale current falls back to the entry points.
    assert_eq!(linear_move(&items, Some(99), true), Some(10));
    let empty: [u32; 0] = [];
    assert_eq!(linear_move(&empty, Some(1), true), None);
}

#[test]
fn zone_cycle_full_loop() {
    use FocusZone::*;
    // Everything available, visual reading order:
    // Search -> Toolbar -> Content -> SubNav -> Search.
    let next = |cur| cycle_zone(cur, true, true, false, false, false);
    assert_eq!(next(None), Some(Toolbar));
    assert_eq!(next(Some(Toolbar)), Some(Content));
    assert_eq!(next(Some(Content)), Some(SubNav));
    assert_eq!(next(Some(SubNav)), None);
    // And backwards.
    let prev = |cur| cycle_zone(cur, false, true, false, false, false);
    assert_eq!(prev(None), Some(SubNav));
    assert_eq!(prev(Some(SubNav)), Some(Content));
    assert_eq!(prev(Some(Content)), Some(Toolbar));
    assert_eq!(prev(Some(Toolbar)), None);
}

#[test]
fn zone_cycle_skips_unavailable() {
    use FocusZone::*;
    // No search field (e.g. Known Hosts): SubNav wraps to Toolbar.
    assert_eq!(
        cycle_zone(Some(SubNav), true, false, false, false, false),
        Some(Toolbar)
    );
    // Empty content (no rows recorded): Toolbar jumps to SubNav.
    assert_eq!(
        cycle_zone(Some(Toolbar), true, true, false, false, true),
        Some(SubNav)
    );
    // Empty toolbar: Search jumps straight to Content.
    assert_eq!(
        cycle_zone(None, true, true, false, true, false),
        Some(Content)
    );
}

#[test]
fn index_move_wraps_and_enters() {
    // Wrap both directions.
    assert_eq!(index_move(3, Some(2), true), Some(0));
    assert_eq!(index_move(3, Some(0), false), Some(2));
    assert_eq!(index_move(3, Some(0), true), Some(1));
    // Entry from no selection.
    assert_eq!(index_move(3, None, true), Some(0));
    assert_eq!(index_move(3, None, false), Some(2));
    // Empty list moves nowhere.
    assert_eq!(index_move(0, None, true), None);
    assert_eq!(index_move(0, Some(1), false), None);
    // Stale index past the end is clamped before stepping.
    assert_eq!(index_move(2, Some(9), true), Some(0));
}

#[test]
fn clamp_index_shrunken_list() {
    assert_eq!(clamp_index(5, 3), Some(2));
    assert_eq!(clamp_index(1, 3), Some(1));
    assert_eq!(clamp_index(0, 0), None);
}

#[test]
fn zone_cycle_nothing_available_is_a_no_op() {
    use FocusZone::*;
    assert_eq!(cycle_zone(Some(SubNav), true, false, true, true, true), Some(SubNav));
    assert_eq!(cycle_zone(None, true, false, true, true, true), None);
}
