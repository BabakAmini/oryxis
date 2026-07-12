//! Helpers for the global keyboard shortcuts wired in
//! `dispatch_terminal.rs`. Kept in its own module so the dispatcher
//! files stay focused on message routing.

use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget;
use iced::Task;

use crate::app::{Message, Oryxis};
use crate::hotkeys::{FamilyMatch, HotkeyAction};
use crate::state::View;

impl Oryxis {
    /// Resolves slot N (0-indexed) of the visual tab strip to the
    /// `Message` that activates that slot, mirroring the order
    /// `views/tab_bar.rs` renders. Returns `None` when the slot is
    /// out of range so Ctrl+5 on a window with two tabs is a no-op
    /// instead of bouncing focus around.
    pub(crate) fn strip_slot_target(&self, slot: usize) -> Option<Message> {
        // Slot 0 is the Home (Hosts) area tab; the rest follow the unified
        // strip order (terminal + SFTP tabs, pinned-first), exactly as
        // `views/tab_bar.rs` renders it, so Ctrl+N lands on the Nth visible
        // chip. SFTP is a tab now, not a fixed Ctrl+2 area tab.
        let mut slots: Vec<Message> = vec![Message::ChangeView(View::Dashboard)];
        slots.extend(self.ordered_tab_refs().iter().filter_map(|r| self.tab_ref_select_msg(r)));
        slots.into_iter().nth(slot)
    }

    /// The unified left-to-right strip order (pinned tabs first), terminal
    /// and SFTP tabs interleaved exactly as `views/tab_bar.rs` renders them.
    /// Shared by Ctrl+N slot resolution and Alt+arrow cycling so both honour
    /// the visible order instead of a storage-vec index (which skips SFTP
    /// tabs and ignores pinning).
    pub(crate) fn ordered_tab_refs(&self) -> Vec<crate::state::TabRef> {
        use crate::state::TabRef;
        let pinned_of = |r: &TabRef| -> bool {
            match r {
                TabRef::Terminal(id) => {
                    self.tabs.iter().find(|t| t._id == *id).map(|t| t.pinned).unwrap_or(false)
                }
                TabRef::Sftp(id) => {
                    self.sftp_tabs.iter().find(|t| t.id == *id).map(|t| t.pinned).unwrap_or(false)
                }
            }
        };
        let mut refs: Vec<TabRef> =
            self.tab_order.iter().copied().filter(|r| pinned_of(r)).collect();
        refs.extend(self.tab_order.iter().copied().filter(|r| !pinned_of(r)));
        refs
    }

    /// The `Message` that activates a given strip tab, or `None` when it can't
    /// be activated (an SFTP tab while SFTP is disabled, or a dangling id).
    pub(crate) fn tab_ref_select_msg(&self, r: &crate::state::TabRef) -> Option<Message> {
        use crate::state::TabRef;
        match r {
            TabRef::Terminal(id) => {
                self.tabs.iter().position(|t| t._id == *id).map(Message::SelectTab)
            }
            TabRef::Sftp(id) => {
                if !self.sftp_enabled {
                    return None;
                }
                self.sftp_tabs.iter().position(|t| t.id == *id).map(Message::SelectSftpTab)
            }
        }
    }

    /// The currently focused tab as a `TabRef`. Mirrors the strip's own
    /// active model (`views/tab_bar.rs`): `active_sftp` is NOT cleared when a
    /// terminal tab is selected, so an SFTP tab counts as active only on the
    /// SFTP surface (`active_tab` empty and the SFTP view showing). Otherwise
    /// the selected terminal tab wins. Checking `active_sftp` first here was
    /// the bug that made Alt+arrow / Ctrl+Tab jump from a stale SFTP slot.
    pub(crate) fn active_tab_ref(&self) -> Option<crate::state::TabRef> {
        use crate::state::TabRef;
        if self.active_tab.is_none()
            && self.active_view == View::Sftp
            && let Some(i) = self.active_sftp
        {
            return self.sftp_tabs.get(i).map(|t| TabRef::Sftp(t.id));
        }
        if let Some(i) = self.active_tab {
            return self.tabs.get(i).map(|t| TabRef::Terminal(t._id));
        }
        None
    }

    /// Resolves the active tab to its position in `self.connections`,
    /// or `None` when no tab is active, the tab is a local shell, or
    /// the saved host has since been deleted. Used by Ctrl+P to open
    /// the host editor for the current connection.
    pub(crate) fn active_tab_connection_idx(&self) -> Option<usize> {
        let tab_idx = self.active_tab?;
        let tab = self.tabs.get(tab_idx)?;
        let base_label = tab.label.trim_end_matches(" (disconnected)");
        self.connections.iter().position(|c| c.label == base_label)
    }

