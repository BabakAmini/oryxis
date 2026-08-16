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

    /// Whether the typed Parent Group value still has to be applied.
    /// The ticks persist with `GroupWrite::Keep` (a mid-word value is
    /// not an answer about the group), so ANY group change is work
    /// only the concluding flush can do; without this the ticks
    /// re-baseline the signature and the flush would judge the form
    /// clean and never apply it.
    fn editor_group_pending(&self) -> bool {
        if !self.editor_autosave_armed() {
            return false;
        }
        let stored = self
            .editor_form
            .editing_id
            .and_then(|id| self.connections.iter().find(|c| c.id == id))
            .and_then(|c| c.group_id)
            .map(|gid| oryxis_core::models::Group::path_of(&self.groups, gid))
            .unwrap_or_default();
        self.editor_form.group_name.trim() != stored.trim()
    }

    /// Persist a still-debouncing change NOW, on a path the USER
    /// concluded: the X / Esc cancel, opening another host, swapping
    /// the panel. That gesture is what makes the typed Parent Group
    /// value an answer, so this is the commit point for it
    /// (`GroupWrite::Create`; the ticks never touch the group).
    pub(crate) fn editor_flush_pending(&mut self) {
        self.editor_flush_with(super::GroupWrite::Create);
    }

    /// Persist a still-debouncing change NOW on a path NOTHING
    /// concluded: the vault locks under an idle user, the window
    /// closes. The edits are theirs and are kept, but the Parent Group
    /// value stays whatever the host already had: an interrupted
    /// "Staging" must not mint a permanent, synced group named "Sta".
    pub(crate) fn editor_flush_interrupted(&mut self) {
        self.editor_flush_with(super::GroupWrite::Keep);
    }

    /// Shared body. Silent on an invalid form: the vault keeps the last
    /// valid save, which is the only coherent answer for a surface
    /// that is going away. A failed WRITE is not silent: it raises the
    /// inline panel error (still on screen on the gesture paths) AND a
    /// toast, and is always logged, because the surfaces that outlive
    /// neither are exactly where the loss would otherwise be silent.
    fn editor_flush_with(&mut self, groups: super::GroupWrite) {
        let group_pending = groups == super::GroupWrite::Create && self.editor_group_pending();
        if !self.editor_autosave_dirty() && !group_pending {
            return;
        }
        // Invalidate any in-flight tick; this flush is its work.
        self.editor_autosave_gen += 1;
        match self.persist_editor_form(groups) {
            Ok(_) => self.editor_autosave_settle(),
            Err(super::PersistError::Invalid(_)) => {}
            Err(super::PersistError::Vault(e)) => {
                tracing::warn!("host editor flush failed: {e}");
                self.host_panel_error = Some(e.clone());
                self.set_toast(format!("{}: {e}", crate::i18n::t("editor_autosave_failed")));
            }
        }
    }

    /// Post-persist bookkeeping shared by the tick and the flush:
    /// re-baseline the signature and return every touched secret
    /// buffer to the untouched "preserve the stored value" state (the
    /// value it holds IS the stored value now), syncing the
    /// has-a-stored-secret placeholders the views read.
    ///
    /// The three side-column flags are NOT recomputed here: the persist
    /// that just ran is the only thing that knows whether it stored the
    /// buffer or performed a DERIVED CLEAR (proxy disabled, TOTP off,
    /// script detached), and it wrote each flag to match what actually
    /// landed. Recomputing them from the buffer alone contradicted that
    /// and disabled the rescue restore: typing a proxy password and
    /// misclicking the proxy off inside one debounce window cleared the
    /// column, then set has_existing back to true, so re-enabling wrote
    /// nothing while the field still showed the secret.
    fn editor_autosave_settle(&mut self) {
        self.editor_saved_snapshot = self.editor_form_signature();
        let f = &mut self.editor_form;
        // The main password has no derived clear (no toggle governs it),
        // so the buffer IS the authority for its flag.
        if f.password.touched() {
            f.has_existing_password = !f.password.as_str().is_empty();
            let v = f.password.as_str().to_string();
            f.password.prefill(v);
        }
        for buffer in [
            &mut f.proxy_password,
            &mut f.totp_secret,
            &mut f.target_password,
        ] {
            if buffer.touched() {
                let v = buffer.as_str().to_string();
                buffer.prefill(v);
            }
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
                // `Keep`: a debounce tick can land mid-word, and a
                // mid-word Parent Group value is not an answer, it
                // must neither mint vault groups ("Sta" out of
                // "Staging") nor reparent onto a group whose label
                // the prefix happens to match ("Prod" while typing
                // "Production"). The concluding flush applies it.
                match self.persist_editor_form(super::GroupWrite::Keep) {
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
