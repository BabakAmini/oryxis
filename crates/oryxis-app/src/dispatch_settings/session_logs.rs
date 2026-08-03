//! Session recording and connection history: what is captured, how long
//! it is kept, and whether the host is probed for its OS.
//!
//! Handled here rather than in `dispatch_ssh` because they are settings
//! rows; the capture itself lives with the session (CLAUDE.md's
//! name-family rule).

use super::*;

impl Oryxis {
    pub(super) fn handle_settings_session_logs(
        &mut self,
        message: SettingsMessage,
    ) -> Result<Task<Message>, SettingsMessage> {
        match message {
            SettingsMessage::SettingToggleSessionLogging => {
                self.prefs.session_logging = !self.prefs.session_logging;
                self.persist_setting(
                    "session_logging",
                    if self.prefs.session_logging { "true" } else { "false" },
                );
            }
            SettingsMessage::SettingToggleSessionLogFull => {
                self.prefs.session_log_full = !self.prefs.session_log_full;
                self.persist_setting(
                    "session_log_full",
                    if self.prefs.session_log_full { "true" } else { "false" },
                );
            }
            SettingsMessage::SettingToggleSessionLogCompress => {
                self.prefs.session_log_compress = !self.prefs.session_log_compress;
                self.persist_setting(
                    "session_log_compress",
                    if self.prefs.session_log_compress { "true" } else { "false" },
                );
            }
            SettingsMessage::SettingToggleConnectionHistory => {
                self.prefs.connection_history = !self.prefs.connection_history;
                self.persist_setting(
                    "connection_history",
                    if self.prefs.connection_history { "true" } else { "false" },
                );
            }
            SettingsMessage::LogsRetentionChanged(code) => {
                self.prefs.logs_retention = code.to_string();
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
            SettingsMessage::SettingToggleOsDetection => {
                self.prefs.os_detection = !self.prefs.os_detection;
                self.persist_setting(
                    "os_detection",
                    if self.prefs.os_detection { "true" } else { "false" },
                );
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }
}
