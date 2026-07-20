//! BSD / macOS fallbacks for the host monitor (issue #83, plan J2).
//!
//! These hosts have no `/proc`, so each probe section carries a second
//! command that runs only when the Linux one produced nothing (`||` in
//! the batched shell line). The parsers here are tried by the Linux
//! parsers as a fallback, chosen by SHAPE rather than by a detected OS:
//! one round trip, no detection state, and a host that reports neither
//! format simply leaves the field empty.
//!
//! Best-effort by design (the plan says so): where a metric has no cheap
//! portable source, the field stays `None` and the UI shows a dash. The
//! notable gap is macOS CPU%, whose only shell source (`top -l1`) is a
//! pre-computed percentage that doesn't fit the delta model the rest of
//! the engine uses; FreeBSD's `kern.cp_time` does fit and is supported.

use super::model::{DiskStat, LoadStat, MemStat, PortStat};

/// FreeBSD `sysctl -n kern.cp_time`: `user nice sys intr idle` in ticks,
/// the same shape as Linux's `/proc/stat` line, so it feeds the very
/// same delta arithmetic.
pub(crate) fn parse_cp_time(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| {
        let mut f = l.split_whitespace();
        f.clone().count() >= 5 && f.all(|t| t.parse::<u64>().is_ok())
    })?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .filter_map(|f| f.parse::<u64>().ok())
        .collect();
    if fields.len() < 5 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    // CPUSTATES order: user, nice, sys, intr, idle.
    Some((total, fields[4]))
}

/// macOS `vm_stat` plus the `HwMemsize:` line the probe prepends from
/// `sysctl -n hw.memsize`.
///
/// Used is active + wired + compressed: the honest "can't be handed to
/// another process without paging" figure, matching what Activity
/// Monitor calls memory used. Free / inactive / speculative pages are
/// reclaimable, so they count as available.
pub(crate) fn parse_vm_stat(text: &str) -> Option<MemStat> {
    let page_size = text
        .lines()
        .find(|l| l.contains("page size of"))
        .and_then(|l| {
            let rest = l.split("page size of").nth(1)?;
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
        .unwrap_or(4096);
    let pages = |label: &str| -> Option<u64> {
        text.lines().find_map(|l| {
            let rest = l.trim().strip_prefix(label)?.strip_prefix(':')?;
            rest.trim().trim_end_matches('.').parse::<u64>().ok()
        })
    };
    let total = text.lines().find_map(|l| {
        l.trim()
            .strip_prefix("HwMemsize:")?
            .trim()
            .parse::<u64>()
            .ok()
    })?;
    let active = pages("Pages active")?;
    let wired = pages("Pages wired down").unwrap_or(0);
    let compressed = pages("Pages occupied by compressor").unwrap_or(0);
    let used = (active + wired + compressed) * page_size;
    Some(MemStat {
        used: used.min(total),
        total,
        // Swap lives behind `sysctl vm.swapusage`, whose human-readable
        // "1024.00M" format is a parser of its own; left out rather than
        // guessed, so the UI simply omits the swap gauge.
        swap_used: 0,
        swap_total: 0,
    })
}

/// `sysctl -n vm.loadavg` on both macOS and FreeBSD: `{ 0.42 0.35 0.31 }`.
/// Process counts have no equivalent here, so they read as zero and the
/// UI omits that row.
pub(crate) fn parse_sysctl_loadavg(text: &str) -> Option<LoadStat> {
    let inner = text.trim().trim_start_matches('{').trim_end_matches('}');
    let mut f = inner.split_whitespace();
    let one = f.next()?.parse::<f32>().ok()?;
    let five = f.next()?.parse::<f32>().ok()?;
    let fifteen = f.next()?.parse::<f32>().ok()?;
    Some(LoadStat { one, five, fifteen, procs_running: 0, procs_total: 0 })
}

/// `netstat -ibn` on BSD / macOS, summing per-interface byte counters.
///
/// The table repeats each interface once per address family, so only the
/// `<Link#N>` rows are counted (they carry the interface totals); taking
/// every row would multiply the figures. Loopback is excluded like the
/// Linux path.
pub(crate) fn parse_netstat_ib(text: &str) -> Option<(u64, u64)> {
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut saw_any = false;
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        // Name Mtu Network Address Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll
        if f.len() < 10 || !f[2].starts_with("<Link") {
            continue;
        }
        if f[0].starts_with("lo") {
            continue;
        }
        let (Ok(ib), Ok(ob)) = (f[6].parse::<u64>(), f[9].parse::<u64>()) else {
            continue;
        };
        rx += ib;
        tx += ob;
        saw_any = true;
    }
    saw_any.then_some((rx, tx))
}

/// Uptime from `sysctl -n kern.boottime` plus the `Now:` line the probe
/// prepends from `date +%s`.
///
/// macOS prints `{ sec = 1700000000, usec = 0 } Tue Nov...`; FreeBSD the
/// same shape. Remote "now" is read on the host rather than locally, so
/// a clock skew between the two machines can't turn into a wrong uptime.
pub(crate) fn parse_boottime(text: &str) -> Option<u64> {
    let now = text.lines().find_map(|l| {
        l.trim().strip_prefix("Now:")?.trim().parse::<u64>().ok()
    })?;
    let boot = text.split("sec =").nth(1).and_then(|rest| {
        rest.trim()
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse::<u64>()
            .ok()
    })?;
    now.checked_sub(boot)
}

/// macOS / BSD `netstat -an` listening sockets, which use DOTS instead
/// of a colon before the port (`*.22`, `127.0.0.1.5432`) and report
/// `tcp4` / `tcp6` / `udp4` protocols.
///
/// Process names are not available here (`netstat -p` on BSD selects a
/// protocol rather than showing PIDs), so every row is unnamed.
pub(crate) fn parse_netstat_an(text: &str) -> Vec<PortStat> {
    let mut out: Vec<PortStat> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let proto = match f[0] {
            "tcp4" | "tcp6" | "tcp46" => "tcp",
            "udp4" | "udp6" | "udp46" => "udp",
            _ => continue,
        };
        // TCP rows must say LISTEN; UDP has no state column at all.
        if proto == "tcp" && !line.contains("LISTEN") {
            continue;
        }
        let Some((_, port)) = f[3].rsplit_once('.') else { continue };
        let Ok(port) = port.parse::<u16>() else { continue };
        if port == 0 || out.iter().any(|p| p.port == port && p.proto == proto) {
            continue;
        }
        out.push(PortStat { port, proto, process: None });
    }
    out.sort_by_key(|p| (p.port, p.proto));
    out
}

