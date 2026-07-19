//! `Oryxis::handle_mcp`: settings-panel-independent dispatch arms for the
//! mcp area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::too_many_lines)]

use iced::Task;

use crate::app::{McpMessage, PluginMessage, Message, Oryxis};
use crate::mcp::{install_mcp_config_to_file, install_mcp_config_to_wsl, mcp_config_json, mcp_config_json_wsl};
use crate::state::{EnvVarForm, PortForwardForm};

impl Oryxis {
    pub(crate) fn handle_mcp(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // ── MCP ──
            Message::EditorToggleMcpEnabled => {
                self.editor_form.mcp_enabled = !self.editor_form.mcp_enabled;
            }
            Message::EditorToggleAgentForwarding => {
                self.editor_form.agent_forwarding = !self.editor_form.agent_forwarding;
            }
            // Cycle the per-host recording override: Default (inherit the
            // global setting) -> On -> Off -> Default.
            Message::EditorCycleSessionLogging => {
                self.editor_form.session_logging = match self.editor_form.session_logging {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
            }
            Message::EditorAddPortForward => {
                self.editor_form.port_forwards.push(PortForwardForm::default());
            }
            Message::EditorRemovePortForward(i) => {
                if i < self.editor_form.port_forwards.len() {
                    self.editor_form.port_forwards.remove(i);
                }
            }
            Message::EditorPortFwdLocalPortChanged(i, v) => {
                if let Some(pf) = self.editor_form.port_forwards.get_mut(i) {
                    pf.local_port = v;
                }
            }
            Message::EditorPortFwdRemoteHostChanged(i, v) => {
                if let Some(pf) = self.editor_form.port_forwards.get_mut(i) {
                    pf.remote_host = v;
                }
            }
            Message::EditorPortFwdRemotePortChanged(i, v) => {
                if let Some(pf) = self.editor_form.port_forwards.get_mut(i) {
                    pf.remote_port = v;
                }
            }
            Message::EditorAddEnvVar => {
                self.editor_form.env_vars.push(EnvVarForm::default());
            }
            Message::EditorRemoveEnvVar(i) => {
                if i < self.editor_form.env_vars.len() {
                    self.editor_form.env_vars.remove(i);
                }
            }
            Message::EditorEnvVarKeyChanged(i, v) => {
                if let Some(e) = self.editor_form.env_vars.get_mut(i) {
                    e.key = v;
                }
            }
            Message::EditorEnvVarValueChanged(i, v) => {
                if let Some(e) = self.editor_form.env_vars.get_mut(i) {
                    e.value = v;
                }
            }
            Message::Mcp(McpMessage::ToggleMcpServer) => {
                self.mcp.server_enabled = !self.mcp.server_enabled;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("mcp_server_enabled", if self.mcp.server_enabled { "true" } else { "false" });
                }
                // MCP ships as a plugin (~5 MB binary external clients
                // like Claude Desktop spawn). First-time enable triggers
                // the install modal; an already-installed plugin or a
                // dev binary on the side both make this a no-op.
                if self.mcp.server_enabled
                    && !crate::mcp_install::is_installed()
                    && !crate::dispatch_plugins::dev_binary_present("mcp")
                {
                    return Ok(Task::done(Message::Plugin(PluginMessage::ShowPluginInstallModal(
                        "mcp".to_string(),
                    ))));
                }
            }
            Message::Mcp(McpMessage::ShowMcpInfo) => {
                self.mcp.show_info = true;
                self.mcp.config_copied = false;
            }
            Message::Mcp(McpMessage::HideMcpInfo) => {
                self.mcp.show_info = false;
                self.mcp.config_copied = false;
            }
            Message::Mcp(McpMessage::CopyMcpConfig) => {
                self.mcp.config_copied = true;
                let vault_pw = self.mcp_vault_pw();
                let json = if self.mcp.target_wsl {
                    mcp_config_json_wsl(&self.mcp.server_token, vault_pw.as_deref())
                } else {
                    mcp_config_json(&self.mcp.server_token, vault_pw.as_deref())
                };
                return Ok(iced::clipboard::write(json).discard());
            }
            Message::Mcp(McpMessage::InstallMcpConfig) => {
                self.mcp.install_status = None;
                let token = self.mcp.server_token.clone();
                let vault_pw = self.mcp_vault_pw();
                let wsl = self.mcp.target_wsl;
                return Ok(Task::perform(
                    async move {
                        if wsl {
                            install_mcp_config_to_wsl(&token, vault_pw.as_deref())
                        } else {
                            install_mcp_config_to_file(&token, vault_pw.as_deref())
                        }
                    },
                    |v| Message::Mcp(McpMessage::InstallMcpConfigResult(v)),
                ));
            }
            Message::Mcp(McpMessage::SetMcpTarget(is_wsl)) => {
                self.mcp.target_wsl = is_wsl;
                // The Copy / Install feedback from the previous target no
                // longer reflects what's on screen.
                self.mcp.config_copied = false;
                self.mcp.install_status = None;
            }
            Message::Mcp(McpMessage::InstallMcpConfigResult(result)) => {
                self.mcp.install_status = Some(result);
            }
            Message::Mcp(McpMessage::RegenerateMcpToken) => {
                use rand::RngCore;
                let mut bytes = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut bytes);
                let mut token = String::with_capacity(64);
                for b in bytes {
                    use std::fmt::Write as _;
                    let _ = write!(token, "{b:02x}");
                }
                self.persist_setting("mcp_server_token", &token);
                self.mcp.server_token = token;
                // Reveal once after regenerating so the user can copy
                // it without an extra click; flip it back to masked
                // explicitly via `ToggleMcpTokenVisibility`.
                self.mcp.token_visible = true;
                // The Claude config on disk still carries the old
                // token, prompt the user to re-install.
                self.mcp.install_status = None;
            }
            Message::Mcp(McpMessage::ToggleMcpTokenVisibility) => {
                self.mcp.token_visible = !self.mcp.token_visible;
            }
            Message::Mcp(McpMessage::CopyMcpToken) => {
                return Ok(iced::clipboard::write(self.mcp.server_token.clone()).discard());
            }
            Message::Mcp(McpMessage::McpVaultPwPromptOpen) => {
                self.mcp.vault_pw_prompt = Some(String::new());
                self.mcp.vault_pw_error = false;
            }
            Message::Mcp(McpMessage::McpVaultPwPromptCancel) => {
                self.mcp.vault_pw_prompt = None;
                self.mcp.vault_pw_error = false;
            }
            Message::Mcp(McpMessage::McpVaultPwInput(v)) => {
                if let Some(buf) = &mut self.mcp.vault_pw_prompt {
                    *buf = v;
                }
            }
            Message::Mcp(McpMessage::McpVaultPwConfirm) => {
                let Some(typed) = self.mcp.vault_pw_prompt.clone() else {
                    return Ok(Task::none());
                };
                let ok = self
                    .vault
                    .as_ref()
                    .map(|v| v.verify_password(&typed).unwrap_or(false))
                    .unwrap_or(false);
                if ok {
                    // Persist the CONSENT, never the password: snippets
                    // and installs read it from `master_password` at use
                    // time. Refresh that copy from the verified input so
                    // the embed can't go stale.
                    self.mcp.include_vault_password = true;
                    self.persist_setting("mcp_config_vault_pw", "true");
                    self.master_password = Some(typed);
                    self.mcp.vault_pw_prompt = None;
                    self.mcp.vault_pw_error = false;
                    // The snippet content changed; stale Copy / Install
                    // feedback would claim the on-disk config already
                    // carries it.
                    self.mcp.config_copied = false;
                    self.mcp.install_status = None;
                } else {
                    self.mcp.vault_pw_error = true;
                    if let Some(buf) = &mut self.mcp.vault_pw_prompt {
                        buf.clear();
                    }
                }
            }
            Message::Mcp(McpMessage::McpVaultPwRemove) => {
                self.mcp.include_vault_password = false;
                self.persist_setting("mcp_config_vault_pw", "false");
                self.mcp.config_copied = false;
                self.mcp.vault_pw_strip_status = None;
                // Actively scrub the plaintext password from EVERY config
                // that carries it (native + WSL), in place and off the UI
                // thread. Flipping the consent alone would leave the
                // credential in `~/.claude.json` while the UI claims it was
                // revoked; and scrubbing only the currently-selected target
                // would miss a copy installed into the other one. The strip
                // is presence-gated per target, so it never creates a
                // config nor promotes the legacy dead-letter.
                let token = self.mcp.server_token.clone();
                return Ok(Task::perform(
                    async move { crate::mcp::strip_vault_password_everywhere(&token) },
                    |v| Message::Mcp(McpMessage::McpVaultPwStripResult(v)),
                ));
            }
            Message::Mcp(McpMessage::McpVaultPwStripResult(res)) => {
                self.mcp.vault_pw_strip_status = Some(res);
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
