//! The shell-integration key and the snippet that carries it.
//!
//! Command history's in-band path reads `OSC 633 ; E`, the sequence where
//! the shell states the command line it parsed. That text lands in the
//! per-host history, where a row is one click from running again, so
//! anything able to write to the terminal could otherwise put words in the
//! user's mouth: a `cat` of a crafted file, a log line replayed on
//! connect, a compromised host.
//!
//! Nothing in the byte stream distinguishes "the shell reported this" from
//! "something printed this", so the snippet echoes a key only the app and
//! the user's own dotfile know, and the sniffer refuses every `E` without
//! it ([`oryxis_terminal::osc::set_global_command_nonce`], fail-closed).
//!
//! The key is per vault, not per host: it lives in one snippet the user
//! copies onto as many hosts as they like, and rotating it is one button
//! that invalidates every copy at once.

/// The snippet, with `__ORYXIS_NONCE__` where the key goes. Kept as a file
/// rather than a string literal so `docs/TMUX.md` can quote the same bytes,
/// which `snippet_matches_the_documented_one` then pins.
const SNIPPET_TEMPLATE: &str = include_str!("../../../resources/shell-integration.sh");

/// Placeholder the template carries and [`snippet`] fills in.
const PLACEHOLDER: &str = "__ORYXIS_NONCE__";

/// Vault setting holding the key.
pub(crate) const SETTING: &str = "shell_integration_nonce";

/// A fresh key: 128 bits, hex. Long enough that guessing it from output the
/// attacker cannot see is hopeless, short enough to read back over a phone
/// call when someone is debugging their dotfile.
pub(crate) fn generate_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The snippet the user installs on a host, carrying `nonce`.
pub(crate) fn snippet(nonce: &str) -> String {
    SNIPPET_TEMPLATE.replace(PLACEHOLDER, nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_hex_and_do_not_repeat() {
        let a = generate_nonce();
        let b = generate_nonce();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        // A key carrying `;` would end the OSC argument early and silently
        // break every capture, so the alphabet is not a cosmetic choice.
        assert!(!a.contains(';'));
    }

    #[test]
    fn the_snippet_carries_the_key_and_keeps_no_placeholder() {
        let s = snippet("deadbeef");
        assert!(s.contains("__oryxis_key=deadbeef"));
        assert!(!s.contains(PLACEHOLDER));
        // The reported line has to end with the key, which is the field the
        // sniffer compares; a snippet that emits the old 2-field form would
        // be refused by every pane.
        assert!(s.contains(r#"__oryxis_osc "633;E;$(__oryxis_esc "$1");$__oryxis_key""#));
    }

    /// `docs/TMUX.md` shows the snippet inline, because a user reading it on
    /// GitHub should not have to open a second file to see what they are
    /// pasting into their shell. Two copies drift, so this pins them: the
    /// documented block must be the template, byte for byte.
    #[test]
    fn snippet_matches_the_documented_one() {
        const DOC: &str = include_str!("../../../docs/TMUX.md");
        let block = DOC
            .split("```sh")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .expect("docs/TMUX.md must open with the snippet in an ```sh block");
        // Line endings are not the subject: both files are checked out CRLF
        // on Windows and LF elsewhere, and a test that failed on half the
        // machines would just get deleted.
        let normalize = |s: &str| s.replace("\r\n", "\n").trim().to_string();
        assert_eq!(
            normalize(block),
            normalize(SNIPPET_TEMPLATE),
            "docs/TMUX.md and resources/shell-integration.sh drifted apart"
        );
    }
}
