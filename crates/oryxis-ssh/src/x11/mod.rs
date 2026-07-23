//! X11 forwarding (`ForwardX11`).
//!
//! Requesting `x11-req` on the session channel makes the remote `sshd`
//! export a `DISPLAY` and open an `x11` channel back to us for every X
//! client the user launches. Each of those channels is bridged to the
//! local X server here.
//!
//! We implement TRUSTED forwarding (OpenSSH's `-Y`). Untrusted mode
//! (`-X`) relies on the X SECURITY extension, which denies the keyboard
//! and pointer grabs that Java/Swing toolkits need, so Oracle-style
//! enterprise GUIs fail to start under it. The cookie is still spoofed
//! (see [`spoof`]): the remote never learns the real one.

pub mod bridge;
pub mod display;
pub mod spoof;
pub mod xauth;

pub use display::{DisplaySpec, X11Target};

use std::sync::Arc;

/// A resolved local X11 endpoint, ready to serve forwarded channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11Forwarding {
    pub target: X11Target,
    pub screen: u32,
    /// Announced to the remote in `x11-req`. ALWAYS sent, even when the
    /// local display needs no authentication.
    ///
    /// Verified empirically against OpenSSH 9.x (`ssh -Y` with no
    /// `~/.Xauthority` logs "No xauth data; using fake authentication
    /// data"): sshd needs a cookie to write into the remote
    /// `.Xauthority`, and handing it an EMPTY auth protocol is not the
    /// same thing. Sending nothing here breaks unauthenticated displays
    /// (WSLg, VcXsrv `-ac`), which is the common Windows setup.
    pub fake_cookie: Vec<u8>,
    /// The local display's real cookie. `None` means the display accepts
    /// unauthenticated clients, and the per-channel swap then STRIPS the
    /// auth rather than substituting one.
    pub real_cookie: Option<Vec<u8>>,
}

impl X11Forwarding {
    /// Resolve the local display and its cookie.
    ///
    /// Returns `None` only when there is no display to forward to, which
    /// the caller treats as "skip the `x11-req`" rather than as a
    /// connection failure: a user with X11 enabled on a host they also
    /// open from a headless machine should still get a shell.
    pub fn resolve() -> Option<Self> {
        let spec = DisplaySpec::from_env()?;
        Some(Self {
            target: spec.target,
            screen: spec.screen,
            fake_cookie: spoof::random_cookie(),
            real_cookie: Self::resolve_real_cookie(spec.number),
        })
    }

    /// Read the local display's cookie. `None` (no authority file, no
    /// entry for this display, or an auth protocol we cannot speak) means
    /// the display is treated as open, and the per-channel swap strips
    /// the auth instead of substituting one.
    fn resolve_real_cookie(display_number: u32) -> Option<Vec<u8>> {
        let path = xauth::authority_path()?;
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(_) => {
                // Normal on Windows (VcXsrv writes no `.Xauthority`) and
                // under WSLg, whose X server has no access control.
                tracing::info!(
                    "X11 forward: {} is unreadable, treating the display as open",
                    path.display()
                );
                return None;
            }
        };
        let entries = xauth::parse(&raw);
        match xauth::select(&entries, display_number, xauth::hostname().as_deref()) {
            Some(entry) if entry.name == spoof::COOKIE_PROTO => Some(entry.data.clone()),
            Some(entry) => {
                // XDM-AUTHORIZATION-1 and friends are not something we
                // can substitute; say so rather than silently failing
                // every X client with an opaque auth rejection.
                tracing::warn!(
                    "X11 forward: display {display_number} uses unsupported auth {}, \
                     treating the display as open",
                    entry.name
                );
                None
            }
            None => {
                tracing::info!(
                    "X11 forward: no cookie for display {display_number}, \
                     treating the display as open"
                );
                None
            }
        }
    }

    /// One-line description of the resolved endpoint, for logs. X11 has
    /// many ways to half-work (wrong display, no cookie, no X server),
    /// and the failure the user sees is always the same opaque "cannot
    /// open display" on the REMOTE side, so the local log has to say
    /// exactly what was resolved.
    pub fn describe(&self) -> String {
        let where_ = match &self.target {
            X11Target::Unix(p) => format!("unix:{}", p.display()),
            X11Target::Tcp(h, p) => format!("tcp:{h}:{p}"),
        };
        let auth = match &self.real_cookie {
            Some(_) => "spoofing a real MIT-MAGIC-COOKIE-1",
            None => "open display (auth stripped on the way in)",
        };
        format!("{where_}, screen {}, {auth}", self.screen)
    }

    /// The `(protocol, cookie)` pair for `x11-req`.
    ///
    /// The cookie is LOWER-CASE HEX, not raw bytes: russh writes the
    /// string straight into the packet, and `sshd` feeds it verbatim to
    /// `xauth add` on the remote side.
    pub fn request_args(&self) -> (String, String) {
        (spoof::COOKIE_PROTO.to_string(), spoof::to_hex(&self.fake_cookie))
    }

    /// Spawn the bridge for one inbound X11 channel.
    pub(crate) fn spawn_bridge(
        self: &Arc<Self>,
        channel: russh::Channel<russh::client::Msg>,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) {
        let cfg = Arc::clone(self);
        tokio::spawn(async move {
            bridge::bridge_x11_channel(channel, cfg, cancel).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarding(real: Option<Vec<u8>>) -> X11Forwarding {
        X11Forwarding {
            target: X11Target::Tcp("localhost".into(), 6000),
            screen: 0,
            fake_cookie: vec![0xDE, 0xAD],
            real_cookie: real,
        }
    }

    #[test]
    fn request_args_carry_the_fake_cookie_in_hex() {
        let (proto, cookie) = forwarding(Some(vec![0xBE, 0xEF])).request_args();
        assert_eq!(proto, "MIT-MAGIC-COOKIE-1");
        // The FAKE cookie, never the real one, and hex-encoded because
        // russh writes the string into the packet untouched.
        assert_eq!(cookie, "dead");
    }

    /// An open local display still announces a cookie to the REMOTE.
    /// Verified against OpenSSH, which logs "No xauth data; using fake
    /// authentication data" and forwards successfully; announcing an
    /// empty auth instead leaves the remote with no usable `DISPLAY`.
    #[test]
    fn open_display_still_announces_a_fake_cookie() {
        let (proto, cookie) = forwarding(None).request_args();
        assert_eq!(proto, "MIT-MAGIC-COOKIE-1");
        assert_eq!(cookie, "dead");
    }
}
