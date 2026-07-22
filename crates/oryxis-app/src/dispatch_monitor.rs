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

/// Localized toast copy for a crossed threshold.
fn breach_message(host: &str, breach: &crate::monitor::alert::Breach) -> String {
    use crate::monitor::alert::Breach;
    let key = match breach {
        Breach::Cpu => "monitor_alert_cpu",
        Breach::Mem => "monitor_alert_mem",
        Breach::Disk(_) => "monitor_alert_disk",
    };
    let text = crate::i18n::t(key).replacen("{host}", host, 1);
    match breach {
        Breach::Disk(mount) => text.replacen("{mount}", mount, 1),
        _ => text,
    }
}

/// Cap on a single probe. Long enough for a loaded host to answer, short
/// enough that a wedged one frees its in-flight slot before the user
/// gives up on the tab.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Default seconds between polls. Frequent enough to feel live, sparse
/// enough that one exec channel per interval is negligible next to an
/// interactive shell.
pub(crate) const MONITOR_INTERVAL_DEFAULT_SECS: u64 = 5;

/// Floor on the configured interval. Below this the probes start
/// overlapping their own round trips on a slow link, which costs the
/// host more than the readings are worth.
const MONITOR_INTERVAL_FLOOR_SECS: u64 = 2;

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
                        // Threshold check on the fresh window. Rising
                        // edge only, so a pegged host is announced once
                        // per crossing; foreground toasts by owner
                        // constraint, never background alerting.
                        let recent = series.tail(3);
                        let (flags, breaches) =
                            crate::monitor::alert::evaluate(&recent, series.breached);
                        series.breached = flags;
                        if !breaches.is_empty() {
                            let host = self
                                .connections
                                .iter()
                                .find(|c| c.id == conn_id)
                                .map(|c| c.label.clone())
                                .unwrap_or_default();
                            let mut tasks: Vec<Task<Message>> = Vec::new();
                            for b in breaches {
                                tasks.push(self.show_toast_secs(breach_message(&host, &b), 8));
                            }
                            self.monitor_error = None;
                            return Task::batch(tasks);
                        }
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
                    // A failed persist must be loud: the flag would work
                    // until restart and then silently vanish.
                    if let Some(vault) = &self.vault
                        && let Err(e) = vault.save_connection(&conn, None)
                    {
                        return self.show_toast_secs(e.to_string(), 6);
                    }
                    // Probe immediately so the tab fills in instead of
                    // waiting out a whole interval on an empty card.
                    return self.monitor_probe_active_pane();
                }
                Task::none()
            }
            MonitorMessage::TogglePorts => {
                self.monitor_ports_open = !self.monitor_ports_open;
                Task::none()
            }
            MonitorMessage::ToggleDisks => {
                self.monitor_disks_open = !self.monitor_disks_open;
                Task::none()
            }
            MonitorMessage::ForwardPort(conn_id, port, bind) => {
                // Prefill a local forward onto the same port and hand the
                // user the editor. The target is dialed FROM THE SERVER:
                // a wildcard or loopback listener answers on 127.0.0.1,
                // but one bound to a specific address only answers THERE,
                // so that address becomes the target instead of a
                // 127.0.0.1 that would dial a closed port.
                let target = match bind.as_deref() {
                    Some(addr) if addr != "127.0.0.1" && addr != "::1" => addr.to_string(),
                    _ => "127.0.0.1".to_string(),
                };
                let label = self
                    .connections
                    .iter()
                    .find(|c| c.id == conn_id)
                    .map(|c| format!("{} :{port}", c.label))
                    .unwrap_or_else(|| format!(":{port}"));
                self.show_port_forward_panel = true;
                self.port_forward_form.editing_id = None;
                self.port_forward_form.label = label;
                self.port_forward_form.kind =
                    oryxis_core::models::port_forward_rule::ForwardKind::Local;
                self.port_forward_form.host_id = Some(conn_id);
                self.port_forward_form.listen_host = "127.0.0.1".into();
                self.port_forward_form.listen_port = port.to_string();
                self.port_forward_form.target_host = target;
                self.port_forward_form.target_port = port.to_string();
                self.port_forward_form.auto_start = false;
                self.port_forward_form.error = None;
                // The editor lives in the Port Forwarding view, so the
                // click navigates there: the rule is reviewed and saved
                // deliberately, never created silently.
                Task::done(Message::Navigation(
                    crate::app::NavigationMessage::ChangeView(
                        crate::state::View::PortForwarding,
                    ),
                ))
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
        if !self.setting_host_monitoring {
            return None;
        }
        let conn_id = self.monitor_pane_connection()?;
        if !self.monitor_host_opted_in(&conn_id) {
            return None;
        }
        let idx = self.active_tab?;
        let pane = self.tabs.get(idx)?.active();
        let ssh = pane.session.as_ref().and_then(|s| s.ssh())?;
        ssh.is_alive().then(|| (conn_id, ssh.clone()))
    }

    /// Effective monitoring opt-in for a host: the global "all hosts"
    /// toggle OR the per-host flag. Shared by the probe target and the
    /// status-bar segment, so switching a host's flag off stops the
    /// RENDER as well as the probing (a lingering series must not keep
    /// painting frozen vitals as if they were live).
    pub(crate) fn monitor_host_opted_in(&self, conn_id: &Uuid) -> bool {
        self.setting_monitor_all_hosts
            || self
                .connections
                .iter()
                .any(|c| c.id == *conn_id && c.monitor_enabled)
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

    /// Effective probe interval: the configured value, floored so a
    /// typo (or an empty field mid-edit) can't hammer the host.
    pub(crate) fn monitor_interval_secs(&self) -> u64 {
        self.setting_monitor_interval
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|s| *s > 0)
            .unwrap_or(MONITOR_INTERVAL_DEFAULT_SECS)
            .max(MONITOR_INTERVAL_FLOOR_SECS)
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

    /// Drop EVERY host's window and invalidate all in-flight probes.
    /// Used by the feature toggle-off and the vault-lock sweeps: without
    /// the stamp bump, a probe already in flight would land after the
    /// sweep and repopulate the state it just cleared (and could fire a
    /// first-sample threshold toast right after the user turned the
    /// feature off).
    pub(crate) fn monitor_reset_all(&mut self) {
        self.monitor = Default::default();
        self.monitor_stamp = self.monitor_stamp.wrapping_add(1);
        self.monitor_error = None;
    }
}

#[cfg(test)]
mod tests {
    /// The interval resolver's contract, exercised without an `Oryxis`
    /// (the parse + floor is the whole rule; the struct only supplies
    /// the string).
    fn resolve(raw: &str) -> u64 {
        raw.trim()
            .parse::<u64>()
            .ok()
            .filter(|s| *s > 0)
            .unwrap_or(super::MONITOR_INTERVAL_DEFAULT_SECS)
            .max(2)
    }

    #[test]
    fn interval_falls_back_and_floors() {
        assert_eq!(resolve("10"), 10);
        // Empty / half-typed / non-numeric fall back to the default
        // rather than freezing the tick at zero.
        assert_eq!(resolve(""), super::MONITOR_INTERVAL_DEFAULT_SECS);
        assert_eq!(resolve("   "), super::MONITOR_INTERVAL_DEFAULT_SECS);
        assert_eq!(resolve("abc"), super::MONITOR_INTERVAL_DEFAULT_SECS);
        // "0" would be a busy loop against the host; the floor catches
        // it and every sub-floor value.
        assert_eq!(resolve("0"), super::MONITOR_INTERVAL_DEFAULT_SECS);
        assert_eq!(resolve("1"), 2);
        assert_eq!(resolve("2"), 2);
    }
}
