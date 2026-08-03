//! Leaving the vault, by hand or by idle timer.
//!
//! The two paths differ on purpose and are worth reading side by
//! side: `LockVault` is a full teardown (sessions, tabs, secrets),
//! `AutoLockVault` is soft (zeroize the key, show the lock screen,
//! keep the live SSH sessions, since an established channel never
//! needs the key again).

use super::*;

impl Oryxis {
    pub(super) fn handle_vault_lock(
        &mut self,
        message: VaultMessage,
    ) -> Task<Message> {
        match message {

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
                    self.panels.host_panel = false;
                    self.host_panel_error = None;
                    self.editor_form = crate::state::ConnectionForm::default();
                    // The key-generation panel carries export
                    // passphrases and a public-key view; sweep it (a
                    // still-running generation task is dropped on
                    // completion by the locked-vault check).
                    self.panels.key_generate_panel = false;
                    self.keys_ui.generate_form = crate::state::KeyGenerateForm::default();
                    // The key import panel (holds a pasted cert / PEM) and
                    // the cert viewer are vault-area surfaces; drop them.
                    // The live PEM editor buffer is private material, so it
                    // is reset too, matching the generate-panel sweep.
                    self.panels.key_panel = false;
                    self.keys_ui.import_form = crate::state::KeyImportForm::default();
                    self.keys_ui.import_content = iced::widget::text_editor::Content::new();
                    self.keys_ui.import_public_content = iced::widget::text_editor::Content::new();
                    self.keys_ui.import_cert_content = iced::widget::text_editor::Content::new();
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
                    // sweep the state so none reappears after unlock. A watch
                    // holding a pending save is dropped with it, matching the
                    // soft-lock promise (secret-bearing UI is discarded, the
                    // live session survives).
                    self.sftp.picker_open = false;
                    self.sftp.new_entry = None;
                    self.sftp.delete_confirm.clear();
                    self.sftp_edit_reopen = None;
                    self.sftp.edit_watches.clear();
                    // Watches parked in standalone / hybrid tab states feed
                    // the same 2s tick; left alive they would keep uploading
                    // local saves (under an autosave grant) behind the lock
                    // screen, and dirty ones would re-prompt after unlock.
                    for tab in self.sftp_tabs.iter_mut() {
                                                tab.state.edit_watches.clear();
                    }
                    for tab in self.tabs.iter_mut() {
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
                        self.panels.host_panel = false;
                        self.host_panel_error = None;
                        self.editor_form = crate::state::ConnectionForm::default();
                        self.panels.key_generate_panel = false;
                        self.keys_ui.generate_form = crate::state::KeyGenerateForm::default();
                        self.panels.key_panel = false;
                        self.keys_ui.import_form = crate::state::KeyImportForm::default();
                        self.keys_ui.import_content = iced::widget::text_editor::Content::new();
                        self.keys_ui.import_public_content = iced::widget::text_editor::Content::new();
                        self.keys_ui.import_cert_content = iced::widget::text_editor::Content::new();
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
                        self.sftp_edit_reopen = None;
                        self.sftp.edit_watches.clear();
                        for tab in self.sftp_tabs.iter_mut() {
                                                        tab.state.edit_watches.clear();
                        }
                        for tab in self.tabs.iter_mut() {
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
            // The parent routed us here, so anything else is a
            // grouping mistake, not a runtime case.
            m => return crate::dispatch::unrouted(m),
        }
        Task::none()
    }
}
