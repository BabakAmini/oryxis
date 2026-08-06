//! Terminal background image: how the picture is laid into the pane and
//! how far it is faded back so text stays readable on top of it.
//!
//! Drawn inside the canvas rather than as a widget layer behind it, for
//! two reasons: tiling needs the measured pixel size (which only the
//! renderer knows), and per-pane is the behaviour every terminal that
//! ships this feature has (Windows Terminal, iTerm2). The VALUE is
//! resolved once per tab from its origin host, so a split shows the same
//! picture in both panes, each laid out in its own pane rather than one
//! picture stretched across the seam.

use iced::{Rectangle, Size};

/// How the picture is scaled into the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BgFit {
    /// Scale (keeping the aspect ratio) until the pane is covered, and
    /// crop the overflow. The only mode that never leaves a gap, which
    /// is why it is the default.
    #[default]
    Cover,
    /// Scale (keeping the aspect ratio) until the whole picture fits,
    /// centred. Leaves background colour on two sides unless the aspect
    /// ratios match exactly.
    Contain,
    /// Fill the pane exactly, distorting the aspect ratio.
    Stretch,
    /// Draw at its own pixel size, centred, no scaling at all.
    Center,
    /// Repeat at its own pixel size from the top-left corner. For the
    /// small seamless textures this mode exists for, a pane-sized
    /// picture would be pure waste.
    Tile,
}

impl BgFit {
    /// Stable string form for the vault (settings row and the per-host
    /// JSON both round-trip through this).
    pub fn as_str(self) -> &'static str {
        match self {
            BgFit::Cover => "cover",
            BgFit::Contain => "contain",
            BgFit::Stretch => "stretch",
            BgFit::Center => "center",
            BgFit::Tile => "tile",
        }
    }

    /// Parse the stored form; anything unrecognised (a hand-edited row,
    /// a value written by a newer build) falls back to the default
    /// rather than dropping the user's picture.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "contain" => BgFit::Contain,
            "stretch" => BgFit::Stretch,
            "center" => BgFit::Center,
            "tile" => BgFit::Tile,
            _ => BgFit::Cover,
        }
    }

    pub const ALL: [BgFit; 5] = [
        BgFit::Cover,
        BgFit::Contain,
        BgFit::Stretch,
        BgFit::Center,
        BgFit::Tile,
    ];
}

/// A picture to lay behind the grid, already resolved (host override or
/// global default) by the app.
#[derive(Debug, Clone)]
pub struct BackgroundImage {
    /// Handle for the decoded picture. Cloning is cheap (the renderer
    /// owns the pixels and caches them by handle id).
    pub handle: iced::advanced::image::Handle,
    pub fit: BgFit,
    /// How far the picture is faded towards the terminal's background
    /// colour, 0.0 (untouched) to 1.0 (invisible). This is what keeps
    /// text readable over a photograph, so the picker offers a generous
    /// default rather than starting at zero.
    pub dim: f32,
}

/// Where a single copy of the picture lands, given the pane and the
/// picture's own pixel size. `Tile` is handled by the caller (it needs
/// as many rectangles as fit); every other mode is exactly one.
///
/// Kept pure and separate from drawing so the geometry can be tested
/// without a renderer.
pub fn place(fit: BgFit, bounds: Rectangle, image: Size<u32>) -> Rectangle {
    let (iw, ih) = (image.width.max(1) as f32, image.height.max(1) as f32);
    let (bw, bh) = (bounds.width.max(0.0), bounds.height.max(0.0));
    match fit {
        BgFit::Stretch => bounds,
        BgFit::Center | BgFit::Tile => Rectangle {
            x: bounds.x + (bw - iw) / 2.0,
            y: bounds.y + (bh - ih) / 2.0,
            width: iw,
            height: ih,
        },
        BgFit::Cover | BgFit::Contain => {
            let scale_w = bw / iw;
            let scale_h = bh / ih;
            // Cover takes the larger scale (the pane is filled and the
            // long side spills over), Contain the smaller one (the
            // whole picture survives and the short side leaves a gap).
            let scale = if fit == BgFit::Cover {
                scale_w.max(scale_h)
            } else {
                scale_w.min(scale_h)
            };
            let (w, h) = (iw * scale, ih * scale);
            Rectangle {
                x: bounds.x + (bw - w) / 2.0,
                y: bounds.y + (bh - h) / 2.0,
                width: w,
                height: h,
            }
        }
    }
}

