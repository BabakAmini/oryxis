//! In-memory sample ring for the host monitor (issue #83, plan J2).
//!
//! One `HostSeries` per monitored MACHINE (issue #156: keyed by
//! [`MonitorKey`], not by vault row, so the rows that point at one
//! server share a window instead of each sampling it on its own slot),
//! holding a bounded window of recent samples (for the sparkline) plus
//! the raw counters the next tick diffs against. Nothing is persisted:
//! the whole thing is dropped on disconnect and on vault lock.

use std::collections::{HashMap, HashSet, VecDeque};

use super::endpoint::MonitorKey;
use super::model::{RawSnapshot, Sample};

/// How many samples a host keeps. At the 5 s default interval this is
/// ten minutes of history, enough for a meaningful sparkline without
/// holding samples nobody looks at.
pub(crate) const SERIES_CAP: usize = 120;

/// A single host's rolling window.
#[derive(Debug, Default)]
pub(crate) struct HostSeries {
    pub samples: VecDeque<Sample>,
    /// Counters from the last probe, the baseline for the next tick's
    /// CPU / network rates.
    pub raw_prev: Option<RawSnapshot>,
    /// Which thresholds this host is currently over, so an alert fires
    /// once per crossing instead of once per tick.
    pub breached: super::alert::BreachFlags,
}

impl HostSeries {
    /// Append a sample, dropping the oldest once the window is full.
    pub fn push(&mut self, sample: Sample, snapshot: RawSnapshot) {
        if self.samples.len() >= SERIES_CAP {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        self.raw_prev = Some(snapshot);
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.samples.back()
    }

    /// The newest `n` samples, oldest first: what the threshold check
    /// needs to see a sustained breach rather than a single spike.
    pub fn tail(&self, n: usize) -> Vec<&Sample> {
        let skip = self.samples.len().saturating_sub(n);
        self.samples.iter().skip(skip).collect()
    }

    /// CPU percentages over the window, oldest first, for the sparkline.
    /// Samples without a reading (the first one after mount) are skipped
    /// so the line starts where real data does.
    pub fn cpu_series(&self) -> Vec<f32> {
        self.samples.iter().filter_map(|s| s.cpu.map(|c| c.pct)).collect()
    }
}

/// Monitor state hanging off the app: one series per monitored machine
/// plus the in-flight guard.
#[derive(Debug, Default)]
pub(crate) struct MonitorState {
    pub series: HashMap<MonitorKey, HostSeries>,
    /// Machines with a probe already in flight. A slow host is skipped
    /// on the next tick instead of queueing probes behind each other,
    /// and since the key is the machine, the sidebar and the dashboard
    /// can't probe one server twice in the same interval either.
    pub probing: HashSet<MonitorKey>,
    /// The open "kill the process on this port" confirmation (issue
    /// #96). Lives here so every monitor sweep drops it: a dialog about
    /// a host that just disconnected (or a vault that just locked) must
    /// not survive to signal anything.
    pub kill: Option<super::kill::PendingKill>,
}

impl MonitorState {
    /// Drop a machine's window entirely (disconnect, monitoring turned
    /// off, a disk selection edited). `conn_id` is the row the reset
    /// came from: the window belongs to the machine, the kill dialog to
    /// the row.
    pub fn forget(&mut self, key: &MonitorKey, conn_id: &uuid::Uuid) {
        self.series.remove(key);
        self.probing.remove(key);
        if self.kill.as_ref().is_some_and(|k| k.conn_id == *conn_id) {
            self.kill = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::model::{CpuStat, Sample};
    use std::time::Instant;

    fn sample_with(pct: Option<f32>, at: Instant) -> (Sample, RawSnapshot) {
        (
            Sample {
                at,
                cpu: pct.map(|pct| CpuStat { pct }),
                mem: None,
                load: None,
                net: None,
                disks: Vec::new(),
                gpus: Vec::new(),
                ports: Vec::new(),
                uptime_secs: None,
            },
            RawSnapshot { cpu_total: 0, cpu_idle: 0, net_rx: 0, net_tx: 0, at },
        )
    }

    #[test]
    fn ring_drops_the_oldest_past_the_cap() {
        let mut series = HostSeries::default();
        let now = Instant::now();
        for i in 0..(SERIES_CAP + 10) {
            let (s, snap) = sample_with(Some(i as f32), now);
            series.push(s, snap);
        }
        assert_eq!(series.samples.len(), SERIES_CAP);
        // The first 10 fell off the front.
        assert_eq!(series.samples.front().unwrap().cpu.unwrap().pct, 10.0);
        assert_eq!(
            series.latest().unwrap().cpu.unwrap().pct,
            (SERIES_CAP + 9) as f32
        );
    }

    #[test]
    fn cpu_series_skips_samples_without_a_reading() {
        let mut series = HostSeries::default();
        let now = Instant::now();
        // The first sample after mount has no baseline, so no CPU%.
        let (s, snap) = sample_with(None, now);
        series.push(s, snap);
        let (s, snap) = sample_with(Some(42.0), now);
        series.push(s, snap);
        assert_eq!(series.cpu_series(), vec![42.0]);
        assert!(series.raw_prev.is_some());
    }

    fn key(host: &str) -> MonitorKey {
        MonitorKey::new(&oryxis_core::models::Connection::new("label", host))
    }

    #[test]
    fn forget_clears_both_the_window_and_the_in_flight_guard() {
        let mut state = MonitorState::default();
        let id = uuid::Uuid::new_v4();
        let k = key("srv.example");
        let (s, snap) = sample_with(Some(1.0), Instant::now());
        state.series.entry(k.clone()).or_default().push(s, snap);
        state.probing.insert(k.clone());
        state.forget(&k, &id);
        assert!(state.series.is_empty());
        assert!(state.probing.is_empty());
    }

    /// The window is keyed on the machine, so a reset from one row
    /// leaves another machine's window alone (issue #156).
    #[test]
    fn forget_drops_only_its_own_machines_window() {
        let mut state = MonitorState::default();
        let (mine, other) = (key("srv.example"), key("other.example"));
        for k in [&mine, &other] {
            let (s, snap) = sample_with(Some(1.0), Instant::now());
            state.series.entry(k.clone()).or_default().push(s, snap);
        }
        state.forget(&mine, &uuid::Uuid::new_v4());
        assert!(!state.series.contains_key(&mine));
        assert!(state.series.contains_key(&other));
    }

    #[test]
    fn forget_drops_only_its_own_hosts_kill_dialog() {
        // A disconnect must take the confirmation with it: leaving one
        // up would offer to signal a session that is already gone.
        // Another host's dialog is none of its business.
        let mut state = MonitorState::default();
        let mine = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let pending = |id| {
            crate::monitor::kill::PendingKill::new(
                id,
                "host".into(),
                &crate::monitor::model::PortStat {
                    port: 8080,
                    proto: "tcp",
                    bind: None,
                    process: Some("node".into()),
                    pid: Some(42),
                },
                crate::monitor::kill::KillSignal::Term,
            )
        };
        let k = key("srv.example");
        state.kill = Some(pending(other));
        state.forget(&k, &mine);
        assert!(state.kill.is_some());
        state.kill = Some(pending(mine));
        state.forget(&k, &mine);
        assert!(state.kill.is_none());
    }

    #[test]
    fn a_full_reset_takes_the_kill_dialog_too() {
        // `monitor_reset_all` is a `Default::default()` assignment, so
        // this pins that the field participates in it.
        let state = MonitorState::default();
        assert!(state.kill.is_none());
    }
}
