//! Keys screen + identity panel + SSH key import panel.

use iced::border::Radius;
use iced::widget::{
    button, column, container, pick_list, scrollable, text, text_editor, text_input, MouseArea,
    Space,
};
use iced::widget::button::Status as BtnStatus;
use iced::{Background, Border, Color, Element, Length, Padding};

use oryxis_core::models::connection::Connection;
use oryxis_core::models::identity::Identity;
use oryxis_core::models::key::SshKey;

use crate::app::{Message, Oryxis, CARD_WIDTH};
use crate::i18n::t;
use crate::theme::OryxisColors;
use crate::widgets::{
    card_grid_columns, dir_align_x, dir_row, distribute_card_grid,
};

impl Oryxis {
    pub(crate) fn view_keys(&self) -> Element<'_, Message> {
        // ── Header toolbar ──
        // Split button: leading half "+ ADD" (opens menu), vertical
        // separator, trailing half "▼" chevron (also opens menu). Both
        // halves invoke the same toggle so the dropdown appears below
        // regardless of which half the user clicks. The leading half
        // gets its outer corners rounded; under RTL `dir_row` swaps the
        // order, so we also swap which physical corners each half
        // rounds, otherwise the rounded edge ends up in the middle.
        let rtl = crate::i18n::is_rtl_layout();
        let label_radius = if rtl {
            // Label sits on the right edge in RTL → round right corners.
            Radius { top_left: 0.0, bottom_left: 0.0, top_right: 6.0, bottom_right: 6.0 }
        } else {
            Radius { top_left: 6.0, bottom_left: 6.0, top_right: 0.0, bottom_right: 0.0 }
        };
        let chevron_radius = if rtl {
            Radius { top_left: 6.0, bottom_left: 6.0, top_right: 0.0, bottom_right: 0.0 }
        } else {
            Radius { top_left: 0.0, bottom_left: 0.0, top_right: 6.0, bottom_right: 6.0 }
        };

