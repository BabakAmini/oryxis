//! UI helper widgets: privacy. Split out of widgets/mod.rs.

use super::*;
/// Mask a sensitive string for Privacy Mode: every non-whitespace char
/// becomes a muted block (`192.168.0.4` -> `███████████`, `deploy@web`
/// -> `██████████`). Separators are masked too, a visible `.` / `@` / `:`
/// would reveal the value's shape (octet count, username length). Used on
/// host cards and in session logs; the terminal does its own per-cell
/// masking against the same block glyph.
pub(crate) fn mask_blocks(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_whitespace() { c } else { '█' })
        .collect()
}

/// Redact IPv4/IPv6 addresses, `user@host` prompt tokens, home-directory
/// usernames (`C:\Users\<name>`, `/home/<name>`, `/Users/<name>`) and the
/// caller's known-sensitive `terms` (saved-connection hostnames, lowercase,
/// see `Oryxis::privacy_terms`) in arbitrary text for Privacy Mode,
/// replacing each match with muted blocks via [`mask_blocks`]. Used by the
/// session-log viewer (which renders recorded terminal output) so a
/// recording hides the same things the live terminal does. `user@host`
/// also catches emails and typed `ssh user@host` targets, which are
/// sensitive too. For home dirs only the name segment is masked, the
/// surrounding path stays readable. Returns the input unchanged when
/// nothing matches.
pub(crate) fn redact_for_display(s: &str, terms: &[String]) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Home dirs and `user@host` first so their tokens win over a bare
        // IP that might sit inside them. IPv4 is shape-only (octet range
        // isn't validated) and home dirs aren't URL-aware: display masking
        // is reversible via Reveal, so erring toward hiding is acceptable.
        // The IPv6 candidate is loose (any hex/colon run with 2+ colons,
        // plus an optional dotted-quad tail for `::ffff:192.0.2.1`) and is
        // validated in code below, regex alternation alone can't express
        // "has `::` or exactly 7 colons" without exploding.
        regex::Regex::new(
            r"(?i:[\\/](?:users|home)[\\/])(?P<hd>[A-Za-z0-9._-]+)|[A-Za-z0-9._-]+@[A-Za-z0-9._-]+|(?P<v6>[0-9A-Fa-f]{0,4}(?::[0-9A-Fa-f]{0,4}){2,}(?:\.\d{1,3}\.\d{1,3}\.\d{1,3})?)|\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b",
        )
        .expect("privacy display pattern")
    });
    let mut out = if re.is_match(s) {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        for caps in re.captures_iter(s) {
            let whole = caps.get(0).expect("regex match always has a group 0");
            if let Some(m) = caps.name("hd") {
                // Home-dir match: mask only the captured name.
                out.push_str(&s[last..m.start()]);
                out.push_str(&mask_blocks(m.as_str()));
            } else if let Some(m) = caps.name("v6") {
                // IPv6 candidate: trim prose colons, reject runs glued to
                // a word (std::io) or that fail the shared validator.
                let (mut a, mut b) = (m.start(), m.end());
                let core_end = s[a..b].find('.').map(|p| a + p).unwrap_or(b);
                if core_end - a >= 2 && bytes[a] == b':' && bytes[a + 1] != b':' {
                    a += 1;
                }
                let mut ce = core_end;
                if ce > a + 1 && bytes[ce - 1] == b':' && bytes[ce - 2] != b':' {
                    ce -= 1;
                }
                // A trimmed hex core with a dotted tail isn't an embedded
                // IPv4 form anymore; drop the tail from the mask.
                if ce != core_end {
                    b = ce;
                }
                let is_wordish = |x: u8| x.is_ascii_alphanumeric() || x == b'_' || x == b'.';
                let glued = (a > 0 && is_wordish(bytes[a - 1]))
                    || (b < s.len() && (bytes[b].is_ascii_alphanumeric() || bytes[b] == b'_'));
                if !glued && oryxis_terminal::looks_like_ipv6(&s[a..ce]) {
                    out.push_str(&s[last..a]);
                    out.push_str(&mask_blocks(&s[a..b]));
                    out.push_str(&s[b..whole.end()]);
                } else {
                    out.push_str(&s[last..whole.end()]);
                }
                last = whole.end();
                continue;
            } else {
                out.push_str(&s[last..whole.start()]);
                out.push_str(&mask_blocks(whole.as_str()));
            }
            last = whole.end();
        }
        out.push_str(&s[last..]);
        out
    } else {
        s.to_string()
    };
    if !terms.is_empty() {
        out = mask_terms(&out, terms);
    }
    out
}

