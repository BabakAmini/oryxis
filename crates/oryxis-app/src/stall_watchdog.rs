//! Event-loop stall watchdog behind the debug-logging toggle (#104).
//!
//! The freeze class in issue #104 does not reproduce anywhere but the
//! reporter's machine, and the one recording we have shows the app not
//! drawing at all: not a slow frame, not a slow message. What decides
//! that issue (and any future "the UI froze" report) is knowing WHICH
//! of four layers stopped, and this module is the instrument:
//!
//! * update heartbeat dead + a message in flight  -> that handler is
//!   blocked; the log names it.
//! * update heartbeat dead + nothing in flight    -> the winit event
//!   loop itself stopped dispatching (platform / driver territory).
//! * update alive + view heartbeat dead while real messages flow ->
//!   iced stopped drawing (presentation / wgpu / compositor).
//! * everything alive but the user sees a freeze  -> pixels leave the
//!   app and die later; the compositor is the suspect.
//!
//! Everything is gated on [`crate::logging::is_enabled`], the same
//! switch as the debug-log file, so a normal session pays one relaxed
//! atomic load per message and nothing else. While enabled, a 500 ms
//! `Message::NoOp` tick (see `subscription.rs`) guarantees the update
//! heartbeat beats on an idle-but-healthy loop, which is what lets the
//! watchdog tell "idle" apart from "dead".

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A message handler running longer than this gets its own log line.
const SLOW_MESSAGE_MS: u64 = 250;
/// No update heartbeat for this long counts as a stall.
const STALL_MS: u64 = 2_000;
/// While a stall persists, re-log at most this often.
const STALL_RELOG_MS: u64 = 5_000;
/// View not running for this long, while real messages flow, counts as
/// a presentation stall.
const VIEW_STALL_MS: u64 = 3_000;
/// How many recent message names the stall report carries.
const RECENT_CAP: usize = 24;
/// Budget for one formatted message name; payloads (PTY chunks!) must
/// never bloat the ring.
const NAME_BUDGET: usize = 96;

/// Milliseconds since the first use of the watchdog in this process.
/// One shared clock so the atomics below can carry plain `u64`s.
fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Last time `update()` was entered (any message, ticks included).
static UPDATE_BEAT: AtomicU64 = AtomicU64::new(0);
/// Last time `update()` was entered for a NON-tick message. `NoOp` is
/// the watchdog's own pacemaker (and the latency re-render tick), so it
/// proves the loop is alive but not that the app is doing anything.
static REAL_BEAT: AtomicU64 = AtomicU64::new(0);
/// Last time `view()` was entered.
static VIEW_BEAT: AtomicU64 = AtomicU64::new(0);
static THREAD_STARTED: AtomicBool = AtomicBool::new(false);

/// The message currently inside `update()`, if any: (entered_at_ms, name).
static IN_FLIGHT: Mutex<Option<(u64, String)>> = Mutex::new(None);
/// The last [`RECENT_CAP`] completed messages: (entered_at_ms, name, took_ms).
static RECENT: Mutex<VecDeque<(u64, String, u64)>> = Mutex::new(VecDeque::new());

/// Diagnostics must never take the app down: shrug off poisoning.
fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `{message:?}` cut at [`NAME_BUDGET`] bytes. The `fmt::Write` error
/// aborts the Debug walk early, so a 64 KB PTY chunk costs nothing past
/// the budget.
fn message_name(message: &crate::app::Message) -> String {
    struct Truncate {
        buf: String,
    }
    impl std::fmt::Write for Truncate {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for ch in s.chars() {
                if self.buf.len() + ch.len_utf8() > NAME_BUDGET {
                    self.buf.push('…');
                    return Err(std::fmt::Error);
                }
                self.buf.push(ch);
            }
            Ok(())
        }
    }
    let mut t = Truncate { buf: String::with_capacity(NAME_BUDGET + 4) };
    let _ = write!(t, "{message:?}");
    t.buf
}

