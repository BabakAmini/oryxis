//! Command-history plumbing: the central user-input write sink (which feeds
//! the capture), vault recording, the sidebar list refresh and the History
//! tab's message handlers.

// The `Err(message)` pass-through of the try_handler! chain carries the full
// Message enum by design; same allowance as the sibling dispatch modules.
#![allow(clippy::result_large_err)]

use crate::app::Oryxis;
use crate::messages::Message;
use iced::Task;
use uuid::Uuid;

impl Oryxis {
    pub(crate) fn handle_command_history(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::HistoryCardHovered(idx) => {
                self.hovered_history_card = Some(idx);
            }
            Message::HistoryCardUnhovered => {
                self.hovered_history_card = None;
            }
            Message::CmdHistorySearchChanged(v) => {
                self.cmd_history_search = v;
            }
            Message::RunHistoryCommand(id) => {
                self.inject_history_command(id, true);
            }
            Message::PasteHistoryCommand(id) => {
                self.inject_history_command(id, false);
            }
            Message::RequestDeleteHistoryCommand(id) => {
                // Deleting is destructive and the trash icon floats over
                // the row on hover, one pixel from the paste click, so it
                // goes through the shared confirm (Enter confirms via the
                // modal keyboard layer, like every other destructive).
                if let Some(entry) = self.command_history.iter().find(|e| e.id == id) {
                    let name: String = entry.command.lines().next().unwrap_or("").chars().take(48).collect();
                    self.confirm_remove(name, Message::DeleteHistoryCommand(id));
                }
            }
            Message::DeleteHistoryCommand(id) => {
                if let Some(ref vault) = self.vault {
                    let _ = vault.delete_command_history_entry(&id);
                }
                self.command_history.retain(|e| e.id != id);
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }

    /// Write user-originated `bytes` to the tab's focused pane (SSH session
    /// or local PTY) and mirror them into the command-history capture. Every
    /// user input path funnels through here; the one deliberate exception is
    /// the sudo-password autofill, which writes directly so a secret never
    /// touches the capture's line mirror.
    pub(crate) fn write_input_to_tab(&mut self, tab_idx: usize, bytes: &[u8]) {
        // Typing into the terminal DISENGAGES the sidebar keynav ring: the
        // user has moved on, and a lingering ring would keep consuming
        // Enter (live-QA bug: Enter appeared dead on an SSH tab because a
        // forgotten ring from a Ctrl+Shift+H test was swallowing it).
        // Sidebar-originated injections use the `_ring_injection_` variant
        // below, which keeps the ring so arrow-Enter-arrow-Enter works.
        self.keynav.sidebar_selected = None;
        self.write_ring_injection_to_tab(tab_idx, bytes);
    }

    /// [`Self::write_input_to_tab`] without the ring disengage, for
    /// injections the sidebar itself fires (row Paste / Run actions).
    pub(crate) fn write_ring_injection_to_tab(&mut self, tab_idx: usize, bytes: &[u8]) {
        if let Some(tab) = self.tabs.get_mut(tab_idx) {
            let pane = tab.active_mut();
            if let Some(ref ssh) = pane.ssh_session {
                let _ = ssh.write(bytes);
            } else if let Ok(mut state) = pane.terminal.lock() {
                state.write(bytes);
            }
        }
        self.feed_input_capture(tab_idx, bytes);
    }

    /// Capture half of [`Self::write_input_to_tab`], for the rare call site
    /// that must write directly (the AI tool-exec path needs the write's
    /// success) but still wants the bytes mirrored into the history capture.
    pub(crate) fn feed_input_capture(&mut self, tab_idx: usize, bytes: &[u8]) {
        if !self.setting_command_history {
            return;
        }
        let mut captured: Vec<(Uuid, String)> = Vec::new();
        if let Some(tab) = self.tabs.get_mut(tab_idx) {
            let pane = tab.active_mut();
            // Only saved hosts get history (quick-connect / local / cloud
            // panes have no persistable identity to key it on).
            if let crate::state::PaneOrigin::Host(hid) = &pane.origin {
                let hid = *hid;
                captured.extend(
                    crate::command_capture::observe_input(pane, bytes)
                        .into_iter()
                        .map(|cmd| (hid, cmd)),
                );
            }
        }
        for (host, cmd) in captured {
            self.record_command_history(host, cmd);
        }
    }

    /// Persist one captured command and keep the open History tab live.
    pub(crate) fn record_command_history(&mut self, host: Uuid, cmd: String) {
        if let Some(ref vault) = self.vault
            && let Err(e) = vault.record_command(&host, &cmd)
        {
            tracing::warn!("command-history write failed: {e}");
        }
        if self.terminal_sidebar_tab == crate::state::TerminalSidebarTab::History
            && self.command_history_host == Some(host)
        {
            self.refresh_command_history();
        }
    }

    /// Reload the sidebar list for the focused pane's host. Called when the
    /// History tab is opened, when tab/pane focus moves, and after a record
    /// while the tab is showing that host.
    pub(crate) fn refresh_command_history(&mut self) {
        let host = self
            .active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(|t| match &t.active().origin {
                crate::state::PaneOrigin::Host(id) => Some(*id),
                _ => None,
            });
        self.command_history_host = host;
        self.command_history = match (host, &self.vault) {
            (Some(h), Some(v)) => v.list_command_history(&h).unwrap_or_default(),
            _ => Vec::new(),
        };
        self.hovered_history_card = None;
    }

    /// Re-insert a history entry into the active terminal, exactly like a
    /// snippet: bracketed-paste wrapped, with the submit newline outside the
    /// bracket when `run`. Goes through the capture sink, so running it
    /// counts another use.
    fn inject_history_command(&mut self, id: Uuid, run: bool) {
        let Some(cmd) = self
            .command_history
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.command.clone())
        else {
            return;
        };
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
        let mut payload = oryxis_terminal::wrap_paste(&cmd, bracketed);
        if run {
            payload.push(b'\n');
        }
        // Ring-preserving: Enter on a ringed row must not drop the ring,
        // so the user can arrow to the next command and Enter again.
        self.write_ring_injection_to_tab(tab_idx, &payload);
    }
}
