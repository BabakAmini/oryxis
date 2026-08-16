//! Host editor auto-save: edits to an EXISTING host persist on their
//! own, debounced, so the drawer carries no Save button for them (the
//! footer states it instead). New hosts keep the explicit Save /
//! Connect pair: a half-typed host must never enter the vault by
//! itself.
//!
//! Mechanics: every editor-domain message re-arms a debounce
//! (`editor_autosave_kick`, called from the `Message::Editor` dispatch
//! in `dispatch.rs`); the tick persists through the same
//! `persist_editor_form` the Save button uses, then re-baselines.
//! Dirtiness is a SIGNATURE comparison (the built `Connection` plus
//! the row-adjacent fields), never a per-arm flag: a flag would have
//! to be remembered in every one of the ~100 field arms, and the first
//! forgotten one would silently stop saving that field. Secrets stay
//! out of the signature (their buffers must not be serialized); their
//! tri-state `touched()` flags are the dirty signal instead.

use super::*;

impl Oryxis {
    /// What the form would persist right now, as a comparable string.
    /// `updated_at` is zeroed (it is stamped per build and would read
    /// as a permanent diff); the group is compared by the TYPED path
    /// (the build runs with `persist_group: false` so a mere check
    /// never materializes group rows); `use_totp` rides along because
    /// its effect (clearing the stored secret) lives in a side column
    /// the `Connection` JSON does not carry. `None` = the form does
    /// not build (half-typed state), which is never dirty on its own.
    fn editor_form_signature(&mut self) -> Option<String> {
        let mut conn = self.connection_from_editor_form(super::GroupWrite::Skip).ok()?;
        conn.updated_at = chrono::DateTime::<chrono::Utc>::MIN_UTC;
        let json = serde_json::to_string(&conn).ok()?;
        Some(format!(
            "{json}|{}|{}",
            self.editor_form.group_name.trim(),
            self.editor_form.use_totp
        ))
    }

    fn editor_secrets_touched(&self) -> bool {
        let f = &self.editor_form;
        f.password.touched()
            || f.proxy_password.touched()
            || f.totp_secret.touched()
            || f.target_password.touched()
    }

    /// Auto-save only ever applies to an existing host with the panel
    /// up. Quick-flow hosts carry no `editing_id`, so they are outside
    /// by construction (their explicit Save is the persist opt-in).
    fn editor_autosave_armed(&self) -> bool {
        self.panels.host_panel && self.editor_form.editing_id.is_some()
    }

    /// Whether the open editor holds changes the vault does not.
    pub(crate) fn editor_autosave_dirty(&mut self) -> bool {
        if !self.editor_autosave_armed() {
            return false;
        }
        if self.editor_secrets_touched() {
            return true;
        }
        match (self.editor_form_signature(), &self.editor_saved_snapshot) {
            (Some(sig), Some(snap)) => sig != *snap,
            // No baseline yet (the first post-open message records it
            // in `editor_autosave_kick`) or an unbuildable form:
            // nothing to persist.
            _ => false,
        }
    }

    /// Post-dispatch hook (`dispatch.rs`): after any editor-domain
    /// message, record the baseline on the first message following an
    /// open, and (re)arm the debounce when the form has drifted from
    /// it. Over-calling is safe: a clean form arms nothing.
    pub(crate) fn editor_autosave_kick(&mut self) -> Task<Message> {
        if !self.editor_autosave_armed() {
            return Task::none();
        }
        if self.editor_saved_snapshot.is_none() {
            // The open arm cleared the snapshot; this very message is
            // the first one after it (usually the open itself), so the
            // form still equals the stored row: record, don't save.
            self.editor_saved_snapshot = self.editor_form_signature();
            return Task::none();
        }
        if !self.editor_autosave_dirty() {
            return Task::none();
        }
        self.editor_autosave_gen += 1;
        let armed = self.editor_autosave_gen;
        Task::perform(
            async {
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            },
            move |_| Message::Editor(EditorMessage::EditorAutoSaveTick(armed)),
        )
    }

    /// Whether the typed Parent Group value names a group that does
    /// not exist yet. The ticks persist with `GroupWrite::ResolveOnly`
    /// (a half-typed name must not mint vault rows), so a completed
    /// NEW name is work only the closing flush can do; without this
    /// the ticks re-baseline the signature and the flush would judge
    /// the form clean and never create it.
    fn editor_group_pending_creation(&self) -> bool {
        if !self.editor_autosave_armed() {
            return false;
        }
        let name = self.editor_form.group_name.trim();
        !name.is_empty()
            && oryxis_core::models::Group::resolve_path_or_label(
                &self.groups,
                name,
                &std::collections::HashSet::new(),
            )
            .is_none()
    }

