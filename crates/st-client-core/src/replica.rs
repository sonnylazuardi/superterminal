//! The client-side [`Replica`] of a Surface — `docs/plan/04-client-native.md` §4.
//!
//! The Server owns the authoritative terminal state machine (invariant I1); a
//! Replica is the read-only mirror the renderer paints from. It is updated by
//! exactly three inbound messages:
//!
//! * [`Snapshot`] — replaces everything ([`Replica::apply_snapshot`]).
//! * [`Delta`] — an incremental update ([`Replica::apply_delta`]).
//! * [`History`] — a page of scrollback ([`Replica::apply_history_page`]).
//!
//! # Line coordinates
//!
//! Every line the Surface has ever produced has an [`AbsLine`] id, assigned
//! once and never renumbered (history reflow is disabled in v1, grilling Q40).
//! Per `02-protocol.md` §8:
//!
//! ```text
//! oldest_available = history_base                  (server's trim point)
//! first_visible    = history_base + history_len    (id of visible row 0)
//! ```
//!
//! The Replica keeps a **local cache** of history rows which is generally a
//! subset of what the Server retains: rows that scroll off the top of the grid
//! are appended to it, and [`FetchHistory`](st_proto::FetchHistory) pages are
//! prepended to it. The cache is always one contiguous run of lines,
//! `[cache_base, cache_base + cached_history_len)`; a page that does not touch
//! the existing run replaces it (documented on [`Replica::apply_history_page`]).
//!
//! # Gap detection (grilling Q38)
//!
//! Deltas are coalesced by the Server, so `seq` legitimately jumps. What must
//! be continuous is [`Delta::since_seq`]: a delta whose `since_seq` is not the
//! Replica's current `seq` means something was missed. [`Replica::apply_delta`]
//! returns [`Gap`] and changes nothing; the caller re-`Attach`es with
//! `want_snapshot: true`.

use std::collections::VecDeque;
use std::ops::Range;

use st_proto::{
    AbsLine, Cursor, Delta, ExitStatus, History, Modes, PackedCell, Row, Seq, Snapshot, Style,
    StyleTable, SurfaceId,
};

/// Default local history cache size, in lines (`04-client-native.md` §4).
///
/// Matches the Server's default `scrollback_lines`; 10 000 rows × 200 cols ×
/// 8 B is a 16 MB worst case per Surface, and far less in practice because
/// rows keep their trailing blanks trimmed.
pub const DEFAULT_HISTORY_CAP: usize = 10_000;

/// Hard ceiling on the configurable history cache (`02-protocol.md` §8).
pub const MAX_HISTORY_CAP: usize = 100_000;

/// How much history a Replica caches locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplicaConfig {
    /// Maximum number of history rows held in memory, clamped to
    /// [`MAX_HISTORY_CAP`].
    pub history_cap: usize,
}

impl Default for ReplicaConfig {
    fn default() -> Self {
        Self {
            history_cap: DEFAULT_HISTORY_CAP,
        }
    }
}

impl ReplicaConfig {
    /// A config with the given cache size, clamped to [`MAX_HISTORY_CAP`].
    #[must_use]
    pub fn with_history_cap(cap: usize) -> Self {
        Self {
            history_cap: cap.min(MAX_HISTORY_CAP),
        }
    }
}

/// A [`Delta`] did not build on the state this Replica holds (grilling Q38).
///
/// The delta was **not** applied and must not be buffered: the caller
/// re-`Attach`es with `want_snapshot: true`, and the Snapshot supersedes
/// everything in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("delta gap on surface {surface_id}: have seq {have}, delta builds on {since} (to {got})")]
pub struct Gap {
    /// The Surface whose stream gapped.
    pub surface_id: SurfaceId,
    /// The sequence number the Replica had applied.
    pub have: Seq,
    /// The sequence number the delta claimed to build on.
    pub since: Seq,
    /// The sequence number the delta would have produced.
    pub got: Seq,
}

/// A client-side mirror of one Surface's terminal state.
#[derive(Debug, Clone)]
pub struct Replica {
    /// The Surface this mirrors.
    surface_id: SurfaceId,
    /// Grid width in cells.
    cols: u16,
    /// Grid height in cells.
    rows: u16,
    /// The visible grid, exactly `rows` entries, top to bottom.
    visible: Vec<Row>,
    /// Locally cached history rows, oldest first.
    history: VecDeque<Row>,
    /// Absolute id of `history[0]`. Meaningless when `history` is empty.
    cache_base: AbsLine,
    /// The Server's trim point: the oldest line it still retains.
    history_base: AbsLine,
    /// How many history lines the Server retains (grilling Q39).
    history_len: u64,
    /// Style table; index 0 is always [`Style::DEFAULT`].
    styles: StyleTable,
    /// Cursor position and appearance.
    cursor: Cursor,
    /// Terminal modes.
    modes: Modes,
    /// Window title.
    title: String,
    /// Last applied sequence number; `0` means "nothing known".
    seq: Seq,
    /// `Some` once the Surface's process has ended.
    exited: Option<ExitStatus>,
    /// Local cache size limit.
    config: ReplicaConfig,
}

impl Replica {
    /// An empty Replica for `surface_id` with the default cache size.
    #[must_use]
    pub fn new(surface_id: SurfaceId) -> Self {
        Self::with_config(surface_id, ReplicaConfig::default())
    }

    /// An empty Replica with an explicit cache size.
    #[must_use]
    pub fn with_config(surface_id: SurfaceId, config: ReplicaConfig) -> Self {
        Self {
            surface_id,
            cols: 0,
            rows: 0,
            visible: Vec::new(),
            history: VecDeque::new(),
            cache_base: AbsLine::ZERO,
            history_base: AbsLine::ZERO,
            history_len: 0,
            styles: StyleTable::new(),
            cursor: Cursor::default(),
            modes: Modes::empty(),
            title: String::new(),
            seq: Seq::ZERO,
            exited: None,
            config,
        }
    }

