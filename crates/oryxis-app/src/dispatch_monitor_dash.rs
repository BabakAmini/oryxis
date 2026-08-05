//! `Oryxis::handle_monitor_dash`: the multi-host monitor dashboard
//! (issue #95).
//!
//! One link per opted-in host: a live terminal tab's session when one
//! exists, otherwise a headless probe-only [`oryxis_ssh::MonitorConn`]
//! dialed with the stored credentials (strict host key, TOTP autofill;
//! auth that would need an interactive answer fails onto the card, and
//! the card's open-terminal action is the interactive path out).
//! Samples land through the same `MonitorMessage::Sampled` handler the
//! sidebar uses, into the same rings, so the two surfaces can never
//! disagree. Polling only runs while the view is up; leaving it arms
//! an idle TTL that closes the dialed connections.

use iced::Task;
use uuid::Uuid;

use crate::app::{Message, MonitorMessage, Oryxis};
use crate::state::{DashLink, DashTransport};

/// Cap on a single dashboard probe, mirroring the sidebar's.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// How long dialed connections survive after the user leaves the view.
/// Long enough that a quick round-trip elsewhere doesn't redial the
/// fleet, short enough that closed dashboards don't hold idle logins.
const IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

impl Oryxis {
    pub(crate) fn handle_monitor_dash(
        &mut self,
        message: MonitorMessage,
    ) -> Result<Task<Message>, MonitorMessage> {
        match message {
            MonitorMessage::DashTick => Ok(self.dash_tick()),
            MonitorMessage::DashDialed(conn_id, stamp, result) => {
                // A sweep while the dial was in flight: the link map it
                // would land in no longer exists, so a successful dial
                // is closed instead of leaked.
                if stamp != self.monitor_dash.stamp {
                    if let Ok(conn) = &result {
                        conn.close();
                    }
                    return Ok(Task::none());
                }
                match result {
                    Ok(conn) => {
                        let transport = DashTransport::Pool(conn);
                        self.monitor_dash
                            .links
                            .insert(conn_id, DashLink::Live(transport.clone()));
                        // First sample now, so the card fills in instead
                        // of waiting out the stagger.
                        Ok(self.dash_probe(conn_id, transport))
                    }
                    Err(e) => {
                        self.monitor_dash.links.insert(conn_id, DashLink::Failed(e));
                        Ok(Task::none())
                    }
                }
            }
            MonitorMessage::DashRetry(conn_id) => {
                // Only a failed card offers the retry; a live one has
                // nothing to retry and a connecting one is already busy.
                if matches!(
                    self.monitor_dash.links.get(&conn_id),
                    Some(DashLink::Failed(_))
                ) {
                    self.monitor_dash.links.remove(&conn_id);
                    return Ok(self.dash_link(conn_id));
                }
                Ok(Task::none())
            }
            MonitorMessage::DashSweepDue(stamp) => {
                // Back on the view: the TTL did its job, keep the links.
                if self.active_view == crate::state::View::Monitoring {
                    return Ok(Task::none());
                }
                if stamp == self.monitor_dash.stamp {
                    self.monitor_dash.sweep();
                }
                Ok(Task::none())
            }
            MonitorMessage::DashSelectHost(conn_id) => {
                self.monitor_dash.selected = Some(conn_id);
                Ok(Task::none())
            }
            MonitorMessage::DashCloseDetail => {
                self.monitor_dash.selected = None;
                Ok(Task::none())
            }
            MonitorMessage::DashSearchChanged(s) => {
                self.monitor_dash.search = s;
                Ok(Task::none())
            }
            MonitorMessage::DashSortBy(key) => {
                if self.monitor_dash.sort_key == key {
                    self.monitor_dash.sort_asc = !self.monitor_dash.sort_asc;
                } else {
                    self.monitor_dash.sort_key = key;
                    // Metrics start descending (the hot host first is
                    // what a fleet sort is for); labels start A-z.
                    self.monitor_dash.sort_asc =
                        matches!(key, crate::state::DashSortKey::Label);
                }
                Ok(Task::none())
            }
            MonitorMessage::DashToggleListView => {
                self.prefs.monitor_dash_list_view = !self.prefs.monitor_dash_list_view;
                self.persist_setting(
                    "monitor_dash_list_view",
                    if self.prefs.monitor_dash_list_view { "true" } else { "false" },
                );
                Ok(Task::none())
            }
            MonitorMessage::DashOpenHost(conn_id) => {
                // An existing tab wins; otherwise the normal connect
                // flow (progress screen, prompts and all).
                if let Some(idx) = self.tab_index_for_host(conn_id) {
                    return Ok(Task::done(Message::Tabs(
                        crate::app::TabsMessage::SelectTab(idx),
                    )));
                }
                if let Some(idx) =
                    self.connections.iter().position(|c| c.id == conn_id)
                {
                    return Ok(Task::done(Message::Ssh(
                        crate::app::SshMessage::ConnectSsh(idx),
                    )));
                }
                Ok(Task::none())
            }
            m => Err(m),
        }
    }