/// Top-left corners for `Tile`, anchored at the pane's top-left corner
/// and repeating right and down, which is what CSS `repeat` and every
/// terminal with this mode do (a centred grid would shift the whole
/// pattern on every resize).
///
/// Bounded: a 1x1 picture over a 4K pane would otherwise ask for eight
/// million draw calls, so past a cap it degrades to one centred copy.
/// Visibly wrong for that pathological file, but the window keeps
/// running, which a per-frame million-draw loop would not.
pub fn tile_origins(bounds: Rectangle, image: Size<u32>) -> Vec<(f32, f32)> {
    const MAX_TILES: usize = 4096;
    let (iw, ih) = (image.width.max(1) as f32, image.height.max(1) as f32);
    let cols = (bounds.width / iw).ceil().max(1.0) as usize;
    let rows = (bounds.height / ih).ceil().max(1.0) as usize;
    if cols.saturating_mul(rows) > MAX_TILES {
        let r = place(BgFit::Center, bounds, image);
        return vec![(r.x, r.y)];
    }
    let mut out = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            out.push((bounds.x + col as f32 * iw, bounds.y + row as f32 * ih));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Rectangle {
        Rectangle {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 200.0,
        }
    }

    #[test]
    fn cover_fills_the_pane_and_crops_the_long_side() {
        // A square picture in a 2:1 pane: covering means scaling to the
        // WIDTH, so the height spills over equally above and below.
        let r = place(BgFit::Cover, pane(), Size::new(100, 100));
        assert_eq!(r.width, 400.0);
        assert_eq!(r.height, 400.0);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, -100.0, "overflow is centred, not anchored to the top");
    }

    #[test]
    fn contain_keeps_the_whole_picture_inside() {
        let r = place(BgFit::Contain, pane(), Size::new(100, 100));
        assert_eq!(r.height, 200.0);
        assert_eq!(r.width, 200.0);
        assert_eq!(r.x, 100.0, "gap is split between both sides");
        assert_eq!(r.y, 0.0);
        assert!(r.width <= pane().width && r.height <= pane().height);
    }

    #[test]
    fn stretch_matches_the_pane_exactly() {
        assert_eq!(place(BgFit::Stretch, pane(), Size::new(37, 991)), pane());
    }

    #[test]
    fn center_never_scales() {
        let r = place(BgFit::Center, pane(), Size::new(100, 50));
        assert_eq!((r.width, r.height), (100.0, 50.0));
        assert_eq!((r.x, r.y), (150.0, 75.0));
    }

    #[test]
    fn tiling_covers_the_pane_and_stays_bounded() {
        let origins = tile_origins(pane(), Size::new(100, 100));
        assert!(!origins.is_empty());
        // Every pane pixel has a tile over it: the grid starts at or
        // before the origin and runs past the far corner.
        let min_x = origins.iter().map(|o| o.0).fold(f32::MAX, f32::min);
        let max_x = origins.iter().map(|o| o.0).fold(f32::MIN, f32::max);
        assert!(min_x <= 0.0, "grid starts at or before the left edge");
        assert!(max_x + 100.0 >= 400.0, "grid runs past the right edge");

        // A one-pixel texture over a large pane must not ask for a
        // million draw calls; it degrades to a single copy instead.
        let degenerate = tile_origins(
            Rectangle {
                x: 0.0,
                y: 0.0,
                width: 3840.0,
                height: 2160.0,
            },
            Size::new(1, 1),
        );
        assert_eq!(degenerate.len(), 1);
    }

    #[test]
    fn fit_round_trips_through_its_stored_form() {
        for fit in BgFit::ALL {
            assert_eq!(BgFit::from_str_or_default(fit.as_str()), fit);
        }
        // Unknown values keep the picture rather than dropping it.
        assert_eq!(BgFit::from_str_or_default("nonsense"), BgFit::Cover);
    }
}
