//! Settings -> Connection: managing saved login automations (#122).
//!
//! The host editor creates them; this handles the operations that need
//! the whole list. The working copy in `login_script_form` is committed
//! to the vault only on save, so a half-edited step list can never be
//! picked up by a connect happening in another tab.

use super::*;

use oryxis_core::login_script::{
    ExpectPattern, LoginStep, NamedKey, ScriptRunner, SecretRef, SendPayload,
};

/// Marker that makes the single "wait for" field able to express both
/// pattern kinds without a second control per row.
const REGEX_PREFIX: &str = "re:";

impl Oryxis {
    pub(super) fn handle_settings_login_scripts(
        &mut self,
        message: SettingsMessage,
    ) -> Task<Message> {
        match message {
            SettingsMessage::LoginScriptEdit(id) => {
                if let Some(script) = self.login_scripts.iter().find(|s| s.id == id) {
                    self.login_script_form = crate::state::LoginScriptForm {
                        editing_id: Some(id),
                        name: script.name.clone(),
                        steps: script.steps.clone(),
                        error: None,
                        confirm_delete: None,
                    };
                }
            }
            SettingsMessage::LoginScriptCancelEdit => {
                self.login_script_form = crate::state::LoginScriptForm::default();
            }
            SettingsMessage::LoginScriptNameChanged(v) => {
                self.login_script_form.name = v;
            }
            SettingsMessage::LoginScriptAddStep => {
                self.login_script_form.steps.push(LoginStep {
                    expect: Some(ExpectPattern::Suffix(String::new())),
                    send: SendPayload::Text(String::new()),
                    timeout_ms: 0,
                    optional: false,
                });
            }
            SettingsMessage::LoginScriptRemoveStep(i) => {
                if i < self.login_script_form.steps.len() {
                    self.login_script_form.steps.remove(i);
                }
            }
            SettingsMessage::LoginScriptStepExpect(i, v) => {
                if let Some(step) = self.login_script_form.steps.get_mut(i) {
                    step.expect = if v.trim().is_empty() {
                        // Blank means "send without waiting", which the
                        // engine models as no pattern at all.
                        None
                    } else if let Some(rest) = v.strip_prefix(REGEX_PREFIX) {
                        Some(ExpectPattern::Regex(rest.to_string()))
                    } else {
                        Some(ExpectPattern::Suffix(v))
                    };
                }
            }
            SettingsMessage::LoginScriptStepSendKind(i, label) => {
                if let Some(step) = self.login_script_form.steps.get_mut(i) {
                    // Compare against the localized labels the picker
                    // rendered, in the same order the view declares.
                    let kinds = crate::views::settings::login_scripts::SEND_KINDS;
                    let picked = kinds.iter().position(|k| crate::i18n::t(k) == label);
                    step.send = match picked {
                        Some(0) => SendPayload::Text(String::new()),
                        Some(1) => SendPayload::Secret(SecretRef::TargetPassword),
                        Some(2) => SendPayload::Secret(SecretRef::ConnectionPassword),
                        Some(3) => SendPayload::Secret(SecretRef::Totp),
                        Some(4) => SendPayload::Key(NamedKey::Enter),
                        Some(5) => SendPayload::Nothing,
                        _ => return Task::none(),
                    };
                }
            }
            SettingsMessage::LoginScriptStepText(i, v) => {
                if let Some(step) = self.login_script_form.steps.get_mut(i)
                    && matches!(step.send, SendPayload::Text(_))
                {
                    step.send = SendPayload::Text(v);
                }
            }
            SettingsMessage::LoginScriptStepOptional(i) => {
                if let Some(step) = self.login_script_form.steps.get_mut(i) {
                    step.optional = !step.optional;
                }
            }
            SettingsMessage::LoginScriptSave => {
                let Some(id) = self.login_script_form.editing_id else {
                    return Task::none();
                };
                let name = self.login_script_form.name.trim().to_string();
                if name.is_empty() {
                    self.login_script_form.error =
                        Some(crate::i18n::t("login_script_name_required").to_string());
                    return Task::none();
                }
                let steps = self.login_script_form.steps.clone();
                // Validated here rather than at connect: a bad regex
                // should be a form error the user can see and fix, not a
                // silent no-op sixty seconds into a failed login.
                if let Err(e) = ScriptRunner::validate(&steps) {
                    self.login_script_form.error =
                        Some(format!("{}: {e}", crate::i18n::t("login_script_invalid")));
                    return Task::none();
                }
                let Some(script) = self.login_scripts.iter_mut().find(|s| s.id == id) else {
                    self.login_script_form = crate::state::LoginScriptForm::default();
                    return Task::none();
                };
                script.name = name;
                script.steps = steps;
                script.updated_at = chrono::Utc::now();
                let saved = script.clone();
                if let Some(vault) = &self.vault
                    && let Err(e) = vault.save_login_script(&saved)
                {
                    self.login_script_form.error = Some(e.to_string());
                    return Task::none();
                }
                self.login_scripts.sort_by(|a, b| a.name.cmp(&b.name));
                self.login_script_form = crate::state::LoginScriptForm::default();
            }
            SettingsMessage::LoginScriptRequestDelete(id) => {
                self.login_script_form.confirm_delete = Some(id);
            }
            SettingsMessage::LoginScriptCancelDelete => {
                self.login_script_form.confirm_delete = None;
            }
            SettingsMessage::LoginScriptDelete(id) => {
                if let Some(vault) = &self.vault {
                    let _ = vault.delete_login_script(&id);
                }
                self.login_scripts.retain(|s| s.id != id);
                // The vault detached every host that referenced it; the
                // in-memory copies have to agree or the editor would
                // still offer a script that no longer exists.
                for conn in &mut self.connections {
                    if conn.login_script_id == Some(id) {
                        conn.login_script_id = None;
                        conn.login_script_vars.clear();
                    }
                }
                if self.editor_form.login_script_id == Some(id) {
                    self.editor_form.login_script_id = None;
                    self.editor_form.login_script_vars.clear();
                }
                self.login_script_form = crate::state::LoginScriptForm::default();
            }
            // Routed here by the parent; anything else is a grouping
            // mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
