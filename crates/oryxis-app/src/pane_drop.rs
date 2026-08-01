//! Where a dragged tab would land if it were dropped right now: the
//! pure geometry behind the split anchors (issue #112).
//!
//! Dropping a tab onto the content area turns its sessions into panes of
//! the tab already showing there, and WHERE in the area you release
//! decides the split: the right third of a pane splits that pane
//! vertically and puts the new one beside it, the outer band of the whole
//! grid lays a pane across the full width underneath everything. Two
//! panes on screen means two sets of anchors, so the same gesture
//! composes as the layout grows.
//!
//! The rules are iced's own, restated here rather than reached for:
//! `pane_grid`'s hit-testing (`in_edge`, `layout_region`) is private to
//! the widget and only ever runs for a drag that STARTED inside the grid.
//! A tab drag starts in the strip, so the widget never enters its
//! dragging state and never computes a target. What keeps the restatement
//! honest is that the pane rectangles are not recomputed at all: every
//! pane already reports its last-drawn rect through a `bounds_reporter`
//! (the same cells the OS-drop router hit-tests), so the only thing
//! mirrored from the fork is the pair of thresholds below. The tests pin
//! them, so drift shows up as a failure rather than as anchors that lie.

use iced::widget::pane_grid::{Edge, Pane, Region, Target};
use iced::{Point, Rectangle};

/// Fraction of the shorter side taken by the grid's outer drop band.
/// `pane_grid::THICKNESS_RATIO`.
const EDGE_THICKNESS_RATIO: f32 = 25.0;

/// The drop the cursor is currently proposing, plus the rectangle to
/// highlight for it.
///
/// The rectangle is what the arriving pane will actually occupy, not a
/// decoration drawn near the cursor: the whole point of showing it is
/// that the user commits to a layout, not to a guess.
// No `PartialEq`: the fork's `Target` doesn't implement it, and the
// consumer adapts to the fork rather than the other way round. Callers
// (and the tests) pattern-match instead, which is what they want anyway.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DropProposal {
    pub target: Target,
    pub highlight: Rectangle,
}

/// Resolve the cursor to a drop target, or `None` when it proposes
/// nothing (outside the grid, or in a pane's neutral middle).
///
/// `panes` is every pane's last-drawn rect in window coordinates.
pub(crate) fn drop_target_at(
    panes: &[(Pane, Rectangle)],
    cursor: Point,
) -> Option<DropProposal> {
    // A pane that exists but hasn't drawn yet still reports the zeroed
    // rect its cell was born with. Left in, it would drag the grid's
    // bounding box back to the origin: inflated area, wrong band
    // thickness, anchors that don't match any pane on screen. Reachable
    // whenever something splits the displayed tab mid-drag (a connect
    // landing in `make_split_pane`), so drop them before anything reads
    // a rectangle.
    let panes: Vec<(Pane, Rectangle)> = panes
        .iter()
        .copied()
        .filter(|(_, r)| r.width > 0.0 && r.height > 0.0)
        .collect();
    let area = grid_area(&panes)?;
    if !contains(area, cursor) {
        return None;
    }
    // The grid's outer band wins over any pane underneath it. That
    // ordering is what makes "drop at the bottom" mean a full-width pane
    // below EVERYTHING rather than a split of whichever pane happens to
    // sit in the corner.
    if let Some(edge) = grid_edge_at(area, cursor) {
        return Some(DropProposal {
            target: Target::Edge(edge),
            highlight: half_towards(area, edge),
        });
    }
    let (pane, bounds) = pane_at(&panes, cursor)?;
    // A pane's middle ninth proposes nothing. iced calls it
    // `Region::Center` and answers it by SWAPPING the two panes, which
    // has no meaning for a pane arriving from another tab; refusing is
    // the honest reading, and it hands the gesture a cancel zone for
    // free: pull back to the middle and release, nothing happens.
    let Region::Edge(edge) = region_at(bounds, cursor) else {
        return None;
    };
    Some(DropProposal {
        target: Target::Pane(pane, Region::Edge(edge)),
        highlight: half_towards(bounds, edge),
    })
}

/// The grid's own rectangle: the bounding box of its panes. Exact rather
/// than approximate, because the outermost panes touch the grid's edges
/// on every side (the spacing only ever sits BETWEEN panes).
fn grid_area(panes: &[(Pane, Rectangle)]) -> Option<Rectangle> {
    let (_, first) = panes.first()?;
    let mut area = *first;
    for (_, r) in panes.iter().skip(1) {
        let x = area.x.min(r.x);
        let y = area.y.min(r.y);
        area = Rectangle {
            x,
            y,
            width: (area.x + area.width).max(r.x + r.width) - x,
            height: (area.y + area.height).max(r.y + r.height) - y,
        };
    }
    (area.width > 0.0 && area.height > 0.0).then_some(area)
}