    /// Returns the `widget::Id` of the search/filter input for the
    /// current view, or `None` when the view has no search field.
    /// Consumed by `Message::FocusViewSearch` (Ctrl+F).
    pub(crate) fn active_view_search_id(&self) -> Option<widget::Id> {
        match self.active_view {
            View::Dashboard => Some(widget::Id::new("search-dashboard")),
            View::Keys => Some(widget::Id::new("search-keys")),
            // Snippets and History only expose their search field on
            // the Workspace-mode sub-nav. In Classic mode there's no
            // search input to focus, so Ctrl+F harmlessly tries to
            // focus an Id that doesn't exist (iced no-ops on a miss).
            View::Snippets => Some(widget::Id::new("search-snippets")),
            View::PortForwarding => Some(widget::Id::new("search-port-forwards")),
            View::History => Some(widget::Id::new("search-history")),
            View::Sftp => {
                // Two filter inputs (local + remote panes); focus
                // the remote one since that's the side that costs an
                // SSH round-trip and is where a typed filter matters
                // most.
                Some(widget::Id::new("search-sftp-remote"))
            }
            View::Cloud => Some(widget::Id::new("search-cloud")),
            View::Proxies => Some(widget::Id::new("search-proxies")),
            // A hybrid tab in Files mode is the SFTP surface: Ctrl+F
            // focuses the remote filter, parity with View::Sftp above.
            View::Terminal => self
                .sftp_surface_visible()
                .then(|| widget::Id::new("search-sftp-remote")),
            View::Settings | View::KnownHosts => None,
        }
    }

    /// Whether the given blocking modal is currently open. The `show_*`
    /// flag / `Option<_>` data field on `Oryxis` is the source of truth;
    /// this exhaustive `match` is what makes `any_modal_blocks_input`
    /// compiler-complete (a new `Modal` variant cannot compile without an
    /// arm here).
    pub(crate) fn is_modal_open(&self, m: crate::state::Modal) -> bool {
        use crate::state::Modal;
        match m {
            Modal::NewTabPicker => self.show_new_tab_picker,
            Modal::TabJump => self.show_tab_jump,
            Modal::IconPicker => self.show_icon_picker,
            Modal::ThemePicker => self.show_theme_picker,
            Modal::ChainEditor => self.show_chain_editor,
            Modal::SessionGroupPanel => self.show_session_group_panel,
            Modal::FolderRename => self.folder_rename.is_some(),
            Modal::FolderDelete => self.folder_delete.is_some(),
            Modal::TabRename => self.tab_rename.is_some(),
            Modal::CarefulPaste => self.pending_paste.is_some(),
            Modal::SnippetVars => self.pending_snippet_vars.is_some(),
            Modal::KbiPrompt => self.pending_kbi_prompt.is_some(),
            // Mirrors KbiPrompt: the flag alone is the source of truth. The
            // inline connect-progress host-key path (which renders inside
            // the progress screen and has no focused PTY behind it) is
            // gated separately by `connecting.is_none()` at the render site.
            Modal::HostKey => self.pending_host_key.is_some(),
            Modal::AgentConfirm => self.agent.pending_confirm.is_some(),
            Modal::ThemeEditor => self.theme_editor.is_some(),
            Modal::ThemeImport => self.show_theme_import,
            Modal::UiThemeEditor => self.ui_theme_editor.is_some(),
            Modal::ShareDialog => self.show_share_dialog,
            Modal::CloudImportConfirm => self.cloud_import_confirm_visible,
            Modal::ErrorDialog => self.error_dialog.is_some(),
            Modal::ClearHistoryConfirm => self.clear_history_confirm,
            Modal::SshImport => self.show_ssh_import_dialog,
            Modal::SftpRename => self.sftp.rename.is_some(),
            Modal::SftpNewEntry => self.sftp.new_entry.is_some(),
            Modal::SftpProperties => self.sftp.properties.is_some(),
            Modal::SftpOverwrite => self.sftp.overwrite_prompt.is_some(),
            Modal::SftpPicker => self.sftp.picker_open,
            Modal::CertificateViewer => self.cert_viewer.is_some(),
        }
    }

