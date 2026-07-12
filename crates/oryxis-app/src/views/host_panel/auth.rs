//! Host editor: the SSH Authentication rows (auth-method picker, key
//! picker, agent forwarding).
use super::*;

impl Oryxis {
    pub(super) fn hp_row_auth_method(&self, is_ssh: bool) -> Element<'_, Message> {
        let auth_value = match self.editor_form.auth_method {
            AuthMethod::Auto => t("auth_auto"),
            AuthMethod::Password => t("auth_password"),
            AuthMethod::Key => t("auth_key"),
            AuthMethod::Agent => t("auth_agent"),
            AuthMethod::Interactive => t("auth_interactive"),
            AuthMethod::PasswordPrompt => t("auth_password_prompt"),
        };

        // Auth method (SSH > Authentication). Left/Right cycle the same
        // options the pick_list offers.
        let auth_options = vec![
            t("auth_auto").to_string(),
            t("auth_password").to_string(),
            t("auth_key").to_string(),
            t("auth_agent").to_string(),
            t("auth_interactive").to_string(),
            t("auth_password_prompt").to_string(),
        ];
        let auth_selected = auth_value.to_string();
        // Focusable select: Tab reaches it, Enter/Space open it, the
        // widget owns arrows/Esc while focused (fork support).
        let row_auth_method: Element<'_, Message> = if is_ssh {
            panel_option_row(
            iced_fonts::lucide::shield(),
            t("auth_method"),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-auth-method")),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(auth_selected), auth_options, |s: &String| s.clone())
                    .on_select(Message::EditorAuthMethodChanged)
                    .id(iced::widget::Id::new("editor-pick-auth-method"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    // Wider than the usual 120px so the longest option
                    // ("Password (ask...)" and its translations) is not truncated.
                    .width(200)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
            ),
            )
        } else {
            empty()
        };
        row_auth_method
    }

    pub(super) fn hp_ssh_key_row(&self, is_ssh: bool) -> Option<Element<'_, Message>> {
        // Key row (SSH > Authentication): only when Auth Method is `Key`
        // (the chosen-method's field) and no identity is set (an identity
        // provides its own key). Layout is [key icon] [combo] [+ Key].
        // Built after the auth-method row so its keyboard rows record
        // right below it, matching the layout.
        let ssh_key_row: Option<Element<'_, Message>> = if is_ssh
            && self.editor_form.selected_identity.is_none()
            && self.editor_form.auth_method == AuthMethod::Key
        {
            // "+ Key" is clickable, opens the existing key import panel.
            let add_key_btn = self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::ShowKeyPanel),
                6.0,
                button(
                    text(t("add_key_btn")).size(12).color(OryxisColors::t().accent),
                )
                .on_press(Message::ShowKeyPanel)
                .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                .style(|_, status| {
                    let bg = match status {
                        BtnStatus::Hovered => Color { a: 0.1, ..OryxisColors::t().accent },
                        _ => Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(Background::Color(bg)),
                        border: Border { radius: Radius::from(6.0), ..Default::default() },
                        ..Default::default()
                    }
                })
                .into(),
            );
            // Forced-selection searchable key combo (same pattern as the
            // startup combo): options + clear-on-focus built in
            // `rebuild_editor_combos` / `EditorKeyComboOpened`.
            let key_selected = self
                .editor_form
                .selected_key
                .clone()
                .unwrap_or_else(|| "(none)".into());
            // Keyboard row: Left/Right cycle the saved keys (the combo
            // itself cannot be focused by id in the fork).
            let (key_prev, key_next) = crate::keynav::slots::cycle_pair(
                self.editor_key_combo.options(),
                &key_selected,
                Message::EditorKeyChanged,
            );
            let key_combo: Element<'_, Message> = self.panel_nav_slot(
                crate::keynav::RowAction::picker(key_prev, key_next),
                10.0,
                iced::widget::combo_box(
                    &self.editor_key_combo,
                    &key_selected,
                    Some(&key_selected),
                    Message::EditorKeyChanged,
                )
                .on_open(Message::EditorKeyComboOpened)
                .padding(10)
                .input_style(crate::widgets::rounded_input_style)
                .menu_style(crate::widgets::combo_menu_style)
                .width(Length::Fill)
                .into(),
            );
            let key_row = dir_row(vec![
                iced_fonts::lucide::key_round()
                    .size(13)
                    .color(OryxisColors::t().text_muted)
                    .into(),
                Space::new().width(10).into(),
                key_combo,
                Space::new().width(8).into(),
                add_key_btn,
            ])
            .align_y(iced::Alignment::Center);

            // Certificate hint (B2): when the selected key carries a cert,
            // show "attached, valid until <date>" (warning color if
            // expired) so the user sees it will be offered at connect.
            let cert_hint: Option<Element<'_, Message>> = self
                .keys
                .iter()
                .find(|k| k.label == key_selected)
                .and_then(|k| k.certificate.as_deref())
                .and_then(|line| ssh_key::Certificate::from_openssh(line.trim()).ok())
                .map(|cert| {
                    let now = chrono::Utc::now().timestamp().max(0) as u64;
                    let expired = cert.valid_before() != 0
                        && cert.valid_before() != u64::MAX
                        && now > cert.valid_before();
                    let until = match chrono::DateTime::<chrono::Utc>::from_timestamp(
                        cert.valid_before().min(i64::MAX as u64) as i64,
                        0,
                    ) {
                        Some(dt) if cert.valid_before() != 0 && cert.valid_before() != u64::MAX => {
                            dt.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()
                        }
                        _ => String::new(),
                    };
                    let (label, color) = if expired {
                        (format!("{} · {}", t("cert_attach"), t("cert_expired")), OryxisColors::t().warning)
                    } else if until.is_empty() {
                        (t("cert_attach").to_string(), OryxisColors::t().text_muted)
                    } else {
                        (
                            format!("{} · {} {}", t("cert_attach"), t("cert_valid_until"), until),
                            OryxisColors::t().text_muted,
                        )
                    };
                    container(text(label).size(11).color(color))
                        .width(Length::Fill)
                        .align_x(dir_align_x())
                        .into()
                });

            let mut col = iced::widget::Column::new()
                .push(key_row)
                .width(Length::Fill)
                .align_x(dir_align_x());
            if let Some(hint) = cert_hint {
                col = col.push(Space::new().height(4)).push(hint);
            }
            Some(col.into())
        } else {
            None
        };
        ssh_key_row
    }

    pub(super) fn hp_row_agent_fwd(&self, is_ssh: bool) -> Element<'_, Message> {
        // Agent forwarding (SSH > Authentication). `share` (not the key
        // glyph) so it doesn't read as a duplicate of the Key row above.
        let row_agent_fwd: Element<'_, Message> = if is_ssh {
            self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::EditorToggleAgentForwarding),
            8.0,
            container(
                dir_row(vec![
                    iced_fonts::lucide::share().size(14).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    text(t("forward_ssh_agent")).size(13).color(OryxisColors::t().text_secondary).into(),
                    Space::new().width(Length::Fill).into(),
                    {
                        let on = self.editor_form.agent_forwarding;
                        let bg = if on { OryxisColors::t().success } else { OryxisColors::t().bg_hover };
                        let fg = crate::theme::contrast_text_for(bg);
                        button(text(if on { crate::i18n::t("toggle_on") } else { crate::i18n::t("toggle_off") }).size(12).color(fg))
                            .on_press(Message::EditorToggleAgentForwarding)
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
        row_agent_fwd
    }
}