    // ------------------------------------------------------------ accessors

    /// The Surface this Replica mirrors.
    #[inline]
    #[must_use]
    pub const fn surface_id(&self) -> SurfaceId {
        self.surface_id
    }

    /// Grid width in cells.
    #[inline]
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// Grid height in cells.
    #[inline]
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// The last applied sequence number; [`Seq::ZERO`] before the first
    /// Snapshot.
    #[inline]
    #[must_use]
    pub const fn seq(&self) -> Seq {
        self.seq
    }

    /// The style table, for resolving [`PackedCell::style_idx`].
    #[inline]
    #[must_use]
    pub const fn styles(&self) -> &StyleTable {
        &self.styles
    }

    /// Looks up a cell's [`Style`], falling back to the default for an
    /// unpopulated index.
    #[inline]
    #[must_use]
    pub fn style_of(&self, cell: PackedCell) -> Style {
        self.styles.get_or_default(cell.style_idx)
    }

    /// Cursor position and appearance.
    #[inline]
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Terminal modes, which drive key and mouse encoding.
    #[inline]
    #[must_use]
    pub const fn modes(&self) -> Modes {
        self.modes
    }

    /// The window title reported by the program.
    #[inline]
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// `Some` once the Surface's process has ended.
    #[inline]
    #[must_use]
    pub const fn exited(&self) -> Option<ExitStatus> {
        self.exited
    }

    /// The Server's trim point: the oldest line it still retains.
    #[inline]
    #[must_use]
    pub const fn history_base(&self) -> AbsLine {
        self.history_base
    }

    /// How many history lines the Server retains (grilling Q39).
    #[inline]
    #[must_use]
    pub const fn history_len(&self) -> u64 {
        self.history_len
    }

    /// Absolute id of visible row 0: `history_base + history_len`
    /// (`02-protocol.md` §8).
    #[inline]
    #[must_use]
    pub const fn first_visible_line(&self) -> AbsLine {
        self.history_base.saturating_add(self.history_len)
    }

    /// Total addressable lines: retained history plus the visible grid. This
    /// is the scrollbar's content length.
    #[inline]
    #[must_use]
    pub const fn total_lines(&self) -> u64 {
        self.history_len + self.rows as u64
    }

    /// The largest scroll offset (distance from the bottom, in lines) the user
    /// can reach: everything above the viewport.
    #[inline]
    #[must_use]
    pub const fn max_scroll_offset(&self) -> u64 {
        self.history_len
    }

    /// The absolute line range the cache actually holds,
    /// `[cache_base, cache_base + cached_history_len)`.
    #[must_use]
    pub fn cached_history_range(&self) -> Range<u64> {
        let base = self.cache_base.get();
        base..base + self.history.len() as u64
    }

    /// Number of history rows held locally, which is at most
    /// [`ReplicaConfig::history_cap`] and usually far below
    /// [`history_len`](Replica::history_len).
    #[inline]
    #[must_use]
    pub fn cached_history_len(&self) -> usize {
        self.history.len()
    }

    /// The visible grid row at `viewport_line` (`0` = top), or `None` when the
    /// index is past the grid.
    ///
    /// This is the renderer's hot accessor when the viewport is at the bottom.
    #[inline]
    #[must_use]
    pub fn row(&self, viewport_line: usize) -> Option<&Row> {
        self.visible.get(viewport_line)
    }

    /// The whole visible grid.
    #[inline]
    #[must_use]
    pub fn visible(&self) -> &[Row] {
        &self.visible
    }

    /// The row with absolute id `line`, from the visible grid or the local
    /// history cache. `None` means "not cached" — the caller paints blanks and
    /// issues a `FetchHistory`.
    #[must_use]
    pub fn line(&self, line: AbsLine) -> Option<&Row> {
        let first_visible = self.first_visible_line();
        if line >= first_visible {
            let idx = line.checked_sub(first_visible)?;
            return self.visible.get(usize::try_from(idx).ok()?);
        }
        let idx = line.checked_sub(self.cache_base)?;
        self.history.get(usize::try_from(idx).ok()?)
    }

    /// The absolute line range a viewport shows at `scroll_offset` lines above
    /// the bottom (grilling Q25: the offset is stored as distance from the
    /// bottom, `0` = following output).
    ///
    /// The offset is clamped to [`max_scroll_offset`](Replica::max_scroll_offset)
    /// so the range never runs below [`history_base`](Replica::history_base).
    #[must_use]
    pub fn viewport_range(&self, scroll_offset: u64) -> Range<AbsLine> {
        let offset = scroll_offset.min(self.max_scroll_offset());
        let top = self.first_visible_line().get().saturating_sub(offset);
        AbsLine::new(top)..AbsLine::new(top + self.rows as u64)
    }

    /// Whether the cursor should be painted: the program must want it *and*
    /// the viewport must be at the bottom (grilling Q48 — the cursor is hidden
    /// while scrolled up).
    #[must_use]
    pub fn cursor_visible_at(&self, scroll_offset: u64) -> bool {
        self.cursor.visible && scroll_offset == 0 && self.exited.is_none()
    }

    /// The absolute id of the line the cursor sits on.
    #[must_use]
    pub fn cursor_line(&self) -> AbsLine {
        self.first_visible_line()
            .saturating_add(self.cursor.row as u64)
    }

    // ------------------------------------------------------------- mutation