    /// The opted-in fleet, sorted by label so the grid order (and the
    /// probe stagger derived from the position) is stable across
    /// re-renders and reboots.
    pub(crate) fn dash_hosts(&self) -> Vec<Uuid> {
        let mut hosts: Vec<(String, Uuid)> = self
            .connections
            .iter()
            .filter(|c| {
                self.prefs.monitor_all_hosts || c.monitor_enabled
            })
            .map(|c| (c.label.to_lowercase(), c.id))
            .collect();
        hosts.sort();
        hosts.into_iter().map(|(_, id)| id).collect()
    }

    /// One-second heartbeat while the view is up: prune hosts that
    /// opted out, establish missing links, redial dead ones, and probe
    /// each live link on its staggered slot.
    fn dash_tick(&mut self) -> Task<Message> {
        self.monitor_dash.tick = self.monitor_dash.tick.wrapping_add(1);
        let interval = self.monitor_interval_secs();
        let hosts = self.dash_hosts();

        // A host edited out of the fleet mid-session: close its dialed
        // connection and drop the card.
        let stale: Vec<Uuid> = self
            .monitor_dash
            .links
            .keys()
            .filter(|id| !hosts.contains(id))
            .copied()
            .collect();
        for id in stale {
            if let Some(DashLink::Live(t)) = self.monitor_dash.links.remove(&id) {
                t.close_pooled();
            }
            // The detail panel dies with its host's fleet membership.
            if self.monitor_dash.selected == Some(id) {
                self.monitor_dash.selected = None;
            }
        }

        let mut tasks: Vec<Task<Message>> = Vec::new();
        for (i, conn_id) in hosts.into_iter().enumerate() {
            match self.monitor_dash.links.get(&conn_id) {
                None => tasks.push(self.dash_link(conn_id)),
                Some(DashLink::Live(t)) if !t.is_alive() => {
                    // The link died (tab closed, network drop): one
                    // automatic re-establish. If the redial fails the
                    // card goes Failed and stays there (no hammering a
                    // down host every second).
                    t.close_pooled();
                    self.monitor_dash.links.remove(&conn_id);
                    tasks.push(self.dash_link(conn_id));
                }
                Some(DashLink::Live(t))
                    if (self.monitor_dash.tick + i as u64).is_multiple_of(interval) =>
                {
                    let t = t.clone();
                    tasks.push(self.dash_probe(conn_id, t));
                }
                _ => {}
            }
        }
        Task::batch(tasks)
    }

    /// Establish a host's link: borrow a live tab session when one
    /// exists (plus an immediate first sample), otherwise dial.
    fn dash_link(&mut self, conn_id: Uuid) -> Task<Message> {
        if let Some(session) = self.live_session_for_host(conn_id) {
            let transport = DashTransport::Tab(session);
            self.monitor_dash
                .links
                .insert(conn_id, DashLink::Live(transport.clone()));
            return self.dash_probe(conn_id, transport);
        }
        self.dash_dial(conn_id)
    }

    /// Headless dial, mirroring the port-forward auto-start path: the
    /// stored credentials and pinned settings apply, nothing prompts.
    fn dash_dial(&mut self, conn_id: Uuid) -> Task<Message> {
        let Some(mut conn) = self
            .connections
            .iter()
            .find(|c| c.id == conn_id)
            .cloned()
        else {
            return Task::none();
        };
        if let Some(vault) = self.vault.as_ref() {
            conn.proxy = vault.resolve_proxy(&conn).ok().flatten();
        }
        let (password, private_key, certificate) = self.resolve_forward_credentials(&conn);
        let pinned_agent = self.pinned_agent_public(&conn);
        let totp_secret = self
            .vault
            .as_ref()
            .and_then(|v| v.get_connection_totp_secret(&conn.id).ok().flatten());
        let resolver = self.build_jump_resolver(&conn);
        let host_key_check = self.build_host_key_check();
        let keepalive = self.effective_keepalive(&conn);

        self.monitor_dash.links.insert(conn_id, DashLink::Connecting);
        let stamp = self.monitor_dash.stamp;
        Task::perform(
            async move {
                let engine = oryxis_ssh::SshEngine::new()
                    .with_host_key_check(host_key_check)
                    .with_strict_host_key(true)
                    .with_totp_secret(totp_secret.as_deref())
                    .with_keepalive(keepalive)
                    .with_address_family(conn.address_family)
                    .with_rekey_limit_mb(conn.rekey_limit_mb)
                    .with_pinned_agent_key(pinned_agent.as_deref())
                    .with_algorithm_overrides(
                        conn.ciphers.clone(),
                        conn.kex.clone(),
                        conn.macs.clone(),
                        conn.host_key_algorithms.clone(),
                    );
                engine
                    .connect_monitor(
                        &conn,
                        password.as_deref(),
                        private_key.as_deref().map(|pem| {
                            oryxis_ssh::KeyMaterial::new(pem, certificate.as_deref())
                        }),
                        resolver.as_ref(),
                    )
                    .await
                    .map(std::sync::Arc::new)
                    .map_err(|e| e.to_string())
            },
            move |res| Message::Monitor(MonitorMessage::DashDialed(conn_id, stamp, res)),
        )
    }

