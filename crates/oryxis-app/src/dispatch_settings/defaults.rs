//! Settings dispatch helpers: new-host defaults (the Default*
//! family). Split out of dispatch_settings/mod.rs.

use super::*;

impl Oryxis {
    /// New-host default arms: port / auth / identity / key / group /
    /// proxy / encoding / env vars and the collapsed state.
    pub(super) fn handle_settings_defaults(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::Settings(SettingsMessage::ToggleDefaultAgentForwarding) => {
                self.setting_default_agent_forwarding = !self.setting_default_agent_forwarding;
                self.persist_setting(
                    "default_agent_forwarding",
                    if self.setting_default_agent_forwarding { "true" } else { "false" },
                );
            }
            Message::Settings(SettingsMessage::DefaultPortChanged(val)) => {
                self.setting_default_port = sanitize_uint(&val, 65_535);
                self.persist_setting("default_port", &self.setting_default_port);
            }
            Message::Settings(SettingsMessage::DefaultKeepaliveChanged(val)) => {
                // Empty stays empty (= inherit the global keepalive); otherwise
                // digits capped at 1 day.
                self.setting_default_keepalive = if val.trim().is_empty() {
                    String::new()
                } else {
                    sanitize_uint(&val, 86_400)
                };
                self.persist_setting("default_keepalive", &self.setting_default_keepalive);
            }
            Message::Settings(SettingsMessage::DefaultTerminalTypeChanged(val)) => {
                self.setting_default_terminal_type = val;
                self.persist_setting("default_terminal_type", &self.setting_default_terminal_type);
            }
            Message::Settings(SettingsMessage::DefaultUsernameChanged(val)) => {
                self.setting_default_username = val;
                self.persist_setting("default_username", &self.setting_default_username);
            }
            Message::Settings(SettingsMessage::DefaultAuthMethodChanged(val)) => {
                self.setting_default_auth_method = crate::util::auth_method_from_label(&val);
                self.persist_setting(
                    "default_auth_method",
                    &crate::util::auth_method_to_setting(&self.setting_default_auth_method),
                );
            }
            Message::Settings(SettingsMessage::DefaultIdentityChanged(val)) => {
                // The picker emits a saved identity's label or the localized
                // "(none)" sentinel; map back to the stable UUID.
                self.setting_default_identity_id = if val == crate::i18n::t("new_default_none") {
                    None
                } else {
                    self.identities.iter().find(|i| i.label == val).map(|i| i.id)
                };
                self.persist_setting(
                    "default_identity_id",
                    &self.setting_default_identity_id.map(|id| id.to_string()).unwrap_or_default(),
                );
            }
            Message::Settings(SettingsMessage::DefaultKeyChanged(val)) => {
                self.setting_default_key_id = if val == crate::i18n::t("new_default_none") {
                    None
                } else {
                    self.keys.iter().find(|k| k.label == val).map(|k| k.id)
                };
                self.persist_setting(
                    "default_key_id",
                    &self.setting_default_key_id.map(|id| id.to_string()).unwrap_or_default(),
                );
            }
            Message::Settings(SettingsMessage::DefaultGroupChanged(val)) => {
                self.setting_default_group_id = if val == crate::i18n::t("new_default_none") {
                    None
                } else {
                    self.groups.iter().find(|g| g.label == val).map(|g| g.id)
                };
                self.persist_setting(
                    "default_group_id",
                    &self.setting_default_group_id.map(|id| id.to_string()).unwrap_or_default(),
                );
            }
            Message::Settings(SettingsMessage::DefaultProxyChanged(val)) => {
                // Default proxy is a saved Proxy Identity reference; inline
                // proxies are per-host by nature and aren't defaulted.
                self.setting_default_proxy_identity_id =
                    if val == crate::i18n::t("new_default_none") {
                        None
                    } else {
                        self.proxy_identities.iter().find(|p| p.label == val).map(|p| p.id)
                    };
                self.persist_setting(
                    "default_proxy_identity_id",
                    &self
                        .setting_default_proxy_identity_id
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                );
            }
            Message::Settings(SettingsMessage::ToggleDefaultMcpEnabled) => {
                self.setting_default_mcp_enabled = !self.setting_default_mcp_enabled;
                self.persist_setting(
                    "default_mcp_enabled",
                    if self.setting_default_mcp_enabled { "true" } else { "false" },
                );
            }
            Message::Settings(SettingsMessage::DefaultEncodingChanged(val)) => {
                // "UTF-8" maps to None (no override), like the host editor.
                self.setting_default_encoding = if val == "UTF-8" { None } else { Some(val) };
                self.persist_setting(
                    "default_encoding",
                    self.setting_default_encoding.as_deref().unwrap_or(""),
                );
            }
            Message::Settings(SettingsMessage::DefaultAddEnvVar) => {
                // A fresh blank row, persisted on the first keystroke (a
                // blank-key row is dropped by `env_vars_to_setting`).
                self.setting_default_env_vars.push(crate::state::EnvVarForm::default());
            }
            Message::Settings(SettingsMessage::DefaultRemoveEnvVar(idx)) => {
                if idx < self.setting_default_env_vars.len() {
                    self.setting_default_env_vars.remove(idx);
                }
                self.persist_setting(
                    "default_env_vars",
                    &crate::util::env_vars_to_setting(&self.setting_default_env_vars),
                );
            }
            Message::Settings(SettingsMessage::DefaultEnvVarKeyChanged(idx, val)) => {
                if let Some(e) = self.setting_default_env_vars.get_mut(idx) {
                    e.key = val;
                }
                self.persist_setting(
                    "default_env_vars",
                    &crate::util::env_vars_to_setting(&self.setting_default_env_vars),
                );
            }
            Message::Settings(SettingsMessage::DefaultEnvVarValueChanged(idx, val)) => {
                if let Some(e) = self.setting_default_env_vars.get_mut(idx) {
                    e.value = val;
                }
                self.persist_setting(
                    "default_env_vars",
                    &crate::util::env_vars_to_setting(&self.setting_default_env_vars),
                );
            }
            Message::Settings(SettingsMessage::ToggleDefaultsCollapsed) => {
                self.setting_defaults_collapsed = !self.setting_defaults_collapsed;
                self.persist_setting(
                    "defaults_collapsed",
                    if self.setting_defaults_collapsed { "true" } else { "false" },
                );
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }
}
