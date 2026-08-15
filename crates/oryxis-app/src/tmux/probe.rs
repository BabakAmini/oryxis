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

/// Marker separating the session lines from the pane lines in the
/// listing payload. The pane lines carry each pane's foreground
/// command, which is what lets a row say a DETACHED session still has
/// work running inside it (issue #159 follow-up: "is there any way to
/// tell if a long-term process is running after you detach?").
const PANES: &str = "---ORYXIS-PANES---";

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
    // The second list rides the same round trip: one line per pane with
    // its foreground command, so the tab can mark sessions that still
    // have something running inside them. The session name goes FIRST
    // because it structurally cannot contain the separator, while a
    // process name theoretically could.
    let panes_format = ["#{session_name}", "#{pane_current_command}"].join(FIELD);
    let batch = format!(
        "command -v tmux >/dev/null 2>&1 || {{ echo {NO_TMUX}; exit 0; }}; \
         tmux list-sessions -F \"{format}\" 2>/dev/null || echo {NO_SERVER}; \
         echo {PANES}; \
         tmux list-panes -a -F \"{panes_format}\" 2>/dev/null"
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
    let mut sessions: Vec<TmuxSession> = Vec::new();
    let mut in_panes = false;
    for line in payload.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim() == PANES {
            in_panes = true;
            continue;
        }
        if line.trim().is_empty() || line.trim() == NO_SERVER {
            continue;
        }
        if in_panes {
            merge_pane_line(&mut sessions, line);
        } else if let Some(session) = parse_line(line) {
            sessions.push(session);
        }
    }
    Listing::Sessions(sessions)
}

/// One `list-panes -a` line: `<session name><FIELD><foreground command>`.
/// Split from the LEFT this time, because here it is the TRAILING field
/// (a process name) that could theoretically carry the separator, while
/// the session name structurally cannot. A command that is just a shell
/// at its prompt is not "work running", so it is skipped; everything
/// else lands on its session, deduplicated.
fn merge_pane_line(sessions: &mut [TmuxSession], line: &str) {
    let Some((name, command)) = line.split_once(FIELD) else {
        return;
    };
    let command = command.trim();
    if command.is_empty() || is_shell(command) {
        return;
    }
    if let Some(session) = sessions.iter_mut().find(|s| s.name == name)
        && !session.running.iter().any(|c| c == command)
    {
        session.running.push(command.to_string());
    }
}

/// Whether a pane's foreground command is a shell sitting at its
/// prompt. The leading dash of a login shell is stripped first
/// (`-bash`). Deliberately a list of shells rather than a heuristic: an
/// unknown command is more useful shown than guessed away.
fn is_shell(command: &str) -> bool {
    matches!(
        command.trim_start_matches('-'),
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "csh"
            | "tcsh"
            | "ksh"
            | "mksh"
            | "dash"
            | "ash"
            | "nu"
            | "pwsh"
            | "elvish"
            | "xonsh"
    )
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
        running: Vec::new(),
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

/// `tmux list-clients -t <session>`: the clients attached to the
/// session this pane is believed to be showing, so the switch can name
/// OUR client explicitly instead of typing into a shell that may be
/// busy (issue #159 follow-up: with `sleep 10` running inside the
/// session, a typed `switch-client` queues in the shell's input buffer
/// until the command finishes).
pub(crate) fn list_clients_command(name: &str) -> Result<String, oryxis_archive::ArchiveError> {
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!(
        "tmux list-clients -t {name} -F \"#{{client_activity}}{FIELD}#{{client_tty}}\""
    ))
}

