//! The Surface engine object (`docs/plan/03-server.md` §4).
//!
//! A [`Surface`] is one PTY, one authoritative VT state machine, one style
//! table and one [`Publisher`], with no threads and no runtime of its own. The
//! Server wraps it in a task that owns the PTY reader/writer/waiter threads and
//! a timer; everything in here is synchronous and deterministic.
//!
//! ```text
//!   feed(bytes) ──► VtEngine.advance ──► drain_events ──► take_damage
//!         │                 │                  │              │
//!         └──► CwdTracker   └──► pty_replies    └──► title/bell└──► Coalesced
//!
//!   flush(now) ──► Publisher.flush ──► Snapshot | Delta | Bell  per Client
//! ```
//!
//! Sequence numbers: a Surface starts at [`Seq::FIRST`] and consumes exactly
//! one number per emitted state change (§6, `02-protocol.md` §6).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use st_proto::{
    AbsLine, AttachMode, Bell, Cursor, DataMsg, Delta, DirtyRow, History, Modes, Row, Seq,
    Snapshot, SurfaceExited, SurfaceId, ViewState,
};

use crate::cwd::CwdTracker;
use crate::pty::{ExitStatus, Pty, PtyConfig, PtyError};
use crate::publisher::{ClientId, Coalesced, EmissionKind, Publisher, PublisherConfig};
use crate::style_table::SurfaceStyleTable;
use crate::vt::alacritty::{AlacrittyEngine, EngineConfig};
use crate::vt::{Rgb, TextAreaSize, VtEngine, VtEvent};

/// A palette lookup used only to answer OSC 4/10/11 colour queries.
///
/// The Server reads it from `[theme]` (grilling Q48); it never renders.
pub type PaletteFn = Arc<dyn Fn(usize) -> Option<Rgb> + Send + Sync>;

/// Whether a Surface's program is still running (`03-server.md` §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceStatus {
    /// The child is alive.
    Running {
        /// Its pid, when the platform reports one.
        pid: Option<u32>,
    },
    /// The child is gone; the grid stays readable (Q22).
    Exited(ExitStatus),
}

impl SurfaceStatus {
    /// `true` while the program is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self, SurfaceStatus::Running { .. })
    }
}

/// Everything needed to build a [`Surface`].
#[derive(Debug, Clone)]
pub struct SurfaceConfig {
    /// The Surface's id.
    pub id: SurfaceId,
    /// Engine settings; `cols`/`rows` also size the PTY.
    pub engine: EngineConfig,
    /// `Some` to open a PTY and spawn a shell; `None` for an engine-only
    /// Surface (replay, fixtures, tests).
    pub pty: Option<PtyConfig>,
    /// Directory the Surface was created in; the last cwd fallback.
    pub spawn_cwd: PathBuf,
    /// Fan-out tunables.
    pub publisher: PublisherConfig,
}

impl Default for SurfaceConfig {
    fn default() -> Self {
        Self {
            id: SurfaceId::ZERO,
            engine: EngineConfig::default(),
            pty: None,
            spawn_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            publisher: PublisherConfig::default(),
        }
    }
}

/// One frame destined for one Client.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientFrame {
    /// Who it is for.
    pub client: ClientId,
    /// The data-plane message to encode.
    pub msg: DataMsg,
}

/// What [`Surface::take_update`] produced.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceUpdate {
    /// Nothing changed since the last call.
    Idle,
    /// An incremental update.
    Delta(Box<Delta>),
    /// A full resync — the style table overflowed, or the Surface was reset.
    Snapshot(Box<Snapshot>),
}

/// One PTY + one authoritative terminal state machine (`03-server.md` §4).
pub struct Surface {
    id: SurfaceId,
    engine: Box<dyn VtEngine>,
    pty: Option<Pty>,
    styles: SurfaceStyleTable,
    cwd: CwdTracker,
    publisher: Publisher,
    palette: Option<PaletteFn>,

    seq: Seq,
    status: SurfaceStatus,
    view_state: ViewState,

    /// Changes since the last sequence bump, for the single-consumer
    /// [`Surface::take_update`] path.
    pending: Coalesced,
    needs_snapshot: bool,
    /// Style-table generation and cursor for the single-consumer
    /// [`Surface::take_update`] path; the fan-out path keeps one per
    /// subscription (ADR 0011).
    own_styles_generation: u32,
    own_styles_sent: u16,

    last_cursor: Cursor,
    last_modes: Modes,
    last_title: String,
    last_history: (AbsLine, u64),

    pty_out: Vec<u8>,
}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface")
            .field("id", &self.id)
            .field("seq", &self.seq)
            .field("cols", &self.engine.cols())
            .field("rows", &self.engine.rows())
            .field("status", &self.status)
            .field("clients", &self.publisher.len())
            .finish()
    }
}

