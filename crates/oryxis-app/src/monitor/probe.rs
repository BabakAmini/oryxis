//! Linux probe pack for the host monitor (issue #83, plan J2).
//!
//! One batched `sh -c` per tick reads every `/proc` file we need plus
//! `df`, separated by sentinels, so a poll costs a single exec channel on
//! the session's live handle rather than one per metric. The parser is
//! pure (`&str` + previous snapshot -> `Sample`) so it unit-tests against
//! captured fixtures without a network.
//!
//! Every section is independently optional: a container with a partial
//! `/proc`, a host without `df`, a shell that refuses a read, all leave
//! their field `None` and the rest of the sample intact.

use super::model::{
    CpuStat, DiskStat, LoadStat, MemStat, NetStat, RawSnapshot, Sample,
};
use std::time::Instant;

/// Sentinel between sections. Long and unlikely enough that a file's own
/// contents can't forge one.
const SEP: &str = "---ORYXIS-MON-SEP---";

/// The batched probe command. Every section is guarded so a missing file
/// yields an empty section instead of an error, and the sentinels keep
/// the split stable regardless.
pub(crate) fn linux_probe_command() -> String {
    [
        "cat /proc/stat 2>/dev/null | head -n1",
        "cat /proc/meminfo 2>/dev/null",
        "cat /proc/loadavg 2>/dev/null",
        "cat /proc/net/dev 2>/dev/null",
        "df -kP 2>/dev/null",
        "cat /proc/uptime 2>/dev/null",
    ]
    .join(&format!("; echo '{SEP}'; "))
}

/// Parse a batched probe payload into a `Sample`, using `prev` (the last
/// tick's counters) to derive the CPU and network rates. Returns the
/// sample plus the fresh snapshot to keep for the next tick.
///
/// `now` is passed in rather than read here so tests are deterministic.
pub(crate) fn parse_linux(
    payload: &str,
    prev: Option<RawSnapshot>,
    now: Instant,
) -> (Sample, RawSnapshot) {
    let mut sections = payload.split(SEP);
    let stat = sections.next().unwrap_or("");
    let meminfo = sections.next().unwrap_or("");
    let loadavg = sections.next().unwrap_or("");
    let netdev = sections.next().unwrap_or("");
    let df = sections.next().unwrap_or("");
    let uptime = sections.next().unwrap_or("");

    let (cpu_total, cpu_idle) = parse_cpu_jiffies(stat).unwrap_or((0, 0));
    let (net_rx, net_tx) = parse_net_dev(netdev).unwrap_or((0, 0));
    let snapshot = RawSnapshot { cpu_total, cpu_idle, net_rx, net_tx, at: now };

    // Rates need a baseline. A counter that went backwards (reboot, or a
    // container's namespace swapped underneath us) is treated as no data
    // rather than a wild spike.
    let cpu = prev.and_then(|p| {
        let d_total = cpu_total.checked_sub(p.cpu_total)?;
        let d_idle = cpu_idle.checked_sub(p.cpu_idle)?;
        if d_total == 0 {
            return None;
        }
        let busy = d_total.saturating_sub(d_idle);
        Some(CpuStat {
            pct: ((busy as f64 / d_total as f64) * 100.0) as f32,
        })
    });
    let net = prev.and_then(|p| {
        let elapsed = now.saturating_duration_since(p.at).as_secs_f64();
        if elapsed <= 0.0 {
            return None;
        }
        let d_rx = net_rx.checked_sub(p.net_rx)?;
        let d_tx = net_tx.checked_sub(p.net_tx)?;
        Some(NetStat {
            rx_bps: (d_rx as f64 / elapsed) as u64,
            tx_bps: (d_tx as f64 / elapsed) as u64,
        })
    });

    let sample = Sample {
        at: now,
        cpu,
        mem: parse_meminfo(meminfo),
        load: parse_loadavg(loadavg),
        net,
        disks: parse_df(df),
        uptime_secs: parse_uptime(uptime),
    };
    (sample, snapshot)
}

