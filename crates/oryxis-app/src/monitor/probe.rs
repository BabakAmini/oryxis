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
    CpuStat, DiskStat, GpuStat, LoadStat, MemStat, NetStat, PortStat, RawSnapshot, Sample,
};
use std::time::Instant;

/// Sentinel between sections. Long and unlikely enough that a file's own
/// contents can't forge one.
const SEP: &str = "---ORYXIS-MON-SEP---";

/// Listening sockets. `ss` is the modern tool; `netstat` covers hosts
/// that still ship net-tools and nothing else. `-p` names only the
/// processes the login user owns unless we are root, so unnamed ports
/// are expected, not a failure.
///
/// Shared with the kill pipeline (issue #96), which re-runs exactly this
/// to re-resolve a port's owner before signalling it: same command, same
/// parser, no second dialect to keep in sync.
pub(crate) const LISTENING_SOCKETS_CMD: &str =
    "ss -tulnp 2>/dev/null || netstat -tulnp 2>/dev/null || netstat -an 2>/dev/null";

/// The batched probe command. Every section is guarded so a missing file
/// yields an empty section instead of an error, and the sentinels keep
/// the split stable regardless.
pub(crate) fn linux_probe_command() -> String {
    probe_command(true)
}

/// The dashboard's cheaper variant (issue #95, owner call): vitals
/// only, the listening-sockets section replaced by a no-op. The slot
/// stays in place (the parser splits by POSITION), it just comes back
/// empty, exactly like a host without `ss`/`netstat`.
pub(crate) fn linux_probe_command_vitals() -> String {
    probe_command(false)
}

fn probe_command(with_ports: bool) -> String {
    let batch = [
        // Each section falls back to its BSD / macOS equivalent when the
        // Linux source produces nothing, so ONE command serves every
        // host and no OS detection has to happen first. The parsers pick
        // the format by shape (see `probe_bsd`).
        //
        // `head` reads the file DIRECTLY (never `cat | head`): a
        // pipeline's exit status is its last command's, and `head`
        // succeeds on empty stdin, so the pipe form would swallow the
        // failure and the `||` fallback could never run. That exact bug
        // kept `kern.cp_time` dead on FreeBSD.
        "head -n1 /proc/stat 2>/dev/null || sysctl -n kern.cp_time 2>/dev/null",
        "cat /proc/meminfo 2>/dev/null || { sysctl -n hw.memsize 2>/dev/null \
             | sed \"s/^/HwMemsize: /\"; vm_stat 2>/dev/null; }",
        "cat /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg 2>/dev/null",
        "cat /proc/net/dev 2>/dev/null || netstat -ibn 2>/dev/null",
        "df -kP 2>/dev/null || df -k 2>/dev/null",
        "cat /proc/uptime 2>/dev/null || { date +%s | sed \"s/^/Now: /\"; \
             sysctl -n kern.boottime 2>/dev/null; }",
        if with_ports { LISTENING_SOCKETS_CMD } else { "true" },
        // GPU (roadmap: host-monitoring GPU gauges). NVIDIA first:
        // nvidia-smi answers one CSV line per device, `nounits` so the
        // fields parse bare (util %, VRAM MiB, temp C), name last since
        // it is the only field that can contain spaces. Fallback: the
        // amdgpu sysfs files, one "AMD" line per card that exposes
        // gpu_busy_percent (the VRAM reads may legitimately fail on
        // older kernels; the parser treats them as optional). No GPU,
        // no tools -> empty section -> the sidebar renders nothing.
        "nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,name \
             --format=csv,noheader,nounits 2>/dev/null \
             || for c in /sys/class/drm/card*/device; do \
                 [ -r \"$c/gpu_busy_percent\" ] && echo \"AMD $(cat \"$c/gpu_busy_percent\") \
                     $(cat \"$c/mem_info_vram_used\" 2>/dev/null) \
                     $(cat \"$c/mem_info_vram_total\" 2>/dev/null)\"; done 2>/dev/null",
    ]
    .join(&format!("; echo \"{SEP}\"; "));
    // The exec channel hands the command to the user's LOGIN shell, and
    // the batch is Bourne syntax (`{ }`, `||`, `2>`): csh/tcsh/fish
    // users would fail every tick forever. Wrapping in `sh -c '...'`
    // makes the probe shell-independent; the batch deliberately contains
    // no single quotes (the seds use double quotes) so the wrapper needs
    // no escaping and stays a plain quoted literal in every login shell.
    debug_assert!(!batch.contains('\''));
    format!("sh -c '{batch}'")
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
    let sockets = sections.next().unwrap_or("");
    let gpu = sections.next().unwrap_or("");

    let (cpu_total, cpu_idle) = parse_cpu_jiffies(stat)
        .or_else(|| super::probe_bsd::parse_cp_time(stat))
        .unwrap_or((0, 0));
    let (net_rx, net_tx) = parse_net_dev(netdev)
        .or_else(|| super::probe_bsd::parse_netstat_ib(netdev))
        .unwrap_or((0, 0));
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
        mem: parse_meminfo(meminfo)
            .or_else(|| super::probe_bsd::parse_vm_stat(meminfo)),
        load: parse_loadavg(loadavg)
            .or_else(|| super::probe_bsd::parse_sysctl_loadavg(loadavg)),
        net,
        disks: {
            // POSIX `df -kP` keeps the mount in column 6; macOS `df -k`
            // pads extra inode columns and puts it last. Both dialects
            // hand their rows to the SAME mount rules, so the pseudo /
            // bind-mount / duplicate handling can't differ by host
            // (issue #135: filtering inside one parser only would let
            // the other one resurrect every row it dropped).
            let rows = parse_df(df);
            let rows =
                if rows.is_empty() { super::probe_bsd::parse_df_bsd(df) } else { rows };
            filter_mounts(rows)
        },
        gpus: parse_gpu(gpu),
        ports: parse_listening_ports_any(sockets),
        uptime_secs: parse_uptime(uptime)
            .or_else(|| super::probe_bsd::parse_boottime(uptime)),
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
    // Saturating arithmetic throughout: the numbers come from the REMOTE
    // host (untrusted input), and forged u64::MAX-scale values must
    // degrade instead of panicking (debug) or wrapping (release).
    let total: u64 = fields.iter().fold(0u64, |a, f| a.saturating_add(*f));
    let idle = fields[3].saturating_add(fields.get(4).copied().unwrap_or(0));
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
            total.saturating_sub(free.saturating_add(buffers).saturating_add(cached))
        }
    };
    let swap_total = kb("SwapTotal").unwrap_or(0);
    let swap_free = kb("SwapFree").unwrap_or(0);
    Some(MemStat {
        used: used_kb.saturating_mul(1024),
        total: total.saturating_mul(1024),
        swap_used: swap_total.saturating_sub(swap_free).saturating_mul(1024),
        swap_total: swap_total.saturating_mul(1024),
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
        rx = rx.saturating_add(fields[0]);
        tx = tx.saturating_add(fields[8]);
        saw_any = true;
    }
    saw_any.then_some((rx, tx))
}