impl Surface {
    /// Builds a Surface, opening a PTY when `config.pty` is `Some`.
    pub fn new(config: SurfaceConfig) -> Result<Self, PtyError> {
        let engine = AlacrittyEngine::new(config.engine.clone());
        let pty = match &config.pty {
            Some(pty_config) => {
                let mut pty_config = pty_config.clone();
                pty_config.cols = config.engine.cols;
                pty_config.rows = config.engine.rows;
                pty_config.surface_id = config.id;
                if pty_config.cwd.is_none() {
                    pty_config.cwd = Some(config.spawn_cwd.clone());
                }
                Some(Pty::spawn(&pty_config)?)
            }
            None => None,
        };
        Ok(Self::assemble(config, Box::new(engine), pty))
    }

    /// Builds a Surface around an arbitrary engine and no PTY.
    ///
    /// This is the seam the `VtEngine` trait exists for: fixtures, replay and
    /// a future `GhosttyEngine` all come in here.
    #[must_use]
    pub fn with_engine(config: SurfaceConfig, engine: Box<dyn VtEngine>) -> Self {
        Self::assemble(config, engine, None)
    }

    fn assemble(config: SurfaceConfig, engine: Box<dyn VtEngine>, pty: Option<Pty>) -> Self {
        let rows = engine.rows() as usize;
        let (cursor, modes) = engine.cursor_and_modes();
        let title = engine.title().to_owned();
        let history = (engine.history_base(), engine.history_len());
        let pid = pty.as_ref().and_then(Pty::pid);
        Self {
            id: config.id,
            engine,
            pty,
            styles: SurfaceStyleTable::new(),
            cwd: CwdTracker::new(config.spawn_cwd),
            publisher: Publisher::new(config.publisher, rows),
            palette: None,
            seq: Seq::FIRST,
            status: SurfaceStatus::Running { pid },
            view_state: ViewState::default(),
            pending: Coalesced::new(rows),
            needs_snapshot: false,
            own_styles_generation: 0,
            own_styles_sent: 0,
            last_cursor: cursor,
            last_modes: modes,
            last_title: title,
            last_history: history,
            pty_out: Vec::new(),
        }
    }

    /// Installs the palette used to answer OSC colour queries (Q48).
    pub fn set_palette(&mut self, palette: Option<PaletteFn>) {
        self.palette = palette;
    }

    /// The Surface's id.
    #[must_use]
    pub fn id(&self) -> SurfaceId {
        self.id
    }

    /// The sequence number of the last emitted state.
    #[must_use]
    pub fn seq(&self) -> Seq {
        self.seq
    }

