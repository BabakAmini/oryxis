//! Settings -> Terminal: the user's own highlight rules (C6).
//!
//! The list is a preference, so unlike the vault-backed families every
//! change here ends in one `save_highlight_rules` (persist the JSON,
//! recompile). The editor works on a copy: a pattern being typed is not
//! a pattern the terminal should be matching against, and half of a
//! regex is usually not a regex at all.

use super::*;

use oryxis_core::models::{HighlightRule, TriggerAction, MAX_HIGHLIGHT_RULES, MAX_PATTERN_LEN};

/// The action pick-list, in display order.
pub(crate) fn action_options() -> Vec<(TriggerAction, &'static str)> {
    vec![
        (TriggerAction::None, crate::i18n::t("hl_action_none")),
        (TriggerAction::Notify, crate::i18n::t("hl_action_notify")),
        (TriggerAction::Beep, crate::i18n::t("hl_action_beep")),
        (
            TriggerAction::Snippet { id: String::new() },
            crate::i18n::t("hl_action_snippet"),
        ),
    ]
}

/// The localized label for an action, for the pick-list's current value.
pub(crate) fn action_label(action: &TriggerAction) -> &'static str {
    match action {
        TriggerAction::None => crate::i18n::t("hl_action_none"),
        TriggerAction::Notify => crate::i18n::t("hl_action_notify"),
        TriggerAction::Beep => crate::i18n::t("hl_action_beep"),
        TriggerAction::Snippet { .. } => crate::i18n::t("hl_action_snippet"),
    }
}

