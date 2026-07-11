//! Settings -> Advanced section view: the debug-logging file toggle and
//! the environment report for GitHub issues.

use super::*;
use iced::widget::column;

impl Oryxis {
    pub(crate) fn view_settings_advanced(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order.
        self.keynav_settings_reset();
        // ── Download mirror (China / blocked-network delivery) ──
        let mirror_section = self.download_mirror_section();
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
                    mirror_section,
                    Space::new().height(12),
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

    /// The download-mirror block: picker (Auto / GitHub / Custom),
    /// and while Custom is selected a URL field plus a Test button
    /// running the reachability probe. Content integrity never
    /// depends on the mirror (sha256/Ed25519 gates), so the URL is
    /// user-configurable without a trust prompt.
    fn download_mirror_section(&self) -> Element<'_, Message> {
        use crate::net_mirror::MirrorChoice;
        let ui = &self.download_mirror;

        let selected_token = if ui.custom_pending || matches!(ui.choice, MirrorChoice::Custom(_))
        {
            "custom"
        } else if ui.choice == MirrorChoice::GitHubDirect {
            "github"
        } else {
            "auto"
        };
        let display = |token: &String| {
            t(match token.as_str() {
                "github" => "download_mirror_github",
                "custom" => "download_mirror_custom",
                _ => "download_mirror_auto",
            })
            .to_string()
        };
        let picker = self.nav_pick_row(
            t("download_mirror"),
            vec!["auto".into(), "github".into(), "custom".into()],
            selected_token.to_string(),
            display,
            220.0,
            Message::DownloadMirrorPicked,
        );

        let mut rows = column![
            picker,
            Space::new().height(4),
            text(t("download_mirror_desc")).size(11).color(OryxisColors::t().text_muted),
        ];

        if selected_token == "custom" {
            // Keyboard rows: the URL field (Enter commits), then Test.
            let url_idx = self.settings_nav_record(crate::keynav::RowAction::input(
                iced::widget::Id::new("set-download-mirror-url"),
            ));
            let url_field = self.settings_nav_ring_at(
                url_idx,
                10.0,
                text_input(t("download_mirror_url_placeholder"), &ui.url_input)
                    .id(iced::widget::Id::new("set-download-mirror-url"))
                    .on_input(Message::DownloadMirrorUrlEdited)
                    .on_submit(Message::DownloadMirrorUrlCommitted)
                    .padding(10)
                    .width(360)
                    .style(crate::widgets::rounded_input_style)
                    .into(),
            );
            let test_btn: Element<'_, Message> = if ui.testing {
                styled_button_opt(t("download_mirror_test_running"), None, OryxisColors::t().bg_selected)
            } else {
                self.settings_nav_slot(
                    crate::keynav::RowAction::activate(Message::DownloadMirrorTest),
                    6.0,
                    styled_button(
                        t("download_mirror_test"),
                        Message::DownloadMirrorTest,
                        OryxisColors::t().bg_selected,
                    ),
                )
            };
            let save_btn = self.settings_nav_slot(
                crate::keynav::RowAction::activate(Message::DownloadMirrorUrlCommitted),
                6.0,
                styled_button(
                    t("save"),
                    Message::DownloadMirrorUrlCommitted,
                    OryxisColors::t().accent,
                ),
            );
            rows = rows
                .push(Space::new().height(10))
                .push(
                    dir_row(vec![
                        url_field,
                        Space::new().width(8).into(),
                        save_btn,
                        Space::new().width(8).into(),
                        test_btn,
                    ])
                    .align_y(iced::Alignment::Center),
                );
            if ui.url_error {
                rows = rows.push(Space::new().height(6)).push(
                    text(t("download_mirror_https_required"))
                        .size(11)
                        .color(OryxisColors::t().error),
                );
            }
            match &ui.test_result {
                Some(Ok(ms)) => {
                    rows = rows.push(Space::new().height(6)).push(
                        text(format!("{} ({ms} ms)", t("download_mirror_test_ok")))
                            .size(11)
                            .color(OryxisColors::t().success),
                    );
                }
                Some(Err(cause)) => {
                    rows = rows.push(Space::new().height(6)).push(
                        text(format!("{}: {cause}", t("download_mirror_test_fail")))
                            .size(11)
                            .color(OryxisColors::t().error),
                    );
                }
                None => {}
            }
        }

        panel_section(rows)
    }
}
