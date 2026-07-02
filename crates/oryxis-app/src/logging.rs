//! Optional debug logging to a file (Settings > Advanced) plus the
//! environment report copied into GitHub issues.
//!
//! A second `tracing_subscriber::fmt` layer (installed in `main.rs`)
//! writes through [`DebugFileWriter`], which forwards every formatted
//! event to a process-global file sink while the feature is on and
//! discards the bytes otherwise. The sink flips at runtime from the
//! Settings toggle without rebuilding the subscriber, and `main.rs`
//! arms it before the subscriber is built (the `debug_logging` setting
//! reads without the master password) so boot lines are captured too.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Fast path checked on every formatted event so the layer costs one
/// relaxed load while the feature is off (the common case).
static ENABLED: AtomicBool = AtomicBool::new(false);

/// The open log file while enabled. A single guarded handle (instead of
/// reopening per write) so enable/disable/clear and the tracing layer
/// never race on a half-open file.
static SINK: Mutex<Option<File>> = Mutex::new(None);

/// Rotation threshold: a debug session left on for weeks must not eat
/// the disk. 5 MB of plain text is plenty of history for an issue.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Lock the sink, shrugging off poisoning: a panic while holding the
/// lock leaves at worst a partially written log line, and diagnostics
/// must never take the app down with them.
fn sink() -> MutexGuard<'static, Option<File>> {
    SINK.lock().unwrap_or_else(|e| e.into_inner())
}

/// `~/.oryxis/oryxis-debug.log`, next to the vault. Self-describing
/// name so the file still identifies itself when attached to an issue.
pub(crate) fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".oryxis").join("oryxis-debug.log"))
}

/// Sibling the oversized log rotates aside to on enable.
fn rotated_path(path: &Path) -> PathBuf {
    path.with_extension("log.old")
}

pub(crate) fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Open (or create) the log file, write a session header and start
/// forwarding tracing events to it. Idempotent while already enabled.
pub(crate) fn enable() -> io::Result<PathBuf> {
    let path = log_path().ok_or_else(|| io::Error::other("no home directory"))?;
    enable_at(&path)?;
    Ok(path)
}

fn enable_at(path: &Path) -> io::Result<()> {
    let mut guard = sink();
    if guard.is_some() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    rotate_if_oversized(path, MAX_LOG_BYTES);
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    write_session_header(&mut file)?;
    *guard = Some(file);
    ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

/// Move a grown log aside instead of truncating it, so one long session
/// doesn't destroy the history that made it interesting. Best-effort:
/// a failed rename just means the file keeps growing for now.
fn rotate_if_oversized(path: &Path, max_bytes: u64) {
    let oversized = std::fs::metadata(path).map(|m| m.len() > max_bytes).unwrap_or(false);
    if oversized {
        let _ = std::fs::rename(path, rotated_path(path));
    }
}

/// Stop forwarding events and close the file. The file itself stays on
/// disk so it can still be attached to an issue after switching off.
pub(crate) fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
    if let Some(mut file) = sink().take() {
        let _ = file.flush();
    }
}

/// Wipe the log (live handle truncated in place, otherwise the file and
/// any rotated leftover are deleted). Returns `false` when there was
/// nothing to clear.
pub(crate) fn clear() -> io::Result<bool> {
    let Some(path) = log_path() else {
        return Ok(false);
    };
    let removed_old = std::fs::remove_file(rotated_path(&path)).is_ok();
    let mut guard = sink();
    if let Some(file) = guard.as_mut() {
        // Append-mode writes land at the new end (offset 0), so a
        // truncate through the live handle is enough. Re-stamp the
        // header so the surviving file still says what produced it.
        file.set_len(0)?;
        write_session_header(file)?;
        return Ok(true);
    }
    drop(guard);
    if path.exists() {
        std::fs::remove_file(&path)?;
        return Ok(true);
    }
    Ok(removed_old)
}

/// Each enable (and each boot while the setting is on) opens with a
/// timestamped banner plus the environment block, so a log spanning
/// several sessions stays legible and always carries the system info
/// the issue template asks for.
fn write_session_header(file: &mut File) -> io::Result<()> {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z");
    writeln!(file, "==== Oryxis debug session started {now} ====")?;
    for line in environment_report(None).lines() {
        writeln!(file, "  {line}")?;
    }
    file.flush()
}

