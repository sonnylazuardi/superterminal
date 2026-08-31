//! Frame instrumentation, read back through `get_prop("stats")`.
//!
//! **04-OQ10 is decided here: `get_prop`, not a `tracing` subscriber.**
//! HANDOVER §8 requires the choice to be fixed before M2 so shaping-cache
//! numbers are comparable across runs. A `get_prop` read is pull-based, costs
//! nothing when nobody asks, and is the same number the React perf harness and
//! a Bun integration test see; a log file would have to be parsed, would drift
//! with the log format, and would not be readable from the harness that is
//! driving the frames. The counters below are therefore the *only* sanctioned
//! source of M2 frame numbers.
//!
//! Per grilling Q52 nothing measured on the WSL2 dev box (wgpu → GL over the
//! WSLg D3D12 adapter) may decide the M2 gate; these numbers are for
//! comparing one build against another on the same host.

use std::time::Duration;

/// How many frames the percentile window keeps: two seconds at 120 Hz.
const WINDOW: usize = 240;

/// Per-frame counters for one `<terminal-grid>`.
#[derive(Debug, Clone)]
pub struct FrameStats {
    frames: u64,
    window: Vec<u32>,
    next: usize,
    last_us: u32,
    /// Runs shaped from scratch in the last frame.
    pub shaped_runs: u32,
    /// Runs served from the cache in the last frame.
    pub cached_runs: u32,
    /// Background quads painted in the last frame, after merging.
    pub bg_quads: u32,
    /// Rows painted in the last frame.
    pub rows: u16,
    /// Columns painted in the last frame.
    pub cols: u16,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            frames: 0,
            window: Vec::with_capacity(WINDOW),
            next: 0,
            last_us: 0,
            shaped_runs: 0,
            cached_runs: 0,
            bg_quads: 0,
            rows: 0,
            cols: 0,
        }
    }
}

impl FrameStats {
    /// Clears the per-frame counters at the top of a frame. The rolling
    /// window and the frame count survive.
    pub fn begin_frame(&mut self) {
        self.shaped_runs = 0;
        self.cached_runs = 0;
        self.bg_quads = 0;
    }

    /// Records how long the frame took.
    pub fn end_frame(&mut self, elapsed: Duration) {
        let micros = u32::try_from(elapsed.as_micros()).unwrap_or(u32::MAX);
        self.frames += 1;
        self.last_us = micros;
        if self.window.len() < WINDOW {
            self.window.push(micros);
        } else {
            self.window[self.next] = micros;
            self.next = (self.next + 1) % WINDOW;
        }
    }

    /// Total frames painted since the element was created.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The most recent frame, in milliseconds.
    #[must_use]
    pub fn last_ms(&self) -> f64 {
        f64::from(self.last_us) / 1000.0
    }

    /// A percentile of the rolling window, in milliseconds. `q` is `0.0..=1.0`.
    #[must_use]
    pub fn percentile_ms(&self, q: f64) -> f64 {
        if self.window.is_empty() {
            return 0.0;
        }
        let mut sorted = self.window.clone();
        sorted.sort_unstable();
        let index = ((sorted.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
        f64::from(sorted[index]) / 1000.0
    }

    /// Mean of the rolling window, in milliseconds.
    #[must_use]
    pub fn mean_ms(&self) -> f64 {
        if self.window.is_empty() {
            return 0.0;
        }
        let total: u64 = self.window.iter().map(|&v| u64::from(v)).sum();
        total as f64 / self.window.len() as f64 / 1000.0
    }

    /// Frames held in the percentile window.
    #[must_use]
    pub fn window_len(&self) -> usize {
        self.window.len()
    }

    /// Drops the window and the counters, so a benchmark can measure one
    /// scenario without the warm-up frames in it.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats_of(samples: &[u64]) -> FrameStats {
        let mut stats = FrameStats::default();
        for &micros in samples {
            stats.begin_frame();
            stats.end_frame(Duration::from_micros(micros));
        }
        stats
    }

    #[test]
    fn an_empty_window_reports_zeroes_rather_than_nan() {
        let stats = FrameStats::default();
        assert_eq!(stats.percentile_ms(0.95), 0.0);
        assert_eq!(stats.mean_ms(), 0.0);
        assert_eq!(stats.last_ms(), 0.0);
        assert_eq!(stats.frames(), 0);
    }

    #[test]
    fn percentiles_come_out_in_milliseconds() {
        let stats = stats_of(&(1..=100).map(|i| i * 1000).collect::<Vec<_>>());
        assert!((stats.percentile_ms(0.5) - 50.5).abs() <= 0.5, "{stats:?}");
        assert!((stats.percentile_ms(0.95) - 95.0).abs() <= 0.5);
        assert!((stats.percentile_ms(1.0) - 100.0).abs() < f64::EPSILON);
        assert!((stats.mean_ms() - 50.5).abs() < 0.01);
        assert_eq!(stats.frames(), 100);
        assert!((stats.last_ms() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_window_is_bounded_but_the_frame_count_is_not() {
        let stats = stats_of(&vec![1000; WINDOW * 3]);
        assert_eq!(stats.window_len(), WINDOW);
        assert_eq!(stats.frames(), (WINDOW * 3) as u64);
    }

    #[test]
    fn the_window_forgets_old_frames() {
        let mut stats = stats_of(&vec![50_000; WINDOW]);
        assert!((stats.percentile_ms(0.5) - 50.0).abs() < f64::EPSILON);
        for _ in 0..WINDOW {
            stats.begin_frame();
            stats.end_frame(Duration::from_micros(1000));
        }
        assert!((stats.percentile_ms(0.5) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn per_frame_counters_are_cleared_but_totals_are_not() {
        let mut stats = FrameStats::default();
        stats.begin_frame();
        stats.shaped_runs = 12;
        stats.end_frame(Duration::from_micros(1));
        stats.begin_frame();
        assert_eq!(stats.shaped_runs, 0);
        assert_eq!(stats.frames(), 1);
    }

    #[test]
    fn a_reset_clears_everything() {
        let mut stats = stats_of(&[1000, 2000]);
        stats.reset();
        assert_eq!(stats.frames(), 0);
        assert_eq!(stats.window_len(), 0);
    }
}
