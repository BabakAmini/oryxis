//! Monitor sidebar tab: agentless host vitals for the focused pane's
//! session (issue #83). CPU / memory / load / network / disks read from
//! `/proc` over the live SSH handle, rendered as compact gauges.
//!
//! Everything here is informational, so the only keyboard row is the
//! opt-in button shown while the host hasn't enabled monitoring.

use iced::border::Radius;
use iced::widget::{column, container, text, Space};
use iced::{Background, Border, Element, Length, Padding};

use crate::app::{Message, MonitorMessage, Oryxis};
use crate::i18n::t;
use crate::state::TerminalSidebarTab;
use crate::theme::OryxisColors;
use crate::widgets::dir_row;

impl Oryxis {
    pub(crate) fn monitor_tab_content(&self) -> Element<'_, Message> {
        // Disconnected mid-view (the tab button hides next frame).
        let Some(idx) = self.active_tab else {
            return placeholder(t("files_no_session"));
        };
        let Some(tab) = self.tabs.get(idx) else {
            return placeholder(t("files_no_session"));
        };
        if tab.active().session.as_ref().and_then(|s| s.ssh()).is_none() {
            return placeholder(t("files_no_session"));
        }

        // Only saved hosts carry the opt-in flag: a quick-connect, local
        // or cloud pane has no vault row to enable it on.
        let Some(conn_id) = self.monitor_pane_connection() else {
            return placeholder(t("monitor_requires_host"));
        };
        let enabled = self
            .connections
            .iter()
            .any(|c| c.id == conn_id && c.monitor_enabled);
        if !enabled {
            return self.monitor_opt_in(conn_id);
        }

        let Some(sample) = self
            .monitor
            .series
            .get(&conn_id)
            .and_then(|s| s.latest())
        else {
            // Probing, or the first probe failed.
            return match &self.monitor_error {
                Some(e) => placeholder(e),
                None => placeholder(t("monitor_sampling")),
            };
        };
        let spark = self
            .monitor
            .series
            .get(&conn_id)
            .map(|s| s.cpu_series())
            .unwrap_or_default();

        let mut body = column![].spacing(14).padding(Padding {
            top: 12.0,
            right: 12.0,
            bottom: 12.0,
            left: 12.0,
        });

        // CPU: percentage bar plus a sparkline over the window. The
        // first sample after mount has no percentage (it is a delta), so
        // the gauge says "sampling" rather than showing a fake zero.
        body = body.push(match sample.cpu {
            Some(cpu) => gauge_block(t("monitor_cpu"), cpu.pct, &format!("{:.0}%", cpu.pct)),
            None => pending_block(t("monitor_cpu")),
        });
        if spark.len() > 1 {
            body = body.push(sparkline(&spark));
        }

        if let Some(mem) = sample.mem {
            body = body.push(gauge_block(
                t("monitor_mem"),
                mem.pct(),
                &format!("{} / {}", fmt_bytes(mem.used), fmt_bytes(mem.total)),
            ));
            if mem.swap_total > 0 {
                let pct = (mem.swap_used as f32 / mem.swap_total as f32) * 100.0;
                body = body.push(gauge_block(
                    t("monitor_swap"),
                    pct,
                    &format!("{} / {}", fmt_bytes(mem.swap_used), fmt_bytes(mem.swap_total)),
                ));
            }
        }

        if let Some(load) = sample.load {
            body = body.push(stat_row(
                t("monitor_load"),
                format!("{:.2}  {:.2}  {:.2}", load.one, load.five, load.fifteen),
            ));
            if load.procs_total > 0 {
                body = body.push(stat_row(
                    t("monitor_procs"),
                    format!("{} / {}", load.procs_running, load.procs_total),
                ));
            }
        }

        if let Some(net) = sample.net {
            body = body.push(stat_row(
                t("monitor_net"),
                format!(
                    "↓ {}/s   ↑ {}/s",
                    fmt_bytes(net.rx_bps),
                    fmt_bytes(net.tx_bps)
                ),
            ));
        }

        if let Some(up) = sample.uptime_secs {
            body = body.push(stat_row(t("monitor_uptime"), fmt_uptime(up)));
        }

        if !sample.disks.is_empty() {
            body = body.push(
                text(t("monitor_disk"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            );
            for disk in &sample.disks {
                body = body.push(gauge_block(
                    &disk.mount,
                    disk.pct(),
                    &format!("{} / {}", fmt_bytes(disk.used), fmt_bytes(disk.total)),
                ));
            }
        }

        // A probe that failed after we already have data: keep the last
        // reading on screen and say so, rather than blanking the tab.
        if let Some(e) = &self.monitor_error {
            body = body.push(
                text(e.clone())
                    .size(11)
                    .color(OryxisColors::t().warning),
            );
        }

        iced::widget::scrollable(body)
            .id(iced::widget::Id::new("sidebar-list-scroll"))
            .height(Length::Fill)
            .into()
    }

    /// Opt-in prompt for a host that hasn't enabled monitoring. The
    /// button is the tab's only keyboard row.
    fn monitor_opt_in(&self, conn_id: uuid::Uuid) -> Element<'_, Message> {
        let btn = crate::widgets::styled_button(
            t("monitor_enable_host"),
            Message::Monitor(MonitorMessage::EnableHost(conn_id)),
            OryxisColors::t().accent,
        );
        column![
            container(
                text(t("monitor_opt_in_hint"))
                    .size(12)
                    .color(OryxisColors::t().text_muted)
            )
            .padding(Padding { top: 24.0, right: 14.0, bottom: 12.0, left: 14.0 }),
            container(self.sidebar_nav_slot(
                crate::keynav::SidebarRow::button(Message::Monitor(
                    MonitorMessage::EnableHost(conn_id),
                )),
                TerminalSidebarTab::Monitor,
                8.0,
                btn,
            ))
            .padding(Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 14.0 }),
        ]
        .width(Length::Fill)
        .into()
    }
}

