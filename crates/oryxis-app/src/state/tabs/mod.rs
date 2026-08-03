//! Terminal tabs and panes (split out of `state.rs`, then split again
//! into the two entities it held).
//!
//! - [`pane`]: one live session and everything mounted on it.
//! - [`tab`]: the grid of panes, and how a tab is addressed and placed.

mod pane;
mod tab;

pub(crate) use pane::*;
pub(crate) use tab::*;