/// `df -k` without POSIX mode (macOS prints extra columns): the mount
/// point is the LAST field rather than the sixth.
pub(crate) fn parse_df_bsd(text: &str) -> Vec<DiskStat> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        let (Ok(total_kb), Ok(used_kb)) = (f[1].parse::<u64>(), f[2].parse::<u64>()) else {
            continue;
        };
        if total_kb == 0 {
            continue;
        }
        let mount = f[f.len() - 1].to_string();
        if !mount.starts_with('/') {
            continue;
        }
        out.push(DiskStat { mount, used: used_kb * 1024, total: total_kb * 1024 });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freebsd_cp_time_matches_the_linux_delta_model() {
        // user nice sys intr idle
        let (total, idle) = parse_cp_time("1000 0 500 100 8400\n").unwrap();
        assert_eq!(total, 10_000);
        assert_eq!(idle, 8_400);
        assert!(parse_cp_time("not numbers").is_none());
    }

    #[test]
    fn macos_vm_stat_counts_active_wired_and_compressed() {
        let text = "HwMemsize: 17179869184\n\
                    Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                    Pages free:                     100000.\n\
                    Pages active:                   200000.\n\
                    Pages inactive:                  50000.\n\
                    Pages speculative:                5000.\n\
                    Pages wired down:               100000.\n\
                    Pages occupied by compressor:    24288.\n";
        let mem = parse_vm_stat(text).unwrap();
        assert_eq!(mem.total, 17_179_869_184);
        // (200000 + 100000 + 24288) pages * 16384
        assert_eq!(mem.used, 324_288 * 16_384);
        // Free / inactive / speculative are reclaimable, so used stays
        // well under total.
        assert!(mem.used < mem.total);
        // No hw.memsize line: nothing to report rather than a guess.
        assert!(parse_vm_stat("Pages active: 1.").is_none());
    }

    #[test]
    fn sysctl_loadavg_parses_the_braced_form() {
        let load = parse_sysctl_loadavg("{ 0.42 0.35 0.31 }").unwrap();
        assert_eq!(load.one, 0.42);
        assert_eq!(load.fifteen, 0.31);
        // No process counts on this path; the UI omits the row.
        assert_eq!(load.procs_total, 0);
        assert!(parse_sysctl_loadavg("{ }").is_none());
    }

    #[test]
    fn netstat_ib_sums_link_rows_only() {
        let text = "Name  Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\n\
                    lo0   16384 <Link#1>                          100     0      10000      100     0      10000     0\n\
                    en0   1500  <Link#2>      aa:bb:cc:dd:ee:ff  5000     0    1000000     4000     0     500000     0\n\
                    en0   1500  192.168.1     192.168.1.10       5000     0    1000000     4000     0     500000     0\n";
        let (rx, tx) = parse_netstat_ib(text).unwrap();
        // Only en0's <Link#2> row: loopback excluded, and the repeated
        // per-family row must not double the totals.
        assert_eq!(rx, 1_000_000);
        assert_eq!(tx, 500_000);
        assert!(parse_netstat_ib("garbage").is_none());
    }

    #[test]
    fn boottime_uses_the_remote_clock() {
        let text = "Now: 1700003600\n{ sec = 1700000000, usec = 123 } Tue Nov 14 22:13:20 2023\n";
        assert_eq!(parse_boottime(text), Some(3_600));
        // A boot time in the future (skew, or a mangled read) yields
        // nothing rather than a wrapped-around uptime.
        let bad = "Now: 1000\n{ sec = 2000, usec = 0 }\n";
        assert_eq!(parse_boottime(bad), None);
    }

    #[test]
    fn bsd_netstat_an_reads_dotted_ports() {
        let text = "Active Internet connections (including servers)\n\
                    Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)\n\
                    tcp4       0      0  *.22                   *.*                    LISTEN\n\
                    tcp6       0      0  *.22                   *.*                    LISTEN\n\
                    tcp4       0      0  192.168.1.10.52000     93.184.216.34.443      ESTABLISHED\n\
                    udp4       0      0  *.68                   *.*\n";
        let ports = parse_netstat_an(text);
        let got: Vec<(u16, &str)> = ports.iter().map(|p| (p.port, p.proto)).collect();
        // v4 + v6 collapse; the ESTABLISHED row is not a listener.
        assert_eq!(got, vec![(22, "tcp"), (68, "udp")]);
        assert!(ports.iter().all(|p| p.process.is_none()));
    }

    #[test]
    fn bsd_df_takes_the_mount_from_the_last_column() {
        let text = "Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n\
                    /dev/disk1s1   488245288 200000000 288245288    41%  500000  700000   42%   /\n\
                    devfs                200       200         0   100%     600     0     100%   /dev\n";
        let disks = parse_df_bsd(text);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].mount, "/");
        assert_eq!(disks[0].used, 200_000_000 * 1024);
        assert_eq!(disks[1].mount, "/dev");
    }
}