fn placeholder(label: &str) -> Element<'_, Message> {
    container(text(label.to_string()).size(12).color(OryxisColors::t().text_muted))
        .center_x(Length::Fill)
        .padding(Padding { top: 40.0, right: 12.0, bottom: 0.0, left: 12.0 })
        .width(Length::Fill)
        .into()
}

/// Label + value line above a filled bar. The fill colour follows the
/// theme's semantic colours so a host in trouble reads as such at a
/// glance.
fn gauge_block<'a>(label: &'a str, pct: f32, value: &str) -> Element<'a, Message> {
    let pct = pct.clamp(0.0, 100.0);
    let fill = if pct >= 90.0 {
        OryxisColors::t().error
    } else if pct >= 75.0 {
        OryxisColors::t().warning
    } else {
        OryxisColors::t().accent
    };
    // FillPortion needs whole numbers; below 1% the bar renders empty,
    // which is the honest reading anyway.
    let filled = pct.round() as u16;
    let rest = 100u16.saturating_sub(filled);
    let bar = dir_row(vec![
        container(Space::new().height(6))
            .width(Length::FillPortion(filled))
            .style(move |_| container::Style {
                background: Some(Background::Color(fill)),
                border: Border { radius: Radius::from(3.0), ..Default::default() },
                ..Default::default()
            })
            .into(),
        container(Space::new().height(6))
            .width(Length::FillPortion(rest))
            .into(),
    ]);
    column![
        dir_row(vec![
            text(label.to_string())
                .size(11)
                .color(OryxisColors::t().text_secondary)
                .width(Length::Fill)
                .into(),
            text(value.to_string())
                .size(11)
                .color(OryxisColors::t().text_primary)
                .into(),
        ])
        .align_y(iced::Alignment::Center),
        Space::new().height(4),
        container(bar).width(Length::Fill).style(|_| container::Style {
            background: Some(Background::Color(OryxisColors::t().bg_surface)),
            border: Border { radius: Radius::from(3.0), ..Default::default() },
            ..Default::default()
        }),
    ]
    .width(Length::Fill)
    .into()
}

/// A metric that needs a second sample before it can be reported.
fn pending_block<'a>(label: &'a str) -> Element<'a, Message> {
    dir_row(vec![
        text(label.to_string())
            .size(11)
            .color(OryxisColors::t().text_secondary)
            .width(Length::Fill)
            .into(),
        text(t("monitor_sampling"))
            .size(11)
            .color(OryxisColors::t().text_muted)
            .into(),
    ])
    .align_y(iced::Alignment::Center)
    .into()
}

fn stat_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    dir_row(vec![
        text(label.to_string())
            .size(11)
            .color(OryxisColors::t().text_secondary)
            .width(Length::Fill)
            .into(),
        text(value)
            .size(11)
            .color(OryxisColors::t().text_primary)
            .font(iced::Font::MONOSPACE)
            .into(),
    ])
    .align_y(iced::Alignment::Center)
    .into()
}

/// CPU history as a row of bars, oldest to newest. A canvas would be
/// smoother but this reuses the gauge's vocabulary and costs nothing.
fn sparkline<'a>(series: &[f32]) -> Element<'a, Message> {
    // Only the tail fits the sidebar's width at a readable bar size.
    let tail = series.len().saturating_sub(40);
    let bars: Vec<Element<'a, Message>> = series[tail..]
        .iter()
        .map(|pct| {
            let h = (pct.clamp(0.0, 100.0) / 100.0 * 24.0).max(1.0);
            container(
                container(Space::new().width(Length::Fill).height(Length::Fixed(h)))
                    .style(|_| container::Style {
                        background: Some(Background::Color(OryxisColors::t().accent)),
                        border: Border { radius: Radius::from(1.0), ..Default::default() },
                        ..Default::default()
                    }),
            )
            .height(Length::Fixed(24.0))
            .width(Length::Fill)
            .align_y(iced::alignment::Vertical::Bottom)
            .into()
        })
        .collect();
    container(iced::widget::Row::with_children(bars).spacing(1))
        .width(Length::Fill)
        .height(Length::Fixed(24.0))
        .into()
}

/// `fmt_bytes` without the space, for the status bar where horizontal
/// room is scarce.
pub(crate) fn fmt_bytes_short(bytes: u64) -> String {
    fmt_bytes(bytes).replace(' ', "")
}

/// Human-readable byte count (1024-based, matching what `free` / `df`
/// report on the hosts these numbers come from).
fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Uptime as the coarsest useful unit, the way `uptime(1)` reads.
fn fmt_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_scale_to_readable_units() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1024 * 1024 * 3 / 2), "1.5 MB");
        // Three digits drop the decimal so the column stays narrow.
        assert_eq!(fmt_bytes(1024 * 1024 * 512), "512 MB");
        assert_eq!(fmt_bytes(1024u64.pow(4)), "1.0 TB");
    }

    #[test]
    fn uptime_reads_like_uptime_1() {
        assert_eq!(fmt_uptime(45), "0m");
        assert_eq!(fmt_uptime(3_600 + 120), "1h 2m");
        assert_eq!(fmt_uptime(86_400 * 4 + 3_600 * 5), "4d 5h");
    }
}
