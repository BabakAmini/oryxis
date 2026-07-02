//! `Oryxis::handle_history`: settings-panel-independent dispatch arms for the
//! history area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::too_many_lines)]

use iced::Task;

use crate::app::{Message, Oryxis};

impl Oryxis {
    pub(crate) fn handle_history(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // -- History --
            // Clear now wipes both feeds the unified History timeline
            // mixes (failed-connect log rows + recorded session rows)
            // so the user gets a true "empty list" instead of seeing
            // every previously recorded session reappear after the
            // wipe finishes.
            Message::RequestClearHistory => {
                // Close the `…` overflow menu before the confirm dialog
                // rises (no-op when triggered from the inline button).
                self.overlay = None;
                self.clear_history_confirm = true;
            }
            Message::CancelClearHistory => {
                self.clear_history_confirm = false;
            }
            Message::ClearLogs => {
                self.clear_history_confirm = false;
                if let Some(vault) = &self.vault {
                    let _ = vault.clear_logs();
                    let _ = vault.clear_session_logs();
                    self.logs_page = 0;
                    self.session_logs_page = 0;
                    self.load_data_from_vault();
                }
            }
            Message::LogsPageNext => {
                let max_page = (self.logs_total.saturating_sub(1)) / 50;
                if self.logs_page < max_page {
                    self.logs_page += 1;
                    if let Some(vault) = &self.vault {
                        self.logs = vault
                            .list_logs_page(self.logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }
            Message::LogsPagePrev => {
                if self.logs_page > 0 {
                    self.logs_page -= 1;
                    if let Some(vault) = &self.vault {
                        self.logs = vault
                            .list_logs_page(self.logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }
            Message::ViewSessionLog(log_id) => {
                // Flush buffered output first so viewing a still-active
                // session shows everything recorded up to this moment,
                // not just what was last persisted.
                self.flush_session_logs_final();
                if let Some(vault) = &self.vault
                    && let Ok(Some(data)) = vault.get_session_data(&log_id) {
                        let palette = self.resolve_global_terminal_palette();
                        let spans = crate::ansi_render::render(&data, &palette);
                        self.viewing_session_log = Some((log_id, spans));
                }
            }
            Message::CloseSessionLogView => {
                self.viewing_session_log = None;
            }
            Message::RequestDeleteSessionLog(idx) => {
                let label = self
                    .session_logs
                    .get(idx)
                    .map(|e| e.label.clone())
                    .unwrap_or_default();
                self.error_dialog = Some(crate::state::ErrorDialog {
                    title: crate::i18n::t("log_delete_confirm_title").to_string(),
                    body: format!(
                        "{label}: {}",
                        crate::i18n::t("log_delete_confirm_body")
                    ),
                    link: None,
                    action: Some(crate::state::ErrorDialogAction {
                        label: crate::i18n::t("delete").to_string(),
                        message: Box::new(Message::DeleteSessionLog(idx)),
                        danger: true,
                    }),
                });
            }
            Message::TogglePrivacyReveal => {
                self.privacy_revealed = !self.privacy_revealed;
            }
            Message::LogRowHovered(id) => {
                self.hovered_log_row = Some(id);
            }
            Message::LogRowUnhovered => {
                self.hovered_log_row = None;
            }
            Message::DeleteSessionLog(idx) => {
                if let Some(entry) = self.session_logs.get(idx) {
                    let id = entry.id;
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_session_log(&id);
                        self.session_logs_total =
                            vault.count_session_logs().unwrap_or(0);
                        // Stepping a page back when the current one is now
                        // empty keeps the prev/next pair from leaving the
                        // user staring at "0 of N" with rows further back.
                        let max_page = self
                            .session_logs_total
                            .saturating_sub(1)
                            / 50;
                        if self.session_logs_page > max_page {
                            self.session_logs_page = max_page;
                        }
                        self.session_logs = vault
                            .list_session_logs_page(self.session_logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
                // Close viewer if we deleted the one being viewed
                if let Some((viewed_id, _)) = &self.viewing_session_log
                    && self.session_logs.iter().all(|s| s.id != *viewed_id) {
                        self.viewing_session_log = None;
                }
            }
            Message::ClearSessionLogs => {
                if let Some(vault) = &self.vault {
                    let _ = vault.clear_session_logs();
                    self.session_logs_page = 0;
                    self.load_data_from_vault();
                }
                self.viewing_session_log = None;
            }
            Message::SessionLogsPageNext => {
                let max_page = self.session_logs_total.saturating_sub(1) / 50;
                if self.session_logs_page < max_page {
                    self.session_logs_page += 1;
                    if let Some(vault) = &self.vault {
                        self.session_logs = vault
                            .list_session_logs_page(self.session_logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }
            Message::SessionLogsPagePrev => {
                if self.session_logs_page > 0 {
                    self.session_logs_page -= 1;
                    if let Some(vault) = &self.vault {
                        self.session_logs = vault
                            .list_session_logs_page(self.session_logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }

            Message::OpenUrl(url) => {
                if let Err(e) = crate::util::open_in_browser(&url) {
                    tracing::warn!("open_in_browser({url}) failed: {e}");
                }
            }
            Message::CopyHostPassword(idx) => {
                // Resolve the effective stored password: the linked
                // identity's wins over the connection's own, mirroring
                // what a connect would send.
                self.card_context_menu = None;
                self.overlay = None;
                let Some(conn) = self.connections.get(idx) else {
                    return Ok(Task::none());
                };
                let pw = self.vault.as_ref().and_then(|v| match conn.identity_id {
                    Some(iid) => v.get_identity_password(&iid).ok().flatten(),
                    None => v.get_connection_password(&conn.id).ok().flatten(),
                });
                let Some(pw) = pw else {
                    self.toast = Some(crate::i18n::t("no_password_stored").to_string());
                    return Ok(Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                        },
                        |_| Message::ToastClear,
                    ));
                };
                let mut ok = false;
                if let Ok(mut clip) = arboard::Clipboard::new() {
                    match clip.set_text(pw.clone()) {
                        Ok(()) => ok = true,
                        Err(e) => tracing::warn!("clipboard set_text failed: {e}"),
                    }
                }
                if !ok {
                    return Ok(Task::none());
                }
                // Arm the credential-clear timer. The generation counter
                // makes a superseded timer a no-op instead of clearing a
                // newer copy.
                let secs = self
                    .setting_clipboard_clear_seconds
                    .parse::<u64>()
                    .ok()
                    .filter(|s| *s > 0);
                let toast = if let Some(secs) = secs {
                    crate::i18n::t("copied_clears_in").replace("{secs}", &secs.to_string())
                } else {
                    crate::i18n::t("copied_to_clipboard").to_string()
                };
                self.toast = Some(toast);
                let toast_task = Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                    },
                    |_| Message::ToastClear,
                );
                let Some(secs) = secs else {
                    return Ok(toast_task);
                };
                self.clipboard_clear_gen = self.clipboard_clear_gen.wrapping_add(1);
                self.pending_clipboard_clear = Some(pw);
                let generation = self.clipboard_clear_gen;
                return Ok(Task::batch(vec![
                    toast_task,
                    Task::perform(
                        async move {
                            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                        },
                        move |_| Message::ClipboardClearDue(generation),
                    ),
                ]));
            }
            Message::ClipboardClearDue(generation) => {
                if generation != self.clipboard_clear_gen {
                    return Ok(Task::none()); // superseded by a newer copy
                }
                let Some(expected) = self.pending_clipboard_clear.take() else {
                    return Ok(Task::none());
                };
                // Only clear when the clipboard still holds the credential;
                // whatever the user copied since is theirs to keep.
                if let Ok(mut clip) = arboard::Clipboard::new()
                    && clip.get_text().is_ok_and(|current| current == expected)
                {
                    let _ = clip.clear();
                    tracing::debug!("cleared credential from clipboard");
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
                    self.toast = Some(crate::i18n::t("copied_to_clipboard").to_string());
                    return Ok(Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                        },
                        |_| Message::ToastClear,
                    ));
                }
            }
            Message::ToastClear => {
                self.toast = None;
            }
            Message::ErrorDialogRunAction => {
                if let Some(dialog) = self.error_dialog.take()
                    && let Some(action) = dialog.action
                {
                    return Ok(self.update(*action.message));
                }
            }
            Message::ErrorDialogDismiss => {
                self.error_dialog = None;
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
