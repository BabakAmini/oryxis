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
    /// Open the card's kebab menu (Edit / Delete).
    ShowPortForwardMenu(usize),
    /// Ask first. Every affordance that removes a rule goes through
    /// here; `DeletePortForwardRule` is only ever reached by confirming.
    RequestDeletePortForwardRule(usize),
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
    /// The agent census the retry tick asked for (sorted
    /// `<endpoint> <fingerprint>` lines, see `oryxis_ssh::agent_key_census`).
    /// A reading that differs from the previous one means the keys moved,
    /// which makes every pending rule due immediately instead of waiting
    /// out a backoff that has already climbed to its ceiling.
    PortForwardAgentCensus(Vec<String>),
    PortForwardCardHovered(usize),
    PortForwardCardUnhovered,
    PortForwardSearchChanged(String),
}
