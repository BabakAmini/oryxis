//! `Oryxis::handle_global`: cross-cutting message arms handled outside
//! any single domain (clipboard, URL open, toasts, error dialog, privacy
//! reveal toggle, no-op). Routed here explicitly from `dispatch_message`.
#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]

use iced::Task;

use crate::app::{Message, Oryxis};

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
                let mut ok = false;
                if let Ok(mut clip) = arboard::Clipboard::new() {
                    match clip.set_text(content) {
                        Ok(()) => ok = true,
                        Err(e) => tracing::warn!("clipboard set_text failed: {e}"),
                    }
                }
                if ok {
                    self.set_toast(crate::i18n::t("copied_to_clipboard").to_string());
                    return Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                        },
                        |_| Message::ToastClear,
                    );
                }
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

            _ => {}
        }
        Task::none()
    }
}