        let add_label = button(
            container(
                dir_row(vec![
                    text("+").size(13).font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                    }).color(OryxisColors::t().button_text).into(),
                    Space::new().width(4).into(),
                    text(t("add_btn")).size(11).font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                    }).color(OryxisColors::t().button_text).into(),
                ])
                .align_y(iced::Alignment::Center),
            )
            .center_y(Length::Fixed(24.0))
            .center_x(Length::Fixed(72.0)),
        )
        .on_press(Message::ToggleKeychainAddMenu)
        .style(move |_, status| {
            let bg = match status {
                BtnStatus::Hovered => OryxisColors::t().button_bg_hover,
                _ => OryxisColors::t().button_bg,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: label_radius,
                    ..Default::default()
                },
                ..Default::default()
            }
        });

        let separator = container(Space::new().width(1).height(16))
            .style(|_| container::Style {
                background: Some(Background::Color(Color { a: 0.3, ..Color::BLACK })),
                ..Default::default()
            });

        // Chevron half, match the left half's vertical metrics so both halves
        // render at identical heights. Lateral padding is kept to the minimum
        // that still gives the glyph breathing room.
        let add_chevron = button(
            container(
                iced_fonts::lucide::chevron_down::<iced::Theme, iced::Renderer>()
                    .size(12).color(OryxisColors::t().button_text),
            )
            .center_y(Length::Fixed(24.0))
            .padding(Padding { top: 0.0, right: 4.0, bottom: 0.0, left: 4.0 }),
        )
        .on_press(Message::ToggleKeychainAddMenu)
        .style(move |_, status| {
            let bg = match status {
                BtnStatus::Hovered => OryxisColors::t().button_bg_hover,
                _ => OryxisColors::t().button_bg,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: chevron_radius,
                    ..Default::default()
                },
                ..Default::default()
            }
        });

        // Report the split group's rect so the ADD dropdown anchors to
        // the real button (2 px below, trailing edges aligned) in every
        // layout, vertical rail included.
        let add_btn: Element<'_, Message> = crate::widgets::bounds_reporter(
            dir_row(vec![
                add_label.into(),
                separator.into(),
                add_chevron.into(),
            ])
            .align_y(iced::Alignment::Center),
            self.toolbar_split_btn_bounds.clone(),
        );

        let sort_btn = crate::widgets::bounds_reporter(
            crate::widgets::sort_toolbar_button(
                crate::state::SortMenuKind::Keys,
                self.keys_sort,
            ),
            self.toolbar_sort_btn_bounds.clone(),
        );

        // Responsive collapse: search yields first, then folds to an
        // icon; when the buttons can't fit they all move into a `…` menu.
        // `keynav_toolbar_slot` records each rendered action for the
        // keyboard router (push order == visual order here).
        let (search_collapsed, buttons_overflow) = self.toolbar_tiers();
        self.keynav_toolbar_reset();
        let search_slot = self.vault_search_slot(search_collapsed);
        let mut row_items: Vec<Element<'_, Message>> = vec![
            if search_collapsed {
                self.keynav_toolbar_slot(crate::keynav::ToolbarItem::SearchIcon, search_slot)
            } else {
                search_slot
            },
            Space::new().width(10).into(),
        ];
        if buttons_overflow {
            // The split/sort triggers are off screen: blank their
            // anchor cells so the menus fall back cleanly.
            self.keynav_toolbar_zero_trigger_bounds();
            row_items.push(self.keynav_toolbar_slot(
                crate::keynav::ToolbarItem::Overflow,
                crate::widgets::bounds_reporter(
                    crate::widgets::toolbar_overflow_icon(matches!(
                        self.overlay.as_ref().map(|o| &o.content),
                        Some(crate::state::OverlayContent::ToolbarOverflow)
                    )),
                    self.toolbar_overflow_btn_bounds.clone(),
                ),
            ));
        } else {
            row_items.push(self.keynav_toolbar_slot(crate::keynav::ToolbarItem::Sort, sort_btn));
            row_items.push(Space::new().width(8).into());
            // Both split halves open the same add menu; one keynav stop.
            row_items
                .push(self.keynav_toolbar_slot(crate::keynav::ToolbarItem::Primary, add_btn));
        }
        let toolbar = container(dir_row(row_items).align_y(iced::Alignment::Center))
            .padding(Padding { top: 16.0, right: 24.0, bottom: 16.0, left: 24.0 })
            .width(Length::Fill);

        // ── Search bar ──
        // Collapses to zero height in Workspace mode where the search
        // lives on the contextual sub-nav (`view_vault_sub_nav`),
        // matching the host-grid / snippets / history treatment.
        // Search now lives in the toolbar (`vault_search_field`); the
        // legacy below-toolbar search bar collapses to nothing.
        let search_bar: Element<'_, Message> = Space::new().height(0).into();

        // ── Status message ──
        // While the import / identity sidebars are open, the panel surfaces
        // its own error/success right next to the field that caused it
        // duplicating the message in the main keychain area is just noise.
        let panel_open =
            self.show_key_panel || self.show_identity_panel || self.show_key_generate_panel;
        let status: Element<'_, Message> = if panel_open {
            Space::new().height(0).into()
        } else if let Some(err) = &self.key_error {
            container(Element::from(text(err.clone()).size(12).color(OryxisColors::t().error)))
                .padding(Padding { top: 0.0, right: 24.0, bottom: 8.0, left: 24.0 })
                .into()
        } else if let Some(ok) = &self.key_success {
            container(Element::from(text(ok.clone()).size(12).color(OryxisColors::t().success)))
                .padding(Padding { top: 0.0, right: 24.0, bottom: 8.0, left: 24.0 })
                .into()
        } else {
            Space::new().height(0).into()
        };

        // ── Keys grid ──
        // Section title in a Fill container so it anchors to the
        // card grid's leading edge (column align_x can push a
        // shrink-fit text past the card border otherwise).
        let section_title = container(
            container(
                text(t("keys_section")).size(14).color(OryxisColors::t().text_muted),
            )
            .width(Length::Fill)
            .align_x(crate::widgets::dir_align_x()),
        )
        .padding(Padding { top: 4.0, right: 0.0, bottom: 8.0, left: 0.0 });

        // Filter keys by search query. Apply the toolbar sort by
        // reordering the index list first so EditKey(idx) / DeleteKey
        // still target the canonical vault index, even though the
        // rendered order changes.
        let search_lower = self.key_search.to_lowercase();
        let mut key_order: Vec<usize> = (0..self.keys.len()).collect();
        self.keys_sort.sort_items(
            &mut key_order,
            |&i| self.keys[i].label.clone(),
            |&i| self.keys[i].created_at,
        );
        let filtered_keys: Vec<(usize, &SshKey)> = key_order
            .into_iter()
            .map(|i| (i, &self.keys[i]))
            .filter(|(_, k)| {
                search_lower.is_empty() || k.label.to_lowercase().contains(&search_lower)
            })
            .collect();

        let mut cards: Vec<Element<'_, Message>> = Vec::new();

        // The full-page empty state only applies when the whole keychain
        // is empty: a vault with identities but no SSH keys must still
        // render the identities section below (issue #70, credentials
        // looked "lost" because this early return hid them).
        if self.keys.is_empty() && self.identities.is_empty() {
            let empty_state = crate::widgets::empty_state_two(
                iced_fonts::lucide::key_round()
                    .size(32)
                    .color(OryxisColors::t().text_muted)
                    .into(),
                crate::i18n::t("add_key_title").to_string(),
                crate::i18n::t("add_key_desc").to_string(),
                (
                    crate::i18n::t("generate_key").to_string(),
                    Message::ShowKeyGeneratePanel,
                ),
                (
                    crate::i18n::t("import_key").to_string(),
                    Message::ShowKeyPanel,
                ),
            );

            // No toolbar when empty: search is hidden and the "+ Add" lives
            // in the empty-state CTA (avoids an orphaned action button).
            // Side panels are hoisted to `view_main` (active_side_panel).
            // Un-record the toolbar items registered above; none of
            // them render on this path. Same for content rows.
            self.keynav_toolbar_reset();
            self.keynav_clear_content();
            let main_content = column![search_bar, status, empty_state]
                .width(Length::Fill)
                .height(Length::Fill);
            return main_content.into();
        } else if filtered_keys.is_empty() && !self.keys.is_empty() {
            let no_results = container(
                text(t("no_keys_match")).size(13).color(OryxisColors::t().text_muted),
            )
            .padding(24)
            .width(CARD_WIDTH);
            cards.push(no_results.into());
        }

        for &(idx, key) in &filtered_keys {
            let algo = format!("{} {}", t("type_label"), key.algorithm);
            let key_style = crate::widgets::resolve_host_icon_style(
                None,
                &self.setting_default_host_icon,
            );
            let glyph_el: Element<'_, Message> = iced_fonts::lucide::key_round()
                .size(16)
                .line_height(1.0)
                .color(Color::WHITE)
                .into();
            let icon_box = crate::widgets::host_icon(
                key_style,
                OryxisColors::t().accent,
                &key.label,
                Some(glyph_el),
                32.0,
            );

            // Floating ⋮ kebab: lives in a Stack overlay on the trailing
            // corner so it doesn't take inline width. Always mounted with
            // a transparent glyph + no-hover bg when not active so the
            // surrounding MouseArea bounds stay stable.
            let key_kb_selected = self.keynav.selected_in(crate::keynav::FocusZone::Content)
                == Some(crate::keynav::NavItem::Key(idx));
            let key_show_dots = self.hovered_key_card == Some(idx)
                || self.key_context_menu == Some(idx)
                || key_kb_selected;
            let key_rtl = crate::i18n::is_rtl_layout();
            // Match the dashboard host-card geometry exactly: the host
            // card wraps its row in `container(...).padding(...)` and
            // lets the outer button add its `DEFAULT_PADDING` (5/10/5
            // /10), producing a 13/16/13/12 effective padding. Since
            // keychain cards override `button.padding()` directly,
            // they need explicit values that match that effective
            // size, otherwise they render ~10 px shorter and ~4 px
            // tighter on the leading edge than the host cards next to
            // them. Trailing stays at 24 to clear the kebab overlay.
            let card_pad_trailing = 24.0_f32;
            let card_padding = if key_rtl {
                Padding { top: 13.0, right: 12.0, bottom: 13.0, left: card_pad_trailing }
            } else {
                Padding { top: 13.0, right: card_pad_trailing, bottom: 13.0, left: 12.0 }
            };

            // Subtitle line: the algorithm, plus a type flag (B2.1 /
            // B3, Termius-style: the row reads as the key's kind). A
            // security key wins over the certificate flag when both
            // apply, it is the more load-bearing fact (signing happens
            // on the hardware token via the agent; the cert shows in
            // the editor).
            let algo_text: Element<'_, Message> = text(algo)
                .size(11)
                .color(OryxisColors::t().text_muted)
                .wrapping(iced::widget::text::Wrapping::None)
                .into();
            let flag = if key.algorithm.is_security_key() {
                Some(t("key_badge_security_key"))
            } else if key.certificate.is_some() {
                Some(t("cert_flag"))
            } else {
                None
            };
            let key_subtitle: Element<'_, Message> = if let Some(flag) = flag {
                dir_row(vec![
                    algo_text,
                    text(" · ").size(11).color(OryxisColors::t().text_muted).into(),
                    text(flag)
                        .size(11)
                        .color(OryxisColors::t().accent)
                        .wrapping(iced::widget::text::Wrapping::None)
                        .into(),
                ])
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                algo_text
            };

            let card = button(
                dir_row(vec![
                    icon_box,
                    Space::new().width(8).into(),
                    column![
                        text(&key.label)
                            .size(13)
                            .color(OryxisColors::t().text_primary)
                            .wrapping(iced::widget::text::Wrapping::None),
                        Space::new().height(2),
                        key_subtitle,
                    ]
                    .width(Length::Fill)
                    .align_x(crate::widgets::dir_align_x())
                    .clip(true)
                    .into(),
                ]).align_y(iced::Alignment::Center),
            )
            .on_press(Message::EditKey(idx))
            .padding(card_padding)
            .width(Length::Fill)
            .style(|_, status| {
                let (bg, border_color, border_width) = match status {
                    BtnStatus::Hovered => (OryxisColors::t().bg_hover, OryxisColors::t().accent, 1.5),
                    BtnStatus::Pressed => (OryxisColors::t().bg_selected, OryxisColors::t().accent, 2.0),
                    _ => (OryxisColors::t().bg_surface, OryxisColors::t().border, 1.0),
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border { radius: Radius::from(10.0), color: border_color, width: border_width },
                    ..Default::default()
                }
            });

            let key_dots_glyph_color = if key_show_dots {
                OryxisColors::t().text_muted
            } else {
                Color::TRANSPARENT
            };
            let dots_btn = crate::widgets::card_kebab_button(
                key_dots_glyph_color,
                key_show_dots,
                Message::ShowKeyMenu(idx),
            );
            let key_dots_align = if key_rtl {
                iced::alignment::Horizontal::Left
            } else {
                iced::alignment::Horizontal::Right
            };
            let key_dots_pad = if key_rtl {
                Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 8.0 }
            } else {
                Padding { top: 0.0, right: 8.0, bottom: 0.0, left: 0.0 }
            };
            let dots_overlay = container(dots_btn)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(key_dots_align)
                .align_y(iced::alignment::Vertical::Center)
                .padding(key_dots_pad);
            let card_element: Element<'_, Message> = iced::widget::Stack::new()
                .push(card)
                .push(dots_overlay)
                .into();

            // Wrap in MouseArea for right-click + hover events that
            // drive the dots-button visibility.
            let wrapped = MouseArea::new(card_element)
                .on_enter(Message::KeyCardHovered(idx))
                .on_exit(Message::KeyCardUnhovered)
                .on_right_press(Message::ShowKeyMenu(idx));

            let card_el: Element<'_, Message> =
                container(wrapped).width(Length::Fill).clip(true).into();
            let card_el = self.card_wash(card_el, OryxisColors::t().accent);
            cards.push(self.keynav_ring_content(key_kb_selected, card_el));
        }

        // Responsive grid: column count derived from the current window
        // width minus the visible chrome (left nav + optional right panel
        // + horizontal padding around the grid). When the user resizes
        // the window or opens/closes the side panel, the next view()
        // recomputes `cols` and the cards rewrap accordingly instead of
        // disappearing into clipped overflow.
        let nav_width = self.vault_rail_width();
        let panel_width = if self.show_key_panel
            || self.show_identity_panel
            || self.show_key_generate_panel
        {
            crate::app::PANEL_WIDTH
        } else {
            0.0
        };
        // 24 px of horizontal padding on each side of the grid column,
        // plus ~12 px reserved for the scrollbar gutter on the trailing
        // edge. Keep this in sync with the `padding` set on the
        // scrollable column further down.
        let available = (self.window_size.width - nav_width - panel_width - 60.0).max(0.0);
        let cols = card_grid_columns(available, CARD_WIDTH, 12.0);
        let keys_grid_elem = distribute_card_grid(cards, cols, 12.0, 12.0);

        // ── Identities section ──
        let identity_section_title = container(
            container(
                text(t("identities")).size(14).color(OryxisColors::t().text_muted),
            )
            .width(Length::Fill)
            .align_x(crate::widgets::dir_align_x()),
        )
        .padding(Padding { top: 16.0, right: 0.0, bottom: 8.0, left: 0.0 });

        let mut identity_order: Vec<usize> = (0..self.identities.len()).collect();
        self.keys_sort.sort_items(
            &mut identity_order,
            |&i| self.identities[i].label.clone(),
            |&i| self.identities[i].created_at,
        );
        let filtered_identities: Vec<(usize, &Identity)> = identity_order
            .into_iter()
            .map(|i| (i, &self.identities[i]))
            .filter(|(_, i)| {
                search_lower.is_empty() || i.label.to_lowercase().contains(&search_lower)
            })
            .collect();

        let mut identity_cards: Vec<Element<'_, Message>> = Vec::new();

        if filtered_identities.is_empty() && self.identities.is_empty() {
            // Don't show identities section at all when empty
        } else if filtered_identities.is_empty() {
            let no_results = container(
                text(t("no_identities_match")).size(13).color(OryxisColors::t().text_muted),
            )
            .padding(24)
            .width(CARD_WIDTH);
            identity_cards.push(no_results.into());
        }

        for (idx, identity) in &filtered_identities {
            let idx = *idx;
            // Build subtitle describing auth methods
            let mut parts: Vec<String> = Vec::new();
            if let Some(u) = &identity.username {
                parts.push(u.clone());
            }
            let has_pw = self.identities_with_password.contains(&identity.id);
            if has_pw {
                parts.push("\u{25CF}\u{25CF}\u{25CF}\u{25CF}".into());
            }
            if let Some(kid) = identity.key_id
                && let Some(k) = self.keys.iter().find(|k| k.id == kid) {
                    parts.push(k.label.clone());
            }
            let subtitle = if parts.is_empty() { t("no_credentials").to_string() } else { parts.join(", ") };

            let id_style = crate::widgets::resolve_host_icon_style(
                None,
                &self.setting_default_host_icon,
            );
            let id_glyph_el: Element<'_, Message> = iced_fonts::lucide::user()
                .size(16)
                .line_height(1.0)
                .color(Color::WHITE)
                .into();
            let icon_box = crate::widgets::host_icon(
                id_style,
                OryxisColors::t().accent,
                &identity.label,
                Some(id_glyph_el),
                32.0,
            );

            // Floating ⋮ kebab in a Stack overlay on the trailing corner,
            // same pattern as host / key cards.
            let id_kb_selected = self.keynav.selected_in(crate::keynav::FocusZone::Content)
                == Some(crate::keynav::NavItem::Identity(idx));
            let id_show_dots = self.hovered_identity_card == Some(idx)
                || self.identity_context_menu == Some(idx)
                || id_kb_selected;
            let id_rtl = crate::i18n::is_rtl_layout();
            // Match the host-card geometry (see key card comment
            // above): 13 top/bottom + 12 leading + 24 trailing brings
            // the identity card to the same visible footprint as the
            // host folder cards on the dashboard, fixing the "card has
            // no padding" feel (was 2 leading) and the 9-px height
            // gap to host cards (was 8 top/bottom).
            let id_pad_trailing = 24.0_f32;
            let id_card_padding = if id_rtl {
                Padding { top: 13.0, right: 12.0, bottom: 13.0, left: id_pad_trailing }
            } else {
                Padding { top: 13.0, right: id_pad_trailing, bottom: 13.0, left: 12.0 }
            };

            let card = button(
                dir_row(vec![
                    icon_box,
                    Space::new().width(8).into(),
                    column![
                        text(&identity.label)
                            .size(13)
                            .color(OryxisColors::t().text_primary)
                            .wrapping(iced::widget::text::Wrapping::None),
                        Space::new().height(2),
                        text(subtitle)
                            .size(11)
                            .color(OryxisColors::t().text_muted)
                            .wrapping(iced::widget::text::Wrapping::None),
                    ]
                    .width(Length::Fill)
                    .align_x(crate::widgets::dir_align_x())
                    .clip(true)
                    .into(),
                ]).align_y(iced::Alignment::Center),
            )
            .on_press(Message::EditIdentity(idx))
            .padding(id_card_padding)
            .width(Length::Fill)
            .style(|_, status| {
                let (bg, border_color, border_width) = match status {
                    BtnStatus::Hovered => (OryxisColors::t().bg_hover, OryxisColors::t().accent, 1.5),
                    BtnStatus::Pressed => (OryxisColors::t().bg_selected, OryxisColors::t().accent, 2.0),
                    _ => (OryxisColors::t().bg_surface, OryxisColors::t().border, 1.0),
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border { radius: Radius::from(10.0), color: border_color, width: border_width },
                    ..Default::default()
                }
            });

            let id_dots_glyph_color = if id_show_dots {
                OryxisColors::t().text_muted
            } else {
                Color::TRANSPARENT
            };
            let dots_btn = crate::widgets::card_kebab_button(
                id_dots_glyph_color,
                id_show_dots,
                Message::ShowIdentityMenu(idx),
            );
            let id_dots_align = if id_rtl {
                iced::alignment::Horizontal::Left
            } else {
                iced::alignment::Horizontal::Right
            };
            let id_dots_pad = if id_rtl {
                Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 8.0 }
            } else {
                Padding { top: 0.0, right: 8.0, bottom: 0.0, left: 0.0 }
            };
            let dots_overlay = container(dots_btn)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(id_dots_align)
                .align_y(iced::alignment::Vertical::Center)
                .padding(id_dots_pad);
            let card_element: Element<'_, Message> = iced::widget::Stack::new()
                .push(card)
                .push(dots_overlay)
                .into();

            let wrapped = MouseArea::new(card_element)
                .on_enter(Message::IdentityCardHovered(idx))
                .on_exit(Message::IdentityCardUnhovered)
                .on_right_press(Message::ShowIdentityMenu(idx));

            let id_card_el: Element<'_, Message> =
                container(wrapped).width(Length::Fill).clip(true).into();
            let id_card_el = self.card_wash(id_card_el, OryxisColors::t().accent);
            identity_cards.push(self.keynav_ring_content(id_kb_selected, id_card_el));
        }

        let identity_grid_elem = distribute_card_grid(identity_cards, cols, 12.0, 12.0);

        // Record the keyboard-navigation order as two Tab sections
        // (Keys, then Identities), both chunked to the same column
        // count the grids render with; arrows flow across both.
        {
            let cw = cols.max(1);
            let key_nav: Vec<crate::keynav::NavItem> = filtered_keys
                .iter()
                .map(|(i, _)| crate::keynav::NavItem::Key(*i))
                .collect();
            let id_nav: Vec<crate::keynav::NavItem> = filtered_identities
                .iter()
                .map(|(i, _)| crate::keynav::NavItem::Identity(*i))
                .collect();
            self.keynav_set_content_sections(vec![
                key_nav.chunks(cw).map(|c| c.to_vec()).collect(),
                id_nav.chunks(cw).map(|c| c.to_vec()).collect(),
            ]);
        }

        // Combine keys and identities into one scrollable area. Either
        // section hides entirely when its list is empty (the both-empty
        // case never reaches here, it takes the empty-state return above).
        let mut all_rows: Vec<Element<'_, Message>> = Vec::new();
        if !self.keys.is_empty() {
            all_rows.push(section_title.into());
            all_rows.push(keys_grid_elem);
        }
        if !self.identities.is_empty() {
            all_rows.push(identity_section_title.into());
            all_rows.push(identity_grid_elem);
        }

        // Right padding here also pushes the content away from the
        // scrollbar, keep it slim so the scrollbar reads as flush
        // against the panel edge rather than floating in dead space.
        // The column needs `Length::Fill` for `align_x` to have any
        // slack to align inside, without it the column shrinks to
        // content and rows hug the leading edge regardless.
        let grid = scrollable(
            column(all_rows)
                .width(Length::Fill)
                .padding(Padding { top: 0.0, right: 24.0, bottom: 24.0, left: 24.0 })
                .align_x(crate::widgets::dir_align_x()),
        )
        // Stable id so the keyboard router can keep the selected card
        // scrolled into view.
        .id(iced::widget::Id::new("keys-grid-scroll"))
        .height(Length::Fill);

        // ── Main content ──
        // Side panels (key import / identity editor) are hoisted to
        // `view_main` (active_side_panel) so they cover the sub-nav band.
        column![toolbar, search_bar, status, grid]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn view_key_import_panel(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order (row mode: Up/Down from any input).
        self.panel_nav_reset();
        let has_content = !self.key_import_form.pem.is_empty();
        // Public-only rows (B3 security keys) save with no private
        // material at all, so a filled public line also arms Save.
        let can_save =
            has_content || !self.key_import_form.public_key.trim().is_empty();
        let panel_title = if self.key_import_form.editing_id.is_some() { t("edit_key") } else { t("add_key") };

        // Panel header
        let panel_header = container(
            dir_row(vec![
                text(panel_title).size(18).color(OryxisColors::t().text_primary).into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{00D7}").size(14).color(OryxisColors::t().text_muted))
                    .on_press(Message::HideKeyPanel)
                    .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                    .style(|_, _| button::Style {
                        background: Some(Background::Color(OryxisColors::t().bg_surface)),
                        border: Border { radius: Radius::from(6.0), ..Default::default() },
                        ..Default::default()
                    }).into(),
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 20.0, right: 20.0, bottom: 16.0, left: 20.0 });

        // Name field
        let name_field = column![
            text(t("name")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(6),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-key-import-name")),
                10.0,
                text_input("my-server-key", &self.key_import_form.label)
                    .id(iced::widget::Id::new("panel-key-import-name"))
                    .on_input(Message::KeyImportLabelChanged)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                    .into(),
            ),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        // File selector: the same small "Browse..." affordance the
        // Certificate section header uses, so all three sections share
        // one visual pattern (label leading, Browse trailing).
        let browse_btn = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::BrowseKeyFile),
            6.0,
            button(text(t("cert_browse")).size(12).color(OryxisColors::t().accent))
                .on_press(Message::BrowseKeyFile)
                .padding(Padding { top: 6.0, right: 10.0, bottom: 6.0, left: 10.0 })
                .style(|_, status| {
                    let bg = match status {
                        BtnStatus::Hovered => Color { a: 0.1, ..OryxisColors::t().accent },
                        BtnStatus::Pressed => Color { a: 0.18, ..OryxisColors::t().accent },
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

        // Status indicator
        let file_status: Element<'_, Message> = if has_content {
            container(
                dir_row(vec![
                    iced_fonts::lucide::circle_check()
                        .size(13)
                        .color(OryxisColors::t().success)
                        .into(),
                    Space::new().width(6).into(),
                    text(
                        t("loaded_bytes")
                            .replacen("{bytes}", &self.key_import_form.pem.len().to_string(), 1),
                    )
                    .size(12).color(OryxisColors::t().success).into(),
                ]).align_y(iced::Alignment::Center),
            )
            .padding(Padding { top: 4.0, right: 0.0, bottom: 4.0, left: 0.0 })
            .into()
        } else {
            Space::new().height(0).into()
        };

        // Editable key content (text_editor = multi-line)
        let editor = self.panel_nav_slot(
            crate::keynav::RowAction::input(iced::widget::Id::new("panel-key-import-content")),
            10.0,
            text_editor(&self.key_import_content)
                .id(iced::widget::Id::new("panel-key-import-content"))
                .on_action(Message::KeyContentAction)
                .padding(10)
                .height(180)
                .font(iced::Font::MONOSPACE)
                .size(11)
                .style(crate::widgets::rounded_editor_style)
                .into(),
        );

        // Passphrase prompt, shown only after import_key signals the key
        // is encrypted. The hint explains the one-time-decrypt model so
        // users understand we're not storing the passphrase anywhere.
        // Recorded only when rendered, so the keyboard row appears in
        // place between the content editor and the Save button.
        let passphrase_section: Element<'_, Message> = if self.key_import_form.passphrase_required {
            // Keyboard rows: the field, then its reveal eye.
            self.panel_nav_record(crate::keynav::RowAction::input(
                iced::widget::Id::new("panel-key-import-passphrase"),
            ));
            column![
                Space::new().height(12),
                text(t("key_passphrase_label")).size(12).color(OryxisColors::t().text_secondary),
                Space::new().height(6),
                dir_row(vec![
                    iced_fonts::lucide::lock().size(13).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    crate::widgets::password_input_with_eye_nav(
                        t("key_passphrase_placeholder"),
                        &self.key_import_form.passphrase,
                        Message::KeyImportPassphraseChanged,
                        Some(Message::ImportKey),
                        self.key_import_form.passphrase_visible,
                        Message::KeyImportPassphraseToggleVisibility,
                        10.0,
                        Some(iced::widget::Id::new("panel-key-import-passphrase")),
                        |eye| self.panel_nav_slot(
                            crate::keynav::RowAction::activate(
                                Message::KeyImportPassphraseToggleVisibility,
                            ),
                            6.0,
                            eye,
                        ),
                    ),
                ]).align_y(iced::Alignment::Center),
                Space::new().height(6),
                text(t("key_passphrase_hint")).size(11).color(OryxisColors::t().text_muted),
            ]
            .width(Length::Fill)
            .align_x(dir_align_x())
            .into()
        } else {
            Space::new().height(0).into()
        };

        // Editable public-key line (B2.1, Termius parity): empty derives
        // from the private key on save; a pasted / edited line must match
        // the private key (the comment may differ, that is the point,
        // it is what the ssh-agent serves). Prefilled from `<key>.pub`
        // on browse and from the stored key on edit. A wrapping textarea
        // rather than a one-line input: OpenSSH public lines are far
        // wider than the panel.
        let public_section = column![
            Space::new().height(16),
            text(t("public_key")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(6),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-key-import-public")),
                10.0,
                text_editor(&self.key_import_public_content)
                    .id(iced::widget::Id::new("panel-key-import-public"))
                    .on_action(Message::KeyImportPublicAction)
                    .placeholder("ssh-ed25519 AAAA...")
                    .padding(10)
                    .height(72)
                    .font(iced::Font::MONOSPACE)
                    .size(11)
                    .style(crate::widgets::rounded_editor_style)
                    .into(),
            ),
            Space::new().height(4),
            text(t("public_key_auto_hint")).size(11).color(OryxisColors::t().text_muted),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        // Attached-certificate section (B2): a paste field + Browse
        // button for a signed `-cert.pub` user certificate. Optional; the
        // auto-probe on file pick prefills it and raises the hint below.
        // Keyboard rows record in build order: Browse, then the field.
        let cert_browse_btn = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::BrowseCertFile),
            6.0,
            button(text(t("cert_browse")).size(12).color(OryxisColors::t().accent))
                .on_press(Message::BrowseCertFile)
                .padding(Padding { top: 6.0, right: 10.0, bottom: 6.0, left: 10.0 })
                .style(|_, status| {
                    let bg = match status {
                        BtnStatus::Hovered => Color { a: 0.1, ..OryxisColors::t().accent },
                        BtnStatus::Pressed => Color { a: 0.18, ..OryxisColors::t().accent },
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
        let mut cert_section = column![
            Space::new().height(16),
            dir_row(vec![
                text(t("certificate")).size(12).color(OryxisColors::t().text_secondary).into(),
                Space::new().width(Length::Fill).into(),
                cert_browse_btn,
            ]).align_y(iced::Alignment::Center),
            Space::new().height(6),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-key-import-cert")),
                10.0,
                text_editor(&self.key_import_cert_content)
                    .id(iced::widget::Id::new("panel-key-import-cert"))
                    .on_action(Message::KeyImportCertAction)
                    .placeholder("ssh-ed25519-cert-v01@openssh.com AAAA...")
                    .padding(10)
                    .height(72)
                    .font(iced::Font::MONOSPACE)
                    .size(11)
                    .style(crate::widgets::rounded_editor_style)
                    .into(),
            ),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());
        if self.key_import_form.cert_detected {
            cert_section = cert_section.push(Space::new().height(6)).push(
                dir_row(vec![
                    iced_fonts::lucide::circle_check()
                        .size(12)
                        .color(OryxisColors::t().success)
                        .into(),
                    Space::new().width(6).into(),
                    text(t("cert_detected_hint")).size(11).color(OryxisColors::t().success).into(),
                ]).align_y(iced::Alignment::Center),
            );
        } else {
            cert_section = cert_section.push(Space::new().height(4)).push(
                text(t("certificate_desc")).size(11).color(OryxisColors::t().text_muted),
            );
        }

        // Shared form chrome: inline error above the footer, disabled
        // Save while there is no key content (structural gating
        // instead of the old color-only hint that still took clicks).
        let panel_error = crate::widgets::form_error(self.key_error.as_deref());
        let save_label = if self.key_import_form.editing_id.is_some() { t("update_key") } else { t("save_key") };
        let footer = crate::widgets::form_footer(
            self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::HideKeyPanel),
                6.0,
                crate::widgets::form_cancel_button(Message::HideKeyPanel),
            ),
            self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::ImportKey),
                6.0,
                crate::widgets::form_save_button(
                    save_label,
                    can_save.then_some(Message::ImportKey),
                ),
            ),
        );

        let panel_content = column![
            panel_header,
            container(
                column![
                    name_field,
                    Space::new().height(16),
                    // Section header matches the Certificate one: label
                    // leading, small Browse trailing.
                    dir_row(vec![
                        text(t("private_key"))
                            .size(12)
                            .color(OryxisColors::t().text_secondary)
                            .into(),
                        Space::new().width(Length::Fill).into(),
                        browse_btn,
                    ])
                    .align_y(iced::Alignment::Center),
                    Space::new().height(6),
                    editor,
                    Space::new().height(4),
                    file_status,
                    passphrase_section,
                    public_section,
                    cert_section,
                ]
                .width(Length::Fill)
                .align_x(dir_align_x()),
            )
            .padding(Padding { top: 0.0, right: 20.0, bottom: 0.0, left: 20.0 })
            .height(Length::Fill),
            panel_error,
            footer,
        ]
        .height(Length::Fill);

        crate::widgets::side_panel_frame(panel_content.into(), OryxisColors::t().bg_sidebar)
    }

    /// The key-generation panel (keychain > ADD > Generate key). Two
    /// screens in one surface: the spec form, then the result screen
    /// once a key was generated and saved (fingerprint + public line +
    /// copy/save/export actions). Private material never enters this
    /// view's state; export re-reads it from the vault.
    pub(crate) fn view_key_generate_panel(&self) -> Element<'_, Message> {
        use crate::state::KeyGenAlgo;

        // Keyboard rows are recorded in visual order.
        self.panel_nav_reset();
        let form = &self.key_generate_form;

        let panel_header = container(
            dir_row(vec![
                text(t("generate_key")).size(18).color(OryxisColors::t().text_primary).into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{00D7}").size(14).color(OryxisColors::t().text_muted))
                    .on_press(Message::HideKeyGeneratePanel)
                    .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                    .style(|_, _| button::Style {
                        background: Some(Background::Color(OryxisColors::t().bg_surface)),
                        border: Border { radius: Radius::from(6.0), ..Default::default() },
                        ..Default::default()
                    })
                    .into(),
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 16.0, right: 16.0, bottom: 12.0, left: 16.0 });

        let body: Element<'_, Message> = if let Some(result) = &form.result {
            // ── Result screen ──
            let public_block = container(
                text(result.public_key.clone())
                    .size(11)
                    .font(iced::Font::MONOSPACE)
                    .color(OryxisColors::t().text_secondary),
            )
            .padding(10)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_surface)),
                border: Border { radius: Radius::from(6.0), color: OryxisColors::t().border, width: 1.0 },
                ..Default::default()
            });

            let mut col = column![
                dir_row(vec![
                    iced_fonts::lucide::circle_check().size(13).color(OryxisColors::t().success).into(),
                    Space::new().width(6).into(),
                    text(t("keygen_result_saved")).size(12).color(OryxisColors::t().success).into(),
                ]).align_y(iced::Alignment::Center),
                Space::new().height(12),
                crate::widgets::panel_field(
                    t("key_fingerprint"),
                    text(result.fingerprint.clone())
                        .size(11)
                        .font(iced::Font::MONOSPACE)
                        .color(OryxisColors::t().text_secondary)
                        .into(),
                ),
                Space::new().height(12),
                crate::widgets::panel_field(t("public_key"), public_block.into()),
                Space::new().height(10),
                dir_row(vec![
                    self.panel_nav_slot(
                        crate::keynav::RowAction::activate(Message::CopyGeneratedPublicKey),
                        6.0,
                        crate::widgets::styled_button(
                            t("keygen_copy_public"),
                            Message::CopyGeneratedPublicKey,
                            OryxisColors::t().bg_selected,
                        ),
                    ),
                    Space::new().width(8).into(),
                    self.panel_nav_slot(
                        crate::keynav::RowAction::activate(Message::SaveGeneratedPublicKeyFile),
                        6.0,
                        crate::widgets::styled_button(
                            t("keygen_save_pub"),
                            Message::SaveGeneratedPublicKeyFile,
                            OryxisColors::t().bg_selected,
                        ),
                    ),
                ]),
                Space::new().height(20),
                text(t("keygen_export_private")).size(13).color(OryxisColors::t().text_primary),
                Space::new().height(6),
                text(t("keygen_export_desc")).size(11).color(OryxisColors::t().text_muted),
                Space::new().height(8),
            ];
            // Export passphrase pair; empty pair = plaintext export with
            // an explicit warning line.
            self.panel_nav_record(crate::keynav::RowAction::input(
                iced::widget::Id::new("keygen-export-pass"),
            ));
            col = col.push(crate::widgets::panel_field(
                t("keygen_export_passphrase"),
                text_input("", &form.export_passphrase)
                    .id(iced::widget::Id::new("keygen-export-pass"))
                    .on_input(Message::KeyGenExportPassphraseChanged)
                    .secure(true)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
            ));
            col = col.push(Space::new().height(8));
            self.panel_nav_record(crate::keynav::RowAction::input(
                iced::widget::Id::new("keygen-export-pass-confirm"),
            ));
            col = col.push(crate::widgets::panel_field(
                t("keygen_export_passphrase_confirm"),
                text_input("", &form.export_passphrase_confirm)
                    .id(iced::widget::Id::new("keygen-export-pass-confirm"))
                    .on_input(Message::KeyGenExportPassphraseConfirmChanged)
                    .secure(true)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
            ));
            if form.export_passphrase.is_empty() && form.export_passphrase_confirm.is_empty() {
                col = col.push(Space::new().height(6));
                col = col.push(
                    text(t("keygen_export_plaintext_warn"))
                        .size(11)
                        .color(OryxisColors::t().warning),
                );
            }
            col = col.push(Space::new().height(10));
            col = col.push(self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::ExportGeneratedPrivateKey),
                6.0,
                crate::widgets::styled_button(
                    t("keygen_export_btn"),
                    Message::ExportGeneratedPrivateKey,
                    OryxisColors::t().bg_selected,
                ),
            ));
            col.width(Length::Fill).align_x(dir_align_x()).into()
        } else {
            // ── Spec form ──
            self.panel_nav_record(crate::keynav::RowAction::input(
                iced::widget::Id::new("keygen-label"),
            ));
            let label_field = crate::widgets::panel_field(
                t("keygen_label"),
                text_input("deploy-key", &form.label)
                    .id(iced::widget::Id::new("keygen-label"))
                    .on_input(Message::KeyGenLabelChanged)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
            );

            let algo_picker = crate::widgets::panel_field(
                t("keygen_algorithm"),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("keygen-algo")),
                    10.0,
                    pick_list(
                        Some(form.algo),
                        [KeyGenAlgo::Ed25519, KeyGenAlgo::Rsa, KeyGenAlgo::Ecdsa],
                        |a: &KeyGenAlgo| a.to_string(),
                    )
                    .on_select(Message::KeyGenAlgoSelected)
                    .id(iced::widget::Id::new("keygen-algo"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            );

            // Dependent sub-picker, only for RSA / ECDSA.
            let sub_picker: Element<'_, Message> = match form.algo {
                KeyGenAlgo::Ed25519 => Space::new().height(0).into(),
                KeyGenAlgo::Rsa => column![
                    Space::new().height(12),
                    crate::widgets::panel_field(
                        t("keygen_bits"),
                        self.panel_nav_slot(
                            crate::keynav::RowAction::input(iced::widget::Id::new("keygen-bits")),
                            10.0,
                            pick_list(
                                Some(form.rsa_bits),
                                [
                                    oryxis_vault::RsaBits::B2048,
                                    oryxis_vault::RsaBits::B3072,
                                    oryxis_vault::RsaBits::B4096,
                                ],
                                |b: &oryxis_vault::RsaBits| b.to_string(),
                            )
                            .on_select(Message::KeyGenBitsSelected)
                            .id(iced::widget::Id::new("keygen-bits"))
                            .on_open(Message::PickOpenChanged(true))
                            .on_close(Message::PickOpenChanged(false))
                            .padding(10)
                            .style(crate::widgets::rounded_pick_list_style)
                            .into(),
                        ),
                    ),
                ]
                .into(),
                KeyGenAlgo::Ecdsa => column![
                    Space::new().height(12),
                    crate::widgets::panel_field(
                        t("keygen_curve"),
                        self.panel_nav_slot(
                            crate::keynav::RowAction::input(iced::widget::Id::new("keygen-curve")),
                            10.0,
                            pick_list(
                                Some(form.ecdsa_curve),
                                [
                                    oryxis_vault::EcdsaCurveChoice::P256,
                                    oryxis_vault::EcdsaCurveChoice::P384,
                                    oryxis_vault::EcdsaCurveChoice::P521,
                                ],
                                |c: &oryxis_vault::EcdsaCurveChoice| c.to_string(),
                            )
                            .on_select(Message::KeyGenCurveSelected)
                            .id(iced::widget::Id::new("keygen-curve"))
                            .on_open(Message::PickOpenChanged(true))
                            .on_close(Message::PickOpenChanged(false))
                            .padding(10)
                            .style(crate::widgets::rounded_pick_list_style)
                            .into(),
                        ),
                    ),
                ]
                .into(),
            };

            self.panel_nav_record(crate::keynav::RowAction::input(
                iced::widget::Id::new("keygen-comment"),
            ));
            let comment_field = crate::widgets::panel_field(
                t("keygen_comment"),
                text_input("user@example.com", &form.comment)
                    .id(iced::widget::Id::new("keygen-comment"))
                    .on_input(Message::KeyGenCommentChanged)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
            );

            let working: Element<'_, Message> = if form.working {
                column![
                    Space::new().height(10),
                    text(t("keygen_working")).size(12).color(OryxisColors::t().text_muted),
                ]
                .into()
            } else {
                Space::new().height(0).into()
            };

            column![
                label_field,
                Space::new().height(12),
                algo_picker,
                sub_picker,
                Space::new().height(12),
                comment_field,
                working,
            ]
            .width(Length::Fill)
            .align_x(dir_align_x())
            .into()
        };

        // Shared form chrome: error above the footer, Cancel/Generate
        // (Generate disabled while a task is in flight; the result
        // screen swaps the primary for Done).
        let panel_error = crate::widgets::form_error(form.error.as_deref());
        let footer = if form.result.is_some() {
            crate::widgets::form_footer(
                Space::new().width(0).into(),
                self.panel_nav_slot(
                    crate::keynav::RowAction::activate(Message::HideKeyGeneratePanel),
                    6.0,
                    crate::widgets::form_save_button(
                        t("done"),
                        Some(Message::HideKeyGeneratePanel),
                    ),
                ),
            )
        } else {
            crate::widgets::form_footer(
                self.panel_nav_slot(
                    crate::keynav::RowAction::activate(Message::HideKeyGeneratePanel),
                    6.0,
                    crate::widgets::form_cancel_button(Message::HideKeyGeneratePanel),
                ),
                self.panel_nav_slot(
                    crate::keynav::RowAction::activate(Message::GenerateKey),
                    6.0,
                    crate::widgets::form_save_button(
                        t("keygen_generate_btn"),
                        (!form.working).then_some(Message::GenerateKey),
                    ),
                ),
            )
        };

        let panel_content = column![
            panel_header,
            scrollable(
                container(body)
                    .padding(Padding { top: 0.0, right: 16.0, bottom: 16.0, left: 16.0 }),
            )
            // Shared id: the keyboard router keeps the selected row in view.
            .id(iced::widget::Id::new("side-panel-scroll"))
            .height(Length::Fill),
            panel_error,
            footer,
        ]
        .height(Length::Fill);

        crate::widgets::side_panel_frame(panel_content.into(), OryxisColors::t().bg_sidebar)
    }

    pub(crate) fn view_identity_panel(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order (row mode: Up/Down from any input).
        self.panel_nav_reset();
        let panel_title = if self.identity_form.editing_id.is_some() { t("edit_identity") } else { t("new_identity") };

        // Panel header
        let panel_header = container(
            dir_row(vec![
                text(panel_title).size(18).color(OryxisColors::t().text_primary).into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{00D7}").size(14).color(OryxisColors::t().text_muted))
                    .on_press(Message::HideIdentityPanel)
                    .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                    .style(|_, _| button::Style {
                        background: Some(Background::Color(OryxisColors::t().bg_surface)),
                        border: Border { radius: Radius::from(6.0), ..Default::default() },
                        ..Default::default()
                    }).into(),
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 20.0, right: 20.0, bottom: 16.0, left: 20.0 });

        // Label field
        let label_field = column![
            text(t("label")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(6),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-identity-label")),
                10.0,
                text_input(t("my_identity_placeholder"), &self.identity_form.label)
                    .id(iced::widget::Id::new("panel-identity-label"))
                    .on_input(Message::IdentityLabelChanged)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                    .into(),
            ),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        // Username field
        let username_field = column![
            text(t("username")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(6),
            dir_row(vec![
                iced_fonts::lucide::user().size(13).color(OryxisColors::t().text_muted).into(),
                Space::new().width(10).into(),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("panel-identity-username")),
                    10.0,
                    text_input("root", &self.identity_form.username)
                        .id(iced::widget::Id::new("panel-identity-username"))
                        .on_input(Message::IdentityUsernameChanged)
                        .padding(10)
                        .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                        .into(),
                ),
            ]).align_y(iced::Alignment::Center),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        // Password field with eye toggle. Keyboard row: Tab focuses the
        // inner input via its id.
        let identity_pw_placeholder: &'static str = if self.identity_form.has_existing_password
            && !self.identity_form.password_touched
        {
            "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"
        } else {
            t("password")
        };
        // Keyboard rows: the field, then its reveal eye.
        self.panel_nav_record(crate::keynav::RowAction::input(
            iced::widget::Id::new("panel-identity-password"),
        ));
        let password_field = column![
            text(t("password")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(6),
            dir_row(vec![
                iced_fonts::lucide::keyboard().size(13).color(OryxisColors::t().text_muted).into(),
                Space::new().width(10).into(),
                crate::widgets::password_input_with_eye_nav(
                    identity_pw_placeholder,
                    &self.identity_form.password,
                    Message::IdentityPasswordChanged,
                    None,
                    self.identity_form.password_visible,
                    Message::IdentityTogglePasswordVisibility,
                    10.0,
                    Some(iced::widget::Id::new("panel-identity-password")),
                    |eye| self.panel_nav_slot(
                        crate::keynav::RowAction::activate(
                            Message::IdentityTogglePasswordVisibility,
                        ),
                        6.0,
                        eye,
                    ),
                ),
            ]).align_y(iced::Alignment::Center),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        // Key selector. Focusable select: Tab reaches it, Enter/Space
        // open it, the widget owns arrows/Esc while focused.
        let key_options = {
            let mut opts = vec!["(none)".to_string()];
            opts.extend(self.keys.iter().map(|k| k.label.clone()));
            opts
        };
        let key_selected = self.identity_form.key.clone().unwrap_or_else(|| "(none)".into());
        let key_field = column![
            text(t("ssh_key")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(6),
            dir_row(vec![
                text(t("add_key_btn")).size(12).color(OryxisColors::t().accent).into(),
                Space::new().width(16).into(),
                self.panel_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("identity-pick-key")),
                    10.0,
                    pick_list(
                        Some(key_selected),
                        key_options,
                        |s: &String| s.clone(),
                    )
                    .on_select(Message::IdentityKeyChanged)
                    .id(iced::widget::Id::new("identity-pick-key"))
                    .on_open(Message::PickOpenChanged(true))
                    .on_close(Message::PickOpenChanged(false))
                    .padding(10).style(crate::widgets::rounded_pick_list_style)
                    .into(),
                ),
            ]).align_y(iced::Alignment::Center),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        // Linked connections (only when editing)
        let linked_section: Element<'_, Message> = if let Some(editing_id) = self.identity_form.editing_id {
            let linked: Vec<&Connection> = self.connections.iter()
                .filter(|c| c.identity_id == Some(editing_id))
                .collect();
            if linked.is_empty() {
                column![
                    Space::new().height(16),
                    text(t("linked_to")).size(12).color(OryxisColors::t().text_muted),
                    Space::new().height(4),
                    text(t("no_connections_identity")).size(11).color(OryxisColors::t().text_muted),
                ].into()
            } else {
                let mut items: Vec<Element<'_, Message>> = vec![
                    Space::new().height(16).into(),
                    Element::from(text(t("linked_to")).size(12).color(OryxisColors::t().text_muted)),
                    Space::new().height(4).into(),
                ];
                for conn in linked {
                    items.push(
                        container(
                            dir_row(vec![
                                iced_fonts::lucide::server().size(11).color(OryxisColors::t().text_muted).into(),
                                Space::new().width(8).into(),
                                text(&conn.label).size(12).color(OryxisColors::t().text_secondary).into(),
                            ]).align_y(iced::Alignment::Center),
                        )
                        .padding(Padding { top: 4.0, right: 0.0, bottom: 4.0, left: 0.0 })
                        .into()
                    );
                }
                column(items).into()
            }
        } else {
            Space::new().height(0).into()
        };

        // Shared form footer: disabled Save while the label is empty
        // (structural gating instead of the old color-only hint that
        // still accepted clicks), Cancel closes the panel like Esc.
        let save_label = if self.identity_form.editing_id.is_some() { crate::i18n::t("update_identity") } else { crate::i18n::t("save_identity") };
        let has_label = !self.identity_form.label.trim().is_empty();
        let footer = crate::widgets::form_footer(
            self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::HideIdentityPanel),
                6.0,
                crate::widgets::form_cancel_button(Message::HideIdentityPanel),
            ),
            self.panel_nav_slot(
                crate::keynav::RowAction::activate(Message::SaveIdentity),
                6.0,
                crate::widgets::form_save_button(
                    save_label,
                    has_label.then_some(Message::SaveIdentity),
                ),
            ),
        );

        let panel_content = column![
            panel_header,
            container(
                column![
                    label_field,
                    Space::new().height(16),
                    username_field,
                    Space::new().height(16),
                    password_field,
                    Space::new().height(16),
                    key_field,
                    linked_section,
                ]
                .width(Length::Fill)
                .align_x(dir_align_x()),
            )
            .padding(Padding { top: 0.0, right: 20.0, bottom: 0.0, left: 20.0 })
            .height(Length::Fill),
            footer,
        ]
        .height(Length::Fill);

        crate::widgets::side_panel_frame(panel_content.into(), OryxisColors::t().bg_sidebar)
    }

    /// Read-only viewer for a key's attached OpenSSH certificate (B2).
    /// Renders the parsed [`crate::state::CertViewerData`]; offers Remove
    /// (behind the standard confirm) and Close. Keynav rows record under
    /// `Modal::CertificateViewer` (Confirm family: Close is the default).
    pub(crate) fn view_cert_viewer_modal(&self) -> Element<'_, Message> {
        let Some(data) = self.cert_viewer.as_ref() else {
            return Space::new().into();
        };
        let c = OryxisColors::t();
        self.modal_nav_reset();

        // One label/value row; value in monospace for ids/fingerprints.
        let info_row = |label: String, value: String, mono: bool| -> Element<'_, Message> {
            let value_widget = text(value).size(12).color(c.text_primary);
            let value_widget = if mono { value_widget.font(iced::Font::MONOSPACE) } else { value_widget };
            column![
                text(label).size(11).color(c.text_muted),
                Space::new().height(2),
                value_widget,
            ]
            .width(Length::Fill)
            .align_x(dir_align_x())
            .into()
        };

        let mut body = column![
            dir_row(vec![
                iced_fonts::lucide::badge_check().size(16).color(c.accent).into(),
                Space::new().width(8).into(),
                container(text(&data.key_label).size(16).color(c.text_primary))
                    .width(Length::Fill)
                    .align_x(dir_align_x())
                    .into(),
            ])
            .align_y(iced::Alignment::Center),
            Space::new().height(14),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        if data.expired {
            body = body.push(
                container(
                    dir_row(vec![
                        iced_fonts::lucide::triangle_alert().size(13).color(c.error).into(),
                        Space::new().width(6).into(),
                        text(t("cert_expired_warn")).size(12).color(c.error).into(),
                    ])
                    .align_y(iced::Alignment::Center),
                )
                .padding(Padding { top: 8.0, right: 10.0, bottom: 8.0, left: 10.0 })
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(Color { a: 0.1, ..c.error })),
                    border: Border { radius: Radius::from(6.0), ..Default::default() },
                    ..Default::default()
                }),
            )
            .push(Space::new().height(12));
        }

        // Type (a full phrase) as its own line, then serial + key id.
        body = body
            .push(
                container(
                    text(t(if data.is_host { "cert_type_host" } else { "cert_type_user" }))
                        .size(12)
                        .color(c.accent),
                )
                .width(Length::Fill)
                .align_x(dir_align_x()),
            )
            .push(Space::new().height(12))
            .push(info_row(
                t("cert_serial").to_string(),
                data.serial.to_string(),
                false,
            ));
        if !data.key_id.is_empty() {
            body = body.push(Space::new().height(10)).push(info_row(
                t("cert_key_id").to_string(),
                data.key_id.clone(),
                false,
            ));
        }
        let principals = if data.principals.is_empty() {
            "*".to_string()
        } else {
            data.principals.join(", ")
        };
        body = body
            .push(Space::new().height(10))
            .push(info_row(t("cert_principals").to_string(), principals, false));
        if !data.valid_from.is_empty() {
            body = body.push(Space::new().height(10)).push(info_row(
                t("cert_valid_from").to_string(),
                data.valid_from.clone(),
                false,
            ));
        }
        let until_label = data.valid_until.clone();
        if !until_label.is_empty() {
            let until_value = text(until_label)
                .size(12)
                .color(if data.expired { c.error } else { c.text_primary });
            body = body.push(Space::new().height(10)).push(
                column![
                    text(t("cert_valid_until")).size(11).color(c.text_muted),
                    Space::new().height(2),
                    until_value,
                ]
                .width(Length::Fill)
                .align_x(dir_align_x()),
            );
        }
        body = body.push(Space::new().height(10)).push(info_row(
            t("key_ca_sha256").to_string(),
            data.ca_fingerprint.clone(),
            true,
        ));

        let buttons = dir_row(vec![
            self.modal_nav_slot(
                crate::keynav::RowAction::activate(Message::RequestRemoveKeyCertificate(data.key_idx)),
                6.0,
                false,
                crate::widgets::styled_button(t("cert_remove"), Message::RequestRemoveKeyCertificate(data.key_idx), c.error),
            ),
            Space::new().width(Length::Fill).into(),
            self.modal_nav_slot_default(
                crate::keynav::RowAction::activate(Message::CloseCertViewer),
                6.0,
                false,
                crate::widgets::styled_button(t("close"), Message::CloseCertViewer, c.accent),
            ),
        ])
        .align_y(iced::Alignment::Center);

        let card = container(
            column![body, Space::new().height(18), buttons]
                .width(Length::Fill)
                .align_x(dir_align_x()),
        )
        .width(Length::Fixed(440.0))
        .padding(24)
        .style(move |_| container::Style {
            background: Some(Background::Color(c.bg_sidebar)),
            border: Border { color: c.border, width: 1.0, radius: Radius::from(12.0) },
            ..Default::default()
        });
        card.into()
    }
}
