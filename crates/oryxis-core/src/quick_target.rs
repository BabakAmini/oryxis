//! Protocol-aware quick-connect parsing.
//!
//! [`SshTarget`] answers "which host", this answers "over what". A
//! quick-connect box is one line of text, and the people who asked for
//! it (issue #174) live on gear that speaks Telnet or hangs off a
//! console server, so the protocol has to be sayable in that same line:
//! `telnet://sw01`, `raw://console:2001`, `serial:///dev/ttyUSB0`.
//!
//! A target with no scheme stays UNDECIDED rather than silently
//! meaning SSH. The caller (the quick-connect card) offers the choice
//! as badges next to the line, so `10.0.0.1` can still be dialled as
//! Telnet without retyping it with a prefix.

use crate::models::connection::ConnectionProtocol;
use crate::ssh_target::SshTarget;

/// Where a quick-connect target points. Network protocols carry a host
/// (and optionally a user and port); `Serial` carries a device path,
/// which is not a host in any sense the host grammar would accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickEndpoint {
    Network(SshTarget),
    Serial {
        /// `/dev/ttyUSB0`, `COM3`, verbatim as typed.
        device: String,
        /// Symbol rate from `serial://<device>:<baud>`. `None` keeps
        /// the 9600 default every serial host starts at.
        baud: Option<u32>,
    },
}

/// A parsed quick-connect line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickTarget {
    /// `None` when the line named no scheme: the text is a valid target
    /// but the protocol is still the user's to pick.
    pub protocol: Option<ConnectionProtocol>,
    /// Set by the `telnets://` scheme, whose whole meaning is "Telnet
    /// inside TLS". Never true for any other scheme.
    pub tls: bool,
    pub endpoint: QuickEndpoint,
}

impl QuickTarget {
    /// Parse a quick-connect line, with or without a `scheme://`.
    ///
    /// Returns `None` for anything that is not unambiguously a target:
    /// an unknown scheme, a malformed host, a `serial://` with no
    /// device, or a `raw://` with no port. That last one is a real
    /// refusal rather than a default, because console servers map every
    /// serial line to its own port (2001, 3001, 7001, ...) and no
    /// vendor agrees on which: any number we picked would be wrong on
    /// most gear while looking authoritative.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let Some((scheme, rest)) = split_scheme(input) else {
            // A device path names its protocol by being one: `/dev/x`
            // and `COM3` are not hosts in any reading, so Serial here is
            // a fact rather than a guess, and the caller has no choice
            // to offer.
            // Split the optional `:<baud>` off FIRST, then judge the
            // device: `COM3:9600` is a port with a rate, and testing the
            // whole string would read that colon as a host port.
            if let Some((device, baud)) = parse_serial_endpoint(input)
                && looks_like_serial_device(&device)
            {
                return Some(QuickTarget {
                    protocol: Some(ConnectionProtocol::Serial),
                    tls: false,
                    endpoint: QuickEndpoint::Serial { device, baud },
                });
            }
            // Otherwise an ordinary `user@host[:port]` line, protocol
            // undecided.
            let target = SshTarget::parse(input)?;
            return Some(QuickTarget {
                protocol: None,
                tls: false,
                endpoint: QuickEndpoint::Network(target),
            });
        };
        let protocol = ConnectionProtocol::from_scheme(scheme)?;
        let tls = scheme.eq_ignore_ascii_case("telnets");
        if protocol == ConnectionProtocol::Serial {
            let (device, baud) = parse_serial_endpoint(rest)?;
            return Some(QuickTarget {
                protocol: Some(protocol),
                tls,
                endpoint: QuickEndpoint::Serial { device, baud },
            });
        }
        let mut target = SshTarget::parse(rest)?;
        if target.port.is_none() {
            // `telnets` has no `default_port` of its own (it is Telnet
            // plus TLS), so its conventional 992 is applied here.
            target.port = if tls { Some(992) } else { protocol.default_port() };
        }
        if protocol == ConnectionProtocol::Raw && target.port.is_none() {
            return None;
        }
        Some(QuickTarget {
            protocol: Some(protocol),
            tls,
            endpoint: QuickEndpoint::Network(target),
        })
    }

    /// The network half, for the callers that only handle hosts.
    pub fn network(&self) -> Option<&SshTarget> {
        match &self.endpoint {
            QuickEndpoint::Network(t) => Some(t),
            QuickEndpoint::Serial { .. } => None,
        }
    }
}

/// Split a leading `scheme://`. Rejects a bare `//` and anything whose
/// scheme is not an RFC 3986 one, so `user:pass@host` (a colon with no
/// slashes) still reaches the ordinary host grammar.
fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = input.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some((scheme, rest))
}

/// Parse `serial://` payloads: `/dev/ttyUSB0`, `COM3`, or either with a
/// `:<baud>` suffix. The device keeps its own colons out of the way by
/// splitting at the LAST one and only accepting it as a baud rate when
/// it is all digits, so a device path is never truncated into one.
fn parse_serial_endpoint(rest: &str) -> Option<(String, Option<u32>)> {
    let rest = rest.trim();
    if rest.is_empty() || rest.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some((device, tail)) = rest.rsplit_once(':')
        && !device.is_empty()
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit())
        && let Ok(baud) = tail.parse::<u32>()
        && baud > 0
    {
        return Some((device.to_string(), Some(baud)));
    }
    Some((rest.to_string(), None))
}

