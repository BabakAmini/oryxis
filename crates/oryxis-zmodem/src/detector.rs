//! Auto-detection of a ZMODEM transfer in a terminal output stream.
//!
//! When a user runs `sz file` or `rz` on the remote, lrzsz emits a
//! ZMODEM initiation header into the terminal output. A terminal that
//! "supports ZMODEM" watches for that header and hands the byte channel
//! to a transfer engine. This is that watcher, structured exactly like
//! `oryxis-terminal`'s `OscSniffer`: a filter that passes normal bytes
//! through to the emulator, holds back a trailing partial match across
//! chunk boundaries, and reports the split point where the ZMODEM wire
//! stream begins.
//!
//! The initiation frames are both ZHEX headers (RFC-less; the lrzsz
//! source is the spec):
//!
//! - `** <ZDLE> B 0 0` = ZRQINIT: the remote is a *sender* (`sz`), so we
//!   become the receiver and DOWNLOAD.
//! - `** <ZDLE> B 0 1` = ZRINIT: the remote is a *receiver* (`rz`), so
//!   we become the sender and UPLOAD.
//!
//! `ZDLE` (0x18) after `**` is a control byte that never appears in
//! ordinary text next to two asterisks, so false positives are
//! negligible; only these two exact type codes trigger, anything else
//! (a stray ZHEX-looking byte run) passes through untouched.

const ZPAD: u8 = b'*'; // 0x2A
const ZDLE: u8 = 0x18;
const ZHEX: u8 = b'B'; // ZHEX encoding marker

/// The 6-byte initiation triggers. The common 5-byte prefix is what the
/// filter holds back when a chunk ends mid-header.
const TRIG_DOWNLOAD: [u8; 6] = [ZPAD, ZPAD, ZDLE, ZHEX, b'0', b'0']; // ZRQINIT
const TRIG_UPLOAD: [u8; 6] = [ZPAD, ZPAD, ZDLE, ZHEX, b'0', b'1']; // ZRINIT
const TRIG_PREFIX: [u8; 5] = [ZPAD, ZPAD, ZDLE, ZHEX, b'0'];

/// Which side of the transfer we take, decided by the initiation frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Remote `sz`: we receive the file (download).
    Download,
    /// Remote `rz`: we send a file (upload).
    Upload,
}

/// Result of feeding one output chunk through the detector.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Bytes to forward to the terminal emulator as usual.
    pub clean: Vec<u8>,
    /// `Some` when a transfer starts in this chunk; `clean` holds
    /// everything before the header and `wire` holds the header plus
    /// any following bytes (the first input for the transfer engine).
    pub detection: Option<Direction>,
    /// The initial ZMODEM wire bytes (from the header onward). Empty
    /// unless `detection` is `Some`.
    pub wire: Vec<u8>,
}

/// Streaming ZMODEM initiation detector.
#[derive(Default)]
pub struct ZmodemDetector {
    /// A trailing partial-header suffix held back from the previous
    /// chunk (at most 5 bytes: the `TRIG_PREFIX` length minus one is
    /// the longest ambiguous hold, but we may hold the full 5-byte
    /// prefix while awaiting the 6th type byte).
    pending: Vec<u8>,
}

impl ZmodemDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one output chunk. Until a transfer is detected, `clean`
    /// carries the bytes for the emulator (a trailing partial header is
    /// withheld until the next chunk resolves it). On detection, the
    /// caller must route every subsequent pane byte into the transfer
    /// engine, starting with `wire`, and stop calling `feed` until the
    /// transfer ends.
    pub fn feed(&mut self, chunk: &[u8]) -> Scan {
        // Reunite any held-back partial with the new bytes so a header
        // split across the boundary is seen whole.
        let mut buf = core::mem::take(&mut self.pending);
        buf.extend_from_slice(chunk);

        // First full trigger anywhere in the buffer wins.
        if let Some((idx, dir)) = find_trigger(&buf) {
            let clean = buf[..idx].to_vec();
            let wire = buf[idx..].to_vec();
            return Scan {
                clean,
                detection: Some(dir),
                wire,
            };
        }

        // No full trigger: hold back the longest trailing suffix that is
        // a prefix of a potential header, forward the rest.
        let hold = trailing_prefix_len(&buf);
        let split = buf.len() - hold;
        self.pending = buf[split..].to_vec();
        Scan {
            clean: buf[..split].to_vec(),
            detection: None,
            wire: Vec::new(),
        }
    }

    /// Drop any withheld partial back to the caller (as clean bytes) and
    /// reset. Called when the pane leaves any ZMODEM-eligible state
    /// (e.g. the session closes) so a dangling partial never strands
    /// bytes that turned out to be ordinary output.
    pub fn flush(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.pending)
    }
}

/// Find the first full initiation header in `buf`, returning its start
/// index and the resulting direction.
fn find_trigger(buf: &[u8]) -> Option<(usize, Direction)> {
    if buf.len() < 6 {
        return None;
    }
    for i in 0..=buf.len() - 6 {
        let window = &buf[i..i + 6];
        if window == TRIG_DOWNLOAD {
            return Some((i, Direction::Download));
        }
        if window == TRIG_UPLOAD {
            return Some((i, Direction::Upload));
        }
    }
    None
}

