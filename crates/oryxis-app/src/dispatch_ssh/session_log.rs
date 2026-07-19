//! Session-logging settings, split out of `dispatch_ssh`: the
//! session-recording / connection-history toggles and the logs
//! retention window (with its immediate prune). Called from
//! `handle_ssh`.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{SettingsMessage, Message, Oryxis};

impl Oryxis {
    pub(super) fn handle_ssh_session_log(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::Settings(SettingsMessage::SettingToggleSessionLogging) => {
                self.setting_session_logging = !self.setting_session_logging;
                self.persist_setting(
                    "session_logging",
                    if self.setting_session_logging { "true" } else { "false" },
                );
            }
            Message::Settings(SettingsMessage::SettingToggleSessionLogFull) => {
                self.setting_session_log_full = !self.setting_session_log_full;
                self.persist_setting(
                    "session_log_full",
                    if self.setting_session_log_full { "true" } else { "false" },
                );
            }
            Message::Settings(SettingsMessage::SettingToggleSessionLogCompress) => {
                self.setting_session_log_compress = !self.setting_session_log_compress;
                self.persist_setting(
                    "session_log_compress",
                    if self.setting_session_log_compress { "true" } else { "false" },
                );
            }
            Message::Settings(SettingsMessage::SettingToggleConnectionHistory) => {
                self.setting_connection_history = !self.setting_connection_history;
                self.persist_setting(
                    "connection_history",
                    if self.setting_connection_history { "true" } else { "false" },
                );
            }
            Message::Settings(SettingsMessage::LogsRetentionChanged(code)) => {
                self.setting_logs_retention = code.to_string();
                self.persist_setting("logs_retention", code);
                // Apply right away so picking a shorter window has a
                // visible effect, then refresh the cached Logs state.
                if let Some(days) = Self::retention_days(code)
                    && let Some(vault) = &self.vault
                {
                    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
                    match vault.prune_logs_older_than(cutoff) {
                        Ok(0) => {}
                        Ok(n) => tracing::info!("logs retention pruned {n} rows"),
                        Err(e) => tracing::warn!("logs retention prune failed: {e}"),
                    }
                    self.logs_page = 0;
                    self.session_logs_page = 0;
                    self.logs_total = vault.count_logs().unwrap_or(0);
                    self.logs = vault.list_logs_page(0, 50).unwrap_or_default();
                    self.session_logs_total = vault.count_session_logs().unwrap_or(0);
                    self.session_logs =
                        vault.list_session_logs_page(0, 50).unwrap_or_default();
                }
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