/// Mask exact, case-insensitive, token-bounded occurrences of `terms`
/// (lowercase) in `s`. The literal-match counterpart of the terminal's
/// KnownHost spans: plain DNS names have no detectable shape (file
/// extensions collide with ccTLDs), so the known values are matched
/// exactly instead of guessed.
fn mask_terms(s: &str, terms: &[String]) -> String {
    let lower = s.to_ascii_lowercase();
    let bytes = s.as_bytes();
    let is_tok =
        |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
    // Collect bounded match ranges first; terms can overlap (one host a
    // substring of another) and ranges must merge, not double-mask.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        if term.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(pos) = lower[from..].find(term.as_str()) {
            let a = from + pos;
            let b = a + term.len();
            let bounded = (a == 0 || !is_tok(bytes[a - 1]))
                && (b >= bytes.len() || !is_tok(bytes[b]));
            if bounded {
                ranges.push((a, b));
            }
            from = b;
        }
    }
    if ranges.is_empty() {
        return s.to_string();
    }
    ranges.sort_unstable();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (a, b) in ranges {
        if a < last {
            continue; // swallowed by a previous (overlapping) range
        }
        out.push_str(&s[last..a]);
        out.push_str(&mask_blocks(&s[a..b]));
        last = b;
    }
    out.push_str(&s[last..]);
    out
}

