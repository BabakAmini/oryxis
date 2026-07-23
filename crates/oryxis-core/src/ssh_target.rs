//! Ad-hoc SSH target parsing for quick connect.
//!
//! Turns a typed `user@host[:port]` string into its parts without ever
//! touching the vault. Follows OpenSSH conventions: the username splits
//! at the *last* `@` (usernames may legally contain `@`), IPv6 literals
//! take RFC 3986 brackets when a port is present (`user@[::1]:2222`),
//! and a bare bracketless IPv6 address is accepted when it parses as
//! one. Anything left over is a rejection, never a truncation, so a
//! search-box string that merely resembles a target does not connect
//! to a mangled host.

use std::net::{Ipv4Addr, Ipv6Addr};

/// A parsed quick-connect target. `username`/`port` stay `None` when
/// omitted so callers can apply their own defaults (local OS user, 22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub username: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl SshTarget {
    /// Parse `[user@]host[:port]`. Returns `None` on anything that is
    /// not unambiguously a connect target (empty parts, whitespace,
    /// invalid port, malformed IPv6 brackets, hostname charset).
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if input.is_empty() || input.chars().any(char::is_whitespace) {
            return None;
        }

        // Username splits at the last `@` so `user@part@host` keeps the
        // full `user@part` as the username, matching OpenSSH.
        let (username, rest) = match input.rfind('@') {
            Some(at) => {
                let (user, host_part) = (&input[..at], &input[at + 1..]);
                if user.is_empty() || host_part.is_empty() {
                    return None;
                }
                (Some(user.to_string()), host_part)
            }
            None => (None, input),
        };

        let (host, port) = parse_host_port(rest)?;
        Some(Self { username, host, port })
    }

    /// Render the target back as `user@host:port`, bracketing IPv6
    /// hosts. Used as the ephemeral connection's label.
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        if let Some(user) = &self.username {
            out.push_str(user);
            out.push('@');
        }
        if self.host.contains(':') {
            out.push('[');
            out.push_str(&self.host);
            out.push(']');
        } else {
            out.push_str(&self.host);
        }
        if let Some(port) = self.port {
            out.push(':');
            out.push_str(&port.to_string());
        }
        out
    }

    /// Whether the host is an IP literal (v4 or v6). Callers use this
    /// to offer quick connect for bare addresses without demanding an
    /// explicit `@` or `:` marker. Goes through the same zone-tolerant
    /// v6 check as `parse` (std `IpAddr` rejects `fe80::1%eth0`), so a
    /// bracketed literal that parsed keeps counting as one here:
    /// `[fe80::1%eth0]` and `[fe80::1%eth0]:22` must classify alike.
    pub fn host_is_ip_literal(&self) -> bool {
        self.host.parse::<Ipv4Addr>().is_ok() || is_ipv6_literal(&self.host)
    }

    /// Whether the input names an EXPLICIT connect target: a username,
    /// a port, or an IP-literal host. A bare hostname is ambiguous (it
    /// could be a saved-host search or the start of an add-host flow);
    /// everything else can only mean "connect to this".
    pub fn is_explicit(&self) -> bool {
        self.username.is_some() || self.port.is_some() || self.host_is_ip_literal()
    }
}

/// Split the post-username remainder into host and optional port.
fn parse_host_port(rest: &str) -> Option<(String, Option<u16>)> {
    if let Some(inner) = rest.strip_prefix('[') {
        // Bracketed IPv6: `[addr]` or `[addr]:port`.
        let (addr, tail) = inner.split_once(']')?;
        if !is_ipv6_literal(addr) {
            return None;
        }
        let port = match tail {
            "" => None,
            _ => Some(parse_port(tail.strip_prefix(':')?)?),
        };
        return Some((addr.to_string(), port));
    }

    match rest.matches(':').count() {
        0 => {
            if !is_hostname(rest) {
                return None;
            }
            Some((rest.to_string(), None))
        }
        1 => {
            let (host, port) = rest.split_once(':').expect("one colon counted");
            if !is_hostname(host) {
                return None;
            }
            Some((host.to_string(), Some(parse_port(port)?)))
        }
        // Two or more colons can only be a bare IPv6 address; a port
        // would be ambiguous and requires the bracket form.
        _ => {
            if !is_ipv6_literal(rest) {
                return None;
            }
            Some((rest.to_string(), None))
        }
    }
}

/// All-digits port in `1..=65535`. Port 0 is not a connectable target.
fn parse_port(s: &str) -> Option<u16> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match s.parse::<u16>() {
        Ok(0) | Err(_) => None,
        Ok(p) => Some(p),
    }
}

/// Structural IPv6 check, tolerating a non-empty `%zone` suffix (std
/// `Ipv6Addr` does not parse zone identifiers).
fn is_ipv6_literal(s: &str) -> bool {
    let addr = match s.split_once('%') {
        Some((addr, zone)) => {
            if zone.is_empty() || !zone.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_') {
                return false;
            }
            addr
        }
        None => s,
    };
    addr.parse::<Ipv6Addr>().is_ok()
}

