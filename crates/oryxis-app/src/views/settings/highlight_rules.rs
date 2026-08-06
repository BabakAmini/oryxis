//! Settings -> Terminal: the user's own highlight rules (C6).
//!
//! A list plus one inline editor, the same shape as the login-automation
//! block in Settings > Connection. Order is precedence (the first
//! matching rule paints the cell), which is why the rows carry move
//! arrows rather than being sorted for the user.

use super::*;
use iced::widget::column;

use oryxis_core::models::{HighlightRule, TriggerAction};

use crate::state::RuleScope;

impl Oryxis {
    /// Settings > Terminal: the global list.
    pub(crate) fn highlight_rules_section(&self) -> Element<'_, Message> {
        panel_section(self.highlight_rules_block(
            RuleScope::Global,
            &self.prefs.highlight_rules,
        ))
    }


    /// Record a row on the keyboard ring that owns this surface. The
    /// block renders in two places with two different rings (Settings'
    /// and the host panel's), and a row recorded on the wrong one is a
    /// row the keyboard cannot reach.
    fn hl_nav_slot<'a>(
        &'a self,
        scope: RuleScope,
        label: &str,
        action: crate::keynav::RowAction,
        radius: f32,
        el: Element<'a, Message>,
    ) -> Element<'a, Message> {
        match scope {
            RuleScope::Global => self.settings_nav_slot_labeled(label, action, radius, el),
            RuleScope::Host => self.panel_nav_slot(action, radius, el),
        }
    }

    /// A toggle row on the ring that owns this surface.
    fn hl_toggle_row<'a>(
        &'a self,
        scope: RuleScope,
        label: &'a str,
        value: bool,
        msg: Message,
    ) -> Element<'a, Message> {
        match scope {
            RuleScope::Global => self.nav_toggle_row(label, value, msg),
            RuleScope::Host => self.panel_nav_slot(
                crate::keynav::RowAction::activate(msg.clone()),
                8.0,
                crate::widgets::toggle_row(label, value, msg),
            ),
        }
    }

    /// A pick-list row on the ring that owns this surface. Mirrors
    /// `nav_pick_row`, which is Settings-only: the ring hugs the picker
    /// itself so Left / Right visibly act on that control.
    fn hl_pick_row<'a>(
        &'a self,
        scope: RuleScope,
        label: &'a str,
        options: Vec<String>,
        selected: String,
        width: f32,
        on_change: impl Fn(String) -> Message + Clone + 'a,
    ) -> Element<'a, Message> {
        if scope == RuleScope::Global {
            return self.nav_pick_row(
                label,
                options,
                selected,
                |l: &String| l.clone(),
                width,
                on_change,
            );
        }
        let (prev, next) = crate::keynav::slots::cycle_pair(&options, &selected, on_change.clone());
        let picker = self.panel_nav_slot(
            crate::keynav::RowAction::picker(prev, next),
            crate::widgets::INPUT_RADIUS,
            iced::widget::pick_list(Some(selected), options, |l: &String| l.clone())
                .on_select(on_change)
                .on_open(Message::Navigation(
                    crate::app::NavigationMessage::PickOpenChanged(true),
                ))
                .on_close(Message::Navigation(
                    crate::app::NavigationMessage::PickOpenChanged(false),
                ))
                .width(width)
                .padding(10)
                .style(crate::widgets::rounded_pick_list_style)
                .into(),
        );
        dir_row(vec![
            text(label).size(13).color(OryxisColors::t().text_primary).into(),
            Space::new().width(Length::Fill).into(),
            picker,
        ])
        .align_y(iced::Alignment::Center)
        .into()
    }

    /// A text input row on the ring that owns this surface.
    fn hl_input_slot<'a>(
        &'a self,
        scope: RuleScope,
        label: &str,
        id: &'static str,
        el: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let action = crate::keynav::RowAction::input(iced::widget::Id::new(id));
        self.hl_nav_slot(scope, label, action, 10.0, el)
    }

    /// The list + editor, shared by Settings (the global rules) and the
    /// host editor (that host's own). Same surface on purpose: they edit
    /// the same kind of thing, and a second implementation would drift.
    pub(crate) fn highlight_rules_block<'a>(
        &'a self,
        scope: RuleScope,
        rules: &'a [HighlightRule],
    ) -> iced::widget::Column<'a, Message> {
        let host = scope == RuleScope::Host;
        let add_button = self.hl_nav_slot(
            scope,
            t("hl_rule_add"),
            crate::keynav::RowAction::activate(Message::Settings(
                SettingsMessage::HighlightRuleAdd(scope),
            )),
            6.0,
            styled_button(
                t("hl_rule_add"),
                Message::Settings(SettingsMessage::HighlightRuleAdd(scope)),
                OryxisColors::t().accent,
            ),
        );
        let heading = column![
            text(t("highlight_rules"))
                .size(13)
                .color(OryxisColors::t().text_primary),
            Space::new().height(4),
            text(if host { t("highlight_rules_host_desc") } else { t("highlight_rules_desc") })
                .size(11)
                .color(OryxisColors::t().text_muted),
        ];
        // The host editor is a narrow side panel: a title, a wrapped
        // description and a button on ONE row leaves the button no width
        // at all (it rendered 0 px wide and could not be clicked), so
        // there the button gets its own line.
        let mut col = if host {
            column![
                heading,
                Space::new().height(10),
                dir_row(vec![add_button, Space::new().width(Length::Fill).into()]),
                Space::new().height(12),
            ]
        } else {
            column![
                dir_row(vec![
                    heading.into(),
                    Space::new().width(Length::Fill).into(),
                    add_button,
                ])
                .align_y(iced::Alignment::Center),
                Space::new().height(12),
            ]
        };

        // The host's append-vs-replace pick. Shown even with an empty
        // list, because "replace with nothing" is the deliberate way to
        // turn highlighting off on a noisy host.
        if host {
            let selected = if self.editor_form.highlight_rules.replace {
                t("hl_host_mode_replace")
            } else {
                t("hl_host_mode_append")
            };
            col = col
                .push(self.hl_pick_row(
                    scope,
                    t("hl_host_mode"),
                    crate::dispatch_settings::host_mode_options()
                        .iter()
                        .map(|l| l.to_string())
                        .collect(),
                    selected.to_string(),
                    220.0,
                    |l| Message::Settings(SettingsMessage::HighlightRuleHostModeChanged(l)),
                ))
                .push(Space::new().height(10));
        }

        let open_here = self.highlight_rule_form.scope == scope;
        if rules.is_empty() && !(open_here && self.highlight_rule_form.creating) {
            return col.push(
                text(t("hl_rule_empty"))
                    .size(12)
                    .color(OryxisColors::t().text_muted),
            );
        }

        for (idx, rule) in rules.iter().enumerate() {
            let editing = open_here
                && self.highlight_rule_form.editing == Some(idx)
                && !self.highlight_rule_form.creating;
            let confirming = open_here && self.highlight_rule_form.confirm_delete == Some(idx);
            let label = if rule.name.trim().is_empty() {
                rule.pattern.clone()
            } else {
                rule.name.clone()
            };
            let swatch_color = oryxis_terminal::parse_hex_color(&rule.color)
                .unwrap_or(crate::highlight_rules::FALLBACK_COLOR);

            // The identity half of the row, and the controls half. On the
            // wide Settings card they share one line; in the narrow host
            // panel they stack, because side by side the controls eat
            // the name (the checkbox rendered ON TOP of the pattern).
            let identity = dir_row(vec![
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
            ])
            .align_y(iced::Alignment::Center);

            let controls = dir_row(vec![
                self.hl_nav_slot(
                    scope,
                    t("hl_rule_enabled"),
                    crate::keynav::RowAction::activate(Message::Settings(
                        SettingsMessage::HighlightRuleToggleEnabled(scope, idx),
                    )),
                    6.0,
                    checkbox(rule.enabled)
                        .on_toggle(move |_| {
                            Message::Settings(SettingsMessage::HighlightRuleToggleEnabled(
                                scope, idx,
                            ))
                        })
                        .size(16)
                        .into(),
                ),
                Space::new().width(8).into(),
                self.hl_move_button(scope, rules.len(), idx, true),
                Space::new().width(4).into(),
                self.hl_move_button(scope, rules.len(), idx, false),
                Space::new().width(8).into(),
                self.hl_nav_slot(
                    scope,
                    t("edit"),
                    crate::keynav::RowAction::activate(Message::Settings(
                        SettingsMessage::HighlightRuleEdit(scope, idx),
                    )),
                    6.0,
                    styled_button(
                        t("edit"),
                        Message::Settings(SettingsMessage::HighlightRuleEdit(scope, idx)),
                        OryxisColors::t().bg_hover,
                    ),
                ),
                Space::new().width(8).into(),
                self.hl_nav_slot(
                    scope,
                    t("delete"),
                    crate::keynav::RowAction::activate(Message::Settings(
                        SettingsMessage::HighlightRuleRequestDelete(scope, idx),
                    )),
                    6.0,
                    styled_button(
                        t("delete"),
                        Message::Settings(SettingsMessage::HighlightRuleRequestDelete(scope, idx)),
                        OryxisColors::t().bg_hover,
                    ),
                ),
            ])
            .align_y(iced::Alignment::Center);

            col = if host {
                // Controls on their own line, reading left to right
                // like every other label in the panel.
                col.push(identity).push(Space::new().height(6)).push(controls)
            } else {
                col.push(
                    dir_row(vec![identity.into(), controls.into()])
                        .align_y(iced::Alignment::Center),
                )
            };

            if confirming {
                col = col.push(Space::new().height(8)).push(
                    dir_row(vec![
                        text(t("hl_rule_delete_confirm"))
                            .size(11)
                            .color(OryxisColors::t().warning)
                            .into(),
                        Space::new().width(Length::Fill).into(),
                        self.hl_nav_slot(
                            scope,
                            t("delete"),
                            crate::keynav::RowAction::activate(Message::Settings(
                                SettingsMessage::HighlightRuleDelete(scope, idx),
                            )),
                            6.0,
                            styled_button(
                                t("delete"),
                                Message::Settings(SettingsMessage::HighlightRuleDelete(scope, idx)),
                                OryxisColors::t().error,
                            ),
                        ),
                        Space::new().width(8).into(),
                        self.hl_nav_slot(
                            scope,
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
        if open_here && self.highlight_rule_form.creating {
            col = col.push(self.highlight_rule_editor());
        }

        col
    }

    /// One of the reorder arrows. Disabled at the ends rather than
    /// hidden, so the row's controls do not shift as a rule moves.
    fn hl_move_button(
        &self,
        scope: RuleScope,
        len: usize,
        idx: usize,
        up: bool,
    ) -> Element<'_, Message> {
        let enabled = if up { idx > 0 } else { idx + 1 < len };
        let label = if up { "\u{2191}" } else { "\u{2193}" };
        let tip = if up { t("hl_rule_move_up") } else { t("hl_rule_move_down") };
        let msg = Message::Settings(SettingsMessage::HighlightRuleMove(scope, idx, up));
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
        let scope = form.scope;
        let rule = &form.rule;
        let snippet_labels: Vec<String> =
            self.snippets.iter().map(|s| s.label.clone()).collect();

        let mut col = column![
            text(t("name"))
                .size(12)
                .color(OryxisColors::t().text_muted),
            Space::new().height(6),
            self.hl_input_slot(
                scope,
                t("name"),
                "set-hl-rule-name",
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
            self.hl_input_slot(
                scope,
                t("hl_rule_pattern"),
                "set-hl-rule-pattern",
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
            self.hl_toggle_row(
                scope,
                t("hl_rule_regex"),
                rule.is_regex,
                Message::Settings(SettingsMessage::HighlightRuleToggleRegex),
            ),
            self.hl_toggle_row(
                scope,
                t("hl_rule_case"),
                rule.case_sensitive,
                Message::Settings(SettingsMessage::HighlightRuleToggleCaseSensitive),
            ),
            Space::new().height(10),
            text(t("hl_rule_color"))
                .size(12)
                .color(OryxisColors::t().text_muted),
            Space::new().height(6),
            self.hl_color_row(scope, &rule.color),
            Space::new().height(12),
        ];

        // The action, and the snippet picker it needs. Recorded in
        // display order so the keyboard walk follows the eye.
        col = col.push(self.hl_pick_row(
            scope,
            t("hl_rule_action"),
            crate::dispatch_settings::action_options()
                .into_iter()
                .map(|(_, l)| l.to_string())
                .collect(),
            crate::dispatch_settings::action_label(&rule.action).to_string(),
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
            col = col.push(self.hl_pick_row(
                scope,
                t("hl_rule_snippet"),
                snippet_labels,
                selected,
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
            self.hl_nav_slot(
                scope,
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
            self.hl_nav_slot(
                scope,
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
    fn hl_color_row<'a>(&'a self, scope: RuleScope, current: &'a str) -> Element<'a, Message> {
        let mut row: Vec<Element<'a, Message>> = Vec::new();
        for preset in crate::highlight_rules::RULE_COLOR_PRESETS {
            let color = oryxis_terminal::parse_hex_color(preset)
                .unwrap_or(crate::highlight_rules::FALLBACK_COLOR);
            let selected = current.eq_ignore_ascii_case(preset);
            let msg =
                Message::Settings(SettingsMessage::HighlightRuleColorChanged(preset.to_string()));
            row.push(self.hl_nav_slot(
                scope,
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
        row.push(self.hl_nav_slot(
            scope,
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
