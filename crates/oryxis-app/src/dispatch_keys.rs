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
                self.key_import_form.public_key.clear();
                self.key_import_form.certificate.clear();
                self.key_import_public_content = text_editor::Content::new();
                self.key_import_cert_content = text_editor::Content::new();
                self.key_import_form.cert_detected = false;
                self.key_error = None;
                self.key_success = None;
                self.key_import_form.editing_id = None;
                self.key_context_menu = None;
                self.overlay = None;
            }
            Message::ShowKeyPanelPublicFocus => {
                // The ADD menu's "Import public key" entry (B3): the same
                // import panel, opened with the public-key field focused,
                // the security-key delegation flow (paste the sk- line,
                // no private material exists to import).
                let open = self.handle_keys(Message::ShowKeyPanel)?;
                return Ok(Task::batch([
                    open,
                    iced::widget::operation::focus(iced::widget::Id::new(
                        "panel-key-import-public",
                    )),
                ]));
            }
            Message::ShowKeyPanelCertFocus => {
                // The ADD menu's "Certificate" entry (B2.1): the same
                // import panel (a certificate lives on its key, one
                // entity), opened with the certificate field focused so
                // the intent lands somewhere visible.
                let open = self.handle_keys(Message::ShowKeyPanel)?;
                return Ok(Task::batch([
                    open,
                    iced::widget::operation::focus(iced::widget::Id::new(
                        "panel-key-import-cert",
                    )),
                ]));
            }
            Message::HideKeyPanel => {
                self.show_key_panel = false;
                self.key_import_form.editing_id = None;
                self.key_import_form.passphrase.clear();
                self.key_import_form.passphrase_required = false;
                self.key_import_form.passphrase_visible = false;
                self.key_import_form.public_key.clear();
                self.key_import_form.certificate.clear();
                self.key_import_public_content = text_editor::Content::new();
                self.key_import_cert_content = text_editor::Content::new();
                self.key_import_form.cert_detected = false;
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
                        self.key_generate_form.error =
                            Some(crate::i18n::t("key_not_found").into());
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
                                // OpenSSH's implicit lookup: the public
                                // line sits next to the key as `<key>.pub`
                                // and a signed user cert as
                                // `<key>-cert.pub`. Auto-probe and prefill
                                // both when present, and only when they
                                // parse (a stray same-named file must not
                                // poison the field). The public line keeps
                                // the user's trailing comment, which
                                // deriving from the private key would lose.
                                let public = std::fs::read_to_string(
                                    format!("{}.pub", path.display()),
                                )
                                .ok()
                                .filter(|p| {
                                    ssh_key::PublicKey::from_openssh(p.trim()).is_ok()
                                });
                                let cert = std::fs::read_to_string(
                                    format!("{}-cert.pub", path.display()),
                                )
                                .ok()
                                .filter(|c| {
                                    ssh_key::Certificate::from_openssh(c.trim()).is_ok()
                                });
                                Ok((filename, content, public, cert))
                            }
                            None => Err("cancelled".to_string()),
                        }
                    }),
                    |result| match result {
                        Ok(Ok((filename, content, public, cert))) => {
                            Message::KeyFileLoaded(filename, content, public, cert)
                        }
                        Ok(Err(e)) => Message::KeyFileBrowseError(e),
                        Err(e) => Message::KeyFileBrowseError(format!("Thread error: {}", e)),
                    },
                ));
            }
            Message::KeyFileLoaded(filename, content, public, cert) => {
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
                // A sibling `<key>.pub` was found and parses: prefill the
                // editable public line (it carries the user's comment,
                // which deriving from the private key would lose).
                if let Some(public) = public {
                    self.key_import_form.public_key = public.trim().to_string();
                    self.key_import_public_content =
                        text_editor::Content::with_text(public.trim());
                }
                // A sibling `<key>-cert.pub` was found and parses: prefill
                // and flag the "certificate detected" hint.
                if let Some(cert) = cert {
                    self.key_import_form.certificate = cert.trim().to_string();
                    self.key_import_cert_content =
                        text_editor::Content::with_text(cert.trim());
                    self.key_import_form.cert_detected = true;
                }
                self.show_key_panel = true;
                self.key_error = None;
                // The sidebar already shows "Loaded (X bytes)"; surfacing
                // a second toast in the main keychain area is just noise.
                self.key_success = None;
            }
            Message::KeyImportPublicAction(action) => {
                let edited = action.is_edit();
                self.key_import_public_content.perform(action);
                if edited {
                    self.key_import_form.public_key = self.key_import_public_content.text();
                    self.key_error = None;
                }
            }
            Message::KeyImportCertAction(action) => {
                let edited = action.is_edit();
                self.key_import_cert_content.perform(action);
                if edited {
                    self.key_import_form.certificate = self.key_import_cert_content.text();
                    self.key_import_form.cert_detected = false;
                    self.key_error = None;
                }
            }
            Message::BrowseCertFile => {
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(|| {
                        let file = rfd::FileDialog::new()
                            .set_title("Select SSH Certificate")
                            .add_filter("SSH certificate", &["pub"])
                            .pick_file();
                        match file {
                            Some(path) => std::fs::read_to_string(&path)
                                .map_err(|e| format!("Failed to read: {}", e)),
                            None => Err("cancelled".to_string()),
                        }
                    }),
                    |result| match result {
                        Ok(Ok(content)) => Message::CertFileLoaded(content),
                        Ok(Err(e)) => Message::KeyFileBrowseError(e),
                        Err(e) => Message::KeyFileBrowseError(format!("Thread error: {}", e)),
                    },
                ));
            }
            Message::CertFileLoaded(content) => {
                self.key_import_form.certificate = content.trim().to_string();
                self.key_import_cert_content =
                    text_editor::Content::with_text(content.trim());
                // Explicitly picked, not auto-probed: no "detected" hint.
                self.key_import_form.cert_detected = false;
                self.key_error = None;
            }
            Message::ViewKeyCertificate(idx) => {
                if let Some(data) = self.build_cert_viewer(idx) {
                    self.cert_viewer = Some(data);
                }
                self.key_context_menu = None;
                self.overlay = None;
            }
            Message::CloseCertViewer => {
                self.cert_viewer = None;
            }
            Message::RequestRemoveKeyCertificate(idx) => {
                if let Some(key) = self.keys.get(idx) {
                    let name = key.label.clone();
                    self.confirm_remove(name, Message::RemoveKeyCertificate(idx));
                }
                // The confirm dialog renders inside the main view (below the
                // cert-viewer overlay), so close the viewer first, otherwise
                // the confirmation would be hidden behind it.
                self.cert_viewer = None;
                self.key_context_menu = None;
                self.overlay = None;
            }
            Message::RemoveKeyCertificate(idx) => {
                if let Some(key) = self.keys.get(idx) {
                    let mut key = key.clone();
                    key.certificate = None;
                    key.updated_at = chrono::Utc::now();
                    if let Some(vault) = &self.vault {
                        // `None` pem preserves the encrypted private blob and
                        // rewrites the (now empty) certificate column.
                        let _ = vault.save_key(&key, None);
                        self.load_data_from_vault();
                        self.key_success =
                            Some(crate::i18n::t("key_certificate_removed").into());
                    }
                }
                self.cert_viewer = None;
                self.key_context_menu = None;
                self.overlay = None;
            }
            Message::KeyFileBrowseError(err) => {
                if !err.contains("cancelled") {
                    self.key_error = Some(err);
                }
            }
            Message::ImportKey => {
                let pem_empty = self.key_import_form.pem.trim().is_empty();
                if pem_empty && self.key_import_form.public_key.trim().is_empty() {
                    self.key_error =
                        Some(crate::i18n::t("key_select_file_first").into());
                    return Ok(Task::none());
                }
                // Public-only import (B3): no private material at all, the
                // security-key / delegation path. The row persists with an
                // explicit NULL private column; edits of an existing row
                // preserve whatever the column holds (`None`), so clearing
                // the editor buffer can never silently drop a stored
                // private key.
                if pem_empty {
                    let label = if self.key_import_form.label.is_empty() {
                        "imported-key".to_string()
                    } else {
                        self.key_import_form.label.clone()
                    };
                    let input = self.key_import_form.public_key.trim().to_string();
                    if input.contains("-----BEGIN") {
                        self.key_error =
                            Some(crate::i18n::t("public_key_only_error").to_string());
                        return Ok(Task::none());
                    }
                    let mut key = match oryxis_vault::import_public_key(&label, &input) {
                        Ok(key) => key,
                        Err(_) => {
                            self.key_error = Some(
                                crate::i18n::t("public_key_invalid_error").to_string(),
                            );
                            return Ok(Task::none());
                        }
                    };
                    let editing = self.key_import_form.editing_id.is_some();
                    if let Some(existing_id) = self.key_import_form.editing_id {
                        key.id = existing_id;
                        if let Some(existing) =
                            self.keys.iter().find(|k| k.id == existing_id)
                        {
                            key.expose_via_agent = existing.expose_via_agent;
                            key.created_at = existing.created_at;
                        }
                        // Editing an existing row keeps its private column
                        // (`private = None` below). If that column holds a
                        // real private key, the public line the user typed
                        // MUST still certify it: otherwise the row would
                        // pair an old private with a new, mismatched public
                        // and the agent would advertise a blob it signs for
                        // with the wrong key. A public-only row (NULL
                        // private) has nothing to check against, so this
                        // only fires when a private is actually stored.
                        let stored_private = self
                            .vault
                            .as_ref()
                            .and_then(|v| v.get_key_private(&existing_id).ok().flatten());
                        if let Some(pem) = stored_private {
                            match oryxis_vault::import_key("_check", &pem, None) {
                                Ok(generated) => {
                                    match validate_public_key(&input, &generated.key.public_key) {
                                        Ok(_) => {}
                                        Err(err_key) => {
                                            self.key_error =
                                                Some(crate::i18n::t(err_key).to_string());
                                            return Ok(Task::none());
                                        }
                                    }
                                    // Carry the stored key's algorithm so a
                                    // public-line edit can't relabel an RSA
                                    // 2048/3072 row as 4096 (public lines
                                    // don't carry the modulus size).
                                    key.algorithm = generated.key.algorithm;
                                }
                                // Stored private unreadable (corrupt / locked):
                                // fail closed rather than save a possibly
                                // mismatched pair.
                                Err(_) => {
                                    self.key_error = Some(
                                        crate::i18n::t("public_key_mismatch_error")
                                            .to_string(),
                                    );
                                    return Ok(Task::none());
                                }
                            }
                        }
                    }
                    // The certificate field wins over a cert embedded in
                    // the public line (same validation as the private
                    // path); empty keeps whatever the line carried.
                    match validate_certificate(
                        &self.key_import_form.certificate,
                        &key.public_key,
                    ) {
                        Ok(Some(cert)) => key.certificate = Some(cert),
                        Ok(None) => {}
                        Err(err_key) => {
                            self.key_error =
                                Some(crate::i18n::t(err_key).to_string());
                            return Ok(Task::none());
                        }
                    }
                    if let Some(vault) = &self.vault {
                        let private = if editing { None } else { Some("") };
                        match vault.save_key(&key, private) {
                            Ok(()) => {
                                let verb = if editing { "updated" } else { "imported" };
                                self.key_error = None;
                                self.key_success =
                                    Some(format!("Key '{}' {}", label, verb));
                                self.key_import_form = crate::state::KeyImportForm::default();
                                self.key_import_content = text_editor::Content::new();
                                self.key_import_public_content =
                                    text_editor::Content::new();
                                self.key_import_cert_content =
                                    text_editor::Content::new();
                                self.show_key_panel = false;
                                self.load_data_from_vault();
                            }
                            Err(e) => self.key_error = Some(e.to_string()),
                        }
                    }
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
                        // If editing an existing key, preserve the fields
                        // that live outside the import form. `import_key`
                        // rebuilds a fresh `SshKey` (expose_via_agent = true,
                        // created_at = now), so re-saving after an edit would
                        // silently re-arm a key the user had removed from the
                        // ssh-agent and reset its creation date (breaking the
                        // by-date sort). Carry the id and both fields over.
                        if let Some(existing_id) = self.key_import_form.editing_id {
                            generated.key.id = existing_id;
                            if let Some(existing) =
                                self.keys.iter().find(|k| k.id == existing_id)
                            {
                                generated.key.expose_via_agent = existing.expose_via_agent;
                                generated.key.created_at = existing.created_at;
                            }
                        }
                        // Apply the editable public line (B2.1). Empty keeps
                        // the derived one; non-empty must parse and carry the
                        // private key's key data (a different comment is
                        // fine, that is the point of the field).
                        match validate_public_key(
                            &self.key_import_form.public_key,
                            &generated.key.public_key,
                        ) {
                            Ok(Some(public)) => generated.key.public_key = public,
                            Ok(None) => {}
                            Err(key) => {
                                self.key_error = Some(crate::i18n::t(key).to_string());
                                return Ok(Task::none());
                            }
                        }
                        // Validate + attach the certificate. A mismatch or a
                        // host cert is an inline error, never a silent save
                        // (the engine's `check_certificate` is the belt to
                        // this brace, but the editor stops it here first).
                        match validate_certificate(
                            &self.key_import_form.certificate,
                            &generated.key.public_key,
                        ) {
                            Ok(cert) => generated.key.certificate = cert,
                            Err(key) => {
                                self.key_error = Some(crate::i18n::t(key).to_string());
                                return Ok(Task::none());
                            }
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
                                    self.key_import_form.public_key.clear();
                                    self.key_import_form.certificate.clear();
                                    self.key_import_public_content =
                                        text_editor::Content::new();
                                    self.key_import_cert_content =
                                        text_editor::Content::new();
                                    self.key_import_form.cert_detected = false;
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
                    self.key_import_form.public_key = key.public_key.clone();
                    self.key_import_public_content =
                        text_editor::Content::with_text(&key.public_key);
                    self.key_import_form.certificate =
                        key.certificate.clone().unwrap_or_default();
                    self.key_import_cert_content = text_editor::Content::with_text(
                        self.key_import_form.certificate.as_str(),
                    );
                    self.key_import_form.cert_detected = false;
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
                    // Opening the ADD menu closes any open editor panel
                    // (import / generate / identity). The menu's entries
                    // just reopen one of those, so a stale panel behind the
                    // menu is never wanted, and leaving it open mis-anchored
                    // the menu on top of the panel (the generate panel was
                    // not even counted in panel_width below).
                    let panel_was_open = self.show_key_panel
                        || self.show_key_generate_panel
                        || self.show_identity_panel;
                    self.show_key_panel = false;
                    self.show_key_generate_panel = false;
                    self.show_identity_panel = false;
                    // The trigger-bounds cell was drawn with that panel
                    // open, and closing it shifts the whole toolbar by the
                    // panel width before the next draw, so the stale rect
                    // would misplace the menu by exactly that much. The
                    // shift is deterministic (the panel occupies the
                    // trailing edge, leading under RTL), so compensate the
                    // rect instead of falling back to the estimate: the
                    // real y stays exact in every nav layout.
                    if panel_was_open {
                        let b = self.toolbar_split_btn_bounds.get();
                        if b.width > 0.0 {
                            let shift = if crate::i18n::is_rtl_layout() {
                                -crate::app::PANEL_WIDTH
                            } else {
                                crate::app::PANEL_WIDTH
                            };
                            self.toolbar_split_btn_bounds
                                .set(iced::Rectangle { x: b.x + shift, ..b });
                        }
                    }
                    // Anchor below the split button, on its real drawn
                    // bounds (2 px gap, trailing edges aligned), so the
                    // menu follows the button through every layout. No
                    // panel is open now (closed just above), so the
                    // fallback estimate spans the full width.
                    // Sync with `overlay_menu_width` (KeychainAdd = the
                    // 150 default).
                    let menu_width = 150.0;
                    let (x, y) = self.toolbar_menu_anchor(
                        &self.toolbar_split_btn_bounds,
                        menu_width,
                        0.0,
                    );
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::KeychainAdd,
                        x,
                        y,
                    });
                }
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}

