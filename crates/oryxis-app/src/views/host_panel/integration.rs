//! Host editor: the SSH Integration rows (expose-to-MCP toggle,
//! remote-desktop block, environment variables, initial command).
use super::*;
use iced::widget::column;

impl Oryxis {
    pub(super) fn hp_row_mcp(&self, is_ssh: bool) -> Element<'_, Message> {
        // Expose to MCP / AI (SSH > Integration).
        let row_mcp: Element<'_, Message> = if is_ssh {
            self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::EditorToggleMcpEnabled),
            8.0,
            container(
                dir_row(vec![
                    iced_fonts::lucide::plug().size(14).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    text(t("expose_to_mcp")).size(13).color(OryxisColors::t().text_secondary).into(),
                    Space::new().width(Length::Fill).into(),
                    {
                        let on = self.editor_form.mcp_enabled;
                        let bg = if on { OryxisColors::t().success } else { OryxisColors::t().bg_hover };
                        let fg = crate::theme::contrast_text_for(bg);
                        button(text(if on { crate::i18n::t("toggle_on") } else { crate::i18n::t("toggle_off") }).size(12).color(fg))
                            .on_press(Message::EditorToggleMcpEnabled)
                            .style(move |_theme, _status| button::Style {
                                background: Some(Background::Color(bg)),
                                border: Border { radius: Radius::from(4.0), ..Default::default() },
                                text_color: fg,
                                ..Default::default()
                            })
                            .into()
                    },
                ]).align_y(iced::Alignment::Center)
            )
            .padding(Padding { top: 8.0, right: 0.0, bottom: 8.0, left: 0.0 }).into(),
            )
        } else {
            empty()
        };
        row_mcp
    }

    pub(super) fn hp_rd_block(&self, is_rd: bool) -> Element<'_, Message> {
        use oryxis_core::models::connection::ConnectionProtocol as Proto;
        // Remote-desktop rows (RemoteDesktop hosts only): the kind picker
        // (RDP/VNC) and the SSH gateway to tunnel through (or Direct). The
        // endpoint (host/port) and login (username/password) reuse the
        // shared fields above.
        let rd_block: Element<'_, Message> = if is_rd {
            use oryxis_core::models::remote_desktop::RemoteDesktopKind;
            let kind_row = panel_option_row(
                iced_fonts::lucide::monitor_smartphone(),
                t("remote_desktop_kind"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-rd-kind")),
                    crate::widgets::INPUT_RADIUS,
                    pick_list(
                        Some(self.editor_form.rd_kind),
                        vec![RemoteDesktopKind::Rdp, RemoteDesktopKind::Vnc],
                        |k: &RemoteDesktopKind| k.to_string(),
                    )
                    .on_select(Message::EditorRdKindChanged)
                    .id(iced::widget::Id::new("editor-pick-rd-kind"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            );
            // Gateway: `None` = Direct, else an SSH host to tunnel through.
            let gw_options: Vec<Option<uuid::Uuid>> = std::iter::once(None)
                .chain(
                    self.connections
                        .iter()
                        .filter(|c| c.protocol == Proto::Ssh)
                        .map(|c| Some(c.id)),
                )
                .collect();
            let gw_labels: std::collections::HashMap<Option<uuid::Uuid>, String> =
                std::iter::once((None, t("remote_desktop_direct").to_string()))
                    .chain(
                        self.connections
                            .iter()
                            .filter(|c| c.protocol == Proto::Ssh)
                            .map(|c| (Some(c.id), c.label.clone())),
                    )
                    .collect();
            let gw_row = panel_option_row(
                iced_fonts::lucide::route(),
                t("remote_desktop_gateway"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-rd-gateway")),
                    crate::widgets::INPUT_RADIUS,
                    pick_list(
                        Some(self.editor_form.rd_gateway_id),
                        gw_options,
                        move |id: &Option<uuid::Uuid>| {
                            gw_labels.get(id).cloned().unwrap_or_default()
                        },
                    )
                    .on_select(Message::EditorRdGatewayChanged)
                    .id(iced::widget::Id::new("editor-pick-rd-gateway"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(200)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            );
            column![kind_row, Space::new().height(ROW_GAP), gw_row].into()
        } else {
            empty()
        };
        rd_block
    }

    pub(super) fn hp_env_items(&self, is_ssh: bool) -> Element<'_, Message> {
        // ── Section: Environment Variables ──
        let env_items: Element<'_, Message> = if is_ssh {
        let mut env_items = column![
            dir_row(vec![
                iced_fonts::lucide::variable().size(14).color(OryxisColors::t().text_muted).into(),
                Space::new().width(10).into(),
                column![
                    text(t("env_vars")).size(13).color(OryxisColors::t().text_secondary),
                    Space::new().height(2),
                    text(t("env_vars_desc")).size(11).color(OryxisColors::t().text_muted),
                ].width(Length::Fill).into(),
                Space::new().width(8).into(),
                self.panel_nav_slot(
                    crate::keynav::RowAction::activate(Message::EditorAddEnvVar),
                    4.0,
                    button(text("+").size(14).color(OryxisColors::t().text_primary))
                        .on_press(Message::EditorAddEnvVar)
                        .style(|_, _| button::Style {
                            background: Some(Background::Color(OryxisColors::t().bg_hover)),
                            border: Border { radius: Radius::from(4.0), ..Default::default() },
                            text_color: OryxisColors::t().text_primary,
                            ..Default::default()
                        })
                        .padding(Padding { top: 2.0, right: 8.0, bottom: 2.0, left: 8.0 })
                        .into(),
                ),
            ]).align_y(iced::Alignment::Center),
        ];

        for (i, e) in self.editor_form.env_vars.iter().enumerate() {
            let idx = i;
            env_items = env_items.push(Space::new().height(8));
            // Same static-id limitation as the port-forward rows: the
            // key/value inputs stay mouse-only, the remove button is
            // the keyboard row.
            env_items = env_items.push(
                dir_row(vec![
                    text_input("LC_EXAMPLE", &e.key)
                        .on_input(move |v| Message::EditorEnvVarKeyChanged(idx, v))
                        .padding(6)
                        .width(Length::FillPortion(2))
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    text("=").size(12).color(OryxisColors::t().text_muted).into(),
                    text_input(crate::i18n::t("env_value_placeholder"), &e.value)
                        .on_input(move |v| Message::EditorEnvVarValueChanged(idx, v))
                        .padding(6)
                        .width(Length::FillPortion(3))
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::activate(Message::EditorRemoveEnvVar(idx)),
                        4.0,
                        button(text("\u{00D7}").size(11).color(OryxisColors::t().error))
                            .on_press(Message::EditorRemoveEnvVar(idx))
                            .style(|_, _| button::Style {
                                background: None,
                                border: Border::default(),
                                text_color: OryxisColors::t().error,
                                ..Default::default()
                            })
                            .padding(Padding { top: 2.0, right: 4.0, bottom: 2.0, left: 4.0 })
                            .into(),
                    ),
                ]).align_y(iced::Alignment::Center).spacing(4),
            );
        }
        env_items.into()
        } else {
            empty()
        };
        env_items
    }

    pub(super) fn hp_startup_block(&self, is_ssh: bool) -> Element<'_, Message> {
        // Initial command / snippet (Terminal), sent to the shell right
        // after the session opens. Universal (keystrokes), so it lives in
        // the universal Terminal section, not the SSH block.
        // Forced-selection searchable combo: the None / Custom sentinels
        // and snippet labels (options built once in
        // `rebuild_editor_combos`). Picking commits via
        // EditorStartupChoiceChanged; typing only filters (no on_input,
        // so there is no free-text path). The current choice's label
        // seeds the selection (and doubles as the focused placeholder).
        let startup_block: Element<'_, Message> = if is_ssh {
        let startup_selected = self.editor_startup_label();
        // Keyboard row: Left/Right cycle None / Custom / snippet labels.
        let (startup_prev, startup_next) = crate::keynav::slots::cycle_pair(
            self.editor_startup_combo.options(),
            &startup_selected,
            Message::EditorStartupChoiceChanged,
        );
        let startup_picker: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::picker(startup_prev, startup_next),
            10.0,
            iced::widget::combo_box(
                &self.editor_startup_combo,
                &startup_selected,
                Some(&startup_selected),
                Message::EditorStartupChoiceChanged,
            )
            .on_open(Message::EditorStartupComboOpened)
            .padding(10)
            .input_style(crate::widgets::rounded_input_style)
            .menu_style(crate::widgets::combo_menu_style)
            .width(Length::Fill)
            .into(),
        );

        let mut startup_block = column![
            text(t("initial_command_label"))
                .size(12)
                .color(OryxisColors::t().text_muted),
            Space::new().height(8),
            startup_picker,
        ];
        if matches!(self.editor_startup_choice, crate::state::StartupChoice::Custom) {
            startup_block = startup_block.push(Space::new().height(8)).push(
                // Multi-line, auto-grows with content; container caps the
                // height (~8 lines) and then it scrolls internally. Supports
                // multi-command scripts (one command per line).
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-initial-command")),
                    10.0,
                    container(
                        text_editor(&self.editor_initial_command)
                            .id(iced::widget::Id::new("editor-initial-command"))
                            .placeholder(t("initial_command_ph"))
                            .on_action(Message::EditorInitialCommandChanged)
                            .padding(10)
                            .height(Length::Shrink)
                            .style(crate::widgets::rounded_editor_style),
                    )
                    .height(Length::Shrink.max(200.0))
                    .into(),
                ),
            );
        }
        startup_block.into()
        } else {
            empty()
        };
        startup_block
    }
}