    /// Close a specific modal: clear its `show_*` flag / `Option<_>` field
    /// plus any companion state, mirroring each modal's own Cancel handler
    /// so Esc leaves nothing stale. The exhaustive `match` is what makes
    /// `close_topmost_modal` compiler-complete. (The chain editor's
    /// two-stage Esc is handled by the caller before this is reached.)
    pub(crate) fn close_modal(&mut self, m: crate::state::Modal) {
        use crate::state::Modal;
        match m {
            Modal::NewTabPicker => {
                // Mirror HideNewTabPicker: abandoning the picker also
                // abandons any pending split-fill intent, so a later
                // unrelated open can't inherit it.
                self.show_new_tab_picker = false;
                self.pending_pane_split = None;
                self.new_tab_picker_group = None;
            }
            Modal::TabJump => self.show_tab_jump = false,
            Modal::IconPicker => {
                self.show_icon_picker = false;
                self.icon_picker.for_id = None;
            }
            Modal::ThemePicker => self.show_theme_picker = false,
            Modal::ChainEditor => self.show_chain_editor = false,
            Modal::SessionGroupPanel => {
                self.show_session_group_panel = false;
                self.session_group_panel_error = None;
            }
            Modal::FolderRename => self.folder_rename = None,
            Modal::FolderDelete => self.folder_delete = None,
            Modal::TabRename => self.tab_rename = None,
            Modal::CarefulPaste => self.pending_paste = None,
            Modal::SnippetVars => self.pending_snippet_vars = None,
            // Full mirror of SshKbiCancel: the engine must receive the
            // cancel (`None`) or the in-flight auth stays parked forever.
            Modal::KbiPrompt => {
                self.pending_kbi_prompt = None;
                self.pending_kbi_quick = None;
                self.kbi_inputs.clear();
                if let Some(ref tx) = self.kbi_response_tx {
                    let _ = tx.try_send(None);
                }
            }
            // Esc rejects the host key: a security prompt's safe default is
            // never to accept an unknown / changed key. Full mirror of
            // SshHostKeyReject: the engine's verifier must receive `false`
            // or the in-flight connect stays parked forever.
            Modal::HostKey => {
                self.pending_host_key = None;
                if let Some(tx) = self.active_host_key_tx.take() {
                    let _ = tx.try_send(false);
                }
            }
            // Esc denies the signature (safe default), firing the
            // responder so the waiting sign task gets its answer. The
            // caller then promotes any queued prompt via
            // `advance_confirm_queue`.
            Modal::AgentConfirm => {
                if let Some(card) = self.agent.pending_confirm.take() {
                    card.respond(false);
                }
            }
            Modal::ThemeEditor => {
                self.theme_editor = None;
                self.theme_color_popover = None;
            }
            Modal::ThemeImport => self.show_theme_import = false,
            Modal::UiThemeEditor => {
                self.ui_theme_editor = None;
                self.ui_color_popover = None;
            }
            Modal::ShareDialog => {
                self.show_share_dialog = false;
                self.share.filter = None;
                self.share.status = None;
                self.share.suggested_name = None;
            }
            Modal::CloudImportConfirm => {
                self.cloud_import_confirm_visible = false;
                self.cloud_discover_default_group_picker_open = false;
            }
            // Esc on the error dialog is always Dismiss, never the
            // dialog's action (mirrors ErrorDialogDismiss).
            Modal::ErrorDialog => self.error_dialog = None,
            // Mirrors CancelClearHistory.
            Modal::ClearHistoryConfirm => self.clear_history_confirm = false,
            // Mirrors SshImportDismiss, companion state included.
            Modal::SshImport => {
                self.show_ssh_import_dialog = false;
                self.ssh_import_hosts.clear();
                self.ssh_import_selected.clear();
                self.ssh_import_existing.clear();
            }
            Modal::SftpRename => self.sftp.rename = None,
            Modal::SftpNewEntry => self.sftp.new_entry = None,
            Modal::SftpProperties => self.sftp.properties = None,
            Modal::SftpOverwrite => self.sftp.overwrite_prompt = None,
            Modal::SftpPicker => self.sftp.picker_open = false,
            Modal::CertificateViewer => self.cert_viewer = None,
        }
    }

    /// `true` when a global picker / modal overlay is open and should
    /// swallow keyboard input instead of letting it fall through to the
    /// PTY underneath. Mirrors the set checked by `close_topmost_modal`
    /// (minus the burger menu, which carries no text field). Used by the
    /// keyboard router in `dispatch_terminal.rs` so typing in a picker's
    /// search field doesn't also leak into the terminal behind it.
    /// True when a blocking modal owns the keyboard, so the global key
    /// subscription must NOT route the press to the active PTY.
    ///
    /// INVARIANT: every modal that contains a text field MUST appear here.
    /// The terminal input arrives via a global subscription
    /// (`subscription.rs`) that bypasses the widget tree, so a modal's own
    /// focused `text_input` does not stop the same press from also reaching
    /// the PTY, only this predicate does. Every modal here MUST also be a
    /// full-window overlay (so a set flag always means a visible, input-
    /// owning modal) and SHOULD appear in `close_topmost_modal` so Esc
    /// dismisses it. The SFTP modals now layer at the app root via
    /// `layer_sftp_modals`, so they satisfy that invariant too.
    pub(crate) fn any_modal_blocks_input(&self) -> bool {
        // Exhaustive over every modal via `is_modal_open` (compiler-checked
        // match) + `Modal::ALL`: a new modal variant can't be added without
        // an `is_modal_open` arm, so it can never silently leak keystrokes
        // into the PTY behind it. The keyboard-interactive (2FA / OTP)
        // prompt is included here (its text fields own the keyboard); the
        // inline connect-progress path is gated separately by
        // `connecting.is_none()`.
        crate::state::Modal::ALL
            .iter()
            .any(|&m| m.blocks_input() && self.is_modal_open(m))
    }

    /// Closes the topmost open modal / overlay if any, and returns
    /// `true` when something was closed. Lets the Esc handler in
    /// `dispatch_terminal.rs` decide whether to also forward the
    /// byte to the active PTY (it doesn't, when this returns true).
    /// Priority follows visual stacking order: pickers on top of
    /// the chrome are checked before background panels.
    pub(crate) fn close_topmost_modal(&mut self) -> bool {
        // Open dropdown / popover overlay (sort menu, kebab menus, the
        // floating toolbar search + overflow). Esc dismisses it first,
        // matching the click-outside backdrop. Lightweight, so it takes
        // priority over the heavier modals below.
        if self.overlay.is_some() {
            self.overlay = None;
            return true;
        }
        // Walk the Esc-close priority order and dismiss the first open
        // modal. `close_modal` is a compiler-checked exhaustive match, so a
        // new modal can't be added without deciding its cleanup; adding it
        // to `ESC_ORDER` then makes Esc dismiss it.
        for &m in crate::state::Modal::ESC_ORDER {
            if self.is_modal_open(m) {
                // The chain editor's Esc is two-stage: in "add a hop" mode
                // the first Esc pops back to the chain list, only a second
                // closes the whole editor.
                if m == crate::state::Modal::ChainEditor && self.chain_editor_adding {
                    self.chain_editor_adding = false;
                    self.chain_editor_search.clear();
                    return true;
                }
                // Same two-stage rule for the new-tab picker drilled
                // into a group: first Esc backs out to the top level
                // (mirrors the Back header), second Esc closes.
                if m == crate::state::Modal::NewTabPicker
                    && self.new_tab_picker_group.is_some()
                {
                    self.new_tab_picker_group = None;
                    self.new_tab_picker_search.clear();
                    return true;
                }
                self.close_modal(m);
                return true;
            }
        }
        // Burger menu last; it's a dropdown rather than a modal but
        // Esc still feels right for it.
        if self.show_burger_menu {
            self.show_burger_menu = false;
            return true;
        }
        false
    }