/// RAII marker for one `update()` pass. Created at the top of
/// `Oryxis::update`, dropped on every exit path (early returns
/// included), which is exactly the window the watchdog attributes to
/// the message.
pub(crate) struct MessageGuard {
    entered: u64,
    armed: bool,
}

pub(crate) fn message_guard(message: &crate::app::Message) -> MessageGuard {
    if !crate::logging::is_enabled() {
        return MessageGuard { entered: 0, armed: false };
    }
    ensure_started();
    let now = now_ms();
    UPDATE_BEAT.store(now, Ordering::Relaxed);
    if !matches!(message, crate::app::Message::NoOp) {
        REAL_BEAT.store(now, Ordering::Relaxed);
    }
    *lock(&IN_FLIGHT) = Some((now, message_name(message)));
    MessageGuard { entered: now, armed: true }
}

impl Drop for MessageGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let now = now_ms();
        UPDATE_BEAT.store(now, Ordering::Relaxed);
        if let Some((entered, name)) = lock(&IN_FLIGHT).take() {
            let took = now.saturating_sub(entered);
            if took >= SLOW_MESSAGE_MS {
                tracing::warn!("stall-watchdog: slow message ({took} ms): {name}");
            }
            let mut recent = lock(&RECENT);
            if recent.len() == RECENT_CAP {
                recent.pop_front();
            }
            recent.push_back((entered, name, took));
        }
        let _ = self.entered;
    }
}

/// Heartbeat from the top of `Oryxis::view`.
pub(crate) fn beat_view() {
    if crate::logging::is_enabled() {
        VIEW_BEAT.store(now_ms(), Ordering::Relaxed);
    }
}

/// One line per entry, newest last, ages relative to `now`.
fn recent_report(now: u64) -> String {
    let recent = lock(&RECENT);
    let mut out = String::new();
    for (entered, name, took) in recent.iter() {
        let _ = writeln!(
            out,
            "  -{:>6} ms  ({took} ms)  {name}",
            now.saturating_sub(*entered)
        );
    }
    out
}

/// One compact memory line appended to every stall report: process
/// RSS, system availability, swap in use, and the kernel's PSI memory
/// pressure. This is the swap-storm discriminator for #104-class
/// reports: a machine-wide memory stall freezes the app exactly like a
/// render stall (no pixels, no CPU, innocent-looking in-flight
/// message), so a stall line that also says "memory was fine" or
/// "memory was thrashing" keeps us from chasing the named message in
/// the wrong world. Empty when /proc is unavailable; each part is
/// best-effort on its own.
#[cfg(target_os = "linux")]
fn memory_summary() -> String {
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| field_kb(&s, "VmRSS:"));
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok();
    let avail = meminfo.as_deref().and_then(|s| field_kb(s, "MemAvailable:"));
    let swap_total = meminfo.as_deref().and_then(|s| field_kb(s, "SwapTotal:"));
    let swap_free = meminfo.as_deref().and_then(|s| field_kb(s, "SwapFree:"));
    // PSI needs CONFIG_PSI (default on modern kernels); absent files
    // just drop the part.
    let psi = std::fs::read_to_string("/proc/pressure/memory")
        .ok()
        .and_then(|s| psi_some_avg10(&s));
    let mut out = String::new();
    if let Some(kb) = rss {
        let _ = write!(out, " rss={}MB", kb / 1024);
    }
    if let Some(kb) = avail {
        let _ = write!(out, " avail={}MB", kb / 1024);
    }
    if let (Some(total), Some(free)) = (swap_total, swap_free) {
        let _ = write!(out, " swap={}/{}MB", (total - free.min(total)) / 1024, total / 1024);
    }
    if let Some(avg10) = psi {
        let _ = write!(out, " psi_mem_some_avg10={avg10}");
    }
    if out.is_empty() { out } else { format!("; mem{out}") }
}

#[cfg(not(target_os = "linux"))]
fn memory_summary() -> String {
    String::new()
}