/// Validate an attached certificate line against the key it belongs to
/// (B2). Returns the value to store (`None` = detach / no cert,
/// `Some(line)` = the trimmed cert) or an i18n error key. The check
/// mirrors the engine's `check_certificate` (same `public_key()` vs
/// key-data comparison) so the two layers can never disagree on the
/// same cert.
fn validate_certificate(
    cert_input: &str,
    public_key_openssh: &str,
) -> Result<Option<String>, &'static str> {
    let trimmed = cert_input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // A pasted private key block is never a certificate; reject it so
    // secret material can never land in the plaintext cert column.
    if trimmed.contains("-----BEGIN") {
        return Err("cert_mismatch_error");
    }
    let cert = ssh_key::Certificate::from_openssh(trimmed)
        .map_err(|_| "cert_mismatch_error")?;
    // Host certificates authenticate servers, not users; reject.
    if cert.cert_type() == ssh_key::certificate::CertType::Host {
        return Err("cert_wrong_type_error");
    }
    let public = ssh_key::PublicKey::from_openssh(public_key_openssh)
        .map_err(|_| "cert_mismatch_error")?;
    if cert.public_key() != public.key_data() {
        return Err("cert_mismatch_error");
    }
    Ok(Some(trimmed.to_string()))
}

/// Validate the editable public-key line against the public key derived
/// from the private key (B2.1). Returns `Ok(None)` when the field is
/// empty (keep the derived line), `Ok(Some(line))` when the input parses
/// and carries the same key data (a different trailing comment is fine,
/// preserving it is the point of the field), or an i18n error key.
fn validate_public_key(
    public_input: &str,
    derived_openssh: &str,
) -> Result<Option<String>, &'static str> {
    let trimmed = public_input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // A pasted private key block is never a public line; reject it so
    // secret material can never land in the plaintext public column.
    if trimmed.contains("-----BEGIN") {
        return Err("public_key_invalid_error");
    }
    let public = ssh_key::PublicKey::from_openssh(trimmed)
        .map_err(|_| "public_key_invalid_error")?;
    let derived = ssh_key::PublicKey::from_openssh(derived_openssh)
        .map_err(|_| "public_key_invalid_error")?;
    if public.key_data() != derived.key_data() {
        return Err("public_key_mismatch_error");
    }
    Ok(Some(trimmed.to_string()))
}

