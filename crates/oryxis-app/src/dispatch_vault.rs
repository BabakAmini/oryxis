//! `Oryxis::handle_vault`: settings-panel-independent dispatch arms for the
//! vault area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{Message, Oryxis};
use crate::state::VaultState;
use oryxis_vault::VaultError;

impl Oryxis {
    pub(crate) fn handle_vault(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // -- Vault --
            Message::VaultPasswordChanged(pw) => {
                self.vault_ui.password_input = pw;
            }
            Message::VaultTogglePasswordVisibility => {
                self.vault_ui.password_visible = !self.vault_ui.password_visible;
            }
            Message::VaultSetup => {
                if self.vault_ui.password_input.len() < 4 {
                    self.vault_ui.error =
                        Some(crate::i18n::t("password_too_short").to_string());
                    return Ok(Task::none());
                }
                if let Some(vault) = &mut self.vault {
                    match vault.set_master_password(&self.vault_ui.password_input) {
                        Ok(()) => {
                            let _ = vault.set_setting("has_user_password", "1");
                            self.vault_ui.has_user_password = true;
                            self.vault_ui.state = VaultState::Unlocked;
                            self.vault_ui.error = None;
                            // Cache for child-window spawn.
                            self.master_password = Some(self.vault_ui.password_input.clone());
                            // Onboarding's biometric opt-in enrolls the
                            // freshly created password in the same flow
                            // (before the input buffer is cleared).
                            let bio_task = self
                                .biometric_setup_enroll(&self.vault_ui.password_input.clone())
                                .unwrap_or_else(Task::none);
                            self.vault_ui.password_input.clear();
                            self.vault_ui.password_visible = false;
                            self.load_data_from_vault();
                            return Ok(Task::batch([
                                bio_task,
                                self.agent_boot_task(),
                                self.take_perf_mode_toast_task(),
                                iced::widget::operation::focus(iced::widget::Id::new(
                                    "search-dashboard",
                                )),
                            ]));
                        }
                        Err(e) => {
                            self.vault_ui.error = Some(e.to_string());
                        }
                    }
                }
            }
            Message::VaultSkipPassword => {
                if let Some(vault) = &mut self.vault {
                    match vault.open_without_password() {
                        Ok(()) => {
                            self.vault_ui.state = VaultState::Unlocked;
                            self.vault_ui.error = None;
                            self.load_data_from_vault();
                            return Ok(Task::batch([
                                self.agent_boot_task(),
                                self.take_perf_mode_toast_task(),
                                iced::widget::operation::focus(iced::widget::Id::new(
                                    "search-dashboard",
                                )),
                            ]));
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
            Message::VaultDestroyConfirm => {
                self.vault_ui.destroy_confirm = !self.vault_ui.destroy_confirm;
            }
            Message::VaultDestroy => {
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
            Message::VaultUnlock => {
                // Ignore the submit when no password was typed (pressing
                // Enter on an empty field or clicking Unlock with it blank
                // shouldn't run a doomed unlock attempt or surface an error).
                if self.vault_ui.password_input.is_empty() {
                    return Ok(Task::none());
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
                            // After a manual unlock, fire any deferred
                            // `--connect <uuid>` from the launch CLI args.
                            if let Some(connect_id) = self.pending_auto_connect.take()
                                && let Some(idx) = self
                                    .connections
                                    .iter()
                                    .position(|c| c.id == connect_id)
                            {
                                unlock_tasks.push(Task::done(Message::ConnectSsh(idx)));
                            } else {
                                // Land on Home with the host search focused
                                // so the user can type / keyboard-navigate
                                // immediately (matches ChangeView behavior).
                                unlock_tasks.push(iced::widget::operation::focus(
                                    iced::widget::Id::new("search-dashboard"),
                                ));
                            }
                            return Ok(Task::batch(unlock_tasks));
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

            // ── Biometric (OS-keystore) unlock ──
            Message::ToggleBiometricUnlock => {
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
                        return Ok(self.show_toast(
                            crate::i18n::t("biometric_unlock_failed").to_string(),
                        ));
                    }
                    let Some(pw) = self.master_password.clone() else {
                        return Ok(self.show_toast(
                            crate::i18n::t("biometric_unlock_failed").to_string(),
                        ));
                    };
                    match self.biometric_vault().map(|bv| bv.enroll(&pw)) {
                        Some(Ok(())) => {
                            self.setting_biometric_unlock_enabled = true;
                            self.persist_setting("biometric_unlock_enabled", "true");
                        }
                        _ => {
                            return Ok(self.show_toast(
                                crate::i18n::t("biometric_unlock_failed").to_string(),
                            ));
                        }
                    }
                }
            }
            Message::BiometricUnlockRequested => {
                let Some(bv) = self.biometric_vault() else {
                    return Ok(Task::none());
                };
                // Localized reason line for the OS prompt (Touch ID sheet /
                // Hello dialog); captured before the move into the worker.
                let prompt = crate::i18n::t("biometric_unlock").to_string();
                // The retrieval blocks on the OS presence prompt, so run it
                // off the UI thread and route the outcome back as a message.
                return Ok(Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            bv.unlock_secret(&prompt).map_err(|e| e.to_string())
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    Message::BiometricUnlockResult,
                ));
            }
            Message::BiometricUnlockResult(res) => match res {
                Ok(password) => {
                    // Feed the released password into the ordinary unlock
                    // path (which sets `master_password`, boots sync, etc).
                    self.vault_ui.password_input = password;
                    return Ok(Task::done(Message::VaultUnlock));
                }
                Err(e) => {
                    tracing::warn!("biometric unlock failed: {e}");
                    self.vault_ui.error =
                        Some(crate::i18n::t("biometric_unlock_failed").to_string());
                    // Drop to the typed-password layout so the user is
                    // never stuck on a prompt the OS keeps rejecting, and
                    // focus the input the error just told them to use.
                    self.vault_ui.password_fallback = true;
                    return Ok(iced::widget::operation::focus(iced::widget::Id::new(
                        "vault-unlock-password",
                    )));
                }
            },
            Message::VaultShowPasswordFallback => {
                // Biometric-first lock screen: reveal the typed-password
                // form. The biometric button stays available below it, so
                // this is a per-lock choice, not a mode switch.
                self.vault_ui.password_fallback = true;
                self.vault_ui.error = None;
                return Ok(iced::widget::operation::focus(iced::widget::Id::new(
                    "vault-unlock-password",
                )));
            }
            Message::ToggleSetupBiometric => {
                self.vault_ui.setup_enable_biometric = !self.vault_ui.setup_enable_biometric;
            }

            // ── Vault password management ──
            Message::ToggleVaultPassword => {
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
                }
            }
            Message::ConfirmRemoveVaultPassword => {
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
            Message::CancelRemoveVaultPassword => {
                self.vault_ui.confirm_remove_password = false;
                self.vault_ui.password_error = None;
            }
            Message::VaultNewPasswordChanged(pw) => {
                self.vault_ui.new_password = pw;
            }
            Message::VaultConfirmPasswordChanged(pw) => {
                self.vault_ui.confirm_password = pw;
            }
            Message::SetVaultPassword => {
                if self.vault_ui.new_password.len() < 4 {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("password_too_short").to_string());
                    return Ok(Task::none());
                }
                // Both fields are hidden, so a typo would otherwise be
                // invisible until the next unlock (when it's too late).
                if self.vault_ui.new_password != self.vault_ui.confirm_password {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("passwords_do_not_match").to_string());
                    return Ok(Task::none());
                }
                if let Some(vault) = &mut self.vault {
                    match vault.set_user_password(&self.vault_ui.new_password) {
                        Ok(()) => {
                            self.vault_ui.has_user_password = true;
                            self.vault_ui.show_password_form = false;
                            self.vault_ui.password_error = None;
                            // Track the newly-set password in memory (a
                            // previously passwordless vault had none), so
                            // sync and a subsequent biometric opt-in have
                            // the credential to work with.
                            self.master_password =
                                Some(self.vault_ui.new_password.clone());
                            // The form's biometric opt-in enrolls the new
                            // password in the same flow (before the form
                            // buffers are cleared).
                            let bio_task = self
                                .biometric_setup_enroll(&self.vault_ui.new_password.clone());
                            self.vault_ui.new_password.clear();
                            self.vault_ui.confirm_password.clear();
                            if let Some(toast) = bio_task {
                                return Ok(toast);
                            }
                        }
                        Err(e) => {
                            self.vault_ui.password_error = Some(e.to_string());
                        }
                    }
                }
            }
            Message::OpenChangeVaultPassword => {
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
            Message::CancelChangeVaultPassword => {
                self.vault_ui.change_password_open = false;
                self.vault_ui.current_password.clear();
                self.vault_ui.new_password.clear();
                self.vault_ui.confirm_password.clear();
                self.vault_ui.password_error = None;
            }
            Message::VaultCurrentPasswordChanged(pw) => {
                self.vault_ui.current_password = pw;
            }
            Message::ConfirmChangeVaultPassword => {
                if self.vault_ui.new_password.len() < 4 {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("password_too_short").to_string());
                    return Ok(Task::none());
                }
                if self.vault_ui.new_password != self.vault_ui.confirm_password {
                    self.vault_ui.password_error =
                        Some(crate::i18n::t("passwords_do_not_match").to_string());
                    return Ok(Task::none());
                }
                if let Some(vault) = &mut self.vault {
                    // Verify the current password before rotating. The vault
                    // is already unlocked, so this guards against someone
                    // changing the password at an unattended session, and
                    // against a typo silently rotating to an unknown key.
                    match vault.verify_password(&self.vault_ui.current_password) {
                        Ok(true) => match vault.set_user_password(&self.vault_ui.new_password) {
                            Ok(()) => {
                                self.vault_ui.change_password_open = false;
                                self.vault_ui.password_error = None;
                                // The in-memory password must track the
                                // rotation, or sync (which re-opens the vault
                                // with `master_password`) would keep using
                                // the old one after a change.
                                self.master_password =
                                    Some(self.vault_ui.new_password.clone());
                                // Refresh the OS-keystore secret to the new
                                // password in the same flow, so biometric
                                // unlock doesn't silently break on a stale
                                // secret. No-op unless opted in.
                                self.biometric_reenroll(&self.vault_ui.new_password);
                                self.vault_ui.current_password.clear();
                                self.vault_ui.new_password.clear();
                                self.vault_ui.confirm_password.clear();
                                return Ok(self.show_toast(
                                    crate::i18n::t("password_updated").to_string(),
                                ));
                            }
                            Err(e) => {
                                self.vault_ui.password_error = Some(e.to_string());
                            }
                        },
                        Ok(false) => {
                            self.vault_ui.password_error =
                                Some(crate::i18n::t("current_password_incorrect").to_string());
                        }
                        Err(e) => {
                            self.vault_ui.password_error = Some(e.to_string());
                        }
                    }
                }
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
