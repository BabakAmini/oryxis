//! Sample types for the agentless host monitor (issue #83, plan J2).
//!
//! Every field is optional by design: a probe reads what the host is
//! willing to give (a container's `/proc` is partial, `df` may be absent,
//! a locked-down shell may refuse), and a missing metric renders as a dash
//! instead of failing the whole sample.
//!
//! Nothing here is serialized: samples live in an in-memory ring and are
//! never persisted, synced or exported (owner constraint).

use std::time::Instant;

/// One poll of a host's vitals.
#[derive(Debug, Clone)]
pub(crate) struct Sample {
    pub at: Instant,
    /// `None` on the first sample after mount: CPU% is a delta and needs
    /// a previous `/proc/stat` snapshot to compare against.
    pub cpu: Option<CpuStat>,
    pub mem: Option<MemStat>,
    pub load: Option<LoadStat>,
    /// `None` on the first sample for the same reason as `cpu`.
    pub net: Option<NetStat>,
    pub disks: Vec<DiskStat>,
    /// Seconds since boot, from `/proc/uptime`.
    pub uptime_secs: Option<u64>,
}

/// Busy percentage over the interval between two `/proc/stat` reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CpuStat {
    pub pct: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemStat {
    pub used: u64,
    pub total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
}

impl MemStat {
    /// Used share of total, 0.0 when the host reports no memory (which
    /// would otherwise divide by zero).
    pub fn pct(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f32 / self.total as f32) * 100.0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LoadStat {
    pub one: f32,
    pub five: f32,
    pub fifteen: f32,
    pub procs_running: u32,
    pub procs_total: u32,
}

/// Throughput in bytes per second, derived from two `/proc/net/dev`
/// snapshots and the real elapsed time between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NetStat {
    pub rx_bps: u64,
    pub tx_bps: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiskStat {
    pub mount: String,
    pub used: u64,
    pub total: u64,
}

impl DiskStat {
    pub fn pct(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f32 / self.total as f32) * 100.0
        }
    }
}

/// Counters kept between ticks so rates need only ONE read per tick: the
/// next sample diffs against this instead of sleeping mid-probe (which
/// would double the effective interval).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawSnapshot {
    /// Sum of all `/proc/stat` `cpu` jiffy fields.
    pub cpu_total: u64,
    /// The idle + iowait share of that sum.
    pub cpu_idle: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub at: Instant,
}
