//! What the session looks and behaves like once it is up.
//!
//! Theme, encoding, `TERM`, the startup command, and the compatibility
//! quirks (backspace, Home/End, function keys, mouse reporting, OSC 52,
//! Option-as-Meta) plus the algorithm overrides.

use super::*;

impl Oryxis {
    pub(super) fn handle_editor_terminal(&mut self, message: EditorMessage) -> Task<Message> {
        match message {
            EditorMessage::EditorOpenThemePicker => {
                self.panels.theme_picker = true;
            }
            EditorMessage::EditorCloseThemePicker => {
                self.panels.theme_picker = false;
            }
            EditorMessage::EditorTerminalThemeChanged(name) => {
                // Empty string == "inherit the global pick".
                self.editor_form.terminal_theme =
                    if name.is_empty() { None } else { Some(name) };
                self.panels.theme_picker = false;
            }
            EditorMessage::EditorEncodingChanged(v) => {
                // "UTF-8" is the implicit default, stored as None so the
                // SSH engine skips transcoding entirely.
                self.editor_form.encoding = if v == "UTF-8" { None } else { Some(v) };
            }
            EditorMessage::EditorTerminalTypeChanged(v) => {
                // "xterm-256color" is the implicit default, stored as None.
                self.editor_form.terminal_type =
                    if v == "xterm-256color" { None } else { Some(v) };
            }
            EditorMessage::EditorAutoTitleChanged(v) => {
                use crate::i18n::t;
                // Map the localized pick label back to the tri-state override.
                self.editor_form.auto_title = if v == t("host_auto_title_show") {
                    Some(true)
                } else if v == t("host_auto_title_hide") {
                    Some(false)
                } else {
                    None
                };
            }
            EditorMessage::EditorPrivacyModeChanged(v) => {
                use crate::i18n::t;
                // Map the localized pick label back to the tri-state override.
                self.editor_form.privacy_mode = if v == t("host_privacy_mode_on") {
                    Some(true)
                } else if v == t("host_privacy_mode_off") {
                    Some(false)
                } else {
                    None
                };
            }
            EditorMessage::EditorSidebarAutoOpenChanged(v) => {
                use crate::i18n::t;
                // Same localized-label mapping as the privacy row above.
                self.editor_form.sidebar_auto_open = if v == t("host_privacy_mode_on") {
                    Some(true)
                } else if v == t("host_privacy_mode_off") {
                    Some(false)
                } else {
                    None
                };
            }
            EditorMessage::EditorSftpInitialPathChanged(v) => {
                // Free text: the path lives on the remote host, so there is
                // nothing to validate locally. Newlines are stripped because
                // a paste can carry one and a remote path never contains it.
                self.editor_form.sftp_initial_path =
                    v.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            }
            EditorMessage::EditorStartupComboOpened => {
                // Focus clears the typed value so the dropdown opens on
                // the full snippet list, not pre-filtered by the current
                // selection (the committed choice is preserved untouched).
                self.reset_editor_startup_combo();
            }
            EditorMessage::EditorStartupChoiceChanged(label) => {
                use crate::state::StartupChoice;
                // Map the picker label back to a source. The None / Custom
                // sentinels come from i18n; anything else is a snippet
                // label. A snippet is stored as a live reference (its id),
                // resolved to the snippet body at connect time, so we
                // don't copy the body into the custom text editor here.
                if label == crate::i18n::t("startup_none") {
                    self.editor_startup_choice = StartupChoice::None;
                    self.editor_initial_command =
                        iced::widget::text_editor::Content::new();
                } else if label == crate::i18n::t("startup_custom") {
                    self.editor_startup_choice = StartupChoice::Custom;
                } else if let Some(s) =
                    self.snippets.iter().find(|s| s.label == label)
                {
                    self.editor_startup_choice = StartupChoice::Snippet(s.id);
                }
            }
            EditorMessage::EditorInitialCommandChanged(action) => {
                self.editor_initial_command.perform(action);
            }
            EditorMessage::EditorQuirkBackspaceChanged(v) => {
                self.editor_form.quirks.backspace = crate::util::quirk_backspace_from_label(&v);
            }
            EditorMessage::EditorQuirkHomeEndChanged(v) => {
                self.editor_form.quirks.home_end = crate::util::quirk_home_end_from_label(&v);
            }
            EditorMessage::EditorQuirkFnKeysChanged(v) => {
                self.editor_form.quirks.function_keys = crate::util::quirk_fn_keys_from_label(&v);
            }
            EditorMessage::EditorQuirkMouseReportingChanged(on) => {
                // Toggle shows the positive "report mouse"; off disables it.
                self.editor_form.quirks.disable_mouse_reporting = !on;
            }
            EditorMessage::EditorQuirkTitleChangeChanged(on) => {
                self.editor_form.quirks.disable_title_change = !on;
            }
            EditorMessage::EditorQuirkOsc52Changed(v) => {
                self.editor_form.quirks.osc52 = crate::util::quirk_osc52_from_label(&v);
            }
            EditorMessage::EditorQuirkOptionAsMetaChanged(v) => {
                self.editor_form.quirks.option_as_meta =
                    crate::util::quirk_option_as_meta_from_label(&v);
            }
            EditorMessage::EditorQuirkRekeyChanged(v) => {
                // Digits only; empty allowed (= default). Clamp to russh's
                // 1 GiB cap (1024 MB) so the field can't exceed it.
                self.editor_form.rekey_limit_mb = if v.trim().is_empty() {
                    String::new()
                } else {
                    crate::util::sanitize_uint(&v, 1024)
                };
            }
            EditorMessage::EditorAlgoSetAuto(cat, auto) => {
                // Auto = None (russh defaults). Switching to custom seeds the
                // list with the safe defaults so the user adds legacy entries
                // (or trims) from a working set rather than from nothing.
                *self.editor_form.algo_list_mut(cat) = if auto {
                    None
                } else {
                    Some(cat.defaults())
                };
            }
            EditorMessage::EditorAlgoToggle(cat, name) => {
                let list = self.editor_form.algo_list_mut(cat).get_or_insert_with(Vec::new);
                if let Some(pos) = list.iter().position(|n| n == &name) {
                    list.remove(pos);
                } else {
                    list.push(name);
                }
            }
            // Routed here by the parent; anything else is a
            // grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
