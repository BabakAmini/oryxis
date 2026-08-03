//! Settings -> Sync section view. Split out of views/settings/mod.rs.

use super::*;
use iced::widget::column;

impl Oryxis {
    /// Engine-state card at the top of the section. Keyboard slots
    /// record during construction, so `view_settings_sync` must call
    /// the per-card builders in on-screen order.
    fn sync_engine_state_card(&self, is_sftp: bool) -> Element<'_, Message> {
        // Enable/disable lives on the Plugins screen now.
        // Live engine state indicator, sits right under the
        // enable toggle so the user sees whether the QUIC /
        // mDNS background tasks are actually up. The SFTP
        // transport runs no background engine, so reporting
        // "Engine stopped" there would read as broken; show a
        // transport-appropriate label instead.
        let engine_state = if is_sftp {
            let (label, color) = if self.sync.enabled {
                (
                    crate::i18n::t("sftp_sync_active_label"),
                    OryxisColors::t().success,
                )
            } else {
                (
                    crate::i18n::t("sync_engine_stopped_label"),
                    OryxisColors::t().text_muted,
                )
            };
            text(label).size(11).color(color)
        } else if self.sync.engine_running {
            text(crate::i18n::t("sync_engine_running_label"))
                .size(11)
                .color(OryxisColors::t().success)
        } else {
            text(crate::i18n::t("sync_engine_stopped_label"))
                .size(11)
                .color(OryxisColors::t().text_muted)
        };

