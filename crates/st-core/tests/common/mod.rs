//! Shared helpers for the `st-core` integration tests.

#![allow(dead_code)]

use st_core::{EngineConfig, Surface, SurfaceConfig};
use st_proto::{CellFlags, Row, Snapshot, Style, StyleIdx};

/// A Surface with no PTY, driven purely by bytes fed to the engine.
pub fn fixture(cols: u16, rows: u16) -> Surface {
    fixture_with_scrollback(cols, rows, 100)
}

/// A Surface with an explicit scrollback size, for eviction tests.
pub fn fixture_with_scrollback(cols: u16, rows: u16, scrollback: usize) -> Surface {
    Surface::new(SurfaceConfig {
        engine: EngineConfig {
            cols,
            rows,
            scrollback_lines: scrollback,
            default_title: "st".into(),
            ..EngineConfig::default()
        },
        pty: None,
        ..SurfaceConfig::default()
    })
    .expect("an engine-only Surface never opens a PTY")
}

/// The text of one row, right-trimmed.
pub fn row_text(row: &Row) -> String {
    let mut out = String::new();
    for cell in &row.cells {
        if cell.flags.contains(CellFlags::WIDE_SPACER) {
            // The trailing half of a wide glyph renders nothing.
            continue;
        }
        match row.grapheme(*cell) {
            Some(grapheme) => out.push_str(grapheme),
            None => match char::from_u32(cell.codepoint) {
                Some('\0') | None => out.push(' '),
                Some(c) => out.push(c),
            },
        }
    }
    out.trim_end().to_owned()
}

/// Every visible row of a Snapshot as text.
pub fn grid_text(snapshot: &Snapshot) -> Vec<String> {
    snapshot.grid.iter().map(row_text).collect()
}

/// The style of one cell of a Snapshot.
pub fn style_at(snapshot: &Snapshot, row: usize, col: usize) -> Style {
    let idx: StyleIdx = snapshot.grid[row].cell_at(col).style_idx;
    snapshot
        .styles
        .get(idx.get() as usize)
        .copied()
        .unwrap_or(Style::DEFAULT)
}
