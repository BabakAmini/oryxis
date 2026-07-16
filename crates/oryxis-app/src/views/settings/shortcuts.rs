//! Settings -> Shortcuts section view. Split out of views/settings/mod.rs.

use super::*;
use iced::widget::column;

impl Oryxis {
    pub(crate) fn view_settings_shortcuts(&self) -> Element<'_, Message> {
        use crate::hotkeys::{default_bindings, HotkeyAction};
        // Keyboard rows are recorded in visual order.
        self.keynav_settings_reset();
        let defaults = default_bindings();

        // Header: title + hint + global reset button.
        let header = column![
                                text(crate::i18n::t("hotkey_edit_hint"))
                .size(11)
                .color(OryxisColors::t().text_muted),
            Space::new().height(10),
            self.settings_nav_slot(
                crate::keynav::RowAction::activate(Message::ResetAllHotkeys),
                6.0,
                styled_button(
                    crate::i18n::t("hotkey_reset_all"),
                    Message::ResetAllHotkeys,
                    OryxisColors::t().bg_selected,
                ),
            ),
            Space::new().height(16),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        let mut rows_col = column![header]
            .spacing(8)
            .width(Length::Fill)
            .align_x(dir_align_x());

        for action in HotkeyAction::all() {
            // The row is not one nav slot: it records a slot per chord
            // chip, plus the add chip and the reset button. Enter on a
            // chip starts a capture for THAT chord.
            rows_col = rows_col.push(self.hotkey_editor_row(*action, defaults.get(action)));
        }

        // Read-only footer for the one terminal gesture that isn't a
        // chord and so can't live in the table above: Ctrl+Wheel zoom
        // is handled in the scroll event. Terminal copy / paste /
        // select-all used to sit here too, as read-only rows, back when
        // they were hard-coded in the widget and the dispatcher; they
        // are ordinary editable actions now (#75).
        let static_rows = column![
            Space::new().height(20),
            text(crate::i18n::t("hotkey_terminal_handled"))
                .size(11)
                .color(OryxisColors::t().text_muted),
            Space::new().height(8),
            shortcut_row(
                vec![key_badge("Ctrl"), key_badge("Wheel")],
                crate::i18n::t("font_zoom_wheel"),
            ),
        ]
        .spacing(8)
        .width(Length::Fill)
        .align_x(dir_align_x());
        rows_col = rows_col.push(static_rows);

        scrollable(
            container(rows_col)
                .padding(Padding { top: 24.0, right: 24.0, bottom: 24.0, left: 24.0 }),
        )
        // Stable id so the keyboard router can keep the selected row
        // in view.
        .id(iced::widget::Id::new("settings-shortcuts-scroll"))
        .height(Length::Fill)
        .into()
    }
}
