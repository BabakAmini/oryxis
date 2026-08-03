//! `Oryxis::handle_global`: cross-cutting message arms handled outside
//! any single domain (clipboard, URL open, toasts, error dialog, privacy
//! reveal toggle, no-op). Routed here explicitly from `dispatch_message`.
#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]

use iced::Task;

use crate::app::{Message, Oryxis};

/// Read the system clipboard as text, best-effort, as a `Task`.
///
/// The single read entry point for the whole app. Reads MUST go through the
/// iced runtime: it serves them one at a time on its own worker thread, and a
/// second concurrent clipboard open in the same process is fatal on Windows
/// (`STATUS_HEAP_CORRUPTION` inside `user32!GetClipboardData`, killing the
/// process with no panic and no log). Never call `arboard` directly.
/// See `oryxis_terminal::host_clipboard` for the field crash.
pub(crate) fn read_clipboard_text(
    to_message: impl Fn(Option<String>) -> Message + Send + 'static,
) -> Task<Message> {
    iced::clipboard::read_text().map(move |result| {
        to_message(result.ok().map(|text| text.to_string()))
    })
}

/// Write `text` to the system clipboard as a `Task`. Same rule as
/// [`read_clipboard_text`]: the runtime owns the clipboard, we only ask.
pub(crate) fn write_clipboard_text(text: String) -> Task<Message> {
    iced::clipboard::write(text).discard()
}

/// Perform the clipboard work `oryxis-terminal` queued for us (copy-on-select,
/// the copy chord, right-click copy, OSC 52 store / load, the widget's paste
/// fallbacks). Drained once per `update()` so the widget layer never needs a
/// clipboard of its own.
///
/// The requests are `Task::chain`ed, not batched: a batch runs its streams
/// concurrently, which would let a queued read answer before a queued write
/// has landed. Order matters because a remote can emit an OSC 52 store and an
/// OSC 52 query inside the same output batch (tmux / vim clipboard
/// integration does), and the query must see the text the store just set.
pub(crate) fn serve_terminal_clipboard_requests() -> Option<Task<Message>> {
    use oryxis_terminal::ClipboardRequest;

    oryxis_terminal::take_clipboard_requests()
        .into_iter()
        .map(|request| match request {
            ClipboardRequest::Write(text) => write_clipboard_text(text),
            // The sink is the terminal crate's own closure (an OSC 52 reply,
            // a PTY paste); it only formats text and writes bytes to a
            // channel, so running it in the task chain is safe.
            ClipboardRequest::Read(sink) => iced::clipboard::read_text().then(move |result| {
                let text = result.map(|t| t.to_string()).unwrap_or_default();
                sink.deliver(&text);
                Task::none()
            }),
        })
        .reduce(Task::chain)
}

impl Oryxis {
    pub(crate) fn handle_global(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TogglePrivacyReveal => {
                self.privacy.revealed = !self.privacy.revealed;
            }

            Message::OpenUrl(url) => {
                if let Err(e) = crate::util::open_in_browser(&url) {
                    tracing::warn!("open_in_browser({url}) failed: {e}");
                }
            }
            Message::CopyToClipboard(content) => {
                // The write is served by the runtime (one clipboard access per
                // process at a time), so its result arrives as
                // `ClipboardWritten` and the toast waits for it: a failed copy
                // must not claim success.
                return iced::clipboard::write(content)
                    .map(|result| Message::ClipboardWritten(result.is_ok()));
            }
            Message::ClipboardWritten(ok) => {
                if !ok {
                    tracing::warn!("clipboard write failed");
                    return Task::none();
                }
                self.set_toast(crate::i18n::t("copied_to_clipboard").to_string());
                return Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                    },
                    |_| Message::ToastClear,
                );
            }

            Message::ToastClear => {
                // Deadline-guarded auto-dismiss (subscription tick or a
                // legacy scheduled sleep-timer). Only the current toast's
                // own elapsed deadline clears it, so a superseded timer can
                // never wipe a newer toast.
                if self
                    .toast_deadline
                    .is_some_and(|d| std::time::Instant::now() >= d)
                {
                    self.toast = None;
                    self.toast_deadline = None;
                }
            }

            Message::ToastDismiss => {
                // Explicit click on the chip: clear immediately.
                self.toast = None;
                self.toast_deadline = None;
            }

            Message::ErrorDialogRunAction => {
                if let Some(dialog) = self.error_dialog.take()
                    && let Some(action) = dialog.action
                {
                    return self.update(*action.message);
                }
            }

            Message::ErrorDialogDismiss => {
                self.error_dialog = None;
            }
            Message::NoOp => {}

            // `update` lists the cross-cutting globals explicitly, so
            // anything else here is a routing mistake, not a runtime
            // case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
