//! `Oryxis::handle_snippets`: settings-panel-independent dispatch arms for the
//! snippets area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{Message, Oryxis};

impl Oryxis {
    /// Lowercased tags of the focused pane's saved host; `None` when
    /// the focused pane isn't a saved host or the host has no tags.
    /// Drives the snippet sidebar's filter-by-host-tags toggle.
    pub(crate) fn focused_host_tags_lower(&self) -> Option<Vec<String>> {
        let tab = self.active_tab.and_then(|i| self.tabs.get(i))?;
        let tags: Vec<String> = match &tab.active().origin {
            crate::state::PaneOrigin::Host(id) => self
                .connections
                .iter()
                .find(|c| c.id == *id)?
                .tags
                .clone(),
            // Local panes resolve through the curated terminal list by
            // command identity (program + args), the same key the
            // re-scan dedup uses; the pane itself only carries the
            // spawn spec.
            crate::state::PaneOrigin::Local(spec) => {
                let key = {
                    let mut k = spec.program.clone();
                    for a in &spec.args {
                        k.push('\u{1f}');
                        k.push_str(a);
                    }
                    k
                };
                self.local_terminals
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .find(|e| e.cmd_key() == key)?
                    .tags
                    .clone()
            }
            _ => return None,
        };
        if tags.is_empty() {
            return None;
        }
        Some(tags.iter().map(|t| t.to_lowercase()).collect())
    }

