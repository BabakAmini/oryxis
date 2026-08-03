//! Identity CRUD, split out of `dispatch_keys` (mirroring the vault
//! store's file-per-entity discipline): panel open/close, form
//! fields, save / edit / delete, the per-card context menu, and the
//! keychain ADD split-button menu. Called from `handle_keys`.

#![allow(clippy::result_large_err)]

use iced::Task;

use oryxis_core::models::identity::Identity;

use crate::app::{KeysMessage, Message, Oryxis};
use crate::state::{OverlayContent, OverlayState};

impl Oryxis {
    pub(super) fn handle_keys_identities(
        &mut self,
        message: KeysMessage,
    ) -> Result<Task<Message>, KeysMessage> {
        match message {
            // ── Identities ──
            KeysMessage::ShowIdentityPanel => {
                self.panels.identity_panel = true;
                self.identity_form.label.clear();
                self.identity_form.username.clear();
                // SecretInput::clear also drops the touched flag.
                self.identity_form.password.clear();
                self.identity_form.key = None;
                self.identity_form.password_visible = false;
                self.identity_form.has_existing_password = false;
                self.identity_form.editing_id = None;
                self.panels.keychain_add_menu = false;
                self.identity_context_menu = None;
                self.overlay = None;
            }
            KeysMessage::HideIdentityPanel => {
                self.panels.identity_panel = false;
            }
            KeysMessage::IdentityLabelChanged(v) => {
                self.identity_form.label = v;
            }
            KeysMessage::IdentityUsernameChanged(v) => {
                self.identity_form.username = v;
            }
            KeysMessage::IdentityPasswordChanged(v) => {
                self.identity_form.password.set(v.into_inner());
            }
            KeysMessage::IdentityTogglePasswordVisibility => {
                self.identity_form.password_visible = !self.identity_form.password_visible;
            }
            KeysMessage::IdentityKeyChanged(v) => {
                self.identity_form.key = if v == "(none)" { None } else { Some(v) };
            }
            KeysMessage::SaveIdentity => {
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

                // Tri-state: untouched preserves the stored password,
                // cleared removes it, typed stores (SecretInput::resolve).
                let password = self.identity_form.password.resolve();

                if let Some(vault) = &self.vault {
                    let _ = vault.save_identity(&identity, password);
                    self.load_data_from_vault();
                }
                self.panels.identity_panel = false;
            }
            KeysMessage::EditIdentity(idx) => {
                if let Some(identity) = self.identities.get(idx) {
                    self.identity_form.editing_id = Some(identity.id);
                    self.identity_form.label = identity.label.clone();
                    self.identity_form.username = identity.username.clone().unwrap_or_default();
                    self.identity_form.password.clear();
                    self.identity_form.password_visible = false;
                    self.identity_form.has_existing_password = self.vault.as_ref()
                        .and_then(|v| v.get_identity_password(&identity.id).ok().flatten())
                        .is_some();
                    self.identity_form.key = identity.key_id.and_then(|kid| {
                        self.keys.iter().find(|k| k.id == kid).map(|k| k.label.clone())
                    });
                    self.panels.identity_panel = true;
                    self.identity_context_menu = None;
                    self.overlay = None;
                }
            }
            KeysMessage::RequestDeleteIdentity(idx) => {
                if let Some(identity) = self.identities.get(idx) {
                    let name = identity.label.clone();
                    self.confirm_remove(name, Message::Keys(KeysMessage::DeleteIdentity(idx)));
                }
            }
            KeysMessage::DeleteIdentity(idx) => {
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
            KeysMessage::ShowIdentityMenu(idx) => {
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
            KeysMessage::ToggleKeychainAddMenu => {
                if self.panels.keychain_add_menu {
                    self.panels.keychain_add_menu = false;
                    self.overlay = None;
                } else {
                    self.panels.keychain_add_menu = true;
                    // Opening the ADD menu closes any open editor panel
                    // (import / generate / identity). The menu's entries
                    // just reopen one of those, so a stale panel behind the
                    // menu is never wanted, and leaving it open mis-anchored
                    // the menu on top of the panel (the generate panel was
                    // not even counted in panel_width below).
                    let panel_was_open = self.panels.key_panel
                        || self.panels.key_generate_panel
                        || self.panels.identity_panel;
                    self.panels.key_panel = false;
                    self.panels.key_generate_panel = false;
                    self.panels.identity_panel = false;
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
