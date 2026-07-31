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
    /// Expand / collapse the listening-ports list in the Monitor tab.
    TogglePorts,
    /// Expand / collapse the disk list in the Monitor tab (issue #83
    /// follow-up: many mounts crowd the sidebar).
    ToggleDisks,
    /// "Forward this port": prefill a `-L` rule for the monitored host
    /// and open the port-forward editor, so the user reviews and saves
    /// it instead of a tunnel appearing behind their back. The `Option`
    /// is the listener's bind address (`None` = wildcard): a service
    /// bound to a specific address only answers there, so it becomes
    /// the rule's target host instead of 127.0.0.1.
    ForwardPort(Uuid, u16, Option<String>),
    /// Right-click (or the Menu key) on a port row: open its actions
    /// popover. Carries the whole row so the menu and the confirmation
    /// it opens describe exactly the socket the user pointed at,
    /// instead of re-looking it up in a sample that may have rolled
    /// over in between.
    ShowPortMenu(Box<crate::monitor::model::PortStat>),
    /// "Kill process" / "Force kill" from that menu: park the
    /// confirmation. Nothing reaches the host until it is confirmed.
    AskKillPort(Box<crate::monitor::model::PortStat>, crate::monitor::kill::KillSignal),
    /// Run the parked kill.
    ConfirmKillPort,
    /// Re-run it escalated, offered after a failure sudo could fix.
    RetryKillWithSudo,
    /// Dismiss the confirmation without touching the host.
    CancelKillPort,
    /// A kill run came back. The `u64` is the monitor request stamp
    /// captured at dispatch: a mismatch means the pane reconnected (or
    /// monitoring was swept) while the run was in flight, so the result
    /// belongs to state that no longer exists.
    KillFinished(u64, crate::monitor::kill::KillOutcome),
}