impl Oryxis {
    /// Parse the certificate attached to key `idx` into a display-ready
    /// [`crate::state::CertViewerData`]. `None` when the key has no cert
    /// or the stored line no longer parses (defensive: it was validated
    /// at import, so this only guards against manual DB tampering).
    pub(crate) fn build_cert_viewer(
        &self,
        idx: usize,
    ) -> Option<crate::state::CertViewerData> {
        let key = self.keys.get(idx)?;
        let cert_line = key.certificate.as_deref()?;
        let cert = ssh_key::Certificate::from_openssh(cert_line.trim()).ok()?;

        // Unix-seconds bounds -> local wall-clock strings. `0` / u64::MAX
        // are OpenSSH's "unbounded" sentinels and render empty.
        let fmt_time = |secs: u64| -> String {
            if secs == 0 || secs == u64::MAX {
                return String::new();
            }
            match chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0) {
                Some(dt) => dt
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
                None => String::new(),
            }
        };
        let now = chrono::Utc::now().timestamp().max(0) as u64;
        let expired = cert.valid_before() != 0
            && cert.valid_before() != u64::MAX
            && now > cert.valid_before();
        let ca_fingerprint = cert
            .signature_key()
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string();

        Some(crate::state::CertViewerData {
            key_idx: idx,
            key_label: key.label.clone(),
            key_id: cert.key_id().to_string(),
            serial: cert.serial(),
            is_host: cert.cert_type() == ssh_key::certificate::CertType::Host,
            principals: cert.valid_principals().to_vec(),
            valid_from: fmt_time(cert.valid_after()),
            valid_until: fmt_time(cert.valid_before()),
            ca_fingerprint,
            expired,
        })
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

#[cfg(test)]
mod cert_validation_tests {
    use super::validate_certificate;
    use rand010 as rand;
    use ssh_key::{certificate, Algorithm, PrivateKey};

