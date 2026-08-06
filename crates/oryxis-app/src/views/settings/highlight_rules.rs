//! Settings -> Terminal: the user's own highlight rules (C6).
//!
//! A list plus one inline editor, the same shape as the login-automation
//! block in Settings > Connection. Order is precedence (the first
//! matching rule paints the cell), which is why the rows carry move
//! arrows rather than being sorted for the user.

use super::*;
use iced::widget::column;

use oryxis_core::models::TriggerAction;

impl Oryxis {
    pub(crate) fn highlight_rules_section(&self) -> Element<'_, Message> {
        let mut col = column![
            dir_row(vec![
                column![
                    text(t("highlight_rules"))
                        .size(13)
                        .color(OryxisColors::t().text_primary),
                    Space::new().height(4),
                    text(t("highlight_rules_desc"))
                        .size(11)
                        .color(OryxisColors::t().text_muted),
                ]
                .into(),
                Space::new().width(Length::Fill).into(),
                self.settings_nav_slot_labeled(
                    t("hl_rule_add"),
                    crate::keynav::RowAction::activate(Message::Settings(
                        SettingsMessage::HighlightRuleAdd,
                    )),
                    6.0,
                    styled_button(
                        t("hl_rule_add"),
                        Message::Settings(SettingsMessage::HighlightRuleAdd),
                        OryxisColors::t().accent,
                    ),
                ),
            ])
            .align_y(iced::Alignment::Center),
            Space::new().height(12),
        ];

        if self.prefs.highlight_rules.is_empty() && self.highlight_rule_form.editing.is_none() {
            col = col.push(
                text(t("hl_rule_empty"))
                    .size(12)
                    .color(OryxisColors::t().text_muted),
            );
            return panel_section(col);
        }

