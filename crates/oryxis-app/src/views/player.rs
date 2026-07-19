//! In-app session player surface (issue #71), rendered by
//! `view_history` while `Oryxis.session_player` is `Some`: the
//! recording replays through the regular terminal widget pinned to its
//! recorded geometry, under a transport bar (play/pause, restart,
//! scrubber, speed). Read-only by construction: the backend has no PTY
//! and no input callback is wired.

use std::sync::Arc;

use iced::border::Radius;
use iced::widget::button::Status as BtnStatus;
use iced::widget::{button, canvas, column, container, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

use oryxis_terminal::widget::TerminalView;

use crate::app::{PlayerMessage, Message, Oryxis};
use crate::state::SessionPlayer;
use crate::theme::OryxisColors;

impl Oryxis {
    pub(crate) fn view_session_player(&self, p: &SessionPlayer) -> Element<'_, Message> {
        // Privacy Mode resolves like the static viewer: the recording's
        // host override wins, a deleted host falls back to the global
        // default; the toolbar Reveal toggle lifts the masking.
        let conn = self
            .session_logs
            .iter()
            .find(|e| e.id == p.log_id)
            .and_then(|e| self.connections.iter().find(|c| c.id == e.connection_id));
        let privacy_applies = conn
            .map(|c| self.privacy_active(c))
            .unwrap_or_else(|| self.privacy_global_active());
        let mask = privacy_applies && !self.privacy.revealed;

        // ── Header: title, geometry chip, reveal, close ──
        let title = if mask {
            crate::widgets::mask_blocks(&p.label)
        } else {
            p.label.clone()
        };
        let mut header_items: Vec<Element<'_, Message>> = vec![
            text(title)
                .size(16)
                .color(OryxisColors::t().text_primary)
                .into(),
            Space::new().width(10).into(),
            text(format!("{}x{}", p.cols, p.rows))
                .size(11)
                .color(OryxisColors::t().text_muted)
                .into(),
            Space::new().width(Length::Fill).into(),
        ];
        if privacy_applies {
            header_items.push(crate::widgets::privacy_reveal_btn(self.privacy.revealed));
            header_items.push(Space::new().width(8).into());
        }
        // Recording actions, mirroring the static viewer's header: a
        // "View log" button back to the log-only surface plus the same
        // `...` menu (exports + delete). Resolved by index like the
        // viewer does; a row deleted underneath the player simply
        // drops the affordances.
        if let Some(idx) = self.session_logs.iter().position(|e| e.id == p.log_id) {
            header_items.push(super::history::viewer_header_btn(
                iced_fonts::lucide::file_text()
                    .size(11)
                    .color(OryxisColors::t().text_secondary)
                    .into(),
                Some(crate::i18n::t("player_view_log")),
                Message::ViewSessionLog(p.log_id),
            ));
            header_items.push(Space::new().width(8).into());
            let menu_open = matches!(
                self.overlay.as_ref().map(|o| &o.content),
                Some(crate::state::OverlayContent::SessionLogViewerActions(i)) if *i == idx
            );
            header_items.push(crate::views::terminal::icon_tooltip(
                super::history::viewer_header_btn(
                    // Same glyph the card kebabs draw.
                    text("\u{22EE}")
                        .size(13)
                        .color(if menu_open {
                            OryxisColors::t().text_primary
                        } else {
                            OryxisColors::t().text_muted
                        })
                        .into(),
                    None,
                    Message::ShowSessionLogViewerMenu(idx),
                ),
                crate::i18n::t("more_actions"),
            ));
            header_items.push(Space::new().width(8).into());
        }
        header_items.push(
            button(
                container(
                    text(crate::i18n::t("close")).size(11).font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                    }).color(OryxisColors::t().text_muted),
                )
                .center_y(Length::Fixed(24.0))
                .padding(Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 14.0 }),
            )
            .on_press(Message::Player(PlayerMessage::Close))
            .style(|_, status| {
                let bg = match status {
                    BtnStatus::Hovered => Color { a: 0.15, ..OryxisColors::t().error },
                    _ => Color::TRANSPARENT,
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        radius: Radius::from(6.0),
                        color: OryxisColors::t().border,
                        width: 1.0,
                    },
                    ..Default::default()
                }
            })
            .into(),
        );
        let header = container(
            crate::widgets::dir_row(header_items).align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 16.0, right: 20.0, bottom: 12.0, left: 20.0 });

        // ── Terminal canvas, pinned to the recording's geometry ──
        // The grid is fixed (the recorded resize events drive it), so
        // the canvas gets exactly the pixel size that grid needs and
        // scrolls inside the surface when it doesn't fit.
        let (px_w, px_h) = oryxis_terminal::widget::grid_pixel_size(
            &self.terminal_font_name,
            self.terminal_font_size,
            p.cols,
            p.rows,
        );
        let term_view = TerminalView::new(Arc::clone(&p.terminal))
            .with_fixed_grid(true)
            // No mouse-tracking reports ever leave a replay (there is
            // nothing to receive them); selection/copy stay local.
            .focused(false)
            .with_mouse_reporting(false)
            .with_font_size(self.terminal_font_size)
            .with_font_name(&self.terminal_font_name)
            .with_copy_on_select(self.setting_copy_on_select)
            .with_right_click_copy(self.setting_right_click_copy)
            .with_bold_is_bright(self.setting_bold_is_bright)
            .with_keyword_highlight(self.setting_keyword_highlight)
            .with_performance(self.setting_performance_mode)
            .with_privacy(mask)
            .with_privacy_terms(&self.privacy_terms())
            .with_privacy_classes(self.privacy_classes())
            .with_smart_contrast(self.setting_smart_contrast)
            .with_word_delimiters(&self.setting_word_delimiters);
        let term_bg = {
            // The palette was resolved at open; read it off the state
            // like the link chip does (non-blocking; a missed frame
            // just paints the previous background).
            p.terminal
                .try_lock()
                .map(|s| s.palette.background)
                .unwrap_or(OryxisColors::t().bg_primary)
        };
        let term_canvas = canvas(term_view)
            .width(Length::Fixed(px_w))
            .height(Length::Fixed(px_h));
        let stage = scrollable(
            container(term_canvas)
                .padding(16)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center),
        )
        .direction(scrollable::Direction::Both {
            vertical: scrollable::Scrollbar::default(),
            horizontal: scrollable::Scrollbar::default(),
        })
        .height(Length::Fill);
        let stage = container(stage)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(term_bg)),
                ..Default::default()
            });

        // ── Transport bar ──
        let (play_icon, play_tip) = if p.playing {
            (iced_fonts::lucide::pause(), crate::i18n::t("player_pause_tip"))
        } else {
            (iced_fonts::lucide::play(), crate::i18n::t("player_play_tip"))
        };
        let play_btn = transport_btn(play_icon, play_tip, Message::Player(PlayerMessage::TogglePlay));
        let restart_btn = transport_btn(
            iced_fonts::lucide::rotate_ccw(),
            crate::i18n::t("player_restart_tip"),
            Message::Player(PlayerMessage::Restart),
        );
        // The knob and label follow the live scrub target while dragging,
        // the playback clock otherwise.
        let display_ms = p.display_ms();
        let time_label = format!(
            "{} / {}",
            format_clock(display_ms as i64),
            format_clock(p.duration_ms),
        );
        // Scrubber over the full timeline in milliseconds. Dragging only
        // records the target (cheap); the one rebuild/replay a backward
        // jump needs happens on release, not per per-ms slider event.
        let scrubber = iced::widget::slider(
            0.0..=(p.duration_ms.max(1) as f64),
            display_ms.clamp(0.0, p.duration_ms as f64),
            |v| Message::Player(PlayerMessage::Scrub(v)),
        )
        .on_release(Message::Player(PlayerMessage::ScrubCommit))
        .step(1.0)
        .width(Length::Fill);
        // Speed chip: cycles the preset steps; trailing x reads the
        // same in every locale.
        let speed_label = if (p.speed.fract()).abs() < f32::EPSILON {
            format!("{}x", p.speed as u32)
        } else {
            format!("{}x", p.speed)
        };
        let speed_btn = crate::views::terminal::icon_tooltip(
            button(
                container(
                    text(speed_label)
                        .size(11)
                        .font(iced::Font {
                            weight: iced::font::Weight::Bold,
                            ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                        })
                        .color(OryxisColors::t().text_secondary),
                )
                .center_y(Length::Fixed(24.0))
                .center_x(Length::Fixed(40.0)),
            )
            .on_press(Message::Player(PlayerMessage::SpeedCycle))
            .style(|_, status| transport_style(status))
            .into(),
            crate::i18n::t("player_speed_tip"),
        );
        let controls = container(
            crate::widgets::dir_row(vec![
                play_btn,
                Space::new().width(4).into(),
                restart_btn,
                Space::new().width(12).into(),
                text(time_label)
                    .size(11)
                    .color(OryxisColors::t().text_muted)
                    .into(),
                Space::new().width(12).into(),
                scrubber.into(),
                Space::new().width(12).into(),
                speed_btn,
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 10.0, right: 20.0, bottom: 12.0, left: 20.0 })
        .width(Length::Fill);

        container(column![header, stage, controls])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_primary)),
                ..Default::default()
            })
            .into()
    }
}

