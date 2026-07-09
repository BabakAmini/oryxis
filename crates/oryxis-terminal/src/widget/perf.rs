/// Rolling per-frame samples for the perf overlay. We track the
/// **max** of each phase over a short window so transient spikes
/// (the kind that actually feel like lag) stay visible for a beat
/// instead of being averaged away, plus window averages and a
/// current/average/peak fps triple (issue #69).
pub(crate) struct PerfStats {
    /// Last few frames of each phase. Old entries are dropped after
    /// `WINDOW` so the max reflects recent activity, not the whole
    /// session.
    pub(crate) samples: std::collections::VecDeque<PerfSample>,
    /// Wall-clock of the previous draw, used so the overlay can
    /// avoid double-counting frames within a single redraw cycle.
    pub(crate) last_draw_at: Option<std::time::Instant>,
}

#[derive(Clone, Copy)]
pub(crate) struct PerfSample {
    pub(crate) frame_gap: std::time::Duration,
    pub(crate) lock: std::time::Duration,
    pub(crate) cells: std::time::Duration,
    pub(crate) highlights: std::time::Duration,
    pub(crate) total: std::time::Duration,
    /// Whether this frame actually rebuilt the grid geometry (a cache
    /// miss). Drives the HUD's rolling cache hit-rate.
    pub(crate) built: bool,
}

/// Frames retained for the rolling max / fps. ~2s of activity at
/// 60 fps; long enough to catch a typing burst, short enough that
/// the HUD recovers when things calm down.
pub(crate) const PERF_WINDOW: usize = 120;

/// Frames feeding the "curr" fps readout. Short enough to track what
/// is happening right now, long enough not to flicker per frame.
const CURRENT_FPS_WINDOW: usize = 10;

/// Gaps longer than this are idle pauses, not slow frames: the canvas
/// redraws on demand, so a quiet second between two keystrokes must
/// not read as "1 fps". Excluded from every fps figure.
const IDLE_GAP: std::time::Duration = std::time::Duration::from_secs(1);

/// Gaps shorter than this are two panes drawing inside the same redraw
/// cycle (the stats are process-global), not two frames. Excluded from
/// the fps figures so a split layout doesn't double them; the phase
/// timings of such samples still count, they measure real work.
const SAME_CYCLE_GAP: std::time::Duration = std::time::Duration::from_millis(1);

impl PerfSample {
    /// Instantaneous fps of this frame, `None` when the gap doesn't
    /// represent a real frame-to-frame interval (first frame, idle
    /// pause, same-cycle sibling pane).
    fn fps(&self) -> Option<f32> {
        (self.frame_gap >= SAME_CYCLE_GAP && self.frame_gap <= IDLE_GAP)
            .then(|| 1.0 / self.frame_gap.as_secs_f32())
    }
}

impl PerfStats {
    /// Mean fps over an iterator of frame-gap samples, honoring the
    /// idle / same-cycle exclusions.
    fn mean_fps<'a>(samples: impl Iterator<Item = &'a PerfSample>) -> f32 {
        let mut n = 0u32;
        let mut total = std::time::Duration::ZERO;
        for s in samples.filter(|s| s.fps().is_some()) {
            n += 1;
            total += s.frame_gap;
        }
        if n == 0 || total.is_zero() {
            0.0
        } else {
            n as f32 / total.as_secs_f32()
        }
    }

    /// Fps over the last few frames only, the "what is happening right
    /// now" readout matching the current-frame phase timings.
    pub(crate) fn current_fps(&self) -> f32 {
        let skip = self.samples.len().saturating_sub(CURRENT_FPS_WINDOW);
        Self::mean_fps(self.samples.iter().skip(skip))
    }

    /// Fps averaged over the whole rolling window.
    pub(crate) fn avg_fps(&self) -> f32 {
        Self::mean_fps(self.samples.iter())
    }

    /// Highest instantaneous fps seen in the window.
    pub(crate) fn peak_fps(&self) -> f32 {
        self.samples
            .iter()
            .filter_map(PerfSample::fps)
            .fold(0.0, f32::max)
    }

    /// Per-sample instantaneous fps, oldest first, for the sparkline.
    /// Excluded gaps (idle / same-cycle / first frame) draw as zero.
    pub(crate) fn fps_series(&self) -> impl Iterator<Item = f32> + '_ {
        self.samples.iter().map(|s| s.fps().unwrap_or(0.0))
    }

    /// Percentage of frames in the window served from the geometry
    /// cache (no snapshot + glyph rebuild).
    pub(crate) fn cache_hit_pct(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let hits = self.samples.iter().filter(|s| !s.built).count();
        100.0 * hits as f32 / self.samples.len() as f32
    }

    fn mean(&self, f: impl Fn(&PerfSample) -> std::time::Duration) -> std::time::Duration {
        if self.samples.is_empty() {
            return std::time::Duration::ZERO;
        }
        self.samples.iter().map(f).sum::<std::time::Duration>() / self.samples.len() as u32
    }

    fn max(&self, f: impl Fn(&PerfSample) -> std::time::Duration) -> std::time::Duration {
        self.samples.iter().map(f).max().unwrap_or_default()
    }

    pub(crate) fn avg_lock(&self) -> std::time::Duration {
        self.mean(|s| s.lock)
    }
    pub(crate) fn avg_cells(&self) -> std::time::Duration {
        self.mean(|s| s.cells)
    }
    pub(crate) fn avg_highlights(&self) -> std::time::Duration {
        self.mean(|s| s.highlights)
    }
    pub(crate) fn avg_total(&self) -> std::time::Duration {
        self.mean(|s| s.total)
    }

    pub(crate) fn max_lock(&self) -> std::time::Duration {
        self.max(|s| s.lock)
    }
    pub(crate) fn max_cells(&self) -> std::time::Duration {
        self.max(|s| s.cells)
    }
    pub(crate) fn max_highlights(&self) -> std::time::Duration {
        self.max(|s| s.highlights)
    }
    pub(crate) fn max_total(&self) -> std::time::Duration {
        self.max(|s| s.total)
    }
}