/// Whether a typed line looks like a serial device rather than a host,
/// which is what decides if the quick-connect card offers a Serial
/// badge at all. Deliberately narrow: the two shapes every serial user
/// already types (`/dev/...` on Unix, `COM<n>` on Windows). A hostname
/// can be neither, so the badge never appears on an ordinary target.
pub fn looks_like_serial_device(input: &str) -> bool {
    let input = input.trim();
    if input.starts_with("/dev/") && input.len() > "/dev/".len() {
        return true;
    }
    let upper = input.to_ascii_uppercase();
    if let Some(n) = upper.strip_prefix("COM")
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(input: &str) -> QuickTarget {
        QuickTarget::parse(input).expect("test input must parse")
    }

    #[test]
    fn a_bare_target_leaves_the_protocol_undecided() {
        // The badges exist because of this: the line is a valid target
        // and the protocol is still an open question.
        let t = net("root@10.0.0.1");
        assert_eq!(t.protocol, None);
        assert!(!t.tls);
        let host = t.network().expect("network endpoint");
        assert_eq!(host.host, "10.0.0.1");
        assert_eq!(host.username.as_deref(), Some("root"));
        assert_eq!(host.port, None, "an unstated port stays unstated");
    }

    #[test]
    fn schemes_pick_the_protocol_and_seed_the_port() {
        let t = net("telnet://sw01");
        assert_eq!(t.protocol, Some(ConnectionProtocol::Telnet));
        assert_eq!(t.network().unwrap().port, Some(23));

        let t = net("ssh://root@web01");
        assert_eq!(t.protocol, Some(ConnectionProtocol::Ssh));
        assert_eq!(t.network().unwrap().port, Some(22));
        assert_eq!(t.network().unwrap().username.as_deref(), Some("root"));

        // A typed port always wins over the scheme's default.
        let t = net("telnet://sw01:2323");
        assert_eq!(t.network().unwrap().port, Some(2323));
    }

    #[test]
    fn telnets_means_telnet_over_tls_on_992() {
        let t = net("telnets://sw01");
        assert_eq!(t.protocol, Some(ConnectionProtocol::Telnet));
        assert!(t.tls);
        assert_eq!(t.network().unwrap().port, Some(992));
    }

    #[test]
    fn raw_requires_a_port() {
        // No vendor agrees on a console-server port, so there is
        // nothing honest to default to.
        assert_eq!(QuickTarget::parse("raw://console"), None);
        let t = net("raw://console:2001");
        assert_eq!(t.protocol, Some(ConnectionProtocol::Raw));
        assert_eq!(t.network().unwrap().port, Some(2001));
    }

    #[test]
    fn serial_takes_a_device_path_not_a_host() {
        let t = net("serial:///dev/ttyUSB0");
        assert_eq!(t.protocol, Some(ConnectionProtocol::Serial));
        assert_eq!(
            t.endpoint,
            QuickEndpoint::Serial { device: "/dev/ttyUSB0".to_string(), baud: None }
        );

        let t = net("serial://COM3:115200");
        assert_eq!(
            t.endpoint,
            QuickEndpoint::Serial { device: "COM3".to_string(), baud: Some(115200) }
        );

        // A path is never truncated into a baud rate.
        let t = net("serial:///dev/serial/by-id/usb-FTDI");
        assert_eq!(
            t.endpoint,
            QuickEndpoint::Serial {
                device: "/dev/serial/by-id/usb-FTDI".to_string(),
                baud: None,
            }
        );
        assert_eq!(QuickTarget::parse("serial://"), None);
    }

    #[test]
    fn unknown_schemes_are_not_targets() {
        // Typing a URL in the search box must not connect to anything.
        for s in ["http://example.com", "https://example.com", "file:///etc/hosts"] {
            assert_eq!(QuickTarget::parse(s), None, "{s}");
        }
    }

    #[test]
    fn a_userinfo_colon_is_not_a_scheme() {
        // `user:secret@host` has a colon but no `//`: it must reach the
        // ordinary host grammar, which routes the secret away from the
        // plaintext username.
        let t = net("root:hunter2@web01");
        assert_eq!(t.protocol, None);
        let host = t.network().unwrap();
        assert_eq!(host.username.as_deref(), Some("root"));
        assert_eq!(host.password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn a_bare_device_path_is_serial_without_a_scheme() {
        // `COM3` and `/dev/tty*` are not hosts under any protocol, so
        // there is nothing for the user to choose between.
        let t = net("/dev/ttyUSB0");
        assert_eq!(t.protocol, Some(ConnectionProtocol::Serial));
        assert_eq!(
            t.endpoint,
            QuickEndpoint::Serial { device: "/dev/ttyUSB0".to_string(), baud: None }
        );
        let t = net("COM3:9600");
        assert_eq!(t.protocol, Some(ConnectionProtocol::Serial));
        assert_eq!(
            t.endpoint,
            QuickEndpoint::Serial { device: "COM3".to_string(), baud: Some(9600) }
        );
        // A hostname that merely starts with the same letters is still
        // a host.
        assert_eq!(net("comet").protocol, None);
    }

    #[test]
    fn serial_badge_only_shows_for_device_shapes() {
        assert!(looks_like_serial_device("/dev/ttyUSB0"));
        assert!(looks_like_serial_device("COM3"));
        assert!(looks_like_serial_device("com12"));
        assert!(!looks_like_serial_device("10.0.0.1"));
        assert!(!looks_like_serial_device("comet"));
        assert!(!looks_like_serial_device("/dev/"));
        assert!(!looks_like_serial_device("web01"));
    }
}
