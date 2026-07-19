//! `Oryxis::handle_history`: settings-panel-independent dispatch arms for the
//! history area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::too_many_lines)]

use iced::Task;

use crate::app::{HistoryMessage, CommandHistoryMessage, PluginMessage, Message, Oryxis};

impl Oryxis {
    pub(crate) fn handle_history(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            // -- History --
            // Clear now wipes both feeds the unified History timeline
            // mixes (failed-connect log rows + recorded session rows)
            // so the user gets a true "empty list" instead of seeing
            // every previously recorded session reappear after the
            // wipe finishes.
            Message::History(HistoryMessage::RequestClearHistory) => {
                // Close the `…` overflow menu before the confirm dialog
                // rises (no-op when triggered from the inline button).
                self.overlay = None;
                self.clear_history_confirm = true;
            }
            Message::History(HistoryMessage::CancelClearHistory) => {
                self.clear_history_confirm = false;
            }
            Message::History(HistoryMessage::ClearLogs) => {
                self.clear_history_confirm = false;
                if let Some(vault) = &self.vault {
                    let _ = vault.clear_logs();
                    let _ = vault.clear_session_logs();
                    self.logs_page = 0;
                    self.session_logs_page = 0;
                    self.load_data_from_vault();
                }
                // The wipe pulled the recording out from under any open
                // viewer / player; drop them with it.
                self.viewing_session_log = None;
                self.session_player = None;
            }
            Message::History(HistoryMessage::LogsPageNext) => {
                let max_page = (self.logs_total.saturating_sub(1)) / 50;
                if self.logs_page < max_page {
                    self.logs_page += 1;
                    if let Some(vault) = &self.vault {
                        self.logs = vault
                            .list_logs_page(self.logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }
            Message::History(HistoryMessage::LogsPagePrev) => {
                if self.logs_page > 0 {
                    self.logs_page -= 1;
                    if let Some(vault) = &self.vault {
                        self.logs = vault
                            .list_logs_page(self.logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }
            Message::History(HistoryMessage::ViewSessionLog(log_id)) => {
                // Flush buffered output first so viewing a still-active
                // session shows everything recorded up to this moment,
                // not just what was last persisted.
                self.flush_session_logs_final();
                if let Some(vault) = &self.vault
                    && let Ok(Some(data)) = vault.get_session_data(&log_id) {
                        let palette = self.resolve_global_terminal_palette();
                        let spans = crate::ansi_render::render(&data, &palette);
                        self.viewing_session_log = Some((log_id, spans));
                        // Mutually exclusive with the player surface.
                        self.session_player = None;
                }
            }
            Message::History(HistoryMessage::CloseSessionLogView) => {
                self.viewing_session_log = None;
            }
            Message::History(HistoryMessage::ShowSessionLogViewerMenu(idx)) => {
                use crate::state::{OverlayContent, OverlayState};
                // Toggle, mirroring the row kebab below.
                let already = matches!(
                    self.overlay.as_ref().map(|o| &o.content),
                    Some(OverlayContent::SessionLogViewerActions(i)) if *i == idx
                );
                if already {
                    self.overlay = None;
                } else {
                    let anchor = self.keynav_take_menu_anchor();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::SessionLogViewerActions(idx),
                        x: anchor.0,
                        y: anchor.1,
                    });
                }
            }
            Message::History(HistoryMessage::ShowSessionLogMenu(idx)) => {
                use crate::state::{OverlayContent, OverlayState};
                // Toggle, mirroring the other card kebabs.
                let already = matches!(
                    self.overlay.as_ref().map(|o| &o.content),
                    Some(OverlayContent::SessionLogActions(i)) if *i == idx
                );
                if already {
                    self.overlay = None;
                } else {
                    let anchor = self.keynav_take_menu_anchor();
                    self.overlay = Some(OverlayState {
                        content: OverlayContent::SessionLogActions(idx),
                        x: anchor.0,
                        y: anchor.1,
                    });
                }
            }
            Message::History(HistoryMessage::ExportSessionCast(log_id)) => {
                self.overlay = None;
                // Flush first so an in-progress session exports complete.
                self.flush_session_logs_final();
                let Some(entry) = self.session_logs.iter().find(|e| e.id == log_id) else {
                    return Ok(Task::none());
                };
                let Some(vault) = &self.vault else {
                    return Ok(Task::none());
                };
                let events = match vault.get_session_events(&log_id) {
                    Ok(ev) => ev,
                    Err(e) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_failed")
                                .replace("{error}", &e.to_string()),
                        ));
                    }
                };
                // Header term.type mirrors what the PTY actually requested:
                // the connection's terminal_type, or the engine's default.
                // A deleted / quick-connect host falls back the same way.
                // The embedded theme resolves like the live pane did:
                // per-host override first, then the global theme.
                let conn = self
                    .connections
                    .iter()
                    .find(|c| c.id == entry.connection_id);
                let term = conn
                    .and_then(|c| c.terminal_type.as_deref())
                    .unwrap_or("xterm-256color");
                let palette = conn
                    .map(|c| self.resolve_terminal_palette_for_connection(c))
                    .unwrap_or_else(|| self.resolve_global_terminal_palette());
                let body =
                    build_asciicast(&entry.label, entry.started_at, term, &palette, &events);
                let default_name = format!(
                    "oryxis-{}-{}.cast",
                    crate::util::sanitize_file_stem(&entry.label),
                    entry.started_at.format("%Y%m%d-%H%M%S"),
                );
                return Ok(save_text_file_task(body, default_name, "cast"));
            }
            Message::History(HistoryMessage::ExportSessionTranscript(log_id)) => {
                self.overlay = None;
                self.flush_session_logs_final();
                let Some(entry) = self.session_logs.iter().find(|e| e.id == log_id) else {
                    return Ok(Task::none());
                };
                let Some(vault) = &self.vault else {
                    return Ok(Task::none());
                };
                let data = match vault.get_session_data(&log_id) {
                    Ok(Some(d)) => d,
                    Ok(None) => return Ok(Task::none()),
                    Err(e) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_failed")
                                .replace("{error}", &e.to_string()),
                        ));
                    }
                };
                // Same pipeline as the in-app viewer: CR overwrites and
                // erase-line resolved, OSC/SGR stripped; what remains is
                // the text a human saw.
                let palette = self.resolve_global_terminal_palette();
                let body: String = crate::ansi_render::render(&data, &palette)
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect();
                let default_name = format!(
                    "oryxis-{}-{}.txt",
                    crate::util::sanitize_file_stem(&entry.label),
                    entry.started_at.format("%Y%m%d-%H%M%S"),
                );
                return Ok(save_text_file_task(body, default_name, "txt"));
            }
            Message::History(HistoryMessage::ExportSessionCommands(log_id)) => {
                self.overlay = None;
                // 'c' rows are written at capture time (never buffered),
                // so no flush is needed here.
                let Some(entry) = self.session_logs.iter().find(|e| e.id == log_id) else {
                    return Ok(Task::none());
                };
                let Some(vault) = &self.vault else {
                    return Ok(Task::none());
                };
                let events = match vault.get_session_commands(&log_id) {
                    Ok(ev) => ev,
                    Err(e) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_failed")
                                .replace("{error}", &e.to_string()),
                        ));
                    }
                };
                // Pre-feature recordings (and sessions where nothing was
                // typed at a prompt) have no command rows; say so instead
                // of silently writing an empty file.
                if events.is_empty() {
                    return Ok(self.show_toast(
                        crate::i18n::t("session_export_commands_empty").to_string(),
                    ));
                }
                let body = build_command_export(&entry.label, entry.started_at, &events);
                let default_name = format!(
                    "oryxis-{}-{}-input.txt",
                    crate::util::sanitize_file_stem(&entry.label),
                    entry.started_at.format("%Y%m%d-%H%M%S"),
                );
                return Ok(save_text_file_task(body, default_name, "txt"));
            }
            Message::History(HistoryMessage::ExportSessionGif(log_id)) => {
                self.overlay = None;
                if self.gif_export.running {
                    return Ok(self
                        .show_toast(crate::i18n::t("gif_export_started").to_string()));
                }
                // Plugin not installed yet: park the export and open the
                // consent modal; `PluginInstallDone("gif", Ok)` resumes.
                let Some(binary) = crate::gif_export::resolve_binary() else {
                    self.gif_export.pending = Some(log_id);
                    return Ok(self.update(Message::Plugin(PluginMessage::ShowPluginInstallModal(
                        crate::gif_export::PROVIDER_ID.to_string(),
                    ))));
                };
                // Same source as the .cast export: flush first, resolve
                // the terminal type + theme like the live pane did (the
                // embedded theme is what colors the GIF; agg reads it
                // from the header, no plumbing across the process).
                self.flush_session_logs_final();
                let Some(entry) = self.session_logs.iter().find(|e| e.id == log_id) else {
                    return Ok(Task::none());
                };
                let Some(vault) = &self.vault else {
                    return Ok(Task::none());
                };
                let events = match vault.get_session_events(&log_id) {
                    Ok(ev) => ev,
                    Err(e) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_failed")
                                .replace("{error}", &e.to_string()),
                        ));
                    }
                };
                let conn = self
                    .connections
                    .iter()
                    .find(|c| c.id == entry.connection_id);
                let term = conn
                    .and_then(|c| c.terminal_type.as_deref())
                    .unwrap_or("xterm-256color");
                let palette = conn
                    .map(|c| self.resolve_terminal_palette_for_connection(c))
                    .unwrap_or_else(|| self.resolve_global_terminal_palette());
                let body =
                    build_asciicast(&entry.label, entry.started_at, term, &palette, &events);
                let default_name = format!(
                    "oryxis-{}-{}.gif",
                    crate::util::sanitize_file_stem(&entry.label),
                    entry.started_at.format("%Y%m%d-%H%M%S"),
                );
                self.gif_export.running = true;
                let start_toast = self
                    .show_toast_secs(crate::i18n::t("gif_export_started").to_string(), 4);
                let render = Task::perform(
                    async move {
                        // Save dialog off the UI thread; a dismissed
                        // dialog reports nothing (None).
                        let picked = tokio::task::spawn_blocking(move || {
                            rfd::FileDialog::new()
                                .set_file_name(&default_name)
                                .add_filter("gif", &["gif"])
                                .save_file()
                        })
                        .await
                        .ok()
                        .flatten();
                        match picked {
                            None => None,
                            Some(path) => Some(
                                crate::gif_export::render(binary, body, path).await,
                            ),
                        }
                    },
                    |v| Message::History(HistoryMessage::GifExportFinished(v)),
                );
                return Ok(Task::batch([start_toast, render]));
            }
            Message::History(HistoryMessage::GifExportFinished(outcome)) => {
                self.gif_export.running = false;
                match outcome {
                    None => {}
                    Some(Ok(path)) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_done")
                                .replace("{path}", &path),
                        ));
                    }
                    Some(Err(cause)) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_failed")
                                .replace("{error}", &cause),
                        ));
                    }
                }
            }
            Message::History(HistoryMessage::RequestDeleteSessionLog(idx)) => {
                // Reached from the row kebab; drop it before the dialog.
                self.overlay = None;
                let label = self
                    .session_logs
                    .get(idx)
                    .map(|e| e.label.clone())
                    .unwrap_or_default();
                self.error_dialog = Some(crate::state::ErrorDialog {
                    title: crate::i18n::t("log_delete_confirm_title").to_string(),
                    body: format!(
                        "{label}: {}",
                        crate::i18n::t("log_delete_confirm_body")
                    ),
                    link: None,
                    action: Some(crate::state::ErrorDialogAction {
                        label: crate::i18n::t("delete").to_string(),
                        message: Box::new(Message::History(HistoryMessage::DeleteSessionLog(idx))),
                        danger: true,
                    }),
                });
            }
            Message::TogglePrivacyReveal => {
                self.privacy.revealed = !self.privacy.revealed;
            }
            Message::History(HistoryMessage::LogRowHovered(id)) => {
                self.hovered_log_row = Some(id);
            }
            Message::History(HistoryMessage::LogRowUnhovered) => {
                self.hovered_log_row = None;
            }
            Message::History(HistoryMessage::DeleteSessionLog(idx)) => {
                if let Some(entry) = self.session_logs.get(idx) {
                    let id = entry.id;
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_session_log(&id);
                        self.session_logs_total =
                            vault.count_session_logs().unwrap_or(0);
                        // Stepping a page back when the current one is now
                        // empty keeps the prev/next pair from leaving the
                        // user staring at "0 of N" with rows further back.
                        let max_page = self
                            .session_logs_total
                            .saturating_sub(1)
                            / 50;
                        if self.session_logs_page > max_page {
                            self.session_logs_page = max_page;
                        }
                        self.session_logs = vault
                            .list_session_logs_page(self.session_logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
                // Close viewer / player if we deleted the one being shown
                if let Some((viewed_id, _)) = &self.viewing_session_log
                    && self.session_logs.iter().all(|s| s.id != *viewed_id) {
                        self.viewing_session_log = None;
                }
                if let Some(p) = &self.session_player
                    && self.session_logs.iter().all(|s| s.id != p.log_id) {
                        self.session_player = None;
                }
            }
            Message::History(HistoryMessage::ClearSessionLogs) => {
                if let Some(vault) = &self.vault {
                    let _ = vault.clear_session_logs();
                    self.session_logs_page = 0;
                    self.load_data_from_vault();
                }
                self.viewing_session_log = None;
                self.session_player = None;
            }
            Message::History(HistoryMessage::SessionLogsPageNext) => {
                let max_page = self.session_logs_total.saturating_sub(1) / 50;
                if self.session_logs_page < max_page {
                    self.session_logs_page += 1;
                    if let Some(vault) = &self.vault {
                        self.session_logs = vault
                            .list_session_logs_page(self.session_logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }
            Message::History(HistoryMessage::SessionLogsPagePrev) => {
                if self.session_logs_page > 0 {
                    self.session_logs_page -= 1;
                    if let Some(vault) = &self.vault {
                        self.session_logs = vault
                            .list_session_logs_page(self.session_logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }

            Message::OpenUrl(url) => {
                if let Err(e) = crate::util::open_in_browser(&url) {
                    tracing::warn!("open_in_browser({url}) failed: {e}");
                }
            }
            Message::History(HistoryMessage::CopyHostSshUrl(idx)) => {
                // Card action: canonical ssh:// URL for the host. Closes
                // the context menu itself (CopyToClipboard stays free of
                // menu state; it is also dispatched from inside overlays).
                self.card_context_menu = None;
                self.overlay = None;
                let Some(conn) = self.connections.get(idx) else {
                    return Ok(Task::none());
                };
                let url = self.host_ssh_url(conn);
                return Ok(self.update(Message::CopyToClipboard(url)));
            }
            Message::CopyToClipboard(content) => {
                let mut ok = false;
                if let Ok(mut clip) = arboard::Clipboard::new() {
                    match clip.set_text(content) {
                        Ok(()) => ok = true,
                        Err(e) => tracing::warn!("clipboard set_text failed: {e}"),
                    }
                }
                if ok {
                    self.set_toast(crate::i18n::t("copied_to_clipboard").to_string());
                    return Ok(Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                        },
                        |_| Message::ToastClear,
                    ));
                }
            }
            Message::ToastClear => {
                // Deadline-guarded auto-dismiss (subscription tick or a
                // legacy scheduled sleep-timer). Only the current toast's
                // own elapsed deadline clears it, so a superseded timer can
                // never wipe a newer toast.
                if self
                    .toast_deadline
                    .is_some_and(|d| std::time::Instant::now() >= d)
                {
                    self.toast = None;
                    self.toast_deadline = None;
                }
            }
            Message::ToastDismiss => {
                // Explicit click on the chip: clear immediately.
                self.toast = None;
                self.toast_deadline = None;
            }
            Message::ErrorDialogRunAction => {
                if let Some(dialog) = self.error_dialog.take()
                    && let Some(action) = dialog.action
                {
                    return Ok(self.update(*action.message));
                }
            }
            Message::ErrorDialogDismiss => {
                self.error_dialog = None;
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}

/// Serialize a recorded session as an asciicast v3 document: a JSON
/// header line (`version: 3`, a `term` object carrying geometry,
/// terminal type and the effective color theme, start timestamp,
/// title), then one `[interval_sec, "o"|"r", data]` line per event,
/// timed as the interval since the PREVIOUS event (v3 semantics; the
/// stored offsets are integer milliseconds, so emitted intervals sum
/// exactly and need no rounding-drift correction). The embedded
/// `term.theme` is what lets players and agg reproduce the terminal
/// colors without any side-channel. Output-only by design: input
/// events are never recorded, so the keystroke-leak class doesn't
/// exist here. Chunks recorded before the timing migration
/// (`offset_ms = None`) replay with a small fixed delta so old logs
/// still play, just without real pacing. No `idle_time_limit` on
/// purpose: capping pauses in the file would bake a pacing opinion
/// into a 1:1 recording; players take it as a playback option instead.
fn build_asciicast(
    label: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    term: &str,
    palette: &oryxis_terminal::TerminalPalette,
    events: &[oryxis_vault::SessionLogEvent],
) -> String {
    // Geometry: the first recorded resize (the initial size lands on
    // the first flush); a legacy log without one replays at 80x24.
    let (width, height) = events
        .iter()
        .find(|e| e.kind == 'r')
        .and_then(|e| {
            let s = String::from_utf8_lossy(&e.data);
            let (c, r) = s.split_once('x')?;
            Some((c.parse::<u16>().ok()?, r.parse::<u16>().ok()?))
        })
        .unwrap_or((80, 24));
    let hex = crate::theme::color_to_hex;
    let theme_palette: String = palette
        .ansi
        .iter()
        .map(|c| hex(*c))
        .collect::<Vec<_>>()
        .join(":");
    let mut out = String::new();
    out.push_str(&format!(
        "{}\n",
        serde_json::json!({
            "version": 3,
            "term": {
                "cols": width,
                "rows": height,
                "type": term,
                "theme": {
                    "fg": hex(palette.foreground),
                    "bg": hex(palette.background),
                    "palette": theme_palette,
                },
            },
            "timestamp": started_at.timestamp(),
            "title": label,
        })
    ));
    /// Untimed-chunk replay step (legacy rows), in milliseconds.
    const LEGACY_DELTA_MS: i64 = 50;
    let mut last_ms: i64 = 0;
    for event in events {
        // Typed-command rows feed the input-only .txt export; the cast
        // replay stays output-only (they are not asciicast "i" events:
        // resolved command lines, not keystrokes, and their echo is
        // already in the output stream).
        if event.kind == 'c' {
            continue;
        }
        let ms = match event.offset_ms {
            // Intervals must be >= 0; clamp against interleavings (a
            // resize stamped at flush time can sit a hair before the
            // chunk rows written in the same batch).
            Some(ms) => ms.max(last_ms),
            None => last_ms + LEGACY_DELTA_MS,
        };
        let interval_ms = ms - last_ms;
        last_ms = ms;
        let kind = if event.kind == 'r' { "r" } else { "o" };
        let data = String::from_utf8_lossy(&event.data);
        out.push_str(&format!(
            "{}\n",
            serde_json::json!([interval_ms as f64 / 1000.0, kind, data])
        ));
    }
    out
}

/// Input-only export body: a small header, then one line per typed
/// command in capture order. Timed rows (full-detail recording)
/// prefix the absolute timestamp, tab-separated, mirroring the
/// per-host command-history export; untimed rows are bare. Multi-line
/// commands indent their continuation lines, same convention as the
/// live-append log.
fn build_command_export(
    label: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    events: &[oryxis_vault::SessionLogEvent],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Oryxis session input: {label}\n# Session started {}\n\n",
        started_at.to_rfc3339()
    ));
    for event in events {
        let cmd = String::from_utf8_lossy(&event.data);
        let cmd_one = cmd.replace('\n', "\n    ");
        match event.offset_ms {
            Some(ms) => {
                let at = started_at + chrono::Duration::milliseconds(ms);
                out.push_str(&format!("{}\t{}\n", at.to_rfc3339(), cmd_one));
            }
            None => out.push_str(&format!("{cmd_one}\n")),
        }
    }
    out
}

/// Save-dialog + write for a text export, off the UI thread. Reports
/// through the shared "Exported to {path}" / failure toast; a
/// dismissed dialog reports nothing.
fn save_text_file_task(
    body: String,
    default_name: String,
    ext: &'static str,
) -> Task<Message> {
    Task::perform(
        tokio::task::spawn_blocking(move || {
            let path = rfd::FileDialog::new()
                .set_file_name(&default_name)
                .add_filter(ext, &[ext])
                .save_file()?;
            Some(
                std::fs::write(&path, body)
                    .map(|_| path.display().to_string())
                    .map_err(|e| e.to_string()),
            )
        }),
        |res| match res {
            Ok(Some(outcome)) => Message::CommandHistory(CommandHistoryMessage::CommandHistoryExported(outcome)),
            _ => Message::NoOp,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::build_asciicast;
    use oryxis_vault::SessionLogEvent;

    fn ev(offset_ms: Option<i64>, kind: char, data: &[u8]) -> SessionLogEvent {
        SessionLogEvent { offset_ms, kind, data: data.to_vec() }
    }

    fn palette() -> oryxis_terminal::TerminalPalette {
        oryxis_terminal::TerminalPalette::default()
    }

    #[test]
    fn asciicast_header_reads_geometry_from_the_first_resize() {
        let started = chrono::DateTime::parse_from_rfc3339("2026-07-04T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let cast = build_asciicast(
            "host-a",
            started,
            "xterm-256color",
            &palette(),
            &[ev(Some(0), 'r', b"120x30"), ev(Some(100), 'o', b"hi")],
        );
        let mut lines = cast.lines();
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["version"], 3);
        assert_eq!(header["term"]["cols"], 120);
        assert_eq!(header["term"]["rows"], 30);
        assert_eq!(header["term"]["type"], "xterm-256color");
        assert_eq!(header["title"], "host-a");
        assert_eq!(header["timestamp"], started.timestamp());
        let first: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first[1], "r");
        assert_eq!(first[2], "120x30");
        let second: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(second[0], 0.1);
        assert_eq!(second[1], "o");
        assert_eq!(second[2], "hi");
    }

    #[test]
    fn asciicast_header_embeds_the_terminal_theme() {
        let cast = build_asciicast(
            "host-a",
            chrono::Utc::now(),
            "xterm-256color",
            &palette(),
            &[ev(Some(0), 'o', b"hi")],
        );
        let header: serde_json::Value =
            serde_json::from_str(cast.lines().next().unwrap()).unwrap();
        let theme = &header["term"]["theme"];
        let is_hex = |v: &serde_json::Value| {
            let s = v.as_str().unwrap();
            s.len() == 7
                && s.starts_with('#')
                && s[1..].chars().all(|c| c.is_ascii_hexdigit())
        };
        assert!(is_hex(&theme["fg"]), "bad fg: {theme}");
        assert!(is_hex(&theme["bg"]), "bad bg: {theme}");
        // The v3 spec wants 8 or 16 colon-separated #rrggbb entries;
        // we always emit the full 16-color ANSI set.
        let colors: Vec<&str> =
            theme["palette"].as_str().unwrap().split(':').collect();
        assert_eq!(colors.len(), 16, "bad palette: {theme}");
        assert!(colors
            .iter()
            .all(|c| c.len() == 7 && c.starts_with('#')));
    }

    #[test]
    fn asciicast_skips_typed_command_rows() {
        let started = chrono::Utc::now();
        let cast = build_asciicast(
            "host-a",
            started,
            "xterm-256color",
            &palette(),
            &[
                ev(Some(0), 'o', b"prompt$ "),
                ev(Some(50), 'c', b"ls -la"),
                ev(Some(100), 'o', b"total 0"),
            ],
        );
        assert!(
            !cast.contains("ls -la"),
            "command row leaked into the cast: {cast}"
        );
        assert_eq!(cast.lines().count(), 3, "header + 2 output events");
    }

    #[test]
    fn command_export_prefixes_timestamps_and_indents_continuations() {
        let started = chrono::DateTime::parse_from_rfc3339("2026-07-08T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let body = super::build_command_export(
            "host-a",
            started,
            &[
                ev(Some(60_000), 'c', b"ls -la"),
                ev(None, 'c', b"for f in *; do\necho $f\ndone"),
            ],
        );
        assert!(body.starts_with("# Oryxis session input: host-a\n"));
        assert!(
            body.contains("2026-07-08T10:01:00+00:00\tls -la\n"),
            "timed row missing its absolute timestamp: {body}"
        );
        // Untimed rows are bare; continuation lines stay indented so
        // the file remains greppable per entry.
        assert!(
            body.contains("\nfor f in *; do\n    echo $f\n    done\n"),
            "untimed multi-line row malformed: {body}"
        );
    }

    #[test]
    fn untimed_events_replay_with_a_fixed_delta_and_intervals_never_regress() {
        let started = chrono::Utc::now();
        let cast = build_asciicast(
            "legacy",
            started,
            "vt100",
            &palette(),
            &[
                ev(None, 'o', b"one"),
                ev(None, 'o', b"two"),
                // A stamped event earlier than the synthetic clock must
                // clamp forward: v3 intervals are relative to the
                // previous event and can never be negative.
                ev(Some(20), 'o', b"three"),
            ],
        );
        let intervals: Vec<f64> = cast
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()[0].as_f64().unwrap())
            .collect();
        assert_eq!(intervals, vec![0.05, 0.05, 0.0]);
        // No resize event anywhere: the header falls back to 80x24.
        let header: serde_json::Value =
            serde_json::from_str(cast.lines().next().unwrap()).unwrap();
        assert_eq!(header["term"]["cols"], 80);
        assert_eq!(header["term"]["rows"], 24);
    }
}
