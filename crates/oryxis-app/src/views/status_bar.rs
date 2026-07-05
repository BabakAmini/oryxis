//! Bottom status bar, connection state, keepalive info, and host summary.

use iced::border::Radius;
use iced::widget::button::Status as BtnStatus;
use iced::widget::{button, column, container, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::app::{Message, Oryxis};
use crate::theme::OryxisColors;

impl Oryxis {
    pub(crate) fn view_status_bar(&self) -> Element<'_, Message> {
        let status_text = if let Some(idx) = self.active_tab {
            if let Some(tab) = self.tabs.get(idx) {
                format!("● {}, connected", tab.label)
            } else {
                crate::i18n::t("no_active_connection").into()
            }
        } else {
            crate::i18n::t("no_active_connection").into()
        };

        let status_color = if self.active_tab.is_some() {
            OryxisColors::t().success
        } else {
            OryxisColors::t().text_muted
        };

        // 1 px hairline on top only, iced's Border has a single width that
        // applies to all four sides, so a dedicated separator widget is the
        // way to keep just the top edge.
        let top_hairline = container(Space::new().height(1))
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().border)),
                ..Default::default()
            });

        let mut items: Vec<Element<'_, Message>> = vec![
            text(status_text).size(12).color(status_color).into(),
            Space::new().width(Length::Fill).into(),
        ];
        // Hybrid tab mode segment (issue #61): redundant with the tab's
        // own glyph on purpose, the status bar is optional
        // (`show_status_bar`), so it can carry a switch but never THE
        // switch. Shown only for a live SSH tab (or one already in
        // Files mode, so the way back stays visible).
        if let Some(idx) = self.active_tab
            && let Some(tab) = self.tabs.get(idx)
            && (tab.files_mode
                || (self.sftp_enabled
                    && tab.active().session.as_ref().and_then(|s| s.ssh()).is_some()))
        {
            items.push(mode_segment_btn(
                idx,
                crate::i18n::t("tab_mode_terminal"),
                !tab.files_mode,
            ));
            items.push(Space::new().width(2).into());
            items.push(mode_segment_btn(
                idx,
                crate::i18n::t("tab_mode_files"),
                tab.files_mode,
            ));
            items.push(Space::new().width(10).into());
        }
        items.push(
            text(concat!("Oryxis v", env!("CARGO_PKG_VERSION")))
                .size(12)
                .color(OryxisColors::t().text_muted)
                .into(),
        );
        let bar = container(
            crate::widgets::dir_row(items)
                .align_y(iced::Alignment::Center)
                .padding(Padding { top: 3.0, right: 12.0, bottom: 3.0, left: 12.0 }),
        )
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(OryxisColors::t().bg_sidebar)),
            ..Default::default()
        });

        column![top_hairline, bar].into()
    }
}

/// One half of the status-bar Terminal/Files segment. The active half
/// is an accent-tinted indicator; the inactive half is the clickable
/// action (clicking the active one would be a no-op, so it gets no
/// `on_press` and no misleading hover state).
fn mode_segment_btn(idx: usize, label: &str, active: bool) -> Element<'_, Message> {
    let c = OryxisColors::t();
    let fg = if active { c.accent } else { c.text_muted };
    let mut btn = button(text(label).size(11).color(fg))
        .padding(Padding { top: 1.0, right: 8.0, bottom: 1.0, left: 8.0 })
        .style(move |_, status| {
            let c = OryxisColors::t();
            let bg = if active {
                Color { a: 0.12, ..c.accent }
            } else {
                match status {
                    BtnStatus::Hovered | BtnStatus::Pressed => c.bg_hover,
                    _ => Color::TRANSPARENT,
                }
            };
            let border_color = if active { Color { a: 0.35, ..c.accent } } else { Color::TRANSPARENT };
            button::Style {
                background: Some(Background::Color(bg)),
                border: Border { radius: Radius::from(5.0), color: border_color, width: 1.0 },
                ..Default::default()
            }
        });
    if !active {
        btn = btn.on_press(Message::ToggleTabFilesMode(idx));
    }
    btn.into()
}
