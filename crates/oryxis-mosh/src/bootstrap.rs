//! Starting `mosh-server` on the far end and reading what it answers.
//!
//! mosh is not dialled, it is HANDED OVER. The client cannot reach a
//! server that does not exist yet and has no port or key to reach it
//! with, so an SSH session runs `mosh-server new` on the host and the
//! server prints the two things the UDP session needs:
//!
//! ```text
//! MOSH CONNECT 60123 x1EPWkMOnscXoutqiy4nMQ
//! ```
//!
//! That single line is the whole handover, and it is why every mosh
//! host is an SSH host first. Nothing here does any I/O: the command
//! is synthesized and the answer is parsed, so both are testable
//! without a server, and the exec channel that carries them belongs to
//! the caller.

use std::fmt::Write as _;

/// What the far end answered, once it is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handover {
    /// The UDP port `mosh-server` is listening on.
    pub port: u16,
    /// The session key, in mosh's 22-character base64 form. Secret:
    /// anyone holding it owns the session, so it is never logged.
    pub key: String,
}

/// Why a handover did not happen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BootstrapError {
    /// The host has no `mosh-server`. By far the commonest failure and
    /// the one worth naming exactly, because the fix is on the HOST and
    /// nothing about the local setup will change it.
    #[error("the host has no mosh-server; install mosh there (package `mosh`)")]
    NotInstalled,
    /// It ran and refused. Carries whatever it said, which is usually
    /// the actual reason (no free port in the range, locale refused).
    #[error("mosh-server would not start: {0}")]
    Refused(String),
    /// It ran, said something, and none of it was the handover line.
    #[error("mosh-server started but never announced a port and key")]
    NoAnnouncement,
    /// The line was there and did not parse.
    #[error("the handover line could not be read: {0}")]
    Malformed(String),
}

/// How to start the server, in the terms a host editor collects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    /// The binary to run. Empty means `mosh-server`, found on `PATH`.
    /// A path is what makes a host with mosh installed somewhere
    /// unusual work at all, so it is per-host rather than assumed.
    pub server_path: String,
    /// UDP ports the server may bind, as mosh's own `-p` takes them.
    /// Empty means let it choose from its default range, which is what
    /// a host with no firewall in the way wants.
    pub port_range: String,
    /// What to run instead of the login shell, if anything.
    pub command: Option<String>,
    /// The locale to hand it. mosh-server refuses a non-UTF-8 one for
    /// the same reason the client does, and the login shell of a host
    /// reached non-interactively often has none set at all.
    pub locale: String,
}

impl Default for ServerCommand {
    fn default() -> Self {
        Self {
            server_path: String::new(),
            port_range: String::new(),
            command: None,
            // What mosh's own wrapper falls back to, and the one locale
            // every system has.
            locale: "en_US.UTF-8".to_string(),
        }
    }
}

/// Single-quote a value for `/bin/sh`.
///
/// Everything between single quotes is literal to a POSIX shell except
/// a single quote itself, which is ended, escaped and reopened. That
/// covers every byte, which matters because these values are typed by
/// a person into a host editor and one of them names a path.
fn shell_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

impl ServerCommand {
    /// The line to run on the host.
    ///
    /// Shaped like mosh's own wrapper: `new` for a fresh session, `-s`
    /// so the server binds the address the SSH connection arrived on
    /// rather than guessing which interface faces the client, and the
    /// locale passed explicitly because a non-interactive login often
    /// has none.
    pub fn render(&self) -> String {
        let binary = if self.server_path.trim().is_empty() {
            "mosh-server".to_string()
        } else {
            shell_quote(self.server_path.trim())
        };
        let mut line = format!("{binary} new -s -c 256");
        if !self.port_range.trim().is_empty() {
            let _ = write!(line, " -p {}", shell_quote(self.port_range.trim()));
        }
        let _ = write!(line, " -l LANG={}", shell_quote(&self.locale));
        if let Some(command) = self.command.as_ref().filter(|c| !c.trim().is_empty()) {
            // Everything after `--` is the command and its arguments,
            // handed to the shell on the host as typed.
            let _ = write!(line, " -- {}", command.trim());
        }
        line
    }
}