    /// A CA-signed certificate of `cert_type` for `user_key`, as its
    /// OpenSSH public line.
    fn make_cert(user_key: &PrivateKey, cert_type: certificate::CertType) -> String {
        let ca = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let mut builder = certificate::Builder::new_with_random_nonce(
            &mut rand::rng(),
            user_key.public_key(),
            0,
            4_000_000_000,
        )
        .unwrap();
        builder.serial(7).unwrap();
        builder.key_id("id").unwrap();
        builder.cert_type(cert_type).unwrap();
        builder.valid_principal("tester").unwrap();
        builder.sign(&ca).unwrap().to_openssh().unwrap()
    }

    fn public_line(key: &PrivateKey) -> String {
        key.public_key().to_openssh().unwrap()
    }

    #[test]
    fn empty_input_detaches() {
        assert_eq!(validate_certificate("   ", "irrelevant"), Ok(None));
    }

    #[test]
    fn private_key_block_is_rejected() {
        // The secret-leak guard: BEGIN-block material must never be
        // accepted into the plaintext certificate column.
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----";
        assert_eq!(validate_certificate(pem, "irrelevant"), Err("cert_mismatch_error"));
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(
            validate_certificate("not a certificate at all", "irrelevant"),
            Err("cert_mismatch_error")
        );
    }

