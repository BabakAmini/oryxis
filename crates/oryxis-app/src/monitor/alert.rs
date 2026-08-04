//! Threshold alerts for the host monitor (issue #83, plan J2).
//!
//! A pegged host should say so once, not every tick. The rule is
//! rising-edge: a metric that crosses its threshold fires a single
//! toast and latches; it only re-arms after the metric comes back down.
//! CPU additionally has to stay high across several samples, because a
//! single 5-second window catching a compile or a backup is noise, not
//! news.
//!
//! Foreground only, by owner constraint: these are toasts while the app
//! is running, never background alerting. Nothing is persisted.

use super::model::Sample;

/// Percentages a metric must exceed to count as breached.
const CPU_PCT: f32 = 90.0;
const MEM_PCT: f32 = 90.0;
const DISK_PCT: f32 = 95.0;

/// How many consecutive samples CPU must stay above `CPU_PCT`. Memory
/// and disk fire on the first reading: they don't spike the way CPU
/// does, so a single sample over the line is already the story.
const CPU_SUSTAINED: usize = 3;

/// Which thresholds a host is currently over. Latched so a breach is
/// announced once per crossing rather than once per tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BreachFlags {
    pub cpu: bool,
    pub mem: bool,
    pub disk: bool,
}

/// A threshold that just went over. The view turns it into a toast; the
/// mount name rides along so a disk alert can say WHICH disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Breach {
    Cpu,
    Mem,
    Disk(String),
}