pub(crate) fn perf_stats() -> &'static std::sync::Mutex<PerfStats> {
    static STATS: std::sync::OnceLock<std::sync::Mutex<PerfStats>> =
        std::sync::OnceLock::new();
    STATS.get_or_init(|| {
        std::sync::Mutex::new(PerfStats {
            samples: std::collections::VecDeque::with_capacity(PERF_WINDOW),
            last_draw_at: None,
        })
    })
}

/// Reads the `ORYXIS_TERM_PERF` env var once and caches it. Set to `1`
/// (or any non-empty value) to render a small FPS/timing HUD in the
/// top-right of every terminal canvas.
pub(crate) fn perf_overlay_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("ORYXIS_TERM_PERF")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

/// Whether the HUD renders full-name metric keys ("total 0.3ms")
/// instead of the compact single letters. Toggled by clicking the HUD
/// panel, the canvas overlay's stand-in for tooltips (issue #69).
/// Process-global on purpose: it's a display preference of one debug
/// HUD, not per-pane state, and it matches `perf_stats` being global.
static HUD_WIDE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn hud_wide() -> bool {
    HUD_WIDE.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn toggle_hud_wide() {
    HUD_WIDE.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(gap: std::time::Duration, built: bool) -> PerfSample {
        PerfSample {
            frame_gap: gap,
            lock: std::time::Duration::from_micros(100),
            cells: std::time::Duration::from_micros(200),
            highlights: std::time::Duration::from_micros(50),
            total: std::time::Duration::from_micros(400),
            built,
        }
    }

    fn stats(gaps_ms: &[u64]) -> PerfStats {
        PerfStats {
            samples: gaps_ms
                .iter()
                .map(|ms| sample(std::time::Duration::from_millis(*ms), true))
                .collect(),
            last_draw_at: None,
        }
    }

    #[test]
    fn current_tracks_recent_frames_average_tracks_window() {
        // 20 slow frames (50 ms = 20 fps) followed by 10 fast ones
        // (10 ms = 100 fps): "curr" reads the recent burst, "avg" the
        // whole window, "peak" the fastest single frame.
        let mut gaps = vec![50u64; 20];
        gaps.extend(vec![10u64; 10]);
        let s = stats(&gaps);
        assert!((s.current_fps() - 100.0).abs() < 1.0, "curr={}", s.current_fps());
        let avg = s.avg_fps();
        assert!(avg > 20.0 && avg < 100.0, "avg={avg}");
        assert!((s.peak_fps() - 100.0).abs() < 1.0, "peak={}", s.peak_fps());
    }

    #[test]
    fn idle_and_same_cycle_gaps_are_excluded_from_fps() {
        // A 5 s idle pause and a 0 ms same-cycle sibling draw must not
        // drag or inflate the figures; only the two real 20 ms frames
        // (50 fps) count.
        let s = stats(&[5000, 0, 20, 20]);
        assert!((s.avg_fps() - 50.0).abs() < 1.0, "avg={}", s.avg_fps());
        assert!((s.peak_fps() - 50.0).abs() < 1.0, "peak={}", s.peak_fps());
        // The excluded gaps still occupy sparkline slots, as zeros.
        let series: Vec<f32> = s.fps_series().collect();
        assert_eq!(series.len(), 4);
        assert_eq!(series[0], 0.0);
        assert_eq!(series[1], 0.0);
        assert!(series[2] > 0.0);
    }

    #[test]
    fn all_excluded_gaps_read_as_zero_fps() {
        let s = stats(&[0, 5000]);
        assert_eq!(s.current_fps(), 0.0);
        assert_eq!(s.avg_fps(), 0.0);
        assert_eq!(s.peak_fps(), 0.0);
    }

    #[test]
    fn cache_hit_rate_counts_unbuilt_frames() {
        let mut s = stats(&[]);
        for built in [true, false, false, false] {
            s.samples
                .push_back(sample(std::time::Duration::from_millis(16), built));
        }
        assert!((s.cache_hit_pct() - 75.0).abs() < 0.01);
        assert_eq!(stats(&[]).cache_hit_pct(), 0.0);
    }
}
