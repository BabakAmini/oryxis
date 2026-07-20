//! Host editor / connection editor side panel.

use iced::border::Radius;
use iced::widget::{button, column, container, pick_list, scrollable, text, text_editor, text_input, Space};
use iced::widget::button::Status as BtnStatus;
use iced::{Background, Border, Color, Element, Length, Padding};

use oryxis_core::models::connection::AuthMethod;
use oryxis_core::models::identity::Identity;

use crate::app::{SettingsMessage, TabsMessage, EditorMessage, KeysMessage, NavigationMessage, Message, Oryxis};
use crate::i18n::t;
use crate::state::ProxyKind;
use crate::theme::OryxisColors;
use crate::widgets::{
    dir_align_x, dir_row, panel_divider, panel_field, panel_option_row,
    panel_section,
};

const GROUP_GAP: f32 = 16.0;
const ROW_GAP: f32 = 10.0;

mod auth;
mod basics;
mod credentials;
mod footer;
mod integration;
mod network;
mod terminal_settings;

/// Empty placeholder element for gated-off (hidden) rows: the reduced
/// Telnet / Serial / RemoteDesktop forms drop the SSH-only widgets, and
/// because `panel_nav_slot` records at build time an ungated build would
/// record invisible Tab targets. Each gated builder resolves to this.
fn empty<'a>() -> Element<'a, Message> {
    Space::new().height(0).into()
}

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
        // Remote desktop: the endpoint (host/port), a login (username /
        // password), a kind (RDP/VNC) and an optional SSH gateway. All the
        // SSH-only rows below are `is_ssh`-gated, so they drop for free.
        let is_rd = self.editor_form.protocol == Proto::RemoteDesktop;
        // Telnet dials TCP too, so it shares the IP-version row below.
        let is_telnet = self.editor_form.protocol == Proto::Telnet;
        // ── Header ──
        // The close (×) is intentionally not a keyboard row: Esc already
        // owns panel close, and recording it would make the header the
        // first Down target instead of the form.
        let panel_header = container(
            dir_row(vec![
                text(title).size(16).color(OryxisColors::t().text_primary).into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{00D7}").size(20).color(OryxisColors::t().text_muted))
                    .on_press(Message::Editor(EditorMessage::EditorCancel))
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

        // Host-card fields, then the protocol block (Credentials /
        // Serial / Authentication / Network / Integration) and the
        // Terminal card. Built in visual order so `panel_nav_slot`
        // records the keyboard rows in the same order they render.
        let label_field = self.hp_label_field();
        let parent_combo = self.hp_parent_combo();
        let tags_field = self.hp_tags_field();
        let hostname_row = self.hp_hostname_row(is_serial);
        let protocol_row = self.hp_protocol_row(is_rd);
        let cloud_transport_row = self.hp_cloud_transport_row();
        let port_input = self.hp_port_input(is_serial);
        let cred_items = self.hp_cred_items(is_serial, is_ssh);
        let serial_params_block = self.hp_serial_params_block(is_serial);
        let row_auth_method = self.hp_row_auth_method(is_ssh);
        let ssh_key_row = self.hp_ssh_key_row(is_ssh);
        let row_agent_fwd = self.hp_row_agent_fwd(is_ssh);
        let row_chaining = self.hp_row_chaining(is_ssh);
        let proxy_rows: Element<'_, Message> = if is_ssh {
            self.build_proxy_rows().into()
        } else {
            empty()
        };
        let pf_items = self.hp_pf_items(is_ssh);
        let row_keepalive = self.hp_row_keepalive(is_ssh);
        let row_address_family = self.hp_row_address_family(is_ssh, is_telnet);
        let row_auto_title = self.hp_row_auto_title(is_ssh);
        let algo_overrides: Element<'_, Message> = if is_ssh {
            self.algo_overrides_section()
        } else {
            empty()
        };
        let row_mcp = self.hp_row_mcp(is_ssh);
        let row_monitor = self.hp_row_monitor(is_ssh);
        let rd_block = self.hp_rd_block(is_rd);
        let env_items = self.hp_env_items(is_ssh);
        let startup_block = self.hp_startup_block(is_ssh);
        let appearance_items = self.hp_appearance_items();
        let row_session_logging = self.hp_row_session_logging();
        let row_privacy_mode = self.hp_row_privacy_mode();
        // C5 Advanced-terminal block: legacy keyboard modes + feature
        // toggles. Terminal protocols only (SSH / Telnet / Serial); an
        // RDP/VNC host drives no terminal pane, so it drops out.
        let advanced_terminal: Element<'_, Message> = if is_rd {
            Space::new().height(0).into()
        } else {
            self.hp_advanced_terminal_items()
        };
        // ── Error ──
        let panel_error: Element<'_, Message> = if let Some(err) = &self.host_panel_error {
            container(Element::from(text(err.clone()).size(11).color(OryxisColors::t().error)))
                .padding(Padding { top: 4.0, right: 16.0, bottom: 4.0, left: 16.0 })
                .into()
        } else {
            Space::new().height(0).into()
        };
        let actions_row = self.hp_actions_row(has_address);
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
        } else if is_rd {
            t("remote_desktop")
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
                .push(row_address_family)
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
                .push(row_monitor)
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
        } else if is_rd {
            // Remote-desktop card: the endpoint login (Credentials) plus the
            // kind + SSH gateway rows. No SSH auth/network/integration.
            let rd_col = column![proto_header]
                .push(group_sep())
                .push(section_header(t("credentials")))
                .push(Space::new().height(ROW_GAP))
                .push(cred_items)
                .push(group_sep())
                .push(section_header(t("remote_desktop")))
                .push(Space::new().height(ROW_GAP))
                .push(rd_block);
            panel_section(rd_col)
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
                .push(Space::new().height(ROW_GAP))
                // Built after the credential rows, so it records after
                // them and must render after them too (keynav order).
                .push(row_address_family)
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
                .push(row_privacy_mode)
                .push(Space::new().height(GROUP_GAP))
                .push(advanced_terminal),
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

        crate::widgets::side_panel_frame(panel_content.into(), OryxisColors::t().bg_surface)
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
    .on_press(Message::Editor(EditorMessage::EditorOpenThemePicker))
    .padding(0)
    .width(Length::Fill)
    .style(move |_, _| button::Style {
        background: Some(Background::Color(bg)),
        border: Border { radius: Radius::from(8.0), ..Default::default() },
        ..Default::default()
    })
    .into()
}
