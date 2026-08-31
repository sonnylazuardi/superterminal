//! Imperative reads (04 §3, "Imperative reads (get_prop)").
//!
//! gpuix has no `get_prop` on `CustomElement` and no napi accessor for one —
//! 04 §3 assumed our patch would add both, but the patch budget is spent
//! (invariant I5 caps it at ~40 lines) and neither is needed: a process-global
//! table of published element state plus one `#[napi]` function on our own
//! cdylib gives React and the Bun tests exactly the same reads, without
//! touching `vendor/gpuix`.
//!
//! Elements publish into this table at the end of every frame and on every
//! prop change; `st_read_prop(surfaceId, key)` reads it from the JS thread.
//! Keyed on Surface rather than element id because a Surface id is what React
//! already holds — an element id is an internal `@gpuix/react` counter.

use std::collections::HashMap;
use std::sync::Mutex;

use st_client_core::{DataPlaneHandle, Selection, SelectionConfig};
use st_proto::{Modes, SurfaceId};

use crate::stats::FrameStats;

/// The stats half of a published snapshot, flattened so the reader does not
/// need the `FrameStats` window.
#[derive(Debug, Clone, Default)]
pub struct StatsSnapshot {
    /// Frames painted since the element was created.
    pub frames: u64,
    /// Most recent frame, in ms.
    pub last_ms: f64,
    /// Mean of the rolling window, in ms.
    pub mean_ms: f64,
    /// p95 of the rolling window, in ms.
    pub p95_ms: f64,
    /// Runs shaped from scratch in the last frame.
    pub shaped_runs: u32,
    /// Runs served from the cache in the last frame.
    pub cached_runs: u32,
    /// Background quads in the last frame.
    pub bg_quads: u32,
    /// Lifetime shaped-run cache hits.
    pub cache_hits: u64,
    /// Lifetime shaped-run cache misses.
    pub cache_misses: u64,
    /// Lifetime hit rate, `0.0..=1.0`.
    pub cache_hit_rate: f64,
    /// Entries currently in the shaped-run cache.
    pub cache_len: usize,
}

impl StatsSnapshot {
    /// Flattens the live counters.
    #[must_use]
    pub fn of(stats: &FrameStats, cache_hits: u64, cache_misses: u64, cache_len: usize) -> Self {
        let total = cache_hits + cache_misses;
        Self {
            frames: stats.frames(),
            last_ms: stats.last_ms(),
            mean_ms: stats.mean_ms(),
            p95_ms: stats.percentile_ms(0.95),
            shaped_runs: stats.shaped_runs,
            cached_runs: stats.cached_runs,
            bg_quads: stats.bg_quads,
            cache_hits,
            cache_misses,
            cache_hit_rate: if total == 0 {
                0.0
            } else {
                cache_hits as f64 / total as f64
            },
            cache_len,
        }
    }

    /// The JSON shape `get_prop("stats")` returns.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "frames": self.frames,
            "lastFrameMs": round3(self.last_ms),
            "meanFrameMs": round3(self.mean_ms),
            "p95FrameMs": round3(self.p95_ms),
            "shapedRuns": self.shaped_runs,
            "cachedRuns": self.cached_runs,
            "bgQuads": self.bg_quads,
            "runCacheHits": self.cache_hits,
            "runCacheMisses": self.cache_misses,
            "runCacheHitRate": round3(self.cache_hit_rate * 1000.0) / 1000.0,
            "runCacheLen": self.cache_len,
        })
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Everything one `<terminal-grid>` publishes for reading back.
#[derive(Clone)]
pub struct GridSnapshot {
    /// `@gpuix/react`'s element id.
    pub element_id: u64,
    /// The attached Surface, if any.
    pub surface: Option<SurfaceId>,
    /// The Data Plane handle, so `selectionText` can be computed on demand
    /// instead of on every frame.
    pub handle: Option<DataPlaneHandle>,
    /// Current selection.
    pub selection: Option<Selection>,
    /// Word characters and trimming, for text extraction.
    pub selection_config: SelectionConfig,
    /// Distance from the bottom, in lines.
    pub scroll_offset: u64,
    /// History plus the visible grid.
    pub content_lines: u64,
    /// Absolute id of the topmost painted line.
    pub viewport_top: u64,
    /// Surface title.
    pub title: String,
    /// Grid width in cells.
    pub cols: u16,
    /// Grid height in cells.
    pub rows: u16,
    /// Cell advance width in px.
    pub cell_width: f32,
    /// Cell height in px.
    pub cell_height: f32,
    /// Socket is up.
    pub connected: bool,
    /// `Attach` has been sent for `surface`.
    pub attached: bool,
    /// Terminal modes, so a test can assert alt-screen or bracketed paste.
    pub modes: Modes,
    /// Frame counters.
    pub stats: StatsSnapshot,
}