/// Privacy Mode reveal toggle (the eye icon). Shows an open eye while
/// revealed (accent tint) and a struck-through eye while masked, with a
/// tooltip describing the action. Shared by the Logs view, the session-log
/// viewer header and the Known Hosts view so the reveal affordance is the
/// same everywhere. Drives `Message::TogglePrivacyReveal`.
pub(crate) fn privacy_reveal_btn<'a>(revealed: bool) -> Element<'a, Message> {
    let (glyph, tip_key) = if revealed {
        (iced_fonts::lucide::eye(), "privacy_hide")
    } else {
        (iced_fonts::lucide::eye_off(), "privacy_reveal")
    };
    let icon = glyph.size(13).color(if revealed {
        OryxisColors::t().accent
    } else {
        OryxisColors::t().text_secondary
    });
    let b = button(
        container(icon)
            .center(Length::Fixed(24.0))
            .height(Length::Fixed(24.0))
            .width(Length::Fixed(28.0)),
    )
    .on_press(Message::TogglePrivacyReveal)
    .style(move |_, status| {
        let bg = match status {
            BtnStatus::Hovered => Color::from_rgba(1.0, 1.0, 1.0, 0.08),
            BtnStatus::Pressed => Color::from_rgba(1.0, 1.0, 1.0, 0.12),
            _ if revealed => Color { a: 0.12, ..OryxisColors::t().accent },
            _ => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: Radius::from(6.0),
                color: OryxisColors::t().border,
                width: 1.0,
            },
            ..Default::default()
        }
    });
    iced::widget::tooltip(
        b,
        container(text(crate::i18n::t(tip_key)).size(11))
            .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_surface)),
                border: Border {
                    radius: Radius::from(6.0),
                    color: OryxisColors::t().border,
                    width: 1.0,
                },
                ..Default::default()
            }),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::{mask_blocks, redact_for_display};

    #[test]
    fn mask_blocks_covers_separators_too() {
        assert_eq!(mask_blocks("192.168.0.4"), "███████████");
        assert_eq!(mask_blocks("deploy@web"), "██████████");
        assert_eq!(mask_blocks("a b"), "█ █");
    }

    #[test]
    fn redacts_ip_and_user_host() {
        assert_eq!(
            redact_for_display("ssh deploy@web from 192.168.0.4", &[]),
            format!("ssh {} from {}", mask_blocks("deploy@web"), mask_blocks("192.168.0.4"))
        );
    }

    #[test]
    fn redacts_windows_home_dir_username_only() {
        assert_eq!(
            redact_for_display(r"PS C:\Users\koobs> winget upgrade", &[]),
            format!(r"PS C:\Users\{}> winget upgrade", mask_blocks("koobs"))
        );
    }

    #[test]
    fn redacts_unix_home_dir_username_only() {
        assert_eq!(
            redact_for_display("cd /home/wilson/dev", &[]),
            format!("cd /home/{}/dev", mask_blocks("wilson"))
        );
        assert_eq!(
            redact_for_display("/Users/wilson/Library", &[]),
            format!("/Users/{}/Library", mask_blocks("wilson"))
        );
    }

    #[test]
    fn home_dir_marker_is_case_insensitive() {
        assert_eq!(
            redact_for_display(r"c:\users\bob>", &[]),
            format!(r"c:\users\{}>", mask_blocks("bob"))
        );
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(
            redact_for_display("winget upgrade Name Id", &[]),
            "winget upgrade Name Id"
        );
    }

    #[test]
    fn redacts_ipv6_forms() {
        assert_eq!(
            redact_for_display("ping ::1 ok", &[]),
            format!("ping {} ok", mask_blocks("::1"))
        );
        assert_eq!(
            redact_for_display("via 2001:db8::1 dev eth0", &[]),
            format!("via {} dev eth0", mask_blocks("2001:db8::1"))
        );
        assert_eq!(
            redact_for_display("addr 2001:0db8:85a3:0000:0000:8a2e:0370:7334", &[]),
            format!("addr {}", mask_blocks("2001:0db8:85a3:0000:0000:8a2e:0370:7334"))
        );
    }

    #[test]
    fn ipv6_with_embedded_ipv4_is_fully_masked() {
        assert_eq!(
            redact_for_display("nat ::ffff:192.0.2.1 ok", &[]),
            format!("nat {} ok", mask_blocks("::ffff:192.0.2.1"))
        );
    }

    #[test]
    fn timestamps_and_rust_paths_are_not_ipv6() {
        assert_eq!(
            redact_for_display("12:34:56 build ok", &[]),
            "12:34:56 build ok"
        );
        assert_eq!(
            redact_for_display("error in std::io::Error", &[]),
            "error in std::io::Error"
        );
        assert_eq!(
            redact_for_display("mac aa:bb:cc:dd:ee:ff up", &[]),
            "mac aa:bb:cc:dd:ee:ff up"
        );
    }

    #[test]
    fn known_terms_masked_token_bounded_case_insensitive() {
        let terms = vec!["web01.prod.internal".to_string()];
        assert_eq!(
            redact_for_display("Connected to WEB01.prod.internal ok", &terms),
            format!("Connected to {} ok", mask_blocks("WEB01.prod.internal"))
        );
        // Inside a longer token: no match.
        assert_eq!(
            redact_for_display("web01.prod.internal-backup", &terms),
            "web01.prod.internal-backup"
        );
    }

    #[test]
    fn overlapping_terms_do_not_double_mask() {
        let terms = vec!["prod.internal".to_string(), "web01.prod.internal".to_string()];
        // Both terms match at overlapping positions; output masks the
        // region once and keeps total length.
        let out = redact_for_display("web01.prod.internal down", &terms);
        assert_eq!(out, format!("{} down", mask_blocks("web01.prod.internal")));
    }
}
