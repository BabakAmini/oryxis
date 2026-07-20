//! In-memory sample ring for the host monitor (issue #83, plan J2).
//!
//! One `HostSeries` per monitored connection, holding a bounded window of
//! recent samples (for the sparkline) plus the raw counters the next tick
//! diffs against. Nothing is persisted: the whole thing is dropped on
//! disconnect and on vault lock.

use std::collections::{HashMap, HashSet, VecDeque};

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

    /// CPU percentages over the window, oldest first, for the sparkline.
    /// Samples without a reading (the first one after mount) are skipped
    /// so the line starts where real data does.
    pub fn cpu_series(&self) -> Vec<f32> {
        self.samples.iter().filter_map(|s| s.cpu.map(|c| c.pct)).collect()
    }
}

/// Monitor state hanging off the app: one series per monitored host plus
/// the in-flight guard.
#[derive(Debug, Default)]
pub(crate) struct MonitorState {
    pub series: HashMap<uuid::Uuid, HostSeries>,
    /// Hosts with a probe already in flight. A slow host is skipped on
    /// the next tick instead of queueing probes behind each other.
    pub probing: HashSet<uuid::Uuid>,
}

impl MonitorState {
    /// Drop a host's window entirely (disconnect, monitoring turned off).
    pub fn forget(&mut self, id: &uuid::Uuid) {
        self.series.remove(id);
        self.probing.remove(id);
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

    #[test]
    fn forget_clears_both_the_window_and_the_in_flight_guard() {
        let mut state = MonitorState::default();
        let id = uuid::Uuid::new_v4();
        let (s, snap) = sample_with(Some(1.0), Instant::now());
        state.series.entry(id).or_default().push(s, snap);
        state.probing.insert(id);
        state.forget(&id);
        assert!(state.series.is_empty());
        assert!(state.probing.is_empty());
    }
}