impl Default for GridSnapshot {
    fn default() -> Self {
        Self {
            element_id: 0,
            surface: None,
            handle: None,
            selection: None,
            selection_config: SelectionConfig::default(),
            scroll_offset: 0,
            content_lines: 0,
            viewport_top: 0,
            title: String::new(),
            cols: 0,
            rows: 0,
            cell_width: 0.0,
            cell_height: 0.0,
            connected: false,
            attached: false,
            modes: Modes::empty(),
            stats: StatsSnapshot::default(),
        }
    }
}

impl GridSnapshot {
    /// The text the current selection covers, extracted from the Replica on
    /// demand. Empty when there is no selection.
    #[must_use]
    pub fn selection_text(&self) -> String {
        let (Some(handle), Some(surface), Some(selection)) =
            (&self.handle, self.surface, &self.selection)
        else {
            return String::new();
        };
        handle
            .with_replica(surface, |replica| {
                selection.text(replica, &self.selection_config)
            })
            .unwrap_or_default()
    }

    /// Answers one `get_prop` key. `None` for an unknown key, so the JS side
    /// can tell "not supported" from "empty".
    #[must_use]
    pub fn read(&self, key: &str) -> Option<serde_json::Value> {
        use serde_json::json;
        Some(match key {
            "scrollOffset" => json!(self.scroll_offset),
            "contentLines" => json!(self.content_lines),
            "viewportTop" => json!(self.viewport_top),
            "title" => json!(self.title),
            "selectionText" => json!(self.selection_text()),
            "hasSelection" => json!(self
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.is_empty())),
            "cellSize" => json!({ "w": self.cell_width, "h": self.cell_height }),
            "size" => json!({ "cols": self.cols, "rows": self.rows }),
            "connected" => json!(self.connected),
            "attached" => json!(self.attached),
            "elementId" => json!(self.element_id),
            "modes" => json!({
                "altScreen": self.modes.contains(Modes::ALT_SCREEN),
                "bracketedPaste": self.modes.contains(Modes::BRACKETED_PASTE),
                "mouse": self.modes.mouse_reporting(),
                "appCursorKeys": self.modes.contains(Modes::APP_CURSOR_KEYS),
            }),
            "stats" => self.stats.to_json(),
            "viewState" => {
                let last = self
                    .surface
                    .and_then(|surface| crate::viewstate::recorder().last_for(surface));
                match last {
                    Some(message) => json!({
                        "scrollOffset": message.scroll_offset.map(st_proto::AbsLine::get),
                        "hasSelection": message.selection.is_some(),
                    }),
                    None => serde_json::Value::Null,
                }
            }
            _ => return None,
        })
    }
}

/// Every key [`GridSnapshot::read`] answers, for documentation and for the
/// `stListProps` napi helper.
pub const READABLE_PROPS: &[&str] = &[
    "scrollOffset",
    "contentLines",
    "viewportTop",
    "title",
    "selectionText",
    "hasSelection",
    "cellSize",
    "size",
    "connected",
    "attached",
    "elementId",
    "modes",
    "stats",
    "viewState",
];

static GRIDS: Mutex<Option<HashMap<u64, GridSnapshot>>> = Mutex::new(None);