    /// Persist a still-debouncing change NOW. Sits on every path that
    /// closes or replaces the editor under an existing host (the X /
    /// Esc cancel, opening another host or panel, the vault locks, the
    /// window close), so the debounce window can never swallow the
    /// last edit. Closing is the commit point for a typed NEW group
    /// name (`GroupWrite::Create`; the ticks only resolve existing
    /// ones). Silent on an invalid form: the vault keeps the last
    /// valid save, which is the only coherent answer for a surface
    /// that is going away. A failed WRITE is not silent: the surface
    /// is going away, so the loss is announced through a toast, the
    /// one channel that survives the close.
    pub(crate) fn editor_flush_pending(&mut self) {
        if !self.editor_autosave_dirty() && !self.editor_group_pending_creation() {
            return;
        }
        // Invalidate any in-flight tick; this flush is its work.
        self.editor_autosave_gen += 1;
        match self.persist_editor_form(super::GroupWrite::Create) {
            Ok(_) => self.editor_autosave_settle(),
            Err(super::PersistError::Invalid(_)) => {}
            Err(super::PersistError::Vault(e)) => {
                tracing::warn!("host editor flush failed: {e}");
                self.set_toast(format!("{}: {e}", crate::i18n::t("editor_autosave_failed")));
            }
        }
    }

    /// Post-persist bookkeeping shared by the tick and the flush:
    /// re-baseline the signature and return every touched secret
    /// buffer to the untouched "preserve the stored value" state (the
    /// value it holds IS the stored value now), syncing the
    /// has-a-stored-secret placeholders the views read.
    fn editor_autosave_settle(&mut self) {
        self.editor_saved_snapshot = self.editor_form_signature();
        let f = &mut self.editor_form;
        if f.password.touched() {
            f.has_existing_password = !f.password.as_str().is_empty();
            let v = f.password.as_str().to_string();
            f.password.prefill(v);
        }
        if f.proxy_password.touched() {
            f.has_existing_proxy_password = !f.proxy_password.as_str().is_empty();
            let v = f.proxy_password.as_str().to_string();
            f.proxy_password.prefill(v);
        }
        if f.totp_secret.touched() {
            f.has_existing_totp = f.use_totp && !f.totp_secret.as_str().trim().is_empty();
            let v = f.totp_secret.as_str().to_string();
            f.totp_secret.prefill(v);
        }
        if f.target_password.touched() {
            f.has_existing_target_password = !f.target_password.as_str().is_empty();
            let v = f.target_password.as_str().to_string();
            f.target_password.prefill(v);
        }
    }

    pub(super) fn handle_editor_autosave(&mut self, message: EditorMessage) -> Task<Message> {
        match message {
            EditorMessage::EditorAutoSaveTick(tick_gen) => {
                // A newer edit re-armed the debounce (or a flush /
                // close already did the work): stale, drop it.
                if tick_gen != self.editor_autosave_gen || !self.editor_autosave_dirty() {
                    return Task::none();
                }
                // `ResolveOnly`: a debounce tick can land on a
                // half-typed Parent Group value, which must not mint
                // vault groups ("Pro" while typing "Production") or
                // reparent the host mid-keystroke. The closing flush
                // is the commit point for a new name.
                match self.persist_editor_form(super::GroupWrite::ResolveOnly) {
                    Ok(_) => {
                        self.editor_autosave_settle();
                        // The inline error only ever holds a stale save
                        // failure here; this save superseded it.
                        self.host_panel_error = None;
                        self.editor_autosave_saved_visible = true;
                        self.editor_autosave_flash_gen += 1;
                        let fgen = self.editor_autosave_flash_gen;
                        return Task::perform(
                            async {
                                tokio::time::sleep(std::time::Duration::from_millis(1600))
                                    .await;
                            },
                            move |_| {
                                Message::Editor(EditorMessage::EditorAutoSaveFlashClear(fgen))
                            },
                        );
                    }
                    // Mid-edit invalid states (a cleared hostname, a
                    // half-typed port) skip silently: the vault keeps
                    // the last valid save and the next tick retries.
                    Err(super::PersistError::Invalid(_)) => {}
                    // A failed WRITE means edits the user believes
                    // saved did not: surface it inline while the
                    // panel is still up to show it.
                    Err(super::PersistError::Vault(e)) => {
                        tracing::warn!("host editor auto-save failed: {e}");
                        self.host_panel_error = Some(e);
                    }
                }
            }
            EditorMessage::EditorAutoSaveFlashClear(flash_gen) => {
                if flash_gen == self.editor_autosave_flash_gen {
                    self.editor_autosave_saved_visible = false;
                }
            }
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