    /// Spawns a fresh top-level Oryxis process. When `source_tab`
    /// names a tab bound to a saved connection, passes
    /// `--connect <uuid>` so the new window auto-opens it. When the
    /// caller already has a master password unlocked, also passes
    /// `--inherit-vault` and pipes the password through stdin so the
    /// secret never appears in argv (which `ps aux` would expose).
    pub(crate) fn spawn_oryxis_child(&self, source_tab: Option<usize>) {
        // Map the tab back to a saved connection so the child opens the
        // same host. SSM-into-EC2 tabs carry a title prefix; strip it so
        // the lookup matches (the child re-routes to SSM via cloud_ref).
        // ECS Exec / kubectl tabs are ephemeral dynamic-group resources
        // with no saved connection, so they resolve to None and the child
        // opens a plain window (a fresh process can't carry an in-memory
        // relaunch message across the boundary).
        let connect_uuid = source_tab.and_then(|idx| {
            self.tabs.get(idx).and_then(|tab| {
                let base_label = tab
                    .label
                    .trim_end_matches(" (disconnected)")
                    .trim_start_matches(crate::app::SSM_TAB_PREFIX)
                    .to_string();
                self.connections
                    .iter()
                    .find(|c| c.label == base_label)
                    .map(|c| c.id)
            })
        });
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("current_exe unavailable: {}", e);
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        if let Some(uuid) = connect_uuid {
            cmd.arg("--connect").arg(uuid.to_string());
        }
        let inherit = self.master_password.is_some();
        if inherit {
            cmd.arg("--inherit-vault");
            cmd.stdin(std::process::Stdio::piped());
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if inherit
                    && let Some(mut stdin) = child.stdin.take()
                    && let Some(pw) = self.master_password.as_ref()
                {
                    use std::io::Write as _;
                    let _ = writeln!(stdin, "{}", pw);
                    // Closing the pipe signals EOF to the child.
                    drop(stdin);
                }
            }
            Err(e) => tracing::error!("Failed to spawn new window: {}", e),
        }
    }

    /// Relaunch the app in place: spawn a fresh process that inherits
    /// the unlocked vault, then exit the current one. Used to apply a
    /// setting that is only read at process start (the graphics
    /// renderer). The child carries `--relaunch` so it waits for this
    /// process's single-instance mutex to release and comes back as
    /// primary. Live SSH sessions and tabs do not survive a process
    /// restart, the caller warns the user before invoking this.
    ///
    /// Never returns on success (`process::exit`). On a spawn failure it
    /// returns so the caller stays running rather than stranding the user
    /// with no window.
    pub(crate) fn relaunch_self(&self) {
        // The replacement process should come back with today's window
        // geometry, and this one exits without passing through the
        // normal close path.
        self.persist_window_geometry();
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("relaunch: current_exe unavailable: {e}");
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--relaunch");
        let inherit = self.master_password.is_some();
        if inherit {
            cmd.arg("--inherit-vault");
            cmd.stdin(std::process::Stdio::piped());
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if inherit
                    && let Some(mut stdin) = child.stdin.take()
                    && let Some(pw) = self.master_password.as_ref()
                {
                    use std::io::Write as _;
                    let _ = writeln!(stdin, "{}", pw);
                    drop(stdin);
                }
                // Hand off cleanly: the child is up, drop this process so
                // the mutex releases and the child promotes to primary.
                std::process::exit(0);
            }
            Err(e) => tracing::error!("relaunch: spawn failed: {e}"),
        }
    }

    /// Pretty-printed binding for the given action (`"Ctrl + K"`),
    /// or `None` when the action has no binding (conflict-unbound).
    /// Used by the burger menu / context menus to surface the
    /// current shortcut next to its action.
    pub(crate) fn hotkey_label_for_action(
        &self,
        action: HotkeyAction,
    ) -> Option<String> {
        let binding = self.hotkey_bindings.get(&action)?;
        Some(binding.badges().join(" + "))
    }

    /// Pretty-printed binding for the Nth strip slot (0-indexed),
    /// e.g. `"Ctrl + 1"` for slot 0 when `SwitchToTabSlot` is
    /// bound to Ctrl + digit. Returns `None` when the family is
    /// unbound. Used by the burger menu to show the per-area
    /// shortcut next to Hosts / SFTP.
    pub(crate) fn hotkey_label_for_strip_slot(
        &self,
        slot: usize,
    ) -> Option<String> {
        let binding = self.hotkey_bindings.get(&HotkeyAction::SwitchToTabSlot)?;
        let mut parts = binding.badges();
        // Drop the family suffix ("1...9") and append the concrete
        // slot digit so the hint reads like a real chord.
        parts.pop();
        parts.push((slot + 1).to_string());
        Some(parts.join(" + "))
    }

    /// Pretty-printed binding for the Nth vault section (1-indexed
    /// digit), e.g. `"Ctrl + Shift + 2"` for Keychain. Same concrete
    /// digit treatment as the strip-slot label; `None` when the
    /// `VaultSectionSlot` family is unbound. Used by the burger
    /// menu's VAULT entries.
    pub(crate) fn hotkey_label_for_vault_slot(
        &self,
        digit: usize,
    ) -> Option<String> {
        let binding = self.hotkey_bindings.get(&HotkeyAction::VaultSectionSlot)?;
        let mut parts = binding.badges();
        parts.pop();
        parts.push(digit.to_string());
        Some(parts.join(" + "))
    }

    /// Main entry point for `dispatch_terminal::Message::KeyboardEvent`.
    /// Returns `Some(task)` when the event was consumed (by capture
    /// mode, a binding match, or the Esc-closes-modal fallback), or
    /// `None` to let the caller fall through to PTY routing.
    pub(crate) fn handle_hotkey_keypress(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
    ) -> Option<Task<Message>> {
        // 1. Capture mode for the Settings → Shortcuts editor wins
        //    over everything: Esc cancels, anything else (modulo
        //    pure-modifier presses) becomes the new binding. Belt
        //    and suspenders: capture only fires when the user is
        //    still on the Shortcuts editor, navigating away cancels
        //    the pending capture so the next keystroke doesn't
        //    silently rebind something on another screen.
        if self.editing_hotkey.is_some() {
            let on_shortcuts_editor = self.active_view == View::Settings
                && self.settings_section == crate::state::SettingsSection::Shortcuts;
            if !on_shortcuts_editor {
                self.editing_hotkey = None;
            } else if let Some(task) = self.handle_hotkey_capture(key, modifiers) {
                return Some(task);
            }
        }

        // 1.5. Snippet-shortcut recorder (armed from either snippet
        //      editor). The next chord becomes the snippet's custom
        //      run hotkey; Esc cancels. Guarded on the editor being
        //      open so a stale flag can't eat keys elsewhere.
        if self.snippet_hotkey_capturing {
            if !self.show_snippet_panel {
                self.snippet_hotkey_capturing = false;
            } else {
                if matches!(key, Key::Named(Named::Escape)) {
                    self.snippet_hotkey_capturing = false;
                    return Some(Task::none());
                }
                if matches!(
                    key,
                    Key::Named(
                        Named::Control | Named::Shift | Named::Alt | Named::Super | Named::Meta
                    )
                ) {
                    // Mid-chord modifier press; keep waiting.
                    return Some(Task::none());
                }
                let Some(binding) = crate::hotkeys::binding_from_event(key, modifiers, true)
                else {
                    self.set_toast(crate::i18n::t("hotkey_must_have_modifier").to_string());
                    return Some(toast_clear_after_secs(2));
                };
                // Plain Ctrl+letter belongs to the shell; a snippet
                // hotkey only ever fires inside a terminal, so binding
                // one would shadow readline/SIGINT keys.
                if binding.is_terminal_control_sequence() {
                    self.set_toast(crate::i18n::t("snippet_hotkey_reserved").to_string());
                    return Some(toast_clear_after_secs(2));
                }
                // Conflicts: the static table and other snippets.
                let in_table = self.hotkey_bindings.values().any(|b| *b == binding);
                let in_snippets = self.snippets.iter().any(|sn| {
                    self.snippet_editing_id != Some(sn.id)
                        && sn.hotkey.as_deref()
                            == Some(binding.serialize()).as_deref()
                });
                if in_table || in_snippets {
                    self.set_toast(crate::i18n::t("snippet_hotkey_in_use").to_string());
                    return Some(toast_clear_after_secs(2));
                }
                self.snippet_hotkey = Some(binding);
                self.snippet_hotkey_capturing = false;
                return Some(Task::none());
            }
        }

        // 2. Binding-table dispatch. First match wins. When the
        //    terminal view is focused, any binding shaped like a
        //    shell control sequence (Ctrl+letter with no other
        //    modifier) is skipped so Ctrl+L/Ctrl+P/Ctrl+K/etc. reach
        //    the PTY. The gate is computed from the CURRENT binding,
        //    so a user who rebinds CloseActiveTab onto a shell key
        //    loses the rebound action in the terminal (but it still
        //    fires elsewhere), and rebinding an old gated action OFF
        //    a shell key restores it everywhere. Iterates over the
        //    'static slice directly; HotkeyBinding is Copy, so we
        //    materialise it before calling dispatch_hotkey_action
        //    (which takes &mut self) and avoid the per-press
        //    allocation that the prior `.to_vec()` paid.
        //
        //    "In a terminal" means A TERMINAL TAB IS FOCUSED, not
        //    `active_view == Terminal`: in workspace mode a focused
        //    terminal runs under the Dashboard view (the PTY key
        //    routing in dispatch_terminal.rs already goes by
        //    `active_tab` for the same reason). Field bug 2026-07-03:
        //    every terminal_only hotkey (FocusSidebarList, splits,
        //    pane focus) was dead on tabs opened under the workspace,
        //    while the same chord worked on a View::Terminal tab of
        //    the same build. `active_tab` is cleared on every
        //    navigation into the vault / settings / SFTP surfaces, so
        //    it is exactly the "keys route to a PTY" signal.
        let in_terminal = self.active_view == View::Terminal || self.active_tab.is_some();
        // Whether the PTY actually owns plain control sequences right
        // now: a hybrid tab in Files mode hides the terminal and gates
        // its byte routing off, so Ctrl+letter bindings (Ctrl+F search)
        // may fire there, exactly like on the standalone SFTP view. The
        // `terminal_only` skip keeps using `in_terminal` so the toggle
        // hotkey itself still works from Files mode.
        let pty_owns_keys = in_terminal
            && !self
                .active_tab
                .and_then(|i| self.tabs.get(i))
                .is_some_and(|t| t.files_mode);
        // A blocking modal owns the keyboard: only Esc may pass (step 3
        // below closes the modal). Skip binding-table and snippet dispatch
        // so chords like ClosePane / SplitPane / the host-editor hotkey
        // cannot fire on the surface hidden behind the modal. The modal's
        // own keyboard navigation runs earlier in the router
        // (`handle_modal_nav_key`), so movement / activation keys are
        // unaffected.
        let modal_owns_keys = self.any_modal_blocks_input();
        for &action in HotkeyAction::all() {
            if modal_owns_keys {
                break;
            }
            // Split-pane actions only apply inside the terminal view.
            // Skipping (not consuming) elsewhere leaves their key free
            // in other views and avoids a confusing no-op.
            if action.terminal_only() && !in_terminal {
                continue;
            }
            // Vault section cycling only applies in the vault area.
            // Skipping (not consuming) leaves Ctrl+PageUp/Down to the
            // PTY inside a terminal tab, where TUIs use it.
            if action.vault_only() && !self.in_vault_area() {
                continue;
            }
            let bind_copy = self.hotkey_bindings.get(&action).copied();
            if pty_owns_keys
                && bind_copy.is_some_and(|b| b.is_terminal_control_sequence())
            {
                continue;
            }
            let Some(b) = bind_copy else { continue };
            if let Some(family) = b.match_event(key, modifiers) {
                tracing::debug!(action = action.id(), "hotkey matched");
                return Some(self.dispatch_hotkey_action(action, family));
            }
        }

        // 2.5. Per-snippet custom hotkeys, derived LIVE from the vault
        //      list (no side registry: deleting a snippet deletes its
        //      shortcut by construction). Terminal-focused only, since
        //      the action types into the focused session; a hybrid tab
        //      in Files mode gates PTY writes off, so firing here would
        //      just dead-end (worst case through the vars modal).
        if !modal_owns_keys && pty_owns_keys && !self.show_snippet_panel {
            let hit = self.snippets.iter().position(|sn| {
                sn.hotkey
                    .as_deref()
                    .and_then(crate::hotkeys::HotkeyBinding::parse)
                    .is_some_and(|b| b.match_event(key, modifiers).is_some())
            });
            if let Some(idx) = hit {
                return Some(self.update(Message::RunSnippet(idx)));
            }
        }

        // 3. Esc closes the topmost open modal as a fallback. Only
        //    fires when nothing else above claimed it, so terminal
        //    apps that rely on raw Esc (vim, less) keep getting the
        //    byte when no modal is open.
        if matches!(key, Key::Named(Named::Escape)) && self.close_topmost_modal() {
            // Closing an agent-confirm prompt promotes the next queued
            // one (no-op for every other modal).
            return Some(self.advance_confirm_queue());
        }

        None
    }

    /// Capture-mode branch of `handle_hotkey_keypress`. Esc cancels;
    /// pure-modifier presses are ignored (they fire `KeyPressed` too);
    /// anything else becomes the new binding (validated by
    /// `binding_from_event::is_safe`). Conflicts with another action
    /// unbind the loser and surface a toast naming it.
    fn handle_hotkey_capture(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
    ) -> Option<Task<Message>> {
        let action = self.editing_hotkey?;
        // Esc cancels without saving.
        if matches!(key, Key::Named(Named::Escape)) {
            self.editing_hotkey = None;
            return Some(Task::none());
        }
        // Pure-modifier KeyPressed (Ctrl alone, Shift alone, ...)
        // shouldn't terminate the capture: the user is mid-way to
        // pressing the full chord.
        if matches!(
            key,
            Key::Named(
                Named::Control
                    | Named::Shift
                    | Named::Alt
                    | Named::Super
                    | Named::Meta
            )
        ) {
            return Some(Task::none());
        }

        let primary_editable = action.primary_editable();
        let captured = crate::hotkeys::binding_from_event(key, modifiers, primary_editable);
        let Some(mut new_binding) = captured else {
            // Plain letter without modifier → reject with toast,
            // leave editing_hotkey set so the user can try again.
            self.set_toast(crate::i18n::t("hotkey_must_have_modifier").to_string());
            return Some(toast_clear_after_secs(2));
        };
        // For family actions we only edit modifiers; preserve the
        // existing primary so the suffix glyph (1...9 / arrows) stays.
        if !primary_editable
            && let Some(existing) = self.hotkey_bindings.get(&action)
        {
            new_binding.primary = existing.primary;
        }

        // Conflict resolution: if another action already owns this
        // exact binding, unbind that action and surface a toast that
        // names *the action* (not the key combo) so the family case
        // reads "Switch to specific Tab is now unbound" instead of
        // "Ctrl+1 is now unbound".
        let conflict: Option<HotkeyAction> = self
            .hotkey_bindings
            .iter()
            .find(|(a, b)| **a != action && **b == new_binding)
            .map(|(a, _)| *a);
        let conflict_toast: Option<Task<Message>> = conflict.map(|other| {
            // Auto-rebind the conflicting action to its factory default
            // when that default doesn't itself collide with the new
            // binding (or with any other live binding). Beats leaving
            // the user with an orphaned action they have to discover
            // and re-set themselves. Falls back to unbinding when the
            // default would be a fresh conflict.
            let defaults = crate::hotkeys::default_bindings();
            let default_for_other = defaults.get(&other).copied();
            let default_safe = default_for_other.is_some_and(|d| {
                d != new_binding
                    && !self.hotkey_bindings.iter().any(|(a, b)| {
                        *a != action && *a != other && *b == d
                    })
            });
            if let Some(d) = default_for_other.filter(|_| default_safe) {
                self.hotkey_bindings.insert(other, d);
                self.persist_setting(
                    &format!("hotkey_{}", other.id()),
                    &d.serialize(),
                );
                self.set_toast(
                    crate::i18n::t("hotkey_conflict_rebound_default")
                        .replace("{action}", crate::i18n::t(other.label_key())),
                );
            } else {
                self.hotkey_bindings.remove(&other);
                self.persist_setting(&format!("hotkey_{}", other.id()), "");
                self.set_toast(
                    crate::i18n::t("hotkey_conflict_unbound")
                        .replace("{action}", crate::i18n::t(other.label_key())),
                );
            }
            toast_clear_after_secs(3)
        });

        self.hotkey_bindings.insert(action, new_binding);
        self.persist_setting(
            &format!("hotkey_{}", action.id()),
            &new_binding.serialize(),
        );
        self.editing_hotkey = None;

        Some(conflict_toast.unwrap_or_else(Task::none))
    }

    /// Translates a matched `(HotkeyAction, FamilyMatch)` into the
    /// concrete `Task<Message>` to dispatch. Returns `Task::none()`
    /// for matched-but-no-op cases (Ctrl+Shift+W with no active tab,
    /// Ctrl+P with no saved-host tab, Alt+arrow with no tabs open).
    /// The action is still considered consumed, so the key doesn't
    /// leak into PTY routing.
    fn dispatch_hotkey_action(
        &mut self,
        action: HotkeyAction,
        family: FamilyMatch,
    ) -> Task<Message> {
        use HotkeyAction::*;
        match action {
            // Route through the message so the new-tab intent is reset the
            // same way the `+` button does: Ctrl+K always opens a fresh
            // new-tab picker, never inherits a `pending_pane_split` left
            // armed by an earlier split-picker that was dismissed with Esc
            // (which would otherwise fill the old tab's split instead of
            // opening a new tab).
            ShowNewTabPicker => Task::done(Message::ShowNewTabPicker),
            ShowTabJump => {
                self.show_tab_jump = true;
                self.tab_jump_search.clear();
                Task::none()
            }
            OpenLocalShell => Task::done(Message::OpenLocalShell),
            NewWindow => Task::done(Message::SpawnNewWindow),
            // Entity creation: the editor panels only render in their
            // owning vault section, so land there first (ShowKeyPanel
            // already navigates itself).
            NewHost => {
                self.active_view = View::Dashboard;
                self.active_tab = None;
                self.update(Message::ShowNewConnection)
            }
            NewKey => self.update(Message::ShowKeyPanel),
            NewIdentity => {
                self.active_view = View::Keys;
                self.active_tab = None;
                self.update(Message::ShowIdentityPanel)
            }
            CloseActiveTab => {
                // With a terminal tab focused (View::Terminal or the
                // workspace) this closes the focused split pane;
                // ClosePane already falls back to closing the whole
                // tab when it's the last pane. Elsewhere there are no
                // panes, so close the active tab directly.
                if self.active_view == View::Terminal || self.active_tab.is_some() {
                    Task::done(Message::ClosePane)
                } else if let Some(idx) = self.active_tab {
                    Task::done(Message::CloseTab(idx))
                } else {
                    Task::none()
                }
            }
            OpenPortForwards => {
                if let Some(idx) = self.active_tab_connection_idx() {
                    Task::done(Message::EditConnection(idx))
                } else if let Some(qid) = self.active_tab.and_then(|i| {
                    self.tabs.get(i).and_then(|t| match &t.active().origin {
                        crate::state::PaneOrigin::QuickHost(qid) => Some(*qid),
                        _ => None,
                    })
                }) {
                    // Ad-hoc tab: "edit host" becomes the save-to-vault
                    // prefill (there is no saved row to edit in place).
                    Task::done(Message::SaveQuickHost(qid))
                } else {
                    Task::none()
                }
            }
            OpenSettings => Task::done(Message::ChangeView(View::Settings)),
            FocusViewSearch => Task::done(Message::FocusViewSearch),
            OpenSftp => {
                if self.sftp_enabled {
                    Task::done(Message::NewSftpTab)
                } else {
                    Task::none()
                }
            }
            SwitchToTabSlot => match family {
                FamilyMatch::Digit(d) => {
                    Task::done(Message::ActivateStripSlot(d as usize - 1))
                }
                _ => Task::none(),
            },
            // Ctrl+Shift+digit: jump straight to a vault section, in
            // the burger menu's VAULT order. Works from anywhere
            // (ChangeView handles leaving a terminal tab); digit 9 is
            // spare, and the Logs slot respects its visibility gate
            // like the menu entry does.
            VaultSectionSlot => match family {
                FamilyMatch::Digit(d) => {
                    let view = match d {
                        1 => Some(View::Dashboard),
                        2 => Some(View::Keys),
                        3 => Some(View::Snippets),
                        4 => Some(View::PortForwarding),
                        5 => self.logs_surface_visible().then_some(View::History),
                        6 => Some(View::Cloud),
                        7 => Some(View::Proxies),
                        8 => Some(View::KnownHosts),
                        _ => None,
                    };
                    match view {
                        Some(v) => Task::done(Message::ChangeView(v)),
                        None => Task::none(),
                    }
                }
                _ => Task::none(),
            },
            CycleTabs => {
                // Walk the unified visual strip (terminal + SFTP, pinned-first)
                // so Alt+arrows step through every chip the user sees, in the
                // order they see it, instead of a raw `self.tabs` index that
                // skipped SFTP tabs and ignored pinning.
                let refs: Vec<crate::state::TabRef> = self
                    .ordered_tab_refs()
                    .into_iter()
                    .filter(|r| self.tab_ref_select_msg(r).is_some())
                    .collect();
                let n = refs.len();
                if n == 0 {
                    return Task::none();
                }
                let cur_pos = self
                    .active_tab_ref()
                    .and_then(|cr| refs.iter().position(|r| *r == cr))
                    .unwrap_or(0);
                let next_pos = match family {
                    FamilyMatch::ArrowRight => (cur_pos + 1) % n,
                    FamilyMatch::ArrowLeft => (cur_pos + n - 1) % n,
                    _ => return Task::none(),
                };
                match self.tab_ref_select_msg(&refs[next_pos]) {
                    Some(msg) => Task::done(msg),
                    None => Task::none(),
                }
            }
            ToggleFullscreen => Task::done(Message::WindowFullscreenToggle),
            FontZoomIn => {
                self.terminal_font_size = (self.terminal_font_size + 1.0).min(24.0);
                self.persist_setting(
                    "terminal_font_size",
                    &format!("{}", self.terminal_font_size),
                );
                Task::none()
            }
            FontZoomOut => {
                self.terminal_font_size = (self.terminal_font_size - 1.0).max(10.0);
                self.persist_setting(
                    "terminal_font_size",
                    &format!("{}", self.terminal_font_size),
                );
                Task::none()
            }
            FontZoomReset => {
                self.terminal_font_size = 14.0;
                self.persist_setting("terminal_font_size", "14");
                Task::none()
            }
            // Terminal split panes. The loop only reaches these in the
            // terminal view (terminal_only gate), so no view check here.
            SplitPaneVertical => {
                Task::done(Message::SplitPane(iced::widget::pane_grid::Axis::Vertical))
            }
            SplitPaneHorizontal => {
                Task::done(Message::SplitPane(iced::widget::pane_grid::Axis::Horizontal))
            }
            FocusPaneLeft => {
                Task::done(Message::FocusPaneDir(iced::widget::pane_grid::Direction::Left))
            }
            FocusPaneRight => {
                Task::done(Message::FocusPaneDir(iced::widget::pane_grid::Direction::Right))
            }
            FocusPaneUp => {
                Task::done(Message::FocusPaneDir(iced::widget::pane_grid::Direction::Up))
            }
            FocusPaneDown => {
                Task::done(Message::FocusPaneDir(iced::widget::pane_grid::Direction::Down))
            }
            // Ring the sidebar lists (Snippets / History); repeat
            // presses cycle the two tabs. Terminal-only like the
            // split-pane family above.
            FocusSidebarList => self.focus_sidebar_list(),
            // Open/close the focused tab's sidebar (owner ask: a
            // keyboard path to close it; the handler already drops
            // the ring + dropdown gate on close).
            ToggleSidebar => Task::done(Message::ToggleChatSidebar),
            // Hybrid tab: Terminal <-> Files for the focused tab (the
            // handler no-ops for tabs without a live SSH session).
            ToggleTabFiles => match self.active_tab {
                Some(idx) => Task::done(Message::ToggleTabFilesMode(idx)),
                None => Task::none(),
            },
            // Vault section cycling: neighbor of the active view in the
            // sub-nav pill order, wrapping. The loop only reaches these
            // in the vault area (vault_only gate above).
            VaultSectionPrev | VaultSectionNext => {
                let sections: Vec<View> =
                    self.subnav_pill_defs().iter().map(|(_, v)| *v).collect();
                let forward = matches!(action, VaultSectionNext);
                let Some(next) = crate::keynav::movement::linear_move(
                    &sections,
                    Some(self.active_view),
                    forward,
                ) else {
                    return Task::none();
                };
                // Keep an active SubNav pill highlight through the
                // switch so arrows / Enter keep working from it.
                if matches!(
                    self.keynav.focus,
                    Some((crate::keynav::FocusZone::SubNav, _))
                ) {
                    self.keynav.focus = Some((
                        crate::keynav::FocusZone::SubNav,
                        crate::keynav::NavItem::SubNav(next),
                    ));
                    self.keynav.keep_focus_through_change_view = true;
                }
                Task::done(Message::ChangeView(next))
            }
        }
    }
}

/// Helper used by the capture branch: dispatch a `Message::ToastClear`
/// after `secs` seconds. Same shape as the existing `CopyToClipboard`
/// toast flow.
fn toast_clear_after_secs(secs: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        },
        |_| Message::ToastClear,
    )
}
