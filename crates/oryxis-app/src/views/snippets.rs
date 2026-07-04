//! Snippets (saved commands) list and editor panel.

use iced::border::Radius;
use iced::widget::{
    button, column, container, scrollable, text, text_editor, text_input, MouseArea, Space,
};
use iced::widget::button::Status as BtnStatus;
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::app::{Message, Oryxis, CARD_WIDTH, PANEL_WIDTH};
use crate::i18n::t;
use crate::theme::OryxisColors;
use crate::widgets::{card_grid_columns, dir_align_x, dir_row, distribute_card_grid};

impl Oryxis {
    pub(crate) fn view_snippets(&self) -> Element<'_, Message> {
        let sort_btn = crate::widgets::sort_toolbar_button(
            crate::state::SortMenuKind::Snippets,
            self.snippets_sort,
        );
        let primary: Element<'_, Message> = {
            let fg = OryxisColors::t().button_text;
            button(
                container(
                    dir_row(vec![
                        text("+").size(13).font(iced::Font {
                            weight: iced::font::Weight::Bold,
                            ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                        }).color(fg).into(),
                        Space::new().width(4).into(),
                        text(t("snippet_btn")).size(11).font(iced::Font {
                            weight: iced::font::Weight::Bold,
                            ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                        }).color(fg).into(),
                    ]).align_y(iced::Alignment::Center),
                )
                .center_y(Length::Fixed(24.0))
                .center_x(Length::Fixed(72.0)),
            )
            .on_press(Message::ShowSnippetPanel)
            .style(|_, status| {
                let bg = match status {
                    BtnStatus::Hovered => OryxisColors::t().button_bg_hover,
                    _ => OryxisColors::t().button_bg,
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border { radius: Radius::from(6.0), ..Default::default() },
                    ..Default::default()
                }
            }).into()
        };
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
            row_items.push(self.keynav_toolbar_slot(
                crate::keynav::ToolbarItem::Overflow,
                crate::widgets::toolbar_overflow_icon(matches!(
                    self.overlay.as_ref().map(|o| &o.content),
                    Some(crate::state::OverlayContent::ToolbarOverflow)
                )),
            ));
        } else {
            row_items.push(self.keynav_toolbar_slot(crate::keynav::ToolbarItem::Sort, sort_btn));
            row_items.push(Space::new().width(8).into());
            row_items.push(self.keynav_toolbar_slot(crate::keynav::ToolbarItem::Primary, primary));
        }
        let toolbar = container(dir_row(row_items).align_y(iced::Alignment::Center))
            .padding(Padding { top: 16.0, right: 24.0, bottom: 16.0, left: 24.0 })
            .width(Length::Fill);

        let status: Element<'_, Message> = if let Some(err) = &self.snippet_error {
            container(Element::from(text(err.clone()).size(12).color(OryxisColors::t().error)))
                .padding(Padding { top: 0.0, right: 24.0, bottom: 8.0, left: 24.0 }).into()
        } else {
            Space::new().height(0).into()
        };

        if self.snippets.is_empty() {
            let empty_state = crate::widgets::empty_state(
                iced_fonts::lucide::code()
                    .size(32)
                    .color(OryxisColors::t().text_muted)
                    .into(),
                crate::i18n::t("create_snippet_title").to_string(),
                crate::i18n::t("create_snippet_desc").to_string(),
                Some((
                    crate::i18n::t("new_snippet").to_string(),
                    Message::ShowSnippetPanel,
                )),
            );

            // No toolbar when empty: search is hidden and the "+ New" lives
            // in the empty-state CTA (avoids an orphaned action button).
            // Side panel is hoisted to `view_main` (active_side_panel).
            // Un-record the toolbar items; none of them render here.
            // Same for content rows.
            self.keynav_toolbar_reset();
            self.keynav_clear_content();
            let main_content = column![status, empty_state]
                .width(Length::Fill)
                .height(Length::Fill);
            return main_content.into();
        }

        let snippet_needle = self.snippet_search.to_lowercase();
        // Apply the toolbar sort by reordering an index list, the source
        // collection stays in insertion order (its index is what the
        // EditSnippet / RunSnippet messages carry). The needle also
        // matches tags and the group name.
        let mut snippet_order: Vec<usize> = (0..self.snippets.len()).collect();
        self.snippets_sort.sort_items(
            &mut snippet_order,
            |&i| self.snippets[i].label.clone(),
            |&i| self.snippets[i].created_at,
        );
        let visible: Vec<usize> = snippet_order
            .into_iter()
            .filter(|&idx| {
                let snip = &self.snippets[idx];
                snippet_needle.is_empty()
                    || snip.label.to_lowercase().contains(&snippet_needle)
                    || snip.command.to_lowercase().contains(&snippet_needle)
                    || snip
                        .tags
                        .iter()
                        .any(|tg| tg.to_lowercase().contains(&snippet_needle))
                    || snip
                        .group
                        .as_ref()
                        .is_some_and(|g| g.to_lowercase().contains(&snippet_needle))
            })
            .collect();
        // Group order: ungrouped first (no header when it is the only
        // section), then each group alphabetically. Keyboard sections
        // follow the same partition so Tab steps between groups like
        // the dashboard's Groups/Hosts split.
        let mut group_sections: Vec<(Option<String>, Vec<usize>)> = vec![(
            None,
            visible
                .iter()
                .copied()
                .filter(|&i| self.snippets[i].group.is_none())
                .collect(),
        )];
        {
            let mut named: Vec<(String, Vec<usize>)> = Vec::new();
            for &i in &visible {
                if let Some(g) = &self.snippets[i].group {
                    match named.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(g)) {
                        Some((_, items)) => items.push(i),
                        None => named.push((g.clone(), vec![i])),
                    }
                }
            }
            named.sort_by_key(|g| g.0.to_lowercase());
            group_sections.extend(named.into_iter().map(|(n, v)| (Some(n), v)));
        }
        group_sections.retain(|(_, items)| !items.is_empty());
        let multi_section = group_sections.len() > 1
            || group_sections.first().is_some_and(|(n, _)| n.is_some());
        // Keyboard-navigation order, per section, chunked to the grid
        // columns at the bottom.
        let mut snippet_nav_sections: Vec<Vec<crate::keynav::NavItem>> = Vec::new();
        let mut section_blocks: Vec<(Option<String>, Vec<Element<'_, Message>>)> = Vec::new();
        for (group_name, items) in group_sections {
            let mut snippet_nav: Vec<crate::keynav::NavItem> = Vec::new();
            let mut cards: Vec<Element<'_, Message>> = Vec::new();
            for idx in items {
            let snip = &self.snippets[idx];
            snippet_nav.push(crate::keynav::NavItem::Snippet(idx));
            let kb_selected = self.keynav.selected_in(crate::keynav::FocusZone::Content)
                == Some(crate::keynav::NavItem::Snippet(idx));
            // Use host_icon so the snippet badge follows the global
            // `default_host_icon` shape (Circular by default in v0.7)
            // and the rest of the cards on this screen look the same.
            let snip_style = crate::widgets::resolve_host_icon_style(
                None,
                &self.setting_default_host_icon,
            );
            // `line_height(1.0)` collapses the default text padding so
            // the glyph sits at the optical centre of the badge; the
            // default ~1.2 multiplier pushed it visually upward and
            // the badge looked misaligned next to the label column.
            let glyph_el: Element<'_, Message> = iced_fonts::lucide::code()
                .size(14)
                .line_height(1.0)
                .color(Color::WHITE)
                .into();
            let icon_box = crate::widgets::host_icon(
                snip_style,
                OryxisColors::t().accent,
                &snip.label,
                Some(glyph_el),
                32.0,
            );

            // Vertical ellipsis (⋮) only when the row is hovered, so it
            // matches the host-card / keychain affordance. A fixed
            // placeholder reserves the slot in the unhovered state so
            // the label column width stays constant.
            const SNIP_DOTS_SLOT_W: f32 = 22.0;
            // Keep the kebab mounted while its context menu is open, even
            // if the pointer drifts off the card, mirroring the host cards.
            let show_dots = self.hovered_snippet_card == Some(idx)
                || self.snippet_context_menu == Some(idx)
                || kb_selected;
            let edit_btn: Element<'_, Message> = if show_dots {
                crate::widgets::card_kebab_button(
                    OryxisColors::t().text_muted,
                    true,
                    Message::ShowSnippetMenu(idx),
                )
                .into()
            } else {
                Space::new()
                    .width(Length::Fixed(SNIP_DOTS_SLOT_W))
                    .height(Length::Fixed(22.0))
                    .into()
            };

            let cmd_preview = if snip.command.len() > 30 {
                format!("{}...", &snip.command[..30])
            } else {
                snip.command.clone()
            };

            // Tags read as a compact hashtag line under the preview;
            // display-only (editing stays in the comma field).
            let mut info_col = column![
                text(&snip.label)
                    .size(13)
                    .color(OryxisColors::t().text_primary)
                    .wrapping(iced::widget::text::Wrapping::None),
                Space::new().height(2),
                text(cmd_preview)
                    .size(10)
                    .color(OryxisColors::t().text_muted)
                    .font(iced::Font::MONOSPACE)
                    .wrapping(iced::widget::text::Wrapping::None),
            ];
            if !snip.tags.is_empty() {
                let hashtags = snip
                    .tags
                    .iter()
                    .map(|tg| format!("#{tg}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                info_col = info_col.push(Space::new().height(2)).push(
                    text(hashtags)
                        .size(10)
                        .color(Color { a: 0.8, ..OryxisColors::t().accent })
                        .wrapping(iced::widget::text::Wrapping::None),
                );
            }

            let card_btn = button(
                container(
                    dir_row(vec![
                        icon_box,
                        Space::new().width(8).into(),
                        info_col.width(Length::Fill).into(),
                        edit_btn,
                    ]).align_y(iced::Alignment::Center),
                )
                // Match the host card padding (top/bottom 8, left 2,
                // right reserved for the trailing slot) so the row
                // height + indent line up with the rest of the grid.
                .padding(Padding { top: 8.0, right: 2.0, bottom: 8.0, left: 2.0 }),
            )
            .on_press(Message::RunSnippet(idx))
            .width(Length::Fill)
            .style(move |_, status| {
                let (bg, bc, bw) = match status {
                    BtnStatus::Hovered => (OryxisColors::t().bg_hover, OryxisColors::t().accent, 1.5),
                    BtnStatus::Pressed => (OryxisColors::t().bg_selected, OryxisColors::t().accent, 2.0),
                    _ => (OryxisColors::t().bg_surface, OryxisColors::t().border, 1.0),
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border { radius: Radius::from(10.0), color: bc, width: bw },
                    ..Default::default()
                }
            });

            // Wrap the button in a MouseArea so we can track hover
            // for the kebab show/hide affordance, same trick the host
            // cards use.
            let wrapped: Element<'_, Message> = MouseArea::new(card_btn)
                .on_enter(Message::SnippetCardHovered(idx))
                .on_exit(Message::SnippetCardUnhovered)
                .into();
            let card_el: Element<'_, Message> =
                container(wrapped).width(Length::Fill).clip(true).into();
            let card_el = self.card_wash(card_el, OryxisColors::t().accent);
            cards.push(self.keynav_ring_content(kb_selected, card_el));
            }
            snippet_nav_sections.push(snippet_nav);
            section_blocks.push((group_name, cards));
        }

        let nav_width = self.vault_rail_width();
        let panel_width = if self.show_snippet_panel { PANEL_WIDTH } else { 0.0 };
        let available = (self.window_size.width - nav_width - panel_width - 48.0).max(0.0);
        let cols = card_grid_columns(available, CARD_WIDTH, 12.0);
        // Chunk each group's keyboard order to the grid's column count;
        // multi-section recording makes Tab step between groups.
        self.keynav_set_content_sections(
            snippet_nav_sections
                .iter()
                .map(|nav| nav.chunks(cols.max(1)).map(|c| c.to_vec()).collect())
                .collect(),
        );
        // One grid per group, stacked under muted captions (ungrouped
        // first, headerless while it is the only section).
        let mut grid_col = column![].spacing(12);
        for (group_name, cards) in section_blocks {
            if let Some(name) = group_name {
                grid_col = grid_col.push(
                    container(
                        text(name)
                            .size(12)
                            .font(iced::Font {
                                weight: iced::font::Weight::Bold,
                                ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                            })
                            .color(OryxisColors::t().text_muted),
                    )
                    .width(Length::Fill)
                    .align_x(dir_align_x()),
                );
            } else if multi_section {
                grid_col = grid_col.push(
                    container(
                        text(t("ungrouped"))
                            .size(12)
                            .font(iced::Font {
                                weight: iced::font::Weight::Bold,
                                ..iced::Font::new(crate::theme::SYSTEM_UI_FAMILY)
                            })
                            .color(OryxisColors::t().text_muted),
                    )
                    .width(Length::Fill)
                    .align_x(dir_align_x()),
                );
            }
            grid_col = grid_col.push(distribute_card_grid(cards, cols, 12.0, 12.0));
        }

        let grid = scrollable(
            column![grid_col].padding(Padding { top: 0.0, right: 24.0, bottom: 24.0, left: 24.0 }),
        )
        // Stable id so the keyboard router can keep the selected card
        // scrolled into view.
        .id(iced::widget::Id::new("snippets-grid-scroll"))
        .height(Length::Fill);

        // Inline search bar in Classic mode (Workspace puts it on
        // the contextual sub-nav). Collapses to zero height in
        // Workspace so we don't render the input twice.
        // Search now lives in the toolbar (`vault_search_field`); the
        // legacy below-toolbar search bar collapses to nothing.
        let search_bar: Element<'_, Message> = Space::new().height(0).into();

        // Side panel hoisted to `view_main` (active_side_panel).
        column![toolbar, search_bar, status, grid]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn view_snippet_panel(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order (row mode: Up/Down from any input).
        self.panel_nav_reset();
        let is_editing = self.snippet_editing_id.is_some();
        let title = if is_editing { t("edit_snippet") } else { t("new_snippet") };

        let panel_header = container(
            dir_row(vec![
                text(title).size(18).color(OryxisColors::t().text_primary).into(),
                Space::new().width(Length::Fill).into(),
                button(text("\u{00D7}").size(14).color(OryxisColors::t().text_muted))
                    .on_press(Message::HideSnippetPanel)
                    .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                    .style(|_, _| button::Style {
                        background: Some(Background::Color(OryxisColors::t().bg_surface)),
                        border: Border { radius: Radius::from(6.0), ..Default::default() },
                        ..Default::default()
                    }).into(),
            ]).align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 20.0, right: 20.0, bottom: 16.0, left: 20.0 });

        let form = column![
            text(t("name")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(4),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-snippet-name")),
                10.0,
                text_input("restart-nginx", &self.snippet_label)
                    .id(iced::widget::Id::new("panel-snippet-name"))
                    .on_input(Message::SnippetLabelChanged)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                    .into(),
            ),
            Space::new().height(14),
            text(t("group")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(4),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-snippet-group")),
                10.0,
                text_input(t("group_optional_placeholder"), &self.snippet_group)
                    .id(iced::widget::Id::new("panel-snippet-group"))
                    .on_input(Message::SnippetGroupChanged)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                    .into(),
            ),
            Space::new().height(14),
            text(t("tags")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(4),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-snippet-tags")),
                10.0,
                text_input(t("tags_placeholder"), &self.snippet_tags_input)
                    .id(iced::widget::Id::new("panel-snippet-tags"))
                    .on_input(Message::SnippetTagsChanged)
                    .padding(10)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                    .into(),
            ),
            Space::new().height(14),
            text(t("command_label")).size(12).color(OryxisColors::t().text_secondary),
            Space::new().height(4),
            // Multi-line, auto-grows with content; container caps the height
            // (~10 lines) and then it scrolls internally.
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("panel-snippet-command")),
                10.0,
                container(
                    text_editor(&self.snippet_command)
                        .id(iced::widget::Id::new("panel-snippet-command"))
                        .placeholder("sudo systemctl restart nginx")
                        .on_action(Message::SnippetCommandAction)
                        .padding(10)
                        .height(Length::Shrink)
                        .style(crate::widgets::rounded_editor_style),
                )
                .height(Length::Shrink.max(240.0))
                .into(),
            ),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        let panel_error: Element<'_, Message> = if let Some(err) = &self.snippet_error {
            Element::from(text(err.clone()).size(11).color(OryxisColors::t().error))
        } else {
            Space::new().height(0).into()
        };

        let save_btn = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::SaveSnippet),
            8.0,
            button(
                container(text(crate::i18n::t("save")).size(13).color(OryxisColors::t().text_primary))
                    .padding(Padding { top: 10.0, right: 0.0, bottom: 10.0, left: 0.0 })
                    .width(Length::Fill).center_x(Length::Fill),
            )
            .on_press(Message::SaveSnippet)
            .width(Length::Fill)
            .style(|_, _| button::Style {
                background: Some(Background::Color(OryxisColors::t().accent)),
                border: Border { radius: Radius::from(8.0), ..Default::default() },
                ..Default::default()
            })
            .into(),
        );

        // The edit panel only saves. Deleting a snippet lives on the
        // card's ⋮ context menu (Edit / Delete), so the destructive
        // action isn't buried inside the editor form.
        let bottom = column![save_btn];

        let panel_content = column![
            panel_header,
            container(
                column![
                    form,
                    Space::new().height(12),
                    panel_error,
                    Space::new().height(Length::Fill),
                    bottom,
                ].height(Length::Fill).width(Length::Fill).align_x(dir_align_x()),
            )
            .padding(Padding { top: 0.0, right: 20.0, bottom: 20.0, left: 20.0 })
            .height(Length::Fill),
        ].height(Length::Fill);

        container(panel_content)
            .width(PANEL_WIDTH)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_sidebar)),
                border: Border { color: OryxisColors::t().border, width: 1.0, radius: Radius::from(0.0) },
                ..Default::default()
            })
            .into()
    }
}
