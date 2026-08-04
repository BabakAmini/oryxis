//! Prompt-driven credential autofill for Telnet sessions.
//!
//! Telnet has no in-protocol authentication: the server just prints a
//! `login:` / `Password:` prompt and reads a line. Every GUI client
//! that "supports" Telnet credentials answers those prompts by
//! watching the output stream; this module is that watcher.
//!
//! The watching itself is not ours any more: it is a two-step
//! [`oryxis_core::login_script`] run, the same engine the host login
//! scripts use, so there is ONE implementation of "read the output and
//! answer it" in the tree and one place where its guards live (window
//! from connect, fire once per step, prompt must terminate the
//! stream). This module only supplies the two prompt patterns and the
//! credentials.
//!
//! Both steps are `optional`, which is what preserves the behavior
//! this file had before the engine existed: gear that goes straight to
//! `Password:` without ever printing a username prompt still gets its
//! password, because the engine skips an optional step when a later
//! one matches.
//!
//! The username half is mostly redundant with NEW-ENVIRON `USER`
//! (which real telnetd consumes to skip the login prompt), but network
//! appliances rarely speak RFC 1572 and just print `Username:`.

use oryxis_core::login_script::{
    ExpectPattern, LoginStep, RunnerAction, ScriptRunner, SendPayload, line_bytes,
};
use std::time::{Duration, Instant};

/// Prompt patterns, matched case-insensitively against the end of the
/// ANSI-stripped stream tail. Cover Unix telnetd (`login:`) and the
/// network-gear dialects (`Username:` on IOS, `User Name:` on some
/// switches).
const USERNAME_PROMPTS: &str = r"(?i)(?:login|username|user name|user):$";
const PASSWORD_PROMPTS: &str = r"(?i)(?:password|passcode):$";

pub struct AutoLogin {
    runner: ScriptRunner,
}

impl AutoLogin {
    pub fn new(username: Option<String>, password: Option<String>, window: Duration) -> Self {
        // Per-step timeout == the window: only the hard disarm ends a
        // Telnet autologin, since a slow appliance banner is normal and
        // there is no later step waiting on this one.
        let timeout_ms = u32::try_from(window.as_millis()).unwrap_or(u32::MAX);
        let mut steps = Vec::with_capacity(2);
        if let Some(user) = username {
            steps.push(LoginStep {
                expect: Some(ExpectPattern::Regex(USERNAME_PROMPTS.into())),
                send: SendPayload::Text(user),
                timeout_ms,
                optional: true,
            });
        }
        if let Some(pass) = password {
            steps.push(LoginStep {
                expect: Some(ExpectPattern::Regex(PASSWORD_PROMPTS.into())),
                send: SendPayload::Text(pass),
                timeout_ms,
                optional: true,
            });
        }
        AutoLogin {
            // The patterns are compile-time constants, so the only
            // failure mode is a bug in this file, caught by the tests
            // below rather than at runtime.
            runner: ScriptRunner::new(&steps, window, Instant::now())
                .expect("built-in telnet prompt patterns compile"),
        }
    }

    /// Feed one decoded output chunk. Returns the line to type back
    /// (terminal-level bytes; the caller's input path adds the CR LF
    /// mapping) when the stream currently ends in a matching prompt.
    pub fn observe(&mut self, output: &[u8]) -> Option<Vec<u8>> {
        if self.runner.is_done() {
            return None;
        }
        self.runner.feed(output);
        let now = Instant::now();
        let mut out: Vec<u8> = Vec::new();
        // Drained to completion so `exhausted` is accurate as soon as
        // the last credential goes out. Both steps clear the tail when
        // they fire, so in practice at most one answers a given chunk.
        while let Some(action) = self.runner.poll(now) {
            match action {
                RunnerAction::Send {
                    payload: SendPayload::Text(text),
                    ..
                } => out.extend_from_slice(&line_bytes(&text)),
                // A Telnet autologin has nothing to report: giving up
                // quietly and letting the user type is the whole
                // fallback behavior.
                RunnerAction::Send { .. } | RunnerAction::Timeout { .. } | RunnerAction::Finished => {}
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// Both credentials spent (or never configured): the caller can
    /// stop routing output through the watcher.
    pub fn exhausted(&self) -> bool {
        self.runner.is_done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(user: Option<&str>, pass: Option<&str>) -> AutoLogin {
        AutoLogin::new(
            user.map(str::to_string),
            pass.map(str::to_string),
            Duration::from_secs(60),
        )
    }

    #[test]
    fn answers_login_then_password_once_each() {
        let mut al = armed(Some("admin"), Some("hunter2"));
        assert_eq!(
            al.observe(b"Ubuntu 22.04\r\nrouter login: "),
            Some(b"admin\r".to_vec())
        );
        assert_eq!(al.observe(b"Password: "), Some(b"hunter2\r".to_vec()));
        assert!(al.exhausted());
        // A second password prompt (rejected credential) falls through
        // to the user, never a retry loop.
        assert_eq!(al.observe(b"Login incorrect\r\nPassword: "), None);
    }

    #[test]
    fn prompt_must_terminate_the_stream() {
        let mut al = armed(Some("admin"), Some("pw"));
        // "login:" mid-line followed by more text is not a prompt.
        assert_eq!(al.observe(b"last login: Tue Jul 1 from 10.0.0.5\r\n"), None);
    }

    #[test]
    fn colored_and_spaced_prompts_match() {
        let mut al = armed(Some("admin"), None);
        // Cisco-style with an SGR color around it.
        assert_eq!(
            al.observe(b"\x1b[1mUsername:\x1b[0m "),
            Some(b"admin\r".to_vec())
        );
    }

    #[test]
    fn prompt_split_across_chunks_matches() {
        let mut al = armed(None, Some("pw"));
        assert_eq!(al.observe(b"Pass"), None);
        assert_eq!(al.observe(b"word: "), Some(b"pw\r".to_vec()));
    }

    #[test]
    fn missing_credentials_never_fire() {
        let mut al = armed(None, None);
        assert!(al.exhausted());
        assert_eq!(al.observe(b"login: "), None);
        assert_eq!(al.observe(b"Password: "), None);
    }

    #[test]
    fn a_password_only_gateway_still_gets_its_password() {
        // Gear that never prints a username prompt: the optional
        // username step is skipped rather than blocking the run.
        let mut al = armed(Some("admin"), Some("pw"));
        assert_eq!(al.observe(b"Password: "), Some(b"pw\r".to_vec()));
    }

    #[test]
    fn window_expiry_disarms() {
        let mut al = AutoLogin::new(None, Some("pw".into()), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(al.observe(b"Password: "), None);
    }
}