    /// Replaces the whole Replica with a [`Snapshot`] (§7).
    ///
    /// The local history cache is dropped: the Snapshot carries no history
    /// rows (only `history_base`/`history_len`), and it is refetched lazily as
    /// the user scrolls.
    pub fn apply_snapshot(&mut self, snap: &Snapshot) {
        self.surface_id = snap.surface_id;
        self.cols = snap.cols;
        self.rows = snap.rows;

        self.visible.clear();
        self.visible.reserve(snap.rows as usize);
        self.visible
            .extend(snap.grid.iter().take(snap.rows as usize).cloned());
        self.visible.resize(snap.rows as usize, Row::new());
        for row in &mut self.visible {
            truncate_row(row, snap.cols);
        }

        self.styles = StyleTable::from_wire(&snap.styles).unwrap_or_else(|| {
            tracing::warn!(
                surface = %snap.surface_id,
                len = snap.styles.len(),
                "snapshot carried a malformed style table; falling back to the default table"
            );
            StyleTable::new()
        });

        self.history.clear();
        self.history_base = snap.history_base;
        self.history_len = snap.history_len;
        self.cache_base = snap.first_visible_line();

        self.cursor = snap.cursor;
        self.modes = snap.modes;
        self.title.clear();
        self.title.push_str(&snap.title);
        self.seq = snap.seq;
        self.exited = snap.exited;
    }

    /// Applies a [`Delta`] (§4.3, §6), in the order given by
    /// `04-client-native.md` §4:
    ///
    /// 1. gap check on [`Delta::since_seq`] — on failure nothing changes;
    /// 2. style-table additions, before any row that references them;
    /// 3. resize, if any (every row then arrives dirty; there is no reflow);
    /// 4. rows that scrolled off the top move into the history cache, *before*
    ///    dirty rows overwrite them;
    /// 5. dirty rows replace their visible row;
    /// 6. cursor, modes, title and `seq`.
    pub fn apply_delta(&mut self, delta: &Delta) -> Result<(), Gap> {
        if delta.since_seq != self.seq {
            return Err(Gap {
                surface_id: delta.surface_id,
                have: self.seq,
                since: delta.since_seq,
                got: delta.seq,
            });
        }

        for &(idx, style) in &delta.new_styles {
            if !self.styles.set(idx, style) {
                tracing::warn!(
                    surface = %delta.surface_id,
                    index = idx.get(),
                    "delta carried a style index beyond the table cap; ignoring it"
                );
            }
        }

        let old_first_visible = self.first_visible_line();

        if let Some((cols, rows)) = delta.resized {
            self.resize(cols, rows);
        }

        let new_first_visible = delta.first_visible_line();
        self.scroll_off(old_first_visible, new_first_visible);

        self.history_base = delta.history_base;
        self.history_len = delta.history_len;
        self.evict_below_trim_point();

        for dirty in &delta.rows {
            let index = dirty.index as usize;
            if index >= self.visible.len() {
                tracing::warn!(
                    surface = %delta.surface_id,
                    index,
                    rows = self.visible.len(),
                    "delta carried a dirty row past the end of the grid; ignoring it"
                );
                continue;
            }
            let mut row = dirty.row.clone();
            truncate_row(&mut row, self.cols);
            self.visible[index] = row;
        }

        self.cursor = delta.cursor;
        self.modes = delta.modes;
        if let Some(title) = &delta.title {
            self.title.clear();
            self.title.push_str(title);
        }
        self.seq = delta.seq;
        Ok(())
    }

    /// Records that the Surface's process ended (`SurfaceExited`, §4.3). This
    /// consumes a sequence number, so it also advances `seq`.
    pub fn apply_exited(&mut self, seq: Seq, status: ExitStatus) {
        self.exited = Some(status);
        self.seq = seq;
    }

    /// Merges a [`History`] page into the local cache (§8).
    ///
    /// The cache stays one contiguous run. A page that extends, overlaps or
    /// abuts the run is merged into it; a page disjoint from it *replaces* it,
    /// because keeping two runs would make [`line`](Replica::line) lie about
    /// what is cached. Rows at or above `first_visible_line` are dropped: the
    /// visible grid is authoritative for those.
    pub fn apply_history_page(&mut self, page: &History) {
        if page.history_base > self.history_base {
            self.history_base = page.history_base;
        }

        let first_visible = self.first_visible_line().get();
        let page_start = page.from_line.get().max(self.history_base.get());
        let skip = usize::try_from(page_start - page.from_line.get()).unwrap_or(usize::MAX);
        let rows: Vec<Row> = page
            .rows
            .iter()
            .skip(skip)
            .take(usize::try_from(first_visible.saturating_sub(page_start)).unwrap_or(usize::MAX))
            .map(|row| {
                let mut row = row.clone();
                truncate_row(&mut row, self.cols);
                row
            })
            .collect();

        if rows.is_empty() {
            self.evict_below_trim_point();
            return;
        }
        let page_end = page_start + rows.len() as u64;
        let cached = self.cached_history_range();

        if self.history.is_empty() || page_end < cached.start || page_start > cached.end {
            // Disjoint from what we hold: the page wins.
            self.history.clear();
            self.history.extend(rows);
            self.cache_base = AbsLine::new(page_start);
        } else {
            if page_start < cached.start {
                let new = usize::try_from(cached.start - page_start).unwrap_or(0);
                for row in rows.iter().take(new).rev() {
                    self.history.push_front(row.clone());
                }
                self.cache_base = AbsLine::new(page_start);
            }
            if page_end > cached.end {
                let from = usize::try_from(cached.end.saturating_sub(page_start)).unwrap_or(0);
                for row in rows.iter().skip(from) {
                    self.history.push_back(row.clone());
                }
            }
        }

        self.evict_below_trim_point();
        self.trim_to_cap();
    }

