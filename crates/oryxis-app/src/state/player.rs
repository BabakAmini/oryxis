//! Session-recording player (issue #71): replays a recorded session's
//! chunks through the same alacritty backend the live terminal uses,
//! read-only by construction (no PTY, no input wiring). The state is a
//! playback clock over a preprocessed event timeline; the view renders
//! the backend with the regular terminal widget pinned to the
//! recording's geometry.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use oryxis_terminal::widget::TerminalState;
use oryxis_terminal::TerminalPalette;
use uuid::Uuid;

/// Replay step for rows recorded before the timing migration
/// (`offset_ms = NULL`). Mirrors the `.cast` export's fallback in
/// `dispatch_history.rs` so both replays pace legacy logs identically.
const LEGACY_DELTA_MS: i64 = 50;

/// Geometry for recordings that carry no resize row (legacy logs).
/// Same fallback the `.cast` export header uses.
const FALLBACK_GEOMETRY: (u16, u16) = (80, 24);

/// Playback speed steps the speed button cycles through.
pub(crate) const PLAYER_SPEEDS: [f32; 5] = [0.5, 1.0, 1.5, 2.0, 4.0];

/// One playable event on an absolute, non-decreasing timeline.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlayerEvent {
    /// Milliseconds since the start of the recording, clamped
    /// non-decreasing (same interleaving guard as the `.cast` export).
    pub at_ms: i64,
    pub kind: PlayerEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PlayerEventKind {
    /// Raw output bytes for the emulator.
    Output(Vec<u8>),
    /// Terminal resize to (cols, rows).
    Resize(u16, u16),
}

/// Convert the vault's timed rows into the player timeline: absolute
/// non-decreasing times, typed-command rows dropped (replay is
/// output-only, like the `.cast` export), malformed resize rows
/// skipped, legacy untimed rows paced with a fixed delta. Returns the
/// events plus the recording's duration and initial geometry.
pub(crate) fn preprocess_events(
    rows: &[oryxis_vault::SessionLogEvent],
) -> (Vec<PlayerEvent>, i64, (u16, u16)) {
    let mut events: Vec<PlayerEvent> = Vec::with_capacity(rows.len());
    let mut last_ms: i64 = 0;
    for row in rows {
        if row.kind == 'c' {
            continue;
        }
        let kind = if row.kind == 'r' {
            let s = String::from_utf8_lossy(&row.data);
            let parsed = s.split_once('x').and_then(|(c, r)| {
                Some((c.parse::<u16>().ok()?, r.parse::<u16>().ok()?))
            });
            match parsed {
                // The emulator rejects grids under 2x2; dropping the
                // row keeps the timeline usable instead of wedging the
                // canvas at a degenerate size.
                Some((c, r)) if c >= 2 && r >= 2 => PlayerEventKind::Resize(c, r),
                _ => continue,
            }
        } else {
            if row.data.is_empty() {
                continue;
            }
            PlayerEventKind::Output(row.data.clone())
        };
        let at_ms = match row.offset_ms {
            // Clamp against interleavings: a resize stamped at flush
            // time can sit a hair before chunk rows written in the
            // same batch (same rule as the `.cast` export).
            Some(ms) => ms.max(last_ms),
            None => last_ms + LEGACY_DELTA_MS,
        };
        last_ms = at_ms;
        events.push(PlayerEvent { at_ms, kind });
    }
    let duration_ms = events.last().map(|e| e.at_ms).unwrap_or(0);
    let geometry = events
        .iter()
        .find_map(|e| match e.kind {
            PlayerEventKind::Resize(c, r) => Some((c, r)),
            _ => None,
        })
        .unwrap_or(FALLBACK_GEOMETRY);
    (events, duration_ms, geometry)
}

/// GIF export machinery for session recordings (issue #71). A sibling
/// of [`SessionPlayer`] rather than a field on it: an export is
/// triggered from the History list without the player open, and a
/// pending export must survive the player closing while the `gif`
/// plugin installs / renders, so it cannot live inside
/// `Oryxis.session_player` (an `Option` that may be `None` the whole
/// time). One field on `Oryxis` (`self.gif_export`).
#[derive(Default)]
pub(crate) struct GifExportState {
    /// Recording waiting for the `gif` plugin install to finish; the
    /// `PluginInstallDone("gif", Ok)` handler resumes the export.
    pub pending: Option<Uuid>,
    /// One GIF render at a time: re-entry shows the "rendering" toast
    /// instead of racing two renders over the save dialog.
    pub running: bool,
}