    /// Grid size as `(cols, rows)`.
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        (self.engine.cols(), self.engine.rows())
    }

    /// Whether the program is running or has exited.
    #[must_use]
    pub fn status(&self) -> &SurfaceStatus {
        &self.status
    }

    /// The current window title.
    #[must_use]
    pub fn title(&self) -> &str {
        self.engine.title()
    }

    /// The Surface's best-known working directory (OSC 7, probe, spawn cwd).
    #[must_use]
    pub fn cwd(&self) -> &std::path::Path {
        self.cwd.current()
    }

    /// The cwd, if it changed since the last call.
    pub fn take_cwd_change(&mut self) -> Option<PathBuf> {
        self.cwd.take_changed()
    }

    /// Re-reads the foreground process's directory (`03-server.md` §9).
    pub fn probe_cwd(&mut self) {
        let pid = self.pty.as_ref().and_then(Pty::foreground_pgid);
        self.cwd.probe(pid);
    }

    /// The PTY, when this Surface has one.
    #[must_use]
    pub fn pty(&self) -> Option<&Pty> {
        self.pty.as_ref()
    }

    /// Mutable access to the PTY (for `resize`, `kill`, `wait`).
    pub fn pty_mut(&mut self) -> Option<&mut Pty> {
        self.pty.as_mut()
    }

    /// The stored View State (scroll offset and selection, Q17/Q43).
    #[must_use]
    pub fn view_state(&self) -> &ViewState {
        &self.view_state
    }

    /// Replaces the View State.
    pub fn set_view_state(&mut self, view_state: ViewState) {
        self.view_state = view_state;
    }

    /// The style table, for inspection and tests.
    #[must_use]
    pub fn styles(&self) -> &SurfaceStyleTable {
        &self.styles
    }

    /// The fan-out state machine.
    #[must_use]
    pub fn publisher(&self) -> &Publisher {
        &self.publisher
    }

    /// Mutable access to the fan-out state machine.
    pub fn publisher_mut(&mut self) -> &mut Publisher {
        &mut self.publisher
    }

    /// Bytes the terminal program is owed in reply (DA, DSR, OSC colours…).
    ///
    /// The Server writes them to the PTY; taking them clears the buffer.
    pub fn take_pty_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pty_out)
    }

    // ------------------------------------------------------------------ input

    /// Feeds PTY output into the engine and accounts for what changed.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.engine.advance(bytes);
        self.cwd.feed(bytes);
        self.drain_events();

        let damage = self.engine.take_damage();
        if !damage.is_empty() {
            let rows = self.engine.rows() as usize;
            self.pending.dirty.union_with(&damage.to_dirty_set(rows));
            self.publisher.record_damage(&damage);
        }
        self.sync_derived_state();
    }

    /// Resizes the grid and the kernel's window size.
    ///
    /// Grilling Q40: last resize wins, history reflow stays off, and the
    /// selection is cleared.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if (cols, rows) == self.size() {
            return Ok(());
        }
        if let Some(pty) = &mut self.pty {
            pty.resize(cols, rows)?;
        }
        self.engine.resize(cols, rows);
        self.view_state.selection = None;

        self.pending.dirty.resize(rows as usize);
        self.pending.dirty.set_all();
        self.pending.resized = Some((cols, rows));
        self.publisher.record_resize(cols, rows);
        self.sync_derived_state();
        Ok(())
    }

    /// Hard reset (RIS or "Clear Scrollback"): the grid, the history and the
    /// style table all start over, so every Client needs a Snapshot.
    pub fn reset(&mut self) {
        self.engine.reset();
        self.styles.reset();
        let _ = self.styles.take_overflow();
        self.needs_snapshot = true;
        self.pending.dirty.set_all();
        self.publisher.force_snapshot_all();
        self.sync_derived_state();
    }

    /// Records that the child has exited and returns the wire event.
    ///
    /// Consumes a sequence number (`02-protocol.md` §4.3).
    pub fn set_exited(&mut self, status: ExitStatus) -> SurfaceExited {
        self.status = SurfaceStatus::Exited(status.clone());
        self.seq = self.seq.next();
        // `pump::reap` sends the event to every attached Client, so every
        // subscription's `since` cursor advances with it (ADR 0011).
        self.publisher.note_event_sent(self.seq);
        SurfaceExited {
            surface_id: self.id,
            seq: self.seq,
            status: status.into(),
        }
    }

    /// Polls the child without blocking; records the exit if it has happened.
    pub fn poll_exit(&mut self) -> Option<SurfaceExited> {
        if !self.status.is_running() {
            return None;
        }
        let status = self.pty.as_mut()?.try_wait().ok().flatten()?;
        Some(self.set_exited(status))
    }

    // ------------------------------------------------------------ frames

    /// `true` when the next frame must be a full Snapshot.
    #[must_use]
    pub fn needs_snapshot(&self) -> bool {
        self.needs_snapshot
    }

    /// `true` when something has changed since the last emitted frame.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.needs_snapshot
            || self.own_styles_generation != self.styles.generation()
            || !self.pending.is_empty()
    }

    /// `true` when the single-consumer cursor cannot describe the table any
    /// more (it was reset by overflow or [`Surface::reset`]).
    fn own_styles_stale(&self) -> bool {
        self.own_styles_generation != self.styles.generation()
    }

    /// Points the single-consumer cursor at the current end of the table.
    fn note_own_styles(&mut self) {
        self.own_styles_generation = self.styles.generation();
        self.own_styles_sent = u16::try_from(self.styles.len()).unwrap_or(u16::MAX);
    }

    /// Points `client`'s per-subscription cursor at the current end of the
    /// table. Called after its frame is built (ADR 0011).
    fn note_client_styles(&mut self, client: ClientId) {
        self.publisher
            .record_styles_sent(client, self.styles.generation(), self.styles.len());
    }

    /// Absolute id of the oldest retained history line.
    #[must_use]
    pub fn history_base(&self) -> AbsLine {
        self.engine.history_base()
    }

    /// Number of retained history lines, in [`AbsLine`] units (grilling Q39).
    #[must_use]
    pub fn history_len(&self) -> u64 {
        self.engine.history_len()
    }

    /// Renders a page of history, answering `FetchHistory` (§8).
    ///
    /// **Note:** [`st_proto::History`] carries no style table, so the rows
    /// reference the Surface's current one. A row can only reference an index
    /// the Client does not have yet in the window between a style-table reset
    /// and the forced Snapshot, and a Client renders an unknown index as the
    /// default style, so nothing can dangle.
    pub fn history(&mut self, from: AbsLine, count: u32) -> History {
        let rows = self.engine.history_lines(from, count, &mut self.styles);
        if self.styles.take_overflow() {
            // Rendering history can intern past the cap, which resets the
            // table under every subscription: the next frame on each must be
            // a Snapshot (ADR 0011). Until then a client renders an unknown
            // index as the default style, so nothing dangles.
            self.needs_snapshot = true;
            self.publisher.force_snapshot_all();
        }
        let base = self.engine.history_base();
        History {
            surface_id: self.id,
            from_line: AbsLine::new(from.get().max(base.get())),
            history_base: base,
            rows,
        }
    }

    /// Renders the whole Surface. Clears the "needs Snapshot" latch.
    ///
    /// A Snapshot sends the whole style table but never resets it, so it
    /// cannot invalidate another subscriber's indices (ADR 0011).
    pub fn snapshot(&mut self) -> Snapshot {
        if !self.pending.is_empty() || self.needs_snapshot {
            self.seq = self.seq.next();
            self.pending.clear();
        }
        self.needs_snapshot = false;
        let snapshot = self.build_snapshot(self.seq);
        self.note_own_styles();
        snapshot
    }

    /// Produces an incremental update, or `None` when nothing changed or a
    /// Snapshot is required (check [`Surface::needs_snapshot`]).
    pub fn take_delta(&mut self) -> Option<Delta> {
        match self.take_update() {
            SurfaceUpdate::Delta(delta) => Some(*delta),
            SurfaceUpdate::Idle | SurfaceUpdate::Snapshot(_) => None,
        }
    }

    /// The single-consumer form of [`Surface::flush`]: whatever one Client
    /// following this Surface would need next.
    pub fn take_update(&mut self) -> SurfaceUpdate {
        if !self.has_pending() {
            return SurfaceUpdate::Idle;
        }
        let seq = self.seq.next();
        if self.needs_snapshot || self.own_styles_stale() {
            self.needs_snapshot = false;
            self.pending.clear();
            self.seq = seq;
            let snapshot = self.build_snapshot(seq);
            self.note_own_styles();
            return SurfaceUpdate::Snapshot(Box::new(snapshot));
        }

        let dirty = std::mem::replace(
            &mut self.pending.dirty,
            crate::vt::DirtySet::new(self.engine.rows() as usize),
        );
        let title = self.pending.title;
        let resized = self.pending.resized;
        let since = self.seq;
        match self.build_delta(
            seq,
            since,
            self.own_styles_sent,
            self.own_styles_generation,
            &dirty,
            title,
            resized,
        ) {
            Some(delta) => {
                self.pending.clear();
                self.seq = seq;
                self.note_own_styles();
                SurfaceUpdate::Delta(Box::new(delta))
            }
            None => {
                // The style table reset mid-build (Q45) or before the build
                // (a `history` fetch crossed the cap): throw the frame away,
                // keep the damage, and resync.
                self.pending.dirty = dirty;
                self.needs_snapshot = false;
                self.pending.clear();
                self.seq = seq;
                let snapshot = self.build_snapshot(seq);
                self.note_own_styles();
                SurfaceUpdate::Snapshot(Box::new(snapshot))
            }
        }
    }

    // ------------------------------------------------------------ fan-out

    /// Subscribes a Client (§6, Attach). Returns `false` on a double attach.
    pub fn attach(&mut self, client: ClientId, mode: AttachMode, now: Instant) -> bool {
        self.publisher.attach(client, mode, now)
    }

    /// Answers a repeated `Attach` with a fresh Snapshot (ADR 0011): the
    /// attachment is kept and the mode may change. Returns `false` when the
    /// Client was never attached.
    pub fn resync(&mut self, client: ClientId, mode: AttachMode, now: Instant) -> bool {
        self.publisher.resync(client, mode, now)
    }

    /// Unsubscribes a Client.
    pub fn detach(&mut self, client: ClientId) -> bool {
        self.publisher.detach(client)
    }

    /// Records a Client's Ack (§6.5).
    pub fn ack(&mut self, client: ClientId, seq: Seq, now: Instant) {
        self.publisher.ack(client, seq, now);
    }

    /// `true` when [`Surface::flush`] would produce something.
    #[must_use]
    pub fn should_flush(&self, now: Instant) -> bool {
        self.publisher.should_flush(now)
    }

    /// Builds the frames every attached Client is owed at `now`.
    ///
    /// This is the composition of [`Publisher::flush`] with the frame
    /// builders; the Server just encodes and sends what comes back. Each
    /// frame's `since_seq` and `new_styles` come from that subscription's own
    /// cursor, and building one Client's frame never invalidates another's
    /// (ADR 0011).
    pub fn flush(&mut self, now: Instant) -> Vec<ClientFrame> {
        if !self.publisher.should_flush(now) {
            return Vec::new();
        }
        let seq = self.seq.next();
        let emissions = self.publisher.flush(now, seq, self.styles.generation());
        if emissions.is_empty() {
            return Vec::new();
        }
        self.seq = seq;
        self.pending.clear();
        self.needs_snapshot = false;

        let mut frames = Vec::with_capacity(emissions.len());
        let mut overflowed = false;
        for emission in emissions {
            if emission.bell {
                frames.push(ClientFrame {
                    client: emission.client,
                    msg: DataMsg::Bell(Bell {
                        surface_id: self.id,
                    }),
                });
            }
            match emission.kind {
                EmissionKind::BellOnly => {}
                EmissionKind::Snapshot => {
                    let snapshot = self.build_snapshot(seq);
                    self.note_client_styles(emission.client);
                    frames.push(ClientFrame {
                        client: emission.client,
                        msg: DataMsg::Snapshot(Box::new(snapshot)),
                    });
                }
                EmissionKind::Delta {
                    dirty,
                    title,
                    resized,
                    styles_from,
                    styles_generation,
                } => {
                    match self.build_delta(
                        seq,
                        emission.since,
                        styles_from,
                        styles_generation,
                        &dirty,
                        title,
                        resized,
                    ) {
                        Some(delta) => {
                            self.note_client_styles(emission.client);
                            frames.push(ClientFrame {
                                client: emission.client,
                                msg: DataMsg::Delta(Box::new(delta)),
                            });
                        }
                        None => {
                            overflowed = true;
                            let snapshot = self.build_snapshot(seq);
                            self.note_client_styles(emission.client);
                            frames.push(ClientFrame {
                                client: emission.client,
                                msg: DataMsg::Snapshot(Box::new(snapshot)),
                            });
                        }
                    }
                }
            }
        }
        if overflowed {
            // Everyone else's indices are stale too (Q45).
            self.publisher.force_snapshot_all();
        }
        frames
    }

    // ------------------------------------------------------------ internals

    fn build_snapshot(&mut self, seq: Seq) -> Snapshot {
        // A Snapshot carries the whole table but must not reset it: another
        // subscription may be mid-generation and hold these exact indices
        // (ADR 0011, A-Q7).
        let _ = self.styles.take_overflow();
        let mut grid = self.engine.snapshot(&mut self.styles);
        if self.styles.take_overflow() {
            // Over 4096 distinct styles on one screen: re-render once and
            // accept that the tail collapses onto the default style.
            grid = self.engine.snapshot(&mut self.styles);
            let _ = self.styles.take_overflow();
        }

        self.last_cursor = grid.cursor;
        self.last_modes = grid.modes;
        self.last_title.clone_from(&grid.title);
        self.last_history = (grid.history_base, grid.history_len);

        Snapshot {
            surface_id: self.id,
            seq,
            cols: grid.cols,
            rows: grid.rows,
            styles: self.styles.as_slice().to_vec(),
            grid: grid.grid,
            cursor: grid.cursor,
            modes: grid.modes,
            title: grid.title,
            history_base: grid.history_base,
            history_len: grid.history_len,
            view_state: self.view_state.clone(),
            exited: match &self.status {
                SurfaceStatus::Exited(status) => Some(status.clone().into()),
                SurfaceStatus::Running { .. } => None,
            },
        }
    }

    /// Builds a Delta, or `None` when the table's generation no longer
    /// matches the caller's cursor, or overflowed while packing rows (the
    /// caller must send a Snapshot instead).
    fn build_delta(
        &mut self,
        seq: Seq,
        since: Seq,
        styles_from: u16,
        styles_generation: u32,
        dirty: &crate::vt::DirtySet,
        title_changed: bool,
        resized: Option<(u16, u16)>,
    ) -> Option<Delta> {
        if self.styles.generation() != styles_generation {
            // The table reset after this Delta was chosen: its cursor names
            // indices of a dead generation.
            return None;
        }
        let mut rows: Vec<DirtyRow> = Vec::with_capacity(dirty.count());
        for index in dirty.iter() {
            let row: Row = self.engine.row(index as u16, &mut self.styles);
            rows.push(DirtyRow {
                index: index as u16,
                row,
            });
        }
        if self.styles.take_overflow() {
            // Interning a row crossed the cap: the table reset mid-build, so
            // the rows above may reference stale indices.
            return None;
        }
        let new_styles = self.styles.styles_from(styles_from as usize);
        let (cursor, modes) = self.engine.cursor_and_modes();
        let title = self.engine.title().to_owned();

        self.last_cursor = cursor;
        self.last_modes = modes;
        let title_field = if title_changed || title != self.last_title {
            self.last_title.clone_from(&title);
            Some(title)
        } else {
            None
        };
        let history_base = self.engine.history_base();
        let history_len = self.engine.history_len();
        self.last_history = (history_base, history_len);

        Some(Delta {
            surface_id: self.id,
            seq,
            since_seq: since,
            history_base,
            history_len,
            resized,
            new_styles,
            rows,
            cursor,
            modes,
            title: title_field,
        })
    }

    fn drain_events(&mut self) {
        let events = self.engine.drain_events();
        if events.is_empty() {
            return;
        }
        let (cols, rows) = (self.engine.cols(), self.engine.rows());
        for event in events {
            match event {
                VtEvent::Title(_) | VtEvent::ResetTitle => {
                    self.pending.title = true;
                    self.publisher.record_title();
                }
                VtEvent::Bell => {
                    self.pending.bell = true;
                    self.publisher.record_bell();
                }
                VtEvent::PtyWrite(bytes) => self.pty_out.extend_from_slice(&bytes),
                VtEvent::ColorRequest { index, reply } => {
                    if let Some(rgb) = self.palette.as_ref().and_then(|p| p(index)) {
                        self.pty_out.extend_from_slice(reply.format(rgb).as_bytes());
                    }
                }
                VtEvent::TextAreaSizeRequest { reply } => {
                    let size = TextAreaSize {
                        rows,
                        cols,
                        cell_width: 0,
                        cell_height: 0,
                    };
                    self.pty_out
                        .extend_from_slice(reply.format(size).as_bytes());
                }
                // OSC 52 is off in v1 (grilling Q48).
                VtEvent::ClipboardStore { .. } => {}
            }
        }
    }

    /// Diffs cursor, modes and history against the last emitted frame.
    fn sync_derived_state(&mut self) {
        let (cursor, modes) = self.engine.cursor_and_modes();
        if cursor != self.last_cursor {
            self.pending.cursor = true;
            self.publisher.record_cursor();
        }
        if modes != self.last_modes {
            self.pending.modes = true;
            self.publisher.record_modes();
        }
        let history = (self.engine.history_base(), self.engine.history_len());
        if history != self.last_history {
            self.pending.history = true;
            self.publisher.record_history();
        }
        if self.engine.title() != self.last_title {
            self.pending.title = true;
            self.publisher.record_title();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// The Server moves a Surface into a per-Surface task, so it must be
    /// `Send` (`docs/plan/01-architecture.md`).
    const fn _assert_send<T: Send>() {}
    const _: () = _assert_send::<Surface>();

    fn fixture() -> Surface {
        Surface::new(SurfaceConfig {
            engine: EngineConfig {
                cols: 20,
                rows: 4,
                default_title: "st".into(),
                ..EngineConfig::default()
            },
            pty: None,
            ..SurfaceConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn a_fresh_surface_is_idle_and_running() {
        let mut s = fixture();
        assert_eq!(s.seq(), Seq::FIRST);
        assert_eq!(s.size(), (20, 4));
        assert!(s.status().is_running());
        assert!(!s.has_pending());
        assert_eq!(s.take_update(), SurfaceUpdate::Idle);
        assert!(s.take_delta().is_none());
        assert_eq!(s.history_len(), 0);
        assert_eq!(s.history_base(), AbsLine::ZERO);
    }

    #[test]
    fn an_engine_only_surface_has_no_pty() {
        let mut s = fixture();
        assert!(s.pty().is_none());
        assert!(s.pty_mut().is_none());
        assert!(s.poll_exit().is_none());
        // The probe is a no-op without a pid and never panics.
        s.probe_cwd();
        assert!(s.cwd().is_absolute());
    }

    #[test]
    fn exiting_consumes_a_sequence_number() {
        let mut s = fixture();
        let before = s.seq();
        let event = s.set_exited(ExitStatus {
            code: Some(2),
            ..ExitStatus::default()
        });
        assert_eq!(event.seq.get(), before.get() + 1);
        assert_eq!(event.status.code, Some(2));
        assert!(!s.status().is_running());
    }

    #[test]
    fn resizing_to_the_same_size_is_a_no_op() {
        let mut s = fixture();
        s.resize(20, 4).unwrap();
        assert!(!s.has_pending());
        s.resize(21, 4).unwrap();
        assert!(s.has_pending());
    }

    #[test]
    fn a_palette_answers_colour_queries() {
        let mut s = fixture();
        s.feed(b"\x1b]11;?\x07");
        assert!(
            s.take_pty_replies().is_empty(),
            "with no palette the query goes unanswered"
        );

        s.set_palette(Some(Arc::new(|_index| {
            Some(Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        })));
        s.feed(b"\x1b]11;?\x07");
        let reply = String::from_utf8(s.take_pty_replies()).unwrap();
        assert!(
            reply.contains("rgb:1212/3434/5656"),
            "the program gets the palette colour back: {reply:?}"
        );
    }

    #[test]
    fn a_text_area_size_query_reports_the_grid() {
        let mut s = fixture();
        s.feed(b"\x1b[18t");
        let reply = String::from_utf8(s.take_pty_replies()).unwrap();
        assert!(reply.contains("4") && reply.contains("20"), "got {reply:?}");
    }

    // ------------------------------------------------- per-subscription fan-out
    // ADR 0011: sequencing and style visibility are per subscription, and a
    // repeated Attach is a Resync.

    use st_proto::{Color, Style, StyleIdx};

    const C1: ClientId = ClientId(1);
    const C2: ClientId = ClientId(2);

    fn snapshot_of(frames: &[ClientFrame], client: ClientId) -> &Snapshot {
        match &frames
            .iter()
            .find(|f| f.client == client)
            .unwrap_or_else(|| panic!("no frame for {client}"))
            .msg
        {
            DataMsg::Snapshot(snapshot) => snapshot,
            other => panic!("expected a Snapshot for {client}, got {other:?}"),
        }
    }

    fn delta_of(frames: &[ClientFrame], client: ClientId) -> &Delta {
        match &frames
            .iter()
            .find(|f| f.client == client)
            .unwrap_or_else(|| panic!("no frame for {client}"))
            .msg
        {
            DataMsg::Delta(delta) => delta,
            other => panic!("expected a Delta for {client}, got {other:?}"),
        }
    }

    fn red() -> Style {
        Style {
            fg: Color::Indexed(1),
            ..Style::DEFAULT
        }
    }

    fn green() -> Style {
        Style {
            fg: Color::Indexed(2),
            ..Style::DEFAULT
        }
    }

    /// R2: a Snapshot for one subscriber no longer resets the shared table, so
    /// the indices every other subscriber holds stay valid.
    #[test]
    fn a_snapshot_for_one_client_does_not_reset_the_table_for_another() {
        let mut s = fixture();
        let t0 = Instant::now();
        s.feed(b"\x1b[31mred");

        assert!(s.attach(C1, AttachMode::Active, t0));
        let frames = s.flush(t0);
        let first = snapshot_of(&frames, C1);
        let red_idx = first
            .styles
            .iter()
            .position(|style| *style == red())
            .expect("red was interned") as u16;
        let seq = first.seq;
        s.ack(C1, seq, t0);

        // A second Client attaches; its Snapshot must not renumber C1's table.
        let t1 = t0 + Duration::from_millis(20);
        assert!(s.attach(C2, AttachMode::Active, t1));
        let frames = s.flush(t1);
        assert_eq!(frames.len(), 1, "only C2 has anything pending");
        assert_eq!(
            snapshot_of(&frames, C2).styles.len(),
            s.styles().len(),
            "a Snapshot carries the whole table"
        );
        assert_eq!(
            s.styles().generation(),
            0,
            "the Snapshot for C2 did not reset the table"
        );
        assert_eq!(
            s.styles().get(StyleIdx::new(red_idx)),
            Some(red()),
            "C1's index still resolves to red"
        );
    }

    /// R2: a style interned while building one Client's Delta is still in the
    /// other Client's cursor range, so its next Delta carries it.
    #[test]
    fn a_style_interned_for_one_client_reaches_another_clients_next_delta() {
        let mut s = fixture();
        let t0 = Instant::now();
        assert!(s.attach(C1, AttachMode::Active, t0));
        assert!(s.attach(C2, AttachMode::Active, t0));
        let mut now = t0;
        let frames = s.flush(now);
        let seq = snapshot_of(&frames, C1).seq;
        assert_eq!(snapshot_of(&frames, C2).seq, seq);
        s.ack(C1, seq, now);
        s.ack(C2, seq, now);

        // Four Deltas fill C1's ack window; C2 acknowledges every frame.
        for row in 1..=4u16 {
            now += Duration::from_millis(20);
            s.feed(format!("\x1b[{row};1Hx").as_bytes());
            let frames = s.flush(now);
            assert_eq!(frames.len(), 2, "round {row}");
            s.ack(C2, delta_of(&frames, C2).seq, now);
        }
        assert!(
            s.publisher()
                .subscription(C1)
                .unwrap()
                .is_window_blocked(st_proto::MAX_UNACKED_DELTAS),
            "C1's window is full"
        );

        // While C1 is blocked, C2's Delta interns a new style.
        now += Duration::from_millis(20);
        s.feed(b"\x1b[32mgreen");
        let frames = s.flush(now);
        assert_eq!(frames.len(), 1, "C1 is window-blocked");
        assert_eq!(frames[0].client, C2);
        let c2 = delta_of(&frames, C2);
        assert!(
            c2.new_styles.iter().any(|(_, style)| *style == green()),
            "C2 got green: {:?}",
            c2.new_styles
        );
        let c2_seq = c2.seq;
        s.ack(C2, c2_seq, now);

        // C1 Acks; its next Delta still carries the style it never saw.
        let c1_last = s.publisher().subscription(C1).unwrap().last_sent_seq();
        s.ack(C1, c1_last, now);
        now += Duration::from_millis(20);
        s.feed(b" more");
        let frames = s.flush(now);
        let c1 = delta_of(&frames, C1);
        assert_eq!(c1.since_seq, c1_last, "no false Gap for C1");
        assert!(
            c1.new_styles.iter().any(|(_, style)| *style == green()),
            "the style interned while C1 was blocked must reach it: {:?}",
            c1.new_styles
        );
    }

    /// A8: a repeated Attach from an attached Client is a Resync, not an
    /// error, and the mode of the second Attach is honoured.
    #[test]
    fn a_repeated_attach_is_a_resync_snapshot() {
        let mut s = fixture();
        let t0 = Instant::now();
        assert!(s.attach(C1, AttachMode::Active, t0));
        let frames = s.flush(t0);
        s.ack(C1, snapshot_of(&frames, C1).seq, t0);

        assert!(
            !s.attach(C1, AttachMode::Active, t0 + Duration::from_millis(20)),
            "the second Attach reports an existing subscription"
        );
        assert!(
            s.resync(C1, AttachMode::Passive, t0 + Duration::from_millis(20)),
            "the Server turns it into a Resync"
        );
        let frames = s.flush(t0 + Duration::from_millis(20));
        assert_eq!(frames.len(), 1);
        assert!(
            matches!(frames[0].msg, DataMsg::Snapshot(_)),
            "a Resync is answered with a Snapshot"
        );
        assert_eq!(
            s.publisher().subscription(C1).unwrap().mode(),
            AttachMode::Passive,
            "the mode of the second Attach is honoured"
        );
    }

    /// `SurfaceExited` consumes a sequence number for the whole Surface; the
    /// next Delta must chain on it or the Client reports a false Gap.
    #[test]
    fn a_delta_after_surface_exited_chains_on_the_exit_sequence() {
        let mut s = fixture();
        let t0 = Instant::now();
        assert!(s.attach(C1, AttachMode::Active, t0));
        let frames = s.flush(t0);
        s.ack(C1, snapshot_of(&frames, C1).seq, t0);

        let exited = s.set_exited(ExitStatus {
            code: Some(0),
            ..ExitStatus::default()
        });
        s.feed(b"final words");
        let frames = s.flush(t0 + Duration::from_millis(20));
        let delta = delta_of(&frames, C1);
        assert_eq!(
            delta.since_seq, exited.seq,
            "the Delta after SurfaceExited must not look like a Gap"
        );
    }

    /// Q45: an overflow resets the table and forces a Snapshot to every
    /// subscriber, in the same flush.
    #[test]
    fn a_style_overflow_forces_a_snapshot_to_every_client() {
        let mut s = fixture();
        let t0 = Instant::now();
        assert!(s.attach(C1, AttachMode::Active, t0));
        assert!(s.attach(C2, AttachMode::Active, t0));
        let mut now = t0;
        let frames = s.flush(now);
        let seq = snapshot_of(&frames, C1).seq;
        s.ack(C1, seq, now);
        s.ack(C2, seq, now);

        let mut snapshots_for_both = 0;
        for i in 0..4_200u32 {
            let (r, g, b) = ((i >> 16) & 0xff, (i >> 8) & 0xff, i & 0xff);
            s.feed(format!("\x1b[1;1H\x1b[38;2;{r};{g};{b}mX").as_bytes());
            now += Duration::from_millis(10);
            let frames = s.flush(now);
            assert_eq!(frames.len(), 2, "both Clients are inside the window");
            if frames.iter().all(|f| matches!(f.msg, DataMsg::Snapshot(_))) {
                snapshots_for_both += 1;
            }
            for frame in &frames {
                let seq = match &frame.msg {
                    DataMsg::Snapshot(snapshot) => snapshot.seq,
                    DataMsg::Delta(delta) => delta.seq,
                    other => panic!("unexpected frame {other:?}"),
                };
                s.ack(frame.client, seq, now);
            }
        }

        assert!(
            (1..=2).contains(&snapshots_for_both),
            "the overflow forces a Snapshot to both subscribers, got {snapshots_for_both}"
        );
        assert_eq!(s.styles().generation(), 1, "the table reset exactly once");
        assert!(s.styles().len() < st_proto::STYLE_TABLE_CAP);
    }
}