/// One `df` row, before the mount rules run. The source column is what
/// decides whether a row is a real filesystem, a bind mount or a
/// duplicate, and it is never rendered, so it rides here instead of
/// widening [`DiskStat`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawMount {
    pub source: String,
    pub stat: DiskStat,
}

/// `df -kP` -> one row per line, in the order `df` printed them, sizes
/// already in bytes. Every filtering decision belongs to
/// [`filter_mounts`], so this is pure column shape.
///
/// POSIX output is one line per filesystem with a fixed column order, so
/// the mount point is everything after the 5th field (mount points can
/// contain spaces; the preceding numeric columns cannot).
fn parse_df(df: &str) -> Vec<RawMount> {
    let mut out = Vec::new();
    for line in df.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let Ok(total_kb) = fields[1].parse::<u64>() else { continue };
        let Ok(used_kb) = fields[2].parse::<u64>() else { continue };
        if total_kb == 0 {
            continue;
        }
        // The mount point opens column 6 and is absolute. macOS's
        // non-POSIX `df -k` pads three inode columns in between, so a
        // relative field here means the row is in the OTHER dialect and
        // belongs to `probe_bsd::parse_df_bsd` rather than to a guess
        // (this parser used to accept those rows and label them
        // "500000 700000 42% /").
        if !fields[5].starts_with('/') {
            continue;
        }
        // Rejoin the mount point, which may contain spaces.
        let mount = fields[5..].join(" ");
        out.push(RawMount {
            source: fields[0].to_string(),
            stat: DiskStat {
                mount,
                used: used_kb.saturating_mul(1024),
                total: total_kb.saturating_mul(1024),
            },
        });
    }
    out
}

/// The mount rules (issue #135), applied to whichever dialect answered.
///
/// A `df` listing is not a list of storage: it is a list of MOUNTS, and
/// one device can appear under dozens of them. A Magisk'd Android phone
/// reported 176, every `/system/bin/*` bind mount repeating `/system`'s
/// figures, which also handed the status bar's "worst disk" segment a
/// bind mount to name. Three rules, in order:
///
/// 1. Drop pseudo filesystems ([`is_pseudo_fs`]): they measure RAM or a
///    kernel interface, never storage.
/// 2. Keep ONE row per source device, the shallowest mount point. Every
///    deeper mount of the same source is a bind mount of a filesystem
///    already listed. Btrfs subvolumes collapse under the same rule,
///    which is deliberate: `/` and `/home` on one device report the
///    same pool figures, so two rows would be the same answer twice.
/// 3. Drop a row whose SOURCE is itself a path (Android's `/data/media`,
///    a plain `mount --bind`) when its numbers match a device-backed
///    row exactly. The path shape alone can't decide it: a CIFS share
///    (`//nas/backup`) and a mergerfs pool (`/mnt/disk1`) are
///    path-shaped too and duplicate nothing.
fn filter_mounts(rows: Vec<RawMount>) -> Vec<DiskStat> {
    let real: Vec<RawMount> =
        rows.into_iter().filter(|m| !is_pseudo_fs(&m.source)).collect();

    // Rule 2. `keep` holds indices into `real`: a source's first row
    // claims the slot and later rows only UPGRADE it, so the survivor
    // keeps the position `df` gave the device rather than jumping to
    // wherever its shallowest mount happened to be printed.
    let mut keep: Vec<usize> = Vec::new();
    for (i, m) in real.iter().enumerate() {
        match keep.iter().position(|&k| real[k].source == m.source) {
            Some(slot) => {
                if is_shallower(&m.stat.mount, &real[keep[slot]].stat.mount) {
                    keep[slot] = i;
                }
            }
            None => keep.push(i),
        }
    }

    // Rule 3. Measured against the DEVICE-backed survivors only, so the
    // outcome can't depend on which of two bind mounts `df` printed
    // first (the device row is the one that must survive).
    let device_sizes: Vec<(u64, u64)> = keep
        .iter()
        .filter(|&&i| !is_path_source(&real[i].source))
        .map(|&i| (real[i].stat.used, real[i].stat.total))
        .collect();
    keep.retain(|&i| {
        !is_path_source(&real[i].source)
            || !device_sizes.contains(&(real[i].stat.used, real[i].stat.total))
    });

    keep.into_iter().map(|i| real[i].stat.clone()).collect()
}