/// Length of the longest suffix of `buf` that is a (proper) prefix of a
/// potential header, i.e. matches `TRIG_PREFIX[..k]`. That suffix is
/// held back until the next chunk can complete or refute it.
///
/// A lone trailing `*` (k == 1) is deliberately NOT held: a single
/// asterisk is far too common at a batch boundary (`ls -F` marks
/// executables with `*`, prompts end in it), and holding one back
/// strands it on screen until the next output arrives. The only cost of
/// not holding it is missing a header whose two `**` bytes are split
/// across the exact batch boundary, which is vanishingly rare (the pair
/// is one write) and self-heals: lrzsz retransmits ZRQINIT on timeout,
/// and the retransmit won't land on the same split.
fn trailing_prefix_len(buf: &[u8]) -> usize {
    let max = TRIG_PREFIX.len().min(buf.len());
    for k in (2..=max).rev() {
        if buf[buf.len() - k..] == TRIG_PREFIX[..k] {
            return k;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_output_passes_through_untouched() {
        let mut d = ZmodemDetector::new();
        let scan = d.feed(b"ls -la\r\n$ echo hi\r\n");
        assert_eq!(scan.clean, b"ls -la\r\n$ echo hi\r\n".to_vec());
        assert_eq!(scan.detection, None);
        assert!(scan.wire.is_empty());
    }

    #[test]
    fn detects_download_and_splits_the_stream() {
        let mut d = ZmodemDetector::new();
        // sz prints a hint then the ZRQINIT header + first data bytes.
        let mut input = b"rz waiting to receive.**\x18B00".to_vec();
        input.extend_from_slice(&[0x30, 0x30, 0x30, 0x30]); // trailing header payload
        let scan = d.feed(&input);
        assert_eq!(scan.detection, Some(Direction::Download));
        assert_eq!(scan.clean, b"rz waiting to receive.".to_vec());
        assert!(scan.wire.starts_with(b"**\x18B00"));
    }

    #[test]
    fn detects_upload() {
        let mut d = ZmodemDetector::new();
        let scan = d.feed(b"**\x18B01000000");
        assert_eq!(scan.detection, Some(Direction::Upload));
        assert!(scan.wire.starts_with(b"**\x18B01"));
        assert!(scan.clean.is_empty());
    }

    #[test]
    fn header_split_across_chunks_still_detects() {
        let mut d = ZmodemDetector::new();
        // First chunk ends mid-header: the partial is held back, not
        // leaked to the terminal.
        let s1 = d.feed(b"done.**\x18");
        assert_eq!(s1.clean, b"done.".to_vec());
        assert_eq!(s1.detection, None);
        let s2 = d.feed(b"B00zzzz");
        assert_eq!(s2.detection, Some(Direction::Download));
        assert!(s2.wire.starts_with(b"**\x18B00"));
    }

    #[test]
    fn boundary_split_at_every_offset_detects() {
        let full = b"before**\x18B01after".to_vec();
        // The header's first `*` is at index 6; a cut at 7 falls between
        // the two ZPADs. Because a lone trailing `*` is not held (see
        // `trailing_prefix_len`), that single split misses, which
        // self-heals via lrzsz's ZRQINIT retransmit. Every OTHER split
        // must still detect.
        const BETWEEN_ZPADS: usize = 7;
        for cut in 0..full.len() {
            let mut d = ZmodemDetector::new();
            let mut seen_dir = None;
            let mut wire = Vec::new();
            for chunk in [&full[..cut], &full[cut..]] {
                let scan = d.feed(chunk);
                if let Some(dir) = scan.detection {
                    seen_dir = Some(dir);
                    wire = scan.wire;
                }
            }
            if cut == BETWEEN_ZPADS {
                assert_eq!(seen_dir, None, "between-ZPADs split should miss");
            } else {
                assert_eq!(seen_dir, Some(Direction::Upload), "cut at {cut}");
                assert!(wire.starts_with(b"**\x18B01"), "cut at {cut}");
            }
        }
    }

    #[test]
    fn asterisks_without_the_control_byte_are_not_a_trigger() {
        let mut d = ZmodemDetector::new();
        // Markdown-ish text and shell globs must never trip detection.
        let scan = d.feed(b"**bold** and rm ** and ***");
        assert_eq!(scan.detection, None);
        // The trailing `**` is a valid 2-byte header prefix, so it is
        // held back; the rest reaches the terminal.
        assert_eq!(scan.clean, b"**bold** and rm ** and *".to_vec());
        let scan2 = d.feed(b"x");
        // The held `**` plus `x` resolve to ordinary output (no header).
        assert_eq!(scan2.clean, b"**x".to_vec());
        assert_eq!(scan2.detection, None);
    }

    #[test]
    fn near_miss_type_code_passes_through() {
        let mut d = ZmodemDetector::new();
        // `**\x18B0` followed by a non-0/1 type is not ZRQINIT/ZRINIT.
        let scan = d.feed(b"**\x18B05rest");
        assert_eq!(scan.detection, None);
        assert_eq!(scan.clean, b"**\x18B05rest".to_vec());
    }

    #[test]
    fn lone_trailing_asterisk_is_not_held() {
        let mut d = ZmodemDetector::new();
        // `ls -F` output ending on an executable's `*` at a batch
        // boundary must reach the terminal immediately, not stick.
        let scan = d.feed(b"run.sh*");
        assert_eq!(scan.clean, b"run.sh*".to_vec());
        assert_eq!(scan.detection, None);
        assert!(d.flush().is_empty(), "nothing should be held");
        // Two asterisks (a real header prefix) are still held.
        let scan = d.feed(b"foo**");
        assert_eq!(scan.clean, b"foo".to_vec());
        assert_eq!(d.flush(), b"**".to_vec());
    }

    #[test]
    fn flush_returns_a_held_partial() {
        let mut d = ZmodemDetector::new();
        let scan = d.feed(b"tail**\x18");
        assert_eq!(scan.clean, b"tail".to_vec());
        // The 3-byte partial is held; flush hands it back.
        assert_eq!(d.flush(), b"**\x18".to_vec());
        assert!(d.flush().is_empty());
    }
}
