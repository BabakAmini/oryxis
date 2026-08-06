//! Watches the output stream for the rules that carry an action.
//!
//! Highlighting reads the grid, because "what is on screen" is exactly
//! what should be coloured. Triggers cannot: a rule that fires a
//! notification has to fire once when the line ARRIVES, and the grid is
//! redrawn constantly (scrolling, resizing, a repaint after a theme
//! change), so a grid-based trigger would fire again every time the same
//! text was looked at. This scanner therefore sits on the byte stream,
//! next to [`crate::osc`], and each line it completes is an event that
//! happened exactly once.
//!
//! It reproduces just enough of a terminal to recover the printed text:
//! escape sequences are dropped, and a write cursor tracks `\r` and `\b`
//! so text is OVERWRITTEN rather than thrown away. That last part is not
//! a nicety: every PTY ends its lines with `\r\n`, so a scanner that
//! treated `\r` as "discard the line" would never fire at all, while one
//! that ignored it would fire once per frame of a progress bar. A line
//! is offered to the rules only when `\n` closes it, so half a line is
//! never matched and a prompt being typed into cannot re-fire on every
//! keystroke.
//!
//! **Full-screen applications are not scanned.** On the alternate screen
//! (`tmux`, `vim`, `htop`, `less`) an application repaints its whole
//! frame with cursor positioning instead of newlines, so there are no
//! lines to complete and any text that did complete would repeat with
//! every repaint. Highlighting still works there, since it reads the
//! grid; actions do not fire. This is a real limitation of watching the
//! stream, and it is the same judgement [`crate::prompt_detect`] makes.

use crate::highlight_rules::CompiledRules;

/// Longest line the scanner will hold before giving up on finding a
/// newline and matching what it has. Bounds memory against output with
/// no newlines at all (a binary dump, `cat` of a minified file).
const MAX_LINE: usize = 4096;

/// Where the byte scan currently is. Sequences are recognised only well
/// enough to know when they end; their contents are always discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Scan {
    #[default]
    Normal,
    /// Saw ESC; the next byte says which kind of sequence this is.
    Esc,
    /// Inside a CSI (`ESC [`), ends at a byte in `0x40..=0x7e`.
    Csi,
    /// Inside a string sequence (OSC / DCS / APC / PM / SOS), ends at
    /// BEL or ST (`ESC \`).
    String,
    /// Saw ESC inside a string sequence: `\` ends it, anything else is
    /// part of the string.
    StringEsc,
    /// A two-byte escape whose second byte is a charset designator
    /// (`ESC ( B`), which takes one more byte.
    Charset,
}

/// One line that matched a rule carrying an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerHit {
    /// The matching rule's id. The terminal crate does not know what
    /// actions are; the app maps this back to what it stored.
    pub rule_id: String,
    /// The rule's name, for the notification title.
    pub rule_name: String,
    /// The line as printed, escapes removed and trailing whitespace
    /// trimmed. Shown to the user, so it is the text they saw.
    pub line: String,
}

/// Streaming line accumulator. Lives in [`crate::backend::TerminalBackend`]
/// and keeps its state across PTY reads, so a line split across chunks is
/// still one line.
#[derive(Debug, Default)]
pub struct TriggerScanner {
    scan: Scan,
    /// The line so far, as raw bytes. Kept undecoded because a UTF-8
    /// sequence can be split across two PTY reads: decoding per byte
    /// would turn a legitimate accented word into replacement
    /// characters and a rule written in the user's own language would
    /// never match. The whole line is decoded once, when it completes.
    line: Vec<u8>,
    /// Where the next byte lands in `line`. Carriage return moves it to
    /// zero WITHOUT clearing, which is what a terminal does, and what
    /// makes the two cases come out right: `\r\n` (every PTY's line
    /// ending) completes the line it just finished, while a progress
    /// bar rewriting itself in place leaves only its final state.
    pos: usize,
    /// Set while the emulator is on the alternate screen. The scanner
    /// keeps consuming bytes to stay in sync with escape sequences, but
    /// completes no lines.
    suppressed: bool,
}

impl TriggerScanner {
    /// Tell the scanner whether the emulator is currently on the
    /// alternate screen. Entering or leaving discards the partial line:
    /// its remainder belongs to the other screen.
    pub fn set_suppressed(&mut self, suppressed: bool) {
        if self.suppressed != suppressed {
            self.suppressed = suppressed;
            self.line.clear();
            self.pos = 0;
            self.scan = Scan::Normal;
        }
    }