/// Mount `a` is more primary than `b`: fewer path components first
/// (`/system` beats `/system/bin/am`), then the shorter string, then the
/// lexicographically smaller one. Total and stable, so the survivor of a
/// device's mounts never depends on `df`'s ordering.
fn is_shallower(a: &str, b: &str) -> bool {
    let depth = |p: &str| p.split('/').filter(|s| !s.is_empty()).count();
    (depth(a), a.len(), a) < (depth(b), b.len(), b)
}

/// A source naming a DIRECTORY rather than a device or a remote export.
/// `/dev/...` is a device node and `//host/share` is CIFS, so both are
/// excluded; what is left is a bind mount of a path that some other row
/// already measures.
fn is_path_source(source: &str) -> bool {
    source.starts_with('/') && !source.starts_with("//") && !source.starts_with("/dev/")
}

/// Virtual / kernel filesystems that would clutter the disk list without
/// telling the user anything about free space.
///
/// Matched on the SOURCE column because POSIX `df` prints no filesystem
/// type (`-T` is a GNU extension that toybox / busybox — i.e. every
/// Android host — reject), and a pseudo filesystem names itself there.
/// `rootfs` is deliberately absent: on Android it is the only row
/// mounting `/`, so filtering it would delete the root filesystem.
fn is_pseudo_fs(source: &str) -> bool {
    matches!(
        source,
        "tmpfs"
            | "devtmpfs"
            | "devfs"
            | "ramfs"
            | "overlay"
            | "overlayfs"
            | "shm"
            | "proc"
            | "sysfs"
            | "cgroup"
            | "cgroup2"
            | "devpts"
            | "squashfs"
            | "debugfs"
            | "tracefs"
            | "securityfs"
            | "selinuxfs"
            | "pstore"
            | "configfs"
            | "bpf"
            | "binfmt_misc"
            | "mqueue"
            | "hugetlbfs"
            | "fusectl"
            | "nsfs"
            | "efivarfs"
            | "autofs"
            | "rpc_pipefs"
            | "sunrpc"
            | "udev"
            | "none"
    ) || source.starts_with("/dev/loop")
}

/// Listening sockets in whichever dialect the host answered in: the
/// Linux `ss` / `netstat` parser first, the BSD `netstat -an` fallback
/// when it found nothing. Shared with the kill pipeline, which re-runs
/// [`LISTENING_SOCKETS_CMD`] on its own and needs the same shape rules.
pub(crate) fn parse_listening_ports_any(sockets: &str) -> Vec<PortStat> {
    let p = parse_listening_ports(sockets);
    if p.is_empty() { super::probe_bsd::parse_netstat_an(sockets) } else { p }
}

/// Listening sockets from `ss -tulnp` or `netstat -tulnp`.
///
/// Both tools are parsed by shape rather than by column index: the first
/// token gives the protocol (`tcp` / `tcp6` / `udp` / `udp6`), the local
/// address is the first `host:port` token, and the process name comes
/// from `ss`'s `users:(("name",...` or `netstat`'s `pid/name`. That
/// survives the column drift between distro versions (and `ss`'s
/// header, which runs its last two labels together).
///
/// Results are deduped and sorted by port: a service bound on both IPv4
/// and IPv6 is one forwardable port, not two rows.
fn parse_listening_ports(sockets: &str) -> Vec<PortStat> {
    let mut out: Vec<PortStat> = Vec::new();
    for line in sockets.lines() {
        let mut fields = line.split_whitespace();
        let Some(first) = fields.next() else { continue };
        let proto = match first {
            "tcp" | "tcp6" => "tcp",
            "udp" | "udp6" => "udp",
            _ => continue, // header or noise
        };
        // Listener whitelist, not a state blacklist: the `-l` commands
        // only print listeners, but the last fallback (`netstat -an`)
        // prints EVERY socket, and a blacklist missed FIN_WAIT / SYN_SENT
        // / LAST_ACK rows, turning ephemeral outbound connections into
        // forwardable "listening ports". TCP rows must say LISTEN;
        // stateless UDP rows pass unless they are connected sockets.
        if proto == "tcp" {
            if !line.contains("LISTEN") {
                continue;
            }
        } else if line.contains("ESTAB") || line.contains("CONNECTED") {
            continue;
        }
        let Some((bind, port)) = fields.clone().find_map(parse_local_addr) else { continue };
        let (process, pid) = match parse_process_entry(line) {
            Some((name, pid)) => (Some(name), pid),
            None => (None, None),
        };
        if let Some(existing) = out.iter_mut().find(|p| p.port == port && p.proto == proto) {
            // Same service on a second address family: keep whichever
            // row managed to name the process, and let a wildcard bind
            // win over a specific one (v4 0.0.0.0 + v6 :: is ONE
            // any-interface listener, not a bound one).
            //
            // Name and PID move TOGETHER (first named row wins,
            // deterministic in payload order): a kill target assembled
            // from two different rows could signal a process that never
            // owned the port.
            if existing.process.is_none() {
                existing.process = process;
                existing.pid = pid;
            }
            if bind.is_none() {
                existing.bind = None;
            }
            continue;
        }
        out.push(PortStat { port, proto, bind, process, pid });
    }
    out.sort_by_key(|p| (p.port, p.proto));
    out
}

