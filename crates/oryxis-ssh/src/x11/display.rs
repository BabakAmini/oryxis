//! `$DISPLAY` parsing: turn the X11 display string into a concrete local
//! endpoint we can dial for each forwarded X11 channel.
//!
//! The grammar is `[host]:<display>[.<screen>]`, but the three platforms
//! we ship on each stress a different corner of it:
//!
//! - Linux/BSD: `:0` / `:0.0` -> the abstract-free unix socket
//!   `/tmp/.X11-unix/X0`.
//! - macOS (XQuartz): `/private/tmp/com.apple.launchd.XXX/org.xquartz:0`
//!   -- the socket file is that ENTIRE string, `:0` included. A naive
//!   split on `:` that assumes `/tmp/.X11-unix/X<n>` silently fails here,
//!   so a host part containing `/` is treated as a literal socket path.
//! - Windows: no native X server. `DISPLAY` is usually unset and the user
//!   runs VcXsrv/Xming on TCP 6000, which `fallback()` targets.

use std::path::PathBuf;

/// Where the local X server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11Target {
    /// A unix-domain socket path.
    Unix(PathBuf),
    /// A TCP endpoint (`host`, `port`), port already resolved to 6000 + n.
    Tcp(String, u16),
}

/// The base TCP port X servers listen on; display N is port 6000 + N.
const X_TCP_BASE: u16 = 6000;

/// A parsed `$DISPLAY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySpec {
    /// The endpoint to dial for every forwarded X11 channel.
    pub target: X11Target,
    /// Display number (the `N` in `:N`), used to match `.Xauthority`
    /// entries, whose `number` field stores it as ASCII.
    pub number: u32,
    /// Screen number (the `S` in `:N.S`), forwarded verbatim in `x11-req`.
    pub screen: u32,
    /// Host part as written, `""` for a local display. Used to pick the
    /// right `.Xauthority` family when matching cookies.
    pub host: String,
}

impl DisplaySpec {
    /// Parse a `$DISPLAY` value. Returns `None` when the string does not
    /// carry a display number at all, which is the only truly malformed
    /// case; everything else resolves to a best-effort endpoint.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }

        // Split at the LAST ':' so an XQuartz launchd path (which itself
        // contains no ':', but a future one might) and IPv6-ish hosts
        // keep their prefix intact.
        let (host, tail) = value.rsplit_once(':')?;

        // `tail` is `N` or `N.S`.
        let (number_str, screen_str) = match tail.split_once('.') {
            Some((n, s)) => (n, Some(s)),
            None => (tail, None),
        };
        let number: u32 = number_str.parse().ok()?;
        let screen: u32 = match screen_str {
            // A present-but-unparseable screen is a malformed display;
            // don't silently pretend it was screen 0.
            Some(s) => s.parse().ok()?,
            None => 0,
        };

        let target = if host.contains('/') {
            // macOS / XQuartz launchd socket: the path INCLUDES the
            // `:N` suffix, so rebuild it rather than using `host`.
            X11Target::Unix(PathBuf::from(format!("{host}:{number}")))
        } else if host.is_empty() || host == "unix" {
            X11Target::Unix(PathBuf::from(format!("/tmp/.X11-unix/X{number}")))
        } else {
            X11Target::Tcp(
                host.to_string(),
                X_TCP_BASE.saturating_add(number.min(u32::from(u16::MAX)) as u16),
            )
        };

        Some(Self { target, number, screen, host: host.to_string() })
    }

    /// Resolve from the environment, falling back to the conventional
    /// third-party X server endpoint on Windows.
    ///
    /// The fallback is deliberately Windows-only: on Linux/macOS an unset
    /// `DISPLAY` means there is no X server to forward to, and guessing
    /// 6000 there would produce a confusing connection-refused per X11
    /// channel instead of one clear "no local display" warning.
    pub fn from_env() -> Option<Self> {
        match std::env::var("DISPLAY") {
            Ok(v) if !v.trim().is_empty() => Self::parse(&v),
            _ => Self::fallback(),
        }
    }

    /// The no-`DISPLAY` fallback: VcXsrv / Xming / MobaXterm on Windows
    /// listen on TCP 6000 with no `DISPLAY` exported to native processes.
    #[cfg(windows)]
    pub fn fallback() -> Option<Self> {
        Some(Self {
            target: X11Target::Tcp("127.0.0.1".to_string(), X_TCP_BASE),
            number: 0,
            screen: 0,
            host: "127.0.0.1".to_string(),
        })
    }

    #[cfg(not(windows))]
    pub fn fallback() -> Option<Self> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(p: &str) -> X11Target {
        X11Target::Unix(PathBuf::from(p))
    }

    #[test]
    fn plain_local_display() {
        let d = DisplaySpec::parse(":0").unwrap();
        assert_eq!(d.target, unix("/tmp/.X11-unix/X0"));
        assert_eq!(d.number, 0);
        assert_eq!(d.screen, 0);
    }

    #[test]
    fn local_display_with_screen() {
        let d = DisplaySpec::parse(":1.2").unwrap();
        assert_eq!(d.target, unix("/tmp/.X11-unix/X1"));
        assert_eq!(d.number, 1);
        assert_eq!(d.screen, 2);
    }

    #[test]
    fn unix_prefix_is_local() {
        let d = DisplaySpec::parse("unix:3").unwrap();
        assert_eq!(d.target, unix("/tmp/.X11-unix/X3"));
        assert_eq!(d.number, 3);
    }

    #[test]
    fn tcp_display_maps_to_6000_plus_n() {
        let d = DisplaySpec::parse("localhost:10.0").unwrap();
        assert_eq!(d.target, X11Target::Tcp("localhost".into(), 6010));
        assert_eq!(d.number, 10);
        assert_eq!(d.screen, 0);
    }

    #[test]
    fn remote_host_display() {
        let d = DisplaySpec::parse("192.168.1.5:0").unwrap();
        assert_eq!(d.target, X11Target::Tcp("192.168.1.5".into(), 6000));
    }

    /// The XQuartz socket file is literally named `org.xquartz:0`, so the
    /// `:0` must survive into the path. This is the macOS regression
    /// guard: a naive `/tmp/.X11-unix/X0` mapping breaks XQuartz.
    #[test]
    fn xquartz_launchd_path_keeps_display_suffix() {
        let raw = "/private/tmp/com.apple.launchd.abc123/org.xquartz:0";
        let d = DisplaySpec::parse(raw).unwrap();
        assert_eq!(d.target, unix(raw));
        assert_eq!(d.number, 0);
        assert_eq!(d.screen, 0);
    }

    #[test]
    fn xquartz_path_with_screen_suffix() {
        let d = DisplaySpec::parse("/tmp/launchd/org.xquartz:0.0").unwrap();
        assert_eq!(d.target, unix("/tmp/launchd/org.xquartz:0"));
        assert_eq!(d.screen, 0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(DisplaySpec::parse("").is_none());
        assert!(DisplaySpec::parse("nocolon").is_none());
        assert!(DisplaySpec::parse(":notanumber").is_none());
        assert!(DisplaySpec::parse(":0.notascreen").is_none());
    }
}