fn pane_at(panes: &[(Pane, Rectangle)], cursor: Point) -> Option<(Pane, Rectangle)> {
    if let Some(hit) = panes.iter().find(|(_, b)| contains(*b, cursor)) {
        return Some(*hit);
    }
    // The spacing between panes belongs to no pane, so a cursor crossing
    // a divider would blink the whole preview off and back on. Fall back
    // to the nearest pane: the gap is 4 px, so the answer is never in
    // doubt and the preview stays continuous across a split.
    panes
        .iter()
        .copied()
        .min_by(|(_, a), (_, b)| distance_sq(*a, cursor).total_cmp(&distance_sq(*b, cursor)))
}

/// Which outer band of the whole grid the cursor is in, if any.
/// Mirrors `pane_grid::in_edge`.
fn grid_edge_at(area: Rectangle, cursor: Point) -> Option<Edge> {
    let thickness = edge_thickness(area);
    if cursor.x > area.x && cursor.x < area.x + thickness {
        Some(Edge::Left)
    } else if cursor.x > area.x + area.width - thickness && cursor.x < area.x + area.width {
        Some(Edge::Right)
    } else if cursor.y > area.y && cursor.y < area.y + thickness {
        Some(Edge::Top)
    } else if cursor.y > area.y + area.height - thickness && cursor.y < area.y + area.height {
        Some(Edge::Bottom)
    } else {
        None
    }
}

/// Which third of a pane the cursor is in. Mirrors
/// `pane_grid::layout_region`: left and right are decided first, so the
/// corners belong to the vertical split rather than the horizontal one.
fn region_at(bounds: Rectangle, cursor: Point) -> Region {
    if cursor.x < bounds.x + bounds.width / 3.0 {
        Region::Edge(Edge::Left)
    } else if cursor.x > bounds.x + 2.0 * bounds.width / 3.0 {
        Region::Edge(Edge::Right)
    } else if cursor.y < bounds.y + bounds.height / 3.0 {
        Region::Edge(Edge::Top)
    } else if cursor.y > bounds.y + 2.0 * bounds.height / 3.0 {
        Region::Edge(Edge::Bottom)
    } else {
        Region::Center
    }
}

fn edge_thickness(area: Rectangle) -> f32 {
    (area.height / EDGE_THICKNESS_RATIO).min(area.width / EDGE_THICKNESS_RATIO)
}

/// The half of `rect` an arriving pane takes when it lands on `edge`.
///
/// Both kinds of target produce a half, because both end in a
/// `Node::split` and every split in `pane_grid` is born at `ratio: 0.5`
/// (`node.rs`), so one function covers them. Note this deliberately
/// diverges from what the WIDGET paints for `Target::Edge`, which is the
/// thin hit band (`edge_bounds`): the band is where you aim, the half is
/// what you get, and a preview that shows the aim instead of the result
/// is a preview nobody trusts twice.
fn half_towards(rect: Rectangle, edge: Edge) -> Rectangle {
    match edge {
        Edge::Top => Rectangle { height: rect.height / 2.0, ..rect },
        Edge::Left => Rectangle { width: rect.width / 2.0, ..rect },
        Edge::Right => Rectangle {
            x: rect.x + rect.width / 2.0,
            width: rect.width / 2.0,
            ..rect
        },
        Edge::Bottom => Rectangle {
            y: rect.y + rect.height / 2.0,
            height: rect.height / 2.0,
            ..rect
        },
    }
}

/// `Rectangle::contains` with the same half-open convention the widget
/// uses, kept local so the whole hit test reads in one place.
fn contains(r: Rectangle, p: Point) -> bool {
    p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height
}