        // Master enable panel sits at the top, same shape as
        // the Enable SFTP / Enable AI panels: a single toggle
        // (with the engine state hint right under it). When
        // the master toggle is off, every other Sync panel
        // is hidden below so the surface collapses to just
        // the on/off knob.
        let enable_section: iced::widget::Column<'_, Message> = column![
            engine_state,
        ];

        panel_section(enable_section)
    }

    /// SFTP snapshot-transport config block: host, remote path,
    /// passphrase, and the group/known-host notes. Joins the
    /// Transport card right below the method picker.
    fn sync_snapshot_card(&self) -> iced::widget::Column<'_, Message> {
        // Host field opens the same rich "Select a host" modal as
        // the SFTP file browser (OS badge + label + address +
        // search), not a flat dropdown. The trigger shows the
        // current selection or a placeholder.
        let selected_conn = self
            .sync.sftp.host_id
            .and_then(|id| self.connections.iter().find(|c| c.id == id));
        let host_trigger_inner: Element<'_, Message> = if let Some(c) = selected_conn {
            dir_row(vec![
                host_badge(c, &self.prefs.default_host_icon, 22.0),
                Space::new().width(10).into(),
                text(c.label.clone())
                    .size(13)
                    .color(OryxisColors::t().text_primary)
                    .into(),
                Space::new().width(Length::Fill).into(),
                text("\u{25BE}").size(12).color(OryxisColors::t().text_muted).into(),
            ])
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            dir_row(vec![
                text(crate::i18n::t("select_a_host"))
                    .size(13)
                    .color(OryxisColors::t().text_muted)
                    .into(),
                Space::new().width(Length::Fill).into(),
                text("\u{25BE}").size(12).color(OryxisColors::t().text_muted).into(),
            ])
            .align_y(iced::Alignment::Center)
            .into()
        };
        let host_pick = self.settings_nav_slot_labeled(
            t("sftp_sync_host"),
            crate::keynav::RowAction::activate(Message::Sync(SyncMessage::SftpHostPickerOpen)),
            8.0,
            button(host_trigger_inner)
                .on_press(Message::Sync(SyncMessage::SftpHostPickerOpen))
                .padding(10)
                .width(300)
                .style(|_, status| {
                    let c = OryxisColors::t();
                    let border = match status {
                        BtnStatus::Hovered => c.accent_hover,
                        _ => c.border,
                    };
                    button::Style {
                        background: Some(Background::Color(c.bg_surface)),
                        text_color: c.text_primary,
                        border: Border {
                            radius: Radius::from(8.0),
                            width: 1.0,
                            color: border,
                        },
                        ..Default::default()
                    }
                })
                .into(),
        );
        let path_input = self.settings_nav_slot_labeled(
            t("sftp_sync_path"),
            crate::keynav::RowAction::input(iced::widget::Id::new("set-sync-sftp-path")),
            10.0,
            text_input(
                "/home/user/oryxis-sync/",
                &self.sync.sftp.remote_path,
            )
            .id(iced::widget::Id::new("set-sync-sftp-path"))
            .on_input(|v| Message::Sync(SyncMessage::SftpPathChanged(v)))
            .padding(10)
            .width(300)
            .style(crate::widgets::rounded_input_style)
            .align_x(dir_align_x())
            .into(),
        );
        let passphrase_input = self.settings_nav_slot_labeled(
            t("sftp_sync_passphrase"),
            crate::keynav::RowAction::input(iced::widget::Id::new(
                "set-sync-sftp-passphrase",
            )),
            10.0,
            text_input(
                crate::i18n::t("sftp_sync_passphrase_placeholder"),
                &self.sync.sftp.passphrase,
            )
            .id(iced::widget::Id::new("set-sync-sftp-passphrase"))
            .on_input(|v| Message::Sync(SyncMessage::SftpPassphraseChanged(v.into())))
            .secure(true)
            .padding(10)
            .width(300)
            .style(crate::widgets::rounded_input_style)
            .align_x(dir_align_x())
            .into(),
        );
        let mut sftp_section_col = column![
            text(crate::i18n::t("sftp_sync_title"))
                .size(13)
                .color(OryxisColors::t().text_primary),
            Space::new().height(8),
            panel_field(crate::i18n::t("sftp_sync_host"), host_pick),
            Space::new().height(8),
            panel_field(crate::i18n::t("sftp_sync_path"), path_input),
            Space::new().height(8),
            panel_field(
                crate::i18n::t("sftp_sync_passphrase"),
                passphrase_input,
            ),
        ];
        // (The round status lives under the Sync Now button in the
        // options panel above, not here, so feedback sits next to
        // the control that triggers it.)
        sftp_section_col = sftp_section_col
            .push(Space::new().height(12))
            .push(
                text(crate::i18n::t("sftp_sync_note_group"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            )
            .push(Space::new().height(4))
            .push(
                text(crate::i18n::t("sftp_sync_note_bridge"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            )
            .push(Space::new().height(4))
            .push(
                text(crate::i18n::t("sftp_sync_note_hostkey"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            );
        sftp_section_col
    }

    /// Transport card: the P2P-vs-SFTP picker plus, under SFTP, the
    /// snapshot config block.
    fn sync_transport_card(&self, is_sftp: bool) -> Element<'_, Message> {
        // Transport picker (P2P vs SFTP), the "one or the other"
        // choice. Always visible while sync is enabled; selecting
        // it persists the setting and (un)mounts the P2P engine.
        let p2p_label = crate::i18n::t("sync_transport_p2p").to_string();
        let sftp_label = crate::i18n::t("sync_transport_sftp").to_string();
        let transport_selected = if is_sftp {
            sftp_label.clone()
        } else {
            p2p_label.clone()
        };
        let transport_options = vec![p2p_label.clone(), sftp_label.clone()];
        // Left/Right cycle the transports without opening the
        // dropdown; same label-to-token mapping as on_select.
        let sftp_label_for_cycle = sftp_label.clone();
        let (transport_prev, transport_next) = crate::keynav::slots::cycle_pair(
            &transport_options,
            &transport_selected,
            move |v: String| {
                let tr = if v == sftp_label_for_cycle || v == "SFTP" {
                    "sftp"
                } else {
                    "p2p"
                };
                Message::Sync(SyncMessage::TransportChanged(tr.to_string()))
            },
        );
        let sftp_label_for_select = sftp_label.clone();
        let transport_pick = pick_list(
            Some(transport_selected),
            transport_options,
            |s: &String| s.clone(),
        )
        .on_select(move |v| {
            let tr = if v == sftp_label_for_select || v == "SFTP" {
                "sftp"
            } else {
                "p2p"
            };
            Message::Sync(SyncMessage::TransportChanged(tr.to_string()))
        })
        .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
        .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
        .text_size(13)
        .padding(10)
        .style(crate::widgets::rounded_pick_list_style);
        let transport_row = self.settings_nav_slot_labeled(
            t("sync_transport_field"),
            crate::keynav::RowAction::picker(transport_prev, transport_next),
            8.0,
            dir_row(vec![
                text(crate::i18n::t("sync_transport_field"))
                    .size(13)
                    .color(OryxisColors::t().text_secondary)
                    .into(),
                Space::new().width(Length::Fill).into(),
                transport_pick.into(),
            ])
            .align_y(iced::Alignment::Center)
            .into(),
        );
        let mut transport_col = column![transport_row];
        // SFTP-transport config joins the Transport card so the
        // method picker and its settings read as one theme; built
        // here so the keyboard rows record before Options renders.
        if is_sftp {
            transport_col = transport_col
                .push(Space::new().height(16))
                .push(self.sync_snapshot_card());
        }
        panel_section(transport_col)
    }

    /// Options card: the Auto/Manual mode picker, the password-sync
    /// toggle, the Sync Now / Cancel action and the status + health
    /// lines under it.
    fn sync_options_card(&self, is_sftp: bool) -> Element<'_, Message> {
        let mode_label = if self.sync.mode == "auto" { t("sync_mode_auto") } else { t("sync_mode_manual") };
        let auto_label = t("sync_mode_auto").to_string();
        let manual_label = t("sync_mode_manual").to_string();
        let mode_options = vec![auto_label.clone(), manual_label.clone()];
        let mode_selected = mode_label.to_string();
        // Left/Right cycle Auto/Manual with the same mapping as
        // the dropdown's on_select below.
        let auto_label_for_cycle = auto_label.clone();
        let (mode_prev, mode_next) = crate::keynav::slots::cycle_pair(
            &mode_options,
            &mode_selected,
            move |v: String| {
                let mode = if v == auto_label_for_cycle || v == "Auto" {
                    "auto"
                } else {
                    "manual"
                };
                Message::Sync(SyncMessage::ModeChanged(mode.to_string()))
            },
        );
        let mode_pick = pick_list(
            Some(mode_selected),
            mode_options,
            |s: &String| s.clone(),
        )
        .on_select(move |v| {
            // Compare against localized labels first; fall back
            // to English so labels persisted in another locale
            // still resolve to a known mode.
            let mode = if v == auto_label || v == "Auto" {
                "auto"
            } else {
                "manual"
            };
            Message::Sync(SyncMessage::ModeChanged(mode.to_string()))
        })
        .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
        .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
        .text_size(13)
        .padding(10)
        .style(crate::widgets::rounded_pick_list_style);
        let mode_row = self.settings_nav_slot_labeled(
            t("sync_mode"),
            crate::keynav::RowAction::picker(mode_prev, mode_next),
            8.0,
            dir_row(vec![
                text(crate::i18n::t("sync_mode")).size(13).color(OryxisColors::t().text_secondary).into(),
                Space::new().width(Length::Fill).into(),
                mode_pick.into(),
            ])
            .align_y(iced::Alignment::Center)
            .into(),
        );

        let passwords_toggle = self.nav_toggle_row(
            crate::i18n::t("sync_passwords"),
            self.sync.passwords,
            Message::Sync(SyncMessage::TogglePasswords),
        );

        let mut options_section: iced::widget::Column<'_, Message> = column![
            mode_row,
            Space::new().height(8),
            passwords_toggle,
            Space::new().height(4),
            text(crate::i18n::t("sync_passwords_desc"))
                .size(11)
                .color(OryxisColors::t().text_muted),
        ];

        if self.sync.mode == "manual" {
            if is_sftp {
                // SFTP round: relabel + disable the button while a
                // round is in flight so the click has immediate
                // feedback. There's no engine/Cancel path (the
                // transfer can't be safely aborted mid-write).
                // Recorded only while enabled, so Enter can't
                // re-fire a round already in flight.
                let (label, msg) = if self.sync.sftp.in_progress {
                    (crate::i18n::t("sftp_sync_running"), None)
                } else {
                    (crate::i18n::t("sync_now"), Some(Message::Sync(SyncMessage::Now)))
                };
                let sync_btn =
                    styled_button_opt(label, msg.clone(), OryxisColors::t().accent);
                let sync_btn: Element<'_, Message> = if let Some(m) = msg {
                    self.settings_nav_slot_labeled(
                        t("sync_now"),
                        crate::keynav::RowAction::activate(m),
                        6.0,
                        sync_btn,
                    )
                } else {
                    sync_btn
                };
                options_section =
                    options_section.push(Space::new().height(8)).push(sync_btn);
            } else {
                // P2P: swap Sync Now <-> Cancel while a sync is in
                // flight. Cancel races a oneshot against the sync
                // future in dispatch; the click drops the QUIC
                // connection immediately.
                let action_btn = if self.sync.in_progress {
                    self.settings_nav_slot_labeled(
                        t("sync_now"),
                        crate::keynav::RowAction::activate(Message::Sync(SyncMessage::CancelInProgress)),
                        6.0,
                        styled_button(
                            crate::i18n::t("sync_pairing_cancel"),
                            Message::Sync(SyncMessage::CancelInProgress),
                            OryxisColors::t().button_bg,
                        ),
                    )
                } else {
                    self.settings_nav_slot_labeled(
                        t("sync_now"),
                        crate::keynav::RowAction::activate(Message::Sync(SyncMessage::Now)),
                        6.0,
                        styled_button(
                            crate::i18n::t("sync_now"),
                            Message::Sync(SyncMessage::Now),
                            OryxisColors::t().accent,
                        ),
                    )
                };
                options_section = options_section
                    .push(Space::new().height(8))
                    .push(action_btn);
            }
        }

        // Status line directly under the action button. SFTP shows
        // its own round outcome (success muted / error red); P2P
        // keeps the engine status string.
        if is_sftp {
            if let Some(status) = &self.sync.sftp.status {
                let (txt, color) = match status {
                    Ok(s) => (s.clone(), OryxisColors::t().text_muted),
                    Err(e) => (e.clone(), OryxisColors::t().error),
                };
                options_section = options_section
                    .push(Space::new().height(8))
                    .push(text(txt).size(12).color(color));
            }
        } else if let Some(status) = &self.sync.status {
            options_section = options_section
                .push(Space::new().height(8))
                .push(text(status.as_str()).size(12).color(OryxisColors::t().text_muted));
        }

        // Persistent P2P health under the (transient) status line:
        // LAN discovery count plus the LAST signaling outcome, which
        // survives later events overwriting `status`, so "signaling
        // has been failing" stays visible instead of flashing once.
        if !is_sftp {
            options_section = options_section.push(Space::new().height(8)).push(
                text(format!(
                    "{} {}",
                    self.sync.discovered.len(),
                    crate::i18n::t("sync_health_lan"),
                ))
                .size(11)
                .color(OryxisColors::t().text_muted),
            );
            if let Some(sig) = &self.sync.signaling_last {
                let (txt, color) = match sig {
                    Ok(addr) => (
                        format!(
                            "{}: {addr}",
                            crate::i18n::t("sync_status_signaling_registered"),
                        ),
                        OryxisColors::t().text_muted,
                    ),
                    Err(reason) => (
                        format!(
                            "{}: {reason}",
                            crate::i18n::t("sync_status_signaling_failed"),
                        ),
                        OryxisColors::t().error,
                    ),
                };
                options_section = options_section
                    .push(Space::new().height(4))
                    .push(text(txt).size(11).color(color));
            }
        }

        panel_section(options_section)
    }

    /// Devices card (P2P only): the device-name field plus the
    /// pairing block (entry buttons / hosting code / join form per
    /// `sync_pairing.state`) and the paired-devices list.
    fn sync_devices_card(&self) -> Element<'_, Message> {
        // Device info
        let device_name_input = self.settings_nav_slot_labeled(
            t("sync_device_name"),
            crate::keynav::RowAction::input(iced::widget::Id::new(
                "set-sync-device-name",
            )),
            10.0,
            text_input(
                crate::i18n::t("sync_device_name_hint"),
                &self.sync.device_name,
            )
            .id(iced::widget::Id::new("set-sync-device-name"))
            .on_input(|v| Message::Sync(SyncMessage::DeviceNameChanged(v)))
            .padding(10)
            .width(300)
            .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
            .into(),
        );

        let device_col = column![
            text(crate::i18n::t("sync_device_name")).size(12).color(OryxisColors::t().text_muted),
            Space::new().height(4),
            device_name_input,
        ];

        // Pairing. The sub-view depends on `sync_pairing.state`:
        // Idle shows the two entry buttons; Hosting shows the
        // generated code; Joining shows the code + address form.
        let mut pairing_section: iced::widget::Column<'_, Message> = column![
            text(crate::i18n::t("sync_pairing")).size(13).color(OryxisColors::t().text_primary),
            Space::new().height(8),
        ];

        match self.sync.pairing.state {
            crate::state::SyncPairingState::Idle => {
                pairing_section = pairing_section.push(dir_row(vec![
                    self.settings_nav_slot_labeled(
                        t("sync_host_pairing"),
                        crate::keynav::RowAction::activate(Message::Sync(SyncMessage::StartPairing)),
                        6.0,
                        styled_button(
                            crate::i18n::t("sync_host_pairing"),
                            Message::Sync(SyncMessage::StartPairing),
                            OryxisColors::t().accent,
                        ),
                    ),
                    Space::new().width(8).into(),
                    self.settings_nav_slot_labeled(
                        t("sync_join_pairing"),
                        crate::keynav::RowAction::activate(
                            Message::Sync(SyncMessage::JoinPairingRequested),
                        ),
                        6.0,
                        styled_button(
                            crate::i18n::t("sync_join_pairing"),
                            Message::Sync(SyncMessage::JoinPairingRequested),
                            OryxisColors::t().button_bg,
                        ),
                    ),
                ]));
                // Live mDNS-discovered devices on the LAN.
                // One-click "Pair" switches to the join form
                // with the address pre-filled, so the user
                // only has to enter the 6-digit code.
                if !self.sync.discovered.is_empty() {
                    pairing_section = pairing_section
                        .push(Space::new().height(14))
                        .push(text(crate::i18n::t("sync_discovered_devices"))
                            .size(12)
                            .color(OryxisColors::t().text_secondary))
                        .push(Space::new().height(6));
                    for peer in &self.sync.discovered {
                        let label = if peer.device_name.is_empty() {
                            crate::i18n::t("sync_discovered_unnamed").to_string()
                        } else {
                            peer.device_name.clone()
                        };
                        let pair_btn = self.settings_nav_slot(
                            crate::keynav::RowAction::activate(
                                Message::Sync(SyncMessage::PairWithDiscovered(peer.device_id)),
                            ),
                            6.0,
                            styled_button(
                                crate::i18n::t("sync_pair_with_this"),
                                Message::Sync(SyncMessage::PairWithDiscovered(peer.device_id)),
                                OryxisColors::t().button_bg,
                            ),
                        );
                        pairing_section = pairing_section
                            .push(dir_row(vec![
                                text(label)
                                    .size(13)
                                    .color(OryxisColors::t().text_primary)
                                    .into(),
                                Space::new().width(8).into(),
                                text(peer.addr.to_string())
                                    .size(11)
                                    .color(OryxisColors::t().text_muted)
                                    .into(),
                                Space::new().width(Length::Fill).into(),
                                pair_btn,
                            ])
                            .align_y(iced::Alignment::Center))
                            .push(Space::new().height(4));
                    }
                }
            }
            crate::state::SyncPairingState::Hosting => {
                pairing_section = pairing_section
                    .push(text(crate::i18n::t("sync_pairing_show_code"))
                        .size(12)
                        .color(OryxisColors::t().text_secondary))
                    .push(Space::new().height(6));
                if let Some(code) = &self.sync.pairing.code {
                    pairing_section = pairing_section
                        .push(text(code.as_str())
                            .size(30)
                            .color(OryxisColors::t().success));
                }
                // Cross-network pairing block: the link + a
                // Copy button + the QR. The link works only
                // when both ends have a signaling URL set
                // (Settings > Sync > Advanced).
                if let Some(link) = &self.sync.pairing.link {
                    pairing_section = pairing_section
                        .push(Space::new().height(12))
                        .push(text(crate::i18n::t("sync_pairing_link_label"))
                            .size(12)
                            .color(OryxisColors::t().text_secondary))
                        .push(Space::new().height(4))
                        .push(text(link.as_str())
                            .size(11)
                            .color(OryxisColors::t().text_muted))
                        .push(Space::new().height(6))
                        .push(self.settings_nav_slot(
                            crate::keynav::RowAction::activate(
                                Message::CopyToClipboard(link.clone()),
                            ),
                            6.0,
                            styled_button(
                                crate::i18n::t("sync_pairing_copy_link"),
                                Message::CopyToClipboard(link.clone()),
                                OryxisColors::t().button_bg,
                            ),
                        ));
                }
                pairing_section = pairing_section
                    .push(Space::new().height(12))
                    .push(self.settings_nav_slot(
                        crate::keynav::RowAction::activate(
                            Message::Sync(SyncMessage::CancelHostingPairing),
                        ),
                        6.0,
                        styled_button(
                            crate::i18n::t("sync_pairing_cancel"),
                            Message::Sync(SyncMessage::CancelHostingPairing),
                            OryxisColors::t().button_bg,
                        ),
                    ));
            }
            crate::state::SyncPairingState::Joining => {
                let code_input = self.settings_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(
                        "set-sync-pair-code",
                    )),
                    10.0,
                    text_input(
                        crate::i18n::t("sync_pairing_code_placeholder"),
                        &self.sync.pairing.join_code_input,
                    )
                    .id(iced::widget::Id::new("set-sync-pair-code"))
                    .on_input(|v| Message::Sync(SyncMessage::JoinCodeChanged(v)))
                    .padding(8)
                    .width(280)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
                );
                let target_input = self.settings_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(
                        "set-sync-pair-target",
                    )),
                    10.0,
                    text_input(
                        crate::i18n::t("sync_pairing_target_placeholder"),
                        &self.sync.pairing.join_target_input,
                    )
                    .id(iced::widget::Id::new("set-sync-pair-target"))
                    .on_input(|v| Message::Sync(SyncMessage::JoinTargetChanged(v)))
                    .padding(8)
                    .width(320)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
                );
                let connect_btn = self.settings_nav_slot(
                    crate::keynav::RowAction::activate(Message::Sync(SyncMessage::JoinPairingConnect)),
                    6.0,
                    styled_button(
                        crate::i18n::t("sync_pairing_connect"),
                        Message::Sync(SyncMessage::JoinPairingConnect),
                        OryxisColors::t().accent,
                    ),
                );
                let cancel_btn = self.settings_nav_slot(
                    crate::keynav::RowAction::activate(Message::Sync(SyncMessage::JoinPairingCancel)),
                    6.0,
                    styled_button(
                        crate::i18n::t("sync_pairing_cancel"),
                        Message::Sync(SyncMessage::JoinPairingCancel),
                        OryxisColors::t().button_bg,
                    ),
                );
                // The link input is built after the Connect and
                // Cancel pair so the keyboard order matches the
                // on-screen order.
                let link_input = self.settings_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(
                        "set-sync-pair-link",
                    )),
                    10.0,
                    text_input(
                        crate::i18n::t("sync_pairing_link_placeholder"),
                        &self.sync.pairing.join_link_input,
                    )
                    .id(iced::widget::Id::new("set-sync-pair-link"))
                    .on_input(|v| Message::Sync(SyncMessage::JoinLinkChanged(v)))
                    .padding(8)
                    .width(360)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
                );
                let link_connect_btn = self.settings_nav_slot(
                    crate::keynav::RowAction::activate(Message::Sync(SyncMessage::JoinPairingByLink)),
                    6.0,
                    styled_button(
                        crate::i18n::t("sync_pairing_connect_with_link"),
                        Message::Sync(SyncMessage::JoinPairingByLink),
                        OryxisColors::t().accent,
                    ),
                );
                pairing_section = pairing_section
                    .push(code_input)
                    .push(Space::new().height(8))
                    .push(target_input)
                    .push(Space::new().height(10))
                    .push(dir_row(vec![
                        connect_btn,
                        Space::new().width(8).into(),
                        cancel_btn,
                    ]))
                    .push(Space::new().height(14))
                    .push(text(crate::i18n::t("sync_pairing_or_separator"))
                        .size(11)
                        .color(OryxisColors::t().text_muted))
                    .push(Space::new().height(6))
                    .push(link_input)
                    .push(Space::new().height(8))
                    .push(link_connect_btn);
            }
        }

        // Inline status banner inside the pairing card. The
        // same `sync_status` field also shows under "Sync Now"
        // in the Options card, but when the user is actively
        // pairing they're looking here, so we mirror it
        // adjacent to the form they're filling in.
        if !matches!(self.sync.pairing.state, crate::state::SyncPairingState::Idle)
            && let Some(status) = &self.sync.status
        {
            pairing_section = pairing_section
                .push(Space::new().height(8))
                .push(text(status.as_str())
                    .size(11)
                    .color(OryxisColors::t().text_muted));
        }

        // Paired devices list. Empty until the first successful
        // pairing on either side; pre-Phase B builds never
        // populated this because the engine wasn't wired.
        if !self.sync.peers.is_empty() {
            pairing_section = pairing_section
                .push(Space::new().height(14))
                .push(text(crate::i18n::t("sync_paired_devices"))
                    .size(12)
                    .color(OryxisColors::t().text_secondary))
                .push(Space::new().height(6));
            for peer in &self.sync.peers {
                let last_sync = peer.last_synced_at
                    // Stored UTC; show in the user's local timezone.
                    .map(|d| {
                        d.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|| crate::i18n::t("sync_never").into());
                let unpair = self.settings_nav_slot(
                    crate::keynav::RowAction::activate(
                        Message::Sync(SyncMessage::UnpairDevice(peer.peer_id)),
                    ),
                    4.0,
                    button(
                        text(crate::i18n::t("sync_unpair")).size(11).color(OryxisColors::t().error)
                    ).on_press(Message::Sync(SyncMessage::UnpairDevice(peer.peer_id))).style(|_, _| button::Style {
                        background: Some(Background::Color(Color::TRANSPARENT)),
                        ..Default::default()
                    })
                    .into(),
                );
                pairing_section = pairing_section.push(
                    dir_row(vec![
                        text(&peer.device_name).size(13).color(OryxisColors::t().text_primary).into(),
                        Space::new().width(Length::Fill).into(),
                        text(last_sync).size(11).color(OryxisColors::t().text_muted).into(),
                        Space::new().width(8).into(),
                        unpair,
                    ]).align_y(iced::Alignment::Center),
                ).push(Space::new().height(4));
            }
        }

        panel_section(
            device_col
                .push(Space::new().height(16))
                .push(pairing_section),
        )
    }

    /// "Set up your own relay" wizard: generates ready-to-paste
    /// server files for the self-hosted oryxis-relay and adopts
    /// the endpoint (signaling URL + token settings) once the
    /// reachability test passes. The in-app fast path for what
    /// SELF_HOSTING.md documents long-form.
    fn sync_relay_wizard_card(&self) -> iced::widget::Column<'_, Message> {
        let w = &self.sync.relay_wizard;
        let mut wizard_col: iced::widget::Column<'_, Message> =
            column![self.settings_nav_slot_labeled(
                t("sync_wizard_button"),
                crate::keynav::RowAction::activate(Message::Sync(SyncMessage::WizardToggle)),
                6.0,
                styled_button(
                    crate::i18n::t("sync_wizard_button"),
                    Message::Sync(SyncMessage::WizardToggle),
                    OryxisColors::t().button_bg,
                ),
            )];
        if w.open {
            wizard_col = wizard_col
                .push(Space::new().height(8))
                .push(
                    text(crate::i18n::t("sync_wizard_intro"))
                        .size(11)
                        .color(OryxisColors::t().text_muted),
                )
                .push(Space::new().height(10))
                .push(
                    text(crate::i18n::t("sync_wizard_domain"))
                        .size(12)
                        .color(OryxisColors::t().text_muted),
                )
                .push(Space::new().height(4))
                .push(self.settings_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(
                        "set-sync-wizard-domain",
                    )),
                    10.0,
                    text_input("relay.example.com", &w.domain)
                        .id(iced::widget::Id::new("set-sync-wizard-domain"))
                        .on_input(|v| Message::Sync(SyncMessage::WizardDomainChanged(v)))
                        .padding(8)
                        .width(320)
                        .style(crate::widgets::rounded_input_style)
                        .align_x(dir_align_x())
                        .into(),
                ))
                .push(Space::new().height(8))
                .push(
                    text(crate::i18n::t("sync_wizard_public_port"))
                        .size(12)
                        .color(OryxisColors::t().text_muted),
                )
                .push(Space::new().height(4))
                .push(self.settings_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new(
                        "set-sync-wizard-port",
                    )),
                    10.0,
                    text_input("443", &w.port)
                        .id(iced::widget::Id::new("set-sync-wizard-port"))
                        .on_input(|v| Message::Sync(SyncMessage::WizardPortChanged(v)))
                        .padding(8)
                        .width(120)
                        .style(crate::widgets::rounded_input_style)
                        .align_x(dir_align_x())
                        .into(),
                ))
                .push(Space::new().height(8))
                .push(
                    text(format!(
                        "{}: {}",
                        crate::i18n::t("sync_wizard_token"),
                        w.token,
                    ))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
                )
                .push(Space::new().height(6))
                .push(self.settings_nav_slot(
                    crate::keynav::RowAction::activate(
                        Message::Sync(SyncMessage::WizardRegenToken),
                    ),
                    6.0,
                    styled_button(
                        crate::i18n::t("sync_wizard_regen"),
                        Message::Sync(SyncMessage::WizardRegenToken),
                        OryxisColors::t().button_bg,
                    ),
                ));
            // Artifact format selector: three plain buttons (the
            // labels are proper nouns, identical in every locale),
            // accent marks the selected one.
            let fmt_btn = |label: &'static str,
                           fmt: crate::state::RelayWizardFormat|
             -> Element<'_, Message> {
                let selected = w.format == fmt;
                self.settings_nav_slot(
                    crate::keynav::RowAction::activate(
                        Message::Sync(SyncMessage::WizardFormatChanged(fmt)),
                    ),
                    6.0,
                    styled_button(
                        label,
                        Message::Sync(SyncMessage::WizardFormatChanged(fmt)),
                        if selected {
                            OryxisColors::t().accent
                        } else {
                            OryxisColors::t().button_bg
                        },
                    ),
                )
            };
            wizard_col = wizard_col.push(Space::new().height(12)).push(dir_row(vec![
                fmt_btn("Docker Compose", crate::state::RelayWizardFormat::Compose),
                Space::new().width(8).into(),
                fmt_btn("systemd", crate::state::RelayWizardFormat::Systemd),
                Space::new().width(8).into(),
                fmt_btn("Caddy", crate::state::RelayWizardFormat::Caddy),
            ]));
            let artifact = w.artifact();
            wizard_col = wizard_col
                .push(Space::new().height(10))
                .push(
                    text(crate::i18n::t("sync_wizard_files"))
                        .size(12)
                        .color(OryxisColors::t().text_secondary),
                )
                .push(Space::new().height(4))
                .push(
                    container(
                        text(artifact.clone())
                            .size(10)
                            .font(iced::Font::MONOSPACE)
                            .color(OryxisColors::t().text_secondary),
                    )
                    .width(Length::Fill)
                    .padding(10)
                    .style(|_| container::Style {
                        background: Some(Background::Color(
                            OryxisColors::t().bg_surface,
                        )),
                        border: Border {
                            radius: Radius::from(6.0),
                            color: OryxisColors::t().border,
                            width: 1.0,
                        },
                        ..Default::default()
                    }),
                )
                .push(Space::new().height(6))
                .push(self.settings_nav_slot(
                    crate::keynav::RowAction::activate(
                        Message::CopyToClipboard(artifact.clone()),
                    ),
                    6.0,
                    styled_button(
                        crate::i18n::t("terminal_copy"),
                        Message::CopyToClipboard(artifact),
                        OryxisColors::t().button_bg,
                    ),
                ))
                .push(Space::new().height(10))
                .push(
                    text(crate::i18n::t("sync_wizard_steps"))
                        .size(11)
                        .color(OryxisColors::t().text_muted),
                )
                .push(Space::new().height(10));
            // Test button only fires with a domain typed and no
            // probe in flight; recorded only when enabled so
            // keyboard Enter can't double-fire a probe.
            let test_msg = (!w.testing && w.base_url().is_some())
                .then_some(Message::Sync(SyncMessage::WizardTest));
            let test_btn = styled_button_opt(
                crate::i18n::t("sync_wizard_test"),
                test_msg.clone(),
                OryxisColors::t().accent,
            );
            let test_btn: Element<'_, Message> = if let Some(m) = test_msg {
                self.settings_nav_slot(
                    crate::keynav::RowAction::activate(m),
                    6.0,
                    test_btn,
                )
            } else {
                test_btn
            };
            wizard_col = wizard_col.push(test_btn);
            if let Some(result) = &w.result {
                let (txt, color) = match result {
                    Ok(()) => (
                        crate::i18n::t("sync_wizard_test_ok").to_string(),
                        OryxisColors::t().text_muted,
                    ),
                    Err(e) => (
                        format!(
                            "{}: {e}",
                            crate::i18n::t("sync_wizard_test_err"),
                        ),
                        OryxisColors::t().error,
                    ),
                };
                wizard_col = wizard_col
                    .push(Space::new().height(8))
                    .push(text(txt).size(11).color(color));
            }
        }
        wizard_col
    }

    /// Advanced card (P2P only): the signaling URL / token, relay URL
    /// and listen-port inputs, followed by the relay wizard.
    fn sync_advanced_card(&self) -> Element<'_, Message> {
        // Advanced
        let signaling_input = self.settings_nav_slot_labeled(
            t("sync_signaling_url"),
            crate::keynav::RowAction::input(iced::widget::Id::new(
                "set-sync-signaling-url",
            )),
            10.0,
            text_input("https://...", &self.sync.signaling_url)
                .id(iced::widget::Id::new("set-sync-signaling-url"))
                .on_input(|v| Message::Sync(SyncMessage::SignalingUrlChanged(v)))
                .padding(8)
                .width(300)
                .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                .into(),
        );
        // Keyboard rows (audit fix: this field recorded
        // nothing): the field, then its reveal eye.
        let signaling_token_idx = self
            .settings_nav_record_labeled(
                t("sync_signaling_token"),
                crate::keynav::RowAction::input(
                    iced::widget::Id::new("set-sync-signaling-token"),
                ),
            );
        let signaling_token_input = self.settings_nav_ring_at(
            signaling_token_idx,
            10.0,
            container(
                crate::widgets::password_input_with_eye_nav(
                    crate::i18n::t("sync_signaling_token_placeholder"),
                    &self.sync.signaling_token,
                    |v| Message::Sync(SyncMessage::SignalingTokenChanged(v.into())),
                    None,
                    self.revealed_secrets
                        .contains(&crate::state::SecretField::SyncSignalingToken),
                    Message::Settings(SettingsMessage::ToggleSecretVisibility(
                        crate::state::SecretField::SyncSignalingToken,
                    )),
                    8.0,
                    Some(iced::widget::Id::new("set-sync-signaling-token")),
                    |eye| self.settings_nav_slot(
                        crate::keynav::RowAction::activate(
                            Message::Settings(SettingsMessage::ToggleSecretVisibility(
                                crate::state::SecretField::SyncSignalingToken,
                            )),
                        ),
                        6.0,
                        eye,
                    ),
                ),
            )
            .width(300)
            .into(),
        );
        let relay_input = self.settings_nav_slot_labeled(
            t("sync_relay_url"),
            crate::keynav::RowAction::input(iced::widget::Id::new("set-sync-relay-url")),
            10.0,
            text_input(crate::i18n::t("sync_relay_optional"), &self.sync.relay_url)
                .id(iced::widget::Id::new("set-sync-relay-url"))
                .on_input(|v| Message::Sync(SyncMessage::RelayUrlChanged(v)))
                .padding(8)
                .width(300)
                .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                .into(),
        );
        let port_input = self.settings_nav_slot_labeled(
            t("sync_listen_port"),
            crate::keynav::RowAction::input(iced::widget::Id::new(
                "set-sync-listen-port",
            )),
            10.0,
            text_input("0", &self.sync.listen_port)
                .id(iced::widget::Id::new("set-sync-listen-port"))
                .on_input(|v| Message::Sync(SyncMessage::ListenPortChanged(v)))
                .padding(8)
                .width(100)
                .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                .into(),
        );

        let advanced_col = column![
            text(crate::i18n::t("sync_signaling_url")).size(12).color(OryxisColors::t().text_muted),
            Space::new().height(4),
            signaling_input,
            Space::new().height(8),
            text(crate::i18n::t("sync_signaling_token")).size(12).color(OryxisColors::t().text_muted),
            Space::new().height(4),
            signaling_token_input,
            Space::new().height(8),
            text(crate::i18n::t("sync_relay_url")).size(12).color(OryxisColors::t().text_muted),
            Space::new().height(4),
            relay_input,
            Space::new().height(8),
            text(crate::i18n::t("sync_listen_port")).size(12).color(OryxisColors::t().text_muted),
            Space::new().height(4),
            port_input,
        ];

        panel_section(
            advanced_col
                .push(Space::new().height(16))
                .push(self.sync_relay_wizard_card()),
        )
    }

    pub(crate) fn view_settings_sync(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order. Recording happens
        // at construction, so every card below is built in its
        // on-screen position and the enabled-only cards are built
        // inside the `sync.enabled` branch. Do not reorder the card
        // calls.
        self.keynav_settings_reset();

        let is_sftp = self.sync.transport == "sftp";

        let enable_card = self.sync_engine_state_card(is_sftp);

        // Plain-language primer: what sync is, that it's optional and
        // LAN-only by default (no Oryxis server), and what the user
        // must set up to sync across networks. Answers the recurring
        // "is sync required / where does my data go?" question.
        let how_section = panel_section(column![
            text(crate::i18n::t("sync_how_body"))
                .size(12)
                .color(OryxisColors::t().text_secondary),
        ]);

        let mut content_col: iced::widget::Column<'_, Message> = column![
            enable_card,
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        content_col = content_col
            .push(Space::new().height(18))
            .push(crate::widgets::settings_group_header(crate::i18n::t("sync_how_title")))
            .push(Space::new().height(8))
            .push(how_section);

        if self.sync.enabled {
            let transport_section = self.sync_transport_card(is_sftp);
            let options_section = self.sync_options_card(is_sftp);

            content_col = content_col
                .push(Space::new().height(18))
                .push(crate::widgets::settings_group_header(crate::i18n::t("sync_transport")))
                .push(Space::new().height(8))
                .push(transport_section)
                .push(Space::new().height(18))
                .push(crate::widgets::settings_group_header(crate::i18n::t("sync_options")))
                .push(Space::new().height(8))
                .push(options_section);

            if !is_sftp {
                let devices_card = self.sync_devices_card();
                let advanced_card = self.sync_advanced_card();

                content_col = content_col
                    .push(Space::new().height(18))
                    .push(crate::widgets::settings_group_header(crate::i18n::t("sync_device")))
                    .push(Space::new().height(8))
                    .push(devices_card)
                    .push(Space::new().height(18))
                    .push(crate::widgets::settings_group_header(crate::i18n::t("sync_advanced")))
                    .push(Space::new().height(8))
                    .push(advanced_card);
            }
        }
        content_col = content_col.push(Space::new().height(24));

        scrollable(
            container(content_col)
                .padding(Padding { top: 24.0, right: 24.0, bottom: 24.0, left: 24.0 }),
        )
        // Stable id so the keyboard router can keep the selected row
        // in view.
        .id(iced::widget::Id::new("settings-sync-scroll"))
        .on_scroll(|v| Message::Settings(SettingsMessage::SectionScrolled(v.relative_offset().y)))
        .height(Length::Fill)
        .into()
    }
}
