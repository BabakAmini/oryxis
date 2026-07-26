//! GNU screen window-title sequences (`ESC k <text> ST`).
//!
//! `ESC k` is screen's "set window name" sequence, terminated by ST
//! (`ESC \`). It is not a VT sequence: the `vte` parser dispatches
//! `ESC k` as an unhandled escape and then PRINTS the title text as
//! ordinary output. That is harmless as long as nothing emits it, but
//! RHEL / CentOS ship this in `/etc/bashrc`:
//!
//! ```text
//! screen*)
//!   PROMPT_COMMAND='printf "\033k%s@%s:%s\033\\" "${USER}" "${HOSTNAME%%.*}" "${PWD/#$HOME/~}"'
//! ```
//!
//! so any host we connect to with a `screen*` TERM paints
//! `user@host:cwd` in front of every prompt (issue #88). Worse, the
//! shell does not count those columns, so readline's redraw (Ctrl+R,
//! long history lines) lands in the wrong place.
//!
//! We announce `screen-256color` whenever the host lacks the requested
//! entry, so we have to speak it: this filter strips the sequence out of
//! the stream and surfaces the title, which is what screen and tmux do
//! with it.
//!
//! Bytes are never dropped on a malformed run. A control byte that
//! cannot appear inside a title (newline, carriage return, NUL) or a
//! title longer than [`MAX_TITLE`] aborts the scan and re-emits
//! everything verbatim, so binary output that happens to contain
//! `ESC k` renders exactly as it did before.

use std::borrow::Cow;

/// Longest title we accept before deciding the run is not a title.
const MAX_TITLE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// Outside any sequence.
    Normal,
    /// Saw ESC; the next byte decides whether this is `ESC k`.
    Esc,
    /// Inside the title body, collecting until ST / BEL.
    Body,
    /// Saw ESC inside the body: `ESC \` ends it, anything else aborts.
    BodyEsc,
}

/// Streaming filter for `ESC k … ST`. Keeps its state across calls, so a
/// sequence split across two PTY reads is still recognised.
#[derive(Debug, Default)]
pub struct ScreenTitleFilter {
    scan: ScanState,
}

#[derive(Debug)]
struct ScanState {
    scan: Scan,
    /// Title bytes collected so far (body only, no ESC k prefix).
    body: Vec<u8>,
}

impl Default for ScanState {
    fn default() -> Self {
        Self { scan: Scan::Normal, body: Vec::new() }
    }
}

impl ScreenTitleFilter {
    /// Remove every complete `ESC k … ST` run from `bytes`.
    ///
    /// Returns the bytes the emulator should see plus the titles found,
    /// in order. The common case (no ESC at all, or an ESC that starts
    /// some other sequence) borrows the input and allocates nothing.
    pub fn filter<'a>(&mut self, bytes: &'a [u8]) -> (Cow<'a, [u8]>, Vec<String>) {
        let mut titles = Vec::new();

        // Fast path: nothing pending and no ESC in this chunk.
        if self.scan.scan == Scan::Normal && !bytes.contains(&0x1b) {
            return (Cow::Borrowed(bytes), titles);
        }

        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        for &b in bytes {
            match self.scan.scan {
                Scan::Normal => {
                    if b == 0x1b {
                        // Hold the ESC back until we know what follows.
                        self.scan.scan = Scan::Esc;
                    } else {
                        out.push(b);
                    }
                }
                Scan::Esc => match b {
                    b'k' => {
                        self.scan.scan = Scan::Body;
                        self.scan.body.clear();
                    }
                    0x1b => out.push(0x1b), // ESC ESC: emit one, stay armed
                    _ => {
                        // Some other escape: hand both bytes back untouched.
                        out.push(0x1b);
                        out.push(b);
                        self.scan.scan = Scan::Normal;
                    }
                },
                Scan::Body => match b {
                    0x07 => {
                        // BEL also terminates in the wild.
                        titles.push(take_title(&mut self.scan.body));
                        self.scan.scan = Scan::Normal;
                    }
                    0x1b => self.scan.scan = Scan::BodyEsc,
                    b'\n' | b'\r' | 0x00 => {
                        // Cannot be a title: replay verbatim, lose nothing.
                        replay(&mut out, &mut self.scan.body);
                        out.push(b);
                        self.scan.scan = Scan::Normal;
                    }
                    _ => {
                        if self.scan.body.len() >= MAX_TITLE {
                            replay(&mut out, &mut self.scan.body);
                            out.push(b);
                            self.scan.scan = Scan::Normal;
                        } else {
                            self.scan.body.push(b);
                        }
                    }
                },
                Scan::BodyEsc => match b {
                    b'\\' => {
                        // ST: the sequence is complete.
                        titles.push(take_title(&mut self.scan.body));
                        self.scan.scan = Scan::Normal;
                    }
                    0x1b => {} // another ESC, keep waiting for the backslash
                    _ => {
                        // ESC + something else inside the body: not a title
                        // we understand. Replay what we swallowed.
                        replay(&mut out, &mut self.scan.body);
                        out.push(0x1b);
                        out.push(b);
                        self.scan.scan = Scan::Normal;
                    }
                },
            }
        }
        (Cow::Owned(out), titles)
    }
}

