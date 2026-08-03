//! Settings dispatch helpers: terminal behavior preferences
//! (toggles, selection / paste, font, scrollback). Split out of
//! dispatch_settings/mod.rs.

use super::*;

impl Oryxis {
    /// Terminal-preference arms: behavior toggles, bell / clipboard /
    /// notification modes, font size + family and scrollback.
    pub(super) fn handle_settings_terminal_prefs(
        &mut self,
        message: SettingsMessage,
    ) -> Result<Task<Message>, SettingsMessage> {
        match message {
            SettingsMessage::BellModeChanged(name) => {
                use crate::util::BellMode;
                if let Some(mode) = BellMode::ALL
                    .iter()
                    .find(|m| crate::i18n::t(m.label_key()) == name)
                {
                    self.setting_bell_mode = *mode;
                    self.persist_setting("terminal_bell_mode", mode.code());
                }
            }
            SettingsMessage::ClipboardAccessChanged(name) => {
                use crate::util::ClipboardAccess;
                if let Some(mode) = ClipboardAccess::ALL
                    .iter()
                    .find(|m| crate::i18n::t(m.label_key()) == name)
                {
                    self.setting_clipboard_access = *mode;
                    self.persist_setting("terminal_clipboard_access", mode.code());
                    let (cw, cr) = mode.flags();
                    oryxis_terminal::set_clipboard_access(cw, cr);
                }
            }
            SettingsMessage::NotificationModeChanged(name) => {
                use crate::util::NotificationMode;
                if let Some(mode) = NotificationMode::ALL
                    .iter()
                    .find(|m| crate::i18n::t(m.label_key()) == name)
                {
                    self.setting_notification_mode = *mode;
                    self.persist_setting("terminal_notification", mode.code());
                }
            }
            SettingsMessage::SettingToggleSmartTabs => {
                self.setting_smart_tabs = !self.setting_smart_tabs;
                self.persist_setting(
                    "smart_tabs",
                    if self.setting_smart_tabs { "true" } else { "false" },
                );
                // Turning it off retires any attention already raised;
                // stale dots surviving the toggle would contradict the
                // "all its UI hidden when off" rule.
                if !self.setting_smart_tabs {
                    for tab in &mut self.tabs {
                        for pane in tab.pane_grid.panes.values_mut() {
                            pane.attention = None;
                            pane.running_cmd = None;
                            pane.last_submitted = None;
                        }
                    }
                }
            }
            SettingsMessage::SmartTabsThresholdChanged(label) => {
                if let Some((secs, _)) = crate::smart_tabs::threshold_options()
                    .into_iter()
                    .find(|(_, l)| *l == label)
                {
                    self.setting_smart_long_secs = secs;
                    self.persist_setting("smart_tabs_long_seconds", &secs.to_string());
                }
            }
            SettingsMessage::TerminalFontSizeIncrease => {
                self.terminal_font_size = (self.terminal_font_size + 1.0).min(24.0);
                self.persist_setting(
                    "terminal_font_size",
                    &format!("{}", self.terminal_font_size),
                );
            }
            SettingsMessage::TerminalFontSizeDecrease => {
                self.terminal_font_size = (self.terminal_font_size - 1.0).max(10.0);
                self.persist_setting(
                    "terminal_font_size",
                    &format!("{}", self.terminal_font_size),
                );
            }
            SettingsMessage::TerminalFontChanged(name) => {
                self.terminal_font_name = name;
                self.persist_setting("terminal_font_name", &self.terminal_font_name);
            }
            SettingsMessage::ToggleCopyOnSelect => {
                self.setting_copy_on_select = !self.setting_copy_on_select;
                self.persist_setting(
                    "copy_on_select",
                    if self.setting_copy_on_select { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleRightClickCopy => {
                self.setting_right_click_copy = !self.setting_right_click_copy;
                self.persist_setting(
                    "right_click_copy",
                    if self.setting_right_click_copy { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleMiddleClickPaste => {
                // Writes through the binding table, which owns the
                // gesture; there is no `middle_click_paste` setting any
                // more (see `set_middle_click_paste`).
                let on = !self.middle_click_pastes();
                return Ok(self.set_middle_click_paste(on));
            }
            SettingsMessage::SettingSftpDefaultEditorChanged(v) => {
                self.setting_sftp_default_editor = v;
                self.persist_setting(
                    "sftp_default_editor",
                    &self.setting_sftp_default_editor.clone(),
                );
            }
            SettingsMessage::SettingSftpDefaultEditorBrowse => {
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(move || {
                        rfd::FileDialog::new()
                            .set_title(crate::i18n::t("setting_default_editor"))
                            .pick_file()
                            .map(|p| p.to_string_lossy().to_string())
                            .ok_or_else(|| "cancelled".to_string())
                    }),
                    |result| {
                        let r = match result {
                            Ok(r) => r,
                            Err(e) => Err(format!("Thread error: {e}")),
                        };
                        Message::Settings(SettingsMessage::SettingSftpDefaultEditorPicked(r))
                    },
                ));
            }
            SettingsMessage::SettingSftpDefaultEditorPicked(result) => {
                if let Ok(path) = result {
                    self.setting_sftp_default_editor = path;
                    self.persist_setting(
                        "sftp_default_editor",
                        &self.setting_sftp_default_editor.clone(),
                    );
                }
                // "cancelled" / thread errors stay silent: the user just
                // closed the dialog.
            }
            SettingsMessage::ToggleSftpEditAutosave => {
                self.setting_sftp_edit_autosave = !self.setting_sftp_edit_autosave;
                self.persist_setting(
                    "sftp_edit_autosave",
                    if self.setting_sftp_edit_autosave { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleSftpAskDownloadDir => {
                self.setting_sftp_ask_download_dir = !self.setting_sftp_ask_download_dir;
                self.persist_setting(
                    "sftp_ask_download_dir",
                    if self.setting_sftp_ask_download_dir { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleSftpForceOsc7 => {
                self.setting_sftp_force_osc7 = !self.setting_sftp_force_osc7;
                self.persist_setting(
                    "sftp_force_osc7",
                    if self.setting_sftp_force_osc7 { "true" } else { "false" },
                );
                // Enabling it applies to LIVE sessions right away (the
                // per-connect inject only covers future connects): the
                // "I turned it on and nothing happened" gap. Inject into
                // every already-connected SSH pane not yet injected; the
                // next prompt then emits OSC 7 and the sidebar snaps to
                // the real cwd.
                if self.setting_sftp_force_osc7 {
                    for tab in &mut self.tabs {
                        for pane in tab.pane_grid.panes.values_mut() {
                            if pane.osc7_injected {
                                continue;
                            }
                            if let Some(ssh) = pane.session.as_ref().and_then(|s| s.ssh())
                                && ssh
                                    .write(crate::state::OSC7_PROMPT_INJECT.as_bytes())
                                    .is_ok()
                            {
                                pane.osc7_injected = true;
                            }
                        }
                    }
                }
            }
            SettingsMessage::TerminalRightClickChanged(name) => {
                use crate::util::RightClickMode;
                if let Some(mode) = RightClickMode::ALL
                    .iter()
                    .find(|m| crate::i18n::t(m.label_key()) == name)
                {
                    self.setting_terminal_right_click = *mode;
                    self.persist_setting("terminal_right_click", mode.code());
                }
            }
            SettingsMessage::SidebarDefaultTabChanged(name) => {
                use crate::state::TerminalSidebarTab;
                // "Last opened" (the sentinel label) clears the pin; any
                // tab label sets it. Match against the translated labels,
                // like the right-click picker.
                if name == crate::i18n::t("sidebar_default_last") {
                    self.setting_sidebar_default_tab = None;
                    self.persist_setting("sidebar_default_tab", "last");
                } else if let Some(tab) = TerminalSidebarTab::ALL
                    .into_iter()
                    .find(|t| crate::i18n::t(t.label_key()) == name)
                {
                    self.setting_sidebar_default_tab = Some(tab);
                    self.persist_setting("sidebar_default_tab", tab.code());
                }
            }
            SettingsMessage::ToggleScrollbackResetKeypress => {
                self.setting_scrollback_reset_keypress = !self.setting_scrollback_reset_keypress;
                self.persist_setting(
                    "scrollback_reset_keypress",
                    if self.setting_scrollback_reset_keypress { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleScrollbackResetOutput => {
                self.setting_scrollback_reset_output = !self.setting_scrollback_reset_output;
                self.persist_setting(
                    "scrollback_reset_output",
                    if self.setting_scrollback_reset_output { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleCarefulPaste => {
                self.setting_careful_paste = !self.setting_careful_paste;
                // Turning the guard off releases nothing: a parked paste
                // (dialog open) still needs its explicit confirm/cancel.
                self.persist_setting(
                    "careful_paste",
                    if self.setting_careful_paste { "true" } else { "false" },
                );
            }
            SettingsMessage::TogglePasteGuard => {
                self.setting_paste_guard = !self.setting_paste_guard;
                self.persist_setting(
                    "paste_guard",
                    if self.setting_paste_guard { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleTerminalAutoTitle => {
                let on = !crate::state::auto_title_enabled();
                crate::state::set_auto_title(on);
                self.persist_setting("terminal_auto_title", if on { "true" } else { "false" });
            }
            SettingsMessage::TogglePaneBorderInactive => {
                self.setting_pane_border_inactive = !self.setting_pane_border_inactive;
                self.persist_setting(
                    "pane_border_inactive",
                    if self.setting_pane_border_inactive { "true" } else { "false" },
                );
            }
            SettingsMessage::OpenTerminalThemeGallery => {
                self.show_terminal_theme_gallery = true;
            }
            SettingsMessage::CloseTerminalThemeGallery => {
                self.show_terminal_theme_gallery = false;
            }
            SettingsMessage::OpenUiThemeGallery => {
                self.show_ui_theme_gallery = true;
            }
            SettingsMessage::CloseUiThemeGallery => {
                self.show_ui_theme_gallery = false;
            }
            SettingsMessage::PaneGapChanged(v) => {
                self.setting_pane_gap = v.clone();
                self.persist_setting("pane_gap", &v);
            }
            SettingsMessage::ToggleBoldIsBright => {
                self.setting_bold_is_bright = !self.setting_bold_is_bright;
                self.persist_setting(
                    "bold_is_bright",
                    if self.setting_bold_is_bright { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleKeywordHighlight => {
                self.setting_keyword_highlight = !self.setting_keyword_highlight;
                self.persist_setting(
                    "keyword_highlight",
                    if self.setting_keyword_highlight { "true" } else { "false" },
                );
            }
            SettingsMessage::ToggleCommandHistory => {
                self.setting_command_history = !self.setting_command_history;
                self.persist_setting(
                    "command_history",
                    if self.setting_command_history { "true" } else { "false" },
                );
            }
            SettingsMessage::TerminalLinkOpened => {
                // First successful ctrl-click on a link in this pane: the
                // hint did its job, retire it for the pane (HintMode::Once).
                // In-memory only, a fresh pane shows it again.
                if let Some(tab_idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(tab_idx)
                {
                    tab.active_mut().link_hint_shown = true;
                }
            }
            SettingsMessage::HintModeChanged(name) => {
                use crate::i18n::t;
                use crate::util::HintMode;
                if let Some(mode) = HintMode::ALL.iter().find(|m| t(m.label_key()) == name) {
                    self.setting_hint_mode = *mode;
                    self.persist_setting("terminal_hint_mode", mode.code());
                }
            }
            SettingsMessage::ToggleSmartContrast => {
                self.setting_smart_contrast = !self.setting_smart_contrast;
                self.persist_setting(
                    "smart_contrast",
                    if self.setting_smart_contrast { "true" } else { "false" },
                );
            }
            SettingsMessage::SettingScrollbackChanged(val) => {
                // Cap at 1M rows, alacritty allocates lazily but >1M is
                // both unreasonable and a foot-gun for memory pressure.
                self.setting_scrollback_rows = sanitize_uint(&val, 1_000_000);
                self.persist_setting("scrollback_rows", &self.setting_scrollback_rows);
                // Applies to terminals opened after this point; existing
                // sessions keep their current buffer.
                oryxis_terminal::set_default_scrollback(resolve_scrollback_rows(
                    &self.setting_scrollback_rows,
                ));
            }
            SettingsMessage::SettingWordDelimitersChanged(val) => {
                // Free-text: any character may delimit a word. Stored as
                // typed; the widget syncs it into the terminal backend on
                // the next double-click. Empty is allowed (no delimiters).
                self.setting_word_delimiters = val;
                self.persist_setting("word_delimiters", &self.setting_word_delimiters);
            }
            SettingsMessage::SettingResetWordDelimiters => {
                self.setting_word_delimiters =
                    oryxis_terminal::DEFAULT_WORD_DELIMITERS.to_string();
                self.persist_setting("word_delimiters", &self.setting_word_delimiters);
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }
}