        for (idx, rule) in self.prefs.highlight_rules.iter().enumerate() {
            let editing = self.highlight_rule_form.editing == Some(idx)
                && !self.highlight_rule_form.creating;
            let confirming = self.highlight_rule_form.confirm_delete == Some(idx);
            let label = if rule.name.trim().is_empty() {
                rule.pattern.clone()
            } else {
                rule.name.clone()
            };
            let swatch_color = oryxis_terminal::parse_hex_color(&rule.color)
                .unwrap_or(crate::highlight_rules::FALLBACK_COLOR);

            let header = dir_row(vec![
                // The colour the rule paints in, as a chip: the point of
                // the rule is what it looks like on screen.
                container(Space::new().width(12).height(12))
                    .style(move |_| container::Style {
                        background: Some(Background::Color(swatch_color)),
                        border: Border {
                            radius: Radius::from(3.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into(),
                Space::new().width(10).into(),
                column![
                    text(label)
                        .size(13)
                        .color(if rule.enabled {
                            OryxisColors::t().text_primary
                        } else {
                            OryxisColors::t().text_muted
                        }),
                    text(rule_summary(rule))
                        .size(11)
                        .color(OryxisColors::t().text_muted),
                ]
                .into(),
                Space::new().width(Length::Fill).into(),
                self.settings_nav_slot_labeled(
                    t("hl_rule_enabled"),
                    crate::keynav::RowAction::activate(Message::Settings(
                        SettingsMessage::HighlightRuleToggleEnabled(idx),
                    )),
                    6.0,
                    checkbox(rule.enabled)
                        .on_toggle(move |_| {
                            Message::Settings(SettingsMessage::HighlightRuleToggleEnabled(idx))
                        })
                        .size(16)
                        .into(),
                ),
                Space::new().width(8).into(),
                self.hl_move_button(idx, true),
                Space::new().width(4).into(),
                self.hl_move_button(idx, false),
                Space::new().width(8).into(),
                self.settings_nav_slot_labeled(
                    t("edit"),
                    crate::keynav::RowAction::activate(Message::Settings(
                        SettingsMessage::HighlightRuleEdit(idx),
                    )),
                    6.0,
                    styled_button(
                        t("edit"),
                        Message::Settings(SettingsMessage::HighlightRuleEdit(idx)),
                        OryxisColors::t().bg_hover,
                    ),
                ),
                Space::new().width(8).into(),
                self.settings_nav_slot_labeled(
                    t("delete"),
                    crate::keynav::RowAction::activate(Message::Settings(
                        SettingsMessage::HighlightRuleRequestDelete(idx),
                    )),
                    6.0,
                    styled_button(
                        t("delete"),
                        Message::Settings(SettingsMessage::HighlightRuleRequestDelete(idx)),
                        OryxisColors::t().bg_hover,
                    ),
                ),
            ])
            .align_y(iced::Alignment::Center);

            col = col.push(header);

            if confirming {
                col = col.push(Space::new().height(8)).push(
                    dir_row(vec![
                        text(t("hl_rule_delete_confirm"))
                            .size(11)
                            .color(OryxisColors::t().warning)
                            .into(),
                        Space::new().width(Length::Fill).into(),
                        self.settings_nav_slot_labeled(
                            t("delete"),
                            crate::keynav::RowAction::activate(Message::Settings(
                                SettingsMessage::HighlightRuleDelete(idx),
                            )),
                            6.0,
                            styled_button(
                                t("delete"),
                                Message::Settings(SettingsMessage::HighlightRuleDelete(idx)),
                                OryxisColors::t().error,
                            ),
                        ),
                        Space::new().width(8).into(),
                        self.settings_nav_slot_labeled(
                            t("cancel"),
                            crate::keynav::RowAction::activate(Message::Settings(
                                SettingsMessage::HighlightRuleCancelDelete,
                            )),
                            6.0,
                            styled_button(
                                t("cancel"),
                                Message::Settings(SettingsMessage::HighlightRuleCancelDelete),
                                OryxisColors::t().bg_hover,
                            ),
                        ),
                    ])
                    .align_y(iced::Alignment::Center),
                );
            }

            if editing {
                col = col.push(Space::new().height(10)).push(self.highlight_rule_editor());
            }

            col = col.push(Space::new().height(14));
        }

        // A rule being CREATED has no row of its own yet, so its editor
        // goes at the end of the list, where the rule itself will land.
        if self.highlight_rule_form.creating {
            col = col.push(self.highlight_rule_editor());
        }

        panel_section(col)
    }

    /// One of the reorder arrows. Disabled at the ends rather than
    /// hidden, so the row's controls do not shift as a rule moves.
    fn hl_move_button(&self, idx: usize, up: bool) -> Element<'_, Message> {
        let enabled = if up {
            idx > 0
        } else {
            idx + 1 < self.prefs.highlight_rules.len()
        };
        let label = if up { "\u{2191}" } else { "\u{2193}" };
        let tip = if up { t("hl_rule_move_up") } else { t("hl_rule_move_down") };
        let msg = Message::Settings(SettingsMessage::HighlightRuleMove(idx, up));
        let button = styled_button_opt(
            label,
            enabled.then_some(msg.clone()),
            OryxisColors::t().bg_hover,
        );
        if !enabled {
            return button;
        }
        self.settings_nav_slot_labeled(
            tip,
            crate::keynav::RowAction::activate(msg),
            6.0,
            button,
        )
    }

    /// The inline editor for the rule being created or changed.
    fn highlight_rule_editor(&self) -> Element<'_, Message> {
        let form = &self.highlight_rule_form;
        let rule = &form.rule;
        let snippet_labels: Vec<String> =
            self.snippets.iter().map(|s| s.label.clone()).collect();

        let mut col = column![
            text(t("name"))
                .size(12)
                .color(OryxisColors::t().text_muted),
            Space::new().height(6),
            self.settings_nav_slot_labeled(
                t("name"),
                crate::keynav::RowAction::input(iced::widget::Id::new("set-hl-rule-name")),
                10.0,
                text_input(t("hl_rule_name_ph"), &rule.name)
                    .id(iced::widget::Id::new("set-hl-rule-name"))
                    .on_input(|v| Message::Settings(SettingsMessage::HighlightRuleNameChanged(v)))
                    .padding(10)
                    .style(crate::widgets::rounded_input_style)
                    .into(),
            ),
            Space::new().height(10),
            text(t("hl_rule_pattern"))
                .size(12)
                .color(OryxisColors::t().text_muted),
            Space::new().height(6),
            self.settings_nav_slot_labeled(
                t("hl_rule_pattern"),
                crate::keynav::RowAction::input(iced::widget::Id::new("set-hl-rule-pattern")),
                10.0,
                text_input(
                    if rule.is_regex { t("hl_rule_pattern_re_ph") } else { t("hl_rule_pattern_ph") },
                    &rule.pattern,
                )
                .id(iced::widget::Id::new("set-hl-rule-pattern"))
                .on_input(|v| Message::Settings(SettingsMessage::HighlightRulePatternChanged(v)))
                .padding(10)
                .font(iced::Font::MONOSPACE)
                .style(crate::widgets::rounded_input_style)
                .into(),
            ),
            Space::new().height(10),
            self.nav_toggle_row(
                t("hl_rule_regex"),
                rule.is_regex,
                Message::Settings(SettingsMessage::HighlightRuleToggleRegex),
            ),
            self.nav_toggle_row(
                t("hl_rule_case"),
                rule.case_sensitive,
                Message::Settings(SettingsMessage::HighlightRuleToggleCaseSensitive),
            ),
            Space::new().height(10),
            text(t("hl_rule_color"))
                .size(12)
                .color(OryxisColors::t().text_muted),
            Space::new().height(6),
            self.hl_color_row(&rule.color),
            Space::new().height(12),
        ];

        // The action, and the snippet picker it needs. Recorded in
        // display order so the keyboard walk follows the eye.
        col = col.push(self.nav_pick_row(
            t("hl_rule_action"),
            crate::dispatch_settings::action_options()
                .into_iter()
                .map(|(_, l)| l.to_string())
                .collect(),
            crate::dispatch_settings::action_label(&rule.action).to_string(),
            |l: &String| l.clone(),
            200.0,
            |l| Message::Settings(SettingsMessage::HighlightRuleActionChanged(l)),
        ));
        if let TriggerAction::Snippet { id } = &rule.action {
            let selected = self
                .snippets
                .iter()
                .find(|s| s.id.to_string() == *id)
                .map(|s| s.label.clone())
                .unwrap_or_default();
            col = col.push(self.nav_pick_row(
                t("hl_rule_snippet"),
                snippet_labels,
                selected,
                |l: &String| l.clone(),
                200.0,
                |l| Message::Settings(SettingsMessage::HighlightRuleSnippetChanged(l)),
            ));
        }
        if rule.action.is_trigger() {
            col = col.push(Space::new().height(4)).push(
                text(t("hl_rule_trigger_note"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            );
        }

        if let Some(err) = &form.error {
            col = col
                .push(Space::new().height(8))
                .push(text(err.clone()).size(11).color(OryxisColors::t().error));
        }

        let footer = dir_row(vec![
            Space::new().width(Length::Fill).into(),
            self.settings_nav_slot_labeled(
                t("save"),
                crate::keynav::RowAction::activate(Message::Settings(
                    SettingsMessage::HighlightRuleSave,
                )),
                6.0,
                styled_button(
                    t("save"),
                    Message::Settings(SettingsMessage::HighlightRuleSave),
                    OryxisColors::t().accent,
                ),
            ),
            Space::new().width(8).into(),
            self.settings_nav_slot_labeled(
                t("cancel"),
                crate::keynav::RowAction::activate(Message::Settings(
                    SettingsMessage::HighlightRuleCancelEdit,
                )),
                6.0,
                styled_button(
                    t("cancel"),
                    Message::Settings(SettingsMessage::HighlightRuleCancelEdit),
                    OryxisColors::t().bg_hover,
                ),
            ),
        ])
        .align_y(iced::Alignment::Center);

        container(col.push(Space::new().height(12)).push(footer))
            .padding(Padding {
                top: 12.0,
                right: 12.0,
                bottom: 12.0,
                left: 12.0,
            })
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_hover)),
                border: Border {
                    radius: Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// Colour presets plus a hex field. The full HSV picker (the custom
    /// theme editor's) is 180 px tall, which is most of this editor;
    /// six terminal-legible swatches and a field that accepts anything
    /// cover the same ground in one row.
    fn hl_color_row<'a>(&'a self, current: &'a str) -> Element<'a, Message> {
        let mut row: Vec<Element<'a, Message>> = Vec::new();
        for preset in crate::highlight_rules::RULE_COLOR_PRESETS {
            let color = oryxis_terminal::parse_hex_color(preset)
                .unwrap_or(crate::highlight_rules::FALLBACK_COLOR);
            let selected = current.eq_ignore_ascii_case(preset);
            let msg =
                Message::Settings(SettingsMessage::HighlightRuleColorChanged(preset.to_string()));
            row.push(self.settings_nav_slot_labeled(
                preset,
                crate::keynav::RowAction::activate(msg.clone()),
                6.0,
                button(Space::new().width(18).height(18))
                    .on_press(msg)
                    .padding(2)
                    .style(move |_, status| button::Style {
                        background: Some(Background::Color(color)),
                        border: Border {
                            radius: Radius::from(4.0),
                            width: if selected || status != BtnStatus::Active { 2.0 } else { 0.0 },
                            color: OryxisColors::t().text_primary,
                        },
                        ..Default::default()
                    })
                    .into(),
            ));
            row.push(Space::new().width(6).into());
        }
        row.push(Space::new().width(6).into());
        row.push(self.settings_nav_slot_labeled(
            t("hl_rule_color"),
            crate::keynav::RowAction::input(iced::widget::Id::new("set-hl-rule-color")),
            8.0,
            text_input("#RRGGBB", current)
                .id(iced::widget::Id::new("set-hl-rule-color"))
                .on_input(|v| Message::Settings(SettingsMessage::HighlightRuleColorChanged(v)))
                .padding(7)
                .size(12)
                .width(Length::Fixed(110.0))
                .style(crate::widgets::rounded_input_style)
                .into(),
        ));
        dir_row(row).align_y(iced::Alignment::Center).into()
    }
}

/// The one-line summary under a rule's name: what it matches and what it
/// does about it.
fn rule_summary(rule: &oryxis_core::models::HighlightRule) -> String {
    let mut parts = vec![rule.pattern.clone()];
    if rule.is_regex {
        parts.push(t("hl_rule_regex").to_string());
    }
    if rule.case_sensitive {
        parts.push(t("hl_rule_case").to_string());
    }
    if rule.action.is_trigger() {
        parts.push(crate::dispatch_settings::action_label(&rule.action).to_string());
    }
    parts.join(" \u{00b7} ")
}