/// DNS-name / IPv4 charset: letters, digits, dot, dash, underscore.
/// Requires at least one alphanumeric so lone punctuation never reads
/// as a host.
fn is_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && s.bytes().any(|b| b.is_ascii_alphanumeric())
}

/// The local OS username, for defaulting an omitted `user@` the way
/// OpenSSH does. Env-based on purpose: covers Linux/macOS (`USER`,
/// `LOGNAME`) and Windows (`USERNAME`) without a new dependency.
pub fn local_username() -> Option<String> {
    ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(username: Option<&str>, host: &str, port: Option<u16>) -> SshTarget {
        SshTarget {
            username: username.map(str::to_string),
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn user_host() {
        assert_eq!(SshTarget::parse("deploy@web01"), Some(t(Some("deploy"), "web01", None)));
    }

    #[test]
    fn user_host_port() {
        assert_eq!(
            SshTarget::parse("deploy@web01.example.com:2222"),
            Some(t(Some("deploy"), "web01.example.com", Some(2222)))
        );
    }

    #[test]
    fn bare_host_and_port() {
        assert_eq!(SshTarget::parse("web01"), Some(t(None, "web01", None)));
        assert_eq!(SshTarget::parse("web01:2222"), Some(t(None, "web01", Some(2222))));
    }

    #[test]
    fn username_keeps_extra_at() {
        // Split at the last `@`: the username may itself contain one.
        assert_eq!(
            SshTarget::parse("user@corp@web01"),
            Some(t(Some("user@corp"), "web01", None))
        );
    }

    #[test]
    fn ipv4_host() {
        assert_eq!(
            SshTarget::parse("root@192.168.0.10:22"),
            Some(t(Some("root"), "192.168.0.10", Some(22)))
        );
    }

    #[test]
    fn ipv6_bracketed() {
        assert_eq!(
            SshTarget::parse("user@[::1]:2222"),
            Some(t(Some("user"), "::1", Some(2222)))
        );
        assert_eq!(SshTarget::parse("[::1]"), Some(t(None, "::1", None)));
        assert_eq!(
            SshTarget::parse("[fe80::1%eth0]:22"),
            Some(t(None, "fe80::1%eth0", Some(22)))
        );
    }

    #[test]
    fn ipv6_bare() {
        assert_eq!(SshTarget::parse("::1"), Some(t(None, "::1", None)));
        assert_eq!(
            SshTarget::parse("user@fe80::abcd"),
            Some(t(Some("user"), "fe80::abcd", None))
        );
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "", " ", "@host", "user@", "user@host stuff", "user @host",
            "host:", ":22", "host:0", "host:99999", "host:22x",
            "a:b:c", "[::1", "[::1]x", "[]", "[not-v6]:22", "user@[::1]:",
            "...", "-", "[fe80::1%]:22",
        ] {
            assert_eq!(SshTarget::parse(bad), None, "should reject {bad:?}");
        }
    }

    #[test]
    fn trims_outer_whitespace() {
        assert_eq!(SshTarget::parse("  user@host  "), Some(t(Some("user"), "host", None)));
    }

    #[test]
    fn canonical_round_trips() {
        for s in ["user@host", "user@host:2222", "host", "host:2222", "user@[::1]:2222", "[::1]"] {
            let parsed = SshTarget::parse(s).unwrap();
            let canon = parsed.canonical();
            assert_eq!(SshTarget::parse(&canon), Some(parsed), "canonical of {s:?} = {canon:?}");
        }
    }

    #[test]
    fn canonical_brackets_v6_only_as_needed() {
        assert_eq!(t(Some("u"), "::1", Some(22)).canonical(), "u@[::1]:22");
        assert_eq!(t(None, "web01", None).canonical(), "web01");
    }

    #[test]
    fn ip_literal_detection() {
        assert!(SshTarget::parse("192.168.0.1").unwrap().host_is_ip_literal());
        assert!(SshTarget::parse("::1").unwrap().host_is_ip_literal());
        assert!(!SshTarget::parse("web01").unwrap().host_is_ip_literal());
        // Zone-id IPv6 counts too: `parse` accepts the zone, so the
        // literal check must agree or `[fe80::1%eth0]` (no port) and
        // `[fe80::1%eth0]:22` classify differently.
        assert!(SshTarget::parse("[fe80::1%eth0]").unwrap().host_is_ip_literal());
    }

    #[test]
    fn explicit_targets() {
        // A username, a port or an IP literal each mark the input as an
        // unambiguous connect target; a bare hostname stays ambiguous.
        assert!(SshTarget::parse("root@web01").unwrap().is_explicit());
        assert!(SshTarget::parse("web01:2222").unwrap().is_explicit());
        assert!(SshTarget::parse("10.0.0.7").unwrap().is_explicit());
        // A zoned bracketed literal is explicit with or without a port.
        assert!(SshTarget::parse("[fe80::1%eth0]").unwrap().is_explicit());
        assert!(SshTarget::parse("[fe80::1%eth0]:22").unwrap().is_explicit());
        assert!(!SshTarget::parse("web01").unwrap().is_explicit());
    }
}