/// Squared distance from a point to a rectangle (0 inside it).
fn distance_sq(r: Rectangle, p: Point) -> f32 {
    let dx = (r.x - p.x).max(0.0).max(p.x - (r.x + r.width));
    let dy = (r.y - p.y).max(0.0).max(p.y - (r.y + r.height));
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::pane_grid;
    use iced::Size;

    /// 1000x500 content area at the origin, so a third is ~333 px and
    /// the outer band is 500/25 = 20 px.
    const AREA: Rectangle = Rectangle { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 };

    /// The rects a live grid would report, built the way the widget lays
    /// them out. Production reads the panes' real drawn rects; deriving
    /// the test's from `pane_regions` keeps the fixtures honest without
    /// putting layout math back into the module.
    fn rects(state: &pane_grid::State<()>, area: Rectangle, spacing: f32) -> Vec<(Pane, Rectangle)> {
        let mut v: Vec<(Pane, Rectangle)> = state
            .layout()
            .pane_regions(spacing, 50.0, Size::new(area.width, area.height))
            .into_iter()
            .map(|(pane, r)| (pane, Rectangle { x: area.x + r.x, y: area.y + r.y, ..r }))
            .collect();
        v.sort_by_key(|(p, _)| format!("{p:?}"));
        v
    }

    fn single_pane() -> pane_grid::State<()> {
        pane_grid::State::new(()).0
    }

    fn two_panes_side_by_side() -> (pane_grid::State<()>, Pane, Pane) {
        let (mut state, first) = pane_grid::State::new(());
        let (second, _) = state.split(pane_grid::Axis::Vertical, first, ()).expect("split");
        (state, first, second)
    }

    fn target_at(panes: &[(Pane, Rectangle)], x: f32, y: f32) -> Target {
        drop_target_at(panes, Point::new(x, y))
            .unwrap_or_else(|| panic!("no target at ({x}, {y})"))
            .target
    }

    /// The reporter's first case: one pane, drop on its right, get a
    /// vertical split with the new pane beside it.
    #[test]
    fn right_of_a_lone_pane_splits_it_vertically() {
        let panes = rects(&single_pane(), AREA, 0.0);
        // Inside the pane's right third, but clear of the grid's own
        // outer band (20 px), so this is a PANE target, not an edge one.
        let target = target_at(&panes, 900.0, 250.0);
        assert!(
            matches!(target, Target::Pane(_, Region::Edge(Edge::Right))),
            "expected a right-edge pane region, got {target:?}"
        );
    }

    /// Its counterpart: the middle of a pane proposes nothing, so it is
    /// both an honest answer (iced's Center means swap, which a pane
    /// arriving from another tab can't do) and the gesture's cancel zone.
    #[test]
    fn the_middle_of_a_pane_proposes_nothing() {
        let panes = rects(&single_pane(), AREA, 0.0);
        assert!(drop_target_at(&panes, Point::new(500.0, 250.0)).is_none());
    }

    /// The reporter's second case: with the grid already split, the
    /// FOOTER of the whole area is a grid edge, so the new pane spans
    /// the full width underneath both. This is the ordering that makes
    /// the gesture mean what it looks like, and it only holds because
    /// the outer band is tested before the pane under the cursor.
    #[test]
    fn the_grid_footer_beats_the_pane_beneath_it() {
        let (state, _, _) = two_panes_side_by_side();
        let panes = rects(&state, AREA, 4.0);
        // 10 px above the bottom: inside the 20 px band, and also inside
        // the right-hand pane's bottom third.
        let target = target_at(&panes, 750.0, 490.0);
        assert!(
            matches!(target, Target::Edge(Edge::Bottom)),
            "the grid's outer band must win over the pane under it, got {target:?}"
        );
    }

    /// And just inside of that band the pane takes over again, so the
    /// two behaviours are reachable without the layout jumping.
    #[test]
    fn just_inside_the_band_the_pane_wins() {
        let (state, _, second) = two_panes_side_by_side();
        let panes = rects(&state, AREA, 4.0);
        let target = target_at(&panes, 750.0, 470.0);
        match target {
            Target::Pane(pane, Region::Edge(Edge::Bottom)) => {
                assert_eq!(pane, second, "the right-hand pane owns x = 750");
            }
            other => panic!("expected the right pane's bottom region, got {other:?}"),
        }
    }

    /// "2 panes, drop on the right, another split": the anchors follow
    /// the layout instead of addressing a fixed root.
    #[test]
    fn each_pane_carries_its_own_anchors() {
        let (state, first, second) = two_panes_side_by_side();
        let panes = rects(&state, AREA, 4.0);
        // Left pane spans x 0..498, right pane 502..1000. Pick points in
        // each one's LEFT third, clear of the grid band.
        match target_at(&panes, 100.0, 250.0) {
            Target::Pane(p, Region::Edge(Edge::Left)) => assert_eq!(p, first),
            other => panic!("expected the left pane, got {other:?}"),
        }
        match target_at(&panes, 600.0, 250.0) {
            Target::Pane(p, Region::Edge(Edge::Left)) => assert_eq!(p, second),
            other => panic!("expected the right pane, got {other:?}"),
        }
    }

    /// The gap between two panes belongs to no pane, so without the
    /// nearest-pane fallback the preview would blink off every time the
    /// cursor crossed a divider.
    #[test]
    fn the_divider_between_panes_still_proposes() {
        let (state, _, _) = two_panes_side_by_side();
        let panes = rects(&state, AREA, 4.0);
        // x = 500 lands in the 4 px spacing, inside no pane's rect.
        assert!(panes.iter().all(|(_, b)| !contains(*b, Point::new(500.0, 250.0))));
        let target = target_at(&panes, 500.0, 250.0);
        assert!(
            matches!(target, Target::Pane(_, Region::Edge(_))),
            "the divider must still resolve to a neighbouring pane, got {target:?}"
        );
    }

    /// A pane that hasn't drawn yet reports the zeroed rect its cell was
    /// born with. Counting it would stretch the grid back to the origin
    /// and every anchor with it, so it is dropped before anything reads
    /// a rectangle.
    #[test]
    fn a_pane_that_has_not_drawn_yet_is_ignored() {
        let area = Rectangle { x: 400.0, y: 100.0, width: 600.0, height: 400.0 };
        let mut panes = rects(&single_pane(), area, 0.0);
        // Stand-in for a pane of the same grid that hasn't drawn yet:
        // `Pane` has no public constructor, and only its identity
        // matters. A split's handle is the second one minted, so it
        // can't collide with the single drawn pane above.
        let (mut scratch, root) = pane_grid::State::new(());
        let (undrawn, _) = scratch.split(pane_grid::Axis::Vertical, root, ()).expect("split");
        panes.push((undrawn, Rectangle { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }));
        let proposal =
            drop_target_at(&panes, Point::new(950.0, 300.0)).expect("a proposal");
        assert!(
            matches!(proposal.target, Target::Pane(p, _) if p != undrawn),
            "the undrawn pane must never be a target"
        );
        assert_eq!(
            proposal.highlight.x, 700.0,
            "the anchors follow the DRAWN pane, not a box stretched to the origin"
        );
        // And a cursor over where that phantom rect sat proposes nothing,
        // rather than resolving to the nearest real pane far away.
        assert!(drop_target_at(&panes, Point::new(10.0, 10.0)).is_none());
    }

    /// A cursor outside the area proposes nothing, so releasing over the
    /// sidebar or the tab strip cannot silently rearrange the grid.
    #[test]
    fn outside_the_area_there_is_no_proposal() {
        let area = Rectangle { x: 200.0, y: 100.0, width: 400.0, height: 300.0 };
        let panes = rects(&single_pane(), area, 0.0);
        assert!(drop_target_at(&panes, Point::new(150.0, 250.0)).is_none());
        assert!(drop_target_at(&panes, Point::new(400.0, 50.0)).is_none());
        assert!(drop_target_at(&panes, Point::new(210.0, 250.0)).is_some());
    }

    /// The highlight is the space the pane will TAKE, which is a half
    /// for BOTH kinds of target: every `pane_grid` split is born at
    /// ratio 0.5. The widget paints its own thin band for a grid edge;
    /// we deliberately paint the result instead.
    #[test]
    fn the_highlight_shows_the_resulting_half() {
        let panes = rects(&single_pane(), AREA, 0.0);
        let proposal =
            drop_target_at(&panes, Point::new(900.0, 250.0)).expect("a proposal");
        assert_eq!(proposal.highlight.x, 500.0, "right half starts at the midpoint");
        assert_eq!(proposal.highlight.width, 500.0, "and is half the width");

        let proposal =
            drop_target_at(&panes, Point::new(500.0, 490.0)).expect("a proposal");
        assert!(
            matches!(proposal.target, Target::Edge(Edge::Bottom)),
            "the footer is a grid edge"
        );
        assert_eq!(proposal.highlight.height, 250.0, "a grid edge lands a HALF");
        assert_eq!(proposal.highlight.y, 250.0, "starting at the midpoint");
    }

    /// The area's origin is not always (0, 0): a sidebar or a side tab
    /// dock shifts it, and a proposal computed in grid-local space would
    /// then highlight the wrong place.
    #[test]
    fn proposals_are_in_window_coordinates() {
        let area = Rectangle { x: 300.0, y: 40.0, width: 600.0, height: 400.0 };
        let panes = rects(&single_pane(), area, 0.0);
        let proposal =
            drop_target_at(&panes, Point::new(320.0, 240.0)).expect("a proposal");
        assert!(matches!(proposal.target, Target::Pane(_, Region::Edge(Edge::Left))));
        assert_eq!(proposal.highlight.x, 300.0, "the highlight is offset by the area");
        assert_eq!(proposal.highlight.y, 40.0);
    }
}
