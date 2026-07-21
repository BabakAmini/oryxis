//! Pre-PTY terminfo probe. Old hosts predate newer `TERM` entries
//! (issue #88: CentOS 7's ncurses 5.9 has no `tmux-256color`, so vim
//! dies with `E437` and nano refuses to start, with no hint of why).
//! Before requesting the PTY with a custom `TERM`, ask the host's own
//! terminfo db whether it knows the name and fall back to the nearest
//! widely-shipped entry when it doesn't. The probe never fails the
//! connect: any transport or parse trouble keeps the requested name.

use super::*;

/// Default `TERM` sent when the connection has no per-host override.
/// The default is never probed: it is present on effectively every
/// terminfo shipped this century, and skipping it keeps the common
/// connect path at zero extra round trips.
pub(crate) const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";

/// Recorded on the session when the probe found the configured `TERM`
/// missing on the host. `used: Some(name)` means the PTY was requested
/// with that fallback instead; `None` means nothing in the fallback
/// chain existed either, so the requested name was kept (best effort).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermFallback {
    pub requested: String,
    pub used: Option<String>,
}

/// What the probe learned about the requested `TERM` on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TermProbe {
    /// The host knows the requested entry; nothing to do.
    Present,
    /// The requested entry is missing; this chain entry exists.
    Fallback(String),
    /// `infocmp` works but neither the requested entry nor any chain
    /// entry exists (empty or exotic terminfo db).
    MissingNoFallback,
    /// The probe could not decide (no `infocmp`, no `sh`, non-POSIX
    /// host, timeout, channel failure). Keep the requested name.
    Inconclusive,
}

/// Stdout marker for the probe. Distinctive so shell rc noise on the
/// exec channel (bash sources `.bashrc` for non-interactive ssh
/// commands) can never be mistaken for a result line.
const MARKER: &str = "ORYXIS-TI:";

/// Fallback candidates for a missing `TERM`, nearest capability first.
/// The screen entries stay ahead of xterm for the tmux/screen family
/// because their key encodings match what the multiplexer emits.
fn fallback_candidates(term: &str) -> &'static [&'static str] {
    if term.starts_with("tmux") {
        &["screen-256color", "screen", "xterm-256color", "xterm"]
    } else if term.starts_with("screen") {
        &["screen", "xterm-256color", "xterm"]
    } else if term.starts_with("vt") || term == "ansi" || term == "linux" {
        // The vt/ansi family targets real terminals and appliances;
        // falling "up" to xterm could emit sequences the device cannot
        // parse, so stay within the family.
        &["vt220", "vt100", "ansi"]
    } else {
        &["xterm-256color", "xterm"]
    }
}

/// Build the remote probe line, or `None` when the name is not a plain
/// terminfo identifier (a `Connection` can arrive via sync or import,
/// not just the picker, so nothing unvetted may ride the shell line).
///
/// The script runs under an explicit `sh -c` so POSIX syntax holds
/// regardless of the user's login shell (csh chokes on `2>&1`); hosts
/// without `sh` (Windows) print no marker, which parses as
/// [`TermProbe::Inconclusive`].
pub(crate) fn probe_command(requested: &str) -> Option<String> {
    let safe = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
    };
    if !safe(requested) {
        return None;
    }
    let mut names = vec![requested];
    names.extend(
        fallback_candidates(requested)
            .iter()
            .copied()
            .filter(|c| *c != requested),
    );
    Some(format!(
        "sh -c 'for t in {}; do infocmp \"$t\" >/dev/null 2>&1 && {{ echo \"{m}$t\"; exit 0; }}; done; \
         command -v infocmp >/dev/null 2>&1 && echo {m}!absent || echo {m}!noinfocmp'",
        names.join(" "),
        m = MARKER,
    ))
}

/// Interpret the probe stdout. Scans for the marker line instead of
/// trusting the stream to be clean (rc-file noise, motd fragments).
pub(crate) fn parse_probe_output(requested: &str, output: &str) -> TermProbe {
    for line in output.lines() {
        let Some(rest) = line.trim().strip_prefix(MARKER) else {
            continue;
        };
        return match rest {
            r if r == requested => TermProbe::Present,
            "!absent" => TermProbe::MissingNoFallback,
            "" | "!noinfocmp" => TermProbe::Inconclusive,
            other => TermProbe::Fallback(other.to_string()),
        };
    }
    TermProbe::Inconclusive
}