/// Put an aborted run back on the wire exactly as it arrived, `ESC k`
/// prefix included, so output is never silently swallowed.
fn replay(out: &mut Vec<u8>, body: &mut Vec<u8>) {
    out.push(0x1b);
    out.push(b'k');
    out.append(body);
}

fn take_title(body: &mut Vec<u8>) -> String {
    let bytes = std::mem::take(body);
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: &mut ScreenTitleFilter, input: &[u8]) -> (Vec<u8>, Vec<String>) {
        let (out, titles) = f.filter(input);
        (out.into_owned(), titles)
    }

    #[test]
    fn strips_a_complete_sequence_and_reports_the_title() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"a\x1bkroot@oldserver:~\x1b\\b");
        assert_eq!(out, b"ab");
        assert_eq!(titles, vec!["root@oldserver:~".to_string()]);
    }

    #[test]
    fn the_centos_prompt_command_leaves_no_text_behind() {
        // Exactly what /etc/bashrc emits on a screen* TERM (issue #88).
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"\x1bkroot@oldserver:~\x1b\\[root@oldserver ~]# ");
        assert_eq!(out, b"[root@oldserver ~]# ");
        assert_eq!(titles, vec!["root@oldserver:~".to_string()]);
    }

    #[test]
    fn bel_terminates_too() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"\x1bktitle\x07rest");
        assert_eq!(out, b"rest");
        assert_eq!(titles, vec!["title".to_string()]);
    }

    #[test]
    fn a_sequence_split_across_reads_is_still_recognised() {
        let mut f = ScreenTitleFilter::default();
        let (out1, t1) = run(&mut f, b"x\x1bkroo");
        assert_eq!(out1, b"x");
        assert!(t1.is_empty());
        let (out2, t2) = run(&mut f, b"t@host\x1b");
        assert_eq!(out2, b"");
        assert!(t2.is_empty());
        let (out3, t3) = run(&mut f, b"\\y");
        assert_eq!(out3, b"y");
        assert_eq!(t3, vec!["root@host".to_string()]);
    }

    #[test]
    fn an_empty_title_is_still_a_title() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"\x1bk\x1b\\");
        assert_eq!(out, b"");
        assert_eq!(titles, vec![String::new()]);
    }

    #[test]
    fn other_escapes_pass_through_untouched() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"\x1b[31mred\x1b[0m\x1b7\x1b8");
        assert_eq!(out, b"\x1b[31mred\x1b[0m\x1b7\x1b8");
        assert!(titles.is_empty());
    }

    #[test]
    fn a_newline_inside_the_body_replays_every_byte() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"\x1bkgarbage\nmore");
        assert_eq!(out, b"\x1bkgarbage\nmore");
        assert!(titles.is_empty());
    }

    #[test]
    fn an_overlong_body_replays_every_byte() {
        let mut f = ScreenTitleFilter::default();
        let mut input = b"\x1bk".to_vec();
        input.extend(std::iter::repeat_n(b'x', MAX_TITLE + 5));
        let (out, titles) = run(&mut f, &input);
        assert_eq!(out, input);
        assert!(titles.is_empty());
    }

    #[test]
    fn an_esc_that_is_not_st_inside_the_body_replays() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"\x1bkabc\x1b[0m");
        assert_eq!(out, b"\x1bkabc\x1b[0m");
        assert!(titles.is_empty());
    }

    #[test]
    fn plain_output_is_borrowed_not_copied() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = f.filter(b"just some output\r\n");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert!(titles.is_empty());
    }

    #[test]
    fn two_sequences_in_one_chunk_both_report() {
        let mut f = ScreenTitleFilter::default();
        let (out, titles) = run(&mut f, b"\x1bkone\x1b\\mid\x1bktwo\x1b\\end");
        assert_eq!(out, b"midend");
        assert_eq!(titles, vec!["one".to_string(), "two".to_string()]);
    }
}