    /// Resizes the grid without reflowing (grilling Q40): rows are added
    /// blank at the bottom or dropped from the bottom, and every surviving row
    /// is truncated to `cols`. The Server marks every row dirty after a
    /// resize, so the contents are corrected by the same Delta.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.visible.resize(rows as usize, Row::new());
        for row in &mut self.visible {
            truncate_row(row, cols);
        }
    }

    /// Shrinks the local history cache to at most `keep` rows, dropping the
    /// oldest. Used when a Tab is hidden: the rest is refetchable
    /// (`04-client-native.md` §4, "Memory budget").
    pub fn shrink_history_to(&mut self, keep: usize) {
        while self.history.len() > keep {
            self.history.pop_front();
            self.cache_base = self.cache_base.saturating_add(1);
        }
        self.history.shrink_to_fit();
    }

    /// Replaces the cache-size limit and applies it immediately.
    pub fn set_config(&mut self, config: ReplicaConfig) {
        self.config = ReplicaConfig::with_history_cap(config.history_cap);
        self.trim_to_cap();
    }

    /// The current cache-size limit.
    #[inline]
    #[must_use]
    pub const fn config(&self) -> ReplicaConfig {
        self.config
    }

    // -------------------------------------------------------------- internals

    /// Moves the rows between `old_first_visible` and `new_first_visible` out
    /// of the visible grid and into the history cache.
    fn scroll_off(&mut self, old_first_visible: AbsLine, new_first_visible: AbsLine) {
        let Some(appended) = new_first_visible.checked_sub(old_first_visible) else {
            // The Surface went backwards: an alt-screen exit, or a Server that
            // restarted. Nothing we hold is addressable any more.
            self.history.clear();
            self.cache_base = new_first_visible;
            return;
        };
        if appended == 0 {
            return;
        }
        if appended > self.visible.len() as u64 || self.cache_base_is_stale(old_first_visible) {
            // More lines scrolled past than we ever saw, so the run would have
            // a hole in it. Drop the cache; it is refetchable.
            self.history.clear();
            self.cache_base = new_first_visible;
            return;
        }
        let n = appended as usize;
        for row in self.visible.drain(..n) {
            self.history.push_back(row);
        }
        self.visible.resize(self.rows as usize, Row::new());
        self.trim_to_cap();
    }

    /// `true` when the cache does not abut the visible grid, so pushing more
    /// rows onto it would create a hole.
    fn cache_base_is_stale(&self, first_visible: AbsLine) -> bool {
        !self.history.is_empty() && self.cached_history_range().end != first_visible.get()
    }

    /// Drops cached rows the Server has trimmed away.
    fn evict_below_trim_point(&mut self) {
        let base = self.history_base.get();
        while !self.history.is_empty() && self.cache_base.get() < base {
            self.history.pop_front();
            self.cache_base = self.cache_base.saturating_add(1);
        }
        if self.history.is_empty() {
            self.cache_base = self.first_visible_line();
        }
    }

    /// Enforces [`ReplicaConfig::history_cap`], evicting the oldest rows.
    fn trim_to_cap(&mut self) {
        let cap = self.config.history_cap.min(MAX_HISTORY_CAP);
        while self.history.len() > cap {
            self.history.pop_front();
            self.cache_base = self.cache_base.saturating_add(1);
        }
    }
}

