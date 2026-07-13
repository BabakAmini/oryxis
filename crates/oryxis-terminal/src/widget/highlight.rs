use super::*;

#[derive(Clone, Copy, PartialEq)]
enum HighlightKind {
    Url,
    Ip,
    Path,
    Number,
    /// `user@host` prompt token (`root@web`). Detected only when Privacy
    /// Mode is on; never colored (see [`highlight_color_at`]), it exists
    /// solely so the draw pass can mask it. Also catches emails / typed
    /// `ssh user@host` targets, which are sensitive too.
    HostUser,
    /// The `<name>` segment of a home-directory path (`C:\Users\<name>`,
    /// `/home/<name>`, `/Users/<name>`). It identifies the local account
    /// just like a prompt `user@host` does, so Privacy Mode masks it.
    /// Same contract as [`HighlightKind::HostUser`]: privacy-only, never
    /// colored.
    UserDir,
    /// An exact occurrence of a saved connection's hostname (passed in by
    /// the app as a privacy term). Plain DNS names have no detectable
    /// shape (file extensions collide with ccTLDs: `main.rs`,
    /// `install.sh` are FQDN-shaped), so the known values are matched
    /// literally instead. Privacy-only, never colored.
    KnownHost,
    /// A range-valid quad-dot token classified as a version string
    /// (`3.9.0.2` in a winget upgrade table) rather than an address, per
    /// issue #53. Colors exactly like [`HighlightKind::Ip`] but is
    /// excluded from Privacy Mode masking; the classification is
    /// [`quad_dot_is_version_like`], with vault-term and private-range
    /// overrides forcing `Ip` before the heuristics run.
    VersionQuad,
}

impl HighlightKind {
    /// Privacy-Mode-only markers: masked by the draw pass, never used as
    /// a keyword-highlight color.
    fn privacy_only(self) -> bool {
        matches!(self, Self::HostUser | Self::UserDir | Self::KnownHost)
    }
}

/// Whether a hex-digit/colon run is IPv6-shaped: the full 8-group form
/// (exactly 7 colons) or the `::`-compressed form, groups of 1-4 hex
/// digits, at most one `::`, no `:::`. A run without `::` and without the
/// full form's 7 colons is rejected, which keeps timestamps (`12:34:56`)
/// and MAC addresses (`aa:bb:cc:dd:ee:ff`) out. Shared by the terminal
/// highlighter and the app-side session-log redaction so both agree on
/// what gets masked; callers are responsible for context (a run glued to
/// a word, like `std::io`, is theirs to reject).
pub fn looks_like_ipv6(run: &str) -> bool {
    let bytes = run.as_bytes();
    if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_hexdigit() || *b == b':') {
        return false;
    }
    if run.contains(":::") || run.matches("::").count() > 1 {
        return false;
    }
    let groups: Vec<&str> = run.split(':').filter(|g| !g.is_empty()).collect();
    if groups.is_empty() || groups.iter().any(|g| g.len() > 4) {
        return false;
    }
    if run.contains("::") {
        // `::` stands for at least one zero group, so at most 7 explicit
        // groups remain; a single leading/trailing colon that isn't part
        // of the `::` is malformed.
        groups.len() <= 7
            && (!run.starts_with(':') || run.starts_with("::"))
            && (!run.ends_with(':') || run.ends_with("::"))
    } else {
        bytes.iter().filter(|b| **b == b':').count() == 7 && groups.len() == 8
    }
}

