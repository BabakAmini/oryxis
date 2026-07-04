//! Host editor / connection editor side panel.

use iced::border::Radius;
use iced::widget::{button, column, container, pick_list, scrollable, text, text_editor, text_input, Space};
use iced::widget::button::Status as BtnStatus;
use iced::{Background, Border, Color, Element, Length, Padding};

use oryxis_core::models::connection::AuthMethod;
use oryxis_core::models::identity::Identity;

use crate::app::{Message, Oryxis};
use crate::i18n::t;
use crate::state::ProxyKind;
use crate::theme::OryxisColors;
use crate::app::PANEL_WIDTH;
use crate::widgets::{
    dir_align_x, dir_row, panel_divider, panel_field, panel_option_row,
    panel_section, password_input_with_eye_id,
};

impl Oryxis {
    pub(crate) fn view_host_panel(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order (row mode: Up/Down from any input).
        self.panel_nav_reset();
        let is_editing = self.editor_form.editing_id.is_some();
        let title = if is_editing { crate::i18n::t("edit_host") } else { crate::i18n::t("new_host") };
        let has_address = !self.editor_form.hostname.is_empty();
        // Telnet hosts hide the whole SSH block (keys, identities,
        // agent-fwd, jump chain, proxy, port-forwards, TOTP, MCP,
        // algorithms, initial command); the reduced form keeps only
        // label/parent/tags, host/port, username/password, encoding and
        // the terminal theme. `is_ssh` gates every SSH-only piece below.
        use oryxis_core::models::connection::ConnectionProtocol as Proto;
        let is_ssh = self.editor_form.protocol == Proto::Ssh;
        // Serial is even more reduced than Telnet: no auth (no
        // username/password), no numeric port, and its own line-param
        // block instead. `is_serial` additionally gates the shared
        // credentials + numeric-port widgets off.
        let is_serial = self.editor_form.protocol == Proto::Serial;
        // SSH-only rows (Authentication / Network / Integration) are
        // GATED on `is_ssh`: the reduced Telnet / Serial forms drop
        // them, and because `panel_nav_slot` records at build time, an
        // ungated build would record invisible Tab targets. Each gated
        // local resolves to this empty element so the keyboard walk only
        // ever sees visible rows.
        let empty = || -> Element<'_, Message> { Space::new().height(0).into() };

        // ── Header ──
        // The close (×) is intentionally not a keyboard row: Esc already
        // owns panel close, and recording it would make the header the
        // first Down target instead of the form.
        let panel_header = container(
            dir_row(vec![
                text(title).size(16).color(OryxisColors::t().text_primary).into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{00D7}").size(20).color(OryxisColors::t().text_muted))
                    .on_press(Message::EditorCancel)
                    .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                    .style(|_, _| button::Style {
                        background: Some(Background::Color(Color::TRANSPARENT)),
                        border: Border::default(),
                        ..Default::default()
                    }).into(),
            ]).align_y(iced::Alignment::Center),
        )
        // top 12 (not 16): the taller ×-button row centres the title, so a
        // 16 top padding optically reads ~4px lower than the 16 left. 12
        // lands the title's top edge level with the left gutter.
        .padding(Padding { top: 12.0, right: 16.0, bottom: 12.0, left: 16.0 });

        // ── Section: Host (label + parent group) ──
        // Built before the Connection widgets so their keyboard rows
        // record ahead of the hostname's (the assembly at the bottom
        // lays the Host card out first).
        let label_field: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::input(iced::widget::Id::new("editor-label")),
            10.0,
            text_input(t("my_server_placeholder"), &self.editor_form.label)
                .id(iced::widget::Id::new("editor-label"))
                .on_input(Message::EditorLabelChanged).on_submit(Message::EditorSave).padding(10)
                .style(crate::widgets::rounded_input_style).align_x(dir_align_x()).into(),
        );

