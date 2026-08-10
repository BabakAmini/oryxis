//! The pane's backdrop when a background picture is set: the palette's
//! background colour with the picture laid over it, as its own canvas
//! UNDER the grid canvas (the app stacks the two, and `iced`'s `Stack`
//! gives every layer above the first its own render layer).
//!
//! Why a separate widget instead of drawing the picture inside the grid
//! canvas: within one render layer the renderer draws by primitive KIND,
//! not by submission order: quads, then meshes, then images, then text.
//! Every `fill_rectangle` is a mesh, so a picture drawn in the grid's own
//! frame sits ON TOP of every fill in that frame no matter where the
//! calls sit in the code: the fade veil, cell backgrounds, the selection,
//! the cursor and the scrollbar all vanished behind it, and only the text
//! stage survived. Splitting the picture into a lower layer restores the
//! real order: backdrop below, every grid fill and glyph above.
//!
//! The fade is baked into the picture (`opacity = 1 - dim`) rather than
//! veiled over it with a translucent fill, because inside THIS frame a
//! veil would lose to the same stage ordering (mesh under image). Over
//! the palette-colour base fill the two composite identically:
//! `image * (1 - dim) + background * dim`.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use iced::widget::canvas::{self, Geometry};
use iced::{mouse, Point, Rectangle, Size, Theme};

use super::background::{self, BackgroundImage, BgFit};
use super::state::TerminalState;

/// Canvas program painting one pane's background colour + picture.
/// Built per pane by the app, only while a picture resolves for the tab;
/// without one the grid canvas keeps painting its own flat backdrop.
pub struct Backdrop {
    /// The pane's terminal state, shared with the grid widget: the base
    /// fill must be the LIVE palette background (OSC 11 can retint it),
    /// not a colour resolved once at build time.
    state: Arc<Mutex<TerminalState>>,
    image: BackgroundImage,
}

impl Backdrop {
    pub fn new(state: Arc<Mutex<TerminalState>>, image: BackgroundImage) -> Self {
        Self { state, image }
    }
}

/// Everything the cached backdrop geometry depends on. `measured` is
/// part of the key on purpose: it is `None` until the picture has been
/// decoded, and without it the first (picture-less) frame would be
/// cached under a key that never changes again.
#[derive(Clone, Copy, PartialEq)]
struct BackdropKey {
    image: iced::advanced::image::Id,
    fit: BgFit,
    /// `dim` quantized so the key stays `Copy + Eq`; the picker's 10%
    /// steps are far coarser than this.
    dim_millis: u32,
    measured: Option<Size<u32>>,
    /// Palette background as 8-bit RGBA, the live OSC 11 value.
    base: [u8; 4],
}

#[derive(Default)]
pub struct BackdropProgramState {
    cache: canvas::Cache,
    last_key: Cell<Option<BackdropKey>>,
}

impl<Message> canvas::Program<Message> for Backdrop {
    type State = BackdropProgramState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let base = match self.state.lock() {
            Ok(s) => s.palette.background,
            Err(p) => p.into_inner().palette.background,
        };
        let measured = {
            use iced::advanced::image::Renderer as _;
            renderer.measure_image(&self.image.handle)
        };

        let key = BackdropKey {
            image: self.image.handle.id(),
            fit: self.image.fit,
            dim_millis: (self.image.dim.clamp(0.0, 1.0) * 1000.0) as u32,
            measured,
            base: base.into_rgba8(),
        };
        if state.last_key.get() != Some(key) {
            state.last_key.set(Some(key));
            state.cache.clear();
        }

        let geometry = state.cache.draw(renderer, bounds.size(), |frame| {
            // The base colour first: it shows through `Contain`'s gaps
            // and through the picture's fade, so the pane reads as the
            // terminal's theme with a picture in it rather than as a
            // picture with text on top.
            frame.fill_rectangle(Point::ORIGIN, bounds.size(), base);
            let Some(measured) = measured else {
                // Still decoding (or unreadable): the flat colour is the
                // whole backdrop this frame; the key above re-runs this
                // closure once a size arrives.
                return;
            };
            let opacity = 1.0 - self.image.dim.clamp(0.0, 1.0);
            // Canvas geometry is pane-local; `bounds` is window-absolute.
            let local = Rectangle::with_size(bounds.size());
            let picture = canvas::Image::new(&self.image.handle).opacity(opacity);
            if self.image.fit == BgFit::Tile {
                for (x, y) in background::tile_origins(local, measured) {
                    frame.draw_image(
                        Rectangle {
                            x,
                            y,
                            width: measured.width as f32,
                            height: measured.height as f32,
                        },
                        picture.clone(),
                    );
                }
            } else {
                frame.draw_image(background::place(self.image.fit, local, measured), picture);
            }
        });
        vec![geometry]
    }
}