/// `MakeWriter` handed to the file `fmt` layer in `main.rs`. Zero-sized;
/// all state lives in the module statics so the writer created per event
/// is free.
#[derive(Clone, Copy)]
pub(crate) struct DebugFileWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DebugFileWriter {
    type Writer = DebugFileWriter;

    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

impl Write for DebugFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if ENABLED.load(Ordering::Relaxed)
            && let Some(file) = sink().as_mut()
        {
            // Swallow write errors: a full disk must never crash the
            // app through its own diagnostics.
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = sink().as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

/// The plain-text environment block: shown in Settings > Advanced,
/// copied to the clipboard for GitHub issues, and stamped into every
/// debug-log session header. `renderer` is the lazily loaded
/// `(backend, adapter)` pair from app state; the line is omitted while
/// it hasn't resolved (e.g. the log header written during boot).
pub(crate) fn environment_report(renderer: Option<&(String, String)>) -> String {
    let channel = match crate::update::build_channel() {
        crate::update::UpdateChannel::Stable => "stable",
        crate::update::UpdateChannel::Nightly => "nightly",
    };
    let sha: String = env!("ORYXIS_GIT_SHA").chars().take(7).collect();
    let mut lines = vec![
        format!("Oryxis: v{} ({channel}, {sha})", env!("CARGO_PKG_VERSION")),
        format!("OS: {}", os_summary()),
    ];
    #[cfg(target_os = "linux")]
    {
        // Wayland-vs-X11 and the desktop in play decide which renderer
        // and clipboard quirks apply, worth one line on Linux.
        let session =
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
        match std::env::var("XDG_CURRENT_DESKTOP") {
            Ok(desktop) if !desktop.is_empty() => {
                lines.push(format!("Display: {session}, {desktop}"));
            }
            _ => lines.push(format!("Display: {session}")),
        }
    }
    #[cfg(target_os = "windows")]
    lines.push(format!(
        "Install: {}",
        if crate::update::is_per_user_install() { "per-user" } else { "system" }
    ));
    if let Some((backend, adapter)) = renderer {
        lines.push(format!("Renderer: {backend}, {adapter}"));
    }
    lines.push(format!("Language: {}", crate::i18n::Language::active().code()));
    lines.join("\n")
}

/// OS name + version + arch, resolved once per process (`os_info::get`
/// reads platform sources, not something to repeat on every redraw of
/// the settings view).
fn os_summary() -> &'static str {
    static SUMMARY: OnceLock<String> = OnceLock::new();
    SUMMARY.get_or_init(|| {
        let info = os_info::get();
        let mut summary =
            format!("{} {} ({})", info.os_type(), info.version(), std::env::consts::ARCH);
        #[cfg(target_os = "linux")]
        {
            // The kernel string tells the WSL / distro-kernel stories
            // the os-release name doesn't.
            if let Ok(out) = std::process::Command::new("uname").arg("-r").output()
                && out.status.success()
            {
                let kernel = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !kernel.is_empty() {
                    summary.push_str(&format!(", kernel {kernel}"));
                }
            }
        }
        summary
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One sequential test for the whole lifecycle: the sink statics are
    /// process-global, so parallel test fns would trample each other.
    #[test]
    fn debug_log_lifecycle() {
        let dir = std::env::temp_dir().join(format!("oryxis-logging-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("oryxis-debug.log");

        // Disabled: the writer discards without erroring.
        assert!(!is_enabled());
        assert_eq!(DebugFileWriter.write(b"dropped\n").unwrap(), 8);

        // Enable creates the dir + file and stamps the session header
        // with the environment block.
        enable_at(&path).unwrap();
        assert!(is_enabled());
        let header = std::fs::read_to_string(&path).unwrap();
        assert!(header.contains("Oryxis debug session started"));
        assert!(header.contains(concat!("Oryxis: v", env!("CARGO_PKG_VERSION"))));

        // Enabled: writes land in the file. Re-enabling is a no-op.
        DebugFileWriter.write_all(b"line-a\n").unwrap();
        enable_at(&path).unwrap();
        DebugFileWriter.write_all(b"line-b\n").unwrap();
        DebugFileWriter.flush().unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("line-a"));
        assert!(body.contains("line-b"));

        // Clear through the live handle truncates and re-stamps the header.
        // (clear() resolves the real home-dir path, so exercise the same
        // truncate-and-restamp branch directly against the test file.)
        {
            let mut guard = sink();
            let file = guard.as_mut().unwrap();
            file.set_len(0).unwrap();
            write_session_header(file).unwrap();
        }
        let cleared = std::fs::read_to_string(&path).unwrap();
        assert!(cleared.contains("Oryxis debug session started"));
        assert!(!cleared.contains("line-a"));

        // Disable closes the sink and the writer goes back to discarding.
        disable();
        assert!(!is_enabled());
        let before = std::fs::read_to_string(&path).unwrap();
        DebugFileWriter.write_all(b"after-disable\n").unwrap();
        assert_eq!(before, std::fs::read_to_string(&path).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_moves_oversized_log_aside() {
        let dir =
            std::env::temp_dir().join(format!("oryxis-rotate-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("oryxis-debug.log");

        std::fs::write(&path, b"0123456789").unwrap();
        rotate_if_oversized(&path, 4);
        assert!(!path.exists());
        assert_eq!(std::fs::read(rotated_path(&path)).unwrap(), b"0123456789");

        // Under the threshold nothing moves.
        std::fs::write(&path, b"abc").unwrap();
        rotate_if_oversized(&path, 4);
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn environment_report_shape() {
        let report = environment_report(Some(&("Vulkan".to_string(), "Test GPU".to_string())));
        assert!(report.contains(concat!("Oryxis: v", env!("CARGO_PKG_VERSION"))));
        assert!(report.contains("OS: "));
        assert!(report.contains("Renderer: Vulkan, Test GPU"));
        assert!(report.contains("Language: "));
        // The renderer line is omitted, not left dangling, while unknown.
        assert!(!environment_report(None).contains("Renderer:"));
    }
}
