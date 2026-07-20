//! `Oryxis::handle_monitor`: the sidebar Monitor tab's polling loop
//! (issue #83, plan J2).
//!
//! Probes run on an exec channel multiplexed on the focused pane's LIVE
//! SSH session, so no extra connection is opened and the host is only
//! ever read from. Parsing lives in `crate::monitor::probe`, which is
//! pure and unit-tested; this file owns the scheduling, the in-flight
//! guard and the stale-result rules.

use iced::Task;
use uuid::Uuid;

use crate::app::{Message, MonitorMessage, Oryxis};

/// Cap on a single probe. Long enough for a loaded host to answer, short
/// enough that a wedged one frees its in-flight slot before the user
/// gives up on the tab.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Seconds between polls while the Monitor tab is open. Frequent enough
/// to feel live, sparse enough that one exec channel per interval is
/// negligible next to an interactive shell.
pub(crate) const MONITOR_INTERVAL_SECS: u64 = 5;

impl Oryxis {
    pub(crate) fn handle_monitor(&mut self, message: MonitorMessage) -> Task<Message> {
        match message {
            MonitorMessage::Tick => self.monitor_probe_active_pane(),
            MonitorMessage::Sampled(conn_id, stamp, result) => {
                self.monitor.probing.remove(&conn_id);
                // A reconnect (or monitoring turned off) while the probe
                // was in flight bumps the stamp; that result belongs to a
                // series that no longer exists.
                if stamp != self.monitor_stamp {
                    return Task::none();
                }
                match result {
                    Ok(payload) => {
                        let series = self.monitor.series.entry(conn_id).or_default();
                        let (sample, snapshot) = crate::monitor::probe::parse_linux(
                            &payload,
                            series.raw_prev,
                            std::time::Instant::now(),
                        );
                        series.push(sample, snapshot);
                    }
                    Err(e) => {
                        // Keep whatever the window already holds (the last
                        // good reading stays on screen) and surface the
                        // failure; the next tick retries.
                        self.monitor_error = Some(e);
                        return Task::none();
                    }
                }
                self.monitor_error = None;
                Task::none()
            }
            MonitorMessage::EnableHost(conn_id) => {
                if let Some(conn) = self.connections.iter_mut().find(|c| c.id == conn_id) {
                    conn.monitor_enabled = true;
                    let conn = conn.clone();
                    if let Some(vault) = &self.vault {
                        let _ = vault.save_connection(&conn, None);
                    }
                    // Probe immediately so the tab fills in instead of
                    // waiting out a whole interval on an empty card.
                    return self.monitor_probe_active_pane();
                }
                Task::none()
            }
        }
    }

    /// Probe the focused pane's host, when it is monitored, connected and
    /// not already being probed. Called from the tick and right after the
    /// user opts a host in.
    fn monitor_probe_active_pane(&mut self) -> Task<Message> {
        let Some((conn_id, session)) = self.monitor_target() else {
            return Task::none();
        };
        // A slow host is skipped rather than queueing probes behind each
        // other: the previous one is still holding a channel.
        if !self.monitor.probing.insert(conn_id) {
            return Task::none();
        }
        let stamp = self.monitor_stamp;
        let command = crate::monitor::probe::linux_probe_command();
        Task::perform(
            async move {
                match session.probe(&command, PROBE_TIMEOUT).await {
                    Some(payload) => Ok(payload),
                    None => Err(crate::i18n::t("monitor_probe_failed").to_string()),
                }
            },
            move |result| Message::Monitor(MonitorMessage::Sampled(conn_id, stamp, result)),
        )
    }

    /// The focused pane's `(connection id, live session)` when that host
    /// has monitoring enabled. `None` for local / ephemeral panes, hosts
    /// that never opted in, and dead sessions.
    pub(crate) fn monitor_target(&self) -> Option<(Uuid, std::sync::Arc<oryxis_ssh::SshSession>)> {
        let conn_id = self.monitor_pane_connection()?;
        if !self.connections.iter().any(|c| c.id == conn_id && c.monitor_enabled) {
            return None;
        }
        let idx = self.active_tab?;
        let pane = self.tabs.get(idx)?.active();
        let ssh = pane.session.as_ref().and_then(|s| s.ssh())?;
        ssh.is_alive().then(|| (conn_id, ssh.clone()))
    }

    /// Connection id behind the focused pane, if it is a saved host.
    /// Quick-connect / local / cloud panes have no vault row to carry the
    /// opt-in flag, so they can't be monitored.
    pub(crate) fn monitor_pane_connection(&self) -> Option<Uuid> {
        let idx = self.active_tab?;
        match self.tabs.get(idx)?.active().origin {
            crate::state::PaneOrigin::Host(id) => Some(id),
            _ => None,
        }
    }

    /// True while the Monitor tab is the visible sidebar tab, which is
    /// what mounts the tick: monitoring never polls a screen nobody is
    /// looking at.
    pub(crate) fn monitor_tab_visible(&self) -> bool {
        self.effective_sidebar_tab() == Some(crate::state::TerminalSidebarTab::Monitor)
    }

    /// Drop a host's window (disconnect, monitoring turned off, lock) and
    /// invalidate any probe still in flight for it.
    pub(crate) fn monitor_reset_host(&mut self, conn_id: &Uuid) {
        self.monitor.forget(conn_id);
        self.monitor_stamp = self.monitor_stamp.wrapping_add(1);
        self.monitor_error = None;
    }
}