/// Pull `(bind address, port)` out of a `host:port` token, rejecting the
/// peer column's wildcard (`0.0.0.0:*`) and anything without a numeric
/// port. The bind is `None` for the any-interface forms (`0.0.0.0`,
/// `::`, `[::]`, `*`); a specific address is kept, stripped of v6
/// brackets and a `%iface` scope (systemd-resolved binds
/// `127.0.0.53%lo`).
fn parse_local_addr(token: &str) -> Option<(Option<String>, u16)> {
    let (host, port) = token.rsplit_once(':')?;
    if host.is_empty() || port == "*" {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|p| *p > 0)?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = host.split('%').next().unwrap_or(host);
    let bind = match host {
        "0.0.0.0" | "::" | "*" => None,
        h => Some(h.to_string()),
    };
    Some((bind, port))
}

/// `ss`: `users:(("sshd",pid=1,fd=3))`. `netstat`: `1234/sshd`.
///
/// Returns the process name and the PID **from the same entry**. `ss`
/// lists one entry per file descriptor and prints them all on one line
/// (`users:(("nginx",pid=1,fd=6),("nginx",pid=2,fd=6))`), so a PID
/// harvested by scanning the whole line could belong to a different
/// worker than the name; only the FIRST entry is read, and the name and
/// PID always travel together. The PID stays `Option` inside the pair
/// because a very old `ss` prints `users:(("sshd",1,3))` without the
/// `pid=` label, which still names the process.
fn parse_process_entry(line: &str) -> Option<(String, Option<u32>)> {
    if let Some(rest) = line.split("users:((").nth(1) {
        // One entry: everything up to the first `)`.
        let entry = rest.split(')').next().unwrap_or(rest);
        let mut quoted = entry.splitn(3, '"');
        let _leading = quoted.next();
        if let Some(name) = quoted.next()
            && !name.is_empty()
        {
            let pid = entry.split("pid=").nth(1).and_then(|p| {
                p.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse::<u32>()
                    .ok()
            });
            return Some((name.to_string(), pid));
        }
    }
    // netstat's PID/Program column is the last token; "-" means the
    // process wasn't visible to this user.
    let last = line.split_whitespace().next_back()?;
    let (pid, name) = last.split_once('/')?;
    let pid = pid.parse::<u32>().ok()?;
    (!name.is_empty()).then(|| (name.to_string(), Some(pid)))
}

