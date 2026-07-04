//! Prompt-driven credential autofill for Telnet sessions.
//!
//! Telnet has no in-protocol authentication: the server just prints a
//! `login:` / `Password:` prompt and reads a line. Every GUI client
//! that "supports" Telnet credentials answers those prompts by
//! watching the output stream; this module is that watcher, with the
//! guards that keep it from becoming a credential leak:
//!
//! - each credential is sent AT MOST ONCE per session (a rejected
//!   password falls through to the user, never a retry loop);
//! - the watcher disarms after a grace window from connect, so a
//!   `password:` string scrolling by in a file listing an hour later
//!   can never trigger an injection;
//! - the prompt must be the LAST thing on the stream (suffix match on
//!   the accumulated tail), not merely appear somewhere in a chunk.
//!
//! The username half is mostly redundant with NEW-ENVIRON `USER`
//! (which real telnetd consumes to skip the login prompt), but network
//! appliances rarely speak RFC 1572 and just print `Username:`.

use std::time::{Duration, Instant};

/// How much decoded output tail is kept for suffix matching. Prompts
/// are short; the cap only needs to survive ANSI-heavy redraws.
const TAIL_CAP: usize = 256;

pub struct AutoLogin {
    username: Option<String>,
    password: Option<String>,
    sent_username: bool,
    sent_password: bool,
    /// Hard disarm time; after this `observe` never fires again.
    deadline: Instant,
    /// Lowercased, ANSI-stripped suffix of everything seen so far.
    tail: String,
}

impl AutoLogin {
    pub fn new(username: Option<String>, password: Option<String>, window: Duration) -> Self {
        AutoLogin {
            username,
            password,
            sent_username: false,
            sent_password: false,
            deadline: Instant::now() + window,
            tail: String::new(),
        }
    }

    /// Feed one decoded output chunk. Returns the line to type back
    /// (terminal-level bytes; the caller's input path adds the CR LF
    /// mapping) when the stream currently ends in a matching prompt.
    pub fn observe(&mut self, output: &[u8]) -> Option<Vec<u8>> {
        if self.exhausted() || Instant::now() > self.deadline {
            return None;
        }
        self.tail.push_str(&strip_ansi_lossy(output).to_lowercase());
        if self.tail.len() > TAIL_CAP {
            let cut = self.tail.len() - TAIL_CAP;
            // Cut on a char boundary; the tail is lossy UTF-8 already.
            let cut = (cut..self.tail.len())
                .find(|i| self.tail.is_char_boundary(*i))
                .unwrap_or(0);
            self.tail.drain(..cut);
        }

        let trimmed = self.tail.trim_end();
        if !self.sent_username
            && let Some(user) = &self.username
            && ends_with_any(trimmed, USERNAME_PROMPTS)
        {
            self.sent_username = true;
            self.tail.clear();
            let mut line = user.clone().into_bytes();
            line.push(b'\r');
            return Some(line);
        }
        if !self.sent_password
            && let Some(pass) = &self.password
            && ends_with_any(trimmed, PASSWORD_PROMPTS)
        {
            self.sent_password = true;
            self.tail.clear();
            let mut line = pass.clone().into_bytes();
            line.push(b'\r');
            return Some(line);
        }
        None
    }

    /// Both credentials spent (or never configured): the caller can
    /// stop routing output through the watcher.
    pub fn exhausted(&self) -> bool {
        (self.username.is_none() || self.sent_username)
            && (self.password.is_none() || self.sent_password)
    }
}

/// Prompt suffixes, matched case-insensitively against the trimmed
/// stream tail. Cover Unix telnetd (`login:`) and the network-gear
/// dialects (`Username:` on IOS, `User Name:` on some switches).
const USERNAME_PROMPTS: &[&str] = &["login:", "username:", "user name:", "user:"];
const PASSWORD_PROMPTS: &[&str] = &["password:", "passcode:"];

fn ends_with_any(tail: &str, prompts: &[&str]) -> bool {
    prompts.iter().any(|p| tail.ends_with(p))
}

/// Drop ANSI escape sequences (CSI, OSC, two-byte ESC forms) and
/// non-text control bytes so a colored `Username:` still suffix-matches.
/// Non-ASCII bytes pass through untouched, real prompts are ASCII and
/// the lossy path only feeds the matcher, never the terminal.
fn strip_ansi_lossy(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            0x1b => {
                i += 1;
                match data.get(i) {
                    // CSI: parameters then one final byte in 0x40..=0x7E.
                    Some(b'[') => {
                        i += 1;
                        while i < data.len() && !(0x40..=0x7e).contains(&data[i]) {
                            i += 1;
                        }
                        i += 1;
                    }
                    // OSC: runs to BEL or ESC \.
                    Some(b']') => {
                        i += 1;
                        while i < data.len() {
                            if data[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if data[i] == 0x1b && data.get(i + 1) == Some(&b'\\') {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    // Two-byte escape (ESC c, ESC =, charset selects...).
                    Some(_) => i += 1,
                    None => {}
                }
            }
            b'\r' | b'\n' | b'\t' => {
                out.push(data[i] as char);
                i += 1;
            }
            c if c < 0x20 || c == 0x7f => i += 1,
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
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
    fn window_expiry_disarms() {
        let mut al = AutoLogin::new(None, Some("pw".into()), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(al.observe(b"Password: "), None);
    }
}
