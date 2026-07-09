//! Mirror of the remote line editor, fed with the exact bytes the app sends
//! to the PTY / SSH channel. It reconstructs the command line the user is
//! composing so the command-history capture can record it on Enter.
//!
//! Only edits whose effect is knowable from this side are applied: printable
//! text, backspace/delete, cursor moves to absolute positions (Home/End,
//! Left/Right) and the classic kill shortcuts (Ctrl+U/K/W). Anything whose
//! outcome depends on remote state, history recall (Up/Down, Ctrl+R), tab
//! completion, Alt word commands, yank, marks the buffer *tainted*: the
//! bytes no longer prove what the line contains, and the capture must fall
//! back to reading the echoed line off the grid. Ctrl+C aborts the line and
//! clears the taint; a submit resets both.
//!
//! Bracketed-paste wrappers (`CSI 200~` / `CSI 201~`) are recognized so a
//! pasted block lands in the buffer as literal text, newlines and control
//! bytes included, mirroring how the remote readline treats it (the paste
//! payload is inserted verbatim into the edit buffer, never interpreted
//! as line-editing commands).

/// A line the user submitted with Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedLine {
    pub text: String,
    /// True when edits this tracker cannot model touched the line (history
    /// recall, tab completion, ...): `text` is then untrustworthy and the
    /// caller must recover the real line from the terminal grid instead.
    pub tainted: bool,
}

/// Keep pathological lines (megabyte pastes without newlines) from growing
/// the mirror without bound. Past the cap the buffer taints, which routes
/// the capture to the grid-based path anyway.
const MAX_LINE: usize = 16 * 1024;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Scan {
    #[default]
    Ground,
    Esc,
    Csi,
    Ss3,
}

#[derive(Default)]
pub struct InputTracker {
    chars: Vec<char>,
    cursor: usize,
    tainted: bool,
    scan: Scan,
    /// CSI parameter bytes accumulated since `ESC [`.
    params: String,
    /// Inside a bracketed paste (`CSI 200~` seen, `CSI 201~` pending).
    in_paste: bool,
    /// Trailing bytes of a UTF-8 scalar split across `feed` calls.
    pending_utf8: Vec<u8>,
}