/// Pick this pane's client tty out of a `list-clients` payload: the
/// most recently ACTIVE client attached to the session the pane is
/// showing. Usually that list has exactly one entry. When the session
/// is shared, the newest activity is the best available discriminator
/// (the user just ran something in this pane); the worst miss moves a
/// co-viewer of the SAME session, never an arbitrary client, which is
/// what makes this safe where the bare `switch-client` of issue #157
/// was not. `None` means no client is attached at all: the "attached
/// here" hint was stale and the caller falls back to the typed attach.
pub(crate) fn parse_clients(payload: &str) -> Option<String> {
    payload
        .lines()
        .filter_map(|line| {
            // Activity first, tty second: a tty path cannot carry the
            // separator, but splitting on the FIRST one keeps a
            // hypothetical odd tty intact anyway.
            let (activity, tty) = line.trim().split_once(FIELD)?;
            let tty = tty.trim();
            (!tty.is_empty())
                .then(|| (activity.trim().parse::<u64>().unwrap_or(0), tty.to_string()))
        })
        .max_by_key(|(activity, _)| *activity)
        .map(|(_, tty)| tty)
}

/// `tmux switch-client -c <tty> -t <session>`, run on an exec channel,
/// never typed. Naming the client is what makes this deterministic:
/// without `-c`, tmux run outside any client resolves "current client"
/// to the most recently used one ANYWHERE and exits 0 having moved it
/// (the issue #157 hijack). Both arguments are text the remote host
/// printed, so both are quoted.
pub(crate) fn switch_client_command(
    tty: &str,
    name: &str,
) -> Result<String, oryxis_archive::ArchiveError> {
    let tty = oryxis_archive::quote::sh_quote(tty)?;
    let name = oryxis_archive::quote::sh_quote(name)?;
    Ok(format!("tmux switch-client -c {tty} -t {name}"))
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
        // The switch pair quotes the name too (the tty check has its
        // own test: it is remote text just the same).
        for built in [
            list_clients_command(evil).unwrap(),
            switch_client_command("/dev/pts/0", evil).unwrap(),
        ] {
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
        assert!(list_clients_command("foo\nbar").is_err());
        // The tty came off the host exactly like the name did, so it
        // follows the same rules: quoted always, refused on a break.
        assert!(switch_client_command("/dev/pts\n/0", "work").is_err());
        assert_eq!(
            switch_client_command("/dev/pts/3", "it's").unwrap(),
            r#"tmux switch-client -c '/dev/pts/3' -t 'it'\''s'"#
        );
    }

    #[test]
    fn pane_lines_mark_sessions_with_work_running() {
        // `sleep` in one pane, shells at their prompt everywhere else:
        // only the sleep survives, on its own session, deduplicated.
        let payload = format!(
            "{}\n{}\n---ORYXIS-PANES---\nwork{FIELD}sleep\nwork{FIELD}sleep\nwork{FIELD}-bash\nbuild{FIELD}zsh\n",
            line("work", "2", "0", "1754600000", ""),
            line("build", "1", "1", "1754600100", "")
        );
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions[0].running, vec!["sleep".to_string()]);
        assert!(sessions[1].running.is_empty());
    }

    #[test]
    fn a_tmux_too_old_for_pane_commands_still_lists() {
        // No marker, no pane lines: exactly the payload every listing
        // produced before this field existed.
        let payload = line("work", "1", "0", "1754600000", "");
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].running.is_empty());
    }

    #[test]
    fn a_pane_line_for_an_unlisted_session_is_dropped() {
        // A session created between the two tmux calls of the batch:
        // its pane line has no row to land on and must not panic or
        // invent one.
        let payload = format!(
            "{}\n---ORYXIS-PANES---\nghost{FIELD}sleep\n",
            line("work", "1", "0", "1754600000", "")
        );
        let Listing::Sessions(sessions) = parse_listing(&payload) else {
            panic!("expected a session list");
        };
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].running.is_empty());
    }

    #[test]
    fn the_most_recently_active_client_wins() {
        // Two clients on the shared session: ours is the one the user
        // just typed into, i.e. the newest activity.
        let payload = format!(
            "1754600100{FIELD}/dev/pts/7\n1754600900{FIELD}/dev/pts/3\n"
        );
        assert_eq!(parse_clients(&payload).as_deref(), Some("/dev/pts/3"));
        // No clients at all: the hint was stale, the caller falls back
        // to the typed attach.
        assert_eq!(parse_clients(""), None);
        // A tty-less line (dead client mid-teardown) cannot be a target.
        assert_eq!(parse_clients(&format!("1754600100{FIELD}\n")), None);
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
