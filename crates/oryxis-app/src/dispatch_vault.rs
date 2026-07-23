//! `Oryxis::handle_vault`: settings-panel-independent dispatch arms for the
//! vault area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{SshMessage, VaultMessage, Message, Oryxis};
use crate::state::{VaultState, View};
use oryxis_vault::VaultError;

impl Oryxis {
    pub(crate) fn handle_vault(
        &mut self,
        message: VaultMessage,
    ) -> Task<Message> {
        match message {
            // -- Vault --
            VaultMessage::VaultPasswordChanged(pw) => {
                self.vault_ui.password_input = pw;
            }
            VaultMessage::VaultTogglePasswordVisibility => {
                self.vault_ui.password_visible = !self.vault_ui.password_visible;
            }
            VaultMessage::VaultSetup => {
                if self.vault_ui.password_input.len() < 4 {
                    self.vault_ui.error =
                        Some(crate::i18n::t("password_too_short").to_string());
                    return Task::none();
                }
                // Phase 1 (E1): calibrate Argon2id off the UI thread, then
                // apply on `VaultKdfCalibrated`. A doubled click while the
                // spinner is up is a no-op.
                if self.vault_ui.calibrating {
                    return Task::none();
                }
                self.vault_ui.calibrating = true;
                self.vault_ui.error = None;
                // Snapshot the confirmed password: the input stays live
                // during the calibration and must not be re-read at apply
                // time (see `pending_kdf_pw`).
                self.vault_ui.pending_kdf_pw = Some(self.vault_ui.password_input.clone());
                return calibrate_kdf_task(crate::state::VaultPwOp::FirstSetup);
            }
            VaultMessage::VaultSkipPassword => {
                if let Some(vault) = &mut self.vault {
                    match vault.open_without_password() {
                        Ok(()) => {
                            self.vault_ui.state = VaultState::Unlocked;
                            self.vault_ui.error = None;
                            self.load_data_from_vault();
                            return Task::batch([
                                self.agent_boot_task(),
                                self.take_perf_mode_toast_task(),
                                iced::widget::operation::focus(iced::widget::Id::new(
                                    "search-dashboard",
                                )),
                            ]);
                        }
                        Err(VaultError::InvalidPassword) => {
                            self.vault_ui.error = Some(
                                crate::i18n::t("vault_already_has_password").to_string(),
                            );
                        }
                        Err(e) => {
                            self.vault_ui.error = Some(format!("Failed to create vault: {}", e));
                        }
                    }
                }
            }
            VaultMessage::VaultDestroyConfirm => {
                self.vault_ui.destroy_confirm = !self.vault_ui.destroy_confirm;
            }
            VaultMessage::VaultDestroy => {
                if let Some(vault) = &mut self.vault {
                    match vault.destroy_and_recreate() {
                        Ok(()) => {
                            self.vault_ui.state = VaultState::NeedSetup;
                            self.vault_ui.error = None;
                            self.vault_ui.destroy_confirm = false;
                            self.vault_ui.password_input.clear();
                            self.vault_ui.password_visible = false;
                        }
                        Err(e) => {
                            self.vault_ui.error = Some(format!("Failed to reset vault: {}", e));
                        }
                    }
                }
            }
            VaultMessage::VaultUnlock => {
                // Ignore the submit when no password was typed (pressing
                // Enter on an empty field or clicking Unlock with it blank
                // shouldn't run a doomed unlock attempt or surface an error).
                if self.vault_ui.password_input.is_empty() {
                    return Task::none();
                }
                if let Some(vault) = &mut self.vault {
                    match vault.unlock(&self.vault_ui.password_input) {
                        Ok(()) => {
                            self.vault_ui.state = VaultState::Unlocked;
                            self.vault_ui.error = None;
                            // Retain the password in memory so we can spawn
                            // child windows with it via stdin pipe.
                            self.master_password = Some(self.vault_ui.password_input.clone());
                            // Keep the OS-keystore copy current so biometric
                            // unlock reflects the live password (self-heals
                            // after a rotation). No-op unless opted in;
                            // enroll never prompts.
                            self.biometric_reenroll(&self.vault_ui.password_input);
                            self.vault_ui.password_input.clear();
                            self.vault_ui.password_visible = false;
                            // Next lock screen leads with biometrics again
                            // (a one-time fallback choice shouldn't stick).
                            self.vault_ui.password_fallback = false;
                            self.load_data_from_vault();
                            // Re-arm the ssh-agent's dedicated handle if a
                            // runtime survived a soft lock.
                            self.agent_on_unlock();
                            // Bring the sync engine up now that the
                            // vault is open, if the user left it on. Only
                            // the P2P transport has a background engine;
                            // SFTP reconciles on the cadence subscription.
                            let sync_task = if self.sync.enabled
                                && self.sync.transport != "sftp"
                            {
                                self.start_sync_engine()
                            } else {
                                Task::none()
                            };
                            // Auto-start port forward rules now that the
                            // vault (and its credentials) is open.
                            let mut unlock_tasks = vec![sync_task];
                            unlock_tasks.extend(self.auto_start_port_forwards());
                            // Plugin migrate-install + auto-update: for a
                            // password vault these are deferred from boot
                            // to here, now that the plugin rows are loaded
                            // (boot saw a locked vault with no rows).
                            unlock_tasks.extend(self.spawn_plugin_unlock_tasks());
                            // One-time performance-mode auto-enable notice.
                            unlock_tasks.push(self.take_perf_mode_toast_task());
                            // Bring the ssh-agent up if the user left it on.
                            unlock_tasks.push(self.agent_boot_task());
                            // The lock sweep dropped the History content-search
                            // results (decrypted excerpts must not sit behind the
                            // lock screen) but the chip and the typed query survive,
                            // consistent with the soft-lock promise that state comes
                            // back. Re-arm the debounced search here so an active
                            // chip reflects live results again instead of rendering
                            // active over nothing until the next keystroke.
                            if self.history_search_content {
                                unlock_tasks.push(self.history_content_debounce());
                            }
                            // After a manual unlock, fire any deferred
                            // `--connect <uuid>` from the launch CLI args.
                            if let Some(connect_id) = self.pending_auto_connect.take()
                                && let Some(idx) = self
                                    .connections
                                    .iter()
                                    .position(|c| c.id == connect_id)
                            {
                                unlock_tasks.push(Task::done(Message::Ssh(SshMessage::ConnectSsh(idx))));
                            } else {
                                // Land on Home with the host search focused
                                // so the user can type / keyboard-navigate
                                // immediately (matches ChangeView behavior).
                                unlock_tasks.push(iced::widget::operation::focus(
                                    iced::widget::Id::new("search-dashboard"),
                                ));
                            }
                            return Task::batch(unlock_tasks);
                        }
                        Err(VaultError::InvalidPassword) => {
                            self.vault_ui.error = Some("Invalid password".into());
                        }
                        Err(e) => {
                            self.vault_ui.error = Some(e.to_string());
                        }
                    }
                }
            }

            // ── Vault lock (manual + idle auto-lock) ──
            VaultMessage::AutoLockVault => {
                // Soft lock: the user walked away, not "I'm done". Zeroize
                // the master key and drop to the lock screen, but keep
                // live SSH sessions and tabs so long-running remote work
                // survives the idle period (established channels never
                // need the key again; credentials are only read at
                // connect time). The manual LockVault stays a full
                // teardown. While locked, the session-log flush and
                // auto-reconnect tickers unmount (subscription.rs), so
                // nothing hits the sealed vault; pane buffers accumulate
                // and drain after unlock.
                if let Some(vault) = &mut self.vault
                    && self.vault_ui.has_user_password
                {
                    vault.lock();
                    self.vault_ui.state = VaultState::Locked;
                    self.master_password = None;
                    // The lock screen leads with biometrics when enrolled;
                    // a fallback choice from a previous lock must not stick.
                    self.vault_ui.password_fallback = false;
                    // Sweep UI that may hold typed or revealed secrets;
                    // everything else (tabs, terminals) stays.
                    self.revealed_secrets.clear();
                    self.show_host_panel = false;
                    self.host_panel_error = None;
                    self.editor_form = crate::state::ConnectionForm::default();
                    // The key-generation panel carries export
                    // passphrases and a public-key view; sweep it (a
                    // still-running generation task is dropped on
                    // completion by the locked-vault check).
                    self.show_key_generate_panel = false;
                    self.key_generate_form = crate::state::KeyGenerateForm::default();
                    // The key import panel (holds a pasted cert / PEM) and
                    // the cert viewer are vault-area surfaces; drop them.
                    // The live PEM editor buffer is private material, so it
                    // is reset too, matching the generate-panel sweep.
                    self.show_key_panel = false;
                    self.key_import_form = crate::state::KeyImportForm::default();
                    self.key_import_content = iced::widget::text_editor::Content::new();
                    self.key_import_public_content = iced::widget::text_editor::Content::new();
                    self.key_import_cert_content = iced::widget::text_editor::Content::new();
                    self.cert_viewer = None;
                    // The session player / log viewer hold DECRYPTED
                    // recording bytes (a session that ran `cat
                    // /etc/shadow` keeps that output in the emulator grid
                    // / rendered spans). That is secret-bearing UI like a
                    // revealed secret, so it must not sit in RAM behind
                    // the lock screen; it can only be rebuilt from the
                    // vault after unlock anyway.
                    self.session_player = None;
                    self.viewing_session_log = None;
                    // The History content-search results hold decrypted
                    // command lines / output excerpts; same rule.
                    self.history_content_reset();
                    // The ssh-agent goes dark (keys ungated) while locked;
                    // the listener stays up so a `git` sees an empty agent.
                    self.agent_on_lock();
                    self.overlay = None;
                    self.card_context_menu = None;
                    // Top-strip pickers (command palette, tab-jump,
                    // new-tab picker) are NOT rendered over the lock
                    // screen, but their `show_*` flags still make
                    // `any_modal_blocks_input()` true, so the modal key
                    // router would keep processing arrows / Enter for the
                    // hidden surface behind the lock screen (the command
                    // palette could even dispatch an action while locked).
                    // Close them like the SFTP modals below.
                    self.close_modal(crate::state::Modal::CommandPalette);
                    self.close_modal(crate::state::Modal::TabJump);
                    self.close_modal(crate::state::Modal::NewTabPicker);
                    // A master-password candidate typed into the change /
                    // set-password form must not survive the soft lock.
                    self.vault_ui.new_password.clear();
                    self.vault_ui.confirm_password.clear();
                    // Abort an in-flight KDF calibration too: its snapshot is
                    // secret material and the apply must not land post-lock.
                    self.vault_ui.pending_kdf_pw = None;
                    // Same for the MCP panel's master-password confirm.
                    self.mcp.vault_pw_prompt = None;
                    self.mcp.vault_pw_error = false;
                    // SFTP modals carry remote paths and live action buttons;
                    // root_view already stops rendering them while locked, but
                    // sweep the state so none reappears after unlock. A dirty
                    // edit_session is dropped with its pending upload, matching
                    // the soft-lock promise (secret-bearing UI is discarded,
                    // the live session survives).
                    self.sftp.picker_open = false;
                    self.sftp.new_entry = None;
                    self.sftp.delete_confirm.clear();
                    self.sftp.edit_session = None;
                    self.sftp.edit_watches.clear();
                    // Watches parked in standalone / hybrid tab states feed
                    // the same 2s tick; left alive they would keep uploading
                    // local saves (under an autosave grant) behind the lock
                    // screen, and dirty ones would re-prompt after unlock.
                    for tab in self.sftp_tabs.iter_mut() {
                        tab.state.edit_session = None;
                        tab.state.edit_watches.clear();
                    }
                    for tab in self.tabs.iter_mut() {
                        tab.files_state.edit_session = None;
                        tab.files_state.edit_watches.clear();
                    }
                    // Monitor samples are host telemetry gathered while
                    // unlocked; drop them with the rest of the sweep so a
                    // locked screen shows nothing about the fleet. The
                    // stamp bump inside makes a probe still in flight land
                    // dead instead of repopulating the swept state.
                    self.monitor_reset_all();
                    self.sftp.overwrite_prompt = None;
                    self.sftp.properties = None;
                    // A pending keyboard-interactive prompt belongs to an
                    // in-flight connect; cancel it cleanly (the engine
                    // treats `None` as auth abort).
                    if self.pending_kbi_prompt.take().is_some() {
                        self.kbi_inputs.clear();
                        if let Some(ref tx) = self.kbi_response_tx {
                            let _ = tx.try_send(None);
                        }
                    }
                    // A pending host-key prompt is a security dialog for an
                    // in-flight backgrounded connect; reject it (safe
                    // default) rather than leaving it rendered over the lock
                    // screen. Mirrors SshHostKeyReject.
                    if self.pending_host_key.take().is_some()
                        && let Some(tx) = self.active_host_key_tx.take()
                    {
                        let _ = tx.try_send(false);
                    }
                    self.pending_kbi_quick = None;
                    // A parked identity/key switch must not fire a
                    // reconnect behind the lock screen.
                    self.pending_auth_switch = None;
                    // Quick-connect entries hold typed plaintext credentials;
                    // sweep the secrets but keep the connections themselves,
                    // matching the soft-lock promise that live tabs survive.
                    // A post-unlock reconnect of a password-based quick host
                    // falls back to the interactive prompt.
                    for entry in self.quick_connects.values_mut() {
                        entry.password = None;
                        entry.totp_secret = None;
                        entry.proxy_password = None;
                    }
                    // Land the keyboard in the unlock field so the user
                    // returning to the machine just types the password.
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "vault-unlock-password",
                    ));
                }
            }
            VaultMessage::LockVault => {
                if let Some(vault) = &mut self.vault {
                    vault.lock();
                    if self.vault_ui.has_user_password {
                        self.vault_ui.state = VaultState::Locked;
                        // The in-memory master password dies with the
                        // lock, like the soft lock already does (it
                        // feeds biometric enroll and the MCP config
                        // embed; neither may outlive the vault key).
                        self.master_password = None;
                        // ssh-agent goes dark on lock (listener stays up).
                        self.agent_on_lock();
                        // And the MCP panel's typed confirm buffer.
                        self.mcp.vault_pw_prompt = None;
                        self.mcp.vault_pw_error = false;
                        // Same reset as the soft lock: lead with biometrics.
                        self.vault_ui.password_fallback = false;
                        self.connections.clear();
                        self.quick_connects.clear();
                        self.keys.clear();
                        self.snippets.clear();
                        self.groups.clear();
                        // Close live remote sessions, not just the panes
                        // referencing them, so locking the vault really
                        // severs the remote connections.
                        for tab in &self.tabs {
                            Self::close_tab_sessions(tab);
                        }
                        // Drop RDP/VNC tunnels too (each Arc drop cancels
                        // the -L forward); locking severs everything.
                        self.remote_desktop_forwards.clear();
                        self.tabs.clear();
                        self.active_tab = None;
                        self.clear_terminal_tab_memory();
                        self.active_view = View::Dashboard;
                        // Mirror the soft-lock UI sweep: the manual lock
                        // used to leave overlays, side panels, revealed
                        // secrets and pending auth prompts armed behind
                        // the lock screen (stale state a stray key or a
                        // late async completion could act on, and typed
                        // or revealed secrets have no business surviving
                        // an explicit "I'm done").
                        self.revealed_secrets.clear();
                        // History content-search results hold decrypted
                        // command lines / output excerpts; sweep like the
                        // soft lock does.
                        self.history_content_reset();
                        self.overlay = None;
                        self.card_context_menu = None;
                        // Top-strip pickers: same reason as the soft lock,
                        // a stray key must not drive the hidden surface (the
                        // command palette could dispatch an action) behind
                        // the lock screen.
                        self.close_modal(crate::state::Modal::CommandPalette);
                        self.close_modal(crate::state::Modal::TabJump);
                        self.close_modal(crate::state::Modal::NewTabPicker);
                        self.error_dialog = None;
                        self.show_host_panel = false;
                        self.host_panel_error = None;
                        self.editor_form = crate::state::ConnectionForm::default();
                        self.show_key_generate_panel = false;
                        self.key_generate_form = crate::state::KeyGenerateForm::default();
                        self.show_key_panel = false;
                        self.key_import_form = crate::state::KeyImportForm::default();
                        self.key_import_content = iced::widget::text_editor::Content::new();
                        self.key_import_public_content = iced::widget::text_editor::Content::new();
                        self.key_import_cert_content = iced::widget::text_editor::Content::new();
                        self.cert_viewer = None;
                        // Decrypted session-recording bytes (player grid /
                        // rendered viewer spans) are secret-bearing and
                        // have no business surviving an explicit "I'm
                        // done"; the soft lock sweeps these too.
                        self.session_player = None;
                        self.viewing_session_log = None;
                        self.vault_ui.new_password.clear();
                        self.vault_ui.confirm_password.clear();
                        // Abort an in-flight KDF calibration (snapshot is
                        // secret material; the apply must not land post-lock).
                        self.vault_ui.pending_kdf_pw = None;
                        self.sftp.picker_open = false;
                        self.sftp.new_entry = None;
                        self.sftp.delete_confirm.clear();
                        self.sftp.edit_session = None;
                        self.sftp.edit_watches.clear();
                        for tab in self.sftp_tabs.iter_mut() {
                            tab.state.edit_session = None;
                            tab.state.edit_watches.clear();
                        }
                        for tab in self.tabs.iter_mut() {
                            tab.files_state.edit_session = None;
                            tab.files_state.edit_watches.clear();
                        }
                        self.monitor_reset_all();
                        self.sftp.overwrite_prompt = None;
                        self.sftp.properties = None;
                        // Cancel a pending keyboard-interactive / host-key
                        // prompt from an in-flight connect (the sessions
                        // were just torn down; the engine treats `None` /
                        // `false` as a clean abort).
                        if self.pending_kbi_prompt.take().is_some() {
                            self.kbi_inputs.clear();
                            if let Some(ref tx) = self.kbi_response_tx {
                                let _ = tx.try_send(None);
                            }
                        }
                        if self.pending_host_key.take().is_some()
                            && let Some(tx) = self.active_host_key_tx.take()
                        {
                            let _ = tx.try_send(false);
                        }
                        self.pending_kbi_quick = None;
                        self.pending_auth_switch = None;
                        // Same auto-focus as the soft lock: the unlock
                        // field is the only thing to interact with.
                        return iced::widget::operation::focus(iced::widget::Id::new(
                            "vault-unlock-password",
                        ));
                    } else {
                        // No user password: re-open immediately
                        let _ = vault.open_without_password();
                    }
                }
            }

