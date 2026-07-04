//! `Oryxis::handle_history`: settings-panel-independent dispatch arms for the
//! history area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::too_many_lines)]

use iced::Task;

use crate::app::{Message, Oryxis};

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
            Message::RequestClearHistory => {
                // Close the `…` overflow menu before the confirm dialog
                // rises (no-op when triggered from the inline button).
                self.overlay = None;
                self.clear_history_confirm = true;
            }
            Message::CancelClearHistory => {
                self.clear_history_confirm = false;
            }
            Message::ClearLogs => {
                self.clear_history_confirm = false;
                if let Some(vault) = &self.vault {
                    let _ = vault.clear_logs();
                    let _ = vault.clear_session_logs();
                    self.logs_page = 0;
                    self.session_logs_page = 0;
                    self.load_data_from_vault();
                }
            }
            Message::LogsPageNext => {
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
            Message::LogsPagePrev => {
                if self.logs_page > 0 {
                    self.logs_page -= 1;
                    if let Some(vault) = &self.vault {
                        self.logs = vault
                            .list_logs_page(self.logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                }
            }
            Message::ViewSessionLog(log_id) => {
                // Flush buffered output first so viewing a still-active
                // session shows everything recorded up to this moment,
                // not just what was last persisted.
                self.flush_session_logs_final();
                if let Some(vault) = &self.vault
                    && let Ok(Some(data)) = vault.get_session_data(&log_id) {
                        let palette = self.resolve_global_terminal_palette();
                        let spans = crate::ansi_render::render(&data, &palette);
                        self.viewing_session_log = Some((log_id, spans));
                }
            }
            Message::CloseSessionLogView => {
                self.viewing_session_log = None;
            }
            Message::ShowSessionLogMenu(idx) => {
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
            Message::ExportSessionCast(log_id) => {
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
                let body = build_asciicast(&entry.label, entry.started_at, &events);
                let default_name = format!(
                    "oryxis-{}-{}.cast",
                    crate::util::sanitize_file_stem(&entry.label),
                    entry.started_at.format("%Y%m%d-%H%M%S"),
                );
                return Ok(save_text_file_task(body, default_name, "cast"));
            }
            Message::ExportSessionTranscript(log_id) => {
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
            Message::RequestDeleteSessionLog(idx) => {
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
                        message: Box::new(Message::DeleteSessionLog(idx)),
                        danger: true,
                    }),
                });
            }
            Message::TogglePrivacyReveal => {
                self.privacy_revealed = !self.privacy_revealed;
            }
            Message::LogRowHovered(id) => {
                self.hovered_log_row = Some(id);
            }
            Message::LogRowUnhovered => {
                self.hovered_log_row = None;
            }
            Message::DeleteSessionLog(idx) => {
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
                // Close viewer if we deleted the one being viewed
                if let Some((viewed_id, _)) = &self.viewing_session_log
                    && self.session_logs.iter().all(|s| s.id != *viewed_id) {
                        self.viewing_session_log = None;
                }
            }
            Message::ClearSessionLogs => {
                if let Some(vault) = &self.vault {
                    let _ = vault.clear_session_logs();
                    self.session_logs_page = 0;
                    self.load_data_from_vault();
                }
                self.viewing_session_log = None;
            }
            Message::SessionLogsPageNext => {
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
            Message::SessionLogsPagePrev => {
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
            Message::CopyHostSshUrl(idx) => {
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
                    self.toast = Some(crate::i18n::t("copied_to_clipboard").to_string());
                    return Ok(Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
                        },
                        |_| Message::ToastClear,
                    ));
                }
            }
            Message::ToastClear => {
                self.toast = None;
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

/// Serialize a recorded session as an asciicast v2 document: a JSON
/// header line (`version: 2`, geometry, start timestamp, title), then
/// one `[time_sec, "o"|"r", data]` line per event. Output-only by
/// design: input events are never recorded, so the keystroke-leak
/// class doesn't exist here. Chunks recorded before the timing
/// migration (`offset_ms = None`) replay with a small fixed delta so
/// old logs still play, just without real pacing.
fn build_asciicast(
    label: &str,
    started_at: chrono::DateTime<chrono::Utc>,
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
    let mut out = String::new();
    out.push_str(&format!(
        "{}\n",
        serde_json::json!({
            "version": 2,
            "width": width,
            "height": height,
            "timestamp": started_at.timestamp(),
            "title": label,
        })
    ));
    /// Untimed-chunk replay step (legacy rows), in milliseconds.
    const LEGACY_DELTA_MS: i64 = 50;
    let mut last_ms: i64 = 0;
    for event in events {
        let ms = match event.offset_ms {
            // Times must be non-decreasing for players; clamp against
            // interleavings (a resize stamped at flush time can sit a
            // hair before the chunk rows written in the same batch).
            Some(ms) => ms.max(last_ms),
            None => last_ms + LEGACY_DELTA_MS,
        };
        last_ms = ms;
        let kind = if event.kind == 'r' { "r" } else { "o" };
        let data = String::from_utf8_lossy(&event.data);
        out.push_str(&format!(
            "{}\n",
            serde_json::json!([ms as f64 / 1000.0, kind, data])
        ));
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
            Ok(Some(outcome)) => Message::CommandHistoryExported(outcome),
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

    #[test]
    fn asciicast_header_reads_geometry_from_the_first_resize() {
        let started = chrono::DateTime::parse_from_rfc3339("2026-07-04T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let cast = build_asciicast(
            "host-a",
            started,
            &[ev(Some(0), 'r', b"120x30"), ev(Some(100), 'o', b"hi")],
        );
        let mut lines = cast.lines();
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["version"], 2);
        assert_eq!(header["width"], 120);
        assert_eq!(header["height"], 30);
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
    fn untimed_events_replay_with_a_fixed_delta_and_times_never_regress() {
        let started = chrono::Utc::now();
        let cast = build_asciicast(
            "legacy",
            started,
            &[
                ev(None, 'o', b"one"),
                ev(None, 'o', b"two"),
                // A stamped event earlier than the synthetic clock must
                // clamp forward, players reject regressing times.
                ev(Some(20), 'o', b"three"),
            ],
        );
        let times: Vec<f64> = cast
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()[0].as_f64().unwrap())
            .collect();
        assert_eq!(times.len(), 3);
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "times regressed: {times:?}");
        // No resize event anywhere: the header falls back to 80x24.
        let header: serde_json::Value =
            serde_json::from_str(cast.lines().next().unwrap()).unwrap();
        assert_eq!(header["width"], 80);
        assert_eq!(header["height"], 24);
    }
}
