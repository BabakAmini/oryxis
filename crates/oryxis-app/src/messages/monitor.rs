//! Sidebar Monitor tab (agentless host vitals) messages, issue #83.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum MonitorMessage {
    /// Periodic poll: probe every monitored host that has a live session
    /// and an open Monitor tab. Mounted only while such a tab is
    /// visible, so idle screens never touch the network.
    Tick,
    /// A probe returned: the raw batched payload for that connection, or
    /// an error to surface on the card. The `u64` is the request stamp
    /// captured at dispatch; a mismatch means the pane reconnected (or
    /// monitoring was turned off) while the probe was in flight and the
    /// result is dropped.
    Sampled(Uuid, u64, Result<String, String>),
    /// Turn monitoring on for the host behind the focused pane, from the
    /// Monitor tab's own opt-in prompt (the same flag the host editor
    /// toggles).
    EnableHost(Uuid),
}
