//! Remote command synthesis and parsing for the tmux manager (issue
//! #116).
//!
//! Everything is read by running tmux itself on an exec channel
//! multiplexed on the pane's live SSH session: nothing is installed on
//! the host, no rc file is written, nothing is injected into the shell.
//!
//! The parser is pure (`&str` -> `Vec<TmuxSession>`) so it unit-tests
//! against captured output without a network.

use super::model::TmuxSession;

/// Field separator inside a `list-sessions` line.
///
/// A colon, because tmux STRUCTURALLY cannot put one in a session name:
/// `:` and `.` are its target-spec delimiters, so `new-session -s a:b`
/// silently creates `a_b` (verified against tmux 3.4). Tab, space, pipe
/// and even control characters ARE accepted in a name, which rules all
/// of them out; and a control character cannot serve as the separator
/// anyway, because tmux escapes those on the way out (`\001` arrives as
/// the four literal characters, not the byte).
///
/// The other four fields are two counts, a timestamp and a group name,
/// and a group name follows the same rule, so no field can carry one
/// either.
const FIELD: &str = ":";

/// Marker printed when the host has no tmux at all, so "not installed"
/// is a distinct answer from "installed with zero sessions". Without
/// it both arrive as empty output and the tab cannot tell the user
/// which one happened.
const NO_TMUX: &str = "---ORYXIS-NO-TMUX---";

/// Marker printed when tmux is present but owns no server yet. tmux
/// exits non-zero with "no server running on ..." in that case, which
/// is not an error worth surfacing: it is the empty state.
const NO_SERVER: &str = "---ORYXIS-NO-SERVER---";

/// The listing command.
///
/// `command -v` is the POSIX way to ask whether a program exists;
/// `which` is not in POSIX and behaves differently across distros. The
/// whole thing is wrapped in `sh -c` for the same reason the monitor
/// probe is: the exec channel hands the command to the user's LOGIN
/// shell, and csh / fish would choke on the Bourne syntax.
///
/// The format string is double-quoted inside a single-quoted wrapper,
/// so the batch must contain no single quote of its own (asserted in
/// debug, exactly like `monitor::probe`).
pub(crate) fn list_sessions_command() -> String {
    let format = [
        "#{session_name}",
        "#{session_windows}",
        "#{session_attached}",
        "#{session_created}",
        "#{?session_grouped,#{session_group},}",
    ]
    .join(FIELD);
    let batch = format!(
        "command -v tmux >/dev/null 2>&1 || {{ echo {NO_TMUX}; exit 0; }}; \
         tmux list-sessions -F \"{format}\" 2>/dev/null || echo {NO_SERVER}"
    );
    debug_assert!(!batch.contains('\''));
    format!("sh -c '{batch}'")
}

/// What a listing probe answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Listing {
    /// tmux is not on the host's PATH.
    NoTmux,
    /// tmux is installed; the sessions it reported (possibly empty,
    /// which means the server is not running or owns nothing).
    Sessions(Vec<TmuxSession>),
}

/// Parse a listing payload. Unparseable lines are skipped rather than
/// failing the whole listing: a tmux old enough to not know one of the
/// format specifiers prints it verbatim, and losing one column is a
/// better answer than losing the tab.
pub(crate) fn parse_listing(payload: &str) -> Listing {
    if payload.lines().any(|l| l.trim() == NO_TMUX) {
        return Listing::NoTmux;
    }
    let mut sessions = Vec::new();
    for line in payload.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() || line.trim() == NO_SERVER {
            continue;
        }
        if let Some(session) = parse_line(line) {
            sessions.push(session);
        }
    }
    Listing::Sessions(sessions)
}

/// One `list-sessions` line. Split from the RIGHT: the four trailing
/// fields cannot contain the separator, so should a tmux old or odd
/// enough to allow one in a NAME ever turn up, the name keeps it
/// instead of every column shifting by one.
fn parse_line(line: &str) -> Option<TmuxSession> {
    let mut parts: Vec<&str> = line.rsplitn(5, FIELD).collect();
    if parts.len() < 5 {
        return None;
    }
    parts.reverse();
    let name = parts[0].to_string();
    if name.is_empty() {
        return None;
    }
    Some(TmuxSession {
        windows: parts[1].trim().parse().unwrap_or(0),
        // `session_attached` is a COUNT of attached clients, not a
        // flag: a session can carry several, and tmux prints 0 for a
        // detached one.
        attached: parts[2].trim().parse().unwrap_or(0),
        created: parts[3].trim().parse().ok(),
        group: (!parts[4].trim().is_empty()).then(|| parts[4].trim().to_string()),
        name,
    })
}

