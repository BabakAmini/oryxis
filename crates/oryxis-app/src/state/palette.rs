//! Command palette (C4) surface state: `Ctrl+Shift+P` fuzzy search
//! over every action. Only the open flag and the query are app state;
//! the row selection rides the modal keynav layer
//! (`ModalSurface::Modal(Modal::CommandPalette)`) and the row list is
//! rebuilt per frame in `crate::palette`.

#[derive(Default)]
pub(crate) struct PaletteState {
    /// Whether the palette modal is on screen.
    pub open: bool,
    /// Live fuzzy query typed in the palette's search input.
    pub query: String,
}
