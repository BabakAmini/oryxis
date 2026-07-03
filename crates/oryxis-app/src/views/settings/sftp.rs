//! Settings -> SFTP section view. Split out of views/settings/mod.rs.

use super::*;
use iced::widget::column;

impl Oryxis {
    pub(crate) fn view_settings_sftp(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order; each input row
        // focuses its field on Enter (ids are static, the fork's
        // widget::Id only takes &'static str). Recording happens at
        // construction, so everything below is built only when it
        // actually renders (`sftp_enabled`).
        self.keynav_settings_reset();
        let build_concurrency_section = || panel_section(column![
            text(t("transfer_parallelism"))
                .size(13)
                .color(OryxisColors::t().text_primary),
            Space::new().height(4),
            text(t("setting_sftp_concurrency_desc"))
                .size(11)
                .color(OryxisColors::t().text_muted),
            Space::new().height(8),
            self.settings_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("set-sftp-concurrency")),
                10.0,
                text_input("2", &self.setting_sftp_concurrency)
                    .id(iced::widget::Id::new("set-sftp-concurrency"))
                    .on_input(Message::SettingSftpConcurrencyChanged)
                    .padding(10)
                    .width(240)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
            ),
        ]);

        let timeout_input = |label: &str,
                             hint: &str,
                             value: &str,
                             id: &'static str,
                             on_input: fn(String) -> Message| {
            panel_section(column![
                text(label.to_string())
                    .size(13)
                    .color(OryxisColors::t().text_primary),
                Space::new().height(4),
                text(hint.to_string())
                    .size(11)
                    .color(OryxisColors::t().text_muted),
                Space::new().height(8),
                self.settings_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(id)),
                    10.0,
                    text_input("0", value)
                        .id(iced::widget::Id::new(id))
                        .on_input(on_input)
                        .padding(10)
                        .width(240)
                        .style(crate::widgets::rounded_input_style)
                        .align_x(dir_align_x())
                        .into(),
                ),
            ])
        };

        // Enable/disable lives on the Plugins screen now; this
        // section only renders while SFTP is enabled, showing its
        // tuning knobs (parallelism, timeouts).
        let mut content_col: iced::widget::Column<'_, Message> = column![]
            .width(Length::Fill)
            .align_x(dir_align_x());

        if self.sftp_enabled {
            content_col = content_col
                .push(build_concurrency_section())
                .push(Space::new().height(12))
                .push(timeout_input(
                    t("connect_timeout"),
                    t("connect_timeout_desc"),
                    &self.setting_sftp_connect_timeout,
                    "set-sftp-connect-timeout",
                    Message::SettingSftpConnectTimeoutChanged,
                ))
                .push(Space::new().height(12))
                .push(timeout_input(
                    t("auth_timeout"),
                    t("auth_timeout_desc"),
                    &self.setting_sftp_auth_timeout,
                    "set-sftp-auth-timeout",
                    Message::SettingSftpAuthTimeoutChanged,
                ))
                .push(Space::new().height(12))
                .push(timeout_input(
                    t("channel_open_timeout"),
                    t("channel_open_timeout_desc"),
                    &self.setting_sftp_session_timeout,
                    "set-sftp-session-timeout",
                    Message::SettingSftpSessionTimeoutChanged,
                ))
                .push(Space::new().height(12))
                .push(timeout_input(
                    t("operation_timeout"),
                    t("operation_timeout_desc"),
                    &self.setting_sftp_op_timeout,
                    "set-sftp-op-timeout",
                    Message::SettingSftpOpTimeoutChanged,
                ));
        }
        content_col = content_col.push(Space::new().height(24));

        scrollable(
            container(content_col)
                .padding(Padding { top: 24.0, right: 24.0, bottom: 24.0, left: 24.0 }),
        )
        // Stable id so the keyboard router can keep the selected row
        // in view.
        .id(iced::widget::Id::new("settings-sftp-scroll"))
        .height(Length::Fill)
        .into()
    }
}