    /// Distinct snippet group names in display order (first spelling
    /// wins, sorted case-insensitively). Shared by the vault view's
    /// folder cards and the keyboard router so `NavItem::SnippetGroup`
    /// indices resolve to the same group either way.
    pub(crate) fn snippet_group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for snip in &self.snippets {
            if let Some(g) = &snip.group
                && !names.iter().any(|x| x.eq_ignore_ascii_case(g))
            {
                names.push(g.clone());
            }
        }
        names.sort_by_key(|s| s.to_lowercase());
        names
    }

    /// Distinct snippet tags, for the vault tag-filter dropdown.
    pub(crate) fn distinct_snippet_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = Vec::new();
        for snip in &self.snippets {
            for tg in &snip.tags {
                if !tags.iter().any(|x| x.eq_ignore_ascii_case(tg)) {
                    tags.push(tg.clone());
                }
            }
        }
        tags.sort_by_key(|t| t.to_lowercase());
        tags
    }

    /// Send `cmd` to the active terminal: bracketed-paste wrapped, the
    /// submit newline OUTSIDE the bracket when `run`. Ring-preserving
    /// (may be fired by the sidebar ring's own Enter).
    fn inject_snippet_text(&mut self, cmd: &str, run: bool) {
        let Some(tab_idx) = self.snippet_injection_tab() else {
            return;
        };
        let Some(tab) = self.tabs.get(tab_idx) else {
            return;
        };
        let bracketed = tab
            .active()
            .terminal
            .lock()
            .map(|s| s.bracketed_paste_enabled())
            .unwrap_or(false);
        let mut payload = oryxis_terminal::wrap_paste(cmd, bracketed);
        if run {
            payload.push(b'\n');
        }
        self.write_ring_injection_to_tab(tab_idx, &payload);
    }

    /// Run/paste gate: a snippet with `{name}` placeholders parks in
    /// the variables modal instead of sending raw braces; everything
    /// else sends immediately. Returns the first input's focus task
    /// when the modal opens.
    fn snippet_send_or_prompt(&mut self, idx: usize, run: bool) -> Task<Message> {
        let Some(snip) = self.snippets.get(idx) else {
            return Task::none();
        };
        let cmd = snip.command.clone();
        let vars = crate::util::snippet_placeholders(&cmd);
        if vars.is_empty() {
            self.inject_snippet_text(&cmd, run);
            return Task::none();
        }
        self.pending_snippet_vars = Some(crate::state::PendingSnippetVars {
            command: cmd,
            run,
            vars,
        });
        iced::widget::operation::focus(iced::widget::Id::new("snippet-var-0"))
    }

    /// Rebuild the Group combo options from the distinct snippet
    /// groups (first spelling wins, sorted), mirroring the host
    /// editor's `editor_parent_combo` reset. Called when either
    /// snippet editor opens.
    fn reset_snippet_group_combo(&mut self) {
        let mut labels: Vec<String> = Vec::new();
        for snip in &self.snippets {
            if let Some(g) = &snip.group
                && !labels.iter().any(|x| x.eq_ignore_ascii_case(g))
            {
                labels.push(g.clone());
            }
        }
        labels.sort_by_key(|s| s.to_lowercase());
        let selection = (!self.snippet_group.is_empty()).then(|| self.snippet_group.clone());
        self.snippet_group_combo =
            iced::widget::combo_box::State::with_selection(labels, selection.as_ref());
    }

    pub(crate) fn handle_snippets(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // -- Local shell --
            // -- Snippets --
            Message::ShowSnippetPanel => {
                self.overlay = None;
                self.show_snippet_panel = true;
                self.snippet_label.clear();
                self.snippet_command = iced::widget::text_editor::Content::new();
                self.snippet_group.clear();
                self.snippet_tags_input.clear();
                self.snippet_editing_id = None;
                self.snippet_error = None;
                self.reset_snippet_group_combo();
            }
            Message::HideSnippetPanel => {
                self.show_snippet_panel = false;
            }
            Message::SnippetLabelChanged(v) => self.snippet_label = v,
            Message::SnippetGroupChanged(v) => self.snippet_group = v,
            Message::SnippetTagsChanged(v) => self.snippet_tags_input = v,
            Message::SnippetCommandAction(action) => self.snippet_command.perform(action),
            Message::ToggleSnippetTagFilter => {
                self.setting_snippet_tag_filter = !self.setting_snippet_tag_filter;
                self.persist_setting(
                    "snippet_tag_filter",
                    if self.setting_snippet_tag_filter { "true" } else { "false" },
                );
            }
            Message::ShowSnippetTagFilterMenu => {
                use crate::state::{OverlayContent, OverlayState};
                let already = matches!(
                    self.overlay.as_ref().map(|o| &o.content),
                    Some(OverlayContent::SnippetTagFilter)
                );
                if already {
                    self.overlay = None;
                } else {
                    let anchor = self.keynav_take_menu_anchor();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::SnippetTagFilter,
                        x: anchor.0,
                        y: anchor.1,
                    });
                }
            }
            Message::ToggleSnippetTagFilterTag(tag) => {
                // Multi-select, dropdown stays open (backdrop closes).
                match self
                    .snippet_filter_tags
                    .iter()
                    .position(|t| t.eq_ignore_ascii_case(&tag))
                {
                    Some(i) => {
                        self.snippet_filter_tags.remove(i);
                    }
                    None => self.snippet_filter_tags.push(tag),
                }
                self.keynav.focus = None;
            }
            Message::ClearSnippetTagFilter => {
                self.snippet_filter_tags.clear();
                self.overlay = None;
                self.keynav.focus = None;
            }
            Message::OpenSnippetGroup(name) => {
                self.active_snippet_group = Some(name);
                self.snippet_search.clear();
                self.keynav.focus = None;
            }
            Message::CloseSnippetGroup => {
                self.active_snippet_group = None;
                self.keynav.focus = None;
            }
            Message::OpenSidebarSnippetGroup(name) => {
                self.sidebar_snippet_group = Some(name);
                // The row list is about to change shape; land the ring
                // back at the top on the next engage.
                self.keynav.sidebar_selected = None;
            }
            Message::CloseSidebarSnippetGroup => {
                self.sidebar_snippet_group = None;
                self.keynav.sidebar_selected = None;
            }
            Message::ShowSnippetMenu(idx) => {
                use crate::state::{OverlayContent, OverlayState};
                // Toggle: clicking the kebab again (or on the same card)
                // dismisses the popup, mirroring the host-card menu.
                if self.snippet_context_menu == Some(idx) {
                    self.snippet_context_menu = None;
                    self.overlay = None;
                } else {
                    self.snippet_context_menu = Some(idx);
                    let anchor = self.keynav_take_menu_anchor();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::SnippetActions(idx),
                        x: anchor.0,
                        y: anchor.1,
                    });
                }
            }
            Message::EditSnippet(idx) => {
                // Reached from the card kebab menu, close the popup.
                self.snippet_context_menu = None;
                self.overlay = None;
                if let Some(snip) = self.snippets.get(idx) {
                    self.show_snippet_panel = true;
                    self.snippet_label = snip.label.clone();
                    self.snippet_command =
                        iced::widget::text_editor::Content::with_text(&snip.command);
                    self.snippet_group = snip.group.clone().unwrap_or_default();
                    self.snippet_tags_input = snip.tags.join(", ");
                    self.snippet_editing_id = Some(snip.id);
                    self.snippet_error = None;
                    self.reset_snippet_group_combo();
                }
            }
            Message::SaveSnippet => {
                if self.snippet_label.is_empty() || self.snippet_command.text().trim().is_empty() {
                    self.snippet_error = Some("Label and command are required".into());
                    return Ok(Task::none());
                }
                let mut snip = if let Some(id) = self.snippet_editing_id {
                    self.snippets.iter().find(|s| s.id == id).cloned()
                        .unwrap_or_else(|| oryxis_core::models::snippet::Snippet::new("", ""))
                } else {
                    oryxis_core::models::snippet::Snippet::new("", "")
                };
                snip.label = self.snippet_label.clone();
                snip.command = self.snippet_command.text().trim_end().to_string();
                snip.group = {
                    let g = self.snippet_group.trim();
                    (!g.is_empty()).then(|| g.to_string())
                };
                snip.tags = crate::util::parse_tags(&self.snippet_tags_input);
                if let Some(vault) = &self.vault {
                    match vault.save_snippet(&snip) {
                        Ok(()) => {
                            self.show_snippet_panel = false;
                            self.snippet_error = None;
                            self.load_data_from_vault();
                        }
                        Err(e) => self.snippet_error = Some(e.to_string()),
                    }
                }
            }
            Message::RequestDeleteSnippet(idx) => {
                if let Some(snip) = self.snippets.get(idx) {
                    let name = snip.label.clone();
                    self.confirm_remove(name, Message::DeleteSnippet(idx));
                }
            }
            Message::DeleteSnippet(idx) => {
                // Reached from the card kebab menu or the edit panel,
                // close the popup either way.
                self.snippet_context_menu = None;
                self.overlay = None;
                if let Some(snip) = self.snippets.get(idx) {
                    let id = snip.id;
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_snippet(&id);
                        self.show_snippet_panel = false;
                        self.load_data_from_vault();
                    }
                }
            }
            Message::RunSnippet(idx) => {
                return Ok(self.snippet_send_or_prompt(idx, true));
            }
            Message::ApplySudoPassword => {
                // Resolve the active terminal's connection by label, decrypt
                // its stored password, and type it + Enter. The password is
                // never logged (only PTY output is recorded, and sudo turns
                // echo off) nor shown in the toast.
                let toast_key = (|| {
                    let tab_idx = self.snippet_injection_tab()?;
                    let label = self.tabs.get(tab_idx)?.label.clone();
                    let conn_id = self
                        .connections
                        .iter()
                        .find(|c| c.label == label)
                        .map(|c| c.id)?;
                    let pw = self
                        .vault
                        .as_ref()
                        .and_then(|v| v.get_connection_password(&conn_id).ok().flatten())
                        .filter(|p| !p.is_empty())?;
                    let data = format!("{pw}\n");
                    if let Some(tab) = self.tabs.get(tab_idx) {
                        if let Some(ref ssh) = tab.active().ssh_session {
                            let _ = ssh.write(data.as_bytes());
                        } else if let Ok(mut state) = tab.active().terminal.lock() {
                            state.write(data.as_bytes());
                        }
                    }
                    Some("sudo_password_sent")
                })()
                .unwrap_or("no_stored_password");
                self.toast = Some(crate::i18n::t(toast_key).to_string());
                return Ok(Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                    },
                    |_| Message::ToastClear,
                ));
            }
            Message::PasteSnippet(idx) => {
                // Same injection path as RunSnippet, but without the
                // trailing newline so the user reviews and presses
                // Enter themselves.
                return Ok(self.snippet_send_or_prompt(idx, false));
            }
            Message::SnippetVarChanged(i, v) => {
                if let Some(pending) = self.pending_snippet_vars.as_mut()
                    && let Some(slot) = pending.vars.get_mut(i)
                {
                    slot.1 = v;
                }
            }
            Message::ConfirmSnippetVars => {
                if let Some(pending) = self.pending_snippet_vars.take() {
                    let cmd =
                        crate::util::substitute_snippet_vars(&pending.command, &pending.vars);
                    self.inject_snippet_text(&cmd, pending.run);
                }
            }
            Message::CancelSnippetVars => {
                self.pending_snippet_vars = None;
            }


            m => return Err(m),
        }
        Ok(Task::none())
    }
}
