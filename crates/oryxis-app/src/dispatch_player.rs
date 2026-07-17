//! `Oryxis::handle_player`: dispatch arms for the in-app session
//! player (issue #71), plus the keyboard layer the player surface owns
//! while it is up on the History view. Returns `Err(message)` for
//! anything it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]

use iced::keyboard;
use iced::Task;

use crate::app::{Message, Oryxis};

/// Arrow-key seek step, in playback milliseconds (the asciinema
/// player's default step).
const SEEK_STEP_MS: f64 = 5_000.0;

/// Cap on the wall-clock delta credited per tick. The subscription
/// unmounts when the user leaves the History view; without the cap the
/// first tick after coming back would credit the whole absence as
/// playback time and skip ahead. Clamping makes a suspended
/// subscription behave as a pause instead.
const MAX_TICK_MS: f64 = 250.0;

impl Oryxis {
    pub(crate) fn handle_player(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::PlaySessionLog(log_id) => {
                // Any open kebab menu should drop before the surface
                // swaps; flush first so an in-progress session plays
                // everything recorded up to this moment.
                self.overlay = None;
                self.flush_session_logs_final();
                let Some(entry) = self.session_logs.iter().find(|e| e.id == log_id) else {
                    return Ok(Task::none());
                };
                let Some(vault) = &self.vault else {
                    return Ok(Task::none());
                };
                let rows = match vault.get_session_events(&log_id) {
                    Ok(rows) => rows,
                    Err(e) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_failed")
                                .replace("{error}", &e.to_string()),
                        ));
                    }
                };
                let (events, duration_ms, geometry) =
                    crate::state::preprocess_events(&rows);
                if events.is_empty() {
                    return Ok(self
                        .show_toast(crate::i18n::t("player_empty").to_string()));
                }
                // The replay wears the same colors the live pane wore:
                // per-host terminal theme override first, then the
                // global theme (mirrors the `.cast` export header).
                let palette = self
                    .connections
                    .iter()
                    .find(|c| c.id == entry.connection_id)
                    .map(|c| self.resolve_terminal_palette_for_connection(c))
                    .unwrap_or_else(|| self.resolve_global_terminal_palette());
                match crate::state::SessionPlayer::new(
                    log_id,
                    entry.label.clone(),
                    events,
                    duration_ms,
                    geometry,
                    palette,
                ) {
                    Ok(mut player) => {
                        // Show the recording's first frame immediately
                        // (initial resize + prompt land at t=0).
                        player.feed_due();
                        player.last_tick = Some(std::time::Instant::now());
                        self.viewing_session_log = None;
                        self.session_player = Some(player);
                    }
                    Err(e) => {
                        return Ok(self.show_toast(
                            crate::i18n::t("history_export_failed")
                                .replace("{error}", &e.to_string()),
                        ));
                    }
                }
            }
            Message::SessionPlayerClose => {
                self.session_player = None;
            }
            Message::SessionPlayerTogglePlay => {
                if let Some(p) = &mut self.session_player {
                    p.toggle_play();
                }
            }
            Message::SessionPlayerRestart => {
                if let Some(p) = &mut self.session_player {
                    p.restart();
                }
            }
            Message::SessionPlayerSpeedCycle => {
                if let Some(p) = &mut self.session_player {
                    p.cycle_speed();
                }
            }
            Message::SessionPlayerSeek(target_ms) => {
                if let Some(p) = &mut self.session_player {
                    p.seek(target_ms);
                }
            }
            Message::SessionPlayerTick => {
                if let Some(p) = &mut self.session_player
                    && p.playing
                {
                    let now = std::time::Instant::now();
                    let dt_ms = p
                        .last_tick
                        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
                        .unwrap_or(0.0)
                        .min(MAX_TICK_MS);
                    p.last_tick = Some(now);
                    p.advance(dt_ms);
                }
            }
            m => return Err(m),
        }
        Ok(Task::none())
    }

    /// Keyboard layer for the player surface: while it is up on the
    /// History view (and no modal sits over it) the transport keys
    /// belong to the player: Space play/pause, Left/Right seek,
    /// Home restart, Esc close. Everything else falls through so app
    /// hotkeys keep working. This is the surface's keyboard-operability
    /// wiring: a media scrubber, like the terminal canvas, is driven by
    /// dedicated transport keys rather than `RowAction` slot walking.
    pub(crate) fn handle_player_key(
        &mut self,
        event: &keyboard::Event,
    ) -> Option<Task<Message>> {
        let player = self.session_player.as_ref()?;
        if self.active_view != crate::state::View::History
            || self.any_modal_blocks_input()
        {
            return None;
        }
        let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
            return None;
        };
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        let clock = player.clock_ms;
        let msg = match key {
            keyboard::Key::Named(keyboard::key::Named::Space) => {
                Message::SessionPlayerTogglePlay
            }
            keyboard::Key::Character(c) if c.as_str() == " " => {
                Message::SessionPlayerTogglePlay
            }
            keyboard::Key::Named(keyboard::key::Named::Escape) => {
                Message::SessionPlayerClose
            }
            keyboard::Key::Named(keyboard::key::Named::ArrowLeft) => {
                Message::SessionPlayerSeek(clock - SEEK_STEP_MS)
            }
            keyboard::Key::Named(keyboard::key::Named::ArrowRight) => {
                Message::SessionPlayerSeek(clock + SEEK_STEP_MS)
            }
            keyboard::Key::Named(keyboard::key::Named::Home) => {
                Message::SessionPlayerRestart
            }
            _ => return None,
        };
        Some(self.update(msg))
    }
}
