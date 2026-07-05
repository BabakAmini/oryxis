//! Settings -> Advanced section view: the debug-logging file toggle and
//! the environment report for GitHub issues.

use super::*;
use iced::widget::column;

impl Oryxis {
    pub(crate) fn view_settings_advanced(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order.
        self.keynav_settings_reset();
        // ── Debug logging ──
        let log_path = crate::logging::log_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        let debug_section = panel_section(column![
            self.nav_toggle_row(
                t("debug_logging"),
                self.setting_debug_logging,
                Message::SettingToggleDebugLogging,
            ),
            Space::new().height(4),
            text(t("debug_logging_desc")).size(11).color(OryxisColors::t().text_muted),
            Space::new().height(12),
            settings_row(t("debug_log_file"), log_path),
            Space::new().height(8),
            dir_row(vec![
                self.settings_nav_slot(
                    crate::keynav::RowAction::activate(Message::RevealDebugLog),
                    6.0,
                    styled_button(
                        crate::i18n::open_in_file_manager_label(),
                        Message::RevealDebugLog,
                        OryxisColors::t().bg_selected,
                    ),
                ),
                Space::new().width(10).into(),
                self.settings_nav_slot(
                    crate::keynav::RowAction::activate(Message::ClearDebugLog),
                    6.0,
                    styled_button(
                        t("debug_log_clear"),
                        Message::ClearDebugLog,
                        OryxisColors::t().bg_selected,
                    ),
                ),
            ]),
        ]);

        // ── Performance HUD ──
        let perf_section = panel_section(column![
            self.nav_toggle_row(
                t("perf_overlay"),
                self.setting_perf_overlay,
                Message::SettingTogglePerfOverlay,
            ),
            Space::new().height(4),
            text(t("perf_overlay_desc")).size(11).color(OryxisColors::t().text_muted),
        ]);

        // ── Environment information ──
        // The report is rendered verbatim so the user sees exactly what
        // the Copy button puts on the clipboard, nothing hidden.
        let env_report = crate::logging::environment_report(self.renderer_active.as_ref());
        let report_block = container(
            text(env_report.clone())
                .size(11)
                .font(iced::Font::MONOSPACE)
                .color(OryxisColors::t().text_secondary),
        )
        .padding(12)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(OryxisColors::t().bg_selected)),
            border: Border { radius: Radius::from(6.0), ..Default::default() },
            ..Default::default()
        });
        let env_section = panel_section(column![
            text(t("env_info")).size(14).color(OryxisColors::t().text_muted),
            Space::new().height(4),
            text(t("env_info_desc")).size(11).color(OryxisColors::t().text_muted),
            Space::new().height(10),
            report_block,
            Space::new().height(10),
            self.settings_nav_slot(
                crate::keynav::RowAction::activate(Message::CopyToClipboard(
                    env_report.clone(),
                )),
                6.0,
                styled_button(
                    t("copy_env_info"),
                    Message::CopyToClipboard(env_report),
                    OryxisColors::t().accent,
                ),
            ),
        ]);

        scrollable(
            container(
                column![
                    debug_section,
                    Space::new().height(12),
                    perf_section,
                    Space::new().height(12),
                    env_section,
                    Space::new().height(24),
                ]
                .width(Length::Fill)
                .align_x(dir_align_x()),
            )
            .padding(Padding { top: 24.0, right: 24.0, bottom: 24.0, left: 24.0 }),
        )
        // Stable id so the keyboard router can keep the selected row
        // in view.
        .id(iced::widget::Id::new("settings-advanced-scroll"))
        .height(Length::Fill)
        .into()
    }
}