/// `tmux kill-session -t <name>`, with the name quoted for the remote
/// shell. Every name here is text the REMOTE HOST printed, so it is
/// never interpolated raw: `sh_quote` refuses a name carrying a line
/// break instead of letting it become a second command.
pub(crate) fn kill_session_command(name: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!("tmux kill-session -t {name}"))
}

/// `tmux new-session -d -s <name>`, detached so it never fights the
/// pane's own PTY: the user attaches afterwards with the same gesture
/// they would use on an existing session.
pub(crate) fn new_session_command(name: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!("tmux new-session -d -s {name}"))
}

/// The line typed into the pane to attach. Unlike the two above this
/// one reaches the user's own shell rather than an exec channel, so the
/// quoting matters just as much: the name came off the host.
///
/// `attach` first, `switch-client` as the fallback, and the ORDER is
/// load-bearing (issue #157). The pane is as likely to be INSIDE tmux
/// as outside it: the second attach in a pane is the whole point of a
/// session manager, and a bare `tmux attach` there answers "sessions
/// should be nested with care" and does nothing, so the chain tries
/// both. It used to run `switch-client` first, which is WRONG outside
/// tmux whenever any other client exists: with no current client tmux
/// resolves the target client to the most recently used one, so the
/// command exits 0 having MOVED SOMEONE ELSE'S CLIENT to the picked
/// session, and the `||` never attaches this pane. The "someone else"
/// is routinely the dead client a hard disconnect leaves behind (the
/// reconnect repro of #157), but a second live terminal gets hijacked
/// just the same. `attach` has no such fallback: outside tmux it
/// attaches (and flushes any dead client on the first write to its
/// tty), inside it refuses (suppressed) and hands over to
/// `switch-client`, which from inside tmux resolves the current client
/// correctly. Asking the shell to try both remains better than
/// tracking which state the pane is in, because the app is not the
/// only thing that changes it: the user can attach by hand, or detach
/// with the prefix key, and any remembered flag would be a guess by
/// then.
pub(crate) fn attach_command(name: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!(
        "tmux attach -t {name} 2>/dev/null || tmux switch-client -t {name}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a payload line the way the remote format string would.
    fn line(name: &str, windows: &str, attached: &str, created: &str, group: &str) -> String {
        [name, windows, attached, created, group].join(FIELD)
    }

    #[test]
    fn parses_a_normal_listing() {
        let payload = format!(
            "{}\n{}\n",
            line("work", "3", "1", "1754600000", ""),
            line("build", "1", "0", "1754600100", "")
        );
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "work");
        assert_eq!(sessions[0].windows, 3);
        assert_eq!(sessions[0].attached, 1);
        assert_eq!(sessions[0].created, Some(1_754_600_000));
        assert_eq!(sessions[1].name, "build");
        assert_eq!(sessions[1].attached, 0);
    }

    #[test]
    fn missing_tmux_is_its_own_answer() {
        assert_eq!(parse_listing("---ORYXIS-NO-TMUX---\n"), Listing::NoTmux);
        // A dead server is NOT the same answer: tmux is there, it just
        // owns nothing, so the tab offers to create a session instead
        // of telling the user to install something they already have.
        assert_eq!(
            parse_listing("---ORYXIS-NO-SERVER---\n"),
            Listing::Sessions(Vec::new())
        );
    }

    #[test]
    fn a_separator_in_the_name_keeps_the_columns_aligned() {
        // tmux rewrites `:` in a session name to `_`, so this line
        // cannot come off a healthy tmux. The right-to-left split means
        // that if one ever did, the name would absorb it instead of
        // every column shifting and the window count landing in the
        // name.
        let payload = line(&format!("we{FIELD}ird"), "2", "0", "1754600000", "");
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, format!("we{FIELD}ird"));
        assert_eq!(sessions[0].windows, 2);
    }

    #[test]
    fn the_separator_survives_tmux_verbatim() {
        // The bug this test exists for: a control character LOOKS like
        // an unforgeable separator, but tmux escapes control characters
        // on the way out, so `\u{1}` arrives as four literal characters
        // and the parser matches nothing, every line is skipped and the
        // tab shows an empty host. Whatever FIELD is, it must be
        // something tmux emits unchanged.
        assert!(
            !FIELD.chars().any(|c| c.is_control()),
            "tmux escapes control characters in -F output"
        );
        assert!(list_sessions_command().contains(FIELD));
    }

    #[test]
    fn grouped_sessions_carry_their_group() {
        let payload = line("mirror", "2", "1", "1754600000", "shared");
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions[0].group.as_deref(), Some("shared"));
    }

    #[test]
    fn an_unparseable_line_is_skipped_not_fatal() {
        let payload = format!(
            "a tmux too old to know the format\n{}\n",
            line("work", "1", "0", "1754600000", "")
        );
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "work");
    }

    #[test]
    fn unparseable_counts_fall_back_instead_of_dropping_the_session() {
        // An ancient tmux without `session_windows` echoes the specifier
        // verbatim. The session is still real and still killable, so it
        // renders with a zero count rather than vanishing.
        let payload = line("work", "#{session_windows}", "", "", "");
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].windows, 0);
        assert_eq!(sessions[0].created, None);
    }

    #[test]
    fn every_command_quotes_the_session_name() {
        // The name is text the remote host printed. Unquoted, this one
        // would run `rm -rf ~` on the way past.
        let evil = "foo; rm -rf ~";
        for built in [
            kill_session_command(evil).unwrap(),
            new_session_command(evil).unwrap(),
            attach_command(evil).unwrap(),
        ] {
            assert!(built.ends_with("'foo; rm -rf ~'"), "unquoted: {built}");
            // The name must appear ONLY inside single quotes. The attach
            // line names it twice (switch-client, then attach), so
            // checking the tail alone would miss an unquoted first use.
            assert!(
                !built.replace("'foo; rm -rf ~'", "").contains("rm -rf"),
                "an unquoted copy survived: {built}"
            );
        }
        // A quote of its own cannot break out either.
        assert_eq!(
            kill_session_command("it's").unwrap(),
            r#"tmux kill-session -t 'it'\''s'"#
        );
    }

    #[test]
    fn attach_covers_a_pane_already_inside_tmux() {
        // `tmux attach` from inside a session refuses ("sessions should
        // be nested with care") and does nothing, which is the second
        // attach in any pane, i.e. the whole point of the tab. The line
        // asks the shell to try the attach first and fall back to the
        // switch, which from inside tmux targets the current client.
        let built = attach_command("work").unwrap();
        assert_eq!(
            built,
            "tmux attach -t 'work' 2>/dev/null || tmux switch-client -t 'work'"
        );
    }

    #[test]
    fn attach_runs_before_switch_client() {
        // Issue #157. Outside tmux, `switch-client` does NOT fail when
        // some other client exists: tmux falls back to the most
        // recently used client, exits 0 having moved THAT client, and
        // the `||` never runs. The lingering dead client of a hard
        // disconnect makes this the state every reconnect lands in, so
        // the attach must come first (verified against tmux 3.4).
        let built = attach_command("work").unwrap();
        let attach = built.find("tmux attach").expect("attach present");
        let switch = built
            .find("tmux switch-client")
            .expect("switch-client present");
        assert!(
            attach < switch,
            "switch-client first hijacks another live client: {built}"
        );
    }

    #[test]
    fn a_name_with_a_line_break_is_refused_not_quoted() {
        // Quoting cannot make this safe for a line-oriented shell, so
        // the command is never built at all.
        assert!(kill_session_command("foo\nbar").is_err());
        assert!(attach_command("foo\rbar").is_err());
    }

    #[test]
    fn the_listing_command_survives_the_single_quote_wrapper() {
        let cmd = list_sessions_command();
        assert!(cmd.starts_with("sh -c '"));
        assert!(cmd.ends_with('\''));
        // Exactly the two wrapper quotes: any third would end the
        // wrapper early and hand the rest to the login shell.
        assert_eq!(cmd.matches('\'').count(), 2);
    }
}
