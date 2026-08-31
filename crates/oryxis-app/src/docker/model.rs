//! Types behind the Docker manager.
//!
//! State is keyed by PANE, not by host: every pane owns the live SSH
//! session the listing is read over, so a pane that disconnects drops
//! exactly its own listing and two panes on the same host each answer
//! for the transport they actually hold.

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// One container as `docker ps` reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DockerContainer {
    /// Container name.
    pub name: String,
    /// Image used to create the container.
    pub image: String,
    /// Current status (e.g. "Up 3 hours", "Exited (0) 2 days ago").
    pub status: String,
    /// Container state: running, exited, paused, etc.
    pub state: String,
    /// Short container ID.
    pub id: String,
}

/// One image as `docker images` reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DockerImage {
    /// Repository name (e.g. "nginx", "myregistry/myapp").
    pub repository: String,
    /// Tag (e.g. "latest", "1.25").
    pub tag: String,
    /// Image size (e.g. "187MB").
    pub size: String,
    /// Short image ID.
    pub id: String,
}

/// A docker-compose project detected in the working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposeProject {
    /// Path to the compose file relative to cwd.
    pub file_path: String,
    /// Project name (directory name or explicit).
    pub project_name: String,
}

/// What the tab knows about one pane's Docker environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum DockerStatus {
    /// No probe has run yet for this pane.
    #[default]
    Idle,
    /// A probe is in flight.
    Loading,
    /// Docker is not installed on the host.
    NoDocker,
    /// Docker answered. Containers and images are populated.
    Ready(DockerData),
    /// The probe failed (transport died, timeout, refused shell).
    Failed(String),
}

/// The full Docker data snapshot for a pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DockerData {
    pub containers: Vec<DockerContainer>,
    pub images: Vec<DockerImage>,
    pub compose_projects: Vec<ComposeProject>,
}

/// Active sub-tab in the Docker panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DockerPanel {
    #[default]
    Containers,
    Images,
    Compose,
}

/// Per-pane Docker tab state: the data plus the small amount of form
/// state the tab carries.
#[derive(Debug, Clone, Default)]
pub(crate) struct PaneDocker {
    pub status: DockerStatus,
    /// Active sub-tab (Containers / Images / Compose).
    pub panel: DockerPanel,
    /// Container name awaiting stop confirmation.
    pub confirm_stop: Option<String>,
    /// Container name awaiting remove confirmation.
    pub confirm_remove: Option<String>,
    /// Compose project path awaiting down confirmation.
    pub confirm_compose_down: Option<String>,
    /// Inline error from the last action.
    pub error: Option<String>,
    /// Filter text for containers.
    pub container_filter: String,
    /// Filter text for images.
    pub image_filter: String,
}

/// Every pane's Docker state, plus the in-flight guard.
#[derive(Debug, Default)]
pub(crate) struct DockerState {
    panes: HashMap<Uuid, PaneDocker>,
    /// Panes with a probe in flight. A slow host is skipped rather than
    /// queueing probes behind each other, same rule as tmux.
    probing: HashSet<Uuid>,
}

impl DockerState {
    pub(crate) fn get(&self, pane_id: &Uuid) -> Option<&PaneDocker> {
        self.panes.get(pane_id)
    }

    pub(crate) fn entry(&mut self, pane_id: Uuid) -> &mut PaneDocker {
        self.panes.entry(pane_id).or_default()
    }

    /// Claim the probe slot for a pane. `false` means one is already in
    /// flight and this request should be dropped.
    pub(crate) fn begin_probe(&mut self, pane_id: Uuid) -> bool {
        self.probing.insert(pane_id)
    }

    pub(crate) fn end_probe(&mut self, pane_id: &Uuid) {
        self.probing.remove(pane_id);
    }

    /// Drop one pane's data (disconnect, pane closed).
    pub(crate) fn forget(&mut self, pane_id: &Uuid) {
        self.panes.remove(pane_id);
        self.probing.remove(pane_id);
    }

    /// Drop everything (feature turned off, vault locked).
    pub(crate) fn clear(&mut self) {
        self.panes.clear();
        self.probing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_slot_is_claimed_once() {
        let mut state = DockerState::default();
        let pane = Uuid::new_v4();
        assert!(state.begin_probe(pane));
        assert!(!state.begin_probe(pane));
        state.end_probe(&pane);
        assert!(state.begin_probe(pane));
    }

    #[test]
    fn forgetting_a_pane_releases_its_probe_slot() {
        let mut state = DockerState::default();
        let pane = Uuid::new_v4();
        state.begin_probe(pane);
        state.entry(pane).container_filter = "test".into();
        state.forget(&pane);
        assert!(state.get(&pane).is_none());
        assert!(state.begin_probe(pane));
    }
}
