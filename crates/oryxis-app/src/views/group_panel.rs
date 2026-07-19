//! Manual host-group editor side panel: label + icon + color. Rendered
//! in the same right-hand slot as the host / session-group editors (from
//! `view_main::active_side_panel` when `group_edit.visible`). Folders had
//! a rename-only modal before; this surfaces the icon/color override the
//! `Group` model already carries.

use iced::border::Radius;
use iced::widget::{button, column, container, scrollable, text, text_input, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::app::{TabsMessage, Message, Oryxis};
use crate::os_icon::BrandIcon;
use crate::theme::OryxisColors;
use crate::widgets::{dir_align_x, dir_row, panel_field, panel_section};

impl Oryxis {
    pub(crate) fn view_group_edit_panel(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order (row mode: Up/Down from any input).
        self.panel_nav_reset();

        // ── Header ──
        // The close (×) is not a keyboard row: Esc already owns panel
        // close, and recording it would make the header the first Down
        // target instead of the form.
        let panel_header = container(
            dir_row(vec![
                text(crate::i18n::t("edit_group"))
                    .size(16)
                    .color(OryxisColors::t().text_primary)
                    .into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{00D7}").size(20).color(OryxisColors::t().text_muted))
                    .on_press(Message::Tabs(TabsMessage::CancelGroupEdit))
                    .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                    .style(|_, _| button::Style {
                        background: Some(Background::Color(Color::TRANSPARENT)),
                        border: Border::default(),
                        ..Default::default()
                    })
                    .into(),
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 12.0, right: 16.0, bottom: 12.0, left: 16.0 });

        // Icon + color badge. Clicking opens the shared icon/color picker,
        // seeded from the in-memory form (deferred save).
        let badge_bg = crate::os_icon::parse_hex_color(&self.group_edit.color)
            .unwrap_or_else(|| OryxisColors::t().accent);
        let badge_glyph = if self.group_edit.icon.is_empty() {
            BrandIcon::Glyph(iced_fonts::lucide::boxes())
        } else {
            crate::os_icon::custom_icon_glyph(&self.group_edit.icon)
        };
        let icon_badge = button(
            container(badge_glyph.view(18.0, Color::WHITE))
                .center_x(Length::Fixed(36.0))
                .center_y(Length::Fixed(36.0)),
        )
        .on_press(Message::Tabs(TabsMessage::ShowGroupEditIconPicker))
        .padding(0)
        .style(move |_, _| button::Style {
            background: Some(Background::Color(badge_bg)),
            border: Border { radius: Radius::from(8.0), ..Default::default() },
            ..Default::default()
        });

        // ── Section: General ──
        let general_section = panel_section(column![
            panel_field(
                crate::i18n::t("name"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("group-edit-name")),
                    10.0,
                    text_input(crate::i18n::t("group_placeholder"), &self.group_edit.label)
                        .id(iced::widget::Id::new("group-edit-name"))
                        .on_input(|v| Message::Tabs(TabsMessage::GroupEditLabelChanged(v)))
                        .on_submit(Message::Tabs(TabsMessage::SaveGroupEdit))
                        .padding(10)
                        .style(crate::widgets::rounded_input_style)
                        .align_x(dir_align_x())
                        .into(),
                ),
            ),
            Space::new().height(10),
            panel_field(
                crate::i18n::t("group_icon_color"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::activate(Message::Tabs(TabsMessage::ShowGroupEditIconPicker)),
                    8.0,
                    icon_badge.into(),
                ),
            ),
        ]);

        let form_scroll = scrollable(
            container(general_section).padding(Padding {
                top: 0.0,
                right: 16.0,
                bottom: 16.0,
                left: 16.0,
            }),
        )
        // Shared id: the keyboard router keeps the selected row in view.
        .id(iced::widget::Id::new("side-panel-scroll"))
        .height(Length::Fill);

        // Full-width Save, standardized with the host editor's footer
        // (the header × acts as Cancel).
        let save_btn = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::Tabs(TabsMessage::SaveGroupEdit)),
            8.0,
            button(
                container(text(crate::i18n::t("save")).size(14).color(OryxisColors::t().text_primary))
                    .padding(Padding { top: 12.0, right: 0.0, bottom: 12.0, left: 0.0 })
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .on_press(Message::Tabs(TabsMessage::SaveGroupEdit))
            .width(Length::Fill)
            .style(|_, _| button::Style {
                background: Some(Background::Color(OryxisColors::t().accent)),
                border: Border { radius: Radius::from(8.0), ..Default::default() },
                ..Default::default()
            })
            .into(),
        );

        let footer = container(save_btn)
            .padding(Padding { top: 8.0, right: 16.0, bottom: 16.0, left: 16.0 });

        let panel_content = column![panel_header, form_scroll, footer].height(Length::Fill);

        crate::widgets::side_panel_frame(panel_content.into(), OryxisColors::t().bg_surface)
    }
}
