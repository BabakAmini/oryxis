//! The bridge between the stored highlight rules and the terminal.
//!
//! A stored rule is text (`oryxis_core::models::HighlightRule`); what the
//! terminal runs is compiled (`oryxis_terminal::CompiledRules`). This
//! module owns the conversion, and it is the ONLY place that does it, so
//! the widget that paints a match and the backend that fires its action
//! can never end up holding different sets.
//!
//! Compilation happens when the list changes, not per frame: a pattern
//! is compiled once and then matched against every visible row of every
//! repaint.

use oryxis_core::models::{HighlightRule, MAX_HIGHLIGHT_RULES};
use oryxis_terminal::{CompiledRule, CompiledRules};
use std::sync::Arc;

use crate::app::Oryxis;

/// The setting the rule list lives in, as a JSON array.
pub(crate) const SETTING_KEY: &str = "terminal_highlight_rules";

/// Colour a rule falls back to when its stored value is not a colour.
/// Amber, which reads on both the light and the dark terminal themes; a
/// rule with a broken colour still has to match, because the user's
/// reason for writing it (an action, or simply seeing the word) does not
/// depend on the swatch.
pub(crate) const FALLBACK_COLOR: iced::Color = iced::Color {
    r: 1.0,
    g: 0.75,
    b: 0.2,
    a: 1.0,
};

/// The palette offered when creating a rule. Deliberately short and
/// high-contrast: these are terminal colours, and a picker is one click
/// away for anything else.
pub(crate) const RULE_COLOR_PRESETS: [&str; 6] = [
    "#ff5f56", // red
    "#ffbd2e", // amber
    "#27c93f", // green
    "#4aa3ff", // blue
    "#c678dd", // magenta
    "#56b6c2", // cyan
];

/// Compile the stored list into what the terminal runs.
///
/// Disabled rules are dropped rather than compiled and skipped: the
/// terminal never has to ask, and the render key changes when a rule is
/// switched off. A rule that fails to compile is dropped too, with its
/// id and the engine's message returned so the editor can say which one
/// and why; the rest of the list keeps working, because one bad regex
/// must not take the user's other rules down with it.
pub(crate) fn compile(rules: &[HighlightRule]) -> (Arc<CompiledRules>, Vec<(String, String)>) {
    let mut compiled = Vec::new();
    let mut errors = Vec::new();
    for rule in rules.iter().filter(|r| r.enabled).take(MAX_HIGHLIGHT_RULES) {
        let color = oryxis_terminal::parse_hex_color(&rule.color).unwrap_or(FALLBACK_COLOR);
        match CompiledRule::new(
            rule.id.clone(),
            rule.name.clone(),
            &rule.pattern,
            rule.is_regex,
            rule.case_sensitive,
            color,
            rule.action.is_trigger(),
        ) {
            Ok(c) => compiled.push(c),
            Err(e) => errors.push((rule.id.clone(), e)),
        }
    }
    (Arc::new(CompiledRules::new(compiled)), errors)
}

/// Whether `pattern` is something the terminal can actually run, as the
/// editor asks before it lets a rule be saved. `Ok(())` or the engine's
/// own message, which names the offending construct.
pub(crate) fn validate(pattern: &str, is_regex: bool) -> Result<(), String> {
    CompiledRule::new(
        "",
        "",
        pattern,
        is_regex,
        false,
        FALLBACK_COLOR,
        false,
    )
    .map(|_| ())
}

impl Oryxis {
    /// Recompile the rules after an edit.
    ///
    /// Nothing is pushed to the panes here. The widget rebuilds from
    /// `prefs` on the next frame, and each backend picks the new set up
    /// on its next output batch (a pointer comparison in the output
    /// funnel), which is the one place every pane passes through no
    /// matter which of the half-dozen creation paths made it.
    pub(crate) fn apply_highlight_rules(&mut self) {
        let (compiled, errors) = compile(&self.prefs.highlight_rules);
        for (id, err) in &errors {
            tracing::warn!("highlight rule {id} did not compile: {err}");
        }
        self.prefs.compiled_highlight_rules = compiled;
    }

    /// Persist the current rule list and apply it.
    pub(crate) fn save_highlight_rules(&mut self) {
        let json = serde_json::to_string(&self.prefs.highlight_rules).unwrap_or_default();
        self.persist_setting(SETTING_KEY, &json);
        self.apply_highlight_rules();
    }
}

/// Parse the stored setting. A payload that no longer deserializes (hand
/// edited, or written by a build that changed the shape) yields an empty
/// list rather than blocking boot: rules are a preference, and losing
/// them is recoverable in a way failing to start is not.
pub(crate) fn parse_setting(value: &str) -> Vec<HighlightRule> {
    if value.trim().is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Vec<HighlightRule>>(value) {
        Ok(rules) => rules,
        Err(e) => {
            tracing::warn!("stored highlight rules are unreadable, ignoring them: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oryxis_core::models::TriggerAction;

    fn rule(id: &str, pattern: &str) -> HighlightRule {
        HighlightRule {
            id: id.to_string(),
            pattern: pattern.to_string(),
            color: "#ff0000".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn a_disabled_rule_is_not_compiled() {
        let mut rules = vec![rule("a", "ERROR"), rule("b", "WARN")];
        rules[1].enabled = false;
        let (compiled, errors) = compile(&rules);
        assert!(errors.is_empty());
        assert_eq!(compiled.rules().len(), 1);
        assert_eq!(compiled.rules()[0].id, "a");
    }

    #[test]
    fn one_broken_rule_does_not_take_the_others_down() {
        let mut bad = rule("bad", "(unclosed");
        bad.is_regex = true;
        let rules = vec![rule("good", "ERROR"), bad];
        let (compiled, errors) = compile(&rules);
        assert_eq!(compiled.rules().len(), 1);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "bad");
    }

    #[test]
    fn a_broken_colour_still_leaves_a_working_rule() {
        let mut r = rule("a", "ERROR");
        r.color = "not a colour".to_string();
        let (compiled, errors) = compile(&[r]);
        assert!(errors.is_empty());
        assert_eq!(compiled.rules()[0].color, FALLBACK_COLOR);
    }

    #[test]
    fn only_action_bearing_rules_ask_for_the_output_scan() {
        let plain = compile(&[rule("a", "ERROR")]).0;
        assert!(!plain.any_triggers());
        let mut r = rule("a", "ERROR");
        r.action = TriggerAction::Beep;
        assert!(compile(&[r]).0.any_triggers());
    }

    #[test]
    fn the_list_is_capped() {
        let rules: Vec<_> = (0..MAX_HIGHLIGHT_RULES + 10)
            .map(|i| rule(&i.to_string(), "x"))
            .collect();
        assert_eq!(compile(&rules).0.rules().len(), MAX_HIGHLIGHT_RULES);
    }

    #[test]
    fn the_stored_list_round_trips_and_survives_garbage() {
        let rules = vec![rule("a", "ERROR")];
        let json = serde_json::to_string(&rules).unwrap();
        assert_eq!(parse_setting(&json), rules);
        assert!(parse_setting("").is_empty());
        // A payload from a hand-edited vault must not stop the app.
        assert!(parse_setting("{not json").is_empty());
    }

    #[test]
    fn validation_matches_what_compilation_accepts() {
        assert!(validate("ERROR", false).is_ok());
        // Regex metacharacters are literal text in a plain rule, so a
        // pattern that would be a broken regex is a fine literal.
        assert!(validate("(unclosed", false).is_ok());
        assert!(validate("(unclosed", true).is_err());
        assert!(validate("", false).is_err());
    }
}
