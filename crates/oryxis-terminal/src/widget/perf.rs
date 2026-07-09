/// Rolling per-frame samples for the perf overlay. We track the
/// **max** of each phase over a short window so transient spikes
/// (the kind that actually feel like lag) stay visible for a beat
/// instead of being averaged away, plus window averages, the busy
/// fraction and the over-budget frame count. No fps figures: this is
/// an on-demand renderer, so redraw cadence tracks user/host activity
/// rather than rendering speed; frame TIME against the 60 Hz budget is
/// the metric that actually answers "is it slow" (issue #69).
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

/// Frames retained for the rolling window. ~2s of activity at 60 fps;
/// long enough to catch a typing burst, short enough that the HUD
/// recovers when things calm down.
pub(crate) const PERF_WINDOW: usize = 120;

/// The 60 Hz frame budget. A draw that costs more than this cannot
/// keep up with a standard display; the HUD flags those frames.
pub(crate) const FRAME_BUDGET: std::time::Duration = std::time::Duration::from_micros(16_667);

/// Half the budget: the "getting expensive" warning threshold.
pub(crate) const FRAME_WARN: std::time::Duration = std::time::Duration::from_micros(8_333);

/// Gaps longer than this are idle pauses, not slow frames: the canvas
/// redraws on demand, so a quiet second between two keystrokes is not
/// render time. Capped out of the busy-fraction denominator.
const IDLE_GAP: std::time::Duration = std::time::Duration::from_secs(1);

impl PerfStats {
    /// Fraction (0-100) of recent active wall-clock spent inside draw.
    /// Denominator caps each inter-frame gap at `IDLE_GAP` so idle time
    /// doesn't dilute it to zero; clamped because sibling panes in one
    /// redraw cycle contribute draw time with a near-zero gap.
    pub(crate) fn busy_pct(&self) -> f32 {
        let drawing: std::time::Duration = self.samples.iter().map(|s| s.total).sum();
        let active: std::time::Duration =
            self.samples.iter().map(|s| s.frame_gap.min(IDLE_GAP)).sum();
        if active.is_zero() {
            return 0.0;
        }
        (100.0 * drawing.as_secs_f32() / active.as_secs_f32()).min(100.0)
    }

    /// Frames in the window whose total draw cost blew the 60 Hz
    /// budget, the classic "dropped frames" figure.
    pub(crate) fn over_budget(&self) -> usize {
        self.samples
            .iter()
            .filter(|s| s.total > FRAME_BUDGET)
            .count()
    }

    /// Per-sample total draw cost in ms, oldest first, for the
    /// frame-time sparkline.
    pub(crate) fn total_series(&self) -> impl Iterator<Item = f32> + '_ {
        self.samples.iter().map(|s| s.total.as_secs_f32() * 1000.0)
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
/// (or any non-empty value) to render a small frame-timing HUD in the
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

    fn sample(gap_ms: u64, total_ms: u64, built: bool) -> PerfSample {
        PerfSample {
            frame_gap: std::time::Duration::from_millis(gap_ms),
            lock: std::time::Duration::from_micros(100),
            cells: std::time::Duration::from_micros(200),
            highlights: std::time::Duration::from_micros(50),
            total: std::time::Duration::from_millis(total_ms),
            built,
        }
    }

    fn stats(samples: &[PerfSample]) -> PerfStats {
        PerfStats {
            samples: samples.iter().copied().collect(),
            last_draw_at: None,
        }
    }

    #[test]
    fn busy_pct_is_draw_time_over_active_time() {
        // 4 frames, 16 ms apart, each costing 4 ms to draw: 25% busy.
        let s = stats(&[sample(16, 4, true); 4]);
        assert!((s.busy_pct() - 25.0).abs() < 0.1, "busy={}", s.busy_pct());
    }

    #[test]
    fn busy_pct_caps_idle_gaps_and_clamps() {
        // A 10 s idle gap counts as only 1 s of active time, so one
        // 5 ms draw after it reads ~0.5% busy, not ~0.05%.
        let s = stats(&[sample(10_000, 5, true)]);
        assert!((s.busy_pct() - 0.5).abs() < 0.05, "busy={}", s.busy_pct());
        // Same-cycle gaps (~0) with real draw cost can't exceed 100%.
        let s = stats(&[sample(0, 5, true); 3]);
        assert!(s.busy_pct() <= 100.0);
    }

    #[test]
    fn over_budget_counts_frames_past_16ms() {
        let s = stats(&[
            sample(16, 2, true),
            sample(16, 17, true),
            sample(16, 30, true),
        ]);
        assert_eq!(s.over_budget(), 2);
    }

    #[test]
    fn cache_hit_rate_counts_unbuilt_frames() {
        let s = stats(&[
            sample(16, 1, true),
            sample(16, 0, false),
            sample(16, 0, false),
            sample(16, 0, false),
        ]);
        assert!((s.cache_hit_pct() - 75.0).abs() < 0.01);
        assert_eq!(stats(&[]).cache_hit_pct(), 0.0);
    }
}
