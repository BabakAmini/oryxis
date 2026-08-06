//! Host-mediated system-clipboard access.
//!
//! Nothing in this crate touches the system clipboard directly. Every copy /
//! paste request is queued here and drained by the host (`oryxis-app`), which
//! performs the operation through the iced runtime.
//!
//! Why: the Win32 clipboard is a per-process resource whose ownership is
//! per-thread. `iced`'s `text_input` pastes by asking the runtime to read the
//! clipboard, and the runtime serves that read on a worker thread
//! (`iced_winit::clipboard::Clipboard::read` -> `thread::spawn` -> arboard).
//! A second `arboard` call from the UI thread at the same moment gives one
//! thread an `HGLOBAL` the other has already released; `GlobalSize` on the
//! dead handle raises `STATUS_HEAP_CORRUPTION` inside
//! `user32!GetClipboardData` and the process dies on the spot: no unwinding,
//! no panic hook, no log line.
//!
//! That is not theoretical. Field crash 2026-07-29 (Ctrl+V in the SFTP path
//! bar, Windows): the minidump has both the main thread and the runtime's
//! clipboard thread inside `GetClipboardData(CF_UNICODETEXT)`, one of them
//! down in `wtdccm.dll` -> `GlobalSize` -> `RtlpHeapHandleError`.
//!
//! Routing everything through the runtime keeps exactly one clipboard
//! operation in flight per process, serialized by the runtime's own mutex.

use std::sync::{Arc, Mutex, OnceLock};

/// Something the host should do with the system clipboard on this crate's
/// behalf.
pub enum ClipboardRequest {
    /// Write this text to the system clipboard (copy-on-select, the copy
    /// chord, right-click copy, OSC 52 store).
    Write(String),
    /// Publish this text as the system PRIMARY selection (finishing a
    /// selection with the mouse). Only ever queued where a PRIMARY
    /// selection exists, see [`write_primary_text`].
    WritePrimary(String),
    /// Read the system clipboard and hand the text to this sink (OSC 52
    /// load, the widget's own paste fallback).
    Read(ClipboardSink),
}

/// Where the text of a [`ClipboardRequest::Read`] goes once the host has it.
///
/// The closure is owned by this crate, so the host never has to know whether
/// a read belongs to an OSC 52 reply or to a paste: it reads, delivers, done.
pub struct ClipboardSink(Box<dyn Fn(&str) + Send + Sync>);

impl ClipboardSink {
    pub(crate) fn new(f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self(Box::new(f))
    }

    /// Hand the clipboard text to the waiting consumer.
    pub fn deliver(&self, text: &str) {
        (self.0)(text);
    }
}

impl std::fmt::Debug for ClipboardRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Write(text) => write!(f, "Write({} chars)", text.chars().count()),
            Self::WritePrimary(text) => {
                write!(f, "WritePrimary({} chars)", text.chars().count())
            }
            Self::Read(_) => write!(f, "Read(..)"),
        }
    }
}

fn queue() -> &'static Mutex<Vec<ClipboardRequest>> {
    static QUEUE: OnceLock<Mutex<Vec<ClipboardRequest>>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Queue a request for the host to perform. Silently drops it if the queue
/// mutex is poisoned: a missed copy is a non-event, and this runs on the UI
/// thread and on the emulator's event path, neither of which may panic.
pub(crate) fn request(req: ClipboardRequest) {
    if let Ok(mut q) = queue().lock() {
        // Bound the queue so a runaway remote (an OSC 52 flood) can't grow it
        // without limit when no host is draining. 64 is far above any real
        // burst: a human copy is one entry, and the host drains every update.
        const MAX_PENDING: usize = 64;
        if q.len() >= MAX_PENDING {
            let _ = q.remove(0);
        }
        q.push(req);
    }
}

/// Drain every pending request. Called by the host once per dispatch cycle.
///
/// A request therefore lands one `update()` after it was queued, so a gesture
/// that queues a copy has to produce a message. Every current one does: mouse
/// release, key presses and PTY output all reach the host's dispatcher, and
/// the right-click press (which the widget captures without publishing) is
/// mapped to a no-op message in the host's global event subscription for
/// exactly this reason.
pub fn take_clipboard_requests() -> Vec<ClipboardRequest> {
    match queue().lock() {
        Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
        _ => Vec::new(),
    }
}

/// Queue a "copy this text" request. The one entry point for every copy in
/// this crate (`widget::set_clipboard_text` is a thin alias kept for the call
/// sites that read better that way).
pub(crate) fn write_text(text: &str) {
    if text.is_empty() {
        return;
    }
    request(ClipboardRequest::Write(text.to_string()));
}

