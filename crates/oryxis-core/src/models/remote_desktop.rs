//! Remote-desktop kind (RDP vs VNC) for a `RemoteDesktop` connection.
//!
//! A remote-desktop host is a first-class `Connection` whose `protocol`
//! is `RemoteDesktop`: `hostname`/`port` are the desktop endpoint,
//! `username`/`password` its login, and `rd_gateway_id` optionally
//! routes it through an SSH host (an ephemeral `-L` tunnel). This enum
//! only picks which client family to launch and the conventional port.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RemoteDesktopKind {
    #[default]
    Rdp,
    Vnc,
}

impl RemoteDesktopKind {
    /// The service's conventional port, pre-filled when the kind is
    /// chosen and the port field is still at the other kind's default.
    pub fn default_port(self) -> u16 {
        match self {
            RemoteDesktopKind::Rdp => 3389,
            RemoteDesktopKind::Vnc => 5900,
        }
    }
}

impl std::fmt::Display for RemoteDesktopKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RemoteDesktopKind::Rdp => "RDP",
            RemoteDesktopKind::Vnc => "VNC",
        })
    }
}
