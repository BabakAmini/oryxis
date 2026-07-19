//! Keys screen + identity panel + SSH key import panel.
//!
//! One view method per sibling file; shared imports are centralized
//! here and pulled into each file via `use super::*`.

pub(crate) use iced::border::Radius;
pub(crate) use iced::widget::button::Status as BtnStatus;
pub(crate) use iced::widget::{
    button, container, pick_list, scrollable, text, text_editor, text_input, MouseArea, Space,
};
pub(crate) use iced::{Background, Border, Color, Element, Length, Padding};

pub(crate) use oryxis_core::models::connection::Connection;
pub(crate) use oryxis_core::models::identity::Identity;
pub(crate) use oryxis_core::models::key::SshKey;

pub(crate) use crate::app::{Message, Oryxis, CARD_WIDTH};
pub(crate) use crate::i18n::t;
pub(crate) use crate::theme::OryxisColors;
pub(crate) use crate::widgets::{card_grid_columns, dir_align_x, dir_row, distribute_card_grid};

// `column` carries both a fn and a `column!` macro; re-exporting it
// through the `use super::*` glob makes the macro ambiguous in the
// submodules (same gotcha as views/settings), so each file imports it
// directly instead.

mod cert_viewer;
mod generate;
mod identity;
mod import;
mod list;
