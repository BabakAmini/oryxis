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
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS RNG unavailable");
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The snippet the user installs on a host, carrying `nonce`.
///
/// Line endings are forced to LF regardless of how the template was
/// checked out. A Windows clone gets CRLF from git, and a `.sh` carrying
/// `\r` fails on the host in ways that read as nonsense (`bash` reports
/// `$'\r': command not found`, zsh mangles the `printf` escapes). The
/// destination is always a POSIX shell, so the format is not the
/// checkout's to decide.
pub(crate) fn snippet(nonce: &str) -> String {
    SNIPPET_TEMPLATE
        .replace("\r\n", "\n")
        .replace(PLACEHOLDER, nonce)
}

/// The template with the placeholder INTACT (LF-normalized), for the
/// install-script preset (issue #147): the preset stores the
/// placeholder and the injection path substitutes the vault's current
/// key at run time, so rotating the key can never leave a stale copy
/// frozen inside the vault.
pub(crate) fn template() -> String {
    SNIPPET_TEMPLATE.replace("\r\n", "\n")
}

/// Substitute the current key into `text` (a snippet body about to be
/// sent). A no-op for the overwhelming majority of snippets, which
/// never carry the placeholder.
pub(crate) fn fill_nonce(text: &str, nonce: &str) -> String {
    if text.contains(PLACEHOLDER) {
        text.replace(PLACEHOLDER, nonce)
    } else {
        text.to_string()
    }
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
        // It is pasted into a POSIX shell. A CRLF checkout (every Windows
        // clone) would otherwise hand the host a file whose every line
        // ends in `\r`, and bash answers that with `$'\r': command not
        // found` on a line that looks perfectly fine.
        assert!(!s.contains('\r'), "the copied snippet must be LF-only");
        // The reported line has to end with the key, which is the field the
        // sniffer compares; a snippet that emits the old 2-field form would
        // be refused by every pane.
        assert!(s.contains(r#"__oryxis_osc "633;E;$(__oryxis_esc "$1");$__oryxis_key""#));
    }

    #[test]
    fn fill_nonce_substitutes_only_when_the_placeholder_is_there() {
        assert_eq!(fill_nonce("key=__ORYXIS_NONCE__", "abc"), "key=abc");
        // The install preset embeds the whole template; every copy of
        // the placeholder must resolve, or the host gets a key that is
        // half real, half literal.
        let body = "a=__ORYXIS_NONCE__\nb=__ORYXIS_NONCE__";
        assert!(!fill_nonce(body, "abc").contains(PLACEHOLDER));
        // And an ordinary snippet passes through untouched.
        assert_eq!(fill_nonce("echo hi", "abc"), "echo hi");
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