/// `/proc/stat`'s aggregate `cpu` line -> `(total jiffies, idle jiffies)`.
/// Idle counts both `idle` and `iowait`: a host blocked on disk is not
/// burning CPU, and every common tool reports it that way.
fn parse_cpu_jiffies(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse::<u64>().ok())
        .collect();
    // user nice system idle iowait irq softirq steal ...
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some((total, idle))
}

/// `/proc/meminfo` -> used/total plus swap, all in bytes.
///
/// Used is `MemTotal - MemAvailable` when the kernel reports availability
/// (2.6.27+, the honest figure since it accounts for reclaimable cache);
/// otherwise it falls back to the classic `Total - Free - Buffers -
/// Cached`.
fn parse_meminfo(meminfo: &str) -> Option<MemStat> {
    let kb = |key: &str| -> Option<u64> {
        meminfo.lines().find_map(|l| {
            let rest = l.strip_prefix(key)?.strip_prefix(':')?;
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
    };
    let total = kb("MemTotal")?;
    let used_kb = match kb("MemAvailable") {
        Some(avail) => total.saturating_sub(avail),
        None => {
            let free = kb("MemFree").unwrap_or(0);
            let buffers = kb("Buffers").unwrap_or(0);
            let cached = kb("Cached").unwrap_or(0);
            total.saturating_sub(free + buffers + cached)
        }
    };
    let swap_total = kb("SwapTotal").unwrap_or(0);
    let swap_free = kb("SwapFree").unwrap_or(0);
    Some(MemStat {
        used: used_kb * 1024,
        total: total * 1024,
        swap_used: swap_total.saturating_sub(swap_free) * 1024,
        swap_total: swap_total * 1024,
    })
}

/// `/proc/loadavg`: `0.42 0.35 0.31 2/431 12345`.
fn parse_loadavg(loadavg: &str) -> Option<LoadStat> {
    let line = loadavg.lines().find(|l| !l.trim().is_empty())?;
    let mut fields = line.split_whitespace();
    let one = fields.next()?.parse::<f32>().ok()?;
    let five = fields.next()?.parse::<f32>().ok()?;
    let fifteen = fields.next()?.parse::<f32>().ok()?;
    let (procs_running, procs_total) = fields
        .next()
        .and_then(|p| p.split_once('/'))
        .map(|(r, t)| {
            (
                r.parse::<u32>().unwrap_or(0),
                t.parse::<u32>().unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    Some(LoadStat { one, five, fifteen, procs_running, procs_total })
}

/// `/proc/net/dev` -> summed rx/tx bytes across real interfaces.
/// Loopback is excluded (it would double-count local traffic and dwarf
/// the real link on a busy host).
fn parse_net_dev(netdev: &str) -> Option<(u64, u64)> {
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut saw_any = false;
    for line in netdev.lines() {
        let Some((iface, rest)) = line.split_once(':') else { continue };
        let iface = iface.trim();
        if iface.is_empty() || iface == "lo" {
            continue;
        }
        let fields: Vec<u64> = rest
            .split_whitespace()
            .map(|f| f.parse::<u64>().unwrap_or(0))
            .collect();
        // rx_bytes is field 0; tx_bytes is field 8.
        if fields.len() < 9 {
            continue;
        }
        rx += fields[0];
        tx += fields[8];
        saw_any = true;
    }
    saw_any.then_some((rx, tx))
}

/// `df -kP` -> per-mount used/total in bytes, real filesystems only.
///
/// POSIX output is one line per filesystem with a fixed column order, so
/// the mount point is everything after the 5th field (mount points can
/// contain spaces; the preceding numeric columns cannot).
fn parse_df(df: &str) -> Vec<DiskStat> {
    let mut out = Vec::new();
    for line in df.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let source = fields[0];
        if is_pseudo_fs(source) {
            continue;
        }
        let Ok(total_kb) = fields[1].parse::<u64>() else { continue };
        let Ok(used_kb) = fields[2].parse::<u64>() else { continue };
        if total_kb == 0 {
            continue;
        }
        // Rejoin the mount point, which may contain spaces.
        let mount = fields[5..].join(" ");
        out.push(DiskStat {
            mount,
            used: used_kb * 1024,
            total: total_kb * 1024,
        });
    }
    out
}

/// Virtual / kernel filesystems that would clutter the disk list without
/// telling the user anything about free space.
fn is_pseudo_fs(source: &str) -> bool {
    matches!(
        source,
        "tmpfs"
            | "devtmpfs"
            | "overlay"
            | "shm"
            | "proc"
            | "sysfs"
            | "cgroup"
            | "cgroup2"
            | "devpts"
            | "squashfs"
            | "udev"
            | "none"
    ) || source.starts_with("/dev/loop")
}

/// `/proc/uptime`: `12345.67 98765.43` (seconds since boot, idle time).
fn parse_uptime(uptime: &str) -> Option<u64> {
    uptime
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|s| s as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Build a payload from per-section strings in probe order.
    fn payload(sections: [&str; 6]) -> String {
        sections.join(&format!("\n{SEP}\n"))
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn cpu_needs_a_baseline_then_reports_the_delta() {
        // 100 jiffies total, 100 idle -> fully idle.
        let first = payload(["cpu 0 0 0 100 0 0 0 0", "", "", "", "", ""]);
        let now = t0();
        let (sample, snap) = parse_linux(&first, None, now);
        assert!(sample.cpu.is_none(), "first sample has no baseline");

        // +100 total, +100 idle -> 0% busy.
        let idle = payload(["cpu 0 0 0 200 0 0 0 0", "", "", "", "", ""]);
        let (sample, snap2) =
            parse_linux(&idle, Some(snap), now + Duration::from_secs(5));
        assert_eq!(sample.cpu.unwrap().pct, 0.0);

        // +100 total, +0 idle -> 100% busy.
        let busy = payload(["cpu 100 0 0 200 0 0 0 0", "", "", "", "", ""]);
        let (sample, _) =
            parse_linux(&busy, Some(snap2), now + Duration::from_secs(10));
        assert_eq!(sample.cpu.unwrap().pct, 100.0);
    }

    #[test]
    fn cpu_counts_iowait_as_idle_and_survives_a_reset() {
        let a = payload(["cpu 100 0 100 800 0 0 0 0", "", "", "", "", ""]);
        let now = t0();
        let (_, snap) = parse_linux(&a, None, now);
        // +200 jiffies total: +100 user (busy) and +100 iowait (idle).
        // Counting iowait as idle makes that 50% busy, not 100%.
        let b = payload(["cpu 200 0 100 800 100 0 0 0", "", "", "", "", ""]);
        let (sample, _) = parse_linux(&b, Some(snap), now + Duration::from_secs(1));
        assert_eq!(sample.cpu.unwrap().pct, 50.0);

        // Counters going backwards (reboot) yield no reading, never a spike.
        let after_reboot = payload(["cpu 1 0 1 1 0 0 0 0", "", "", "", "", ""]);
        let (_, snap_b) = parse_linux(&b, None, now);
        let (sample, _) =
            parse_linux(&after_reboot, Some(snap_b), now + Duration::from_secs(2));
        assert!(sample.cpu.is_none());
    }

    #[test]
    fn meminfo_prefers_available_and_falls_back() {
        let modern = "MemTotal:       8000000 kB\n\
                      MemFree:         500000 kB\n\
                      MemAvailable:   6000000 kB\n\
                      Buffers:         100000 kB\n\
                      Cached:         1000000 kB\n\
                      SwapTotal:      2000000 kB\n\
                      SwapFree:       1500000 kB\n";
        let (sample, _) =
            parse_linux(&payload(["", modern, "", "", "", ""]), None, t0());
        let mem = sample.mem.unwrap();
        assert_eq!(mem.total, 8_000_000 * 1024);
        // 8000000 - 6000000 available.
        assert_eq!(mem.used, 2_000_000 * 1024);
        assert_eq!(mem.swap_used, 500_000 * 1024);

        // Pre-2.6.27 kernels: Total - Free - Buffers - Cached.
        let legacy = "MemTotal:       8000000 kB\n\
                      MemFree:         500000 kB\n\
                      Buffers:         100000 kB\n\
                      Cached:         1000000 kB\n";
        let (sample, _) =
            parse_linux(&payload(["", legacy, "", "", "", ""]), None, t0());
        assert_eq!(sample.mem.unwrap().used, 6_400_000 * 1024);
    }

    #[test]
    fn loadavg_and_uptime_parse() {
        let (sample, _) = parse_linux(
            &payload(["", "", "0.42 0.35 0.31 2/431 9999", "", "", "12345.67 500.0"]),
            None,
            t0(),
        );
        let load = sample.load.unwrap();
        assert_eq!(load.one, 0.42);
        assert_eq!(load.fifteen, 0.31);
        assert_eq!(load.procs_running, 2);
        assert_eq!(load.procs_total, 431);
        assert_eq!(sample.uptime_secs, Some(12345));
    }

    #[test]
    fn net_excludes_loopback_and_derives_a_rate() {
        let dev = "Inter-|   Receive                    |  Transmit\n\
                   face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets\n\
                   lo: 999999 10 0 0 0 0 0 0 999999 10 0 0 0 0 0 0\n\
                   eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n";
        let now = t0();
        let (sample, snap) = parse_linux(&payload(["", "", "", dev, "", ""]), None, now);
        assert!(sample.net.is_none(), "no baseline yet");
        // lo's counters must not be in the snapshot.
        assert_eq!(snap.net_rx, 1000);
        assert_eq!(snap.net_tx, 2000);

        let dev2 = "Inter-|\nface |\n\
                    lo: 999999 10 0 0 0 0 0 0 999999 10 0 0 0 0 0 0\n\
                    eth0: 6000 10 0 0 0 0 0 0 4000 20 0 0 0 0 0 0\n";
        let (sample, _) = parse_linux(
            &payload(["", "", "", dev2, "", ""]),
            Some(snap),
            now + Duration::from_secs(5),
        );
        let net = sample.net.unwrap();
        assert_eq!(net.rx_bps, 1000); // +5000 over 5s
        assert_eq!(net.tx_bps, 400); // +2000 over 5s
    }

    #[test]
    fn df_filters_pseudo_filesystems_and_keeps_spaced_mounts() {
        let df = "Filesystem     1024-blocks     Used Available Capacity Mounted on\n\
                  /dev/sda1         10000000  4000000   6000000      40% /\n\
                  tmpfs              1000000        0   1000000       0% /dev/shm\n\
                  /dev/loop0           50000    50000         0     100% /snap/core\n\
                  /dev/sdb1         20000000  1000000  19000000       5% /mnt/my disk\n";
        let (sample, _) = parse_linux(&payload(["", "", "", "", df, ""]), None, t0());
        let mounts: Vec<&str> =
            sample.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/mnt/my disk"]);
        assert_eq!(sample.disks[0].used, 4_000_000 * 1024);
        assert_eq!(sample.disks[0].pct(), 40.0);
    }

    #[test]
    fn missing_and_malformed_sections_degrade_to_none() {
        // A restricted container: no /proc reads, no df, everything empty.
        let (sample, _) = parse_linux(&payload(["", "", "", "", "", ""]), None, t0());
        assert!(sample.cpu.is_none());
        assert!(sample.mem.is_none());
        assert!(sample.load.is_none());
        assert!(sample.net.is_none());
        assert!(sample.disks.is_empty());
        assert!(sample.uptime_secs.is_none());

        // Garbage in every section must not panic either.
        let junk = payload([
            "cpu not numbers here",
            "MemTotal: banana",
            "not a load average",
            "eth0 no colon separator",
            "short line",
            "definitely not a float",
        ]);
        let (sample, _) = parse_linux(&junk, None, t0());
        assert!(sample.mem.is_none());
        assert!(sample.load.is_none());
        assert!(sample.disks.is_empty());
    }

    #[test]
    fn a_truncated_payload_still_parses_what_arrived() {
        // Only the first two sections made it (channel cut short).
        let partial = format!(
            "cpu 1 2 3 4 5 6 7 8\n{SEP}\nMemTotal:  1000 kB\nMemAvailable: 400 kB\n"
        );
        let (sample, snap) = parse_linux(&partial, None, t0());
        assert_eq!(sample.mem.unwrap().used, 600 * 1024);
        assert!(snap.cpu_total > 0);
        assert!(sample.disks.is_empty());
    }
}
