//! The password-suggest popup (issue #117): credential rows offered at
//! a password prompt, anchored under the terminal caret.
//!
//! It rides the overlay-menu machinery for positioning and chrome but
//! is NOT a menu: it never becomes the keyboard's owner, so it does not
//! record `RowAction` slots and does not appear in `modal_nav_surface`.
//! Selection is a local index in the overlay content, driven by
//! `handle_password_suggest_key`, because the surface underneath is a
//! live PTY and every key not aimed at the popup has to reach it.

use super::*;
use crate::state::PasswordSource;
use iced::widget::column;

/// Vertical metrics of the popup. The builder draws by these and
/// [`password_suggest_menu_height`] sums them, so the height the layout
/// places the box by cannot drift from the box that gets painted.
const TITLE_SIZE: f32 = 11.0;
const LABEL_SIZE: f32 = 12.0;
const SUBLABEL_SIZE: f32 = 10.0;
const HINT_SIZE: f32 = 10.0;
/// The key glyph, which sets the row's floor when the credential has no
/// username to print under its label.
const ICON_SIZE: f32 = 14.0;
const TITLE_PAD: Padding = Padding {
    top: 2.0,
    right: 12.0,
    bottom: 4.0,
    left: 12.0,
};
const ROW_PAD: Padding = Padding {
    top: 6.0,
    right: 12.0,
    bottom: 6.0,
    left: 12.0,
};
const HINT_PAD: Padding = Padding {
    top: 4.0,
    right: 12.0,
    bottom: 2.0,
    left: 12.0,
};
const COLUMN_SPACING: f32 = 2.0;

/// Drawn height of one text line. iced's default line height is 1.3x
/// the font size, which the harness measures back exactly (size 12
/// reports 16 px, size 10 reports 13).
fn line_height(size: f32) -> f32 {
    size * 1.3
}

/// On-screen height of the whole popup, chrome included.
///
/// Measured instead of estimated, because two placements ride on it:
/// the clamp that keeps the box inside the window, and the flip over
/// the caret. The generic per-item guess in `overlay_menu_height` has
/// no way to express a row that is two lines tall only when the
/// credential carries a username, and it came out 26 px short on a
/// one-row popup, which is exactly the hint line plus the bottom
/// padding that a prompt on the last terminal row rendered off the
/// bottom edge of the window.
pub(super) fn password_suggest_menu_height(entries: &[PasswordSource]) -> f32 {
    let rows: f32 = entries
        .iter()
        .map(|e| {
            let labels = if e.sublabel.is_empty() {
                line_height(LABEL_SIZE)
            } else {
                line_height(LABEL_SIZE) + line_height(SUBLABEL_SIZE)
            };
            ROW_PAD.top + labels.max(line_height(ICON_SIZE)) + ROW_PAD.bottom
        })
        .sum();
    let title = TITLE_PAD.top + line_height(TITLE_SIZE) + TITLE_PAD.bottom;
    let hint = HINT_PAD.top + line_height(HINT_SIZE) + HINT_PAD.bottom;
    // One gap per sibling pair in the column: title, the rows, hint.
    let gaps = entries.len() as f32 * COLUMN_SPACING + COLUMN_SPACING;
    2.0 * super::menus::MENU_CHROME_PAD_V + title + rows + hint + gaps
}

impl Oryxis {
    /// Borrows nothing: every label is cloned into an owned widget, so
    /// the returned element outlives both `self` and the overlay it was
    /// built from (which are two different lifetimes at the call site).
    pub(crate) fn build_menu_password_suggest(
        &self,
        entries: &[crate::state::PasswordSource],
        selected: Option<usize>,
    ) -> Element<'static, Message> {
        let mut col = column![
            container(
                text(crate::i18n::t("password_suggest_title"))
                    .size(TITLE_SIZE)
                    .color(OryxisColors::t().text_muted)
            )
            .padding(TITLE_PAD)
            .width(Length::Fill)
            .align_x(dir_align_x()),
        ]
        .spacing(COLUMN_SPACING);