        // Parent Group is a native iced combo_box: a single field that
        // filters the existing (visible) groups as you type and lets you
        // pick one, while still accepting a brand new name. The typed /
        // picked value flows through `EditorGroupChanged` into
        // `editor_form.group_name`, so the save path (find-or-create by
        // label) is unchanged. The `selection` prop drives the unfocused
        // display (the combo clears its internal value after a pick).
        let parent_selection = (!self.editor_form.group_name.is_empty())
            .then_some(&self.editor_form.group_name);
        // Keyboard row: Left/Right cycle the existing group names (the
        // fork's combo_box has no id hook, so Enter cannot focus it;
        // free-text entry stays a mouse/typing affordance).
        let (group_prev, group_next) = crate::keynav::slots::cycle_pair(
            self.editor_parent_combo.options(),
            &self.editor_form.group_name,
            Message::EditorGroupChanged,
        );
        let parent_combo: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::picker(group_prev, group_next),
            10.0,
            iced::widget::combo_box(
                &self.editor_parent_combo,
                t("group_placeholder"),
                parent_selection,
                Message::EditorGroupChanged,
            )
            .on_input(Message::EditorGroupChanged)
            .padding(10)
            .input_style(crate::widgets::rounded_input_style)
            .menu_style(crate::widgets::combo_menu_style)
            .width(Length::Fill)
            .into(),
        );

        // Tags: comma-separated free text, parsed on save. Feeds the
        // snippet sidebar's filter-by-host-tags toggle.
        let tags_field: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::input(iced::widget::Id::new("editor-tags")),
            10.0,
            text_input(t("tags_placeholder"), &self.editor_form.tags_text)
                .id(iced::widget::Id::new("editor-tags"))
                .on_input(Message::EditorTagsChanged)
                .on_submit(Message::EditorSave)
                .padding(10)
                .style(crate::widgets::rounded_input_style)
                .align_x(dir_align_x())
                .into(),
        );

        // ── Section: Address ──
        // Icon + color reflect the detected OS (once the silent probe has
        // run) or a user-picked override.
        let editing_conn = self.editor_form.editing_id.and_then(|id| {
            self.connections.iter().find(|c| c.id == id)
        });
        let (addr_glyph, addr_color) = crate::os_icon::resolve_for(
            editing_conn.and_then(|c| c.detected_os.as_deref()),
            editing_conn.and_then(|c| c.custom_icon.as_deref()),
            editing_conn.and_then(|c| c.custom_color.as_deref()),
            editing_conn.and_then(|c| c.username.as_deref()),
            OryxisColors::t().accent,
        );
        // Icon is a button when we're editing an existing host, clicking it
        // opens the icon/color picker so the user can override the OS mark.
        // For new (unsaved) hosts the id doesn't exist yet, so it's just a
        // static badge until the first save (and not a keyboard row).
        let icon_element: Element<'_, Message> = if let Some(id) = self.editor_form.editing_id {
            self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::ShowIconPicker(id)),
                8.0,
                button(
                    container(addr_glyph.view(18.0, Color::WHITE))
                        .width(Length::Fixed(32.0))
                        .height(Length::Fixed(32.0))
                        .center_x(Length::Fixed(32.0))
                        .center_y(Length::Fixed(32.0)),
                )
                .on_press(Message::ShowIconPicker(id))
                .padding(0)
                .style(move |_, status| {
                    let ring = match status {
                        BtnStatus::Hovered => Color::from_rgba(1.0, 1.0, 1.0, 0.25),
                        _ => Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(Background::Color(addr_color)),
                        border: Border { radius: Radius::from(8.0), color: ring, width: 1.5 },
                        ..Default::default()
                    }
                })
                .into(),
            )
        } else {
            container(addr_glyph.view(18.0, Color::WHITE))
                .width(Length::Fixed(32.0))
                .height(Length::Fixed(32.0))
                .center_x(Length::Fixed(32.0))
                .center_y(Length::Fixed(32.0))
                .style(move |_| container::Style {
                    background: Some(Background::Color(addr_color)),
                    border: Border { radius: Radius::from(8.0), ..Default::default() },
                    ..Default::default()
                })
                .into()
        };

        // Hostname row (Connection).
        let hostname_row: Element<'_, Message> = dir_row(vec![
            icon_element,
            Space::new().width(10).into(),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-hostname")),
                10.0,
                text_input(
                    if is_serial { t("serial_port_path_ph") } else { t("ip_or_hostname") },
                    &self.editor_form.hostname,
                )
                    .id(iced::widget::Id::new("editor-hostname"))
                    .on_input(Message::EditorHostnameChanged)
                    .on_submit(Message::EditorSave)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x()).into(),
            ),
        ]).align_y(iced::Alignment::Center).into();

        // Protocol picker (Connection). A cloud-imported host has its own
        // transport picker below and is always SSH-family, so the two are
        // mutually exclusive: hide the protocol picker on cloud hosts.
        let protocol_row: Option<Element<'_, Message>> = if self.editor_form.cloud_transport
            .is_some()
        {
            None
        } else {
            let options = vec![Proto::Ssh, Proto::Telnet, Proto::Serial];
            let picker = self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-protocol")),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(self.editor_form.protocol), options, |p| p.to_string())
                    .on_select(Message::EditorProtocolChanged)
                    .id(iced::widget::Id::new("editor-pick-protocol"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
            );
            Some(
                column![
                    text(t("protocol")).size(12).color(OryxisColors::t().text_muted),
                    Space::new().height(8),
                    picker,
                ]
                .into(),
            )
        };

        // Cloud-managed transport picker (Connection), only when the
        // connection being edited carries a `cloud_ref` (i.e. it was
        // imported from a cloud provider). Lets the user flip between
        // SSH (default) and AWS Instance Connect / SSM transports.
        // Built here (before the SSH card widgets) so its keyboard row
        // records in visual order inside the Host card.
        let cloud_transport_row: Option<Element<'_, Message>> =
            self.editor_form.cloud_transport.map(|current| {
                use oryxis_core::models::cloud::TransportKind;
                let options = vec![
                    TransportKind::Ssh,
                    TransportKind::InstanceConnect,
                    TransportKind::Ssm,
                ];
                // Focusable select: Tab reaches it, Enter/Space open it,
                // the widget owns arrows/Esc while focused (fork support).
                let picker = self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(
                        "editor-pick-cloud-transport",
                    )),
                    crate::widgets::INPUT_RADIUS,
                    pick_list(Some(current), options, |t| match t {
                        TransportKind::Ssh => "SSH".to_string(),
                        TransportKind::InstanceConnect => "EC2 Instance Connect".to_string(),
                        TransportKind::Ssm => "SSM Session".to_string(),
                        TransportKind::EcsExec => "ECS Exec".to_string(),
                        TransportKind::KubectlExec => "kubectl exec".to_string(),
                    })
                    .on_select(Message::EditorCloudTransportChanged)
                    .id(iced::widget::Id::new("editor-pick-cloud-transport"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                );
                column![
                    text(t("cloud_dynamic_form_transport")).size(12).color(OryxisColors::t().text_muted),
                    Space::new().height(8),
                    picker,
                ].into()
            });

        // ── Connection / Credentials / SSH fields ──
        // The host editor is being reorganised into a universal region
        // (General, Connection, Credentials, Terminal) and an SSH-only
        // region (Authentication, Network, Integration) so a future
        // protocol switch can hide the SSH block wholesale. Each widget
        // is extracted into a local here, then composed into sections in
        // the assembly at the bottom; nothing about the form state, save
        // path, or messages changes. Locals are built in the same order
        // the assembly lays them out so keyboard rows record in visual
        // order.

        // Numeric port, dropped inline into the SSH/Telnet card header
        // ("SSH ........ [22] port"). Serial has no TCP port, so it is
        // gated off (empty) and the serial header omits it.
        let port_input: Element<'_, Message> = if is_serial {
            empty()
        } else {
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-port")),
                10.0,
                text_input("22", &self.editor_form.port)
                    .id(iced::widget::Id::new("editor-port"))
                    .on_input(Message::EditorPortChanged)
                    .on_submit(Message::EditorSave)
                    .padding(6)
                    .width(56)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x()).into(),
            )
        };

        // Credentials column: username, then identity suggestions, then
        // either the "managed by identity" banner is hoisted to the SSH
        // Authentication group, or the password row is appended below.
        // Gated off for Serial, which has no auth (and whose param block
        // replaces it); building it there would record dead Tab targets.
        let cred_items: Element<'_, Message> = if is_serial {
            empty()
        } else {
        let mut cred_items = column![
            dir_row(vec![
                iced_fonts::lucide::user().size(13).color(OryxisColors::t().text_muted).into(),
                Space::new().width(10).into(),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-username")),
                    10.0,
                    text_input(t("username"), &self.editor_form.username)
                        .id(iced::widget::Id::new("editor-username"))
                        .on_input(Message::EditorUsernameChanged)
                        .on_submit(Message::EditorSave)
                        .padding(10)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x()).into(),
                ),
            ]).align_y(iced::Alignment::Center)
        ];

        // Identity suggestion dropdown (only when username field is
        // focused). Identities are an SSH-auth concept, so the reduced
        // Telnet form never offers them.
        if is_ssh && self.editor_form.username_focused && self.editor_form.selected_identity.is_none() && !self.identities.is_empty() {
            let search = self.editor_form.username.to_lowercase();
            let matching: Vec<&Identity> = if search.is_empty() {
                self.identities.iter().collect()
            } else {
                self.identities.iter()
                    .filter(|i| i.label.to_lowercase().contains(&search)
                        || i.username.as_deref().unwrap_or("").to_lowercase().contains(&search))
                    .collect()
            };
            if !matching.is_empty() {
                for identity in matching.iter().take(3) {
                    let label = identity.label.clone();
                    let subtitle = format!(
                        "{}{}",
                        identity.username.as_deref().unwrap_or(""),
                        if identity.key_id.is_some() {
                            let key_name = identity.key_id.and_then(|kid| {
                                self.keys.iter().find(|k| k.id == kid).map(|k| k.label.as_str())
                            }).unwrap_or("key");
                            format!(", {}", key_name)
                        } else { String::new() },
                    );
                    let ident_label = identity.label.clone();
                    cred_items = cred_items.push(self.panel_nav_slot(
                        crate::keynav::RowAction::activate(Message::EditorIdentityChanged(
                            ident_label.clone(),
                        )),
                        6.0,
                        button(
                            container(
                                dir_row(vec![
                                    iced_fonts::lucide::user().size(12).color(OryxisColors::t().accent).into(),
                                    Space::new().width(8).into(),
                                    column![
                                        text(label.clone()).size(12).color(OryxisColors::t().text_primary),
                                        text(subtitle.clone()).size(10).color(OryxisColors::t().text_muted),
                                    ].into(),
                                ]).align_y(iced::Alignment::Center),
                            )
                            .padding(Padding { top: 6.0, right: 10.0, bottom: 6.0, left: 10.0 })
                            .width(Length::Fill)
                            .style(|_| container::Style {
                                background: Some(Background::Color(OryxisColors::t().bg_surface)),
                                border: Border { radius: Radius::from(6.0), color: OryxisColors::t().border, width: 1.0 },
                                ..Default::default()
                            }),
                        )
                        .on_press(Message::EditorIdentityChanged(ident_label))
                        .width(Length::Fill)
                        .style(|_, status| {
                            let bg = match status {
                                BtnStatus::Hovered => OryxisColors::t().bg_hover,
                                _ => Color::TRANSPARENT,
                            };
                            button::Style {
                                background: Some(Background::Color(bg)),
                                ..Default::default()
                            }
                        })
                        .into(),
                    ));
                    cred_items = cred_items.push(Space::new().height(2));
                }
            }
        }

        // Identity selected -> the "managed by identity" banner replaces
        // both the password (Credentials) and the key (SSH Authentication).
        // We compute the banner / key / password as separate optionals so
        // each lands in its own section below.
        let ssh_identity_banner: Option<Element<'_, Message>> = self
            .editor_form
            .selected_identity
            .as_ref()
            .filter(|_| is_ssh)
            .map(|ident_label| {
                container(
                    dir_row(vec![
                        iced_fonts::lucide::user().size(14).color(OryxisColors::t().accent).into(),
                        Space::new().width(8).into(),
                        column![
                            text(format!("{}: {}", t("identity"), ident_label)).size(12).color(OryxisColors::t().text_primary),
                            text(t("managed_by_identity")).size(10).color(OryxisColors::t().text_muted),
                        ].into(),
                        Space::new().width(Length::Fill).into(),
                        button(text("\u{00D7}").size(11).color(OryxisColors::t().text_muted))
                            .on_press(Message::EditorIdentityChanged("(none)".into()))
                            .padding(4)
                            .style(|_, _| button::Style::default()).into(),
                    ]).align_y(iced::Alignment::Center),
                )
                .padding(10)
                .width(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color { a: 0.15, ..OryxisColors::t().accent })),
                    border: Border { radius: Radius::from(8.0), color: OryxisColors::t().accent, width: 1.0 },
                    ..Default::default()
                })
                .into()
            });

        // Credentials body: password row when no identity, else the
        // "managed by identity" banner (both belong with the login).
        cred_items = cred_items.push(Space::new().height(8));
        if let Some(banner) = ssh_identity_banner {
            // Keyboard row: Enter/Space clears the identity (the
            // banner's only verb, same as its × button).
            cred_items = cred_items.push(self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::EditorIdentityChanged("(none)".into())),
                8.0,
                banner,
            ));
        } else if is_ssh && self.editor_form.auth_method == AuthMethod::PasswordPrompt {
            // "Ask every time": no password is stored, so there is no field
            // to fill. A one-line note explains the prompt-on-connect flow.
            // SSH-only (the auth-method picker that sets it is hidden on
            // Telnet, which always shows a stored-password field).
            cred_items = cred_items.push(
                dir_row(vec![
                    iced_fonts::lucide::keyboard().size(13).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    text(t("auth_password_prompt_note"))
                        .size(12)
                        .color(OryxisColors::t().text_muted)
                        .into(),
                ]).align_y(iced::Alignment::Center)
            );
        } else {
            let pw_placeholder: &'static str = if self.editor_form.has_existing_password
                && !self.editor_form.password_touched
            {
                "••••••••"
            } else {
                t("password")
            };
            // Keyboard row: Tab focuses the inner input via its id.
            cred_items = cred_items.push(self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-password")),
                10.0,
                dir_row(vec![
                    iced_fonts::lucide::keyboard().size(13).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    password_input_with_eye_id(
                        pw_placeholder,
                        &self.editor_form.password,
                        Message::EditorPasswordChanged,
                        Some(Message::EditorSave),
                        self.editor_form.password_visible,
                        Message::EditorTogglePasswordVisibility,
                        10.0,
                        Some(iced::widget::Id::new("editor-password")),
                    ),
                ]).align_y(iced::Alignment::Center).into(),
            ));
        }

        // TOTP secret (2FA autofill). Shown for every auth method: a
        // keyboard-interactive second factor can follow any first factor
        // (password, key, agent). Same masked-placeholder tri-state as
        // the password field above. Telnet has no keyboard-interactive
        // second factor, so the reduced form hides it.
        if is_ssh {
            let totp_placeholder: &'static str = if self.editor_form.has_existing_totp
                && !self.editor_form.totp_touched
            {
                "••••••••"
            } else {
                t("totp_secret")
            };
            // Keyboard row: Tab focuses the inner input via its id.
            cred_items = cred_items.push(self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-totp")),
                10.0,
                dir_row(vec![
                    iced_fonts::lucide::shield_check().size(13).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    password_input_with_eye_id(
                        totp_placeholder,
                        &self.editor_form.totp_secret,
                        Message::EditorTotpChanged,
                        Some(Message::EditorSave),
                        self.editor_form.totp_visible,
                        Message::EditorToggleTotpVisibility,
                        10.0,
                        Some(iced::widget::Id::new("editor-totp")),
                    ),
                ]).align_y(iced::Alignment::Center).into(),
            ));
        }
        cred_items.into()
        };

        // Serial line parameters (reduced Serial form). Built here, after
        // the credentials region and before the terminal card, so the
        // keynav walk records them right after the protocol picker.
        // Gated off (empty) for non-serial protocols so they never
        // record invisible Tab targets.
        let serial_params_block: Element<'_, Message> = if is_serial {
            use oryxis_core::models::serial::{
                SerialFlowControl, SerialLineEnding, SerialParity, SerialStopBits,
                COMMON_BAUD_RATES,
            };
            let p = self.editor_form.serial.unwrap_or_default();
            let radius = crate::widgets::INPUT_RADIUS;
            // Baud: common rates (the model still accepts any u32).
            let baud_row = panel_option_row(
                iced_fonts::lucide::gauge(),
                t("serial_baud"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-serial-baud")),
                    radius,
                    pick_list(Some(p.baud), COMMON_BAUD_RATES.to_vec(), |b: &u32| b.to_string())
                        .on_select(Message::EditorSerialBaudChanged)
                        .id(iced::widget::Id::new("editor-serial-baud"))
                        .on_open(Message::PickOpenChanged(true))
                        .on_close(Message::PickOpenChanged(false))
                        .width(120)
                        .padding(10)
                        .style(crate::widgets::rounded_pick_list_style)
                        .into(),
                ),
            );
            let data_bits_row = panel_option_row(
                iced_fonts::lucide::binary(),
                t("serial_data_bits"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-serial-data")),
                    radius,
                    pick_list(Some(p.data_bits), vec![8u8, 7, 6, 5], |n: &u8| n.to_string())
                        .on_select(Message::EditorSerialDataBitsChanged)
                        .id(iced::widget::Id::new("editor-serial-data"))
                        .on_open(Message::PickOpenChanged(true))
                        .on_close(Message::PickOpenChanged(false))
                        .width(120)
                        .padding(10)
                        .style(crate::widgets::rounded_pick_list_style)
                        .into(),
                ),
            );
            let parity_row = panel_option_row(
                iced_fonts::lucide::sigma(),
                t("serial_parity"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-serial-parity")),
                    radius,
                    pick_list(
                        Some(p.parity),
                        vec![SerialParity::None, SerialParity::Odd, SerialParity::Even],
                        |v: &SerialParity| v.to_string(),
                    )
                    .on_select(Message::EditorSerialParityChanged)
                    .id(iced::widget::Id::new("editor-serial-parity"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            );
            let stop_row = panel_option_row(
                iced_fonts::lucide::octagon_pause(),
                t("serial_stop_bits"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-serial-stop")),
                    radius,
                    pick_list(
                        Some(p.stop_bits),
                        vec![SerialStopBits::One, SerialStopBits::Two],
                        |v: &SerialStopBits| v.to_string(),
                    )
                    .on_select(Message::EditorSerialStopBitsChanged)
                    .id(iced::widget::Id::new("editor-serial-stop"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            );
            let flow_row = panel_option_row(
                iced_fonts::lucide::waves(),
                t("serial_flow_control"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-serial-flow")),
                    radius,
                    pick_list(
                        Some(p.flow_control),
                        vec![
                            SerialFlowControl::None,
                            SerialFlowControl::Software,
                            SerialFlowControl::Hardware,
                        ],
                        |v: &SerialFlowControl| v.to_string(),
                    )
                    .on_select(Message::EditorSerialFlowChanged)
                    .id(iced::widget::Id::new("editor-serial-flow"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(140)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            );
            let line_ending_row = panel_option_row(
                iced_fonts::lucide::corner_down_left(),
                t("serial_line_ending"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-serial-eol")),
                    radius,
                    pick_list(
                        Some(p.line_ending),
                        vec![
                            SerialLineEnding::Cr,
                            SerialLineEnding::Lf,
                            SerialLineEnding::CrLf,
                        ],
                        |v: &SerialLineEnding| v.to_string(),
                    )
                    .on_select(Message::EditorSerialLineEndingChanged)
                    .id(iced::widget::Id::new("editor-serial-eol"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            );
            // Local echo: ON/OFF toggle (raw serial has no ECHO
            // negotiation, so a non-echoing device shows nothing typed
            // until this is on).
            let echo_row = self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::EditorSerialLocalEchoToggled),
                8.0,
                container(
                    dir_row(vec![
                        iced_fonts::lucide::eye().size(13).color(OryxisColors::t().text_muted).into(),
                        Space::new().width(10).into(),
                        text(t("serial_local_echo")).size(13).color(OryxisColors::t().text_secondary).into(),
                        Space::new().width(Length::Fill).into(),
                        {
                            let on = p.local_echo;
                            let bg = if on { OryxisColors::t().success } else { OryxisColors::t().bg_hover };
                            let fg = crate::theme::contrast_text_for(bg);
                            button(text(if on { "ON" } else { "OFF" }).size(12).color(fg))
                                .on_press(Message::EditorSerialLocalEchoToggled)
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
            );
            column![
                baud_row,
                Space::new().height(ROW_GAP),
                data_bits_row,
                Space::new().height(ROW_GAP),
                parity_row,
                Space::new().height(ROW_GAP),
                stop_row,
                Space::new().height(ROW_GAP),
                flow_row,
                Space::new().height(ROW_GAP),
                line_ending_row,
                Space::new().height(ROW_GAP),
                echo_row,
            ]
            .into()
        } else {
            empty()
        };

        // ── Section: Advanced Options ──
        // Chain summary for the "Host Chaining" row: the hop labels
        // joined in order (bastion > db-proxy > ...), or "disabled"
        // when empty. Hops pointing at a since-deleted host resolve to
        // a placeholder rather than vanishing, so the count stays
        // honest until the user opens the editor and prunes them.
        let chain_summary = if self.editor_form.jump_chain.is_empty() {
            t("disabled").to_string()
        } else {
            self.editor_form
                .jump_chain
                .iter()
                .map(|id| {
                    self.connections
                        .iter()
                        .find(|c| c.id == *id)
                        .map(|c| c.label.clone())
                        .unwrap_or_else(|| t("unknown").to_string())
                })
                .collect::<Vec<_>>()
                .join(" › ")
        };
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
            Some(
                dir_row(vec![
                    iced_fonts::lucide::key_round()
                        .size(13)
                        .color(OryxisColors::t().text_muted)
                        .into(),
                    Space::new().width(10).into(),
                    key_combo,
                    Space::new().width(8).into(),
                    add_key_btn,
                ]).align_y(iced::Alignment::Center).into(),
            )
        } else {
            None
        };

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
                        button(text(if on { "ON" } else { "OFF" }).size(12).color(fg))
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

        // Single "Host Chaining" entry point (SSH > Network). Clicking
        // opens the chain editor (Termius-style multi-hop). Replaces the
        // old read-only row + separate single-host "Jump Host" picker.
        let row_chaining: Element<'_, Message> = if is_ssh {
            self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::OpenChainEditor),
            6.0,
            container(
                button(
                    dir_row(vec![
                        iced_fonts::lucide::link().size(14).color(OryxisColors::t().text_muted).into(),
                        Space::new().width(10).into(),
                        text(t("host_chaining")).size(13).color(OryxisColors::t().text_secondary).into(),
                        Space::new().width(Length::Fill).into(),
                        text(chain_summary)
                            .size(13)
                            .color(OryxisColors::t().text_primary)
                            .into(),
                        Space::new().width(8).into(),
                        iced_fonts::lucide::chevron_right().size(12).color(OryxisColors::t().text_muted).into(),
                    ])
                    .align_y(iced::Alignment::Center),
                )
                .on_press(Message::OpenChainEditor)
                .padding(Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 0.0 })
                .style(|_, status| {
                    let bg = match status {
                        BtnStatus::Hovered => OryxisColors::t().bg_hover,
                        _ => Color::TRANSPARENT,
                    };
                    button::Style {
                        background: Some(Background::Color(bg)),
                        border: Border { radius: Radius::from(6.0), ..Default::default() },
                        ..Default::default()
                    }
                })
            ).into(),
            )
        } else {
            empty()
        };

        // Proxy rows (SSH > Network), nested inline (no card wrapper).
        let proxy_rows: Element<'_, Message> = if is_ssh {
            self.build_proxy_rows().into()
        } else {
            empty()
        };

        // ── Section: Port Forwarding ──
        let pf_items: Element<'_, Message> = if is_ssh {
        let mut pf_items = column![
            dir_row(vec![
                iced_fonts::lucide::arrow_right_left().size(14).color(OryxisColors::t().text_muted).into(),
                Space::new().width(10).into(),
                text(t("port_forwarding")).size(13).color(OryxisColors::t().text_secondary).into(),
                Space::new().width(Length::Fill).into(),
                self.panel_nav_slot(
                    crate::keynav::RowAction::activate(Message::EditorAddPortForward),
                    4.0,
                    button(text("+").size(14).color(OryxisColors::t().text_primary))
                        .on_press(Message::EditorAddPortForward)
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

        for (i, pf) in self.editor_form.port_forwards.iter().enumerate() {
            let idx = i;
            pf_items = pf_items.push(Space::new().height(8));
            // The three per-rule inputs stay mouse-only: the fork's
            // Id::new takes &'static str, so dynamic rows cannot carry
            // unique focus ids. The remove button is the keyboard row.
            pf_items = pf_items.push(
                dir_row(vec![
                    text_input("8080", &pf.local_port)
                        .on_input(move |v| Message::EditorPortFwdLocalPortChanged(idx, v))
                        .padding(6)
                        .width(70)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    text(" -> ").size(12).color(OryxisColors::t().text_muted).into(),
                    text_input("localhost", &pf.remote_host)
                        .on_input(move |v| Message::EditorPortFwdRemoteHostChanged(idx, v))
                        .padding(6)
                        .width(Length::Fill)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    text(":").size(12).color(OryxisColors::t().text_muted).into(),
                    text_input("3306", &pf.remote_port)
                        .on_input(move |v| Message::EditorPortFwdRemotePortChanged(idx, v))
                        .padding(6)
                        .width(70)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::activate(Message::EditorRemovePortForward(idx)),
                        4.0,
                        button(text("\u{00D7}").size(11).color(OryxisColors::t().error))
                            .on_press(Message::EditorRemovePortForward(idx))
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
        pf_items.into()
        } else {
            empty()
        };

        // pf_items (the port-forwarding column) is nested into SSH >
        // Network in the assembly below.

        // Per-host keepalive override (SSH > Network). Empty placeholder
        // reflects the global default so the user sees what "inherit"
        // means; "0" disables keepalive on this host.
        let row_keepalive: Element<'_, Message> = if is_ssh {
            container(
            dir_row(vec![
                iced_fonts::lucide::activity().size(14).color(OryxisColors::t().text_muted).into(),
                Space::new().width(10).into(),
                column![
                    text(t("host_keepalive")).size(13).color(OryxisColors::t().text_secondary),
                    Space::new().height(2),
                    text(t("host_keepalive_desc")).size(11).color(OryxisColors::t().text_muted),
                ].width(Length::Fill).into(),
                Space::new().width(12).into(),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("editor-keepalive")),
                    10.0,
                    text_input(
                        &self.setting_keepalive_interval,
                        &self.editor_form.keepalive_interval,
                    )
                        .id(iced::widget::Id::new("editor-keepalive"))
                        .on_input(Message::EditorKeepaliveChanged)
                        .on_submit(Message::EditorSave)
                        .padding(6)
                        .width(100)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                ),
            ]).align_y(iced::Alignment::Center)
            )
            .padding(Padding { top: 8.0, right: 0.0, bottom: 8.0, left: 0.0 }).into()
        } else {
            empty()
        };

        // Per-host auto-title (OSC 0/2) override: Default (inherit global) /
        // Show (always use the shell title) / Hide (always keep this host's
        // curated label).
        let auto_title_selected = match self.editor_form.auto_title {
            Some(true) => t("host_auto_title_show"),
            Some(false) => t("host_auto_title_hide"),
            None => t("host_auto_title_default"),
        }
        .to_string();
        let auto_title_options = vec![
            t("host_auto_title_default").to_string(),
            t("host_auto_title_show").to_string(),
            t("host_auto_title_hide").to_string(),
        ];
        // Focusable select (Tab + Enter/Space, widget-owned keys).
        let row_auto_title: Element<'_, Message> = if is_ssh {
            panel_option_row(
            iced_fonts::lucide::file_text(),
            t("host_auto_title"),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-auto-title")),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(auto_title_selected), auto_title_options, |s: &String| s.clone())
                    .on_select(Message::EditorAutoTitleChanged)
                    .id(iced::widget::Id::new("editor-pick-auto-title"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
            ),
            )
        } else {
            empty()
        };

        // Algorithm overrides (SSH > Network). Called here (not at the
        // assembly) so its keyboard rows record between the Network and
        // Integration rows, matching the layout.
        let algo_overrides: Element<'_, Message> = if is_ssh {
            self.algo_overrides_section()
        } else {
            empty()
        };

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
                        button(text(if on { "ON" } else { "OFF" }).size(12).color(fg))
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

        // Remote desktop (RDP/VNC over SSH), SSH > Integration. A toggle
        // reveals the kind picker + target host/port. SSH-only (the
        // launcher tunnels through the SSH session).
        let rd_block: Element<'_, Message> = if is_ssh {
            use oryxis_core::models::remote_desktop::RemoteDesktopKind;
            let enabled = self.editor_form.remote_desktop.is_some();
            let toggle = self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::EditorRemoteDesktopToggled),
                8.0,
                container(
                    dir_row(vec![
                        iced_fonts::lucide::monitor().size(14).color(OryxisColors::t().text_muted).into(),
                        Space::new().width(10).into(),
                        text(t("remote_desktop")).size(13).color(OryxisColors::t().text_secondary).into(),
                        Space::new().width(Length::Fill).into(),
                        {
                            let bg = if enabled { OryxisColors::t().success } else { OryxisColors::t().bg_hover };
                            let fg = crate::theme::contrast_text_for(bg);
                            button(text(if enabled { "ON" } else { "OFF" }).size(12).color(fg))
                                .on_press(Message::EditorRemoteDesktopToggled)
                                .style(move |_t, _s| button::Style {
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
            );
            let mut col = column![toggle];
            if let Some(rd) = self.editor_form.remote_desktop.clone() {
                let radius = crate::widgets::INPUT_RADIUS;
                let kind_row = panel_option_row(
                    iced_fonts::lucide::monitor_smartphone(),
                    t("remote_desktop_kind"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-rd-kind")),
                        radius,
                        pick_list(
                            Some(rd.kind),
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
                let host_row = panel_option_row(
                    iced_fonts::lucide::server(),
                    t("remote_desktop_target_host"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-rd-host")),
                        10.0,
                        text_input("localhost", &rd.target_host)
                            .id(iced::widget::Id::new("editor-rd-host"))
                            .on_input(Message::EditorRdTargetHostChanged)
                            .on_submit(Message::EditorSave)
                            .padding(8)
                            .width(160)
                            .style(crate::widgets::rounded_input_style)
                            .align_x(dir_align_x())
                            .into(),
                    ),
                );
                let port_row = panel_option_row(
                    iced_fonts::lucide::plug(),
                    t("remote_desktop_target_port"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-rd-port")),
                        10.0,
                        text_input("3389", &rd.target_port.to_string())
                            .id(iced::widget::Id::new("editor-rd-port"))
                            .on_input(Message::EditorRdTargetPortChanged)
                            .on_submit(Message::EditorSave)
                            .padding(8)
                            .width(80)
                            .style(crate::widgets::rounded_input_style)
                            .align_x(dir_align_x())
                            .into(),
                    ),
                );
                col = col
                    .push(Space::new().height(ROW_GAP))
                    .push(kind_row)
                    .push(Space::new().height(ROW_GAP))
                    .push(host_row)
                    .push(Space::new().height(ROW_GAP))
                    .push(port_row);
            }
            col.into()
        } else {
            empty()
        };

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
                    text_input("value", &e.value)
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

        // env_items (the environment-variables column) is nested into
        // SSH > Integration in the assembly below.

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

        // ── Section: Terminal appearance ──
        // A single "click to open picker" tile that mirrors the
        // current pick (palette swatches if a specific theme is set,
        // a plain "inherit" row otherwise). The full picker lives in
        // its own modal so this section stays compact.
        // Themed preview tile: shows the chosen per-host palette, or the
        // inherited global theme when there's no override, so the row is
        // always a real preview instead of a bare "use global" dropdown.
        // Click opens the full picker modal.
        // Resolve the override (built-in OR custom) to a palette for the
        // preview swatch; fall back to the inherited global when there's no
        // override (or the named custom theme was deleted).
        let override_name = self
            .editor_form
            .terminal_theme
            .as_deref()
            .filter(|name| self.terminal_palette_for_name(name).is_some());
        let (preview_palette, theme_label) = match override_name {
            Some(name) => (
                self.terminal_palette_for_name(name).unwrap(),
                name.to_string(),
            ),
            None => (
                self.resolve_global_terminal_palette(),
                format!(
                    "{} ({})",
                    crate::i18n::t("terminal_theme_inherit_global"),
                    self.resolve_global_terminal_theme_name()
                ),
            ),
        };
        let theme_trigger: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::EditorOpenThemePicker),
            8.0,
            terminal_theme_trigger(preview_palette, theme_label),
        );

        // Per-host icon shape override. The "Use default" entry maps to
        // an empty string which clears the override (resolved to the
        // global default_host_icon at render time).
        // Tokens drive the picker value (same pattern as Settings
        // -> Interface). Empty string is the "use default" token; the
        // dispatcher treats it as a None override on the form field.
        let icon_options = vec![
            String::new(),
            "circular".to_string(),
            "square".to_string(),
            "rounded".to_string(),
            "outline".to_string(),
            "initials".to_string(),
        ];
        let icon_selected = self.editor_form.icon_style.clone().unwrap_or_default();
        let icon_picker = pick_list(
            Some(icon_selected),
            icon_options,
            |s: &String| {
                let key = match s.as_str() {
                    "circular" => "icon_circular",
                    "square" => "icon_square",
                    "rounded" => "icon_rounded",
                    "outline" => "icon_outline",
                    "initials" => "icon_initials",
                    _ => "icon_use_default",
                };
                crate::i18n::t(key).to_string()
            },
        )
        .on_select(Message::EditorIconStyleChanged)
        .id(iced::widget::Id::new("editor-pick-icon-style"))
        .on_open(Message::PickOpenChanged(true))
        .on_close(Message::PickOpenChanged(false))
        .width(170)
        .padding(10)
        .style(crate::widgets::rounded_pick_list_style);
        // Focusable select (Tab + Enter/Space, widget-owned keys).
        let icon_row: Element<'_, Message> = dir_row(vec![
            text(crate::i18n::t("host_icon_style")).size(13).color(OryxisColors::t().text_secondary).into(),
            Space::new().width(Length::Fill).into(),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-icon-style")),
                crate::widgets::INPUT_RADIUS,
                icon_picker.into(),
            ),
        ]).align_y(iced::Alignment::Center).into();

        // Per-host terminal encoding. "UTF-8" is the default (stored as
        // None); the rest are encoding_rs labels the SSH engine transcodes.
        let encoding_options: Vec<String> = [
            "UTF-8", "Big5", "GBK", "gb18030", "Shift_JIS", "EUC-JP",
            "EUC-KR", "ISO-8859-1", "ISO-8859-15", "windows-1251",
            "windows-1252", "KOI8-R",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let encoding_selected = self
            .editor_form
            .encoding
            .clone()
            .unwrap_or_else(|| "UTF-8".to_string());
        let encoding_picker = pick_list(Some(encoding_selected), encoding_options, |s: &String| s.clone())
            .on_select(Message::EditorEncodingChanged)
            .id(iced::widget::Id::new("editor-pick-encoding"))
            .on_open(Message::PickOpenChanged(true))
            .on_close(Message::PickOpenChanged(false))
            .width(170)
            .padding(10)
            .style(crate::widgets::rounded_pick_list_style);
        // Focusable select, same treatment as the icon row.
        let encoding_row: Element<'_, Message> = dir_row(vec![
            text(crate::i18n::t("host_encoding")).size(13).color(OryxisColors::t().text_secondary).into(),
            Space::new().width(Length::Fill).into(),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-encoding")),
                crate::widgets::INPUT_RADIUS,
                encoding_picker.into(),
            ),
        ]).align_y(iced::Alignment::Center).into();

        // Per-host TERM. "xterm-256color" is the default (stored as None);
        // the rest are fallbacks for hosts whose terminfo trips on it.
        let term_options: Vec<String> = [
            "xterm-256color", "xterm", "screen-256color", "tmux-256color",
            "screen", "linux", "vt220", "vt100", "ansi",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let term_selected = self
            .editor_form
            .terminal_type
            .clone()
            .unwrap_or_else(|| "xterm-256color".to_string());
        let term_picker = pick_list(Some(term_selected), term_options, |s: &String| s.clone())
            .on_select(Message::EditorTerminalTypeChanged)
            .id(iced::widget::Id::new("editor-pick-term"))
            .on_open(Message::PickOpenChanged(true))
            .on_close(Message::PickOpenChanged(false))
            .width(170)
            .padding(10)
            .style(crate::widgets::rounded_pick_list_style);
        // Focusable select, same treatment as the icon row.
        let term_row: Element<'_, Message> = dir_row(vec![
            text(crate::i18n::t("host_terminal_type")).size(13).color(OryxisColors::t().text_secondary).into(),
            Space::new().width(Length::Fill).into(),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-term")),
                crate::widgets::INPUT_RADIUS,
                term_picker.into(),
            ),
        ]).align_y(iced::Alignment::Center).into();

        // Terminal card body: the theme keeps its full-width preview tile
        // (it's a live swatch, not a plain dropdown); icon and encoding
        // are compact inline rows (label left, picker right) like Auth
        // Method, so the section reads tight instead of three stacked
        // label+description blocks.
        let appearance_items = column![
            text(crate::i18n::t("terminal_theme"))
                .size(13)
                .color(OryxisColors::t().text_secondary),
            Space::new().height(8),
            theme_trigger,
            Space::new().height(14),
            icon_row,
            Space::new().height(12),
            encoding_row,
            Space::new().height(12),
            term_row,
        ];

        // Session logging (universal -> Terminal). Tri-state: Default
        // (inherit global) / On / Off. Enter/Space cycles the state.
        let row_session_logging: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::EditorCycleSessionLogging),
            8.0,
            container(
                dir_row(vec![
                    iced_fonts::lucide::file_text().size(14).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    text(t("session_logging")).size(13).color(OryxisColors::t().text_secondary).into(),
                    Space::new().width(Length::Fill).into(),
                    {
                        let (label_key, bg) = match self.editor_form.session_logging {
                            None => ("session_log_default", OryxisColors::t().bg_hover),
                            Some(true) => ("session_log_on", OryxisColors::t().success),
                            Some(false) => ("session_log_off", OryxisColors::t().error),
                        };
                        let fg = crate::theme::contrast_text_for(bg);
                        button(text(t(label_key)).size(12).color(fg))
                            .on_press(Message::EditorCycleSessionLogging)
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
        );

        // Per-host Privacy Mode override: Default (inherit global) / On
        // (always hide sensitive data for this host) / Off (never hide).
        let privacy_mode_selected = match self.editor_form.privacy_mode {
            Some(true) => t("host_privacy_mode_on"),
            Some(false) => t("host_privacy_mode_off"),
            None => t("host_privacy_mode_default"),
        }
        .to_string();
        let privacy_mode_options = vec![
            t("host_privacy_mode_default").to_string(),
            t("host_privacy_mode_on").to_string(),
            t("host_privacy_mode_off").to_string(),
        ];
        // Focusable select (Tab + Enter/Space, widget-owned keys).
        let row_privacy_mode: Element<'_, Message> = panel_option_row(
            iced_fonts::lucide::eye_off(),
            t("host_privacy_mode"),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-privacy-mode")),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(privacy_mode_selected), privacy_mode_options, |s: &String| s.clone())
                    .on_select(Message::EditorPrivacyModeChanged)
                    .id(iced::widget::Id::new("editor-pick-privacy-mode"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
            ),
        );

        // ── Error ──
        let panel_error: Element<'_, Message> = if let Some(err) = &self.host_panel_error {
            container(Element::from(text(err.clone()).size(11).color(OryxisColors::t().error)))
                .padding(Padding { top: 4.0, right: 16.0, bottom: 4.0, left: 16.0 })
                .into()
        } else {
            Space::new().height(0).into()
        };

        // ── Bottom actions ──
        // "Connect" (without saving) sits BEFORE Save both visually
        // (leading side of the row) and in the panel-nav recording, so
        // the keyboard walk matches the layout. Built lazily below;
        // `save_btn` is a closure so the nav slot records the two
        // buttons in visual order.
        let save_btn_bg = if has_address { OryxisColors::t().accent } else { OryxisColors::t().bg_surface };
        let make_save_btn = |app: &Self| -> Element<'_, Message> {
            app.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::EditorSave),
                8.0,
                button(
                    container(text(crate::i18n::t("save")).size(14).color(OryxisColors::t().text_primary))
                        .padding(Padding { top: 12.0, right: 0.0, bottom: 12.0, left: 0.0 })
                        .width(Length::Fill)
                        .center_x(Length::Fill),
                )
                .on_press(Message::EditorSave)
                .width(Length::Fill)
                .style(move |_, _| button::Style {
                    background: Some(Background::Color(save_btn_bg)),
                    border: Border { radius: Radius::from(8.0), ..Default::default() },
                    ..Default::default()
                })
                .into(),
            )
        };

        // "Connect" (without saving): quick-connect straight from the form,
        // new-host flow only (an existing host already has a card to
        // connect from, and its stored secrets would not ride along).
        // Sits beside Save; the short label gets a tooltip spelling out
        // that nothing is written to the vault.
        let actions_row: Element<'_, Message> = if self.editor_form.editing_id.is_none() {
            let connect_btn = self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::EditorConnectWithoutSaving),
                8.0,
                button(
                    container(
                        text(crate::i18n::t("connect"))
                            .size(14)
                            .color(if has_address {
                                OryxisColors::t().text_primary
                            } else {
                                OryxisColors::t().text_muted
                            }),
                    )
                    .padding(Padding { top: 12.0, right: 0.0, bottom: 12.0, left: 0.0 })
                    .width(Length::Fill)
                    .center_x(Length::Fill),
                )
                .on_press(Message::EditorConnectWithoutSaving)
                .width(Length::Fill)
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered | button::Status::Pressed => {
                            OryxisColors::t().bg_hover
                        }
                        _ => OryxisColors::t().bg_surface,
                    };
                    button::Style {
                        background: Some(Background::Color(bg)),
                        border: Border {
                            radius: Radius::from(8.0),
                            width: 1.0,
                            color: OryxisColors::t().border,
                        },
                        ..Default::default()
                    }
                })
                .into(),
            );
            let connect_btn: Element<'_, Message> = iced::widget::tooltip(
                connect_btn,
                container(
                    text(crate::i18n::t("quick_connect_not_saved"))
                        .size(11)
                        .color(OryxisColors::t().text_primary),
                )
                .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                .style(|_| container::Style {
                    background: Some(Background::Color(OryxisColors::t().bg_surface)),
                    border: Border {
                        radius: Radius::from(6.0),
                        color: OryxisColors::t().border,
                        width: 1.0,
                    },
                    ..Default::default()
                }),
                iced::widget::tooltip::Position::Top,
            )
            .into();
            dir_row(vec![connect_btn, Space::new().width(8).into(), make_save_btn(self)])
                .width(Length::Fill)
                .into()
        } else {
            make_save_btn(self)
        };

        // The error must live OUTSIDE the scrollable so it sits above
        // the Save button at the bottom of the panel, otherwise long
        // forms hide it below the fold and the user clicks Save again
        // wondering why nothing happens.
        let bottom = column![panel_error, actions_row].spacing(8);

        // ── Compose one card per semantic group ──
        // Host (label / parent / connection target), SSH (everything
        // protocol-specific, including the port in its header and the
        // login/password right below it), and Terminal (appearance +
        // session logging). The SSH card is the whole protocol block, so
        // a future Telnet switch hides it in one move while keeping the
        // universal-for-Telnet bits (port, login, password) at its top.
        //
        // Spacing: GROUP_GAP (Space + divider + Space) between subgroups,
        // ROW_GAP between rows. No per-row dividers, so nothing hugs a
        // field.
        const GROUP_GAP: f32 = 16.0;
        const ROW_GAP: f32 = 10.0;
        let group_sep = || -> Element<'_, Message> {
            column![
                Space::new().height(GROUP_GAP),
                panel_divider(),
                Space::new().height(GROUP_GAP),
            ].into()
        };

        // Host card: label, parent group, then the connection target.
        let mut host_col = column![
            section_header(t("host")),
            Space::new().height(ROW_GAP),
            panel_field(t("label"), label_field),
            Space::new().height(ROW_GAP),
            panel_field(t("parent_group"), parent_combo),
            Space::new().height(ROW_GAP),
            panel_field(t("tags"), tags_field),
        ];
        host_col = host_col
            .push(group_sep())
            .push(section_header(t("connection")))
            .push(Space::new().height(ROW_GAP))
            .push(hostname_row);
        if let Some(pr) = protocol_row {
            host_col = host_col.push(Space::new().height(ROW_GAP)).push(pr);
        }
        if let Some(ct) = cloud_transport_row {
            host_col = host_col.push(Space::new().height(ROW_GAP)).push(ct);
        }
        let host_section = panel_section(host_col);

        // Protocol card header: "<PROTO> .......... [port] port". The
        // accent label names the active protocol so the card reads the
        // same whether it holds the full SSH block or the reduced
        // Telnet one.
        let proto_label = if is_ssh {
            t("ssh")
        } else if is_serial {
            t("serial")
        } else {
            t("telnet")
        };
        // Serial has no numeric port, so its header is just the label;
        // SSH/Telnet append the "[22] port" field.
        let proto_header = if is_serial {
            dir_row(vec![
                text(proto_label).size(14).color(OryxisColors::t().accent).into(),
                Space::new().width(Length::Fill).into(),
            ])
            .align_y(iced::Alignment::Center)
        } else {
            dir_row(vec![
                text(proto_label).size(14).color(OryxisColors::t().accent).into(),
                Space::new().width(Length::Fill).into(),
                port_input,
                Space::new().width(8).into(),
                text(t("port")).size(12).color(OryxisColors::t().text_muted).into(),
            ])
            .align_y(iced::Alignment::Center)
        };

        // Protocol card. SSH holds the full block (Credentials,
        // Authentication, Network, Integration, initial command); Telnet
        // holds only the port header, username/password credentials and
        // an honest one-line cleartext note. Everything else is SSH-only
        // and is dropped from the reduced form, not disabled.
        let protocol_section: Element<'_, Message> = if is_ssh {
            let mut ssh_col = column![proto_header]
                .push(group_sep())
                .push(section_header(t("credentials")))
                .push(Space::new().height(ROW_GAP))
                .push(cred_items)
                .push(group_sep())
                .push(section_header(t("authentication")))
                .push(Space::new().height(ROW_GAP))
                .push(row_auth_method);
            // The chosen method's field: Key shows a key picker; the other
            // methods need no extra input here (password lives in Credentials).
            if let Some(k) = ssh_key_row {
                ssh_col = ssh_col.push(Space::new().height(ROW_GAP)).push(k);
            }
            ssh_col = ssh_col.push(Space::new().height(ROW_GAP)).push(row_agent_fwd);
            // Network subgroup.
            ssh_col = ssh_col
                .push(group_sep())
                .push(section_header(t("network")))
                .push(Space::new().height(ROW_GAP))
                .push(row_chaining)
                .push(Space::new().height(ROW_GAP))
                .push(proxy_rows)
                .push(Space::new().height(ROW_GAP))
                .push(pf_items)
                .push(Space::new().height(ROW_GAP))
                .push(row_keepalive)
                .push(Space::new().height(ROW_GAP))
                .push(row_auto_title)
                .push(Space::new().height(ROW_GAP))
                .push(algo_overrides);
            // Integration subgroup + initial command.
            ssh_col = ssh_col
                .push(group_sep())
                .push(section_header(t("integration")))
                .push(Space::new().height(ROW_GAP))
                .push(row_mcp)
                .push(Space::new().height(ROW_GAP))
                .push(rd_block)
                .push(Space::new().height(ROW_GAP))
                .push(env_items)
                .push(group_sep())
                .push(startup_block);
            panel_section(ssh_col)
        } else if is_serial {
            // Serial card: the line-parameter block under the header.
            // No credentials (serial has no auth); the port path lives
            // in the Host card's connection target above.
            let serial_col = column![proto_header]
                .push(group_sep())
                .push(section_header(t("serial_line")))
                .push(Space::new().height(ROW_GAP))
                .push(serial_params_block);
            panel_section(serial_col)
        } else {
            // Telnet cleartext note: honest UX, not a lecture. The user
            // is the only party on the path without a secure option.
            let cleartext_note = dir_row(vec![
                iced_fonts::lucide::triangle_alert()
                    .size(13)
                    .color(OryxisColors::t().warning)
                    .into(),
                Space::new().width(8).into(),
                text(t("telnet_cleartext_note"))
                    .size(11)
                    .color(OryxisColors::t().text_muted)
                    .into(),
            ])
            .align_y(iced::Alignment::Center);
            let telnet_col = column![proto_header]
                .push(group_sep())
                .push(section_header(t("credentials")))
                .push(Space::new().height(ROW_GAP))
                .push(cred_items)
                .push(Space::new().height(GROUP_GAP))
                .push(cleartext_note);
            panel_section(telnet_col)
        };

        // Terminal card: appearance + session logging.
        let terminal_section = panel_section(
            column![section_header(t("terminal_settings")), Space::new().height(ROW_GAP)]
                .push(appearance_items)
                .push(Space::new().height(GROUP_GAP))
                .push(row_session_logging)
                .push(Space::new().height(GROUP_GAP))
                .push(row_privacy_mode),
        );

        // ── Layout ──
        let form_scroll = scrollable(
            column![
                host_section,
                Space::new().height(10),
                protocol_section,
                Space::new().height(10),
                terminal_section,
            ]
            .padding(Padding { top: 0.0, right: 16.0, bottom: 16.0, left: 16.0 }),
        )
        // Shared id: the keyboard router keeps the selected row in view.
        .id(iced::widget::Id::new("side-panel-scroll"))
        .height(Length::Fill);

        let panel_content = column![
            panel_header,
            form_scroll,
            container(bottom)
                .padding(Padding { top: 8.0, right: 16.0, bottom: 16.0, left: 16.0 }),
        ]
        .height(Length::Fill);

        container(panel_content)
            .width(PANEL_WIDTH)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_surface)),
                border: Border { color: OryxisColors::t().border, width: 1.0, radius: Radius::from(0.0) },
                ..Default::default()
            })
            .into()
    }

    /// Per-host legacy-algorithm overrides: one block per negotiation
    /// category (ciphers / kex / MACs / host keys). Each is `Auto` (the
    /// safe russh default, untouched) until toggled off, which reveals a
    /// checklist seeded from the defaults so the user can add the cbc /
    /// 3des / sha1 / dh-group1 entries a legacy server needs.
    fn algo_overrides_section(&self) -> Element<'_, Message> {
        use crate::state::AlgoCategory;
        use iced::widget::checkbox;
        let c = OryxisColors::t();
        let mut col = column![
            text(t("algo_overrides")).size(13).color(c.text_secondary),
            Space::new().height(2),
            text(t("algo_overrides_desc")).size(11).color(c.text_muted),
        ];
        for cat in AlgoCategory::ALL {
            let is_auto = self.editor_form.algo_list(cat).is_none();
            col = col.push(Space::new().height(10));
            // Explicit "Auto / Custom" picker per category; choosing Custom
            // reveals the algorithm checklist below. Left/Right cycle the
            // two modes from the keyboard.
            let auto_label = t("algo_auto");
            let selected = if is_auto { auto_label } else { t("algo_custom") }.to_string();
            let mode_options = vec![auto_label.to_string(), t("algo_custom").to_string()];
            let mk_mode = move |s: String| Message::EditorAlgoSetAuto(cat, s == auto_label);
            // Focusable select; the id must be unique per category so
            // Tab focuses exactly one of the four mode pickers.
            let mode_id: &'static str = match cat {
                crate::state::AlgoCategory::Cipher => "editor-pick-algo-cipher",
                crate::state::AlgoCategory::Kex => "editor-pick-algo-kex",
                crate::state::AlgoCategory::Mac => "editor-pick-algo-mac",
                crate::state::AlgoCategory::HostKey => "editor-pick-algo-hostkey",
            };
            col = col.push(panel_option_row(
                iced_fonts::lucide::shield(),
                t(cat.label_key()),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(mode_id)),
                    crate::widgets::INPUT_RADIUS,
                    pick_list(Some(selected), mode_options, |s: &String| s.clone())
                        .on_select(mk_mode)
                        .id(iced::widget::Id::new(mode_id))
                        .on_open(Message::PickOpenChanged(true))
                        .on_close(Message::PickOpenChanged(false))
                        .width(120)
                        .padding(10)
                        .style(crate::widgets::rounded_pick_list_style)
                        .into(),
                ),
            ));
            if !is_auto {
                let selected: Vec<String> =
                    self.editor_form.algo_list(cat).clone().unwrap_or_default();
                let mut checks = column![].spacing(4);
                for algo in cat.supported() {
                    let name = algo.to_string();
                    let checked = selected.iter().any(|n| n == algo);
                    // Each algorithm checkbox is its own keyboard row;
                    // Enter/Space flips it like a click.
                    checks = checks.push(self.panel_nav_slot(
                        crate::keynav::RowAction::activate(Message::EditorAlgoToggle(
                            cat,
                            name.clone(),
                        )),
                        4.0,
                        checkbox(checked)
                            .label(algo)
                            .on_toggle(move |_| Message::EditorAlgoToggle(cat, name.clone()))
                            .size(15)
                            .text_size(12)
                            .into(),
                    ));
                }
                col = col.push(container(checks).padding(Padding {
                    top: 4.0,
                    right: 0.0,
                    bottom: 4.0,
                    left: 16.0,
                }));
            }
        }
        col.into()
    }

    /// Build the Proxy rows (no card wrapper, the caller nests them in
    /// the SSH > Network subgroup). The picker mixes the static proxy
    /// types (None / SOCKS5 / SOCKS4 / HTTP / Command) with the user's
    /// saved `ProxyIdentity` entries, selecting an identity hides the
    /// inline fields and shows a readonly summary instead.
    fn build_proxy_rows(&self) -> iced::widget::Column<'_, Message> {
        let kind = self.editor_form.proxy_kind;

        // Compose the picker option list. Identity entries come from
        // `self.proxy_identities` so the user can pick any saved
        // config without leaving the host editor.
        let mut options: Vec<ProxyKind> = ProxyKind::STATIC.to_vec();
        for pi in &self.proxy_identities {
            options.push(ProxyKind::Identity(pi.id));
        }

        // Capture the identities by reference so the closure can render
        // the user-chosen label for `Identity(_)` entries instead of
        // the generic Display fallback. The borrow lives as long as
        // `self`, which covers the returned Element, so no clone of
        // the Vec is needed per render.
        let identities = &self.proxy_identities;
        // Inline row (label left, picker right) mirroring the Auth Method
        // row, so the two pickers read as the same control family. The
        // type-dependent fields still stack below.
        // Focusable select (Tab + Enter/Space, widget-owned keys).
        let picker: Element<'_, Message> = panel_option_row(
            iced_fonts::lucide::route(),
            crate::i18n::t("proxy_type"),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-proxy-kind")),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(kind), options, move |k: &ProxyKind| match k {
                    ProxyKind::Identity(id) => identities
                        .iter()
                        .find(|pi| pi.id == *id)
                        .map(|pi| format!("📌 {}", pi.label))
                        .unwrap_or_else(|| crate::i18n::t("proxy_type_identity_deleted").into()),
                    other => other.to_string(),
                })
                .on_select(Message::EditorProxyKindChanged)
                .id(iced::widget::Id::new("editor-pick-proxy-kind"))
                .on_open(Message::PickOpenChanged(true))
                .on_close(Message::PickOpenChanged(false))
                .width(140)
                .padding(10)
                .style(crate::widgets::rounded_pick_list_style)
                .into(),
            ),
        );

        let mut col = column![picker];

        // Saved-identity selection: show a small readonly summary so
        // the user can see what they picked without flipping screens.
        // The actual identity edits live under Settings → Proxies.
        if let ProxyKind::Identity(id) = kind {
            let summary = identities
                .iter()
                .find(|pi| pi.id == id)
                .map(|pi| {
                    let kind_label = match &pi.proxy_type {
                        oryxis_core::models::connection::ProxyType::Socks5 => "SOCKS5",
                        oryxis_core::models::connection::ProxyType::Socks4 => "SOCKS4",
                        oryxis_core::models::connection::ProxyType::Http => "HTTP",
                        oryxis_core::models::connection::ProxyType::Command(_) => "CMD",
                    };
                    let user_part = pi
                        .username
                        .as_deref()
                        .map(|u| format!(" ({u})"))
                        .unwrap_or_default();
                    format!("{kind_label}, {}:{}{}", pi.host, pi.port, user_part)
                })
                .unwrap_or_else(|| crate::i18n::t("proxy_type_identity_deleted").into());
            col = col.push(Space::new().height(8)).push(
                text(summary).size(12).color(OryxisColors::t().text_muted),
            );
            return col;
        }

        if kind == ProxyKind::None {
            return col;
        }

        if kind == ProxyKind::Command {
            col = col
                .push(Space::new().height(8))
                .push(panel_field(
                    crate::i18n::t("proxy_command"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-proxy-command")),
                        10.0,
                        text_input(
                            crate::i18n::t("proxy_command_placeholder"),
                            &self.editor_form.proxy_command,
                        )
                        .id(iced::widget::Id::new("editor-proxy-command"))
                        .on_input(Message::EditorProxyCommandChanged)
                        .on_submit(Message::EditorSave)
                        .padding(10)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    ),
                ));
            return col;
        }

        if kind.needs_endpoint() {
            col = col
                .push(Space::new().height(8))
                .push(panel_field(
                    crate::i18n::t("proxy_host"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-proxy-host")),
                        10.0,
                        text_input(
                            crate::i18n::t("proxy_host_placeholder"),
                            &self.editor_form.proxy_host,
                        )
                        .id(iced::widget::Id::new("editor-proxy-host"))
                        .on_input(Message::EditorProxyHostChanged)
                        .on_submit(Message::EditorSave)
                        .padding(10)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    ),
                ))
                .push(Space::new().height(8))
                .push(panel_field(
                    crate::i18n::t("proxy_port"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-proxy-port")),
                        10.0,
                        text_input("1080", &self.editor_form.proxy_port)
                            .id(iced::widget::Id::new("editor-proxy-port"))
                            .on_input(Message::EditorProxyPortChanged)
                            .on_submit(Message::EditorSave)
                            .padding(6)
                            .width(70)
                            .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                            .into(),
                    ),
                ))
                .push(Space::new().height(8))
                .push(panel_field(
                    crate::i18n::t("proxy_username"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-proxy-username")),
                        10.0,
                        text_input(
                            crate::i18n::t("proxy_username_placeholder"),
                            &self.editor_form.proxy_username,
                        )
                        .id(iced::widget::Id::new("editor-proxy-username"))
                        .on_input(Message::EditorProxyUsernameChanged)
                        .on_submit(Message::EditorSave)
                        .padding(10)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                    ),
                ));
        }

        if kind.supports_password() {
            // Mirror the main connection-password UX: show a hint when
            // the encrypted column already holds a value, and let the
            // user clear or replace it via the touched flag.
            let placeholder: &str = if self.editor_form.has_existing_proxy_password
                && !self.editor_form.proxy_password_touched
            {
                crate::i18n::t("proxy_password_existing")
            } else {
                crate::i18n::t("proxy_password_placeholder")
            };
            col = col
                .push(Space::new().height(8))
                .push(panel_field(
                    crate::i18n::t("proxy_password"),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::input(iced::widget::Id::new("editor-proxy-password")),
                        10.0,
                        crate::widgets::password_input_with_eye_id(
                            placeholder,
                            &self.editor_form.proxy_password,
                            Message::EditorProxyPasswordChanged,
                            Some(Message::EditorSave),
                            self.revealed_secrets
                                .contains(&crate::state::SecretField::ProxyPassword),
                            Message::ToggleSecretVisibility(
                                crate::state::SecretField::ProxyPassword,
                            ),
                            10.0,
                            Some(iced::widget::Id::new("editor-proxy-password")),
                        ),
                    ),
                ));
        }

        col
    }
}

