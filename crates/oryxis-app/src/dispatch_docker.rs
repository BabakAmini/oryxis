//! Docker manager dispatch.
//!
//! All actions ride an exec channel multiplexed on the pane's live SSH
//! session, same as the tmux manager. The pane's CWD (from OSC 7) is
//! passed to the probe so docker-compose files can be detected.

use std::time::Duration;

use iced::Task;
use uuid::Uuid;

use crate::app::{DockerMessage, Message, Oryxis};
use crate::docker::model::DockerStatus;

/// A probe is a handful of short commands; a host that cannot answer
/// in this long is a host the tab should report on rather than wait for.
const DOCKER_TIMEOUT: Duration = Duration::from_secs(10);

impl Oryxis {
    pub(crate) fn handle_docker(&mut self, message: DockerMessage) -> Task<Message> {
        match message {
            DockerMessage::Refresh(pane_id) => self.docker_probe(pane_id),
            DockerMessage::Listed(pane_id, result) => {
                self.docker.end_probe(&pane_id);
                if self.docker.get(&pane_id).is_none() {
                    return Task::none();
                }
                let entry = self.docker.entry(pane_id);
                entry.status = match result {
                    Ok(ref payload) => {
                        match crate::docker::probe::parse_probe(payload) {
                            crate::docker::probe::ProbeResult::NoDocker => DockerStatus::NoDocker,
                            crate::docker::probe::ProbeResult::NoDaemon => {
                                DockerStatus::Failed(crate::i18n::t("docker_no_daemon").to_string())
                            }
                            crate::docker::probe::ProbeResult::Data(data) => {
                                DockerStatus::Ready(data)
                            }
                        }
                    }
                    Err(e) => DockerStatus::Failed(e),
                };
                Task::none()
            }
            DockerMessage::SwitchPanel(pane_id, code) => {
                use crate::docker::model::DockerPanel;
                let panel = match code {
                    0 => DockerPanel::Containers,
                    1 => DockerPanel::Images,
                    2 => DockerPanel::Compose,
                    _ => DockerPanel::Containers,
                };
                self.docker.entry(pane_id).panel = panel;
                Task::none()
            }
            DockerMessage::StartContainer(pane_id, name) => {
                self.docker_container_action(pane_id, &name, "start")
            }
            DockerMessage::AskStop(pane_id, name) => {
                let entry = self.docker.entry(pane_id);
                entry.confirm_stop = Some(name);
                entry.error = None;
                Task::none()
            }
            DockerMessage::StopContainer(pane_id, name) => {
                self.docker_container_action(pane_id, &name, "stop")
            }
            DockerMessage::ConfirmStop(pane_id) => {
                let name = self.docker.entry(pane_id).confirm_stop.take();
                match name {
                    Some(name) => self.docker_container_action(pane_id, &name, "stop"),
                    None => Task::none(),
                }
            }
            DockerMessage::CancelStop(pane_id) => {
                self.docker.entry(pane_id).confirm_stop = None;
                Task::none()
            }
            DockerMessage::RestartContainer(pane_id, name) => {
                self.docker_container_action(pane_id, &name, "restart")
            }
            DockerMessage::AskRemove(pane_id, name) => {
                let entry = self.docker.entry(pane_id);
                entry.confirm_remove = Some(name);
                entry.error = None;
                Task::none()
            }
            DockerMessage::ConfirmRemove(pane_id) => {
                let name = self.docker.entry(pane_id).confirm_remove.take();
                match name {
                    Some(name) => self.docker_container_action(pane_id, &name, "rm"),
                    None => Task::none(),
                }
            }
            DockerMessage::CancelRemove(pane_id) => {
                self.docker.entry(pane_id).confirm_remove = None;
                Task::none()
            }
            DockerMessage::ActionDone(pane_id, result) => {
                if self.docker.get(&pane_id).is_none() {
                    return Task::none();
                }
                match result {
                    Ok(()) => {
                        self.docker.entry(pane_id).error = None;
                        self.docker_probe(pane_id)
                    }
                    Err(e) => {
                        self.docker.entry(pane_id).error = Some(e);
                        Task::none()
                    }
                }
            }
            DockerMessage::ComposeUp(pane_id, path) => {
                self.docker_compose_action(pane_id, &path, "up")
            }
            DockerMessage::AskComposeDown(pane_id, path) => {
                let entry = self.docker.entry(pane_id);
                entry.confirm_compose_down = Some(path);
                entry.error = None;
                Task::none()
            }
            DockerMessage::ConfirmComposeDown(pane_id) => {
                let path = self.docker.entry(pane_id).confirm_compose_down.take();
                match path {
                    Some(path) => self.docker_compose_action(pane_id, &path, "down"),
                    None => Task::none(),
                }
            }
            DockerMessage::CancelComposeDown(pane_id) => {
                self.docker.entry(pane_id).confirm_compose_down = None;
                Task::none()
            }
            DockerMessage::ComposeActionDone(pane_id, result) => {
                if self.docker.get(&pane_id).is_none() {
                    return Task::none();
                }
                match result {
                    Ok(()) => {
                        self.docker.entry(pane_id).error = None;
                        self.docker_probe(pane_id)
                    }
                    Err(e) => {
                        self.docker.entry(pane_id).error = Some(e);
                        Task::none()
                    }
                }
            }
            DockerMessage::ContainerFilterChanged(pane_id, v) => {
                self.docker.entry(pane_id).container_filter = v;
                Task::none()
            }
            DockerMessage::ImageFilterChanged(pane_id, v) => {
                self.docker.entry(pane_id).image_filter = v;
                Task::none()
            }
        }
    }