/// `Key:   123456 kB` -> `123456`, from /proc/self/status or
/// /proc/meminfo shaped text.
#[cfg(any(target_os = "linux", test))]
fn field_kb(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

/// The `avg10` value of the `some` line of a /proc/pressure file:
/// share of the last 10s in which at least one task was stalled on
/// memory, which is the earliest system-wide thrashing signal.
#[cfg(any(target_os = "linux", test))]
fn psi_some_avg10(text: &str) -> Option<String> {
    text.lines()
        .find(|line| line.starts_with("some"))
        .and_then(|line| {
            line.split_whitespace().find_map(|token| token.strip_prefix("avg10="))
        })
        .map(|v| v.to_string())
}

/// Spawn the monitor thread once. Parked (cheap sleep loop) while the
/// debug toggle is off, so flipping the setting at runtime arms and
/// disarms it without restarts.
fn ensure_started() {
    if THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // Seed the beats so the first poll after enabling doesn't see a
    // process-lifetime "stall".
    let now = now_ms();
    UPDATE_BEAT.store(now, Ordering::Relaxed);
    REAL_BEAT.store(now, Ordering::Relaxed);
    VIEW_BEAT.store(now, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("stall-watchdog".into())
        .spawn(monitor_loop)
        .map(|_| ())
        .unwrap_or_else(|e| {
            tracing::warn!("stall-watchdog: could not spawn monitor thread: {e}");
        });
}

fn monitor_loop() {
    // When the current stall was first logged, so persistence re-logs at
    // STALL_RELOG_MS and recovery is reported once with its duration.
    let mut update_stall_since: Option<u64> = None;
    let mut last_stall_log: u64 = 0;
    let mut view_stall_since: Option<u64> = None;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        if !crate::logging::is_enabled() {
            update_stall_since = None;
            view_stall_since = None;
            continue;
        }
        let now = now_ms();
        let update_age = now.saturating_sub(UPDATE_BEAT.load(Ordering::Relaxed));
        let real_age = now.saturating_sub(REAL_BEAT.load(Ordering::Relaxed));
        let view_age = now.saturating_sub(VIEW_BEAT.load(Ordering::Relaxed));

        // Layer 1 + 2: the update loop stopped beating. With the 500 ms
        // pacemaker tick subscribed, a healthy loop can never be this
        // quiet, so this is a real stall wherever it sits.
        if update_age >= STALL_MS {
            let since = *update_stall_since.get_or_insert(now.saturating_sub(update_age));
            if now.saturating_sub(last_stall_log) >= STALL_RELOG_MS {
                last_stall_log = now;
                // Sampled AT the stall, not after: a swap storm and a
                // render stall look identical from inside the process,
                // the memory line is what tells them apart.
                let mem = memory_summary();
                match &*lock(&IN_FLIGHT) {
                    Some((entered, name)) => tracing::warn!(
                        "stall-watchdog: UPDATE STALLED for {} ms; handler in flight for {} ms: {name}{mem}\nrecent messages:\n{}",
                        now.saturating_sub(since),
                        now.saturating_sub(*entered),
                        recent_report(now),
                    ),
                    None => tracing::warn!(
                        "stall-watchdog: EVENT LOOP SILENT for {} ms with no handler in flight (last view {} ms ago); \
                         the loop is stuck outside update() (event dispatch / redraw / present){mem}\nrecent messages:\n{}",
                        now.saturating_sub(since),
                        view_age,
                        recent_report(now),
                    ),
                }
            }
        } else if let Some(since) = update_stall_since.take() {
            last_stall_log = 0;
            tracing::warn!(
                "stall-watchdog: recovered after {} ms of update stall",
                now.saturating_sub(since)
            );
        }

        // Layer 3: messages are being processed but view() stopped
        // running. Gated on REAL messages so an idle app (pacemaker
        // ticks only, iced legitimately skips redraws) never trips it.
        if update_age < STALL_MS && view_age >= VIEW_STALL_MS && real_age < VIEW_STALL_MS {
            if view_stall_since.is_none() {
                view_stall_since = Some(now.saturating_sub(view_age));
                tracing::warn!(
                    "stall-watchdog: PRESENTATION STALLED: update alive (last real message {} ms ago) \
                     but view() has not run for {} ms; iced/wgpu is not drawing{}",
                    real_age,
                    view_age,
                    memory_summary(),
                );
            }
        } else if let Some(since) = view_stall_since.take()
            && view_age < VIEW_STALL_MS
        {
            tracing::warn!(
                "stall-watchdog: presentation recovered after {} ms",
                now.saturating_sub(since)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_name_truncates_at_budget() {
        // A paste-sized payload must not bloat the ring: the name stops
        // at the budget plus the ellipsis.
        let big = "x".repeat(64 * 1024);
        let msg = crate::app::Message::OpenUrl(big);
        let name = message_name(&msg);
        assert!(name.len() <= NAME_BUDGET + '…'.len_utf8());
        assert!(name.starts_with("OpenUrl"));
        assert!(name.ends_with('…'));
    }

    /// The ring feeds `recent_report`, which is written into the
    /// debug-log file users attach to issues. A message that carries a
    /// secret must reach it redacted: the payload here is the value of
    /// a text input, so an unwrapped one would record the password one
    /// keystroke at a time, ending with the whole thing.
    #[test]
    fn message_name_never_records_a_secret() {
        let msg = crate::app::Message::Vault(crate::app::VaultMessage::VaultPasswordChanged(
            "hunter2".into(),
        ));
        let name = message_name(&msg);
        assert!(!name.contains("hunter2"), "{name}");
        assert!(name.contains("<redacted>"), "{name}");

        // Same for the text that comes back from the system clipboard:
        // pasting a password into a sudo prompt is the everyday case.
        let pasted = crate::app::Message::Terminal(
            crate::app::TerminalMessage::TerminalPasteResolved(
                uuid::Uuid::nil(),
                Some("hunter2".into()),
            ),
        );
        let name = message_name(&pasted);
        assert!(!name.contains("hunter2"), "{name}");
    }

    #[test]
    fn message_name_keeps_short_names_whole() {
        let name = message_name(&crate::app::Message::NoOp);
        assert_eq!(name, "NoOp");
    }

    #[test]
    fn proc_parsers_read_the_fields() {
        let status = "VmPeak:\t  500000 kB\nVmRSS:\t  431104 kB\nThreads:\t30\n";
        assert_eq!(field_kb(status, "VmRSS:"), Some(431_104));
        assert_eq!(field_kb(status, "MemAvailable:"), None);

        let meminfo = "MemTotal:       32000000 kB\nMemAvailable:   9800000 kB\n\
                       SwapTotal:       8388604 kB\nSwapFree:        8388604 kB\n";
        assert_eq!(field_kb(meminfo, "MemAvailable:"), Some(9_800_000));

        let psi = "some avg10=12.34 avg60=5.00 avg300=1.00 total=123456\n\
                   full avg10=3.00 avg60=1.00 avg300=0.10 total=6543\n";
        assert_eq!(psi_some_avg10(psi).as_deref(), Some("12.34"));
        assert_eq!(psi_some_avg10("garbage"), None);
    }

    /// Live read on Linux: the stall reports depend on this never
    /// erroring, whatever the kernel config; at minimum the process's
    /// own RSS must resolve.
    #[test]
    #[cfg(target_os = "linux")]
    fn memory_summary_reads_proc() {
        let mem = memory_summary();
        assert!(mem.starts_with("; mem"), "{mem}");
        assert!(mem.contains("rss="), "{mem}");
    }

    #[test]
    fn recent_ring_is_capped() {
        {
            let mut recent = lock(&RECENT);
            recent.clear();
            for i in 0..(RECENT_CAP as u64 + 10) {
                if recent.len() == RECENT_CAP {
                    recent.pop_front();
                }
                recent.push_back((i, format!("m{i}"), 0));
            }
            assert_eq!(recent.len(), RECENT_CAP);
            // Oldest entries were dropped, newest kept.
            assert_eq!(recent.front().unwrap().1, "m10");
        }
        let report = recent_report(now_ms());
        assert_eq!(report.lines().count(), RECENT_CAP);
        assert!(report.contains("m10"));
    }
}