/// Muted section title used to head each card in the host editor
/// (General / Connection / Credentials / Authentication / ...). Keeps
/// the cards visually labeled so the form reads as semantic groups.
fn section_header<'a>(label: &'a str) -> Element<'a, Message> {
    text(label).size(12).color(OryxisColors::t().text_muted).into()
}

/// Full-width "click to open the theme picker" tile, painted in a
/// terminal palette: `label` in the theme foreground, ANSI swatches on
/// the trailing edge, the theme background as the fill. Used for both a
/// chosen per-host theme and the "use global" state (where it previews
/// the inherited global theme).
fn terminal_theme_trigger<'a>(
    palette: oryxis_terminal::TerminalPalette,
    label: String,
) -> Element<'a, Message> {
    let bg = palette.background;
    let fg = palette.foreground;
    let swatches: Vec<Element<'a, Message>> = [1usize, 2, 3, 4, 5, 6]
        .iter()
        .map(|&i| {
            let color = palette.ansi[i];
            container(
                Space::new()
                    .width(Length::Fixed(10.0))
                    .height(Length::Fixed(10.0)),
            )
            .style(move |_| container::Style {
                background: Some(Background::Color(color)),
                border: Border { radius: Radius::from(5.0), ..Default::default() },
                ..Default::default()
            })
            .into()
        })
        .collect();
    button(
        container(
            dir_row(vec![
                text(label).size(13).color(fg).into(),
                Space::new().width(Length::Fill).into(),
                iced::widget::Row::with_children(swatches).spacing(4).into(),
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 10.0, right: 12.0, bottom: 10.0, left: 12.0 })
        .width(Length::Fill),
    )
    .on_press(Message::EditorOpenThemePicker)
    .padding(0)
    .width(Length::Fill)
    .style(move |_, _| button::Style {
        background: Some(Background::Color(bg)),
        border: Border { radius: Radius::from(8.0), ..Default::default() },
        ..Default::default()
    })
    .into()
}
