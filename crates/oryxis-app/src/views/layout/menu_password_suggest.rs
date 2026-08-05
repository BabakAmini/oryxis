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
use iced::widget::column;

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
                    .size(11)
                    .color(OryxisColors::t().text_muted)
            )
            .padding(Padding {
                top: 2.0,
                right: 12.0,
                bottom: 4.0,
                left: 12.0,
            })
            .width(Length::Fill)
            .align_x(dir_align_x()),
        ]
        .spacing(2);

        for (idx, entry) in entries.iter().enumerate() {
            let is_selected = selected == Some(idx);
            let mut labels = column![text(entry.label.clone())
                .size(12)
                .color(OryxisColors::t().text_primary)];
            if !entry.sublabel.is_empty() {
                labels = labels.push(
                    text(entry.sublabel.clone())
                        .size(10)
                        .color(OryxisColors::t().text_muted),
                );
            }
            let row: Element<'_, Message> = button(
                container(
                    dir_row(vec![
                        iced_fonts::lucide::key_round()
                            .size(14)
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
            .padding(Padding {
                top: 6.0,
                right: 12.0,
                bottom: 6.0,
                left: 12.0,
            })
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
            // Hover moves the selection so the mouse and the keyboard
            // agree on what is highlighted. It is presentation only:
            // the pick carries its own row.
            col = col.push(
                MouseArea::new(row)
                    .on_enter(Message::Terminal(TerminalMessage::PasswordSuggestHover(idx))),
            );
        }

        col.push(
            container(
                text(crate::i18n::t("password_suggest_hint"))
                    .size(10)
                    .color(OryxisColors::t().text_muted),
            )
            .padding(Padding {
                top: 4.0,
                right: 12.0,
                bottom: 2.0,
                left: 12.0,
            })
            .width(Length::Fill)
            .align_x(dir_align_x()),
        )
        .into()
    }
}