/// Byte spans (`start..end`, end exclusive) of range-valid IPv4-shaped
/// tokens in a row: exactly 4 dot-separated groups of 1-3 digits, each
/// `<= 255`, not glued to an alphanumeric or `.` on either side. This is
/// the syntactic candidate set of the IPv4 highlight pass; whether a
/// candidate masks as an address or stays readable as a version string is
/// decided per candidate by the version classifier and its overrides.
pub fn scan_quad_dot_candidates(row: &str) -> Vec<(usize, usize)> {
    let bytes = row.as_bytes();
    let len = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < len {
        if bytes[i].is_ascii_digit() {
            if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.') {
                i += 1;
                continue;
            }
            let start = i;
            let mut groups = 0u8;
            let mut j = i;
            loop {
                let group_start = j;
                while j < len && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                let group_len = j - group_start;
                if group_len == 0 || group_len > 3 {
                    break;
                }
                if let Ok(val) = row[group_start..j].parse::<u16>() {
                    if val > 255 { break; }
                } else {
                    break;
                }
                groups += 1;
                if groups == 4 { break; }
                if j < len && bytes[j] == b'.' {
                    j += 1;
                } else {
                    break;
                }
            }
            if groups == 4 {
                if j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.') {
                    i += 1;
                    continue;
                }
                out.push((start, j));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Version-context signals for a range-valid quad-dot candidate at
/// `row[start..]` (issue #53). PER-CANDIDATE only: the evidence must be
/// glued to THIS token, never merely somewhere on the row. A masking
/// candidate is by definition a valid all-<=255 dotted quad, so it is
/// byte-for-byte indistinguishable from an IPv4 address; only a marker
/// attached to the token itself tells the two apart, either a
/// version-word right before it (`v` / `ver` / `version` / `versão`,
/// with or without a trailing colon: `v 1.2.3.4`, `version: 1.2.3.4`,
/// `pandoc version 1.2.3.4`) or a `product/1.2.3.4` agent string
/// (slash-terminated word).
///
/// Row-wide signals (a keyword anywhere on the line, or a sibling
/// version token elsewhere) are deliberately NOT used: they would
/// unmask an unrelated public IP that happens to share the line (an
/// access-log or `ip route` row), which is the exact leak class #53's
/// masking exists to prevent. In Privacy Mode the safe error is to mask,
/// so a bare `3.9.0.2` with no local marker classifies as an address.
/// A directly-attached `v3.9.0.2` never reaches here; the scanner's glue
/// rule already dropped it.
fn quad_dot_version_context(row: &str, start: usize) -> bool {
    // The token immediately before the candidate, split on ANY ASCII
    // whitespace (a tab-separated `version:\t1.2.3.4` is still glued).
    let trimmed = row[..start].trim_end_matches([' ', '\t']);
    let tok = &trimmed[trimmed.rfind([' ', '\t']).map(|p| p + 1).unwrap_or(0)..];
    if tok.is_empty() {
        return false;
    }
    // Unicode-aware lowercase so `VERSÃO` folds to `versão` (the ASCII
    // fold would leave `Ã` and miss the pt-BR keyword).
    let lower = tok.to_lowercase();
    let word = lower.strip_suffix(':').unwrap_or(&lower);
    if matches!(word, "v" | "ver" | "version" | "versão") {
        return true;
    }
    let tb = tok.as_bytes();
    tb.len() >= 2 && tb[tb.len() - 1] == b'/' && tb[tb.len() - 2].is_ascii_alphabetic()
}

/// Whether the range-valid quad-dot at `row[start..end]` reads as a
/// version string rather than an address. Shared with the app-side
/// session-log redaction (`redact_for_display`) so live terminal and
/// recorded output classify identically. Callers apply the two masking
/// overrides FIRST (vault-term hit, [`ipv4_is_private_or_loopback`]);
/// those always win over version context.
pub fn quad_dot_is_version_like(row: &str, start: usize, _end: usize) -> bool {
    quad_dot_version_context(row, start)
}

/// Whether an IPv4 candidate sits in private/loopback/link-local space
/// (10/8, 127/8, 169.254/16, 172.16/12, 192.168/16). Privacy Mode always
/// masks these even in a version-like row: a version string colliding
/// with RFC1918 space is rare and masking is the safe error.
pub fn ipv4_is_private_or_loopback(candidate: &str) -> bool {
    let mut octets = candidate.split('.').map(|g| g.parse::<u8>().ok());
    let (Some(Some(a)), Some(Some(b))) = (octets.next(), octets.next()) else {
        return false;
    };
    a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
}

pub(crate) struct Highlight {
    row: u16,
    start_col: u16,
    end_col: u16, // inclusive
    color: Color,
    kind: HighlightKind,
}

/// Scan row text for IPv4/IPv6 addresses, URLs, and Unix file paths (no
/// regex). Takes `(row, non-blank cells)` pairs; rows with no printable
/// chars are simply absent (the draw pass builds this per frame, so a
/// dense Vec beats re-hashing every row into a map). `privacy_terms` are
/// extra strings (saved-connection hostnames, lowercase) masked wherever
/// they appear, Privacy Mode only.
pub(crate) fn detect_highlights(
    row_chars: &[(u16, Vec<(u16, char)>)],
    palette: &TerminalPalette,
    privacy: bool,
    privacy_terms: &[String],
) -> Vec<Highlight> {
    let ip_color = palette.ansi[5];   // magenta
    let url_color = palette.ansi[4];  // blue
    let path_color = palette.ansi[6]; // cyan
    let num_color = palette.ansi[5];  // magenta, same as IP, easy scan

    let mut highlights = Vec::new();

    for (row, cols) in row_chars {
        let row = *row;
        let max_col = cols.iter().map(|(c, _)| *c).max().unwrap_or(0) as usize;
        let mut chars = vec![' '; max_col + 1];
        for &(col, ch) in cols {
            if (col as usize) <= max_col {
                chars[col as usize] = ch;
            }
        }
        let row_str: String = chars.iter().collect();
        let bytes = row_str.as_bytes();
        let len = bytes.len();

        // --- URLs: "http://" or "https://" followed by non-whitespace ---
        {
            let mut i = 0;
            while i < len {
                // Only slice at ASCII 'h', guaranteed char boundary. Skipping this
                // guard panics when i lands mid-UTF-8 (e.g. typing "ç" crashed the app).
                if bytes[i] != b'h' {
                    i += 1;
                    continue;
                }
                let rest = &row_str[i..];
                if rest.starts_with("http://") || rest.starts_with("https://") {
                    let start = i;
                    let mut end = i;
                    for ch in row_str[i..].chars() {
                        if ch.is_whitespace() || ch == '\0' {
                            break;
                        }
                        end += ch.len_utf8();
                    }
                    if end > start {
                        while end > start {
                            let last = bytes[end - 1];
                            if last == b')' || last == b']' || last == b'>'
                                || last == b',' || last == b'.' || last == b';'
                            {
                                end -= 1;
                            } else {
                                break;
                            }
                        }
                        highlights.push(Highlight {
                            row,
                            start_col: start as u16,
                            end_col: (end - 1) as u16,
                            color: url_color,
                            kind: HighlightKind::Url,
                        });
                        i = end;
                        continue;
                    }
                }
                i += 1;
            }
        }

        // --- IPv4: digit groups separated by dots (4 groups, each 0-255).
        // Version-shaped candidates (winget/rustc tables, issue #53)
        // classify as VersionQuad: same keyword color, excluded from
        // Privacy masking. Vault-saved addresses and private/loopback
        // ranges always stay Ip, overrides win over version context.
        {
            let candidates = scan_quad_dot_candidates(&row_str);
            for &(start, end) in &candidates {
                let dominated = highlights.iter().any(|h| {
                    h.row == row && start as u16 >= h.start_col && (start as u16) <= h.end_col
                });
                if dominated {
                    continue;
                }
                let text = &row_str[start..end];
                let kind = if privacy_terms.iter().any(|t| t == text)
                    || ipv4_is_private_or_loopback(text)
                    || !quad_dot_version_context(&row_str, start)
                {
                    HighlightKind::Ip
                } else {
                    HighlightKind::VersionQuad
                };
                highlights.push(Highlight {
                    row,
                    start_col: start as u16,
                    end_col: (end - 1) as u16,
                    color: ip_color,
                    kind,
                });
            }
        }

        // --- IPv6: hex-digit groups separated by colons, validated by
        // `looks_like_ipv6` (needs `::` or the full form's 7 colons, so
        // timestamps and MACs stay out). Runs glued to a word on either
        // side (std::io, Vec::new, beef42) are identifiers, not
        // addresses. A single leading/trailing colon is prose
        // punctuation and is trimmed off first. Same kind as IPv4:
        // colored by keyword highlighting, masked by Privacy Mode. An
        // embedded dotted-quad tail (`::ffff:192.0.2.1`) is already
        // covered by the IPv4 pass above; the two spans sit side by side.
        {
            let mut i = 0;
            while i < len {
                if !bytes[i].is_ascii_hexdigit() && bytes[i] != b':' {
                    i += 1;
                    continue;
                }
                // Take the whole run up front so a rejected start skips it
                // entirely instead of re-matching at every inner offset.
                let start = i;
                let mut j = i;
                while j < len && (bytes[j].is_ascii_hexdigit() || bytes[j] == b':') {
                    j += 1;
                }
                let glued = (start > 0
                    && (is_word_byte(bytes[start - 1]) || bytes[start - 1] == b'.'))
                    || (j < len && is_word_byte(bytes[j]));
                let mut s2 = start;
                let mut e2 = j;
                if e2 - s2 >= 2 && bytes[s2] == b':' && bytes[s2 + 1] != b':' {
                    s2 += 1;
                }
                if e2 - s2 >= 2 && bytes[e2 - 1] == b':' && bytes[e2 - 2] != b':' {
                    e2 -= 1;
                }
                let dominated = highlights.iter().any(|h| {
                    h.row == row && s2 as u16 >= h.start_col && (s2 as u16) <= h.end_col
                });
                if !glued && !dominated && looks_like_ipv6(&row_str[s2..e2]) {
                    highlights.push(Highlight {
                        row,
                        start_col: s2 as u16,
                        end_col: (e2 - 1) as u16,
                        color: ip_color,
                        kind: HighlightKind::Ip,
                    });
                }
                i = j;
            }
        }

        // --- user@host prompt tokens (Privacy Mode only): word@word ---
        // Anchored on '@' with token chars on both sides. Token chars are
        // alnum plus `. _ -` (host labels, usernames, email locals). This
        // catches the unix prompt (`root@web`), emails, and typed
        // `ssh user@host` targets, all of which are sensitive. Never
        // colored, only masked, so it runs solely under Privacy Mode.
        if privacy {
            let is_tok = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
            let mut i = 0;
            while i < len {
                if bytes[i] == b'@'
                    && i > 0
                    && i + 1 < len
                    && is_tok(bytes[i - 1])
                    && is_tok(bytes[i + 1])
                {
                    let mut start = i;
                    while start > 0 && is_tok(bytes[start - 1]) {
                        start -= 1;
                    }
                    let mut end = i + 1;
                    while end < len && is_tok(bytes[end]) {
                        end += 1;
                    }
                    highlights.push(Highlight {
                        row,
                        start_col: start as u16,
                        end_col: (end - 1) as u16,
                        color: ip_color,
                        kind: HighlightKind::HostUser,
                    });
                    i = end;
                    continue;
                }
                i += 1;
            }
        }

        // --- Home-directory usernames (Privacy Mode only): the `<name>` in
        // `C:\Users\<name>`, `/home/<name>` or `/Users/<name>` (Windows /
        // Linux / macOS prompts and paths). Only the name segment is
        // masked, the rest of the path stays readable. Markers compare
        // case-insensitively (`c:\users\` prompts exist too).
        if privacy {
            let is_tok = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
            const MARKERS: [&[u8]; 3] = [b"\\users\\", b"/home/", b"/users/"];
            let mut i = 0;
            while i < len {
                if bytes[i] != b'\\' && bytes[i] != b'/' {
                    i += 1;
                    continue;
                }
                let Some(mlen) = MARKERS
                    .iter()
                    .find(|m| i + m.len() <= len && bytes[i..i + m.len()].eq_ignore_ascii_case(m))
                    .map(|m| m.len())
                else {
                    i += 1;
                    continue;
                };
                let start = i + mlen;
                let mut end = start;
                while end < len && is_tok(bytes[end]) {
                    end += 1;
                }
                // A `/home/` or `/users/` inside a detected URL is a web
                // path (`https://cdn.io/users/42`), not this machine's
                // account name; leave those alone.
                let inside_url = highlights.iter().any(|h| {
                    h.kind == HighlightKind::Url
                        && h.row == row
                        && start as u16 >= h.start_col
                        && (start as u16) <= h.end_col
                });
                if end > start && !inside_url {
                    highlights.push(Highlight {
                        row,
                        start_col: start as u16,
                        end_col: (end - 1) as u16,
                        color: ip_color,
                        kind: HighlightKind::UserDir,
                    });
                }
                i = end.max(i + 1);
            }
        }

        // --- Saved-connection hostnames (Privacy Mode only): exact,
        // case-insensitive, token-bounded occurrences of the vault's host
        // addresses, provided by the app in `privacy_terms` (lowercase).
        // Plain DNS names have no detectable shape, file extensions
        // collide with ccTLDs (`main.rs`, `install.sh` are FQDN-shaped),
        // so the known values are matched literally instead of guessed.
        if privacy && !privacy_terms.is_empty() {
            let is_tok = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
            let lower = row_str.to_ascii_lowercase();
            for term in privacy_terms {
                let mut from = 0;
                while let Some(pos) = lower[from..].find(term.as_str()) {
                    let s0 = from + pos;
                    let e0 = s0 + term.len();
                    let bounded = (s0 == 0 || !is_tok(bytes[s0 - 1]))
                        && (e0 >= len || !is_tok(bytes[e0]));
                    if bounded {
                        highlights.push(Highlight {
                            row,
                            start_col: s0 as u16,
                            end_col: (e0 - 1) as u16,
                            color: ip_color,
                            kind: HighlightKind::KnownHost,
                        });
                    }
                    from = e0;
                }
            }
        }

        // --- Unix file paths: "/" followed by alphanumeric/dot/dash/underscore/slash ---
        {
            let mut i = 0;
            while i < len {
                if bytes[i] == b'/' {
                    if i > 0 {
                        let prev = bytes[i - 1];
                        if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'-' || prev == b'.' {
                            i += 1;
                            continue;
                        }
                    }
                    let start = i;
                    let mut j = i + 1;
                    while j < len {
                        let b = bytes[j];
                        if b.is_ascii_alphanumeric()
                            || b == b'.' || b == b'-' || b == b'_' || b == b'/' || b == b'~'
                        {
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    if j - start >= 3 {
                        while j > start + 1 && (bytes[j - 1] == b'.' || bytes[j - 1] == b'/') {
                            j -= 1;
                        }
                        let dominated = highlights.iter().any(|h| {
                            h.row == row && start as u16 >= h.start_col && (start as u16) <= h.end_col
                        });
                        if !dominated && j - start >= 3 {
                            highlights.push(Highlight {
                                row,
                                start_col: start as u16,
                                end_col: (j - 1) as u16,
                                color: path_color,
                                kind: HighlightKind::Path,
                            });
                        }
                        i = j;
                        continue;
                    }
                }
                i += 1;
            }
        }

        // --- Standalone numbers: int/float, optional minus, optional %.
        // Examples: 1634, -273.1, 23.3%, 0.0. Skipped when the run is part
        // of an existing highlight (IP/path/URL) or is inside a word.
        {
            let mut i = 0;
            while i < len {
                let b = bytes[i];
                let is_start = b.is_ascii_digit()
                    || (b == b'-'
                        && i + 1 < len
                        && bytes[i + 1].is_ascii_digit()
                        && (i == 0 || !is_word_byte(bytes[i - 1])));
                if !is_start {
                    i += 1;
                    continue;
                }
                // Reject when prefixed by a word character (e.g. "abc123",
                // version strings), those should keep the surrounding fg.
                if i > 0 && b.is_ascii_digit() && is_word_byte(bytes[i - 1]) {
                    i += 1;
                    continue;
                }
                let start = i;
                let mut j = i;
                if bytes[j] == b'-' {
                    j += 1;
                }
                while j < len && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                // Optional decimal part, must be `.<digit>+`.
                if j + 1 < len && bytes[j] == b'.' && bytes[j + 1].is_ascii_digit() {
                    j += 1;
                    while j < len && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                // Optional trailing percent.
                if j < len && bytes[j] == b'%' {
                    j += 1;
                }
                // Reject when followed by a letter (e.g. "10.0.0.1",
                // "v1.2-rc", the IP path already handled the first; we
                // also avoid colouring "rc" parts).
                if j < len && is_word_byte(bytes[j]) {
                    i = j;
                    continue;
                }
                let dominated = highlights.iter().any(|h| {
                    h.row == row
                        && start as u16 >= h.start_col
                        && (start as u16) <= h.end_col
                });
                if !dominated && j > start {
                    highlights.push(Highlight {
                        row,
                        start_col: start as u16,
                        end_col: (j - 1) as u16,
                        color: num_color,
                        kind: HighlightKind::Number,
                    });
                }
                i = j;
            }
        }
    }

    highlights
}

/// WCAG 2.x relative luminance for an sRGB colour in `[0, 1]`. Used by
/// the smart-contrast fallback to decide whether a too-close cell
/// should flip its foreground to white or near-black.
pub(crate) fn relative_luminance(c: Color) -> f32 {
    fn channel(v: f32) -> f32 {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// WCAG contrast ratio between two opaque colours: 1.0 = identical,
/// 21.0 = white-on-black. We trip the smart-contrast fallback below
/// `2.5`, well under the AA-body threshold of `4.5` so we only act
/// on visually disappearing pairs and leave merely-low-contrast
/// styling alone.
pub(crate) fn contrast_ratio(a: Color, b: Color) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (lighter, darker) = if la >= lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

#[inline]
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Check if a cell position falls within any highlight, returning the color.
#[inline]
pub(crate) fn highlight_color_at(highlights: &[Highlight], row: u16, col: u16) -> Option<Color> {
    for h in highlights {
        // HostUser / UserDir are Privacy-Mode-only markers, never a syntax
        // color: skip them so prompts aren't tinted when keyword
        // highlighting is on.
        if !h.kind.privacy_only() && h.row == row && col >= h.start_col && col <= h.end_col {
            return Some(h.color);
        }
    }
    None
}

/// Find the IP / `user@host` privacy span covering a cell, returning its
/// `(row, start_col, end_col)` (inclusive). Used by the draw pass to
/// reveal the span the cursor is over while the rest stay masked, the same
/// hover-reveal mechanic as [`hovered_url_range`].
#[inline]
pub(crate) fn privacy_span_at(
    highlights: &[Highlight],
    row: u16,
    col: u16,
) -> Option<(u16, u16, u16)> {
    highlights
        .iter()
        .find(|h| {
            (h.kind == HighlightKind::Ip || h.kind.privacy_only())
                && h.row == row
                && col >= h.start_col
                && col <= h.end_col
        })
        .map(|h| (h.row, h.start_col, h.end_col))
}

/// Whether a cell falls inside any IP / `user@host` privacy span. The draw
/// pass masks such cells (block glyph + muted color) unless they're in the
/// currently revealed span.
#[inline]
pub(crate) fn is_privacy_cell(highlights: &[Highlight], row: u16, col: u16) -> bool {
    highlights.iter().any(|h| {
        (h.kind == HighlightKind::Ip || h.kind.privacy_only())
            && h.row == row
            && col >= h.start_col
            && col <= h.end_col
    })
}

/// All privacy spans with their text, resolved from the same per-frame row
/// data the draw pass uses. The draw pass matches these against the
/// click-pinned value set so every occurrence of a pinned value stays
/// revealed, wherever it appears.
pub(crate) fn privacy_spans_with_text(
    highlights: &[Highlight],
    row_chars: &[(u16, Vec<(u16, char)>)],
) -> Vec<((u16, u16, u16), String)> {
    highlights
        .iter()
        .filter(|h| h.kind == HighlightKind::Ip || h.kind.privacy_only())
        .filter_map(|h| {
            let (_, cells) = row_chars.iter().find(|(r, _)| *r == h.row)?;
            let mut text = String::with_capacity((h.end_col - h.start_col + 1) as usize);
            for col in h.start_col..=h.end_col {
                text.push(
                    cells
                        .iter()
                        .find(|(c, _)| *c == col)
                        .map(|(_, ch)| *ch)
                        .unwrap_or(' '),
                );
            }
            Some(((h.row, h.start_col, h.end_col), text))
        })
        .collect()
}

/// Text of the privacy span covering a cell, scroll-aware. Rebuilds the
/// one grid row (the way `smart_span_at` does), reruns the privacy
/// detection on it, and returns the covered span's text. Drives the
/// click-to-pin reveal: the returned value keys the pinned set.
pub(crate) fn privacy_value_at_cell(
    term: &alacritty_terminal::Term<crate::backend::EventProxy>,
    palette: &TerminalPalette,
    privacy_terms: &[String],
    line: i32,
    col: u16,
) -> Option<String> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    let l = Line(line);
    if l < grid.topmost_line() || l > grid.bottommost_line() {
        return None;
    }
    let row = &grid[l];
    let ncols = grid.columns();
    let mut cols: Vec<(u16, char)> = Vec::new();
    for ci in 0..ncols {
        let c = row[Column(ci)].c;
        if c != ' ' && c != '\0' {
            cols.push((ci as u16, c));
        }
    }
    if cols.is_empty() {
        return None;
    }
    let rows = [(0u16, cols)];
    let highlights = detect_highlights(&rows, palette, true, privacy_terms);
    privacy_spans_with_text(&highlights, &rows)
        .into_iter()
        .find(|((_, sc, ec), _)| col >= *sc && col <= *ec)
        .map(|(_, text)| text)
}

/// Returns true when the given cell is part of a URL highlight, used by the
/// draw pass to paint an underline under clickable links.
#[inline]
/// Find the URL highlight that contains a specific cell, used by the
/// draw pass to underline only the URL the cursor is over (instead of
/// every URL in the viewport, which made even un-hovered links look
/// "linkable" with no Ctrl-click feedback).
pub(crate) fn hovered_url_range(
    highlights: &[Highlight],
    row: u16,
    col: u16,
) -> Option<(u16, u16, u16)> {
    highlights
        .iter()
        .find(|h| {
            h.kind == HighlightKind::Url
                && h.row == row
                && col >= h.start_col
                && col <= h.end_col
        })
        .map(|h| (h.row, h.start_col, h.end_col))
}

/// Extract the URL string at a given cell from the current viewport, if any.
/// Walks the row the cursor is on, finds the URL highlight that covers the
/// column, and returns the full URL text. Returns `None` when the click
/// lands outside any URL.
pub(crate) fn url_at_cell(
    term: &alacritty_terminal::Term<crate::backend::EventProxy>,
    target_line: i32,
    target_col: u16,
) -> Option<String> {
    use alacritty_terminal::index::{Column, Line};
    // Index the one grid row directly (the way `smart_span_at` does)
    // instead of walking the whole viewport display iterator to pick
    // a single row out of it. `target_line` is a grid line (scroll
    // adjusted, negative for scrollback), not an on-screen row, so
    // Ctrl+click and hover stay correct when scrolled into history.
    let grid = term.grid();
    let line = Line(target_line);
    if line < grid.topmost_line() || line > grid.bottommost_line() {
        return None;
    }
    let row_data = &grid[line];
    let ncols = grid.columns();
    let mut row_chars: Vec<(u16, char)> = Vec::with_capacity(ncols);
    for ci in 0..ncols {
        let c = row_data[Column(ci)].c;
        if c != ' ' && c != '\0' {
            row_chars.push((ci as u16, c));
        }
    }
    if row_chars.is_empty() {
        return None;
    }

    let max_col = row_chars.iter().map(|(c, _)| *c).max().unwrap_or(0) as usize;
    let mut chars = vec![' '; max_col + 1];
    for &(col, ch) in &row_chars {
        if (col as usize) <= max_col {
            chars[col as usize] = ch;
        }
    }
    let row_str: String = chars.iter().collect();
    let bytes = row_str.as_bytes();
    let len = bytes.len();

    let mut i = 0;
    while i < len {
        if bytes[i] != b'h' {
            i += 1;
            continue;
        }
        let rest = &row_str[i..];
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let start = i;
            let mut end = i;
            for ch in row_str[i..].chars() {
                if ch.is_whitespace() || ch == '\0' {
                    break;
                }
                end += ch.len_utf8();
            }
            if end > start {
                while end > start {
                    let last = bytes[end - 1];
                    if last == b')' || last == b']' || last == b'>'
                        || last == b',' || last == b'.' || last == b';'
                    {
                        end -= 1;
                    } else {
                        break;
                    }
                }
                if (start as u16) <= target_col && target_col <= (end - 1) as u16 {
                    return Some(row_str[start..end].to_string());
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Explicit OSC 8 hyperlink at a cell, with the column run on this row that
/// shares the same link. Returns `(uri, start_col, end_col)` (inclusive cols).
///
/// Unlike [`url_at_cell`], which scrapes a literal `http(s)://` token out of
/// the rendered text, the URI here is an attribute alacritty parsed from the
/// OSC 8 escape, so it works when the displayed label differs from the target
/// (e.g. `\e]8;;https://example.com\e\\click here\e]8;;\e\\`). The run is
/// grouped by alacritty's hyperlink id (which ties the cells of one logical
/// link together) plus the uri.
pub(crate) fn osc8_link_at_cell(
    term: &alacritty_terminal::Term<crate::backend::EventProxy>,
    target_line: i32,
    target_col: u16,
) -> Option<(String, u16, u16)> {
    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    let line = Line(target_line);
    if line < grid.topmost_line() || line > grid.bottommost_line() {
        return None;
    }
    let row = &grid[line];
    let ncols = grid.columns();
    let col = target_col as usize;
    if col >= ncols {
        return None;
    }
    let link = row[Column(col)].hyperlink()?;
    let uri = link.uri().to_string();
    let id = link.id().to_string();
    let same = |c: usize| -> bool {
        row[Column(c)]
            .hyperlink()
            .is_some_and(|h| h.id() == id && h.uri() == uri)
    };
    let mut start = col;
    while start > 0 && same(start - 1) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < ncols && same(end + 1) {
        end += 1;
    }
    Some((uri, start as u16, end as u16))
}

/// The full run of an OSC 8 hyperlink at a cell, following a wrapped link
/// across grid rows. Returns `(uri, segments)` where each segment is
/// `(grid_line, start_col, end_col)` (inclusive cols), ordered top to bottom.
///
/// Unlike [`osc8_link_at_cell`] (which clamps to the hovered row and drives
/// the open / hint paths), this powers the hover underline, which must
/// cover every row a long link wraps onto. The walk only crosses a row
/// boundary on a genuine wrap: the current row's run must be flush against
/// the far edge AND the adjacent row's near edge must carry the same
/// hyperlink `id + uri`. This never merges two same-`id` but disjoint
/// regions (an explicit `id=` can repeat), only a contiguous wrap. Capped at
/// `MAX_ROWS` so a pathologically long link can't walk the whole scrollback
/// on the draw hot path (it keeps a partial underline past the cap).
/// One row's slice of a hyperlink run: `(grid_line, start_col, end_col)`.
pub(crate) type Osc8Segment = (i32, u16, u16);

pub(crate) fn osc8_link_run(
    term: &alacritty_terminal::Term<crate::backend::EventProxy>,
    target_line: i32,
    target_col: u16,
) -> Option<(String, Vec<Osc8Segment>)> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    let topmost = grid.topmost_line().0;
    let bottommost = grid.bottommost_line().0;
    let ncols = grid.columns();
    if target_line < topmost || target_line > bottommost || ncols == 0 {
        return None;
    }
    let col = target_col as usize;
    if col >= ncols {
        return None;
    }
    let anchor = grid[Line(target_line)][Column(col)].hyperlink()?;
    let uri = anchor.uri().to_string();
    let id = anchor.id().to_string();
    let same_on = |line: i32, c: usize| -> bool {
        grid[Line(line)][Column(c)]
            .hyperlink()
            .is_some_and(|h| h.id() == id && h.uri() == uri)
    };
    // The contiguous run on one row around a known-matching column.
    let seg = |line: i32, from: usize| -> Osc8Segment {
        let mut s = from;
        while s > 0 && same_on(line, s - 1) {
            s -= 1;
        }
        let mut e = from;
        while e + 1 < ncols && same_on(line, e + 1) {
            e += 1;
        }
        (line, s as u16, e as u16)
    };
    const MAX_ROWS: usize = 8;
    let last_col = ncols - 1;
    let mut segments = vec![seg(target_line, col)];
    // Walk up: cross only when the current top segment starts at col 0
    // (i.e. it wrapped from above) and the previous row's last cell is the
    // same link.
    let mut line = target_line;
    while segments.len() < MAX_ROWS
        && line > topmost
        && segments[0].1 == 0
        && same_on(line - 1, last_col)
    {
        line -= 1;
        segments.insert(0, seg(line, last_col));
    }
    // Walk down: cross only when the current bottom segment ends at the last
    // col (wraps onward) and the next row's first cell is the same link.
    let mut line = target_line;
    while segments.len() < MAX_ROWS
        && line < bottommost
        && segments[segments.len() - 1].2 as usize == last_col
        && same_on(line + 1, 0)
    {
        line += 1;
        segments.push(seg(line, 0));
    }
    Some((uri, segments))
}

/// The scheme of a URI, lowercased, iff the URI begins with a well-formed
/// `scheme:` per RFC 3986 (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`).
///
/// Runs on attacker-controlled OSC 8 bytes, so it is strict: the run before
/// the first `:` must start with a letter and contain only scheme characters.
/// A leading space, a control char or a newline anywhere in that run
/// (`java\nscript:`, ` javascript:`) fails to parse, so it can never be
/// mistaken for an allowed scheme.
pub(crate) fn osc8_scheme(uri: &str) -> Option<String> {
    let (scheme, _rest) = uri.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    chars
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .then(|| scheme.to_ascii_lowercase())
}

/// Whether an OSC 8 target may be followed (pointer cursor + Ctrl+click open).
///
/// OSC 8 URIs are attacker-controlled server output and, unlike the scraped
/// URLs (which only ever match `http(s)://`), can name any scheme, including
/// `javascript:`, `file:` and OS-handler schemes with side effects. Allow only
/// the safe, widely-understood web-facing schemes; everything else gets a
/// "link type not allowed" chip with no pointer, underline or open affordance,
/// so a hostile server cannot phish a click into an arbitrary handler.
///
/// `ssh://` is intentionally NOT allowed here yet: it should route to the
/// in-app quick-connect path (v1.0 `ssh://` handler follow-up), not open
/// blindly through the OS.
pub(crate) fn osc8_scheme_allowed(uri: &str) -> bool {
    matches!(
        osc8_scheme(uri).as_deref(),
        Some("http" | "https" | "mailto" | "ftp")
    )
}

/// Smart-select span for double-click: if the cell at grid-line `line`,
/// column `col` falls inside a detected URL / IP / path token, return its
/// `(start_col, end_col)` (inclusive). Returns `None` otherwise (caller
/// falls back to delimiter-word selection). Numbers are excluded, they are
/// too granular to be a useful "word" target. Reads the grid directly by
/// line so it stays correct when scrolled into history (unlike
/// `url_at_cell`, which indexes by on-screen row number and so only
/// matches the live screen).
pub(crate) fn smart_span_at(
    term: &alacritty_terminal::Term<crate::backend::EventProxy>,
    palette: &TerminalPalette,
    line: i32,
    col: u16,
) -> Option<(u16, u16)> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    let l = Line(line);
    if l < grid.topmost_line() || l > grid.bottommost_line() {
        return None;
    }
    let row = &grid[l];
    let ncols = grid.columns();
    let mut present = vec![false; ncols];
    let mut cols: Vec<(u16, char)> = Vec::new();
    for ci in 0..ncols {
        let c = row[Column(ci)].c;
        if c != ' ' && c != '\0' {
            present[ci] = true;
            cols.push((ci as u16, c));
        }
    }
    if cols.is_empty() || !present.get(col as usize).copied().unwrap_or(false) {
        return None;
    }
    // Expand to the whitespace-bounded token containing the click.
    let mut left = col;
    while left > 0 && present[left as usize - 1] {
        left -= 1;
    }
    let mut right = col;
    while (right as usize + 1) < ncols && present[right as usize + 1] {
        right += 1;
    }
    // Trigger only when that token overlaps a detected URL / IP / path
    // highlight, so plain prose words still fall through to delimiter-word
    // selection. The highlighter's own URL span may be shorter than the
    // token (its matcher is loose), hence the overlap test rather than a
    // containment test. `detect_highlights` takes (row, cells) pairs; a
    // single synthetic row 0 is enough as long as we match on the same key.
    let rows = [(0u16, cols)];
    let hit = detect_highlights(&rows, palette, false, &[]).into_iter().any(|h| {
        h.row == 0
            && h.kind != HighlightKind::Number
            && h.start_col <= right
            && h.end_col >= left
    });
    hit.then_some((left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_from(s: &str) -> Vec<(u16, Vec<(u16, char)>)> {
        vec![(
            0,
            s.chars()
                .enumerate()
                .filter(|(_, c)| *c != ' ')
                .map(|(i, c)| (i as u16, c))
                .collect(),
        )]
    }

    /// `(start, end)` byte spans of the UserDir highlights detected in `s`.
    fn user_dir_spans(s: &str, privacy: bool) -> Vec<(usize, usize)> {
        let rows = rows_from(s);
        detect_highlights(&rows, &TerminalPalette::default(), privacy, &[])
            .into_iter()
            .filter(|h| h.kind == HighlightKind::UserDir)
            .map(|h| (h.start_col as usize, h.end_col as usize))
            .collect()
    }

    fn masked_text(s: &str, spans: &[(usize, usize)]) -> Vec<String> {
        spans.iter().map(|&(a, b)| s[a..=b].to_string()).collect()
    }

    #[test]
    fn windows_prompt_masks_username_only() {
        let s = r"PS C:\Users\koobs> winget upgrade";
        let spans = user_dir_spans(s, true);
        assert_eq!(masked_text(s, &spans), vec!["koobs"]);
    }

    #[test]
    fn windows_deep_path_masks_username_only() {
        let s = r"C:\Users\koobs\AppData\Local\Oryxis";
        let spans = user_dir_spans(s, true);
        assert_eq!(masked_text(s, &spans), vec!["koobs"]);
    }

    #[test]
    fn windows_marker_is_case_insensitive() {
        let s = r"c:\users\bob>";
        let spans = user_dir_spans(s, true);
        assert_eq!(masked_text(s, &spans), vec!["bob"]);
    }

    #[test]
    fn linux_home_masks_username_only() {
        let s = "drwxr-xr-x /home/wilson/dev";
        let spans = user_dir_spans(s, true);
        assert_eq!(masked_text(s, &spans), vec!["wilson"]);
    }

    #[test]
    fn macos_users_masks_username_only() {
        let s = "/Users/wilson/Library/Logs";
        let spans = user_dir_spans(s, true);
        assert_eq!(masked_text(s, &spans), vec!["wilson"]);
    }

    #[test]
    fn user_dir_requires_privacy_mode() {
        let s = r"PS C:\Users\koobs>";
        assert!(user_dir_spans(s, false).is_empty());
    }

    #[test]
    fn url_paths_are_not_user_dirs() {
        let s = "GET https://cdn.example.com/users/42/avatar.png";
        assert!(user_dir_spans(s, true).is_empty());
    }

    #[test]
    fn user_dir_cells_are_privacy_cells() {
        let s = r"PS C:\Users\koobs> ";
        let rows = rows_from(s);
        let hs = detect_highlights(&rows, &TerminalPalette::default(), true, &[]);
        let name_start = s.find("koobs").unwrap() as u16;
        // Every cell of the name is masked; the separators around it are not.
        for col in name_start..name_start + 5 {
            assert!(is_privacy_cell(&hs, 0, col), "col {col} should be masked");
        }
        assert!(!is_privacy_cell(&hs, 0, name_start - 1));
        assert!(!is_privacy_cell(&hs, 0, name_start + 5));
        // Hover-reveal resolves the same span.
        assert_eq!(
            privacy_span_at(&hs, 0, name_start),
            Some((0, name_start, name_start + 4))
        );
    }

    #[test]
    fn user_dir_is_never_a_syntax_color() {
        let s = r"cd C:\Users\koobs";
        let rows = rows_from(s);
        let hs = detect_highlights(&rows, &TerminalPalette::default(), true, &[]);
        let name_start = s.find("koobs").unwrap() as u16;
        // The Windows path isn't a Unix-path highlight and UserDir itself
        // must not tint cells, so the name carries no keyword color.
        assert_eq!(highlight_color_at(&hs, 0, name_start), None);
    }

    /// `(start, end)` spans of Ip-kind highlights detected in `s`.
    fn ip_spans(s: &str) -> Vec<(usize, usize)> {
        let rows = rows_from(s);
        detect_highlights(&rows, &TerminalPalette::default(), false, &[])
            .into_iter()
            .filter(|h| h.kind == HighlightKind::Ip)
            .map(|h| (h.start_col as usize, h.end_col as usize))
            .collect()
    }

    fn ip_texts(s: &str) -> Vec<String> {
        ip_spans(s).into_iter().map(|(a, b)| s[a..=b].to_string()).collect()
    }

    #[test]
    fn ipv6_full_form_detected() {
        assert_eq!(
            ip_texts("addr 2001:0db8:85a3:0000:0000:8a2e:0370:7334 up"),
            vec!["2001:0db8:85a3:0000:0000:8a2e:0370:7334"]
        );
    }

    #[test]
    fn ipv6_compressed_forms_detected() {
        assert_eq!(ip_texts("ping ::1 ok"), vec!["::1"]);
        assert_eq!(ip_texts("via 2001:db8::1 dev"), vec!["2001:db8::1"]);
        assert_eq!(ip_texts("prefix 2001:db8:: len"), vec!["2001:db8::"]);
        assert_eq!(ip_texts("inet6 fe80::215:5dff:fe10:a3b1 scope"),
            vec!["fe80::215:5dff:fe10:a3b1"]);
    }

    #[test]
    fn ipv6_bracketed_leaves_port_visible() {
        // `[::1]:22`: the address is detected; the `:22` after the bracket
        // is a lone-colon run and stays visible.
        assert_eq!(ip_texts("connect [2001:db8::1]:8080"), vec!["2001:db8::1"]);
    }

    #[test]
    fn ipv6_trailing_prose_colon_is_trimmed() {
        assert_eq!(ip_texts("gateway 2001:db8::1: unreachable"), vec!["2001:db8::1"]);
    }

    #[test]
    fn timestamps_and_macs_are_not_ipv6() {
        assert!(ip_texts("12:34:56 log line").is_empty());
        assert!(ip_texts("mac aa:bb:cc:dd:ee:ff up").is_empty());
    }

    #[test]
    fn rust_paths_are_not_ipv6() {
        assert!(ip_texts("use std::io and Vec::new()").is_empty());
        assert!(ip_texts("err at core::fmt::Debug").is_empty());
    }

    #[test]
    fn ipv6_with_embedded_ipv4_is_fully_covered() {
        // Two side-by-side spans (hex part + dotted-quad tail from the
        // IPv4 pass); together they must cover the whole address.
        let s = "nat ::ffff:192.0.2.1 ok";
        let spans = ip_spans(s);
        let addr_start = s.find("::ffff").unwrap();
        let addr_end = s.find(" ok").unwrap() - 1;
        for col in addr_start..=addr_end {
            assert!(
                spans.iter().any(|&(a, b)| col >= a && col <= b),
                "col {col} ({}) uncovered", &s[col..=col]
            );
        }
    }

    #[test]
    fn looks_like_ipv6_validator_edges() {
        assert!(looks_like_ipv6("::1"));
        assert!(looks_like_ipv6("2001:db8::"));
        assert!(looks_like_ipv6("1:2:3:4:5:6:7:8"));
        assert!(!looks_like_ipv6("12:34:56"));
        assert!(!looks_like_ipv6("1:2:3:4:5:6:7:8:9"));
        assert!(!looks_like_ipv6("12345::1"));
        assert!(!looks_like_ipv6("1::2::3"));
        assert!(!looks_like_ipv6(":::"));
        assert!(!looks_like_ipv6(":1:2:3"));
        assert!(!looks_like_ipv6("1:2:3:"));
        assert!(!looks_like_ipv6("::")); // path separator, needs a group
        assert!(!looks_like_ipv6("g::1")); // non-hex
    }

    /// `(start, end)` spans of KnownHost-kind highlights.
    fn term_spans(s: &str, terms: &[&str]) -> Vec<String> {
        let rows = rows_from(s);
        let terms: Vec<String> = terms.iter().map(|t| t.to_string()).collect();
        detect_highlights(&rows, &TerminalPalette::default(), true, &terms)
            .into_iter()
            .filter(|h| h.kind == HighlightKind::KnownHost)
            .map(|h| s[h.start_col as usize..=h.end_col as usize].to_string())
            .collect()
    }

    #[test]
    fn known_host_terms_masked_case_insensitively() {
        assert_eq!(
            term_spans("ping WEB01.prod.internal ok", &["web01.prod.internal"]),
            vec!["WEB01.prod.internal"]
        );
    }

    #[test]
    fn known_host_terms_are_token_bounded() {
        // "web01" inside "web01-backup" is a different token; only the
        // standalone occurrence matches.
        assert_eq!(
            term_spans("host web01 and web01-backup", &["web01"]),
            vec!["web01"]
        );
    }

    #[test]
    fn known_host_terms_require_privacy_mode() {
        let rows = rows_from("ping web01 ok");
        let terms = vec!["web01".to_string()];
        let hs = detect_highlights(&rows, &TerminalPalette::default(), false, &terms);
        assert!(hs.iter().all(|h| h.kind != HighlightKind::KnownHost));
    }

    /// Texts of VersionQuad-kind highlights detected in `s`.
    fn version_quad_texts(s: &str, terms: &[&str]) -> Vec<String> {
        let rows = rows_from(s);
        let terms: Vec<String> = terms.iter().map(|t| t.to_string()).collect();
        detect_highlights(&rows, &TerminalPalette::default(), true, &terms)
            .into_iter()
            .filter(|h| h.kind == HighlightKind::VersionQuad)
            .map(|h| s[h.start_col as usize..=h.end_col as usize].to_string())
            .collect()
    }

    /// Ip-kind texts with privacy on and vault terms, the maskable set.
    fn masked_ip_texts(s: &str, terms: &[&str]) -> Vec<String> {
        let rows = rows_from(s);
        let terms: Vec<String> = terms.iter().map(|t| t.to_string()).collect();
        detect_highlights(&rows, &TerminalPalette::default(), true, &terms)
            .into_iter()
            .filter(|h| h.kind == HighlightKind::Ip)
            .map(|h| s[h.start_col as usize..=h.end_col as usize].to_string())
            .collect()
    }

    #[test]
    fn unmarked_quad_table_masks_in_privacy() {
        // Issue #53 narrowed (per-candidate scoping): a bare four-octet
        // all-<=255 quad with NO marker glued to it is byte-for-byte an
        // IP, so Privacy Mode masks it. A winget version table and an
        // `ip route` address list are the same shape; the safe error is
        // to mask. Locally-marked versions stay VersionQuad (next test).
        // The 3-part `3.13.0` is not a quad candidate, so it never masks.
        let s = "Python 3  Python.3  3.9.0.2  3.13.0  winget";
        assert_eq!(masked_ip_texts(s, &[]), vec!["3.9.0.2"]);
        assert!(version_quad_texts(s, &[]).is_empty());

        let s2 = "Visual Studio Code  1.96.0.0  1.96.0.1";
        assert_eq!(masked_ip_texts(s2, &[]), vec!["1.96.0.0", "1.96.0.1"]);
    }

    #[test]
    fn version_keyword_and_prefix_rows_not_masked() {
        // A version-word or product-slash glued to the token keeps it
        // readable (per-candidate evidence).
        assert!(masked_ip_texts("pandoc version 3.9.0.2 installed", &[]).is_empty());
        assert!(masked_ip_texts("agent curl/8.4.0.1 sent", &[]).is_empty());
        assert!(masked_ip_texts("ver 1.2.3.4", &[]).is_empty());
        assert_eq!(version_quad_texts("ver 1.2.3.4", &[]), vec!["1.2.3.4"]);
        // A row-wide keyword is NOT local evidence: `available` sits away
        // from the quad, and unmasking on it would leak a sibling public
        // IP on an access-log line, so the quad masks.
        assert_eq!(masked_ip_texts("rustc 1.96.0.0 available", &[]), vec!["1.96.0.0"]);
    }

    #[test]
    fn sibling_ip_not_unmasked_by_a_row_version() {
        // A genuine version token on the line must not unmask an
        // unrelated public IP sharing it (the per-candidate leak class).
        assert_eq!(
            masked_ip_texts("app 5.6.7 listening on 8.8.8.8", &[]),
            vec!["8.8.8.8"]
        );
        assert_eq!(
            masked_ip_texts("default via 203.0.113.1 dev eth0 src 203.0.113.55", &[]),
            vec!["203.0.113.1", "203.0.113.55"]
        );
    }

    #[test]
    fn oversized_octet_never_matched() {
        // `2365` fails the 3-digit/255 caps: no Ip and no VersionQuad
        // span, the current escape stays locked in.
        let s = "Microsoft Edge 122.0.2365.106 here";
        assert!(masked_ip_texts(s, &[]).is_empty());
        assert!(version_quad_texts(s, &[]).is_empty());
    }

    #[test]
    fn real_ips_still_masked() {
        assert_eq!(masked_ip_texts("ping 8.8.8.8", &[]), vec!["8.8.8.8"]);
        assert_eq!(masked_ip_texts("ssh 203.0.113.7", &[]), vec!["203.0.113.7"]);
        // Private/loopback ranges override version context.
        assert_eq!(
            masked_ip_texts("update available at 192.168.1.10", &[]),
            vec!["192.168.1.10"]
        );
        assert_eq!(masked_ip_texts("upgrade via 10.0.0.1 now", &[]), vec!["10.0.0.1"]);
    }

    #[test]
    fn repeated_address_is_not_a_version_table() {
        // `PING 8.8.8.8 (8.8.8.8)` carries two quad-dots but only one
        // DISTINCT value: an echoed endpoint, not an installed/available
        // pair, so it must stay masked (found by harness QA).
        assert_eq!(
            masked_ip_texts("PING 8.8.8.8 (8.8.8.8) 56(84) bytes of data.", &[]),
            vec!["8.8.8.8", "8.8.8.8"]
        );
    }

    #[test]
    fn vault_hostname_quad_dot_always_masked() {
        // A saved connection address wins over any version context.
        assert_eq!(
            masked_ip_texts("installed 3.9.0.2 available", &["3.9.0.2"]),
            vec!["3.9.0.2"]
        );
        assert!(version_quad_texts("installed 3.9.0.2 available", &["3.9.0.2"]).is_empty());
    }

    #[test]
    fn version_quads_keep_the_keyword_color_and_skip_privacy() {
        let s = "version 3.9.0.2 ok";
        let rows = rows_from(s);
        let hs = detect_highlights(&rows, &TerminalPalette::default(), true, &[]);
        let start = s.find("3.9.0.2").unwrap() as u16;
        // Colored like an Ip span...
        assert_eq!(
            highlight_color_at(&hs, 0, start),
            Some(TerminalPalette::default().ansi[5])
        );
        // ...but never a privacy cell.
        for col in start..start + 7 {
            assert!(!is_privacy_cell(&hs, 0, col), "col {col} must not mask");
        }
        assert_eq!(privacy_span_at(&hs, 0, start), None);
    }

    #[test]
    fn version_classification_is_privacy_flag_independent() {
        // With Privacy Mode off the same span exists with the same color,
        // so toggling privacy changes masking only, never coloring.
        let s = "version 3.9.0.2 ok";
        let rows = rows_from(s);
        let start = s.find("3.9.0.2").unwrap() as u16;
        let on = detect_highlights(&rows, &TerminalPalette::default(), true, &[]);
        let off = detect_highlights(&rows, &TerminalPalette::default(), false, &[]);
        assert_eq!(
            highlight_color_at(&on, 0, start),
            highlight_color_at(&off, 0, start)
        );
    }

    #[test]
    fn private_or_loopback_validator_edges() {
        assert!(ipv4_is_private_or_loopback("10.1.2.3"));
        assert!(ipv4_is_private_or_loopback("127.0.0.1"));
        assert!(ipv4_is_private_or_loopback("169.254.0.5"));
        assert!(ipv4_is_private_or_loopback("172.16.0.1"));
        assert!(ipv4_is_private_or_loopback("172.31.255.1"));
        assert!(ipv4_is_private_or_loopback("192.168.0.4"));
        assert!(!ipv4_is_private_or_loopback("172.32.0.1"));
        assert!(!ipv4_is_private_or_loopback("8.8.8.8"));
        assert!(!ipv4_is_private_or_loopback("169.253.0.1"));
        assert!(!ipv4_is_private_or_loopback("not.an.ip.at.all"));
    }

    // ── OSC 8 hyperlinks (C3) ──

    #[test]
    fn osc8_scheme_parses_only_well_formed_schemes() {
        assert_eq!(osc8_scheme("https://a.com").as_deref(), Some("https"));
        // Case-folded.
        assert_eq!(osc8_scheme("HTTPS://a.com").as_deref(), Some("https"));
        assert_eq!(osc8_scheme("mailto:x@y").as_deref(), Some("mailto"));
        // Scheme chars per RFC 3986 (`+`, `-`, `.`) are allowed in the run.
        assert_eq!(osc8_scheme("view-source:http://a").as_deref(), Some("view-source"));
        // A leading space, a control char or a digit-first run is not a scheme.
        assert_eq!(osc8_scheme(" javascript:alert(1)"), None);
        assert_eq!(osc8_scheme("java\nscript:alert(1)"), None);
        assert_eq!(osc8_scheme("1http://a"), None);
        // No colon at all.
        assert_eq!(osc8_scheme("example.com/path"), None);
    }

    #[test]
    fn osc8_scheme_allowlist_blocks_dangerous_handlers() {
        for ok in ["http://a", "https://a", "mailto:a@b", "ftp://a/f"] {
            assert!(osc8_scheme_allowed(ok), "{ok} should be allowed");
        }
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "vscode://x",
            "ssh://host",          // deliberately not allowed yet (quick-connect follow-up)
            " https://spoof",      // leading space defeats the scheme parse
            "data:text/html,<x>",
        ] {
            assert!(!osc8_scheme_allowed(bad), "{bad} should be blocked");
        }
    }

    /// Build a term, feed OSC 8 escapes, and return its grid for the run
    /// queries. `TerminalBackend` owns the alacritty `Term` the widget reads.
    fn osc8_term(bytes: &[u8]) -> crate::backend::TerminalBackend {
        let mut backend = crate::backend::TerminalBackend::new(80, 4);
        backend.process(bytes);
        backend
    }

    #[test]
    fn osc8_run_covers_the_whole_label() {
        // `\e]8;;URI\e\\LABEL\e]8;;\e\\` — the label is 10 cells ("click here").
        let b = osc8_term(b"\x1b]8;;https://example.com\x1b\\click here\x1b]8;;\x1b\\");
        let hit = osc8_link_at_cell(&b.term, 0, 3).expect("cell inside the label is a link");
        assert_eq!(hit, ("https://example.com".to_string(), 0, 9));
        // A cell past the label carries no link.
        assert!(osc8_link_at_cell(&b.term, 0, 20).is_none());
    }

    #[test]
    fn osc8_adjacent_distinct_links_do_not_merge() {
        // Two back-to-back links with different ids AND uris must stay
        // separate runs, never a single run spanning both.
        let b = osc8_term(
            b"\x1b]8;id=1;https://a.com\x1b\\AAA\x1b]8;;\x1b\\\x1b]8;id=2;https://b.com\x1b\\BBB\x1b]8;;\x1b\\",
        );
        let first = osc8_link_at_cell(&b.term, 0, 1).expect("first link");
        assert_eq!(first, ("https://a.com".to_string(), 0, 2));
        let second = osc8_link_at_cell(&b.term, 0, 4).expect("second link");
        assert_eq!(second, ("https://b.com".to_string(), 3, 5));
    }

    #[test]
    fn osc8_link_at_cell_ignores_plain_text() {
        // Bare text (no OSC 8) has no hyperlink attribute, even when it looks
        // like a URL, that path is the scraped `url_at_cell`, not this one.
        let b = osc8_term(b"visit https://plain.example.com now");
        assert!(osc8_link_at_cell(&b.term, 0, 10).is_none());
    }

    /// Build a narrow term so a long label wraps across rows.
    fn osc8_narrow_term(bytes: &[u8]) -> crate::backend::TerminalBackend {
        let mut backend = crate::backend::TerminalBackend::new(10, 4);
        backend.process(bytes);
        backend
    }

    #[test]
    fn osc8_run_follows_a_wrapped_link_across_rows() {
        // A 13-char label on a 10-col grid wraps: row 0 cols 0..9, row 1
        // cols 0..2. alacritty carries the same hyperlink across the wrap.
        let b = osc8_narrow_term(b"\x1b]8;;https://example.com\x1b\\ABCDEFGHIJKLM\x1b]8;;\x1b\\");
        let (uri, segs) = osc8_link_run(&b.term, 0, 5).expect("run from the top row");
        assert_eq!(uri, "https://example.com");
        assert_eq!(segs, vec![(0, 0, 9), (1, 0, 2)]);
        // Hovering the tail row resolves the identical full run.
        let (_, from_tail) = osc8_link_run(&b.term, 1, 1).expect("run from the tail row");
        assert_eq!(from_tail, vec![(0, 0, 9), (1, 0, 2)]);
    }

    #[test]
    fn osc8_run_does_not_merge_stacked_distinct_links() {
        // Link A exactly fills row 0 (10 chars, flush to the edge); link B
        // starts row 1. Different ids, so the walk must NOT treat B as A's
        // wrap even though A is flush-right and B is flush-left.
        let b = osc8_narrow_term(
            b"\x1b]8;id=1;https://a.com\x1b\\AAAAAAAAAA\x1b]8;;\x1b\\\x1b]8;id=2;https://b.com\x1b\\BBB\x1b]8;;\x1b\\",
        );
        let (uri_a, segs_a) = osc8_link_run(&b.term, 0, 4).expect("link A");
        assert_eq!(uri_a, "https://a.com");
        assert_eq!(segs_a, vec![(0, 0, 9)]);
        let (uri_b, segs_b) = osc8_link_run(&b.term, 1, 1).expect("link B");
        assert_eq!(uri_b, "https://b.com");
        assert_eq!(segs_b, vec![(1, 0, 2)]);
    }
}