/// Compare the newest readings against `flags` and return the updated
/// flags plus whatever crossed a line on THIS sample.
///
/// `recent` is the tail of the host's window, oldest first: CPU reads
/// the last `CPU_SUSTAINED` entries, everything else only the newest.
pub(crate) fn evaluate(recent: &[&Sample], flags: BreachFlags) -> (BreachFlags, Vec<Breach>) {
    let mut out = Vec::new();
    let mut next = flags;

    let Some(latest) = recent.last() else {
        return (next, out);
    };

    // CPU: every one of the last N samples must be over the line. A
    // window that isn't full yet can't breach, which is what we want on
    // a freshly opened tab.
    let cpu_high = recent.len() >= CPU_SUSTAINED
        && recent[recent.len() - CPU_SUSTAINED..]
            .iter()
            .all(|s| s.cpu.is_some_and(|c| c.pct > CPU_PCT));
    // Re-arming needs a reading: a sample with no CPU% (the first after
    // mount) leaves the latch alone rather than silently clearing it.
    if cpu_high && !next.cpu {
        next.cpu = true;
        out.push(Breach::Cpu);
    } else if !cpu_high && latest.cpu.is_some() {
        next.cpu = false;
    }

    if let Some(mem) = latest.mem {
        let high = mem.pct() > MEM_PCT;
        if high && !next.mem {
            next.mem = true;
            out.push(Breach::Mem);
        } else if !high {
            next.mem = false;
        }
    }

    // Disks latch as a group: one flag, so a host with three full mounts
    // doesn't fire three toasts. The name of the worst one is what the
    // toast reports.
    if !latest.disks.is_empty() {
        let worst = latest
            .disks
            .iter()
            .filter(|d| d.pct() > DISK_PCT)
            .max_by(|a, b| a.pct().total_cmp(&b.pct()));
        match worst {
            Some(d) if !next.disk => {
                next.disk = true;
                out.push(Breach::Disk(d.mount.clone()));
            }
            None => next.disk = false,
            _ => {}
        }
    }

    (next, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::model::{CpuStat, DiskStat, MemStat, Sample};
    use std::time::Instant;

    fn sample(cpu: Option<f32>, mem_pct: Option<f32>, disks: &[(&str, f32)]) -> Sample {
        Sample {
            at: Instant::now(),
            cpu: cpu.map(|pct| CpuStat { pct }),
            mem: mem_pct.map(|pct| MemStat {
                used: (pct * 10.0) as u64,
                total: 1000,
                swap_used: 0,
                swap_total: 0,
            }),
            load: None,
            net: None,
            disks: disks
                .iter()
                .map(|(mount, pct)| DiskStat {
                    mount: (*mount).to_string(),
                    used: (*pct * 10.0) as u64,
                    total: 1000,
                })
                .collect(),
            gpus: Vec::new(),
            ports: Vec::new(),
            uptime_secs: None,
        }
    }

    fn eval(samples: &[Sample], flags: BreachFlags) -> (BreachFlags, Vec<Breach>) {
        let refs: Vec<&Sample> = samples.iter().collect();
        evaluate(&refs, flags)
    }

    #[test]
    fn cpu_needs_to_stay_high_and_fires_once() {
        let hot = || sample(Some(95.0), None, &[]);
        // Two samples over the line is a spike, not a breach.
        let (flags, breaches) = eval(&[hot(), hot()], BreachFlags::default());
        assert!(breaches.is_empty());
        assert!(!flags.cpu);

        // The third crossing fires exactly one alert...
        let (flags, breaches) = eval(&[hot(), hot(), hot()], flags);
        assert_eq!(breaches, vec![Breach::Cpu]);
        assert!(flags.cpu);

        // ...and staying hot does NOT keep firing.
        let (flags, breaches) = eval(&[hot(), hot(), hot()], flags);
        assert!(breaches.is_empty());
        assert!(flags.cpu);

        // Coming back down re-arms, so the next episode is announced.
        let cool = sample(Some(10.0), None, &[]);
        let (flags, breaches) = eval(&[hot(), hot(), cool], flags);
        assert!(breaches.is_empty());
        assert!(!flags.cpu);
        let (_, breaches) = eval(&[hot(), hot(), hot()], flags);
        assert_eq!(breaches, vec![Breach::Cpu]);
    }

    #[test]
    fn a_sample_without_cpu_leaves_the_latch_alone() {
        let latched = BreachFlags { cpu: true, ..Default::default() };
        // The first sample after a reconnect has no CPU%: that is not
        // evidence the host recovered, so the latch must survive.
        let (flags, breaches) = eval(&[sample(None, None, &[])], latched);
        assert!(flags.cpu, "no reading must not clear the latch");
        assert!(breaches.is_empty());
    }

    #[test]
    fn memory_fires_on_the_first_reading_over_the_line() {
        let (flags, breaches) = eval(&[sample(None, Some(95.0), &[])], BreachFlags::default());
        assert_eq!(breaches, vec![Breach::Mem]);
        assert!(flags.mem);
        // Still high: silent.
        let (flags, breaches) = eval(&[sample(None, Some(97.0), &[])], flags);
        assert!(breaches.is_empty());
        // Recovered: re-armed.
        let (flags, _) = eval(&[sample(None, Some(40.0), &[])], flags);
        assert!(!flags.mem);
    }

    #[test]
    fn disks_latch_as_a_group_and_report_the_worst() {
        // Two mounts over the line produce ONE alert, naming the fuller.
        let (flags, breaches) = eval(
            &[sample(None, None, &[("/", 96.0), ("/var", 99.0)])],
            BreachFlags::default(),
        );
        assert_eq!(breaches, vec![Breach::Disk("/var".to_string())]);
        assert!(flags.disk);

        // A third mount filling up later doesn't re-fire while latched.
        let (flags, breaches) = eval(
            &[sample(None, None, &[("/", 96.0), ("/srv", 99.9)])],
            flags,
        );
        assert!(breaches.is_empty());

        // Freeing space anywhere below the line clears the latch.
        let (flags, _) = eval(&[sample(None, None, &[("/", 10.0)])], flags);
        assert!(!flags.disk);
    }

    #[test]
    fn a_host_under_every_threshold_stays_quiet() {
        let (flags, breaches) = eval(
            &[
                sample(Some(50.0), Some(50.0), &[("/", 50.0)]),
                sample(Some(50.0), Some(50.0), &[("/", 50.0)]),
                sample(Some(50.0), Some(50.0), &[("/", 50.0)]),
            ],
            BreachFlags::default(),
        );
        assert!(breaches.is_empty());
        assert_eq!(flags, BreachFlags::default());
    }
}