/// Queue a "publish this as the PRIMARY selection" request.
///
/// A no-op off X11 / Wayland, and deliberately so: only there is PRIMARY a
/// separate buffer. Everywhere else the runtime serves a PRIMARY write from
/// the ordinary clipboard, so publishing here would wipe the user's Ctrl+C
/// every time they highlighted a word.
pub(crate) fn write_primary_text(text: &str) {
    if !cfg!(target_os = "linux") || text.is_empty() {
        return;
    }
    request(ClipboardRequest::WritePrimary(text.to_string()));
}

/// Whether this platform has a PRIMARY selection of its own. Gates both
/// halves of the feature, so the widget and the host can't disagree.
pub fn has_primary_selection() -> bool {
    cfg!(target_os = "linux")
}

/// Queue a "read the clipboard, then run this" request.
pub(crate) fn read_text(sink: impl Fn(&str) + Send + Sync + 'static) {
    request(ClipboardRequest::Read(ClipboardSink::new(sink)));
}

/// Convenience for the paste paths: read the clipboard and write it into
/// `state`'s PTY, bracketed-paste-wrapped when the focused app asked for it.
///
/// Used only by the widget's fallback paths (hosts that wire
/// `on_paste_request` never reach them), so the wrap decision is taken at
/// delivery time, when the mode is current.
pub(crate) fn paste_into(state: Arc<Mutex<crate::widget::TerminalState>>) {
    read_text(move |text| {
        if text.is_empty() {
            return;
        }
        if let Ok(mut state) = state.lock() {
            let bracketed = state.bracketed_paste_enabled();
            state.write(&crate::wrap_paste(text, bracketed));
        }
    });
}

/// Serializes the tests that touch the process-wide queue (this module's own
/// and the backend's OSC 52 cases) and hands them a drained queue. Cargo runs
/// tests in threads, so without this they would steal each other's requests.
#[cfg(test)]
pub(crate) fn test_exclusive() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _ = take_clipboard_requests();
    guard
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        super::test_exclusive()
    }

    #[test]
    fn write_queues_text_and_take_drains_once() {
        let _serial = exclusive();
        write_text("hello");
        let reqs = take_clipboard_requests();
        assert_eq!(reqs.len(), 1);
        match &reqs[0] {
            ClipboardRequest::Write(text) => assert_eq!(text, "hello"),
            other => panic!("expected a write, got {other:?}"),
        }
        assert!(take_clipboard_requests().is_empty(), "a drained queue stays empty");
    }

    #[test]
    fn empty_copies_are_never_queued() {
        let _serial = exclusive();
        write_text("");
        assert!(take_clipboard_requests().is_empty());
    }

    #[test]
    fn primary_writes_are_queued_only_where_primary_exists() {
        let _serial = exclusive();
        write_primary_text("selected");
        let reqs = take_clipboard_requests();

        if has_primary_selection() {
            assert_eq!(reqs.len(), 1);
            match &reqs[0] {
                ClipboardRequest::WritePrimary(text) => assert_eq!(text, "selected"),
                other => panic!("expected a primary write, got {other:?}"),
            }
        } else {
            // Off X11 / Wayland the runtime would serve this from the
            // ordinary clipboard, so every selection would clobber the
            // user's Ctrl+C. Queueing nothing is the whole guard.
            assert!(reqs.is_empty(), "primary write leaked to a platform without PRIMARY");
        }

        write_primary_text("");
        assert!(take_clipboard_requests().is_empty(), "empty selections are never published");
    }

    #[test]
    fn read_sink_delivers_the_text_it_is_handed() {
        let _serial = exclusive();
        let seen = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&seen);
        read_text(move |text| {
            if let Ok(mut s) = sink.lock() {
                *s = text.to_string();
            }
        });
        let reqs = take_clipboard_requests();
        assert_eq!(reqs.len(), 1);
        match &reqs[0] {
            ClipboardRequest::Read(sink) => sink.deliver("pasted"),
            other => panic!("expected a read, got {other:?}"),
        }
        assert_eq!(seen.lock().unwrap().as_str(), "pasted");
    }

    #[test]
    fn the_queue_is_bounded_and_drops_the_oldest() {
        let _serial = exclusive();
        for i in 0..70 {
            write_text(&format!("{i}"));
        }
        let reqs = take_clipboard_requests();
        assert_eq!(reqs.len(), 64, "queue is capped");
        match &reqs[0] {
            // 0..=5 were evicted to make room for 64..=69.
            ClipboardRequest::Write(text) => assert_eq!(text, "6"),
            other => panic!("expected a write, got {other:?}"),
        }
    }
}