/// Drops cells past `cols`. Rows keep their trailing blanks trimmed (grilling
/// Q41); [`Row::cell_at`] re-pads on read, so the renderer never notices.
fn truncate_row(row: &mut Row, cols: u16) {
    if row.cells.len() > cols as usize {
        row.cells.truncate(cols as usize);
    }
    row.trim_trailing_blanks();
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{CellFlags, Color, DirtyRow, StyleIdx};

    pub(crate) fn row_of(text: &str) -> Row {
        let mut row = Row {
            cells: text
                .chars()
                .map(|c| PackedCell::from_char(c, StyleIdx::ZERO))
                .collect(),
            extras: Vec::new(),
            wrapped: false,
        };
        row.trim_trailing_blanks();
        row
    }

    pub(crate) fn row_text(row: &Row, cols: u16) -> String {
        (0..cols)
            .map(|c| {
                let cell = row.cell_at(c as usize);
                char::from_u32(cell.codepoint).unwrap_or(' ')
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    pub(crate) fn snapshot(cols: u16, rows: u16, lines: &[&str]) -> Snapshot {
        Snapshot {
            surface_id: SurfaceId(7),
            seq: Seq(1),
            cols,
            rows,
            styles: vec![Style::DEFAULT],
            grid: lines.iter().map(|l| row_of(l)).collect(),
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: "shell".into(),
            history_base: AbsLine(0),
            history_len: 0,
            view_state: st_proto::ViewState::default(),
            exited: None,
        }
    }

    pub(crate) fn delta(seq: u64, since: u64) -> Delta {
        Delta {
            surface_id: SurfaceId(7),
            seq: Seq(seq),
            since_seq: Seq(since),
            history_base: AbsLine(0),
            history_len: 0,
            resized: None,
            new_styles: Vec::new(),
            rows: Vec::new(),
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: None,
        }
    }

    fn grid(replica: &Replica) -> Vec<String> {
        replica
            .visible()
            .iter()
            .map(|r| row_text(r, replica.cols()))
            .collect()
    }

    #[test]
    fn snapshot_then_deltas_build_the_expected_grid() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(10, 3, &["one", "two", "three"]));
        assert_eq!(replica.seq(), Seq(1));
        assert_eq!(replica.cols(), 10);
        assert_eq!(replica.rows(), 3);
        assert_eq!(grid(&replica), vec!["one", "two", "three"]);
        assert_eq!(replica.title(), "shell");
        assert_eq!(replica.total_lines(), 3);

        let mut d = delta(2, 1);
        d.rows = vec![DirtyRow {
            index: 1,
            row: row_of("TWO"),
        }];
        d.title = Some("vim".into());
        replica.apply_delta(&d).unwrap();
        assert_eq!(grid(&replica), vec!["one", "TWO", "three"]);
        assert_eq!(replica.title(), "vim");

        // Coalesced: seq jumps but since_seq is continuous.
        let mut d = delta(9, 2);
        d.rows = vec![
            DirtyRow {
                index: 0,
                row: row_of("ONE"),
            },
            DirtyRow {
                index: 2,
                row: row_of("THREE"),
            },
        ];
        d.cursor = Cursor {
            row: 2,
            col: 5,
            ..Cursor::default()
        };
        replica.apply_delta(&d).unwrap();
        assert_eq!(grid(&replica), vec!["ONE", "TWO", "THREE"]);
        assert_eq!(replica.seq(), Seq(9));
        assert_eq!(replica.cursor().col, 5);
        assert_eq!(replica.cursor_line(), AbsLine(2));
    }

    #[test]
    fn a_delta_with_the_wrong_since_seq_is_a_gap() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(10, 2, &["a", "b"]));

        let mut d = delta(3, 2); // we are at 1
        d.rows = vec![DirtyRow {
            index: 0,
            row: row_of("nope"),
        }];
        let gap = replica.apply_delta(&d).unwrap_err();
        assert_eq!(
            gap,
            Gap {
                surface_id: SurfaceId(7),
                have: Seq(1),
                since: Seq(2),
                got: Seq(3),
            }
        );
        // Nothing was applied.
        assert_eq!(grid(&replica), vec!["a", "b"]);
        assert_eq!(replica.seq(), Seq(1));
        assert!(gap.to_string().contains("have seq 1"));
    }

    #[test]
    fn a_delta_going_backwards_is_also_a_gap() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(4, 1, &["x"]));
        let mut d = delta(2, 1);
        d.rows = vec![DirtyRow {
            index: 0,
            row: row_of("y"),
        }];
        replica.apply_delta(&d).unwrap();
        // Server restarted and replays from 1.
        assert!(replica.apply_delta(&delta(2, 1)).is_err());
    }

    #[test]
    fn scrolled_off_rows_land_in_the_history_cache() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(8, 3, &["l0", "l1", "l2"]));
        assert_eq!(replica.first_visible_line(), AbsLine(0));

        // Two lines scroll off; the grid ends up ["l2", "n0", "n1"].
        let mut d = delta(2, 1);
        d.history_len = 2;
        d.rows = vec![
            DirtyRow {
                index: 1,
                row: row_of("n0"),
            },
            DirtyRow {
                index: 2,
                row: row_of("n1"),
            },
        ];
        replica.apply_delta(&d).unwrap();

        assert_eq!(grid(&replica), vec!["l2", "n0", "n1"]);
        assert_eq!(replica.first_visible_line(), AbsLine(2));
        assert_eq!(replica.cached_history_len(), 2);
        assert_eq!(replica.cached_history_range(), 0..2);
        assert_eq!(row_text(replica.line(AbsLine(0)).unwrap(), 8), "l0");
        assert_eq!(row_text(replica.line(AbsLine(1)).unwrap(), 8), "l1");
        assert_eq!(row_text(replica.line(AbsLine(2)).unwrap(), 8), "l2");
        assert_eq!(replica.line(AbsLine(5)), None);
        assert_eq!(replica.total_lines(), 5);
        assert_eq!(replica.max_scroll_offset(), 2);
    }

    #[test]
    fn viewport_range_clamps_to_the_available_history() {
        let mut replica = Replica::new(SurfaceId(7));
        let mut snap = snapshot(8, 3, &["a", "b", "c"]);
        snap.history_base = AbsLine(100);
        snap.history_len = 4;
        replica.apply_snapshot(&snap);

        assert_eq!(replica.first_visible_line(), AbsLine(104));
        assert_eq!(replica.viewport_range(0), AbsLine(104)..AbsLine(107));
        assert_eq!(replica.viewport_range(2), AbsLine(102)..AbsLine(105));
        // Clamped: only 4 lines of history exist.
        assert_eq!(replica.viewport_range(99), AbsLine(100)..AbsLine(103));
    }

    #[test]
    fn the_cursor_is_hidden_while_scrolled_up() {
        let mut replica = Replica::new(SurfaceId(7));
        let mut snap = snapshot(8, 2, &["a", "b"]);
        snap.history_len = 10;
        replica.apply_snapshot(&snap);
        assert!(replica.cursor_visible_at(0));
        assert!(!replica.cursor_visible_at(1));

        replica.apply_exited(
            Seq(2),
            ExitStatus {
                code: Some(0),
                signal: None,
            },
        );
        assert!(!replica.cursor_visible_at(0));
        assert_eq!(replica.seq(), Seq(2));
        assert_eq!(replica.exited().unwrap().code, Some(0));
    }

    #[test]
    fn history_pages_are_prepended_and_merged() {
        let mut replica = Replica::new(SurfaceId(7));
        let mut snap = snapshot(8, 2, &["v0", "v1"]);
        snap.history_base = AbsLine(0);
        snap.history_len = 10;
        replica.apply_snapshot(&snap);
        assert_eq!(replica.first_visible_line(), AbsLine(10));
        assert_eq!(replica.cached_history_len(), 0);

        // Fetch [6, 10).
        replica.apply_history_page(&History {
            surface_id: SurfaceId(7),
            from_line: AbsLine(6),
            history_base: AbsLine(0),
            rows: vec![row_of("h6"), row_of("h7"), row_of("h8"), row_of("h9")],
        });
        assert_eq!(replica.cached_history_range(), 6..10);
        assert_eq!(row_text(replica.line(AbsLine(6)).unwrap(), 8), "h6");

        // Fetch [2, 6): prepends onto the same run.
        replica.apply_history_page(&History {
            surface_id: SurfaceId(7),
            from_line: AbsLine(2),
            history_base: AbsLine(0),
            rows: vec![row_of("h2"), row_of("h3"), row_of("h4"), row_of("h5")],
        });
        assert_eq!(replica.cached_history_range(), 2..10);
        assert_eq!(row_text(replica.line(AbsLine(2)).unwrap(), 8), "h2");
        assert_eq!(row_text(replica.line(AbsLine(9)).unwrap(), 8), "h9");
        assert_eq!(replica.line(AbsLine(1)), None);
    }

    #[test]
    fn a_history_page_past_the_trim_point_is_clipped() {
        let mut replica = Replica::new(SurfaceId(7));
        let mut snap = snapshot(8, 1, &["v"]);
        snap.history_base = AbsLine(5);
        snap.history_len = 5;
        replica.apply_snapshot(&snap);

        // The server has trimmed to 7 since we asked.
        replica.apply_history_page(&History {
            surface_id: SurfaceId(7),
            from_line: AbsLine(5),
            history_base: AbsLine(7),
            rows: vec![row_of("h5"), row_of("h6"), row_of("h7"), row_of("h8")],
        });
        assert_eq!(replica.history_base(), AbsLine(7));
        assert_eq!(replica.cached_history_range(), 7..9);
        assert_eq!(row_text(replica.line(AbsLine(7)).unwrap(), 8), "h7");
        assert_eq!(replica.line(AbsLine(6)), None);
    }

    #[test]
    fn a_disjoint_history_page_replaces_the_cache() {
        let mut replica = Replica::new(SurfaceId(7));
        let mut snap = snapshot(8, 1, &["v"]);
        snap.history_len = 100;
        replica.apply_snapshot(&snap);

        replica.apply_history_page(&History {
            surface_id: SurfaceId(7),
            from_line: AbsLine(90),
            history_base: AbsLine(0),
            rows: vec![row_of("a"), row_of("b")],
        });
        assert_eq!(replica.cached_history_range(), 90..92);

        replica.apply_history_page(&History {
            surface_id: SurfaceId(7),
            from_line: AbsLine(10),
            history_base: AbsLine(0),
            rows: vec![row_of("c"), row_of("d")],
        });
        assert_eq!(replica.cached_history_range(), 10..12);
        assert_eq!(replica.line(AbsLine(90)), None);
    }

    #[test]
    fn the_cache_is_trimmed_at_the_configured_cap() {
        let mut replica = Replica::with_config(SurfaceId(7), ReplicaConfig::with_history_cap(16));
        replica.apply_snapshot(&snapshot(8, 1, &["v0"]));

        for i in 0..40u64 {
            // seq n+2 builds on n+1: the snapshot was seq 1.
            let mut d = delta(i + 2, i + 1);
            d.history_len = i + 1;
            d.rows = vec![DirtyRow {
                index: 0,
                row: row_of(&format!("v{}", i + 1)),
            }];
            replica.apply_delta(&d).unwrap();
        }

        assert_eq!(replica.history_len(), 40);
        assert_eq!(replica.cached_history_len(), 16);
        assert_eq!(replica.cached_history_range(), 24..40);
        assert_eq!(row_text(replica.line(AbsLine(24)).unwrap(), 8), "v24");
        assert_eq!(replica.line(AbsLine(23)), None);
        assert_eq!(replica.total_lines(), 41);

        replica.shrink_history_to(4);
        assert_eq!(replica.cached_history_range(), 36..40);
    }

    #[test]
    fn setting_a_smaller_cap_trims_immediately() {
        let mut replica = Replica::with_config(SurfaceId(7), ReplicaConfig::with_history_cap(50));
        replica.apply_snapshot(&snapshot(4, 1, &["v"]));
        replica.apply_history_page(&History {
            surface_id: SurfaceId(7),
            from_line: AbsLine(0),
            history_base: AbsLine(0),
            rows: Vec::new(),
        });
        for i in 0..20u64 {
            let mut d = delta(i + 2, i + 1);
            d.history_len = i + 1;
            replica.apply_delta(&d).unwrap();
        }
        assert_eq!(replica.cached_history_len(), 20);
        replica.set_config(ReplicaConfig::with_history_cap(5));
        assert_eq!(replica.cached_history_len(), 5);
        assert_eq!(replica.config().history_cap, 5);

        // The hard ceiling is enforced.
        assert_eq!(
            ReplicaConfig::with_history_cap(usize::MAX).history_cap,
            MAX_HISTORY_CAP
        );
    }

    #[test]
    fn a_big_scroll_jump_drops_the_cache_rather_than_leaving_a_hole() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(8, 3, &["a", "b", "c"]));

        // 100 lines scrolled past a 3-row grid: we never saw 97 of them.
        let mut d = delta(2, 1);
        d.history_len = 100;
        replica.apply_delta(&d).unwrap();
        assert_eq!(replica.cached_history_len(), 0);
        assert_eq!(replica.first_visible_line(), AbsLine(100));
        assert_eq!(replica.line(AbsLine(50)), None);
    }

    #[test]
    fn a_resize_drops_rows_without_reflowing() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(10, 3, &["aaaaaaaa", "bbbbbbbb", "cccccccc"]));

        let mut d = delta(2, 1);
        d.resized = Some((4, 2));
        d.rows = vec![
            DirtyRow {
                index: 0,
                row: row_of("aaaa"),
            },
            DirtyRow {
                index: 1,
                row: row_of("bbbb"),
            },
        ];
        replica.apply_delta(&d).unwrap();
        assert_eq!(replica.cols(), 4);
        assert_eq!(replica.rows(), 2);
        assert_eq!(grid(&replica), vec!["aaaa", "bbbb"]);

        // A dirty row wider than cols is clipped, not wrapped.
        let mut d = delta(3, 2);
        d.rows = vec![DirtyRow {
            index: 0,
            row: row_of("0123456789"),
        }];
        replica.apply_delta(&d).unwrap();
        assert_eq!(grid(&replica)[0], "0123");
    }

    #[test]
    fn wide_chars_and_spacers_survive_intact() {
        let mut replica = Replica::new(SurfaceId(7));
        let wide = Row {
            cells: vec![
                PackedCell::from_char('a', StyleIdx::ZERO),
                PackedCell::new('世' as u32, StyleIdx::ZERO, CellFlags::WIDE),
                PackedCell::new(0, StyleIdx::ZERO, CellFlags::WIDE_SPACER),
                PackedCell::new(0, StyleIdx::new(1), CellFlags::GRAPHEME_EXT),
            ],
            extras: vec!["e\u{301}".into()],
            wrapped: true,
        };
        let mut snap = snapshot(8, 1, &["x"]);
        snap.grid = vec![wide.clone()];
        snap.styles = vec![
            Style::DEFAULT,
            Style {
                fg: Color::Indexed(3),
                ..Style::DEFAULT
            },
        ];
        replica.apply_snapshot(&snap);

        let row = replica.row(0).unwrap();
        assert_eq!(row, &wide);
        assert!(row.cell_at(1).flags.contains(CellFlags::WIDE));
        assert!(row.cell_at(2).flags.contains(CellFlags::WIDE_SPACER));
        assert_eq!(row.grapheme(row.cell_at(3)), Some("e\u{301}"));
        assert!(row.wrapped);
        assert_eq!(replica.style_of(row.cell_at(3)).fg, Color::Indexed(3));
        // Past the trimmed tail the row reads as blanks.
        assert_eq!(row.cell_at(7), PackedCell::BLANK);
    }

    #[test]
    fn new_styles_are_installed_before_the_rows_that_use_them() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(4, 1, &["a"]));

        let styled = Style {
            fg: Color::Rgb(1, 2, 3),
            ..Style::DEFAULT
        };
        let mut d = delta(2, 1);
        d.new_styles = vec![(StyleIdx::new(5), styled)];
        d.rows = vec![DirtyRow {
            index: 0,
            row: Row {
                cells: vec![PackedCell::from_char('z', StyleIdx::new(5))],
                extras: Vec::new(),
                wrapped: false,
            },
        }];
        replica.apply_delta(&d).unwrap();
        let cell = replica.row(0).unwrap().cell_at(0);
        assert_eq!(replica.style_of(cell), styled);
        assert_eq!(replica.styles().get(StyleIdx::new(4)), Some(Style::DEFAULT));
    }

    #[test]
    fn a_snapshot_supersedes_everything() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(8, 2, &["a", "b"]));
        let mut d = delta(2, 1);
        d.history_len = 2;
        replica.apply_delta(&d).unwrap();
        assert_eq!(replica.cached_history_len(), 2);

        let mut snap = snapshot(4, 1, &["fresh"]);
        snap.seq = Seq(50);
        snap.history_base = AbsLine(9);
        snap.history_len = 3;
        replica.apply_snapshot(&snap);
        assert_eq!(replica.seq(), Seq(50));
        assert_eq!(replica.cached_history_len(), 0);
        assert_eq!(replica.first_visible_line(), AbsLine(12));
        assert_eq!(grid(&replica), vec!["fres"]);
        // A gapped delta stream now resyncs from 50.
        assert!(replica.apply_delta(&delta(51, 49)).is_err());
        assert!(replica.apply_delta(&delta(51, 50)).is_ok());
    }

    #[test]
    fn a_short_snapshot_grid_is_padded_and_a_long_one_clipped() {
        let mut replica = Replica::new(SurfaceId(7));
        let mut snap = snapshot(4, 4, &["a"]);
        replica.apply_snapshot(&snap);
        assert_eq!(replica.visible().len(), 4);
        assert_eq!(grid(&replica), vec!["a", "", "", ""]);

        snap.rows = 2;
        snap.grid = vec![row_of("a"), row_of("b"), row_of("c")];
        replica.apply_snapshot(&snap);
        assert_eq!(replica.visible().len(), 2);
    }

    #[test]
    fn a_dirty_row_past_the_grid_is_ignored() {
        let mut replica = Replica::new(SurfaceId(7));
        replica.apply_snapshot(&snapshot(4, 1, &["a"]));
        let mut d = delta(2, 1);
        d.rows = vec![DirtyRow {
            index: 9,
            row: row_of("boom"),
        }];
        replica.apply_delta(&d).unwrap();
        assert_eq!(grid(&replica), vec!["a"]);
        assert_eq!(replica.seq(), Seq(2));
    }

    #[test]
    fn a_malformed_snapshot_style_table_falls_back_to_the_default() {
        let mut replica = Replica::new(SurfaceId(7));
        let mut snap = snapshot(4, 1, &["a"]);
        snap.styles = Vec::new();
        replica.apply_snapshot(&snap);
        assert_eq!(replica.styles().len(), 1);
        assert_eq!(replica.styles().get(StyleIdx::ZERO), Some(Style::DEFAULT));
    }
}