    #[test]
    fn matching_user_cert_is_accepted() {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let cert = make_cert(&key, certificate::CertType::User);
        let stored = validate_certificate(&cert, &public_line(&key)).unwrap();
        assert_eq!(stored.as_deref(), Some(cert.trim()));
    }

    #[test]
    fn cert_for_another_key_is_rejected() {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let other = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let cert = make_cert(&other, certificate::CertType::User);
        assert_eq!(
            validate_certificate(&cert, &public_line(&key)),
            Err("cert_mismatch_error")
        );
    }

    #[test]
    fn host_cert_is_rejected() {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let cert = make_cert(&key, certificate::CertType::Host);
        assert_eq!(
            validate_certificate(&cert, &public_line(&key)),
            Err("cert_wrong_type_error")
        );
    }
}

#[cfg(test)]
mod public_key_validation_tests {
    use super::validate_public_key;
    use rand010 as rand;
    use ssh_key::{Algorithm, PrivateKey};

    fn public_line(key: &PrivateKey) -> String {
        key.public_key().to_openssh().unwrap()
    }

    #[test]
    fn empty_input_keeps_the_derived_line() {
        assert_eq!(validate_public_key("   ", "irrelevant"), Ok(None));
    }

    #[test]
    fn private_key_block_is_rejected() {
        // The secret-leak guard: BEGIN-block material must never be
        // accepted into the plaintext public-key column.
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----";
        assert_eq!(validate_public_key(pem, "irrelevant"), Err("public_key_invalid_error"));
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(
            validate_public_key("not a public key", "irrelevant"),
            Err("public_key_invalid_error")
        );
    }

    #[test]
    fn matching_line_with_custom_comment_is_kept() {
        // Editing the trailing comment is the field's use case (the
        // comparison is on key data, not the string).
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let derived = public_line(&key);
        let blob = derived.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        let edited = format!("{blob} wilson@workstation");
        assert_eq!(
            validate_public_key(&edited, &derived),
            Ok(Some(edited.clone()))
        );
    }

    #[test]
    fn another_keys_public_line_is_rejected() {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let other = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        assert_eq!(
            validate_public_key(&public_line(&other), &public_line(&key)),
            Err("public_key_mismatch_error")
        );
    }
}
