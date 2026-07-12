//! `Oryxis::handle_keys`, match arms for the Keys + Identities
//! panels: import/edit/delete keys, manage identities, keychain menu.

#![allow(clippy::result_large_err)]

use iced::widget::text_editor;
use iced::Task;

use oryxis_core::models::identity::Identity;
use oryxis_vault::VaultError;

use crate::app::{Message, Oryxis};
use crate::state::{OverlayContent, OverlayState, View};

impl Oryxis {
    pub(crate) fn handle_keys(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // -- Keys --
            Message::ShowKeyPanel => {
                // Also navigate to the Keys screen, the import panel is rendered
                // inside view_keys(), so the user needs to be there to see it
                // (e.g. when they click "+ Key" from the host editor).
                self.active_view = View::Keys;
                self.active_tab = None;
                self.show_key_panel = true;
                self.key_import_form.label.clear();
                self.key_import_content = text_editor::Content::new();
                self.key_import_form.pem.clear();
                self.key_import_form.passphrase.clear();
                self.key_import_form.passphrase_required = false;
                self.key_import_form.passphrase_visible = false;
                self.key_error = None;
                self.key_success = None;
                self.key_import_form.editing_id = None;
                self.key_context_menu = None;
                self.overlay = None;
            }
            Message::HideKeyPanel => {
                self.show_key_panel = false;
                self.key_import_form.editing_id = None;
                self.key_import_form.passphrase.clear();
                self.key_import_form.passphrase_required = false;
                self.key_import_form.passphrase_visible = false;
                // Errors raised inside the sidebar are scoped to it.
                // Closing the panel discards that context so the main
                // keychain area doesn't inherit a stale message.
                self.key_error = None;
                self.key_success = None;
            }
            // -- Key generation (keychain > ADD > Generate key) --
            Message::ShowKeyGeneratePanel => {
                self.active_view = View::Keys;
                self.active_tab = None;
                // Mutually exclusive with the import/identity panels.
                self.show_key_panel = false;
                self.show_identity_panel = false;
                self.show_key_generate_panel = true;
                self.key_generate_form = crate::state::KeyGenerateForm::default();
                self.key_error = None;
                self.key_success = None;
                self.key_context_menu = None;
                self.overlay = None;
            }
            Message::HideKeyGeneratePanel => {
                self.show_key_generate_panel = false;
                self.key_generate_form = crate::state::KeyGenerateForm::default();
            }
            Message::KeyGenLabelChanged(v) => {
                self.key_generate_form.label = v;
                self.key_generate_form.error = None;
            }
            Message::KeyGenCommentChanged(v) => self.key_generate_form.comment = v,
            Message::KeyGenAlgoSelected(a) => self.key_generate_form.algo = a,
            Message::KeyGenBitsSelected(b) => self.key_generate_form.rsa_bits = b,
            Message::KeyGenCurveSelected(c) => self.key_generate_form.ecdsa_curve = c,
            Message::GenerateKey => {
                if self.key_generate_form.working {
                    return Ok(Task::none());
                }
                let label = self.key_generate_form.label.trim().to_string();
                if label.is_empty() {
                    self.key_generate_form.error =
                        Some(crate::i18n::t("keygen_label_required").to_string());
                    return Ok(Task::none());
                }
                let comment = self.key_generate_form.comment.trim().to_string();
                let spec = match self.key_generate_form.algo {
                    crate::state::KeyGenAlgo::Ed25519 => oryxis_vault::GenerateSpec::Ed25519,
                    crate::state::KeyGenAlgo::Rsa => oryxis_vault::GenerateSpec::Rsa {
                        bits: self.key_generate_form.rsa_bits,
                    },
                    crate::state::KeyGenAlgo::Ecdsa => oryxis_vault::GenerateSpec::Ecdsa {
                        curve: self.key_generate_form.ecdsa_curve,
                    },
                };
                self.key_generate_form.working = true;
                self.key_generate_form.error = None;
                // RSA 4096 takes seconds: run on the blocking pool so
                // the UI keeps painting the spinner.
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(move || {
                        oryxis_vault::generate_key(&label, &comment, spec)
                            .map(std::sync::Arc::new)
                            .map_err(|e| e.to_string())
                    }),
                    |result| match result {
                        Ok(inner) => Message::KeyGenerated(inner),
                        Err(e) => Message::KeyGenerated(Err(format!("Thread error: {}", e))),
                    },
                ));
            }
            Message::KeyGenerated(result) => {
                self.key_generate_form.working = false;
                match result {
                    Ok(generated) => {
                        // A soft auto-lock may have landed while the task
                        // ran; a locked vault cannot encrypt the private
                        // material, so the generated key is dropped (the
                        // panel was already swept by the lock sweep).
                        if self.vault_ui.state != crate::state::VaultState::Unlocked {
                            return Ok(Task::none());
                        }
                        let Some(vault) = &self.vault else {
                            return Ok(Task::none());
                        };
                        if let Err(e) =
                            vault.save_key(&generated.key, Some(&generated.private_pem))
                        {
                            self.key_generate_form.error = Some(e.to_string());
                            return Ok(Task::none());
                        }
                        self.keys = vault.list_keys().unwrap_or_default();
                        self.key_generate_form.result =
                            Some(crate::state::GeneratedKeyView {
                                id: generated.key.id,
                                label: generated.key.label.clone(),
                                fingerprint: generated.key.fingerprint.clone(),
                                public_key: generated.key.public_key.clone(),
                            });
                    }
                    Err(e) => self.key_generate_form.error = Some(e),
                }
            }
            Message::CopyGeneratedPublicKey => {
                if let Some(result) = &self.key_generate_form.result {
                    return Ok(iced::clipboard::write(result.public_key.clone()).discard());
                }
            }
            Message::SaveGeneratedPublicKeyFile => {
                let Some(result) = self.key_generate_form.result.clone() else {
                    return Ok(Task::none());
                };
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(move || {
                        let file = rfd::FileDialog::new()
                            .set_title("Save public key")
                            .set_file_name(format!("{}.pub", sanitize_key_filename(&result.label)))
                            .save_file();
                        match file {
                            Some(path) => std::fs::write(&path, format!("{}\n", result.public_key))
                                .map_err(|e| format!("Failed to write: {}", e)),
                            None => Err("cancelled".to_string()),
                        }
                    }),
                    |result| match result {
                        Ok(Ok(())) => Message::KeyFileBrowseError(String::new()),
                        Ok(Err(e)) => Message::KeyFileBrowseError(e),
                        Err(e) => Message::KeyFileBrowseError(format!("Thread error: {}", e)),
                    },
                ));
            }
            Message::KeyGenExportPassphraseChanged(v) => {
                self.key_generate_form.export_passphrase = v;
                self.key_generate_form.error = None;
            }
            Message::KeyGenExportPassphraseConfirmChanged(v) => {
                self.key_generate_form.export_passphrase_confirm = v;
                self.key_generate_form.error = None;
            }
            Message::ExportGeneratedPrivateKey => {
                let Some(result) = self.key_generate_form.result.clone() else {
                    return Ok(Task::none());
                };
                let pass = self.key_generate_form.export_passphrase.clone();
                let confirm = self.key_generate_form.export_passphrase_confirm.clone();
                if pass != confirm {
                    self.key_generate_form.error =
                        Some(crate::i18n::t("keygen_passphrase_mismatch").to_string());
                    return Ok(Task::none());
                }
                // The PEM is re-read from the vault (never held in form
                // state) and passphrase-encrypted here when one was
                // given; an empty pair exports plaintext (the panel
                // shows an explicit warning line for that case).
                let pem = match self.vault.as_ref().map(|v| v.get_key_private(&result.id)) {
                    Some(Ok(Some(pem))) => pem,
                    Some(Ok(None)) | None => {
                        self.key_generate_form.error = Some("key not found".into());
                        return Ok(Task::none());
                    }
                    Some(Err(e)) => {
                        self.key_generate_form.error = Some(e.to_string());
                        return Ok(Task::none());
                    }
                };
                let payload = if pass.is_empty() {
                    pem
                } else {
                    match oryxis_vault::encrypt_private_pem(&pem, &pass) {
                        Ok(enc) => enc,
                        Err(e) => {
                            self.key_generate_form.error = Some(e.to_string());
                            return Ok(Task::none());
                        }
                    }
                };
                let name = sanitize_key_filename(&result.label);
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(move || {
                        let file = rfd::FileDialog::new()
                            .set_title("Export private key")
                            .set_file_name(name)
                            .save_file();
                        match file {
                            Some(path) => {
                                write_private_key_file(&path, &payload)
                                    .map_err(|e| format!("Failed to write: {}", e))
                            }
                            None => Err("cancelled".to_string()),
                        }
                    }),
                    |result| match result {
                        Ok(Ok(())) => Message::KeyFileBrowseError(String::new()),
                        Ok(Err(e)) => Message::KeyFileBrowseError(e),
                        Err(e) => Message::KeyFileBrowseError(format!("Thread error: {}", e)),
                    },
                ));
            }
            Message::KeyImportLabelChanged(v) => self.key_import_form.label = v,
            Message::KeyContentAction(action) => {
                self.key_import_content.perform(action);
                let new_text = self.key_import_content.text();
                // Re-detect on every edit. If the user pastes an encrypted
                // PEM, the passphrase row should appear; if they swap to an
                // unencrypted one, it should hide. Clearing the cached
                // passphrase prevents leftover input from being applied
                // against a different key.
                if new_text != self.key_import_form.pem {
                    let encrypted = oryxis_vault::is_key_encrypted(&new_text);
                    if encrypted != self.key_import_form.passphrase_required {
                        self.key_import_form.passphrase.clear();
                    }
                    self.key_import_form.passphrase_required = encrypted;
                }
                self.key_import_form.pem = new_text;
            }
            Message::KeyImportPassphraseChanged(v) => {
                self.key_import_form.passphrase = v;
                // Clear stale "wrong passphrase" feedback as the user types.
                self.key_error = None;
            }
            Message::KeyImportPassphraseToggleVisibility => {
                self.key_import_form.passphrase_visible = !self.key_import_form.passphrase_visible;
            }
            Message::BrowseKeyFile => {
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(|| {
                        let file = rfd::FileDialog::new()
                            .set_title("Select SSH Private Key")
                            .pick_file();
                        match file {
                            Some(path) => {
                                let filename = path
                                    .file_name()
                                    .map(|f| f.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "imported-key".into());
                                let content = std::fs::read_to_string(&path)
                                    .map_err(|e| format!("Failed to read: {}", e))?;
                                Ok((filename, content))
                            }
                            None => Err("cancelled".to_string()),
                        }
                    }),
                    |result| match result {
                        Ok(Ok((filename, content))) => Message::KeyFileLoaded(filename, content),
                        Ok(Err(e)) => Message::KeyFileBrowseError(e),
                        Err(e) => Message::KeyFileBrowseError(format!("Thread error: {}", e)),
                    },
                ));
            }
            Message::KeyFileLoaded(filename, content) => {
                if self.key_import_form.label.is_empty() {
                    self.key_import_form.label = filename;
                }
                self.key_import_content = text_editor::Content::with_text(&content);
                self.key_import_form.passphrase.clear();
                // Detect encryption now so the passphrase row appears as soon
                // as the file lands, not only after the user clicks Save.
                self.key_import_form.passphrase_required =
                    oryxis_vault::is_key_encrypted(&content);
                self.key_import_form.pem = content;
                self.show_key_panel = true;
                self.key_error = None;
                // The sidebar already shows "Loaded (X bytes)"; surfacing
                // a second toast in the main keychain area is just noise.
                self.key_success = None;
            }
            Message::KeyFileBrowseError(err) => {
                if !err.contains("cancelled") {
                    self.key_error = Some(err);
                }
            }
            Message::ImportKey => {
                if self.key_import_form.pem.is_empty() {
                    self.key_error = Some("Select a key file first".into());
                    return Ok(Task::none());
                }
                // If we already know the key is encrypted but the user
                // clicked Save with an empty passphrase, give explicit
                // feedback instead of silently leaving the row visible.
                if self.key_import_form.passphrase_required && self.key_import_form.passphrase.is_empty() {
                    self.key_error =
                        Some(crate::i18n::t("key_passphrase_required_msg").to_string());
                    return Ok(Task::none());
                }
                let label = if self.key_import_form.label.is_empty() {
                    "imported-key".to_string()
                } else {
                    self.key_import_form.label.clone()
                };
                let pass_opt = if self.key_import_form.passphrase.is_empty() {
                    None
                } else {
                    Some(self.key_import_form.passphrase.as_str())
                };
                match oryxis_vault::import_key(&label, &self.key_import_form.pem, pass_opt) {
                    Ok(mut generated) => {
                        // If editing an existing key, preserve its ID
                        if let Some(existing_id) = self.key_import_form.editing_id {
                            generated.key.id = existing_id;
                        }
                        if let Some(vault) = &self.vault {
                            match vault.save_key(&generated.key, Some(&generated.private_pem)) {
                                Ok(()) => {
                                    let verb = if self.key_import_form.editing_id.is_some() { "updated" } else { "imported" };
                                    self.key_error = None;
                                    self.key_success = Some(format!("Key '{}' {}", label, verb));
                                    self.key_import_form.label.clear();
                                    self.key_import_content = text_editor::Content::new();
                                    self.key_import_form.pem.clear();
                                    self.key_import_form.passphrase.clear();
                                    self.key_import_form.passphrase_required = false;
                                    self.key_import_form.passphrase_visible = false;
                                    self.show_key_panel = false;
                                    self.key_import_form.editing_id = None;
                                    self.load_data_from_vault();
                                }
                                Err(e) => self.key_error = Some(e.to_string()),
                            }
                        }
                    }
                    Err(VaultError::KeyNeedsPassphrase) => {
                        self.key_import_form.passphrase_required = true;
                        self.key_error = None;
                    }
                    Err(VaultError::WrongKeyPassphrase) => {
                        self.key_import_form.passphrase_required = true;
                        self.key_error = Some(crate::i18n::t("key_passphrase_wrong").to_string());
                    }
                    Err(VaultError::UnsupportedKeyKind(kind)) => {
                        self.key_error = Some(
                            crate::i18n::t("key_unsupported_kind").replace("{kind}", &kind),
                        );
                    }
                    Err(e) => self.key_error = Some(format!("Import failed: {}", e)),
                }
            }
            Message::RequestDeleteKey(idx) => {
                if let Some(key) = self.keys.get(idx) {
                    let name = key.label.clone();
                    self.confirm_remove(name, Message::DeleteKey(idx));
                }
            }
            Message::DeleteKey(idx) => {
                if let Some(key) = self.keys.get(idx) {
                    let id = key.id;
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_key(&id);
                        self.load_data_from_vault();
                        self.key_success = Some("Key deleted".into());
                    }
                }
                self.key_context_menu = None;
                self.overlay = None;
            }
            Message::ShowKeyMenu(idx) => {
                if self.key_context_menu == Some(idx) {
                    self.key_context_menu = None;
                    self.overlay = None;
                } else {
                    self.key_context_menu = Some(idx);
                    let anchor = self.keynav_take_menu_anchor();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::KeyActions(idx),
                        x: anchor.0,
                        y: anchor.1,
                    });
                }
            }
            Message::HideKeyMenu => {
                self.key_context_menu = None;
                self.identity_context_menu = None;
                self.show_keychain_add_menu = false;
                self.overlay = None;
            }
            Message::EditKey(idx) => {
                if let Some(key) = self.keys.get(idx) {
                    self.key_import_form.editing_id = Some(key.id);
                    self.key_import_form.label = key.label.clone();
                    // Load existing private key PEM from vault
                    let pem = self.vault.as_ref()
                        .and_then(|v| v.get_key_private(&key.id).ok().flatten())
                        .unwrap_or_default();
                    self.key_import_content = text_editor::Content::with_text(&pem);
                    self.key_import_form.pem = pem;
                    // Stored PEM is always unencrypted; no passphrase prompt.
                    self.key_import_form.passphrase.clear();
                    self.key_import_form.passphrase_required = false;
                    self.key_import_form.passphrase_visible = false;
                    self.show_key_panel = true;
                    self.key_error = None;
                    self.key_success = None;
                    self.key_context_menu = None;
                    self.overlay = None;
                }
            }
            Message::KeySearchChanged(v) => {
                self.key_search = v;
            }
            Message::SnippetSearchChanged(v) => {
                self.snippet_search = v;
            }
            Message::HistorySearchChanged(v) => {
                self.history_search = v;
            }

            // ── Identities ──
            Message::ShowIdentityPanel => {
                self.show_identity_panel = true;
                self.identity_form.label.clear();
                self.identity_form.username.clear();
                self.identity_form.password.clear();
                self.identity_form.key = None;
                self.identity_form.password_visible = false;
                self.identity_form.password_touched = false;
                self.identity_form.has_existing_password = false;
                self.identity_form.editing_id = None;
                self.show_keychain_add_menu = false;
                self.identity_context_menu = None;
                self.overlay = None;
            }
            Message::HideIdentityPanel => {
                self.show_identity_panel = false;
            }
            Message::IdentityLabelChanged(v) => {
                self.identity_form.label = v;
            }
            Message::IdentityUsernameChanged(v) => {
                self.identity_form.username = v;
            }
            Message::IdentityPasswordChanged(v) => {
                self.identity_form.password_touched = true;
                self.identity_form.password = v;
            }
            Message::IdentityTogglePasswordVisibility => {
                self.identity_form.password_visible = !self.identity_form.password_visible;
            }
            Message::IdentityKeyChanged(v) => {
                self.identity_form.key = if v == "(none)" { None } else { Some(v) };
            }
            Message::SaveIdentity => {
                if self.identity_form.label.trim().is_empty() {
                    return Ok(Task::none());
                }
                let mut identity = if let Some(id) = self.identity_form.editing_id {
                    self.identities.iter().find(|i| i.id == id).cloned()
                        .unwrap_or_else(|| Identity::new(""))
                } else {
                    Identity::new("")
                };
                identity.label = self.identity_form.label.clone();
                identity.username = if self.identity_form.username.is_empty() {
                    None
                } else {
                    Some(self.identity_form.username.clone())
                };
                identity.key_id = self.identity_form.key.as_ref().and_then(|label| {
                    self.keys.iter().find(|k| k.label == *label).map(|k| k.id)
                });
                identity.updated_at = chrono::Utc::now();

                let password = if !self.identity_form.password_touched {
                    None
                } else if self.identity_form.password.is_empty() {
                    Some("")
                } else {
                    Some(self.identity_form.password.as_str())
                };

                if let Some(vault) = &self.vault {
                    let _ = vault.save_identity(&identity, password);
                    self.load_data_from_vault();
                }
                self.show_identity_panel = false;
            }
            Message::EditIdentity(idx) => {
                if let Some(identity) = self.identities.get(idx) {
                    self.identity_form.editing_id = Some(identity.id);
                    self.identity_form.label = identity.label.clone();
                    self.identity_form.username = identity.username.clone().unwrap_or_default();
                    self.identity_form.password.clear();
                    self.identity_form.password_touched = false;
                    self.identity_form.password_visible = false;
                    self.identity_form.has_existing_password = self.vault.as_ref()
                        .and_then(|v| v.get_identity_password(&identity.id).ok().flatten())
                        .is_some();
                    self.identity_form.key = identity.key_id.and_then(|kid| {
                        self.keys.iter().find(|k| k.id == kid).map(|k| k.label.clone())
                    });
                    self.show_identity_panel = true;
                    self.identity_context_menu = None;
                    self.overlay = None;
                }
            }
            Message::RequestDeleteIdentity(idx) => {
                if let Some(identity) = self.identities.get(idx) {
                    let name = identity.label.clone();
                    self.confirm_remove(name, Message::DeleteIdentity(idx));
                }
            }
            Message::DeleteIdentity(idx) => {
                if let Some(identity) = self.identities.get(idx) {
                    let id = identity.id;
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_identity(&id);
                        self.load_data_from_vault();
                    }
                }
                self.identity_context_menu = None;
                self.overlay = None;
            }
            Message::ShowIdentityMenu(idx) => {
                if self.identity_context_menu == Some(idx) {
                    self.identity_context_menu = None;
                    self.overlay = None;
                } else {
                    self.identity_context_menu = Some(idx);
                    let anchor = self.keynav_take_menu_anchor();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::IdentityActions(idx),
                        x: anchor.0,
                        y: anchor.1,
                    });
                }
            }
            Message::ToggleKeychainAddMenu => {
                if self.show_keychain_add_menu {
                    self.show_keychain_add_menu = false;
                    self.overlay = None;
                } else {
                    self.show_keychain_add_menu = true;
                    // Anchor below the split button regardless of where
                    // the cursor was when the click happened. Right-align
                    // the menu's right edge with the chevron's right edge
                    // (= toolbar right padding from the visible content's
                    // right edge), and drop it just below the button row.
                    let panel_width = if self.show_key_panel || self.show_identity_panel {
                        crate::app::PANEL_WIDTH
                    } else {
                        0.0
                    };
                    // Sync with `views/layout.rs::view_main` overlay.
                    let menu_width = 150.0;
                    let toolbar_padding = 24.0;
                    // Toolbar uses dir_row, so under RTL the "+ ADD ▼"
                    // group sits at the leading (left) edge. The render
                    // path subtracts menu_width again under RTL; pre-
                    // compensate here so the final left edge lands at
                    // panel_width + toolbar_padding.
                    let x = if crate::i18n::is_rtl_layout() {
                        panel_width + toolbar_padding + menu_width
                    } else {
                        self.window_size.width
                            - panel_width
                            - toolbar_padding
                            - menu_width
                    };
                    // Helper accounts for the Workspace contextual
                    // sub-nav under the tab bar; in Classic it returns
                    // the legacy 56 px (toolbar top + button + gap).
                    let y = self.dashboard_dropdown_anchor_y();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::KeychainAdd,
                        x: x.max(0.0),
                        y,
                    });
                }
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}

/// A filesystem-safe file stem from a key label.
fn sanitize_key_filename(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    if cleaned.is_empty() { "oryxis-key".into() } else { cleaned }
}

/// Write exported private-key material with owner-only permissions on
/// Unix (0600, matching ssh-keygen); Windows relies on the user
/// profile ACLs like every SSH tool there.
fn write_private_key_file(path: &std::path::Path, payload: &str) -> std::io::Result<()> {
    std::fs::write(path, payload)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}