/// Shared style for the transport-bar icon buttons: transparent at
/// rest, `bg_hover` fill on hover, accent tint on press (the app-wide
/// button-feedback convention).
fn transport_style(status: BtnStatus) -> button::Style {
    let bg = match status {
        BtnStatus::Hovered => OryxisColors::t().bg_hover,
        BtnStatus::Pressed => Color { a: 0.25, ..OryxisColors::t().accent },
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            radius: Radius::from(6.0),
            color: OryxisColors::t().border,
            width: 1.0,
        },
        ..Default::default()
    }
}

/// One transport icon button with its tooltip.
fn transport_btn<'a>(
    icon: iced::widget::Text<'a>,
    tip: &'a str,
    msg: Message,
) -> Element<'a, Message> {
    crate::views::terminal::icon_tooltip(
        button(
            container(icon.size(14).color(OryxisColors::t().text_secondary))
                .center(Length::Fixed(28.0)),
        )
        .on_press(msg)
        .style(|_, status| transport_style(status))
        .into(),
        tip,
    )
}

/// `m:ss` (or `h:mm:ss` past the hour) for the transport clock.
fn format_clock(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_clock;

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(format_clock(0), "0:00");
        assert_eq!(format_clock(59_999), "0:59");
        assert_eq!(format_clock(65_000), "1:05");
        assert_eq!(format_clock(3_600_000), "1:00:00");
        assert_eq!(format_clock(-5), "0:00");
    }
}
