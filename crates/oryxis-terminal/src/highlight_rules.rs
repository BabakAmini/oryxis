//! Compiled form of the app's user-defined highlight rules.
//!
//! The stored rule (`oryxis_core::HighlightRule`) is text: a pattern, a
//! hex colour, some flags. This is what the terminal actually runs, and
//! it is compiled once when the settings change rather than per frame,
//! because both consumers are hot: the render pass walks every visible
//! row, and the trigger scanner walks every byte the host sends.
//!
//! Everything is a regular expression here, including a plain-text rule,
//! which is compiled through [`regex::escape`]. One matcher means one
//! set of edge cases (case folding, overlapping matches, UTF-8
//! boundaries) instead of two, and the `regex` crate resolves a literal
//! pattern to a substring search internally, so the simple case pays
//! nothing for the generality. It also has no backtracking, so a
//! pathological pattern costs compile-time memory (bounded below), never
//! exponential match time on remote output.
//!
//! The compiled rule deliberately does NOT know what its action does. It
//! carries a single [`CompiledRule::triggers`] flag so the scanner can
//! stay inert for a rule that only paints; the app resolves the id back
//! to the action it stored.

use iced::Color;

/// Bound on a compiled pattern's memory, and on the lazy DFA's cache.
/// A regex is user input, and the defaults are generous enough that a
/// nested repetition could allocate tens of megabytes before failing.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// How many matches of ONE rule are painted on ONE row. A row is at most
/// a few hundred cells, so this can only be reached by a pattern that
/// matches almost every character, where the extra spans are invisible
/// anyway.
const MAX_MATCHES_PER_ROW: usize = 64;

/// A rule ready to run: its matcher, the colour to paint, and whether
/// the output stream has to be scanned for it at all.
#[derive(Debug)]
pub struct CompiledRule {
    /// The stored rule's id, echoed back on a trigger hit so the app can
    /// find the action without the terminal crate knowing what actions
    /// are.
    pub id: String,
    /// The rule's display name, carried along so a notification can be
    /// titled without a lookup.
    pub name: String,
    /// Colour for matching cells.
    pub color: Color,
    /// Whether this rule wants the output stream scanned (it has an
    /// action). A rule that only paints never reaches the scanner.
    pub triggers: bool,
    /// Kept for the render key: the compiled regex's source string does
    /// not carry the builder's case flag, so two rules that differ only
    /// in case sensitivity would otherwise digest identically.
    case_sensitive: bool,
    matcher: regex::Regex,
}

impl CompiledRule {
    /// Compile one rule. `Err` carries a human-readable reason, which the
    /// editor shows inline; a rule that fails to compile is dropped from
    /// the set rather than failing the whole list.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        pattern: &str,
        is_regex: bool,
        case_sensitive: bool,
        color: Color,
        triggers: bool,
    ) -> Result<Self, String> {
        if pattern.is_empty() {
            return Err("empty pattern".to_string());
        }
        let source = if is_regex {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };
        let matcher = regex::RegexBuilder::new(&source)
            .case_insensitive(!case_sensitive)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            id: id.into(),
            name: name.into(),
            color,
            triggers,
            case_sensitive,
            matcher,
        })
    }

    /// Whether the rule matches anywhere in `hay`. Used by the trigger
    /// scanner, which cares that a line matched, not where.
    pub fn matches(&self, hay: &str) -> bool {
        self.matcher.is_match(hay)
    }

    /// Byte spans (`start..end`, end exclusive) of every match in `hay`,
    /// appended to `out`. Zero-width matches are skipped: a pattern like
    /// `x*` matches the empty string at every position, and painting
    /// nothing at every column is not what the user asked for.
    pub fn find_spans(&self, hay: &str, out: &mut Vec<(usize, usize)>) {
        for m in self.matcher.find_iter(hay).take(MAX_MATCHES_PER_ROW) {
            if m.end() > m.start() {
                out.push((m.start(), m.end()));
            }
        }
    }
}

/// Parse a `#rrggbb` colour, tolerating a missing `#` and either case.
/// `None` when the value is not a colour, which the caller turns into
/// the theme's foreground rather than dropping the rule: a bad colour
/// should not silently stop a rule from matching.
pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(Color::from_rgb8(
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    ))
}

/// A compiled rule set plus the digest the widget's render key uses.
///
/// The digest covers everything that changes what gets painted (id,
/// pattern source, flags, colour), so editing a rule or switching one
/// off invalidates the cached grid geometry. Without it a colour change
/// would only appear after the next output batch.
#[derive(Debug, Default)]
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
    hash: u64,
    any_triggers: bool,
}