/// GPU section in either dialect, one device per line.
///
/// NVIDIA (`nvidia-smi --format=csv,noheader,nounits`):
/// `42, 1234, 8192, 65, NVIDIA GeForce RTX 3080` (util %, VRAM used /
/// total MiB, temp C, name). A field an old driver can't answer prints
/// `[Not Supported]` / `[N/A]`; only utilization is required, the rest
/// degrade to `None`. The name is the tail so its own commas (none in
/// practice) or spaces can't shift the numeric columns.
///
/// AMD (our sysfs fallback): `AMD <busy%> [<vram_used> <vram_total>]`
/// in bytes, VRAM optional (older kernels don't expose the files).
fn parse_gpu(gpu: &str) -> Vec<GpuStat> {
    let mut out = Vec::new();
    for line in gpu.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("AMD ") {
            let mut fields = rest.split_whitespace();
            let Some(util) = fields.next().and_then(|f| f.parse::<f32>().ok()) else {
                continue;
            };
            let mem_used = fields.next().and_then(|f| f.parse::<u64>().ok());
            let mem_total = fields.next().and_then(|f| f.parse::<u64>().ok());
            out.push(GpuStat {
                name: None,
                util_pct: util.clamp(0.0, 100.0),
                // Either both or neither: a used figure against an
                // unknown total renders as a lie of a gauge.
                mem_used: mem_total.and(mem_used),
                mem_total: mem_used.and(mem_total),
                temp_c: None,
            });
            continue;
        }
        // NVIDIA CSV row: 4 numeric columns, then the name.
        let fields: Vec<&str> = line.splitn(5, ',').map(str::trim).collect();
        if fields.len() < 5 {
            continue;
        }
        let Ok(util) = fields[0].parse::<f32>() else { continue };
        let mib = |f: &str| f.parse::<u64>().ok().map(|v| v.saturating_mul(1024 * 1024));
        let mem_used = mib(fields[1]);
        let mem_total = mib(fields[2]);
        out.push(GpuStat {
            name: (!fields[4].is_empty()).then(|| fields[4].to_string()),
            util_pct: util.clamp(0.0, 100.0),
            mem_used: mem_total.and(mem_used),
            mem_total: mem_used.and(mem_total),
            temp_c: fields[3].parse::<i32>().ok(),
        });
    }
    out
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

    /// Build a payload from per-section strings in probe order. Const
    /// generic so the older 7-section fixtures keep compiling: a
    /// truncated payload is a real case the parser must handle anyway.
    fn payload<const N: usize>(sections: [&str; N]) -> String {
        sections.join(&format!("\n{SEP}\n"))
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn cpu_needs_a_baseline_then_reports_the_delta() {
        // 100 jiffies total, 100 idle -> fully idle.
        let first = payload(["cpu 0 0 0 100 0 0 0 0", "", "", "", "", "", ""]);
        let now = t0();
        let (sample, snap) = parse_linux(&first, None, now);
        assert!(sample.cpu.is_none(), "first sample has no baseline");

        // +100 total, +100 idle -> 0% busy.
        let idle = payload(["cpu 0 0 0 200 0 0 0 0", "", "", "", "", "", ""]);
        let (sample, snap2) =
            parse_linux(&idle, Some(snap), now + Duration::from_secs(5));
        assert_eq!(sample.cpu.unwrap().pct, 0.0);

        // +100 total, +0 idle -> 100% busy.
        let busy = payload(["cpu 100 0 0 200 0 0 0 0", "", "", "", "", "", ""]);
        let (sample, _) =
            parse_linux(&busy, Some(snap2), now + Duration::from_secs(10));
        assert_eq!(sample.cpu.unwrap().pct, 100.0);
    }

    #[test]
    fn cpu_counts_iowait_as_idle_and_survives_a_reset() {
        let a = payload(["cpu 100 0 100 800 0 0 0 0", "", "", "", "", "", ""]);
        let now = t0();
        let (_, snap) = parse_linux(&a, None, now);
        // +200 jiffies total: +100 user (busy) and +100 iowait (idle).
        // Counting iowait as idle makes that 50% busy, not 100%.
        let b = payload(["cpu 200 0 100 800 100 0 0 0", "", "", "", "", "", ""]);
        let (sample, _) = parse_linux(&b, Some(snap), now + Duration::from_secs(1));
        assert_eq!(sample.cpu.unwrap().pct, 50.0);

        // Counters going backwards (reboot) yield no reading, never a spike.
        let after_reboot = payload(["cpu 1 0 1 1 0 0 0 0", "", "", "", "", "", ""]);
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
            parse_linux(&payload(["", modern, "", "", "", "", ""]), None, t0());
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
            parse_linux(&payload(["", legacy, "", "", "", "", ""]), None, t0());
        assert_eq!(sample.mem.unwrap().used, 6_400_000 * 1024);
    }

    #[test]
    fn loadavg_and_uptime_parse() {
        let (sample, _) = parse_linux(
            &payload(["", "", "0.42 0.35 0.31 2/431 9999", "", "", "12345.67 500.0", ""]),
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
        let (sample, snap) = parse_linux(&payload(["", "", "", dev, "", "", ""]), None, now);
        assert!(sample.net.is_none(), "no baseline yet");
        // lo's counters must not be in the snapshot.
        assert_eq!(snap.net_rx, 1000);
        assert_eq!(snap.net_tx, 2000);

        let dev2 = "Inter-|\nface |\n\
                    lo: 999999 10 0 0 0 0 0 0 999999 10 0 0 0 0 0 0\n\
                    eth0: 6000 10 0 0 0 0 0 0 4000 20 0 0 0 0 0 0\n";
        let (sample, _) = parse_linux(
            &payload(["", "", "", dev2, "", "", ""]),
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
        let (sample, _) = parse_linux(&payload(["", "", "", "", df, "", ""]), None, t0());
        let mounts: Vec<&str> =
            sample.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/mnt/my disk"]);
        assert_eq!(sample.disks[0].used, 4_000_000 * 1024);
        assert_eq!(sample.disks[0].pct(), 40.0);
    }

    /// Issue #135, reported from a Magisk'd Android phone over Termux:
    /// 176 disk rows, nearly all of them `/system/bin/*` bind mounts of
    /// the SYSTEM partition, which also gave the status bar's worst-disk
    /// segment a bind mount to name.
    #[test]
    fn android_bind_mounts_collapse_to_one_row_per_device() {
        let sys = "/dev/block/platform/13540000.dwmmc0/by-name/SYSTEM";
        let df = format!(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
             rootfs      1258291    8704  1249587  1% /\n\
             {sys}       4718592 4299161   419431 92% /system\n\
             {sys}       4718592 4299161   419431 92% /system/bin\n\
             {sys}       4718592 4299161   419431 92% /system/bin/am\n\
             {sys}       4718592 4299161   419431 92% /system/bin/wvkprov\n\
             {sys}       4718592 4299161   419431 92% /system/priv-app/Settings\n\
             /dev/block/platform/13540000.dwmmc0/by-name/CACHE   197632    12288   181248  7% /cache\n\
             /dev/block/platform/13540000.dwmmc0/by-name/USERDATA 24117248 10485760 13631488 45% /data\n\
             /dev/block/platform/13540000.dwmmc0/by-name/EFS       16384     2969    13312 19% /efs\n\
             tmpfs       1363148    3482  1359666  1% /dev\n\
             magisk      1363148    3482  1359666  1% /sbin\n\
             /data/media 24117248 10485760 13631488 45% /storage/emulated\n"
        );
        let (sample, _) = parse_linux(&payload(["", "", "", "", &df, "", ""]), None, t0());
        let mounts: Vec<&str> = sample.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/system", "/cache", "/data", "/efs", "/sbin"]);
        // The device's PRIMARY mount is what survives, not whichever
        // bind mount `df` happened to print (that is the row the status
        // bar names when a disk fills up).
        assert_eq!(sample.disks[1].mount, "/system");
        assert_eq!(sample.disks[1].pct().round(), 91.0);
        // `/storage/emulated` is a bind mount of `/data/media`: a
        // path-shaped source repeating a device row's figures.
        assert!(!mounts.contains(&"/storage/emulated"));
        // `magisk` at /sbin is a tmpfs, but POSIX `df` prints no
        // filesystem type and the source doesn't say so, so it stays a
        // row rather than being guessed away.
        assert_eq!(sample.disks[5].mount, "/sbin");
    }

    #[test]
    fn real_filesystems_survive_the_bind_mount_rules() {
        let df = "Filesystem   1024-blocks      Used Available Capacity Mounted on\n\
                  /dev/sda1       10000000   4000000   6000000      40% /\n\
                  //nas/backup   500000000 100000000 400000000      20% /mnt/backup\n\
                  /mnt/disk1     200000000  50000000 150000000      25% /mnt/pool\n\
                  tank/home      300000000  30000000 270000000      10% /home\n\
                  /dev/sda1       10000000   4000000   6000000      40% /var/lib/docker/btrfs\n";
        let (sample, _) = parse_linux(&payload(["", "", "", "", df, "", ""]), None, t0());
        let mounts: Vec<&str> = sample.disks.iter().map(|d| d.mount.as_str()).collect();
        // A CIFS share and a mergerfs pool are path-shaped sources that
        // duplicate nothing; a ZFS dataset is its own source. Only the
        // second mount of /dev/sda1 collapses.
        assert_eq!(mounts, vec!["/", "/mnt/backup", "/mnt/pool", "/home"]);
    }

    #[test]
    fn the_bsd_fallback_cannot_resurrect_filtered_rows() {
        // Every row is pseudo, so the Linux rules leave nothing. The
        // macOS fallback must not re-parse the same text and hand them
        // all back (it only runs when the POSIX SHAPE found nothing).
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                  tmpfs         1000000     0   1000000       0% /dev/shm\n\
                  devfs             200   200         0     100% /dev\n";
        let (sample, _) = parse_linux(&payload(["", "", "", "", df, "", ""]), None, t0());
        assert!(sample.disks.is_empty());
    }

    #[test]
    fn the_bsd_dialect_gets_the_same_mount_rules() {
        // macOS `df -k`: extra inode columns, mount last. devfs is
        // pseudo here too, and a second mount of the same device
        // collapses onto its shallowest mount point.
        let df = "Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n\
                  /dev/disk1s1   488245288 200000000 288245288    41%  500000  700000   42%   /\n\
                  devfs                200       200         0   100%     600       0  100%   /dev\n\
                  /dev/disk1s1   488245288 200000000 288245288    41%  500000  700000   42%   /System/Volumes/Data\n";
        let (sample, _) = parse_linux(&payload(["", "", "", "", df, "", ""]), None, t0());
        let mounts: Vec<&str> = sample.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/"]);
    }

    #[test]
    fn ss_output_yields_listening_ports_with_names() {
        // Real `ss -tulnp` shape, including the header whose last two
        // labels run together and a `%lo` scoped address.
        let ss = "Netid State  Recv-Q Send-Q  Local Address:Port Peer Address:PortProcess\n\
                  udp   UNCONN 0      0       127.0.0.53%lo:53        0.0.0.0:*\n\
                  tcp   LISTEN 0      128           0.0.0.0:22        0.0.0.0:*     users:((\"sshd\",pid=1,fd=3))\n\
                  tcp   LISTEN 0      511         127.0.0.1:8080      0.0.0.0:*     users:((\"node\",pid=42,fd=20))\n\
                  tcp   LISTEN 0      128              [::]:22           [::]:*     users:((\"sshd\",pid=1,fd=4))\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ss]), None, t0());
        // sshd on v4 + v6 is ONE forwardable port, and the rows are
        // sorted by port.
        let got: Vec<(u16, &str, Option<&str>)> = sample
            .ports
            .iter()
            .map(|p| (p.port, p.proto, p.process.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                (22, "tcp", Some("sshd")),
                (53, "udp", None),
                (8080, "tcp", Some("node")),
            ]
        );
        // Bind addresses feed click-to-forward: wildcard listeners have
        // none, a `%lo` scope is stripped, a loopback bind is kept.
        let binds: Vec<Option<&str>> =
            sample.ports.iter().map(|p| p.bind.as_deref()).collect();
        assert_eq!(binds, vec![None, Some("127.0.0.53"), Some("127.0.0.1")]);
        // PIDs feed the kill action (issue #96); an unnamed row has none.
        let pids: Vec<Option<u32>> = sample.ports.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![Some(1), None, Some(42)]);
    }

    #[test]
    fn a_multi_worker_row_pairs_the_name_with_its_own_pid() {
        // `ss` prints one entry per fd on ONE line. Scanning the whole
        // line for `pid=` would pair worker 3's number with worker 1's
        // name; only the first entry is read, whole.
        let ss = "tcp LISTEN 0 511 0.0.0.0:80 0.0.0.0:* \
                  users:((\"nginx\",pid=811,fd=6),(\"nginx\",pid=810,fd=6),(\"nginx\",pid=809,fd=6))\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ss]), None, t0());
        assert_eq!(sample.ports.len(), 1);
        assert_eq!(sample.ports[0].process.as_deref(), Some("nginx"));
        assert_eq!(sample.ports[0].pid, Some(811));
    }

    #[test]
    fn an_ss_without_the_pid_label_still_names_the_process() {
        // Very old `ss` prints `users:(("sshd",1,3))`. The name is
        // usable; inventing a PID out of the positional fields is not.
        let ss = "tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",1,3))\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ss]), None, t0());
        assert_eq!(sample.ports[0].process.as_deref(), Some("sshd"));
        assert_eq!(sample.ports[0].pid, None);
    }

    #[test]
    fn the_v4_v6_collapse_keeps_the_name_and_pid_together() {
        // The v6 row is the one that names the process; the merged row
        // must take BOTH of its fields, never a name from one row and a
        // PID from another (that would signal the wrong process).
        let ss = "tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:*\n\
                  tcp LISTEN 0 128 [::]:22 [::]:* users:((\"sshd\",pid=904,fd=4))\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ss]), None, t0());
        assert_eq!(sample.ports.len(), 1);
        assert_eq!(sample.ports[0].process.as_deref(), Some("sshd"));
        assert_eq!(sample.ports[0].pid, Some(904));

        // Reversed order: the FIRST named row wins, deterministically,
        // and the loser's PID never leaks onto the winner's name.
        let ss = "tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1,fd=3))\n\
                  tcp LISTEN 0 128 [::]:22 [::]:* users:((\"sshd\",pid=904,fd=4))\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ss]), None, t0());
        assert_eq!(sample.ports[0].pid, Some(1));
    }

    #[test]
    fn wildcard_bind_wins_the_v4_v6_collapse() {
        // A specific v4 bind plus a v6 wildcard on the same port is ONE
        // any-interface listener; the merged row must not claim the
        // narrower bind (the forward would work, but the row would lie).
        let ss = "tcp   LISTEN 0      128         127.0.0.1:9000      0.0.0.0:*\n\
                  tcp   LISTEN 0      128              [::]:9000         [::]:*\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ss]), None, t0());
        assert_eq!(sample.ports.len(), 1);
        assert_eq!(sample.ports[0].bind, None);
    }

    #[test]
    fn netstat_fallback_and_non_listening_rows() {
        let netstat = "Active Internet connections (only servers)\n\
                       Proto Recv-Q Send-Q Local Address           Foreign Address         State       PID/Program name\n\
                       tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN      1/sshd\n\
                       tcp        0      0 127.0.0.1:5432          0.0.0.0:*               LISTEN      -\n\
                       tcp        0      0 10.0.0.5:54321          10.0.0.9:443            ESTABLISHED 99/curl\n\
                       udp        0      0 0.0.0.0:68              0.0.0.0:*                           7/dhclient\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", netstat]), None, t0());
        let got: Vec<(u16, &str, Option<&str>)> = sample
            .ports
            .iter()
            .map(|p| (p.port, p.proto, p.process.as_deref()))
            .collect();
        // The ESTABLISHED row is not a listening port; the `-` process
        // column means "not ours to see", not a name.
        assert_eq!(
            got,
            vec![
                (22, "tcp", Some("sshd")),
                (68, "udp", Some("dhclient")),
                (5432, "tcp", None),
            ]
        );
        let binds: Vec<Option<&str>> =
            sample.ports.iter().map(|p| p.bind.as_deref()).collect();
        assert_eq!(binds, vec![None, None, Some("127.0.0.1")]);
        // The `-` column is "not ours to see", so neither name nor PID.
        let pids: Vec<Option<u32>> = sample.ports.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![Some(1), Some(7), None]);
    }

    #[test]
    fn netstat_an_ephemeral_states_are_not_listeners() {
        // The last fallback (`netstat -an` on a busybox host) prints
        // EVERY socket. Ephemeral outbound states must not become
        // forwardable "listening ports" (whitelist, not blacklist).
        let netstat = "Proto Recv-Q Send-Q Local Address           Foreign Address         State\n\
                       tcp        0      0 0.0.0.0:22              0.0.0.0:*               LISTEN\n\
                       tcp        0      0 10.0.0.5:39112          10.0.0.9:443            FIN_WAIT1\n\
                       tcp        0      0 10.0.0.5:39113          10.0.0.9:443            SYN_SENT\n\
                       tcp        0      0 10.0.0.5:39114          10.0.0.9:443            LAST_ACK\n\
                       udp        0      0 10.0.0.5:47000          10.0.0.9:53             ESTABLISHED\n\
                       udp        0      0 0.0.0.0:68              0.0.0.0:*\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", netstat]), None, t0());
        let got: Vec<(u16, &str)> = sample.ports.iter().map(|p| (p.port, p.proto)).collect();
        assert_eq!(got, vec![(22, "tcp"), (68, "udp")]);
    }

    #[test]
    fn probe_command_is_shell_wrapped_and_head_reads_directly() {
        let cmd = linux_probe_command();
        // The batch must reach a POSIX shell regardless of the user's
        // login shell, and must contain no inner single quote (the
        // wrapper relies on it).
        assert!(cmd.starts_with("sh -c '") && cmd.ends_with('\''));
        assert_eq!(cmd.matches('\'').count(), 2);
        // `head file || fallback`, never `cat file | head || fallback`:
        // the pipe form always exits 0 and kills the BSD fallback.
        assert!(cmd.contains("head -n1 /proc/stat"));
        assert!(!cmd.contains("| head"));
        assert!(cmd.contains("kern.cp_time"));
    }

    #[test]
    fn hostile_huge_counters_degrade_instead_of_panicking() {
        // A malicious host can print any numbers it likes; sums and unit
        // conversions must saturate, not overflow.
        let max = u64::MAX;
        let evil = payload([
            &format!("cpu {max} {max} {max} {max} {max} 0 0 0"),
            &format!("MemTotal: {max} kB\nMemFree: {max} kB\nBuffers: {max} kB\nCached: {max} kB\nSwapTotal: {max} kB\nSwapFree: 0 kB"),
            "",
            &format!("eth0: {max} 0 0 0 0 0 0 0 {max} 0 0 0 0 0 0 0\neth1: {max} 0 0 0 0 0 0 0 {max} 0 0 0 0 0 0 0"),
            &format!("Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda1 {max} {max} 0 100% /"),
            "",
            "",
        ]);
        let (sample, snap) = parse_linux(&evil, None, t0());
        assert_eq!(snap.cpu_total, u64::MAX);
        assert_eq!(snap.net_rx, u64::MAX);
        assert_eq!(sample.mem.unwrap().total, u64::MAX);
        assert_eq!(sample.disks[0].total, u64::MAX);
    }

    #[test]
    fn nvidia_csv_rows_parse_with_units_and_name() {
        let gpu = "42, 1234, 8192, 65, NVIDIA GeForce RTX 3080\n\
                   7, 100, 24576, 41, NVIDIA RTX A5000\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", "", gpu]), None, t0());
        assert_eq!(sample.gpus.len(), 2);
        let g = &sample.gpus[0];
        assert_eq!(g.util_pct, 42.0);
        assert_eq!(g.mem_used, Some(1234 * 1024 * 1024));
        assert_eq!(g.mem_total, Some(8192 * 1024 * 1024));
        assert_eq!(g.temp_c, Some(65));
        assert_eq!(g.name.as_deref(), Some("NVIDIA GeForce RTX 3080"));
        assert_eq!(sample.gpus[1].name.as_deref(), Some("NVIDIA RTX A5000"));
    }

    #[test]
    fn nvidia_not_supported_fields_degrade_per_field() {
        // Old drivers answer `[Not Supported]` per field; utilization is
        // the only hard requirement, and VRAM is all-or-nothing (a used
        // figure against an unknown total would render a lying gauge).
        let gpu = "13, [Not Supported], 8192, [N/A], Tesla K80\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", "", gpu]), None, t0());
        let g = &sample.gpus[0];
        assert_eq!(g.util_pct, 13.0);
        assert_eq!(g.mem_used, None);
        assert_eq!(g.mem_total, None);
        assert_eq!(g.temp_c, None);
        // A row whose utilization is unreadable is no row at all.
        let gpu = "[Not Supported], 1, 2, 3, Ghost GPU\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", "", gpu]), None, t0());
        assert!(sample.gpus.is_empty());
    }

    #[test]
    fn amd_sysfs_rows_parse_with_optional_vram() {
        // Full row: busy% plus the two VRAM byte counters.
        let gpu = "AMD 37 1234567890 17163091968\nAMD 5\n";
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", "", gpu]), None, t0());
        assert_eq!(sample.gpus.len(), 2);
        assert_eq!(sample.gpus[0].util_pct, 37.0);
        assert_eq!(sample.gpus[0].mem_used, Some(1_234_567_890));
        assert_eq!(sample.gpus[0].mem_total, Some(17_163_091_968));
        assert!(sample.gpus[0].name.is_none());
        // Kernel without the VRAM files: busy% only.
        assert_eq!(sample.gpus[1].util_pct, 5.0);
        assert_eq!(sample.gpus[1].mem_used, None);
        assert_eq!(sample.gpus[1].mem_total, None);
    }

    #[test]
    fn hosts_without_a_gpu_render_no_gpu_section() {
        // No 8th section at all (older payload), an empty one, and shell
        // noise must all yield the same empty list.
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ""]), None, t0());
        assert!(sample.gpus.is_empty());
        let (sample, _) = parse_linux(
            &payload(["", "", "", "", "", "", "", "sh: nvidia-smi: not found"]),
            None,
            t0(),
        );
        assert!(sample.gpus.is_empty());
        // Hostile utilization values clamp instead of blowing the gauge.
        let (sample, _) = parse_linux(
            &payload(["", "", "", "", "", "", "", "AMD 9999"]),
            None,
            t0(),
        );
        assert_eq!(sample.gpus[0].util_pct, 100.0);
    }

    #[test]
    fn hosts_without_ss_or_netstat_report_no_ports() {
        let (sample, _) =
            parse_linux(&payload(["", "", "", "", "", "", ""]), None, t0());
        assert!(sample.ports.is_empty());
        // Garbage must not panic or invent ports either.
        let (sample, _) = parse_linux(
            &payload(["", "", "", "", "", "", "sh: ss: not found\n???"]),
            None,
            t0(),
        );
        assert!(sample.ports.is_empty());
    }

    #[test]
    fn missing_and_malformed_sections_degrade_to_none() {
        // A restricted container: no /proc reads, no df, everything empty.
        let (sample, _) = parse_linux(&payload(["", "", "", "", "", "", ""]), None, t0());
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
            "",
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
