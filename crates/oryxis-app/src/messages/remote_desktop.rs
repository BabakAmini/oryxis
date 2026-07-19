//! RDP/VNC-over-SSH launcher lifecycle, wrapped by [`crate::messages::Message::RemoteDesktop`]. Handled by `Oryxis::handle_remote_desktop`.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum RemoteDesktopMessage {
    /// Tunnel + client-spawn result: `Ok((session, local_port))` keeps
    /// the managed forward alive; `Err` is a ready-to-toast message. The
    /// `u64` is the launch generation, so a stale result from a superseded
    /// launch can't clobber a newer tunnel for the same host.
    RemoteDesktopReady(
        Uuid,
        u64,
        Result<(std::sync::Arc<oryxis_ssh::ForwardSession>, u16), String>,
    ),
    /// The ephemeral RDP/VNC tunnel closed on its own (desktop client
    /// disconnected and it went idle). Carries the launch generation so
    /// only the matching map entry is dropped.
    RemoteDesktopClientClosed(Uuid, u64),
    /// Tear down the RDP/VNC tunnel for this host connection id.
    StopRemoteDesktop(Uuid),
}