impl SshEngine {
    /// Run the terminfo probe on a side channel of `handle` (nothing
    /// reaches the user's PTY; same shape as `SshSession::probe`).
    pub(crate) async fn probe_terminfo(
        &self,
        handle: &client::Handle<ClientHandler>,
        requested: &str,
    ) -> TermProbe {
        let Some(cmd) = probe_command(requested) else {
            return TermProbe::Inconclusive;
        };
        let Ok(mut channel) = handle.channel_open_session().await else {
            return TermProbe::Inconclusive;
        };
        if channel.exec(true, cmd.as_str()).await.is_err() {
            return TermProbe::Inconclusive;
        }
        // The reply is one marker line; the cap only bounds hostile or
        // misconfigured hosts that stream rc noise at the channel.
        const PROBE_STDOUT_CAP: usize = 64 * 1024;
        // Bounded so a wedged host costs a fraction of the session
        // timeout, not all of it; expiry parses whatever arrived.
        const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
        let mut stdout = Vec::new();
        let collect = async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) if stdout.len() < PROBE_STDOUT_CAP => {
                        let room = PROBE_STDOUT_CAP - stdout.len();
                        stdout.extend_from_slice(&data[..data.len().min(room)]);
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::ExitStatus { .. }) | None => break,
                    _ => {}
                }
            }
        };
        let _ = tokio::time::timeout(PROBE_TIMEOUT, collect).await;
        parse_probe_output(requested, &String::from_utf8_lossy(&stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lists_requested_first_then_chain() {
        let cmd = probe_command("tmux-256color").unwrap();
        assert!(cmd.contains(
            "for t in tmux-256color screen-256color screen xterm-256color xterm;"
        ));
    }

    #[test]
    fn command_skips_duplicate_of_requested() {
        let cmd = probe_command("screen").unwrap();
        assert!(cmd.contains("for t in screen xterm-256color xterm;"));
    }

    #[test]
    fn command_refuses_non_terminfo_names() {
        for bad in ["", "tmux;rm -rf /", "$(reboot)", "a b", "x'y", "ab\"cd", "e\nf"] {
            assert!(probe_command(bad).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn command_accepts_all_picker_entries() {
        for term in [
            "xterm-256color", "xterm", "screen-256color", "tmux-256color",
            "screen", "linux", "vt220", "vt100", "ansi",
        ] {
            assert!(probe_command(term).is_some(), "refused {term:?}");
        }
    }

    #[test]
    fn vt_family_never_falls_up_to_xterm() {
        let cmd = probe_command("vt220").unwrap();
        assert!(!cmd.contains("xterm"));
        assert!(cmd.contains("for t in vt220 vt100 ansi;"));
    }

    #[test]
    fn parse_present() {
        assert_eq!(
            parse_probe_output("tmux-256color", "ORYXIS-TI:tmux-256color\n"),
            TermProbe::Present
        );
    }

    #[test]
    fn parse_fallback_ignores_rc_noise() {
        let out = "motd: welcome\nbash: warning\nORYXIS-TI:screen-256color\n";
        assert_eq!(
            parse_probe_output("tmux-256color", out),
            TermProbe::Fallback("screen-256color".into())
        );
    }

    #[test]
    fn parse_absent_and_noinfocmp() {
        assert_eq!(
            parse_probe_output("tmux-256color", "ORYXIS-TI:!absent\n"),
            TermProbe::MissingNoFallback
        );
        assert_eq!(
            parse_probe_output("tmux-256color", "ORYXIS-TI:!noinfocmp\n"),
            TermProbe::Inconclusive
        );
    }

    #[test]
    fn parse_empty_or_garbage_is_inconclusive() {
        assert_eq!(parse_probe_output("tmux-256color", ""), TermProbe::Inconclusive);
        assert_eq!(
            parse_probe_output("tmux-256color", "'sh' is not recognized\n"),
            TermProbe::Inconclusive
        );
        // A bare marker (hostile echo) must not read as a fallback name.
        assert_eq!(
            parse_probe_output("tmux-256color", "ORYXIS-TI:\n"),
            TermProbe::Inconclusive
        );
    }
}