impl Oryxis {
    pub(super) fn handle_settings_highlight_rules(
        &mut self,
        message: SettingsMessage,
    ) -> Result<Task<Message>, SettingsMessage> {
        match message {
            SettingsMessage::HighlightRuleAdd => {
                if self.prefs.highlight_rules.len() >= MAX_HIGHLIGHT_RULES {
                    return Ok(self.show_toast(
                        crate::i18n::t("hl_rule_limit")
                            .replace("{max}", &MAX_HIGHLIGHT_RULES.to_string()),
                    ));
                }
                self.highlight_rule_form = crate::state::HighlightRuleForm {
                    editing: Some(self.prefs.highlight_rules.len()),
                    creating: true,
                    rule: HighlightRule {
                        id: uuid::Uuid::new_v4().to_string(),
                        // The first preset, so a new rule is visible the
                        // moment it saves instead of painting in the
                        // fallback colour of an empty string.
                        color: crate::highlight_rules::RULE_COLOR_PRESETS[0].to_string(),
                        ..Default::default()
                    },
                    ..Default::default()
                };
            }
            SettingsMessage::HighlightRuleEdit(idx) => {
                if let Some(rule) = self.prefs.highlight_rules.get(idx) {
                    self.highlight_rule_form = crate::state::HighlightRuleForm {
                        editing: Some(idx),
                        creating: false,
                        rule: rule.clone(),
                        ..Default::default()
                    };
                }
            }
            SettingsMessage::HighlightRuleCancelEdit => {
                self.highlight_rule_form = crate::state::HighlightRuleForm::default();
            }
            SettingsMessage::HighlightRuleNameChanged(v) => {
                self.highlight_rule_form.rule.name = v;
            }
            SettingsMessage::HighlightRulePatternChanged(v) => {
                self.highlight_rule_form.rule.pattern =
                    v.chars().filter(|c| *c != '\n' && *c != '\r').take(MAX_PATTERN_LEN).collect();
                // Typing is how a bad pattern gets fixed, so the error
                // clears as soon as it changes rather than sitting there
                // until the next save attempt.
                self.highlight_rule_form.error = None;
            }
            SettingsMessage::HighlightRuleToggleRegex => {
                self.highlight_rule_form.rule.is_regex = !self.highlight_rule_form.rule.is_regex;
                self.highlight_rule_form.error = None;
            }
            SettingsMessage::HighlightRuleToggleCaseSensitive => {
                self.highlight_rule_form.rule.case_sensitive =
                    !self.highlight_rule_form.rule.case_sensitive;
            }
            SettingsMessage::HighlightRuleColorChanged(hex) => {
                self.highlight_rule_form.rule.color = hex;
            }
            SettingsMessage::HighlightRuleActionChanged(label) => {
                if let Some((action, _)) =
                    action_options().into_iter().find(|(_, l)| *l == label)
                {
                    // Picking "send a snippet" keeps whichever snippet was
                    // already chosen, so cycling through the list and back
                    // does not silently drop it.
                    self.highlight_rule_form.rule.action = match action {
                        TriggerAction::Snippet { .. } => {
                            let id = match &self.highlight_rule_form.rule.action {
                                TriggerAction::Snippet { id } => id.clone(),
                                _ => self
                                    .snippets
                                    .first()
                                    .map(|s| s.id.to_string())
                                    .unwrap_or_default(),
                            };
                            TriggerAction::Snippet { id }
                        }
                        other => other,
                    };
                }
            }
            SettingsMessage::HighlightRuleSnippetChanged(label) => {
                if let Some(snippet) = self.snippets.iter().find(|s| s.label == label) {
                    self.highlight_rule_form.rule.action = TriggerAction::Snippet {
                        id: snippet.id.to_string(),
                    };
                }
            }
            SettingsMessage::HighlightRuleSave => {
                let rule = self.highlight_rule_form.rule.clone();
                if let Err(e) = crate::highlight_rules::validate(&rule.pattern, rule.is_regex) {
                    self.highlight_rule_form.error = Some(if rule.pattern.trim().is_empty() {
                        crate::i18n::t("hl_rule_pattern_required").to_string()
                    } else {
                        format!("{}: {e}", crate::i18n::t("hl_rule_bad_pattern"))
                    });
                    return Ok(Task::none());
                }
                // An action that names a snippet which no longer exists
                // is refused here rather than failing silently at match
                // time, when there is nobody to tell.
                if let TriggerAction::Snippet { id } = &rule.action
                    && !self.snippets.iter().any(|s| s.id.to_string() == *id)
                {
                    self.highlight_rule_form.error =
                        Some(crate::i18n::t("hl_rule_snippet_required").to_string());
                    return Ok(Task::none());
                }
                let Some(idx) = self.highlight_rule_form.editing else {
                    return Ok(Task::none());
                };
                if self.highlight_rule_form.creating {
                    self.prefs.highlight_rules.push(rule);
                } else if let Some(slot) = self.prefs.highlight_rules.get_mut(idx) {
                    *slot = rule;
                }
                self.highlight_rule_form = crate::state::HighlightRuleForm::default();
                self.save_highlight_rules();
            }
            SettingsMessage::HighlightRuleToggleEnabled(idx) => {
                if let Some(rule) = self.prefs.highlight_rules.get_mut(idx) {
                    rule.enabled = !rule.enabled;
                    self.save_highlight_rules();
                }
            }
            SettingsMessage::HighlightRuleMove(idx, up) => {
                let other = if up { idx.checked_sub(1) } else { Some(idx + 1) };
                if let Some(other) = other
                    && other < self.prefs.highlight_rules.len()
                    && idx < self.prefs.highlight_rules.len()
                {
                    self.prefs.highlight_rules.swap(idx, other);
                    // The editor addresses rules by index, so a move
                    // under an open editor would retarget it.
                    self.highlight_rule_form = crate::state::HighlightRuleForm::default();
                    self.save_highlight_rules();
                }
            }
            SettingsMessage::HighlightRuleRequestDelete(idx) => {
                self.highlight_rule_form.confirm_delete = Some(idx);
            }
            SettingsMessage::HighlightRuleCancelDelete => {
                self.highlight_rule_form.confirm_delete = None;
            }
            SettingsMessage::HighlightRuleDelete(idx) => {
                if idx < self.prefs.highlight_rules.len() {
                    self.prefs.highlight_rules.remove(idx);
                    self.highlight_rule_form = crate::state::HighlightRuleForm::default();
                    self.save_highlight_rules();
                }
            }
            // Routed here by the parent; anything else is a grouping
            // mistake, not a runtime case.
            m => return Err(m),
        }
        Ok(Task::none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_label_and_maps_back_from_it() {
        for (action, label) in action_options() {
            assert_eq!(action_label(&action), label);
            assert!(!label.is_empty());
        }
    }
}
