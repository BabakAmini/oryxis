//! User-defined terminal highlight rules, and the action a match may
//! fire (WindTerm-style triggers).
//!
//! A rule is two things at once, and they are deliberately not split:
//! the pattern that paints matching text in the terminal, and,
//! optionally, what should happen when that text arrives. Users think of
//! "watch for OOM" as one thing, so one rule carries both halves.
//!
//! The list is a user preference, not a per-host setting: it is stored
//! whole as JSON in the `terminal_highlight_rules` setting rather than
//! as a vault entity. That is why the id is a plain string here (minted
//! by the app) instead of a `Uuid` column.
//!
//! The rule is stored uncompiled. Compiling a pattern needs a regex
//! engine and a colour type, which belong to the terminal crate, so this
//! module holds only what has to survive a restart.

use serde::{Deserialize, Serialize};

/// What a rule does when its pattern shows up in the output.
///
/// Exactly one action per rule. The desktop notification already makes a
/// sound on every OS we ship to, so "notify plus beep" would be a
/// combination almost nobody wants; a user who genuinely wants two
/// effects writes two rules with the same pattern.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerAction {
    /// Colour the match, nothing else. The default, and the only action
    /// that cannot surprise anyone.
    #[default]
    None,
    /// A desktop notification (in-app toast when the OS refuses it),
    /// through the same path an OSC 9 notification takes.
    Notify,
    /// The native system beep.
    Beep,
    /// Type a stored snippet into the pane the match arrived on. Guarded
    /// by a per-rule, per-session confirmation: the trigger is driven by
    /// REMOTE output, so without the confirmation any host able to print
    /// text could choose what the user's shell runs next.
    Snippet {
        /// The snippet's `Uuid`, as a string. A snippet that no longer
        /// exists disables the action rather than the rule: the
        /// highlight still paints.
        id: String,
    },
}

impl TriggerAction {
    /// Whether this action needs the output stream to be scanned at all.
    /// A rule that only paints costs nothing outside the render pass.
    pub fn is_trigger(&self) -> bool {
        !matches!(self, TriggerAction::None)
    }
}

/// One highlight rule: what to look for, how to paint it, and what to do
/// about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightRule {
    /// Stable identity, minted by the app as a `Uuid` string. Rules are
    /// reorderable and deletable, so the per-session trigger state
    /// (cooldowns, the snippet grant) has to key on something that is
    /// not the list index.
    pub id: String,
    /// What the user calls it. Shown in the rules list and used as the
    /// notification title, so it is worth a real name.
    #[serde(default)]
    pub name: String,
    /// The text or regular expression to match.
    pub pattern: String,
    /// Whether `pattern` is a regular expression. Off by default:
    /// most rules are a word like `ERROR`, and a plain substring cannot
    /// be typed wrong.
    #[serde(default)]
    pub is_regex: bool,
    /// Whether case matters. Off by default, so `error` catches `Error`
    /// and `ERROR` without three rules.
    #[serde(default)]
    pub case_sensitive: bool,
    /// Match colour as `#rrggbb`. An unparseable value falls back to the
    /// theme's foreground rather than dropping the rule.
    #[serde(default)]
    pub color: String,
    /// A rule switched off keeps its definition but stops matching, so
    /// experimenting does not mean deleting.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// What a match fires, if anything.
    #[serde(default)]
    pub action: TriggerAction,
}

fn default_true() -> bool {
    true
}

impl Default for HighlightRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            pattern: String::new(),
            is_regex: false,
            case_sensitive: false,
            color: String::new(),
            enabled: true,
            action: TriggerAction::None,
        }
    }
}

/// How many rules one list may hold. Every rule is a pass over every
/// visible row on every frame that rebuilds the grid, so this is a
/// performance ceiling rather than a modelling opinion; it is far above
/// what a real rule set looks like.
pub const MAX_HIGHLIGHT_RULES: usize = 32;

/// Longest accepted pattern. A regex's compiled size is bounded
/// separately by the engine; this bounds what the editor accepts.
pub const MAX_PATTERN_LEN: usize = 512;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_written_by_an_older_build_still_loads() {
        // Everything except id and pattern is `serde(default)`, so a
        // payload from a build that predates a field keeps working. The
        // one default that is not `Default::default()` is `enabled`:
        // a rule with no flag has to match, not sit silently dead.
        let json = r#"{"id":"a","pattern":"ERROR"}"#;
        let rule: HighlightRule = serde_json::from_str(json).unwrap();
        assert!(rule.enabled);
        assert!(!rule.is_regex);
        assert_eq!(rule.action, TriggerAction::None);
    }

    #[test]
    fn actions_round_trip_through_json() {
        for action in [
            TriggerAction::None,
            TriggerAction::Notify,
            TriggerAction::Beep,
            TriggerAction::Snippet { id: "s1".into() },
        ] {
            let rule = HighlightRule {
                id: "r".into(),
                pattern: "x".into(),
                action: action.clone(),
                ..Default::default()
            };
            let back: HighlightRule =
                serde_json::from_str(&serde_json::to_string(&rule).unwrap()).unwrap();
            assert_eq!(back.action, action);
        }
    }

    #[test]
    fn only_an_action_bearing_rule_wants_the_output_scanned() {
        assert!(!TriggerAction::None.is_trigger());
        assert!(TriggerAction::Notify.is_trigger());
        assert!(TriggerAction::Beep.is_trigger());
        assert!(TriggerAction::Snippet { id: "s".into() }.is_trigger());
    }
}