            // ── Biometric (OS-keystore) unlock ──
            VaultMessage::ToggleBiometricUnlock => {
                if self.setting_biometric_unlock_enabled {
                    // Opt out: forget the stored secret unconditionally, then
                    // flip the setting off and persist.
                    self.biometric_forget();
                    self.setting_biometric_unlock_enabled = false;
                    self.persist_setting("biometric_unlock_enabled", "false");
                } else {
                    // Opt in: needs an available backend and the master
                    // password in hand (we are unlocked). Enroll first; only
                    // turn the setting on if the store accepted it.
                    if !self.biometric_available {
                        return self.show_toast(
                            crate::i18n::t("biometric_unlock_failed").to_string(),
                        );
                    }
                    let Some(pw) = self.master_password.clone() else {
                        return self.show_toast(
                            crate::i18n::t("biometric_unlock_failed").to_string(),
                        );
                    };
                    match self.biometric_vault().map(|bv| bv.enroll(&pw)) {
                        Some(Ok(())) => {
                            self.setting_biometric_unlock_enabled = true;
                            self.persist_setting("biometric_unlock_enabled", "true");
                        }
                        _ => {
                            return self.show_toast(
                                crate::i18n::t("biometric_unlock_failed").to_string(),
                            );
                        }
                    }
                }
            }
            VaultMessage::BiometricUnlockRequested => {
                let Some(bv) = self.biometric_vault() else {
                    return Task::none();
                };
                // Localized reason line for the OS prompt (Touch ID sheet /
                // Hello dialog); captured before the move into the worker.
                let prompt = crate::i18n::t("biometric_unlock").to_string();
                // The retrieval blocks on the OS presence prompt, so run it
                // off the UI thread and route the outcome back as a message.
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            bv.unlock_secret(&prompt).map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    |v| Message::Vault(VaultMessage::BiometricUnlockResult(v)),
                );
            }
            VaultMessage::BiometricUnlockResult(res) => match res {
                Ok(password) => {
                    // Feed the released password into the ordinary unlock
                    // path (which sets `master_password`, boots sync, etc).
                    self.vault_ui.password_input = password;
                    return Task::done(Message::Vault(VaultMessage::VaultUnlock));
                }
                Err(e) => {
                    tracing::warn!("biometric unlock failed: {e}");
                    self.vault_ui.error =
                        Some(crate::i18n::t("biometric_unlock_failed").to_string());
                    // Drop to the typed-password layout so the user is
                    // never stuck on a prompt the OS keeps rejecting, and
                    // focus the input the error just told them to use.
                    self.vault_ui.password_fallback = true;
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "vault-unlock-password",
                    ));
                }
            },
            VaultMessage::VaultShowPasswordFallback => {
                // Biometric-first lock screen: reveal the typed-password
                // form. The biometric button stays available below it, so
                // this is a per-lock choice, not a mode switch.
                self.vault_ui.password_fallback = true;
                self.vault_ui.error = None;
                return iced::widget::operation::focus(iced::widget::Id::new(
                    "vault-unlock-password",
                ));
            }
            VaultMessage::ToggleSetupBiometric => {
                self.vault_ui.setup_enable_biometric = !self.vault_ui.setup_enable_biometric;
            }

            // ── Vault password management ──
            VaultMessage::ToggleVaultPassword => {
                if self.vault_ui.has_user_password {
                    // Removing encryption is destructive: arm the confirm
                    // prompt instead of dropping the password on a single
                    // click. The switch stays on until the user confirms.
                    // Close the change form so the two can't stack.
                    self.vault_ui.confirm_remove_password = true;
                    self.vault_ui.change_password_open = false;
                    self.vault_ui.password_error = None;
                } else {
                    // No password yet: the switch reveals / hides the inline
                    // set-password form. Nothing is committed until the user
                    // types, confirms, and presses Set Password.
                    self.vault_ui.show_password_form = !self.vault_ui.show_password_form;
                    // Offer the biometric opt-in pre-checked whenever the
                    // platform can service it (password creation is the
                    // natural moment to enable the convenience layer).
                    self.vault_ui.setup_enable_biometric = self.biometric_available;
                    self.vault_ui.new_password.clear();
                    self.vault_ui.confirm_password.clear();
                    self.vault_ui.password_error = None;
                    // Hiding the form is a cancel: abort any calibration
                    // that is still in flight (see `pending_kdf_pw`).
                    self.vault_ui.pending_kdf_pw = None;
                }
            }
            VaultMessage::ConfirmRemoveVaultPassword => {
                if let Some(vault) = &mut self.vault {
                    match vault.remove_user_password() {
                        Ok(()) => {
                            self.vault_ui.has_user_password = false;
                            self.vault_ui.show_password_form = false;
                            self.vault_ui.confirm_remove_password = false;
                            self.vault_ui.password_error = None;
                            self.vault_ui.new_password.clear();
                            self.vault_ui.confirm_password.clear();
                            // A passwordless vault has nothing to gate, so
                            // drop any biometric enrollment and turn the
                            // setting off (it would otherwise dangle on).
                            if self.setting_biometric_unlock_enabled {
                                self.biometric_forget();
                                self.setting_biometric_unlock_enabled = false;
                                self.persist_setting("biometric_unlock_enabled", "false");
                            }
                        }
                        Err(e) => {
                            self.vault_ui.password_error = Some(e.to_string());
                        }
                    }
                }
            }
            VaultMessage::CancelRemoveVaultPassword => {
                self.vault_ui.confirm_remove_password = false;
                self.vault_ui.password_error = None;
            }
            VaultMessage::VaultNewPasswordChanged(pw) => {
                self.vault_ui.new_password = pw;
            }
            VaultMessage::VaultConfirmPasswordChanged(pw) => {
                self.vault_ui.confirm_password = pw;
            }
            VaultMessage::SetVaultPassword => {
                if self.vault_ui.new_password.len() < 4 {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("password_too_short").to_string());
                    return Task::none();
                }
                // Both fields are hidden, so a typo would otherwise be
                // invisible until the next unlock (when it's too late).
                if self.vault_ui.new_password != self.vault_ui.confirm_password {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("passwords_do_not_match").to_string());
                    return Task::none();
                }
                // Phase 1 (E1): calibrate off-thread, apply on callback.
                if self.vault_ui.calibrating {
                    return Task::none();
                }
                self.vault_ui.calibrating = true;
                self.vault_ui.password_error = None;
                self.vault_ui.pending_kdf_pw = Some(self.vault_ui.new_password.clone());
                return calibrate_kdf_task(crate::state::VaultPwOp::SetUser);
            }
            VaultMessage::OpenChangeVaultPassword => {
                // Reveal the change form; start from a clean slate so a
                // stale value from a previous open can't leak in. Dismiss
                // any armed remove-confirm so the two can't stack.
                self.vault_ui.change_password_open = true;
                self.vault_ui.confirm_remove_password = false;
                self.vault_ui.current_password.clear();
                self.vault_ui.new_password.clear();
                self.vault_ui.confirm_password.clear();
                self.vault_ui.password_error = None;
            }
            VaultMessage::CancelChangeVaultPassword => {
                self.vault_ui.change_password_open = false;
                self.vault_ui.current_password.clear();
                self.vault_ui.new_password.clear();
                self.vault_ui.confirm_password.clear();
                self.vault_ui.password_error = None;
                // Cancel during the ~1s calibration: dropping the snapshot
                // aborts the pending apply, so the rotation can't land after
                // the user backed out.
                self.vault_ui.pending_kdf_pw = None;
            }
            VaultMessage::VaultCurrentPasswordChanged(pw) => {
                self.vault_ui.current_password = pw;
            }
            VaultMessage::ConfirmChangeVaultPassword => {
                if self.vault_ui.new_password.len() < 4 {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("password_too_short").to_string());
                    return Task::none();
                }
                if self.vault_ui.new_password != self.vault_ui.confirm_password {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("passwords_do_not_match").to_string());
                    return Task::none();
                }
                if self.vault_ui.calibrating {
                    return Task::none();
                }
                // Verify the current password BEFORE the (slow) calibration:
                // no reason to calibrate for a change that will be rejected.
                // The vault is already unlocked, so this guards against
                // someone changing the password at an unattended session and
                // against a typo silently rotating to an unknown key.
                if let Some(vault) = &self.vault {
                    match vault.verify_password(&self.vault_ui.current_password) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.vault_ui.password_error = Some(
                                crate::i18n::t("current_password_incorrect").to_string(),
                            );
                            return Task::none();
                        }
                        Err(e) => {
                            self.vault_ui.password_error = Some(e.to_string());
                            return Task::none();
                        }
                    }
                }
                // Phase 1 (E1): current verified, calibrate off-thread.
                self.vault_ui.calibrating = true;
                self.vault_ui.password_error = None;
                self.vault_ui.pending_kdf_pw = Some(self.vault_ui.new_password.clone());
                return calibrate_kdf_task(crate::state::VaultPwOp::Change);
            }
            VaultMessage::VaultKdfCalibrated(op, params) => {
                // Phase 2 (E1): apply the pending set / change-password with
                // the tuned KDF params. The ~1s derive here runs on the UI
                // thread, same cost as an unlock (the plan accepts that);
                // only the multi-probe calibration went off-thread.
                self.vault_ui.calibrating = false;
                // The password to apply is the snapshot taken when the user
                // confirmed, never the live buffers (they may have been
                // edited or cleared during the calibration). A missing
                // snapshot means the flow was cancelled: discard the result.
                let Some(pw) = self.vault_ui.pending_kdf_pw.take() else {
                    return Task::none();
                };
                let Some(vault) = &mut self.vault else {
                    return Task::none();
                };
                match op {
                    crate::state::VaultPwOp::FirstSetup => {
                        match vault.set_master_password_with_params(&pw, params) {
                            Ok(()) => {
                                let _ = vault.set_setting("has_user_password", "1");
                                self.vault_ui.has_user_password = true;
                                self.vault_ui.state = VaultState::Unlocked;
                                self.vault_ui.error = None;
                                self.master_password = Some(pw.clone());
                                let bio_task = self
                                    .biometric_setup_enroll(&pw)
                                    .unwrap_or_else(Task::none);
                                self.vault_ui.password_input.clear();
                                self.vault_ui.password_visible = false;
                                self.load_data_from_vault();
                                return Task::batch([
                                    bio_task,
                                    self.agent_boot_task(),
                                    self.take_perf_mode_toast_task(),
                                    iced::widget::operation::focus(iced::widget::Id::new(
                                        "search-dashboard",
                                    )),
                                ]);
                            }
                            Err(e) => self.vault_ui.error = Some(e.to_string()),
                        }
                    }
                    crate::state::VaultPwOp::SetUser => {
                        match vault.set_user_password_with_params(&pw, params) {
                            Ok(()) => {
                                self.vault_ui.has_user_password = true;
                                self.vault_ui.show_password_form = false;
                                self.vault_ui.password_error = None;
                                self.master_password = Some(pw.clone());
                                let bio_task = self.biometric_setup_enroll(&pw);
                                self.vault_ui.new_password.clear();
                                self.vault_ui.confirm_password.clear();
                                if let Some(toast) = bio_task {
                                    return toast;
                                }
                            }
                            Err(e) => self.vault_ui.password_error = Some(e.to_string()),
                        }
                    }
                    crate::state::VaultPwOp::Change => {
                        match vault.set_user_password_with_params(&pw, params) {
                            Ok(()) => {
                                self.vault_ui.change_password_open = false;
                                self.vault_ui.password_error = None;
                                self.master_password = Some(pw.clone());
                                self.biometric_reenroll(&pw);
                                self.vault_ui.current_password.clear();
                                self.vault_ui.new_password.clear();
                                self.vault_ui.confirm_password.clear();
                                return self.show_toast(
                                    crate::i18n::t("password_updated").to_string(),
                                );
                            }
                            Err(e) => self.vault_ui.password_error = Some(e.to_string()),
                        }
                    }
                }
            }
        }
        Task::none()
    }
}

/// Phase 1 of an E1 set / change-password flow: run the Argon2id KDF
/// calibration on a blocking worker thread (it does several ~100 ms
/// hashes), then fire `VaultKdfCalibrated` so the handler applies the
/// vault mutation with the tuned parameters. A calibration that somehow
/// fails resolves to the default profile rather than blocking the flow.
fn calibrate_kdf_task(op: crate::state::VaultPwOp) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(oryxis_vault::calibrate_kdf)
                .await
                .unwrap_or(oryxis_vault::KdfParams::DEFAULT)
        },
        move |params| Message::Vault(VaultMessage::VaultKdfCalibrated(op, params)),
    )
}