#[cfg(test)]
mod property_tests {
    //! The equivalence property: a Replica driven by a stream of Deltas must
    //! equal a naive model that simply overwrites rows and pushes scrolled-off
    //! rows into an unbounded list. This is the closest this crate can get to
    //! "the Replica equals the Server's grid" without depending on `st-core`.

    use super::tests::{delta, row_of, row_text, snapshot};
    use super::*;
    use proptest::prelude::*;
    use st_proto::{DirtyRow, StyleIdx};

    const COLS: u16 = 12;
    const ROWS: u16 = 6;

    /// One step of a random Delta stream.
    #[derive(Debug, Clone)]
    struct Step {
        /// `(row index, text)` pairs.
        dirty: Vec<(u16, String)>,
        /// How many lines scroll off the top in this step.
        scrolled: u64,
        /// Cursor column, to check the scalar fields ride along too.
        cursor_col: u16,
    }

    fn step_strategy() -> impl Strategy<Value = Step> {
        (
            prop::collection::vec(
                (0..ROWS, "[a-z]{0,12}").prop_map(|(i, s)| (i, s)),
                0..(ROWS as usize + 2),
            ),
            0u64..(ROWS as u64 + 1),
            0..COLS,
        )
            .prop_map(|(dirty, scrolled, cursor_col)| Step {
                dirty,
                scrolled,
                cursor_col,
            })
    }

