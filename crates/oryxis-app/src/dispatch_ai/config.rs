//! Settings > AI: the provider, the model, the key, and the toggles.
//!
//! Everything here is one field written and persisted. The key is the
//! exception: it is encrypted per-field like a password, so it is saved
//! through its own arm rather than on every keystroke.

use iced::Task;
use crate::app::{AiMessage, Message, Oryxis};


impl Oryxis {
    pub(super) fn handle_ai_config(&mut self, message: AiMessage) -> Task<Message> {
        match message {
            AiMessage::ToggleAiEnabled => {
                self.ai.enabled = !self.ai.enabled;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_enabled", if self.ai.enabled { "true" } else { "false" });
                }
            }
            AiMessage::ToggleAiSaveHistory => {
                self.ai.save_history = !self.ai.save_history;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting(
                        "ai_save_history",
                        if self.ai.save_history { "true" } else { "false" },
                    );
                }
                // Turning it off does NOT delete what is stored: that is a
                // separate, destructive act the user did not ask for, and
                // the History screen already has a delete for it. It does
                // detach the live tabs, so a conversation resumed after the
                // switch comes back on starts a new row instead of
                // appending to one the user thought they had stopped.
                if !self.ai.save_history {
                    for idx in 0..self.tabs.len() {
                        self.detach_saved_chat(idx);
                    }
                }
            }
            AiMessage::ToggleAiReasoning => {
                self.ai.reasoning = !self.ai.reasoning;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting(
                        "ai_reasoning",
                        if self.ai.reasoning { "true" } else { "false" },
                    );
                }
            }
            AiMessage::AiProviderChanged(provider) => {
                // Accept either a display name (from the dropdown) or the
                // internal id. Fall back to keeping the current provider if
                // the value can't be resolved.
                let info = crate::ai::provider_from_display(&provider)
                    .unwrap_or_else(|| crate::ai::provider_info(&provider));
                self.ai.provider = info.id.to_string();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_provider", &self.ai.provider);
                }
                // Suggest the provider's default model when the user hasn't
                // picked one. For Custom we keep whatever model is set.
                if !info.default_model.is_empty() {
                    self.ai.model = info.default_model.into();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_setting("ai_model", &self.ai.model);
                    }
                }
                // Presets always use their bundled URL; clear any stale
                // override so Save doesn't carry it across providers.
                if info.kind != crate::ai::ProviderKind::Custom {
                    self.ai.api_url.clear();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_setting("ai_api_url", "");
                    }
                }
            }
            AiMessage::AiModelChanged(model) => {
                self.ai.model = model;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_model", &self.ai.model);
                }
            }
            AiMessage::AiApiKeyChanged(key) => {
                self.ai.api_key = key.into_inner();
            }
            AiMessage::AiApiUrlChanged(url) => {
                self.ai.api_url = url;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_api_url", &self.ai.api_url);
                }
            }
            AiMessage::AiSystemPromptAction(action) => {
                let was_edit = action.is_edit();
                self.ai.system_prompt.perform(action);
                if was_edit
                    && let Some(vault) = &self.vault
                {
                    let _ = vault.set_setting("ai_system_prompt", &self.ai.system_prompt.text());
                }
            }
            AiMessage::SaveAiApiKey => {
                if !self.ai.api_key.is_empty()
                    && let Some(vault) = &self.vault
                    && vault.set_ai_api_key(&self.ai.api_key).is_ok() {
                        self.ai.api_key.clear();
                        self.ai.api_key_set = true;
                }
            }
            // Routed here by `handle_ai`; anything else is a
            // grouping mistake rather than a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