impl InputTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop all line state (reconnect, manual clear). Keeps nothing.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed bytes headed to the PTY. Returns the lines completed by an Enter
    /// inside this chunk, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SubmittedLine> {
        let mut submitted = Vec::new();
        for &b in bytes {
            match self.scan {
                Scan::Ground => self.ground(b, &mut submitted),
                Scan::Esc => self.esc(b),
                Scan::Csi => self.csi(b),
                Scan::Ss3 => self.ss3(b),
            }
        }
        submitted
    }

    fn ground(&mut self, b: u8, submitted: &mut Vec<SubmittedLine>) {
        // A multi-byte UTF-8 scalar in progress consumes continuation bytes
        // first; a non-continuation byte aborts the partial sequence.
        if !self.pending_utf8.is_empty() {
            if b & 0xC0 == 0x80 {
                self.pending_utf8.push(b);
                self.try_flush_utf8();
                return;
            }
            self.pending_utf8.clear();
        }
        // Inside a bracketed paste the payload is content, not commands:
        // readline's bracketed-paste-begin inserts the raw block into the
        // edit buffer verbatim, so control bytes (Ctrl+U, Ctrl+W, DEL, ...)
        // must not line-edit here. Only ESC still needs scanning, the
        // `CSI 201~` terminator is the sole exit from the paste.
        if self.in_paste {
            match b {
                // The remote editor inserts the pasted newline literally
                // instead of executing.
                0x0d | 0x0a => self.insert('\n'),
                0x1b => self.scan = Scan::Esc,
                _ => {
                    if b < 0x80 {
                        self.insert(b as char);
                    } else {
                        self.pending_utf8.push(b);
                        self.try_flush_utf8();
                    }
                }
            }
            return;
        }
        match b {
            0x0d | 0x0a => {
                submitted.push(SubmittedLine {
                    text: self.chars.iter().collect(),
                    tainted: self.tainted,
                });
                self.clear_line();
            }
            0x1b => self.scan = Scan::Esc,
            0x7f | 0x08 => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                }
            }
            0x09 => {
                // Tab completion rewrites the line remotely.
                self.tainted = true;
            }
            0x01 => self.cursor = 0,                 // Ctrl+A
            0x05 => self.cursor = self.chars.len(),  // Ctrl+E
            0x02 => self.cursor = self.cursor.saturating_sub(1), // Ctrl+B
            0x06 => self.cursor = (self.cursor + 1).min(self.chars.len()), // Ctrl+F
            0x03 => {
                // Ctrl+C aborts the line: readline starts a fresh, clean one.
                self.clear_line();
            }
            0x04 => {
                // Ctrl+D: delete-char at cursor; on an empty line it is EOF
                // (nothing to track either way).
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            0x15 => {
                // Ctrl+U (unix-line-discard): kill from start to cursor.
                self.chars.drain(..self.cursor);
                self.cursor = 0;
            }
            0x0b => {
                // Ctrl+K: kill from cursor to end.
                self.chars.truncate(self.cursor);
            }
            0x17 => {
                // Ctrl+W (unix-word-rubout): kill the whitespace-delimited
                // word before the cursor, exactly bash's rule.
                let mut i = self.cursor;
                while i > 0 && self.chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                while i > 0 && !self.chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                self.chars.drain(i..self.cursor);
                self.cursor = i;
            }
            0x14 => {
                // Ctrl+T: transpose the chars around the cursor and advance
                // (at end of line: swap the last two, stay).
                let len = self.chars.len();
                if self.cursor == len && len >= 2 {
                    self.chars.swap(len - 2, len - 1);
                } else if self.cursor > 0 && self.cursor < len {
                    self.chars.swap(self.cursor - 1, self.cursor);
                    self.cursor += 1;
                }
            }
            0x0c => {} // Ctrl+L clears the screen, the line survives
            0x00..=0x1f => {
                // Ctrl+R search, Ctrl+P/N history, Ctrl+Y yank, ...: the
                // resulting line depends on remote state we don't have.
                self.tainted = true;
            }
            _ => {
                if b < 0x80 {
                    self.insert(b as char);
                } else {
                    self.pending_utf8.push(b);
                    self.try_flush_utf8();
                }
            }
        }
    }

    fn esc(&mut self, b: u8) {
        match b {
            b'[' => {
                self.params.clear();
                self.scan = Scan::Csi;
            }
            b'O' => self.scan = Scan::Ss3,
            0x1b => {} // stay armed
            _ => {
                // Alt+key (word moves, Alt+d, Alt+BS, ...): remote word
                // boundaries, not modeled.
                self.tainted = true;
                self.scan = Scan::Ground;
            }
        }
    }

    fn csi(&mut self, b: u8) {
        match b {
            0x20..=0x3f => {
                if self.params.len() < 16 {
                    self.params.push(b as char);
                } else {
                    self.tainted = true;
                    self.scan = Scan::Ground;
                }
                return;
            }
            0x40..=0x7e => {}
            _ => {
                // Malformed CSI (C0 inside, ...): bail out conservatively.
                self.tainted = true;
                self.scan = Scan::Ground;
                return;
            }
        }
        let params = std::mem::take(&mut self.params);
        self.scan = Scan::Ground;
        if self.in_paste {
            // Only the paste terminator means anything mid-paste.
            if b == b'~' && params == "201" {
                self.in_paste = false;
            } else {
                self.tainted = true;
            }
            return;
        }
        match (b, params.as_str()) {
            (b'D', "") => self.cursor = self.cursor.saturating_sub(1),
            (b'C', "") => self.cursor = (self.cursor + 1).min(self.chars.len()),
            (b'H', "") | (b'~', "1") | (b'~', "7") => self.cursor = 0,
            (b'F', "") | (b'~', "4") | (b'~', "8") => self.cursor = self.chars.len(),
            (b'~', "3") => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            (b'~', "200") => self.in_paste = true,
            // Up/Down history, PgUp/PgDn, F-keys, modified arrows
            // (word-jumps), mouse reports...: not modeled.
            _ => self.tainted = true,
        }
    }

    fn ss3(&mut self, b: u8) {
        self.scan = Scan::Ground;
        match b {
            b'D' => self.cursor = self.cursor.saturating_sub(1),
            b'C' => self.cursor = (self.cursor + 1).min(self.chars.len()),
            b'H' => self.cursor = 0,
            b'F' => self.cursor = self.chars.len(),
            _ => self.tainted = true, // SS3 A/B arrows-as-history, F1-F4
        }
    }

    fn insert(&mut self, c: char) {
        if self.chars.len() >= MAX_LINE {
            self.tainted = true;
            return;
        }
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn try_flush_utf8(&mut self) {
        let need = match self.pending_utf8.first() {
            Some(b) if b >> 5 == 0b110 => 2,
            Some(b) if b >> 4 == 0b1110 => 3,
            Some(b) if b >> 3 == 0b11110 => 4,
            _ => {
                // Stray continuation / invalid lead byte: drop it.
                self.pending_utf8.clear();
                return;
            }
        };
        if self.pending_utf8.len() < need {
            return;
        }
        if let Ok(s) = std::str::from_utf8(&self.pending_utf8) {
            let chars: Vec<char> = s.chars().collect();
            for c in chars {
                self.insert(c);
            }
        }
        self.pending_utf8.clear();
    }

    fn clear_line(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.tainted = false;
        self.pending_utf8.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submit_one(tracker: &mut InputTracker, bytes: &[u8]) -> SubmittedLine {
        let mut lines = tracker.feed(bytes);
        assert_eq!(lines.len(), 1, "expected exactly one submitted line");
        lines.remove(0)
    }

    #[test]
    fn plain_typing_and_enter() {
        let mut t = InputTracker::new();
        let line = submit_one(&mut t, b"ls -la\r");
        assert_eq!(line.text, "ls -la");
        assert!(!line.tainted);
    }

    #[test]
    fn backspace_edits() {
        let mut t = InputTracker::new();
        let line = submit_one(&mut t, b"lss\x7f -la\r");
        assert_eq!(line.text, "ls -la");
        assert!(!line.tainted);
    }

    #[test]
    fn cursor_moves_and_midline_insert() {
        let mut t = InputTracker::new();
        // "cat x", Left x2 (before the space), insert " m", End, append "y".
        let line = submit_one(&mut t, b"cat x\x1b[D\x1b[D m\x1b[Fy\r");
        assert_eq!(line.text, "cat m xy");
        assert!(!line.tainted);
    }

    #[test]
    fn home_end_ctrl_a_e_and_kills() {
        let mut t = InputTracker::new();
        // Type, Ctrl+A, Ctrl+K kills everything.
        let line = submit_one(&mut t, b"garbage\x01\x0bls\r");
        assert_eq!(line.text, "ls");
        // Ctrl+U from end clears line.
        let line = submit_one(&mut t, b"noise\x15pwd\r");
        assert_eq!(line.text, "pwd");
        // Ctrl+W kills the previous word only.
        let line = submit_one(&mut t, b"git status extra\x17\r");
        assert_eq!(line.text, "git status ");
        assert!(!line.tainted);
    }

    #[test]
    fn delete_key_and_ctrl_d() {
        let mut t = InputTracker::new();
        // "lxs": Home, Right, Delete removes the x.
        let line = submit_one(&mut t, b"lxs\x1b[H\x1b[C\x1b[3~\r");
        assert_eq!(line.text, "ls");
        // Ctrl+D at cursor.
        let line = submit_one(&mut t, b"lxs\x01\x06\x04\r");
        assert_eq!(line.text, "ls");
        assert!(!line.tainted);
    }

    #[test]
    fn up_arrow_taints_ctrl_c_recovers() {
        let mut t = InputTracker::new();
        let line = submit_one(&mut t, b"\x1b[A\r");
        assert!(line.tainted, "history recall must taint");
        // After the tainted submit the next line is clean again.
        let line = submit_one(&mut t, b"ls\r");
        assert!(!line.tainted);
        // Taint then Ctrl+C then fresh typing: clean.
        let mut lines = t.feed(b"\x1b[A\x03");
        assert!(lines.is_empty());
        let line = submit_one(&mut t, b"pwd\r");
        assert_eq!(line.text, "pwd");
        assert!(!line.tainted);
        let _ = lines.pop();
    }

    #[test]
    fn tab_and_alt_taint() {
        let mut t = InputTracker::new();
        let line = submit_one(&mut t, b"cd pro\x09\r");
        assert!(line.tainted, "tab completion must taint");
        let line = submit_one(&mut t, b"ls x\x1bb\r");
        assert!(line.tainted, "Alt+b word move must taint");
    }

    #[test]
    fn ss3_application_cursor_arrows() {
        let mut t = InputTracker::new();
        // DECCKM arrows: ESC O D (left) then type.
        let line = submit_one(&mut t, b"ab\x1bODc\r");
        assert_eq!(line.text, "acb");
        assert!(!line.tainted);
        let line = submit_one(&mut t, b"\x1bOA\r");
        assert!(line.tainted, "SS3 up-arrow must taint");
    }

    #[test]
    fn bracketed_paste_inserts_literally() {
        let mut t = InputTracker::new();
        // Paste "echo a\necho b" wrapped in brackets, then Enter executes.
        let mut lines = t.feed(b"\x1b[200~echo a\recho b\x1b[201~\r");
        assert_eq!(lines.len(), 1);
        let line = lines.remove(0);
        assert_eq!(line.text, "echo a\necho b");
        assert!(!line.tainted);
    }

    #[test]
    fn bracketed_paste_c0_bytes_insert_literally() {
        let mut t = InputTracker::new();
        // Ctrl+A / Ctrl+U / Ctrl+W / DEL inside a paste are payload, not
        // line editing: nothing moves the cursor or kills text, and the
        // catch-all C0 taint must not fire either.
        let line = submit_one(&mut t, b"\x1b[200~a\x01b\x15c\x17d\x7fe\x1b[201~\r");
        assert_eq!(line.text, "a\x01b\x15c\x17d\x7fe");
        assert!(!line.tainted);
    }

    #[test]
    fn bracketed_paste_ctrl_c_backspace_and_tab_are_literal() {
        let mut t = InputTracker::new();
        // Ctrl+C must not abort the line, backspace must not delete the
        // typed prefix, and TAB is content rather than completion.
        let line = submit_one(&mut t, b"pre \x1b[200~ab\x03c\x08d\te\x1b[201~\r");
        assert_eq!(line.text, "pre ab\x03c\x08d\te");
        assert!(!line.tainted);
    }

    #[test]
    fn unbracketed_multiline_paste_submits_each_line() {
        let mut t = InputTracker::new();
        let lines = t.feed(b"echo a\recho b\r");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "echo a");
        assert_eq!(lines[1].text, "echo b");
    }

    #[test]
    fn utf8_split_across_feeds() {
        let mut t = InputTracker::new();
        // "é" = 0xC3 0xA9 split across two feeds.
        assert!(t.feed(b"caf\xc3").is_empty());
        let line = submit_one(&mut t, b"\xa9\r");
        assert_eq!(line.text, "café");
        assert!(!line.tainted);
    }

    #[test]
    fn ctrl_t_transpose() {
        let mut t = InputTracker::new();
        let line = submit_one(&mut t, b"sl\x14\r");
        assert_eq!(line.text, "ls");
        assert!(!line.tainted);
    }

    #[test]
    fn overflow_taints() {
        let mut t = InputTracker::new();
        let big = vec![b'a'; MAX_LINE + 10];
        let _ = t.feed(&big);
        let line = submit_one(&mut t, b"\r");
        assert!(line.tainted);
        assert_eq!(line.text.len(), MAX_LINE);
    }
}
