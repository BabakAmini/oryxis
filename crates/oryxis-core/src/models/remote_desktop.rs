//! Per-host remote-desktop launch config (RDP / VNC over SSH).
//!
//! Not a protocol Oryxis speaks: it's a launcher. When set, the host
//! offers a one-click action that opens a `-L` tunnel through its SSH
//! connection to `target_host:target_port` and spawns the OS-native
//! desktop client (mstsc / FreeRDP / Remmina / Microsoft Remote
//! Desktop / a VNC viewer) pointed at the local end of the tunnel.
//!
//! `target_host` is resolved on the SSH *server* side, so the default
//! `localhost` means "the RDP/VNC service running on the box you SSH
//! into", the common case (tunnel to a machine's own desktop). A
//! different value reaches another host on the server's network.

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDesktopConfig {
    pub kind: RemoteDesktopKind,
    /// Destination reachable from the SSH server. `localhost` (default)
    /// targets the SSH host's own desktop service.
    pub target_host: String,
    /// Service port on `target_host` (3389 RDP / 5900 VNC by default).
    pub target_port: u16,
}

impl Default for RemoteDesktopConfig {
    fn default() -> Self {
        RemoteDesktopConfig {
            kind: RemoteDesktopKind::Rdp,
            target_host: "localhost".to_string(),
            target_port: RemoteDesktopKind::Rdp.default_port(),
        }
    }
}
