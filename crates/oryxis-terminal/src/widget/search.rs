//! Scrollback search (C1): find-in-buffer over the alacritty grid.
//!
//! Pure read of `Term`: builds the set of matches for a literal needle
//! across the whole buffer (screen + scrollback), tracks the active one,
//! and hands per-match grid coordinates to the draw pass for
//! highlighting and to the app for scroll-to. Never writes the PTY.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Point as TermPoint};
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::Term;

/// Hard cap on stored matches so a `yes`-style firehose can't build an
/// unbounded vec; the UI shows `9999+` past this.
pub const MAX_MATCHES: usize = 10_000;

/// One search hit, in grid-line coordinates. `line` is alacritty's
/// signed grid line: 0..screen_lines-1 is the visible screen, negative
/// values climb into scrollback. Columns are inclusive of `end_col`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub start_line: i32,
    pub start_col: u16,
    pub end_line: i32,
    pub end_col: u16,
}

/// Live buffer-search state, held on `TerminalState` so the draw pass
/// can read the matches under the shared lock.
#[derive(Default)]
pub struct BufferSearch {
    /// The raw needle typed by the user (mirrored from the app-side input).
    pub query: String,
    /// Compiled DFA for `query`. `None` while the query is empty or fails
    /// to compile (never observed for an escaped literal, but handled).
    regex: Option<RegexSearch>,
    /// Every match, top-to-bottom, capped at [`MAX_MATCHES`].
    pub matches: Vec<SearchMatch>,
    /// Index into `matches` of the highlighted-as-active hit.
    pub active: usize,
    /// Bumped on every query change / step / rebuild / open / close, so
    /// the widget's `RenderKey` invalidates its geometry cache.
    pub generation: u64,
    /// `render_epoch` the matches were built at; a mismatch means new
    /// output landed and the matches are stale (rebuilt lazily, never
    /// per output chunk).
    pub scanned_epoch: u64,
}

impl BufferSearch {
    /// Set the needle and rebuild matches against `term`. Literal search:
    /// the needle is regex-escaped, so `a.b` matches the three characters
    /// `a.b`, never `axb`. Smart case rides alacritty's `RegexSearch`,
    /// which is case-insensitive unless the needle carries an uppercase
    /// character (escaping letters is a no-op, so the detection still
    /// sees the user's case).
    pub fn set_query<T>(&mut self, query: &str, term: &Term<T>, epoch: u64) {
        self.query = query.to_string();
        self.generation = self.generation.wrapping_add(1);
        if query.is_empty() {
            self.regex = None;
            self.matches.clear();
            self.active = 0;
            self.scanned_epoch = epoch;
            return;
        }
        self.regex = RegexSearch::new(&regex_escape(query)).ok();
        self.rebuild(term, epoch);
    }

    /// Re-scan the buffer for the current query (called after new output
    /// invalidated the previous match set, or on resize). Keeps the query
    /// and clamps `active` into the new match count.
    pub fn rebuild<T>(&mut self, term: &Term<T>, epoch: u64) {
        self.scanned_epoch = epoch;
        self.matches.clear();
        let Some(regex) = self.regex.as_mut() else {
            self.active = 0;
            return;
        };
        let start = TermPoint::new(term.topmost_line(), Column(0));
        let end = TermPoint::new(term.bottommost_line(), term.last_column());
        for m in RegexIter::new(start, end, Direction::Right, term, regex).take(MAX_MATCHES) {
            let s = m.start();
            let e = m.end();
            self.matches.push(SearchMatch {
                start_line: s.line.0,
                start_col: s.column.0 as u16,
                end_line: e.line.0,
                end_col: e.column.0 as u16,
            });
        }
        if self.active >= self.matches.len() {
            self.active = 0;
        }
    }

    /// Move the active match `forward` (or backward), wrapping at the ends
    /// (owner decision: wrap, the counter is the only cue). Returns the
    /// new active match so the caller can scroll it into view. `None` when
    /// there are no matches.
    pub fn step(&mut self, forward: bool) -> Option<SearchMatch> {
        if self.matches.is_empty() {
            self.generation = self.generation.wrapping_add(1);
            return None;
        }
        let n = self.matches.len();
        self.active = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
        self.generation = self.generation.wrapping_add(1);
        self.matches.get(self.active).copied()
    }

    /// The active match, if any (for the initial scroll-to on open).
    pub fn active_match(&self) -> Option<SearchMatch> {
        self.matches.get(self.active).copied()
    }
}

/// Escape the regex metacharacters in `s` so a literal needle is searched
/// verbatim. Dependency-free (the terminal crate doesn't pull `regex`);
/// covers every character the regex-automata syntax treats as special.
fn regex_escape(s: &str) -> String {
    const SPECIAL: &str = r"\.+*?()|[]{}^$#&-~";
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if SPECIAL.contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_covers_metacharacters() {
        assert_eq!(regex_escape("a.b"), r"a\.b");
        assert_eq!(regex_escape("a+b*c"), r"a\+b\*c");
        assert_eq!(regex_escape("plain"), "plain");
        assert_eq!(regex_escape("[x]"), r"\[x\]");
    }
}
