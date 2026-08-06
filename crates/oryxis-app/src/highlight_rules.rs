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

use oryxis_core::models::{HighlightRule, HostHighlightRules, MAX_HIGHLIGHT_RULES};
use oryxis_terminal::{CompiledRule, CompiledRules};
use std::sync::Arc;

use crate::app::Oryxis;

/// The setting the GLOBAL rule list lives in, as a JSON array. A host's
/// own rules live on its connection row instead, so they ride sync and
/// the portable export like every other per-host field.
pub(crate) const SETTING_KEY: &str = "terminal_highlight_rules";

/// How many hosts' resolved rule sets are kept compiled at once.
const MAX_CACHED_HOSTS: usize = 64;

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
    // Said out loud rather than trimmed quietly: a host appending its
    // rules to a full global list is the one way to reach the cap
    // without the editor having refused anything.
    let enabled = rules.iter().filter(|r| r.enabled).count();
    if enabled > MAX_HIGHLIGHT_RULES {
        tracing::warn!(
            "{enabled} enabled highlight rules resolve for this host; \
             only the first {MAX_HIGHLIGHT_RULES} are applied"
        );
    }
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

/// The rules that actually apply on a host: its own, plus the global
/// list unless it asked to replace them.
///
/// The host's rules come FIRST because order is precedence (the first
/// matching rule paints the cell), and the more specific list is the one
/// that should win. `replace` with an empty host list is meaningful: it
/// resolves to nothing at all, which is how a noisy host turns
/// highlighting off.
pub(crate) fn effective_rules(
    global: &[HighlightRule],
    host: Option<&HostHighlightRules>,
) -> Vec<HighlightRule> {
    match host {
        None => global.to_vec(),
        Some(h) if h.replace => h.rules.clone(),
        Some(h) => h.rules.iter().chain(global.iter()).cloned().collect(),
    }
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

    /// The compiled rules a pane on `conn_id` should paint and watch
    /// with: the global list, plus (or replaced by) that host's own.
    ///
    /// Called once per output batch, so it is cached. The cache key
    /// carries a SIGNATURE of the inputs rather than being invalidated
    /// by hand: the global digest is already computed (`CompiledRules::
    /// hash`), and the host's rules are a handful of small structs, so
    /// checking is cheap and cannot go stale. Manual invalidation would
    /// have to be remembered at every site that edits a host, imports
    /// one, or receives one over sync, and one of them would eventually
    /// be missed.
    pub(crate) fn highlight_rules_for(&self, conn_id: Option<uuid::Uuid>) -> Arc<CompiledRules> {
        let global = self.prefs.compiled_highlight_rules.clone();
        let Some(id) = conn_id else {
            return global;
        };
        let Some(host) = self
            .connections
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.highlight_rules.as_ref())
        else {
            return global;
        };
        let sig = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            global.hash().hash(&mut h);
            host.hash(&mut h);
            h.finish()
        };
        // The hit is cloned OUT of the borrow rather than returned from
        // inside an `if let`: a temporary `Ref` in the scrutinee lives
        // for the whole `if let`, which would still be alive at the
        // `borrow_mut` below on a miss. That is a `BorrowMutError`
        // panic in the render path, and the borrow checker cannot see
        // it.
        let cached = self
            .highlight_rules_cache
            .borrow()
            .get(&id)
            .and_then(|(cached_sig, rules)| (*cached_sig == sig).then(|| rules.clone()));
        if let Some(rules) = cached {
            return rules;
        }
        let effective = effective_rules(&self.prefs.highlight_rules, Some(host));
        let (compiled, errors) = compile(&effective);
        for (rule_id, err) in &errors {
            tracing::warn!("highlight rule {rule_id} did not compile: {err}");
        }
        // Bounded: one entry per host that has had a pane open. Dropping
        // the whole map at the cap is fine, it costs one recompile.
        let mut cache = self.highlight_rules_cache.borrow_mut();
        if cache.len() >= MAX_CACHED_HOSTS {
            cache.clear();
        }
        cache.insert(id, (sig, compiled.clone()));
        compiled
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

    #[test]
    fn a_host_without_rules_gets_exactly_the_global_list() {
        let global = vec![rule("a", "ERROR")];
        assert_eq!(effective_rules(&global, None), global);
    }

    #[test]
    fn appending_puts_the_host_first_because_order_is_precedence() {
        // Both lists match "ERROR"; the host's colour must be the one
        // that paints, so its rule has to come first.
        let global = vec![rule("g", "ERROR")];
        let host = HostHighlightRules {
            rules: vec![rule("h", "ERROR")],
            replace: false,
        };
        let out = effective_rules(&global, Some(&host));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "h");
        assert_eq!(out[1].id, "g");
    }

    #[test]
    fn replacing_drops_the_global_list() {
        let global = vec![rule("g", "ERROR")];
        let host = HostHighlightRules {
            rules: vec![rule("h", "WARN")],
            replace: true,
        };
        let out = effective_rules(&global, Some(&host));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "h");
    }

    #[test]
    fn replacing_with_nothing_turns_highlighting_off_on_that_host() {
        // The noisy-host escape hatch, and the reason `replace` with an
        // empty list is not the same as "no override".
        let global = vec![rule("g", "ERROR")];
        let host = HostHighlightRules { rules: Vec::new(), replace: true };
        assert!(effective_rules(&global, Some(&host)).is_empty());
        // ... whereas an all-default override IS the same as none.
        assert!(HostHighlightRules::default().is_empty());
        assert_eq!(HostHighlightRules::default().into_option(), None);
    }
}