fn with_grids<R>(f: impl FnOnce(&mut HashMap<u64, GridSnapshot>) -> R) -> R {
    let mut guard = GRIDS.lock().unwrap_or_else(|error| error.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// Publishes (or replaces) one element's snapshot.
pub fn publish(snapshot: GridSnapshot) {
    with_grids(|grids| {
        grids.insert(snapshot.element_id, snapshot);
    });
}

/// Removes an element's snapshot on `destroy()`.
pub fn retire(element_id: u64) {
    with_grids(|grids| {
        grids.remove(&element_id);
    });
}

/// The snapshot for a Surface. When several elements claim the same Surface —
/// a remount mid-frame — the most recently painted one wins.
#[must_use]
pub fn snapshot_for_surface(surface: SurfaceId) -> Option<GridSnapshot> {
    with_grids(|grids| {
        grids
            .values()
            .filter(|snapshot| snapshot.surface == Some(surface))
            .max_by_key(|snapshot| snapshot.stats.frames)
            .cloned()
    })
}

/// Every published Surface id, for `stListGrids`.
#[must_use]
pub fn published_surfaces() -> Vec<u32> {
    with_grids(|grids| {
        let mut ids: Vec<u32> = grids
            .values()
            .filter_map(|snapshot| snapshot.surface)
            .map(|surface| surface.0)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    })
}

/// Drops every snapshot. Only for tests and app teardown.
pub fn clear() {
    with_grids(HashMap::clear);
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_client_core::selection::{AbsPoint, SelectionMode};
    use st_proto::AbsLine;

    fn snapshot(element_id: u64, surface: u32) -> GridSnapshot {
        GridSnapshot {
            element_id,
            surface: Some(SurfaceId(surface)),
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 17.0,
            content_lines: 1024,
            scroll_offset: 12,
            title: "zsh".to_string(),
            ..GridSnapshot::default()
        }
    }

    #[test]
    fn every_documented_key_answers() {
        let snapshot = snapshot(1, 1);
        for key in READABLE_PROPS {
            assert!(snapshot.read(key).is_some(), "{key} returned None");
        }
        assert!(snapshot.read("nonsense").is_none());
    }

    #[test]
    fn the_documented_keys_have_the_documented_shapes() {
        let snapshot = snapshot(1, 1);
        assert_eq!(snapshot.read("scrollOffset").unwrap(), 12);
        assert_eq!(snapshot.read("contentLines").unwrap(), 1024);
        assert_eq!(snapshot.read("title").unwrap(), "zsh");
        assert_eq!(snapshot.read("selectionText").unwrap(), "");
        let cell = snapshot.read("cellSize").unwrap();
        assert_eq!(cell["w"], 8.0);
        assert_eq!(cell["h"], 17.0);
        let size = snapshot.read("size").unwrap();
        assert_eq!(size["cols"], 80);
        assert_eq!(size["rows"], 24);
    }

    #[test]
    fn stats_json_carries_the_numbers_m2_compares() {
        let mut stats = FrameStats::default();
        stats.begin_frame();
        stats.shaped_runs = 7;
        stats.end_frame(std::time::Duration::from_micros(4200));
        let snapshot = GridSnapshot {
            stats: StatsSnapshot::of(&stats, 30, 10, 25),
            ..snapshot(1, 1)
        };
        let json = snapshot.read("stats").unwrap();
        assert_eq!(json["frames"], 1);
        assert!((json["lastFrameMs"].as_f64().unwrap() - 4.2).abs() < 0.001);
        assert_eq!(json["shapedRuns"], 7);
        assert_eq!(json["runCacheHits"], 30);
        assert_eq!(json["runCacheMisses"], 10);
        assert!((json["runCacheHitRate"].as_f64().unwrap() - 0.75).abs() < 0.001);
        assert_eq!(json["runCacheLen"], 25);
    }

    #[test]
    fn has_selection_is_false_for_a_bare_caret() {
        let empty = Selection::new(
            AbsPoint {
                line: AbsLine::new(3),
                col: 1,
            },
            SelectionMode::Char,
        );
        let snapshot = GridSnapshot {
            selection: Some(empty),
            ..snapshot(1, 1)
        };
        assert_eq!(snapshot.read("hasSelection").unwrap(), false);
    }

    #[test]
    fn selection_text_without_a_connection_is_empty_not_a_panic() {
        let mut selection = Selection::new(
            AbsPoint {
                line: AbsLine::new(0),
                col: 0,
            },
            SelectionMode::Char,
        );
        selection.extend_to(AbsPoint {
            line: AbsLine::new(0),
            col: 5,
        });
        let snapshot = GridSnapshot {
            selection: Some(selection),
            ..snapshot(1, 1)
        };
        assert_eq!(snapshot.selection_text(), "");
    }

    #[test]
    fn publishing_and_retiring_round_trips_through_the_global_table() {
        // The table is process-global and `cargo test` is parallel, so this
        // asserts on *its own* surface rather than on the whole table.
        publish(snapshot(9911, 7777));
        assert_eq!(
            snapshot_for_surface(SurfaceId(7777)).unwrap().element_id,
            9911
        );
        assert!(published_surfaces().contains(&7777));
        retire(9911);
        assert!(snapshot_for_surface(SurfaceId(7777)).is_none());
        assert!(!published_surfaces().contains(&7777));
    }

    #[test]
    fn the_most_recently_painted_element_wins_a_duplicate_surface() {
        let mut old = snapshot(8801, 5555);
        old.stats.frames = 3;
        let mut new = snapshot(8802, 5555);
        new.stats.frames = 90;
        publish(old);
        publish(new);
        assert_eq!(
            snapshot_for_surface(SurfaceId(5555)).unwrap().element_id,
            8802
        );
        retire(8801);
        retire(8802);
    }
}