    /// Run the Docker probe for a pane, if it holds a live SSH session.
    fn docker_probe(&mut self, pane_id: Uuid) -> Task<Message> {
        let Some(session) = self.docker_session_for_pane(pane_id) else {
            return Task::none();
        };
        if !self.docker.begin_probe(pane_id) {
            return Task::none();
        }
        let entry = self.docker.entry(pane_id);
        if matches!(entry.status, DockerStatus::Idle) {
            entry.status = DockerStatus::Loading;
        }
        // Read the pane's CWD for compose file detection.
        let cwd = self
            .pane_by_id(pane_id)
            .and_then(|p| p.cwd.clone());
        let command = crate::docker::probe::probe_command(cwd.as_deref());
        Task::perform(
            async move {
                match session.probe(&command, DOCKER_TIMEOUT).await {
                    Some(payload) => Ok(payload),
                    None => Err(crate::i18n::t("docker_probe_failed").to_string()),
                }
            },
            move |result| Message::Docker(DockerMessage::Listed(pane_id, result)),
        )
    }

    /// Run a container management command (start/stop/restart/rm).
    fn docker_container_action(
        &mut self,
        pane_id: Uuid,
        name: &str,
        action: &str,
    ) -> Task<Message> {
        let Some(session) = self.docker_session_for_pane(pane_id) else {
            return Task::none();
        };
        let command = match action {
            "start" => crate::docker::probe::start_container_command(name),
            "stop" => crate::docker::probe::stop_container_command(name),
            "restart" => crate::docker::probe::restart_container_command(name),
            "rm" => crate::docker::probe::remove_container_command(name),
            _ => return Task::none(),
        };
        let command = match command {
            Ok(cmd) => cmd,
            Err(e) => {
                self.docker.entry(pane_id).error = Some(e.to_string());
                return Task::none();
            }
        };
        Task::perform(
            async move {
                match session.probe(&command, DOCKER_TIMEOUT).await {
                    Some(output) if output.trim().is_empty() => Ok(()),
                    Some(output) => {
                        let trimmed = output.trim().to_string();
                        if trimmed.contains("Error") || trimmed.contains("error") {
                            Err(trimmed)
                        } else {
                            Ok(())
                        }
                    }
                    None => Err(crate::i18n::t("docker_action_failed").to_string()),
                }
            },
            move |result| Message::Docker(DockerMessage::ActionDone(pane_id, result)),
        )
    }

    /// Run a compose management command (up/down).
    fn docker_compose_action(
        &mut self,
        pane_id: Uuid,
        path: &str,
        action: &str,
    ) -> Task<Message> {
        let Some(session) = self.docker_session_for_pane(pane_id) else {
            return Task::none();
        };
        let command = match action {
            "up" => crate::docker::probe::compose_up_command(path),
            "down" => crate::docker::probe::compose_down_command(path),
            _ => return Task::none(),
        };
        let command = match command {
            Ok(cmd) => cmd,
            Err(e) => {
                self.docker.entry(pane_id).error = Some(e.to_string());
                return Task::none();
            }
        };
        Task::perform(
            async move {
                match session.probe(&command, Duration::from_secs(30)).await {
                    Some(output) if output.trim().is_empty() => Ok(()),
                    Some(output) => {
                        let trimmed = output.trim().to_string();
                        if trimmed.to_lowercase().contains("error") {
                            Err(trimmed)
                        } else {
                            Ok(())
                        }
                    }
                    None => Err(crate::i18n::t("docker_action_failed").to_string()),
                }
            },
            move |result| Message::Docker(DockerMessage::ComposeActionDone(pane_id, result)),
        )
    }

    /// The live SSH session behind a pane, when the feature is on and
    /// the transport is still up. `None` for local / serial / telnet
    /// panes and for dead sessions.
    pub(crate) fn docker_session_for_pane(
        &self,
        pane_id: Uuid,
    ) -> Option<std::sync::Arc<oryxis_ssh::SshSession>> {
        if !self.prefs.docker_manager {
            return None;
        }
        let pane = self.pane_by_id(pane_id)?;
        let ssh = pane.session.as_ref().and_then(|s| s.ssh())?;
        ssh.is_alive().then(|| ssh.clone())
    }

    /// True while the Docker tab is the visible sidebar tab.
    pub(crate) fn docker_tab_visible(&self) -> bool {
        self.sidebar_tab_shown(crate::state::TerminalSidebarTab::Docker)
    }

    /// Idempotent "the Docker tab is on screen for this pane" sync.
    pub(crate) fn docker_sync(&mut self) -> Task<Message> {
        if !self.docker_tab_visible() {
            return Task::none();
        }
        let Some(pane_id) = self.active_pane_mut().map(|p| p.id) else {
            return Task::none();
        };
        if self.docker_session_for_pane(pane_id).is_none() {
            return Task::none();
        }
        self.docker.entry(pane_id);
        self.docker_probe(pane_id)
    }

    /// Drop a pane's data on disconnect / close.
    pub(crate) fn docker_reset_pane(&mut self, pane_id: &Uuid) {
        self.docker.forget(pane_id);
    }

    /// Drop every listing (feature turned off, vault locked).
    pub(crate) fn docker_reset_all(&mut self) {
        self.docker.clear();
    }
}