    /// Probe one link. The in-flight guard, the stamp and the landing
    /// handler are the sidebar's own (`MonitorMessage::Sampled`), which
    /// is what keeps the two surfaces on identical data.
    fn dash_probe(&mut self, conn_id: Uuid, transport: DashTransport) -> Task<Message> {
        if !self.monitor.probing.insert(conn_id) {
            return Task::none();
        }
        let stamp = self.monitor_stamp;
        // Vitals only (owner call), EXCEPT the host whose detail panel
        // is open: its panel shows the sidebar's full presentation,
        // ports section included, so that one host pays for the full
        // probe. The unused slot stays in place either way (the parser
        // splits by position).
        let command = if self.monitor_dash.selected == Some(conn_id) {
            crate::monitor::probe::linux_probe_command()
        } else {
            crate::monitor::probe::linux_probe_command_vitals()
        };
        Task::perform(
            async move {
                let payload = match &transport {
                    DashTransport::Tab(s) => s.probe(&command, PROBE_TIMEOUT).await,
                    DashTransport::Pool(c) => c.probe(&command, PROBE_TIMEOUT).await,
                };
                match payload {
                    Some(payload) => Ok(payload),
                    None => Err(crate::i18n::t("monitor_probe_failed").to_string()),
                }
            },
            move |result| Message::Monitor(MonitorMessage::Sampled(conn_id, stamp, result)),
        )
    }

    /// Entering the Monitoring view: establish every link right away so
    /// the grid fills without waiting out the stagger.
    pub(crate) fn dash_enter(&mut self) -> Task<Message> {
        let hosts = self.dash_hosts();
        let mut tasks: Vec<Task<Message>> = Vec::new();
        for conn_id in hosts {
            match self.monitor_dash.links.get(&conn_id) {
                None => tasks.push(self.dash_link(conn_id)),
                // A quick round-trip elsewhere kept the links warm
                // (that is the idle TTL's point); refresh them now.
                Some(DashLink::Live(t)) if t.is_alive() => {
                    let t = t.clone();
                    tasks.push(self.dash_probe(conn_id, t));
                }
                _ => {}
            }
        }
        Task::batch(tasks)
    }

    /// Leaving the Monitoring view: arm the idle TTL that closes the
    /// dialed connections unless the user comes back first.
    pub(crate) fn dash_leave(&mut self) -> Task<Message> {
        if self.monitor_dash.links.is_empty() {
            return Task::none();
        }
        let stamp = self.monitor_dash.stamp;
        Task::perform(
            async move {
                tokio::time::sleep(IDLE_TTL).await;
            },
            move |_| Message::Monitor(MonitorMessage::DashSweepDue(stamp)),
        )
    }

    /// A live SSH session already connected to this host, from any tab
    /// and pane (not just the focused one, unlike the sidebar's
    /// `monitor_target`).
    fn live_session_for_host(
        &self,
        conn_id: Uuid,
    ) -> Option<std::sync::Arc<oryxis_ssh::SshSession>> {
        for tab in &self.tabs {
            for pane in tab.pane_grid.panes.values() {
                if matches!(pane.origin, crate::state::PaneOrigin::Host(id) if id == conn_id)
                    && let Some(ssh) = pane.session.as_ref().and_then(|s| s.ssh())
                    && ssh.is_alive()
                {
                    return Some(ssh.clone());
                }
            }
        }
        None
    }

    /// Index of a tab whose active pane belongs to this host, for the
    /// card's open-terminal action.
    fn tab_index_for_host(&self, conn_id: Uuid) -> Option<usize> {
        self.tabs.iter().position(|t| {
            t.pane_grid.panes.values().any(|p| {
                matches!(p.origin, crate::state::PaneOrigin::Host(id) if id == conn_id)
                    && p.session.as_ref().and_then(|s| s.ssh()).is_some_and(|s| s.is_alive())
            })
        })
    }
}