    /// Feed a chunk of output and return the rules it fired.
    ///
    /// Returns immediately when no rule carries an action, which is the
    /// normal case: a user with only colouring rules never pays for this
    /// scan, and neither does the session player or the history viewer,
    /// which never install rules at all.
    pub fn feed(&mut self, bytes: &[u8], rules: &CompiledRules) -> Vec<TriggerHit> {
        let mut hits = Vec::new();
        if !rules.any_triggers() || self.suppressed {
            // Nothing to look for. Drop any half-line so a rule enabled
            // mid-session cannot fire on text that arrived before it.
            if !self.line.is_empty() {
                self.line.clear();
                self.pos = 0;
            }
            return hits;
        }
        for &b in bytes {
            match self.scan {
                Scan::Normal => match b {
                    0x1b => self.scan = Scan::Esc,
                    b'\n' => {
                        self.match_line(rules, &mut hits);
                        self.line.clear();
                        self.pos = 0;
                    }
                    // Back to column zero. What is already there stays
                    // until something overwrites it, so `\r\n` completes
                    // the line and a progress bar redrawing in place
                    // ends up as one line, in its final state.
                    b'\r' => self.pos = 0,
                    0x08 => self.pos = self.pos.saturating_sub(1),
                    b'\t' => self.push_byte(b' '),
                    // Other C0 controls (BEL, SO/SI, ...) print nothing.
                    0x00..=0x1f | 0x7f => {}
                    _ => self.push_byte(b),
                },
                Scan::Esc => {
                    self.scan = match b {
                        b'[' => Scan::Csi,
                        // OSC / DCS / APC / PM / SOS all run to a string
                        // terminator.
                        b']' | b'P' | b'_' | b'^' | b'X' => Scan::String,
                        // Charset designators take one more byte.
                        b'(' | b')' | b'*' | b'+' => Scan::Charset,
                        // Anything else is a complete two-byte escape.
                        _ => Scan::Normal,
                    };
                }
                Scan::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.scan = Scan::Normal;
                    }
                }
                Scan::String => match b {
                    0x07 => self.scan = Scan::Normal,
                    0x1b => self.scan = Scan::StringEsc,
                    _ => {}
                },
                Scan::StringEsc => {
                    self.scan = if b == b'\\' { Scan::Normal } else { Scan::String };
                }
                Scan::Charset => self.scan = Scan::Normal,
            }
        }
        hits
    }

    /// Write one printable byte at the cursor, overwriting what is
    /// there. Bytes only, never chars: a UTF-8 sequence can be split
    /// across PTY reads, so the line is decoded once, at the end.
    fn push_byte(&mut self, b: u8) {
        if self.pos >= MAX_LINE {
            return;
        }
        if self.pos < self.line.len() {
            self.line[self.pos] = b;
        } else {
            self.line.push(b);
        }
        self.pos += 1;
    }

    /// Test the accumulated line against every action-bearing rule. One
    /// hit per rule: a line that matches the same rule three times is
    /// still one event.
    fn match_line(&mut self, rules: &CompiledRules, hits: &mut Vec<TriggerHit>) {
        let decoded = String::from_utf8_lossy(&self.line);
        let line = decoded.trim_end();
        if line.is_empty() {
            return;
        }
        for rule in rules.rules().iter().filter(|r| r.triggers) {
            if rule.matches(line) {
                hits.push(TriggerHit {
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    line: line.to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight_rules::CompiledRule;
    use iced::Color;

    fn rules(patterns: &[(&str, bool)]) -> CompiledRules {
        CompiledRules::new(
            patterns
                .iter()
                .enumerate()
                .map(|(i, (p, triggers))| {
                    CompiledRule::new(
                        format!("r{i}"),
                        format!("rule {i}"),
                        p,
                        false,
                        false,
                        Color::WHITE,
                        *triggers,
                    )
                    .unwrap()
                })
                .collect(),
        )
    }

    #[test]
    fn a_completed_line_fires_once() {
        let rs = rules(&[("ERROR", true)]);
        let mut s = TriggerScanner::default();
        let hits = s.feed(b"all fine\nERROR: disk full\n", &rs);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, "ERROR: disk full");
        assert_eq!(hits[0].rule_id, "r0");
    }

    #[test]
    fn a_line_split_across_chunks_is_still_one_line() {
        let rs = rules(&[("disk full", true)]);
        let mut s = TriggerScanner::default();
        assert!(s.feed(b"ERROR: disk ", &rs).is_empty());
        // The half line must not match on its own, and must not be lost.
        assert!(s.feed(b"fu", &rs).is_empty());
        assert_eq!(s.feed(b"ll\n", &rs).len(), 1);
    }

    #[test]
    fn escape_sequences_are_not_part_of_the_text() {
        let rs = rules(&[("ERROR", true)]);
        let mut s = TriggerScanner::default();
        // Colour codes around the word, an OSC title in front of it, and
        // a bracketed-paste style CSI after: none of it is text.
        let hits = s.feed(
            b"\x1b]0;title\x07\x1b[31mERROR\x1b[0m: nope\x1b[?25h\n",
            &rs,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, "ERROR: nope");
    }

    #[test]
    fn a_pattern_that_only_exists_across_an_escape_does_not_match() {
        // `ERROR` is really `ER` + a colour change + `ROR`. The user sees
        // the word, so the scanner must too.
        let rs = rules(&[("ERROR", true)]);
        let mut s = TriggerScanner::default();
        assert_eq!(s.feed(b"ER\x1b[31mROR here\n", &rs).len(), 1);
    }

    #[test]
    fn a_carriage_return_rewrites_the_line_in_place() {
        let rs = rules(&[("50%", true)]);
        let mut s = TriggerScanner::default();
        // A progress bar rewrites the same line. Only its final state
        // reaches the rules, and it does so once.
        let hits = s.feed(b"10%\r20%\r50%\r99%\n", &rs);
        assert!(hits.is_empty());
        assert_eq!(s.feed(b"50%\n", &rs).len(), 1);
    }

    #[test]
    fn crlf_completes_the_line_it_ends() {
        // Every PTY ends its lines with CRLF, so treating CR as "throw
        // the line away" would mean no rule with an action ever fired.
        let rs = rules(&[("No space left", true)]);
        let mut s = TriggerScanner::default();
        assert_eq!(s.feed(b"writing: No space left on device\r\n", &rs).len(), 1);
    }

    #[test]
    fn backspace_deletes() {
        let rs = rules(&[("ERROR", true)]);
        let mut s = TriggerScanner::default();
        // A typo rubbed out and retyped still reads as the word.
        assert_eq!(s.feed(b"ERROX\x08R here\n", &rs).len(), 1);
        // And a word rubbed out is gone.
        assert!(s.feed(b"ERROR\x08 here\n", &rs).is_empty());
    }

    #[test]
    fn a_rule_without_an_action_never_produces_a_hit() {
        // It still highlights; it just does not need the stream watched.
        let rs = rules(&[("ERROR", false)]);
        let mut s = TriggerScanner::default();
        assert!(s.feed(b"ERROR: nope\n", &rs).is_empty());
    }

    #[test]
    fn one_hit_per_rule_per_line() {
        let rs = rules(&[("ERROR", true)]);
        let mut s = TriggerScanner::default();
        assert_eq!(s.feed(b"ERROR ERROR ERROR\n", &rs).len(), 1);
    }

    #[test]
    fn two_rules_on_one_line_both_fire() {
        let rs = rules(&[("ERROR", true), ("disk", true)]);
        let mut s = TriggerScanner::default();
        assert_eq!(s.feed(b"ERROR: disk full\n", &rs).len(), 2);
    }

    #[test]
    fn the_alternate_screen_is_not_scanned() {
        let rs = rules(&[("ERROR", true)]);
        let mut s = TriggerScanner::default();
        s.set_suppressed(true);
        assert!(s.feed(b"ERROR: nope\n", &rs).is_empty());
        // Leaving the alternate screen drops whatever the full-screen
        // app left half-written, so the first line after it is clean.
        s.set_suppressed(false);
        assert_eq!(s.feed(b"ERROR: real\n", &rs).len(), 1);
    }

    #[test]
    fn an_endless_line_is_bounded() {
        let rs = rules(&[("ERROR", true)]);
        let mut s = TriggerScanner::default();
        let junk = vec![b'x'; MAX_LINE * 2];
        assert!(s.feed(&junk, &rs).is_empty());
        // Everything past the cap is dropped, so memory is bounded and a
        // match that arrives beyond it is missed rather than buffered
        // forever.
        assert_eq!(s.line.len(), MAX_LINE);
        assert_eq!(s.pos, MAX_LINE);
        s.feed(b"\n", &rs);
        assert!(s.line.is_empty());
    }

    #[test]
    fn a_multi_byte_character_split_across_chunks_survives() {
        // A rule written in the user's own language must match even when
        // the PTY read lands in the middle of a character.
        let rs = rules(&[("não", true)]);
        let mut s = TriggerScanner::default();
        let bytes = "servidor não responde\n".as_bytes();
        // Split inside the "ã".
        let cut = "servidor n\u{c3}".len();
        assert!(s.feed(&bytes[..cut], &rs).is_empty());
        assert_eq!(s.feed(&bytes[cut..], &rs).len(), 1);
    }

    #[test]
    fn a_blank_line_is_not_an_event() {
        // `.` matches any character, so an empty line would fire it if
        // blank lines reached the rules at all.
        let rs = CompiledRules::new(vec![
            CompiledRule::new("r", "n", ".*", true, false, Color::WHITE, true).unwrap(),
        ]);
        let mut s = TriggerScanner::default();
        assert!(s.feed(b"\n\n   \n", &rs).is_empty());
    }
}
