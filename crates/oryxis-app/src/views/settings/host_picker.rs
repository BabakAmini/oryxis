//! Host badge helper + the "Select a host" modal for the SFTP-sync
//! backup host. Split out of views/settings/mod.rs.

use super::*;
use iced::widget::column;

/// OS-icon avatar for a host, matching the dashboard card and the SFTP
/// file-browser picker. Output lifetime is tied to `conn` (the glyph and
/// label borrow it); `default_icon` only feeds the owned style lookup.
pub(crate) fn host_badge<'a>(
    conn: &'a oryxis_core::models::connection::Connection,
    default_icon: &str,
    size: f32,
) -> Element<'a, Message> {
    let (glyph, default_color) =
        crate::os_icon::resolve_icon(conn.detected_os.as_deref(), OryxisColors::t().accent);
    let badge_style =
        crate::widgets::resolve_host_icon_style(conn.icon_style.as_deref(), default_icon);
    let badge_color = conn
        .custom_color
        .as_deref()
        .or(conn.color.as_deref())
        .and_then(crate::widgets::parse_hex_color)
        .unwrap_or(default_color);
    let glyph_el: Element<'a, Message> = glyph.view(size * 0.58, Color::WHITE);
    crate::widgets::host_icon(badge_style, badge_color, &conn.label, Some(glyph_el), size)
}

/// The "Select a host" modal for the SFTP-sync backup host. Mirrors the
/// SFTP file-browser picker: a searchable list of saved hosts, each row an
/// OS badge + label + address. Rendered as a dimming scrim plus a centered
/// dialog; the caller stacks it over the settings page.
pub(super) fn sync_host_picker_modal(app: &Oryxis) -> Element<'_, Message> {
    let q = app.sync.sftp.picker_search.to_lowercase();
    let mut list = column![].spacing(2);
    for conn in app.connections.iter().filter(|c| {
        q.is_empty()
            || c.label.to_lowercase().contains(&q)
            || c.hostname.to_lowercase().contains(&q)
    }) {
        let badge = host_badge(conn, &app.setting_default_host_icon, 24.0);
        let row_btn = button(
            dir_row(vec![
                badge,
                Space::new().width(10).into(),
                column![
                    text(conn.label.clone())
                        .size(13)
                        .color(OryxisColors::t().text_primary),
                    text(conn.hostname.clone())
                        .size(10)
                        .color(OryxisColors::t().text_muted),
                ]
                .width(Length::Fill)
                .align_x(dir_align_x())
                .into(),
            ])
            .align_y(iced::Alignment::Center),
        )
        .on_press(Message::Sync(SyncMessage::SftpHostChanged(conn.id)))
        .padding(Padding { top: 8.0, right: 12.0, bottom: 8.0, left: 12.0 })
        .width(Length::Fill)
        .style(|_, status| {
            let bg = match status {
                BtnStatus::Hovered => OryxisColors::t().bg_hover,
                _ => Color::TRANSPARENT,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
        list = list.push(row_btn);
    }

    let dialog = container(
        column![
            dir_row(vec![
                text(t("select_a_host"))
                    .size(15)
                    .color(OryxisColors::t().text_primary)
                    .into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{2715}").size(13).color(OryxisColors::t().text_muted))
                    .on_press(Message::Sync(SyncMessage::SftpClosePicker))
                    .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                    .style(|_, status| {
                        let bg = match status {
                            BtnStatus::Hovered => OryxisColors::t().bg_hover,
                            _ => Color::TRANSPARENT,
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
                    .into(),
            ])
            .align_y(iced::Alignment::Center)
            .width(Length::Fill),
            Space::new().height(8),
            text_input(t("search_hosts"), &app.sync.sftp.picker_search)
                .on_input(|v| Message::Sync(SyncMessage::SftpPickerSearch(v)))
                .padding(10)
                .style(crate::widgets::rounded_input_style)
                .align_x(dir_align_x()),
            Space::new().height(8),
            scrollable(list).height(Length::Fixed(360.0)),
        ]
        .padding(20)
        .width(Length::Fixed(440.0))
        .align_x(dir_align_x()),
    )
    .style(|_| container::Style {
        background: Some(Background::Color(OryxisColors::t().bg_surface)),
        border: Border {
            radius: Radius::from(12.0),
            color: OryxisColors::t().border,
            width: 1.0,
        },
        ..Default::default()
    });

    let scrim: Element<'_, Message> = iced::widget::opaque(
        iced::widget::MouseArea::new(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                    ..Default::default()
                }),
        )
        .on_press(Message::Sync(SyncMessage::SftpClosePicker)),
    );

    let centered = container(iced::widget::MouseArea::new(dialog).on_press(Message::NoOp))
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    iced::widget::Stack::new()
        .push(scrim)
        .push(centered)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