/// The open player: one recording, one read-only terminal backend, a
/// scaled playback clock. Lives in `Oryxis.session_player` while the
/// player surface is up on the History screen.
pub(crate) struct SessionPlayer {
    /// The recording being played (used to close the player when its
    /// log is deleted underneath it).
    pub log_id: Uuid,
    /// Connection label of the recording, for the header.
    pub label: String,
    /// Preprocessed timeline (see [`preprocess_events`]).
    pub events: Vec<PlayerEvent>,
    /// Index of the first event not yet fed to the backend.
    pub next_event: usize,
    /// Playback position in milliseconds. `f64` so sub-tick speed
    /// scaling accumulates without drift.
    pub clock_ms: f64,
    /// Timeline length in milliseconds (last event's time).
    pub duration_ms: i64,
    pub playing: bool,
    /// While the user drags the scrubber, the pending target in
    /// milliseconds. The knob and time label follow it live (O(1)), but
    /// the emulator is only rebuilt/replayed once, on release
    /// ([`commit_scrub`]). Without this a backward drag rebuilt and
    /// replayed the whole timeline on every per-millisecond slider
    /// event, freezing the UI on a long recording.
    pub scrub: Option<f64>,
    /// Clock multiplier, one of [`PLAYER_SPEEDS`].
    pub speed: f32,
    /// Wall-clock instant of the previous tick while playing; `None`
    /// while paused so resuming can't count the paused gap.
    pub last_tick: Option<Instant>,
    /// The replay emulator, PTY-less and never wired for input.
    /// `Arc<Mutex<..>>` because the terminal widget shares state with
    /// the app the same way the live panes do.
    pub terminal: Arc<Mutex<TerminalState>>,
    /// Current grid geometry (tracks fed resize events), used by the
    /// view to size the fixed-grid canvas.
    pub cols: u16,
    pub rows: u16,
    /// Geometry to rebuild with on a backward seek / restart.
    initial_geometry: (u16, u16),
    /// Palette applied to (re)built backends, resolved once at open
    /// like the live pane (per-host override, then global).
    palette: TerminalPalette,
}

impl SessionPlayer {
    /// Build a player over a preprocessed timeline. Fails only if the
    /// emulator can't be constructed.
    pub fn new(
        log_id: Uuid,
        label: String,
        events: Vec<PlayerEvent>,
        duration_ms: i64,
        geometry: (u16, u16),
        palette: TerminalPalette,
    ) -> oryxis_terminal::widget::TerminalResult<Self> {
        let terminal = Self::build_terminal(geometry, &palette)?;
        Ok(Self {
            log_id,
            label,
            events,
            next_event: 0,
            clock_ms: 0.0,
            duration_ms,
            playing: true,
            scrub: None,
            speed: 1.0,
            last_tick: None,
            terminal,
            cols: geometry.0,
            rows: geometry.1,
            initial_geometry: geometry,
            palette,
        })
    }

    fn build_terminal(
        geometry: (u16, u16),
        palette: &TerminalPalette,
    ) -> oryxis_terminal::widget::TerminalResult<Arc<Mutex<TerminalState>>> {
        let mut state = TerminalState::new_no_pty(geometry.0, geometry.1)?;
        state.palette = palette.clone();
        Ok(Arc::new(Mutex::new(state)))
    }

    /// Advance the clock by `dt_ms` of wall time (pre-clamped by the
    /// tick handler) scaled by the current speed, feed the events that
    /// became due, and pause at the end of the timeline.
    pub fn advance(&mut self, dt_ms: f64) {
        self.clock_ms =
            (self.clock_ms + dt_ms * f64::from(self.speed)).min(self.duration_ms as f64);
        self.feed_due();
        if self.finished() {
            self.playing = false;
            self.last_tick = None;
        }
    }

    /// Whether the whole timeline has been fed.
    pub fn finished(&self) -> bool {
        self.next_event >= self.events.len()
    }

    /// Feed every event at or before the current clock into the
    /// backend, in order.
    pub fn feed_due(&mut self) {
        let due = self.clock_ms.floor() as i64;
        let mut state = self
            .terminal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while let Some(ev) = self.events.get(self.next_event) {
            if ev.at_ms > due {
                break;
            }
            match &ev.kind {
                PlayerEventKind::Output(bytes) => state.process(bytes),
                PlayerEventKind::Resize(c, r) => {
                    state.resize(*c, *r);
                    self.cols = *c;
                    self.rows = *r;
                }
            }
            self.next_event += 1;
        }
    }

