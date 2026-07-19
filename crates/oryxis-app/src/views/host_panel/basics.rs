//! Host editor: universal Host-card fields (label, parent group, tags,
//! connection target, protocol / cloud-transport pickers, numeric port).
use super::*;
use iced::widget::column;

impl Oryxis {
    pub(super) fn hp_label_field(&self) -> Element<'_, Message> {
        // ── Section: Host (label + parent group) ──
        // Built before the Connection widgets so their keyboard rows
        // record ahead of the hostname's (the assembly at the bottom
        // lays the Host card out first).
        let label_field: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::input(iced::widget::Id::new("editor-label")),
            10.0,
            text_input(t("my_server_placeholder"), &self.editor_form.label)
                .id(iced::widget::Id::new("editor-label"))
                .on_input(|v| Message::Editor(EditorMessage::EditorLabelChanged(v))).on_submit(Message::Editor(EditorMessage::EditorSave)).padding(10)
                .style(crate::widgets::rounded_input_style).align_x(dir_align_x()).into(),
        );
        label_field
    }

    pub(super) fn hp_parent_combo(&self) -> Element<'_, Message> {
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
            |v| Message::Editor(EditorMessage::EditorGroupChanged(v)),
        );
        let parent_combo: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::picker(group_prev, group_next),
            10.0,
            iced::widget::combo_box(
                &self.editor_parent_combo,
                t("group_placeholder"),
                parent_selection,
                |v| Message::Editor(EditorMessage::EditorGroupChanged(v)),
            )
            .on_input(|v| Message::Editor(EditorMessage::EditorGroupChanged(v)))
            .padding(10)
            .input_style(crate::widgets::rounded_input_style)
            .menu_style(crate::widgets::combo_menu_style)
            .width(Length::Fill)
            .into(),
        );
        parent_combo
    }

    pub(super) fn hp_tags_field(&self) -> Element<'_, Message> {
        // Tags: comma-separated free text, parsed on save. Feeds the
        // snippet sidebar's filter-by-host-tags toggle.
        let tags_field: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::input(iced::widget::Id::new("editor-tags")),
            10.0,
            text_input(t("tags_placeholder"), &self.editor_form.tags_text)
                .id(iced::widget::Id::new("editor-tags"))
                .on_input(|v| Message::Editor(EditorMessage::EditorTagsChanged(v)))
                .on_submit(Message::Editor(EditorMessage::EditorSave))
                .padding(10)
                .style(crate::widgets::rounded_input_style)
                .align_x(dir_align_x())
                .into(),
        );
        tags_field
    }

    pub(super) fn hp_hostname_row(&self, is_serial: bool) -> Element<'_, Message> {
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
                crate::keynav::RowAction::activate(Message::Tabs(TabsMessage::ShowIconPicker(id))),
                8.0,
                button(
                    container(addr_glyph.view(18.0, Color::WHITE))
                        .width(Length::Fixed(32.0))
                        .height(Length::Fixed(32.0))
                        .center_x(Length::Fixed(32.0))
                        .center_y(Length::Fixed(32.0)),
                )
                .on_press(Message::Tabs(TabsMessage::ShowIconPicker(id)))
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
                    .on_input(|v| Message::Editor(EditorMessage::EditorHostnameChanged(v)))
                    .on_submit(Message::Editor(EditorMessage::EditorSave))
                    .padding(10)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x()).into(),
            ),
        ]).align_y(iced::Alignment::Center).into();
        hostname_row
    }

    pub(super) fn hp_protocol_row(&self, is_rd: bool) -> Option<Element<'_, Message>> {
        use oryxis_core::models::connection::ConnectionProtocol as Proto;
        // Protocol picker (Connection). A cloud-imported host has its own
        // transport picker below and is always SSH-family, so the two are
        // mutually exclusive: hide the protocol picker on cloud hosts. A
        // remote-desktop host is a distinct kind created via "Add remote
        // desktop" (not converted from SSH), so hide it there too.
        let protocol_row: Option<Element<'_, Message>> = if self.editor_form.cloud_transport
            .is_some()
            || is_rd
        {
            None
        } else {
            let options = vec![Proto::Ssh, Proto::Telnet, Proto::Serial];
            let picker = self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-protocol")),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(self.editor_form.protocol), options, |p| p.to_string())
                    .on_select(|v| Message::Editor(EditorMessage::EditorProtocolChanged(v)))
                    .id(iced::widget::Id::new("editor-pick-protocol"))
                    .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
                    .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
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
        protocol_row
    }

    pub(super) fn hp_cloud_transport_row(&self) -> Option<Element<'_, Message>> {
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
                    .on_select(|v| Message::Editor(EditorMessage::EditorCloudTransportChanged(v)))
                    .id(iced::widget::Id::new("editor-pick-cloud-transport"))
                    .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
                    .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
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
        cloud_transport_row
    }

    pub(super) fn hp_port_input(&self, is_serial: bool) -> Element<'_, Message> {
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
                    .on_input(|v| Message::Editor(EditorMessage::EditorPortChanged(v)))
                    .on_submit(Message::Editor(EditorMessage::EditorSave))
                    .padding(6)
                    .width(56)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x()).into(),
            )
        };
        port_input
    }
}