impl CompiledRules {
    /// Build a set from already-compiled rules.
    pub fn new(rules: Vec<CompiledRule>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        rules.len().hash(&mut h);
        for r in &rules {
            r.id.hash(&mut h);
            r.matcher.as_str().hash(&mut h);
            r.case_sensitive.hash(&mut h);
            // `Color` is not `Hash` (it is f32-based), and the bytes are
            // what the user actually chose.
            [
                (r.color.r * 255.0) as u8,
                (r.color.g * 255.0) as u8,
                (r.color.b * 255.0) as u8,
            ]
            .hash(&mut h);
            r.triggers.hash(&mut h);
        }
        let any_triggers = rules.iter().any(|r| r.triggers);
        Self {
            hash: h.finish(),
            rules,
            any_triggers,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }

    /// Digest for the widget's render key. `0` for an empty set, so a
    /// user with no rules never pays a hash round trip.
    pub fn hash(&self) -> u64 {
        if self.rules.is_empty() { 0 } else { self.hash }
    }

    /// Whether any rule carries an action. The trigger scanner returns
    /// immediately when this is false, which is the normal case.
    pub fn any_triggers(&self) -> bool {
        self.any_triggers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, is_regex: bool, case_sensitive: bool) -> CompiledRule {
        CompiledRule::new(
            "id",
            "name",
            pattern,
            is_regex,
            case_sensitive,
            Color::WHITE,
            false,
        )
        .unwrap()
    }

    #[test]
    fn a_plain_pattern_is_matched_literally() {
        // The dots are characters, not "any character": a literal rule
        // for a version must not match a different one.
        let r = rule("1.2.3", false, false);
        assert!(r.matches("version 1.2.3 ready"));
        assert!(!r.matches("version 10203 ready"));
    }

    #[test]
    fn case_folding_follows_the_flag() {
        assert!(rule("error", false, false).matches("ERROR: nope"));
        assert!(!rule("error", false, true).matches("ERROR: nope"));
        assert!(rule("error", false, true).matches("error: nope"));
    }

    #[test]
    fn spans_cover_each_match_and_skip_empty_ones() {
        let mut out = Vec::new();
        rule("ab", false, false).find_spans("ab-ab", &mut out);
        assert_eq!(out, vec![(0, 2), (3, 5)]);

        // `x*` matches the empty string everywhere; painting a
        // zero-width span at every column is noise, not a highlight.
        out.clear();
        rule("x*", true, false).find_spans("aaa", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_broken_regex_is_an_error_not_a_panic() {
        assert!(CompiledRule::new("i", "n", "(unclosed", true, false, Color::WHITE, false).is_err());
        // An empty pattern would match at every position, so it is
        // refused rather than compiled into a rule that paints nothing
        // over everything.
        assert!(CompiledRule::new("i", "n", "", false, false, Color::WHITE, false).is_err());
    }

    #[test]
    fn the_digest_moves_with_anything_that_changes_the_painting() {
        let base = CompiledRules::new(vec![rule("a", false, false)]);
        let recolored = CompiledRules::new(vec![
            CompiledRule::new("id", "name", "a", false, false, Color::BLACK, false).unwrap(),
        ]);
        let refolded = CompiledRules::new(vec![rule("a", false, true)]);
        let repatterned = CompiledRules::new(vec![rule("b", false, false)]);
        assert_ne!(base.hash(), recolored.hash());
        assert_ne!(base.hash(), refolded.hash());
        assert_ne!(base.hash(), repatterned.hash());
        // An empty set is the same "nothing to draw" as no set at all.
        assert_eq!(CompiledRules::default().hash(), 0);
    }

    #[test]
    fn a_set_without_actions_reports_that_it_needs_no_scanning() {
        assert!(!CompiledRules::new(vec![rule("a", false, false)]).any_triggers());
        let with = CompiledRule::new("i", "n", "a", false, false, Color::WHITE, true).unwrap();
        assert!(CompiledRules::new(vec![with]).any_triggers());
    }

    #[test]
    fn hex_colors_parse_with_or_without_the_hash() {
        assert_eq!(parse_hex_color("#ff0000"), Some(Color::from_rgb8(255, 0, 0)));
        assert_eq!(parse_hex_color("00FF00"), Some(Color::from_rgb8(0, 255, 0)));
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color("nope"), None);
    }
}