    /// Jump to `target_ms`. Forward seeks feed incrementally; backward
    /// seeks rebuild the emulator and replay from zero up to the
    /// target (`process()` is fast enough that keyframes aren't
    /// needed; see the issue #71 spec). A failed rebuild leaves the
    /// current frame in place rather than blanking the player.
    pub fn seek(&mut self, target_ms: f64) {
        // A committed seek supersedes any in-flight scrub preview.
        self.scrub = None;
        let target = target_ms.clamp(0.0, self.duration_ms as f64);
        if target < self.clock_ms {
            let Ok(fresh) = Self::build_terminal(self.initial_geometry, &self.palette) else {
                return;
            };
            self.terminal = fresh;
            self.next_event = 0;
            self.cols = self.initial_geometry.0;
            self.rows = self.initial_geometry.1;
        }
        self.clock_ms = target;
        self.feed_due();
        // Seeking away from the end revives the play button's meaning;
        // seeking onto the end pauses like natural completion.
        if self.finished() {
            self.playing = false;
            self.last_tick = None;
        }
    }

    /// The position the transport should display: the live scrub target
    /// while dragging, otherwise the playback clock.
    pub fn display_ms(&self) -> f64 {
        self.scrub.unwrap_or(self.clock_ms)
    }

    /// Record a scrubber drag without touching the emulator (cheap): the
    /// knob and label follow, the frame catches up on release.
    pub fn scrub_to(&mut self, target_ms: f64) {
        self.scrub = Some(target_ms.clamp(0.0, self.duration_ms as f64));
    }

    /// Apply the pending scrub target once, on release (a single
    /// rebuild/replay instead of one per drag event).
    pub fn commit_scrub(&mut self) {
        if let Some(target) = self.scrub.take() {
            self.seek(target);
        }
    }

    /// Restart from zero, playing.
    pub fn restart(&mut self) {
        self.scrub = None;
        self.seek(0.0);
        self.playing = true;
        self.last_tick = Some(Instant::now());
    }

    /// Toggle play/pause. Playing again after the timeline ended
    /// restarts from zero (the expected media-player affordance).
    pub fn toggle_play(&mut self) {
        if self.playing {
            self.playing = false;
            self.last_tick = None;
        } else if self.finished() {
            self.restart();
        } else {
            self.playing = true;
            self.last_tick = Some(Instant::now());
        }
    }