    /// The reference model: a plain `Vec<String>` grid plus an unbounded
    /// history, updated in the order the spec prescribes.
    #[derive(Debug)]
    struct Model {
        visible: Vec<String>,
        history: Vec<String>,
    }

    impl Model {
        fn new(initial: &[&str]) -> Self {
            let mut visible: Vec<String> = initial.iter().map(|s| (*s).to_string()).collect();
            visible.resize(ROWS as usize, String::new());
            Self {
                visible,
                history: Vec::new(),
            }
        }

        fn apply(&mut self, step: &Step) {
            for _ in 0..step.scrolled {
                self.history.push(self.visible.remove(0));
                self.visible.push(String::new());
            }
            for (index, text) in &step.dirty {
                let index = *index as usize;
                if index < self.visible.len() {
                    self.visible[index] = text.clone();
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn a_delta_stream_matches_the_naive_model(steps in prop::collection::vec(step_strategy(), 1..40)) {
            let initial = ["r0", "r1", "r2", "r3", "r4", "r5"];
            let mut replica = Replica::new(SurfaceId(7));
            replica.apply_snapshot(&snapshot(COLS, ROWS, &initial));
            let mut model = Model::new(&initial);

            let mut seq = 1u64;
            let mut history_len = 0u64;
            for step in &steps {
                let mut d = delta(seq + 1, seq);
                seq += 1;
                history_len += step.scrolled;
                d.history_len = history_len;
                d.rows = step
                    .dirty
                    .iter()
                    .map(|(index, text)| DirtyRow { index: *index, row: row_of(text) })
                    .collect();
                d.cursor = Cursor { col: step.cursor_col, ..Cursor::default() };
                replica.apply_delta(&d).unwrap();
                model.apply(step);

                // Scalars.
                prop_assert_eq!(replica.seq(), Seq(seq));
                prop_assert_eq!(replica.cursor().col, step.cursor_col);
                prop_assert_eq!(replica.history_len(), model.history.len() as u64);
                prop_assert_eq!(
                    replica.first_visible_line(),
                    AbsLine(model.history.len() as u64)
                );
                prop_assert_eq!(replica.total_lines(), model.history.len() as u64 + ROWS as u64);

                // The visible grid must match exactly.
                let got: Vec<String> = replica
                    .visible()
                    .iter()
                    .map(|r| row_text(r, COLS))
                    .collect();
                prop_assert_eq!(&got, &model.visible);

                // Every history line the Replica claims to cache must match
                // the model, and the cache must never exceed the cap.
                let cached = replica.cached_history_range();
                prop_assert!(replica.cached_history_len() <= DEFAULT_HISTORY_CAP);
                for line in cached.clone() {
                    let row = replica.line(AbsLine(line)).expect("cached line is readable");
                    prop_assert_eq!(row_text(row, COLS), model.history[line as usize].clone());
                }
                // Whatever the Replica does cache must abut the visible grid,
                // so `line()` never has a hole.
                if !cached.is_empty() {
                    prop_assert_eq!(cached.end, model.history.len() as u64);
                }
            }
        }

        #[test]
        fn any_since_seq_other_than_the_current_one_gaps(
            have in 1u64..1000,
            since in 0u64..1000,
        ) {
            let mut replica = Replica::new(SurfaceId(7));
            let mut snap = snapshot(4, 1, &["x"]);
            snap.seq = Seq(have);
            replica.apply_snapshot(&snap);

            let mut d = delta(have + 5, since);
            d.rows = vec![DirtyRow { index: 0, row: row_of("y") }];
            let result = replica.apply_delta(&d);
            prop_assert_eq!(result.is_ok(), since == have);
            if result.is_err() {
                // A rejected delta leaves the Replica untouched.
                prop_assert_eq!(replica.seq(), Seq(have));
                prop_assert_eq!(row_text(replica.row(0).unwrap(), 4), "x".to_string());
            }
        }

        #[test]
        fn style_indices_never_panic(indices in prop::collection::vec(0u16..5000, 0..20)) {
            let mut replica = Replica::new(SurfaceId(7));
            replica.apply_snapshot(&snapshot(4, 1, &["x"]));
            let mut d = delta(2, 1);
            d.new_styles = indices
                .iter()
                .map(|i| (StyleIdx::new(*i), Style::DEFAULT))
                .collect();
            replica.apply_delta(&d).unwrap();
            prop_assert!(replica.styles().len() <= st_proto::STYLE_TABLE_CAP);
        }
    }
}
