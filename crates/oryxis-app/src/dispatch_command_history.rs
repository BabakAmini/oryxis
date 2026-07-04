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
            Message::ExportCommandHistory => {
                let Some(host) = self.command_history_host else {
                    return Ok(Task::none());
                };
                if self.command_history.is_empty() {
                    return Ok(Task::none());
                }
                let label = self
                    .connections
                    .iter()
                    .find(|c| c.id == host)
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| host.to_string());
                let body = render_history_txt(&label, &self.command_history);
                let default_name = format!("oryxis-history-{}.txt", crate::util::sanitize_file_stem(&label));
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(move || {
                        let path = rfd::FileDialog::new()
                            .set_title("Export command history")
                            .set_file_name(&default_name)
                            .add_filter("Text", &["txt"])
                            .save_file()?;
                        Some(
                            std::fs::write(&path, body)
                                .map(|_| path.display().to_string())
                                .map_err(|e| e.to_string()),
                        )
                    }),
                    |res| match res {
                        Ok(Some(outcome)) => Message::CommandHistoryExported(outcome),
                        // Dialog dismissed or the blocking task died:
                        // nothing to report.
                        _ => Message::NoOp,
                    },
                ));
            }
            Message::CommandHistoryExported(result) => {
                return Ok(match result {
                    Ok(path) => self.show_toast(
                        crate::i18n::t("history_export_done").replace("{path}", &path),
                    ),
                    Err(e) => self.show_toast(
                        crate::i18n::t("history_export_failed").replace("{error}", &e),
                    ),
                });
            }
            Message::ToggleCommandHistoryFile => {
                self.setting_command_history_file = !self.setting_command_history_file;
                self.persist_setting(
                    "command_history_file",
                    if self.setting_command_history_file { "true" } else { "false" },
                );
            }
            Message::PickCommandHistoryDir => {
                return Ok(Task::perform(
                    tokio::task::spawn_blocking(|| {
                        rfd::FileDialog::new()
                            .set_title("Command log folder")
                            .pick_folder()
                            .map(|p| p.display().to_string())
                    }),
                    |res| Message::CommandHistoryDirPicked(res.ok().flatten()),
                ));
            }
            Message::CommandHistoryDirPicked(dir) => {
                if let Some(dir) = dir {
                    self.persist_setting("command_history_file_dir", &dir);
                    self.setting_command_history_file_dir = Some(dir);
                }
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
                // Tripwire for the debug log: history rows only ever
                // leave the vault through here (or a host deletion), so
                // any future "my history vanished" report is
                // attributable at a glance.
                tracing::info!(%id, "command-history: entry deleted by user");
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
            // A ZMODEM transfer owns the byte channel: user keystrokes
            // would interleave with the protocol and corrupt it, so
            // input is suppressed until the transfer ends. Cancelling is
            // done from the overlay's Cancel button (`ZmodemCancel`).
            if pane.zmodem.is_some() {
                return;
            }
            if let Some(ref session) = pane.session {
                let _ = session.write(bytes);
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
        // Optional plain-text mirror: append to the host's log file for
        // offline reference / support sharing. Plain filesystem write on
        // purpose (no vault), that is the feature.
        if self.setting_command_history_file {
            let label = self
                .connections
                .iter()
                .find(|c| c.id == host)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| host.to_string());
            if let Err(e) = self.append_command_log(&host, &label, &cmd) {
                tracing::warn!("command-history file append failed: {e}");
            }
        }
        if self.terminal_sidebar_tab == crate::state::TerminalSidebarTab::History
            && self.command_history_host == Some(host)
        {
            self.refresh_command_history();
        }
    }

    /// The folder the per-host command logs live in: the configured
    /// setting, or `~/.oryxis/command-history/` by default.
    pub(crate) fn command_history_dir(&self) -> std::path::PathBuf {
        match &self.setting_command_history_file_dir {
            Some(dir) => std::path::PathBuf::from(dir),
            None => dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".oryxis")
                .join("command-history"),
        }
    }

    /// Append one captured command to the host's log file
    /// (`<dir>/<label>-<uuid8>.txt`, one `timestamp<TAB>command` line).
    /// The uuid suffix keeps two hosts with the same label apart and
    /// the file name stable across renames of neither.
    fn append_command_log(
        &self,
        host: &Uuid,
        label: &str,
        cmd: &str,
    ) -> std::io::Result<()> {
        use std::io::Write;
        let dir = self.command_history_dir();
        std::fs::create_dir_all(&dir)?;
        let short: String = host.to_string().chars().take(8).collect();
        let path = dir.join(format!("{}-{}.txt", crate::util::sanitize_file_stem(label), short));
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        // Multi-line commands (bracketed paste) stay one log line each:
        // continuation lines are indented so the file remains greppable
        // per entry.
        let cmd_one = cmd.replace('\n', "\n    ");
        writeln!(f, "{}\t{}", chrono::Utc::now().to_rfc3339(), cmd_one)
    }
}

/// Human-readable export body: a small header, then one line per
/// captured command, oldest first (the in-memory list is
/// most-recent-first). Multi-line commands indent their continuation
/// lines, same convention as the live-append log.
fn render_history_txt(
    label: &str,
    entries: &[oryxis_vault::CommandHistoryEntry],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Oryxis command history: {label}\n# Exported {}\n\n",
        chrono::Utc::now().to_rfc3339()
    ));
    for e in entries.iter().rev() {
        let cmd_one = e.command.replace('\n', "\n    ");
        let uses = if e.use_count > 1 {
            format!("\t(x{})", e.use_count)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "{}{}\t{}\n",
            e.last_used_at.to_rfc3339(),
            uses,
            cmd_one
        ));
    }
    out
}


impl Oryxis {
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