/// Read the handover out of whatever `mosh-server` printed.
///
/// Takes stdout and stderr together on purpose. mosh-server writes the
/// announcement to stdout and its complaints to stderr, and a caller
/// that only kept one of them would either lose the answer or lose the
/// reason there was none.
pub fn parse(output: &str) -> Result<Handover, BootstrapError> {
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("MOSH CONNECT") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let (Some(port), Some(key)) = (parts.next(), parts.next()) else {
            return Err(BootstrapError::Malformed(line.to_string()));
        };
        let port: u16 = port
            .parse()
            .map_err(|_| BootstrapError::Malformed(line.to_string()))?;
        // The key is base64 of a 128-bit key, which is 22 characters
        // with the padding dropped. Checking the shape here means a
        // truncated line is reported as the transport problem it is,
        // rather than as an authentication failure minutes later.
        if key.len() != 22 {
            return Err(BootstrapError::Malformed(line.to_string()));
        }
        return Ok(Handover { port, key: key.to_string() });
    }

    // No announcement. What the host said instead is the diagnosis.
    let lowered = output.to_ascii_lowercase();
    if lowered.contains("command not found")
        || lowered.contains("not found")
        || lowered.contains("no such file")
    {
        return Err(BootstrapError::NotInstalled);
    }
    let complaint: String = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join("; ");
    if complaint.is_empty() {
        Err(BootstrapError::NoAnnouncement)
    } else {
        Err(BootstrapError::Refused(complaint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_announcement_is_read_out_of_a_real_greeting() {
        // Exactly what mosh-server 1.4.0 prints, blank line and all.
        let said = "MOSH CONNECT 60123 x1EPWkMOnscXoutqiy4nMQ\n\n\
                    mosh-server (mosh 1.4.0) [build mosh 1.4.0]\n\
                    Copyright 2012 Keith Winstein\n";
        let got = parse(said).expect("the line is there");
        assert_eq!(got.port, 60123);
        assert_eq!(got.key, "x1EPWkMOnscXoutqiy4nMQ");
    }

    #[test]
    fn a_host_without_mosh_says_so_in_its_own_words() {
        for said in [
            "bash: mosh-server: command not found",
            "sh: 1: mosh-server: not found",
            "/usr/bin/env: 'mosh-server': No such file or directory",
        ] {
            assert_eq!(parse(said), Err(BootstrapError::NotInstalled), "{said}");
        }
    }

    #[test]
    fn a_refusal_carries_what_the_host_said() {
        // The one a firewall-constrained port range produces.
        let said = "mosh-server: Could not bind to any port in range 60000:60005\n";
        match parse(said) {
            Err(BootstrapError::Refused(why)) => assert!(why.contains("Could not bind"), "{why}"),
            other => panic!("expected a refusal carrying the reason, got {other:?}"),
        }
    }

    #[test]
    fn silence_is_its_own_answer() {
        assert_eq!(parse(""), Err(BootstrapError::NoAnnouncement));
        assert_eq!(parse("   \n\n"), Err(BootstrapError::NoAnnouncement));
    }

    #[test]
    fn a_truncated_key_is_a_transport_problem_not_an_auth_one() {
        // Half a line, which is what a closed channel mid-write leaves.
        // Reported now, rather than as a session that authenticates
        // against nothing several seconds later.
        let said = "MOSH CONNECT 60123 x1EPWkMOnsc\n";
        assert!(matches!(parse(said), Err(BootstrapError::Malformed(_))));
    }

    #[test]
    fn a_port_that_is_not_a_number_is_malformed() {
        let said = "MOSH CONNECT sixty x1EPWkMOnscXoutqiy4nMQ\n";
        assert!(matches!(parse(said), Err(BootstrapError::Malformed(_))));
    }

    #[test]
    fn the_default_command_is_moshs_own() {
        let line = ServerCommand::default().render();
        assert!(line.starts_with("mosh-server new -s -c 256"), "{line}");
        assert!(line.contains("-l LANG='en_US.UTF-8'"), "{line}");
        // No `-p` when no range was asked for: mosh picks from its own
        // range, which is what a host with nothing in the way wants.
        assert!(!line.contains(" -p "), "{line}");
    }

    #[test]
    fn a_port_range_and_a_command_reach_the_line() {
        let line = ServerCommand {
            port_range: "60000:60010".into(),
            command: Some("tmux new -A -s work".into()),
            ..Default::default()
        }
        .render();
        assert!(line.contains(" -p '60000:60010'"), "{line}");
        assert!(line.ends_with(" -- tmux new -A -s work"), "{line}");
    }

    #[test]
    fn a_path_with_a_quote_in_it_cannot_break_out_of_the_line() {
        // Not paranoia about the user: these values ride sync and
        // portable import, so one can arrive from a peer that a person
        // here never typed into. The same reasoning that put a consent
        // gate on command proxies applies to anything that becomes a
        // remote shell word.
        let line = ServerCommand {
            server_path: "/opt/x'; rm -rf /; echo '".into(),
            ..Default::default()
        }
        .render();
        assert!(line.starts_with("'/opt/x'\\''; rm -rf /; echo '\\'''"), "{line}");
        assert!(!line.contains("; rm -rf /; echo ' new"), "not a separate word: {line}");
    }

    #[test]
    fn the_announcement_is_found_among_noise_before_it() {
        // A login banner, a motd, an sshrc that echoes. All of it
        // arrives on the same channel and none of it is the answer.
        let said = "Welcome to Ubuntu 24.04 LTS\n\
                    Last login: Sat Aug 23 21:00:00 2026\n\
                    MOSH CONNECT 60999 AAAAAAAAAAAAAAAAAAAAAA\n";
        assert_eq!(parse(said).unwrap().port, 60999);
    }
}
