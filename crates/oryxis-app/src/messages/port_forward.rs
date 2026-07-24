//! Standalone port-forward rule entity: CRUD, the editor panel and
//! tunnel start/stop lifecycle, wrapped by [`crate::messages::Message::PortForward`].

use std::sync::Arc;
use uuid::Uuid;
use oryxis_ssh::{ForwardSession};
use oryxis_core::models::port_forward_rule::ForwardKind;

#[derive(Debug, Clone)]
pub enum PortForwardMessage {
    ShowPortForwardPanel,
    HidePortForwardPanel,
    PfLabelChanged(String),
    PfKindChanged(ForwardKind),
    PfHostChanged(Uuid),
    PfListenHostChanged(String),
    PfListenPortChanged(String),
    PfTargetHostChanged(String),
    PfTargetPortChanged(String),
    PfAutoStartToggled(bool),
    SavePortForwardRule,
    EditPortForwardRule(usize),
    DeletePortForwardRule(usize),
    /// Toggle a rule on: opens a dedicated PTY-less SSH session.
    StartPortForward(Uuid),
    /// Toggle a rule off: drops its `ForwardSession` (cancels the tunnel).
    StopPortForward(Uuid),
    /// Result of a `StartPortForward` connect attempt.
    PortForwardStarted(Uuid, Result<Arc<ForwardSession>, String>),
    /// Periodic liveness sweep; drops forwards whose connection died.
    PortForwardLivenessTick,
    /// Periodic sweep that re-attempts `auto_start` rules that failed to
    /// come up (or dropped): self-heals the KeePassXC-key-not-ready and
    /// network-loss cases with a capped exponential backoff.
    PortForwardRetryTick,
    PortForwardCardHovered(usize),
    PortForwardCardUnhovered,
    PortForwardSearchChanged(String),
}
