//! Docker sidebar tab messages.
//!
//! Every variant shares the `Docker` prefix the wrapper already supplies,
//! so the prefix is stripped (`clippy::enum_variant_names`), like the
//! `sync` / `player` / `tray` / `onboarding` / `tmux` domains.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum DockerMessage {
    /// List containers, images and compose projects on the pane's host.
    /// Fired when the tab becomes visible and from the Refresh action.
    Refresh(Uuid),
    /// A probe came back for that pane: the raw payload, or an error.
    Listed(Uuid, Result<String, String>),

    // ── Container actions ──
    /// Switch the active sub-panel. 0 = Containers, 1 = Images, 2 = Compose.
    SwitchPanel(Uuid, u8),
    /// Start a stopped container.
    StartContainer(Uuid, String),
    /// Stop a running container (reserved for direct actions).
    #[allow(dead_code)]
    StopContainer(Uuid, String),
    /// Ask to stop a container (parks confirmation).
    AskStop(Uuid, String),
    /// Run the parked stop.
    ConfirmStop(Uuid),
    /// Dismiss the stop confirmation.
    CancelStop(Uuid),
    /// Restart a container.
    RestartContainer(Uuid, String),
    /// Ask to remove a container (parks confirmation).
    AskRemove(Uuid, String),
    /// Run the parked remove.
    ConfirmRemove(Uuid),
    /// Dismiss the remove confirmation.
    CancelRemove(Uuid),
    /// A start/stop/restart/remove finished.
    ActionDone(Uuid, Result<(), String>),

    // ── Compose actions ──
    /// Run `docker compose up -d` for a compose project.
    ComposeUp(Uuid, String),
    /// Ask to bring down a compose project (parks confirmation).
    AskComposeDown(Uuid, String),
    /// Run the parked compose down.
    ConfirmComposeDown(Uuid),
    /// Dismiss the compose down confirmation.
    CancelComposeDown(Uuid),
    /// A compose action finished.
    ComposeActionDone(Uuid, Result<(), String>),

    // ── Filter ──
    /// Container filter text changed.
    ContainerFilterChanged(Uuid, String),
    /// Image filter text changed.
    ImageFilterChanged(Uuid, String),
}