        for (idx, entry) in entries.iter().enumerate() {
            let is_selected = selected == Some(idx);
            let mut labels = column![text(entry.label.clone())
                .size(LABEL_SIZE)
                .color(OryxisColors::t().text_primary)];
            if !entry.sublabel.is_empty() {
                labels = labels.push(
                    text(entry.sublabel.clone())
                        .size(SUBLABEL_SIZE)
                        .color(OryxisColors::t().text_muted),
                );
            }
            let row: Element<'_, Message> = button(
                container(
                    dir_row(vec![
                        iced_fonts::lucide::key_round()
                            .size(ICON_SIZE)
                            .color(if is_selected {
                                OryxisColors::t().accent
                            } else {
                                OryxisColors::t().text_secondary
                            })
                            .into(),
                        Space::new().width(8).into(),
                        labels.into(),
                    ])
                    .align_y(iced::Alignment::Center),
                )
                .width(Length::Fill)
                .align_x(dir_align_x()),
            )
            // The row's own index, not the selection: iced fires
            // `on_enter` off a cursor MOVE, so a popup that opens under
            // a stationary pointer (the caret is exactly where the user
            // last clicked) never sees a hover, and a pick that read
            // the selection back would silently do nothing.
            .on_press(Message::Terminal(TerminalMessage::PasswordSuggestPick(idx)))
            .width(Length::Fill)
            .padding(ROW_PAD)
            .style(move |_, status| {
                // The keyboard selection reads like a hover, so the two
                // input paths look the same to the user.
                let bg = if is_selected
                    || matches!(
                        status,
                        iced::widget::button::Status::Hovered
                            | iced::widget::button::Status::Pressed
                    ) {
                    OryxisColors::t().bg_hover
                } else {
                    Color::TRANSPARENT
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        radius: Radius::from(4.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .into();
            // No hover wiring on purpose. The button's own `Hovered`
            // status already highlights the row under the cursor, and
            // hover must never reach `selected`: that would arm Enter,
            // and the popup opens at the caret, right where the pointer
            // usually sits. `selected` is the KEYBOARD's, exclusively.
            col = col.push(row);
        }

        col.push(
            container(
                text(crate::i18n::t("password_suggest_hint"))
                    .size(HINT_SIZE)
                    .color(OryxisColors::t().text_muted),
            )
            .padding(HINT_PAD)
            .width(Length::Fill)
            .align_x(dir_align_x()),
        )
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PasswordSourceKind;

    fn entry(sublabel: &str) -> PasswordSource {
        PasswordSource {
            label: "ops-admin".into(),
            sublabel: sublabel.into(),
            kind: PasswordSourceKind::Identity(uuid::Uuid::nil()),
        }
    }

    #[test]
    fn one_credential_with_a_username_measures_what_the_harness_drew() {
        // Measured through the harness at 1200x750: chrome 6+6, title
        // 2+14+4, row 6+16+13+6, hint 4+13+2, three 2 px gaps. The old
        // per-item estimate answered 70 for this popup, and the 26 px
        // it was short is exactly the hint line and the bottom padding
        // that rendered off the window at a prompt on the last row.
        let h = password_suggest_menu_height(&[entry("wilson")]);
        assert!(
            (h - 96.0).abs() < 1.0,
            "expected ~96 px for a one-line-subtitle row, got {h}"
        );
    }

    #[test]
    fn a_credential_without_a_username_is_a_shorter_row() {
        // A host saved with no username prints no subtitle, so a flat
        // per-entry constant would be wrong in both directions.
        let with = password_suggest_menu_height(&[entry("wilson")]);
        let without = password_suggest_menu_height(&[entry("")]);
        let delta = with - without;
        // It does not shrink by the whole subtitle line: with only one
        // label left the row bottoms out on the 14 px key glyph, which
        // is taller than the 12 px label beside it.
        assert!(delta > 0.0 && delta < line_height(SUBLABEL_SIZE));
        let floor = line_height(LABEL_SIZE) + line_height(SUBLABEL_SIZE) - line_height(ICON_SIZE);
        assert!((delta - floor).abs() < 0.01, "got {delta}, expected {floor}");
    }

    #[test]
    fn every_extra_credential_adds_its_own_row() {
        let one = password_suggest_menu_height(&[entry("wilson")]);
        let two = password_suggest_menu_height(&[entry("wilson"), entry("root")]);
        assert!((two - one - (41.0 + COLUMN_SPACING)).abs() < 1.0);
    }
}