    /// Step to the next speed in [`PLAYER_SPEEDS`], wrapping.
    pub fn cycle_speed(&mut self) {
        let idx = PLAYER_SPEEDS
            .iter()
            .position(|s| (*s - self.speed).abs() < f32::EPSILON)
            .unwrap_or(1);
        self.speed = PLAYER_SPEEDS[(idx + 1) % PLAYER_SPEEDS.len()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oryxis_vault::SessionLogEvent;

    fn ev(offset_ms: Option<i64>, kind: char, data: &[u8]) -> SessionLogEvent {
        SessionLogEvent { offset_ms, kind, data: data.to_vec() }
    }

    #[test]
    fn preprocess_builds_a_non_decreasing_timeline() {
        let (events, duration, _) = preprocess_events(&[
            ev(Some(0), 'r', b"120x30"),
            ev(Some(100), 'o', b"hi"),
            // Stamped earlier than the previous event: clamps forward.
            ev(Some(40), 'o', b"there"),
        ]);
        let times: Vec<i64> = events.iter().map(|e| e.at_ms).collect();
        assert_eq!(times, vec![0, 100, 100]);
        assert_eq!(duration, 100);
    }

    #[test]
    fn preprocess_paces_untimed_rows_with_the_legacy_delta() {
        let (events, duration, geometry) = preprocess_events(&[
            ev(None, 'o', b"one"),
            ev(None, 'o', b"two"),
        ]);
        let times: Vec<i64> = events.iter().map(|e| e.at_ms).collect();
        assert_eq!(times, vec![LEGACY_DELTA_MS, LEGACY_DELTA_MS * 2]);
        assert_eq!(duration, LEGACY_DELTA_MS * 2);
        // No resize row anywhere: same 80x24 fallback as the export.
        assert_eq!(geometry, FALLBACK_GEOMETRY);
    }

    #[test]
    fn preprocess_drops_command_rows_and_malformed_resizes() {
        let (events, _, geometry) = preprocess_events(&[
            ev(Some(0), 'c', b"ls -la"),
            ev(Some(10), 'r', b"garbage"),
            ev(Some(20), 'r', b"1x1"),
            ev(Some(30), 'r', b"100x40"),
            ev(Some(40), 'o', b"total 0"),
        ]);
        assert_eq!(events.len(), 2, "only the valid resize and the output stay");
        assert_eq!(geometry, (100, 40));
        assert!(events.iter().all(|e| match &e.kind {
            PlayerEventKind::Output(d) => d == b"total 0",
            PlayerEventKind::Resize(c, r) => (*c, *r) == (100, 40),
        }));
    }

    fn player_over(rows: &[SessionLogEvent]) -> SessionPlayer {
        let (events, duration, geometry) = preprocess_events(rows);
        SessionPlayer::new(
            Uuid::nil(),
            "test".into(),
            events,
            duration,
            geometry,
            TerminalPalette::default(),
        )
        .expect("headless player")
    }

    fn cell(p: &SessionPlayer, row: i32, col: usize) -> char {
        use oryxis_terminal::alacritty_terminal::index::{Column, Line};
        let state = p.terminal.lock().unwrap();
        state.backend.term.grid()[Line(row)][Column(col)].c
    }

    #[test]
    fn advance_feeds_due_events_and_pauses_at_the_end() {
        let mut p = player_over(&[
            ev(Some(0), 'o', b"A"),
            ev(Some(1_000), 'o', b"B"),
        ]);
        p.feed_due();
        assert_eq!(cell(&p, 0, 0), 'A');
        assert_eq!(cell(&p, 0, 1), ' ', "future event must not be fed yet");

        // 300 ms of wall time at 2x = 600 ms of playback: still short.
        p.speed = 2.0;
        p.advance(300.0);
        assert_eq!(cell(&p, 0, 1), ' ');
        assert!(p.playing);

        // Another 300 ms lands on 1200 ms: the second event plays and
        // the clock clamps to the duration, pausing playback.
        p.advance(300.0);
        assert_eq!(cell(&p, 0, 1), 'B');
        assert!(!p.playing, "reaching the end pauses");
        assert_eq!(p.clock_ms, 1_000.0, "clock clamps to the duration");
    }

    #[test]
    fn backward_seek_rebuilds_and_replays_from_zero() {
        // Real recordings stamp their initial size at t=0 (first
        // flush); that first resize is the header geometry the player
        // (re)builds with, mirroring the `.cast` export.
        let mut p = player_over(&[
            ev(Some(0), 'r', b"120x30"),
            ev(Some(100), 'o', b"A"),
            ev(Some(500), 'r', b"100x40"),
            ev(Some(1_000), 'o', b"B"),
        ]);
        p.seek(1_000.0);
        assert_eq!((p.cols, p.rows), (100, 40));
        assert_eq!(cell(&p, 0, 1), 'B');

        // Back to 200 ms: fresh emulator, replayed through the first
        // two events only, geometry back to the recording's initial.
        p.seek(200.0);
        assert_eq!(cell(&p, 0, 0), 'A');
        assert_eq!(cell(&p, 0, 1), ' ');
        assert_eq!((p.cols, p.rows), (120, 30));
        assert_eq!(p.next_event, 2);
    }

    #[test]
    fn scrub_defers_the_rebuild_to_commit() {
        let mut p = player_over(&[
            ev(Some(0), 'o', b"A"),
            ev(Some(1_000), 'o', b"B"),
        ]);
        p.seek(1_000.0);
        assert_eq!(cell(&p, 0, 1), 'B');
        let events_at_end = p.next_event;

        // Dragging backward only moves the knob/label; the emulator is
        // untouched (no replay), so the frame still shows the end.
        p.scrub_to(100.0);
        assert_eq!(p.display_ms(), 100.0, "knob follows the scrub");
        assert_eq!(p.clock_ms, 1_000.0, "clock not moved yet");
        assert_eq!(p.next_event, events_at_end, "no rebuild during the drag");
        assert_eq!(cell(&p, 0, 1), 'B', "frame unchanged until release");

        // Release applies it once: clock jumps back and the frame
        // rebuilds to the earlier position.
        p.commit_scrub();
        assert_eq!(p.scrub, None);
        assert_eq!(p.clock_ms, 100.0);
        assert_eq!(p.display_ms(), 100.0);
        assert_eq!(cell(&p, 0, 1), ' ', "past the future event again");
    }

    #[test]
    fn toggle_play_after_the_end_restarts() {
        let mut p = player_over(&[ev(Some(0), 'o', b"A"), ev(Some(100), 'o', b"B")]);
        p.seek(100.0);
        assert!(p.finished());
        assert!(!p.playing);
        p.toggle_play();
        assert!(p.playing, "play at the end restarts");
        assert_eq!(p.clock_ms, 0.0);
        assert_eq!(p.next_event, 1, "the t=0 event is re-fed on restart");
    }

    #[test]
    fn cycle_speed_walks_the_steps_and_wraps() {
        let mut p = player_over(&[ev(Some(0), 'o', b"A")]);
        assert_eq!(p.speed, 1.0);
        p.cycle_speed();
        assert_eq!(p.speed, 1.5);
        p.speed = 4.0;
        p.cycle_speed();
        assert_eq!(p.speed, 0.5);
    }
}
