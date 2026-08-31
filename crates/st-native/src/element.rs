//! `<terminal-grid>` — the GPUI custom element (04 §3, §5–§9).
//!
//! # Shape
//!
//! One `div` carrying the `FocusHandle`, the key and pointer listeners and the
//! theme background, with a single `gpui::canvas` child that does all the
//! painting ([`crate::paint`]). Everything mutable lives in [`GridState`]
//! behind an `Rc<RefCell<…>>`, because gpui listeners are `'static` closures
//! that outlive the `&mut self` of one `render()`.
//!
//! # Events emitted
//!
//! `EventPayload` is one flat struct for every event type (gpuix's design), so
//! each of ours picks the fields that fit rather than serialising JSON:
//!
//! | event | fields |
//! |---|---|
//! | `title` | `value` = the title |
//! | `bell` | — |
//! | `exited` | `start_index` = exit code, `end_index` = signal |
//! | `selection` | `hovered` = has a non-empty selection |
//! | `scroll` | `start_index` = offset from bottom, `end_index` = content lines, `delta_y` = rows |
//! | `resize` | `start_index` = cols, `end_index` = rows |
//! | `modes` | `hovered` = alt screen, `precise` = bracketed paste, `is_held` = mouse reporting |
//! | `shortcut` | `key` = gpui key name, `value` = the chord, `modifiers` |

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Bounds, Pixels, Window};
use gpuix_native::{
    CustomElement, CustomElementFactory, CustomRenderContext, EventPayload, GpuixView,
};
use st_client_core::keys::{prepare_paste, Mods};
use st_client_core::mouse::{
    encode_mouse, handle_wheel, reports_to_program, MouseEncoding, MouseEvent, MouseProtocol,
    WheelAction,
};
use st_client_core::selection::{hit_edge, hit_test, AbsPoint};
use st_client_core::{DataPlaneEvent, DataPlaneHandle, Selection, SelectionMode};
use st_proto::{AbsLine, Modes, SurfaceId};

use crate::conn::SharedDataPlane;
use crate::geometry::{GridGeometry, ScrollbarThumb, SCROLLBAR_FADE_MS};
use crate::input::{chord_string, handle_key, mods_from_gpui, KeyOutcome};
use crate::mouse::{hit_zone, selection_mode_for, HitZone, MotionThrottle, WheelAccumulator};
use crate::paint::{paint_frame, RunKey, BLINK_MS};
use crate::props::{Command, GridProps, ScrollbarMode, SUPPORTED_EVENTS, SUPPORTED_PROPS};
use crate::registry::{GridSnapshot, StatsSnapshot};
use crate::runs::RunCache;
use crate::stats::FrameStats;
use crate::viewstate::{self, Trigger, ViewStateDebouncer};
use crate::wake::Waker;

/// Element type string React writes as `<terminal-grid />`.
pub const ELEMENT_TYPE: &str = "terminal-grid";

/// Most wheel reports one event may produce. A flung trackpad can accumulate
/// dozens of lines in a frame and xterm-style reporting has one press per
/// line; past this the program cannot tell the difference anyway.
const MAX_WHEEL_REPORTS: usize = 10;

/// gpuix's event callback. Named structurally so we never have to name the
/// `pub(crate)` alias inside `gpuix-native`.
type Emitter = Arc<dyn Fn(EventPayload) + Send + Sync>;

/// What a pointer drag is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    /// Extending the text selection.
    Selection,
    /// Dragging the scrollbar thumb; the value is the grab offset inside it.
    Scrollbar,
}

/// Everything one `<terminal-grid>` owns.
pub struct GridState {
    /// `@gpuix/react`'s element id.
    pub id: u64,
    /// The prop bag.
    pub props: GridProps,
    /// The shared socket, once `surfaceId` and `socketPath` are both set.
    pub plane: Option<Arc<SharedDataPlane>>,
    /// Cheap handle onto the same connection.
    pub handle: Option<DataPlaneHandle>,
    /// The Surface an `Attach` has been sent for.
    pub attached: Option<SurfaceId>,
    /// The socket path the current `plane` is on.
    pub connected_path: Option<String>,
    /// Last connection error, surfaced through `get_prop`.
    pub connect_error: Option<String>,

    /// Wakes GPUI from the Data Plane thread.
    pub waker: Option<Waker>,
    /// The task draining the wake channel; dropping it stops the drain.
    wake_task: Option<gpui::Task<()>>,
    /// The cursor blink ticker; `None` while unfocused, so a background
    /// terminal costs no frames (04 §6 step 6).
    blink_task: Option<gpui::Task<()>>,
    /// Reset to "on" by every keystroke.
    blink_epoch: Instant,

    /// Focus, either gpuix's (when React declared `onKeyDown`/`tabIndex`) or
    /// our own.
    pub focus: Option<gpui::FocusHandle>,
    /// `true` when the handle came from gpuix, so `focusElement` works.
    pub focus_is_gpuix: bool,
    /// Events React actually listens for.
    declared_events: HashSet<String>,
    /// gpuix's callback into JS.
    emitter: Option<Emitter>,

    /// Cell metrics, recomputed when the font key changes.
    pub cell: Option<crate::geometry::CellSize>,
    /// The font key the metrics were computed for.
    pub cell_font_key: Option<(String, u32, u32)>,
    /// Last painted geometry.
    pub geometry: GridGeometry,
    /// Last painted bounds, for pointer hit testing.
    pub bounds: Option<Bounds<Pixels>>,
    /// Shaped-line cache (04 §6 step 3).
    pub run_cache: RunCache<RunKey, gpui::ShapedLine>,
    /// Frame counters.
    pub stats: FrameStats,

    /// Distance from the bottom, in lines (Q25).
    pub scroll_offset: u64,
    /// The current selection.
    pub selection: Option<Selection>,
    /// What the pointer is doing.
    drag: Option<Drag>,
    /// Sub-line wheel remainder.
    wheel: WheelAccumulator,
    /// One mouse report per cell.
    motion: MotionThrottle,
    /// Last painted scrollbar thumb.
    pub scrollbar_thumb: Option<ScrollbarThumb>,
    /// Thumb is hovered, so it paints wider.
    pub scrollbar_hover: bool,
    /// When the pointer last moved, for `scrollbar: "auto"`.
    last_pointer_move: Option<Instant>,

    /// Copied out of the Replica each frame, for `get_prop`.
    pub title: String,
    /// Ditto.
    pub modes: Modes,
    /// Ditto.
    pub content_lines: u64,
    /// Ditto.
    pub viewport_top: u64,
    /// The Replica's own grid size, for the Q40 letterbox.
    pub replica_size: (u16, u16),
    /// The size the last `Resize` asked for.
    pub last_sent_size: Option<(u16, u16)>,
    /// The history page currently being fetched.
    pub pending_history: Option<u64>,

    /// `SetViewState` coalescing.
    debouncer: ViewStateDebouncer,
    /// Commands queued by `set_prop`, run at the top of the next frame.
    pending_commands: Vec<Command>,
    /// React set `focused` and the next frame has to act on it.
    pending_focus: bool,
    /// Element start, the clock the debouncer counts in.
    epoch: Instant,
    /// The last values published as events, so nothing is emitted twice.
    emitted: Emitted,
}

#[derive(Debug, Default)]
struct Emitted {
    title: Option<String>,
    modes: Option<Modes>,
    scroll: Option<(u64, u64)>,
    has_selection: Option<bool>,
}

impl Default for GridState {
    fn default() -> Self {
        Self {
            id: 0,
            props: GridProps::default(),
            plane: None,
            handle: None,
            attached: None,
            connected_path: None,
            connect_error: None,
            waker: None,
            wake_task: None,
            blink_task: None,
            blink_epoch: Instant::now(),
            focus: None,
            focus_is_gpuix: false,
            declared_events: HashSet::new(),
            emitter: None,
            cell: None,
            cell_font_key: None,
            geometry: GridGeometry::fit(
                0.0,
                0.0,
                crate::geometry::CellSize::new(1.0, 1.0),
                crate::props::Padding::default(),
            ),
            bounds: None,
            run_cache: RunCache::new(256),
            stats: FrameStats::default(),
            scroll_offset: 0,
            selection: None,
            drag: None,
            wheel: WheelAccumulator::default(),
            motion: MotionThrottle::default(),
            scrollbar_thumb: None,
            scrollbar_hover: false,
            last_pointer_move: None,
            title: String::new(),
            modes: Modes::empty(),
            content_lines: 0,
            viewport_top: 0,
            replica_size: (0, 0),
            last_sent_size: None,
            pending_history: None,
            debouncer: ViewStateDebouncer::default(),
            pending_commands: Vec::new(),
            pending_focus: false,
            epoch: Instant::now(),
            emitted: Emitted::default(),
        }
    }
}

impl GridState {
    /// The font for a run. Weight and slant come from the run's style key;
    /// family, features and fallbacks from the props.
    #[must_use]
    pub fn font(&self, bold: bool, italic: bool) -> gpui::Font {
        gpui::Font {
            family: self.props.font_family.clone().into(),
            features: gpui::FontFeatures::default(),
            fallbacks: None,
            weight: if bold {
                gpui::FontWeight::BOLD
            } else {
                gpui::FontWeight::NORMAL
            },
            style: if italic {
                gpui::FontStyle::Italic
            } else {
                gpui::FontStyle::Normal
            },
        }
    }

    /// The attached Surface, if any.
    #[must_use]
    pub fn surface(&self) -> Option<SurfaceId> {
        self.props.surface_id.map(SurfaceId)
    }

    /// `true` when this element holds keyboard focus.
    #[must_use]
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus
            .as_ref()
            .is_some_and(|handle| handle.is_focused(window))
    }

    /// The blink phase: on for the first half of each 530 ms period, counted
    /// from the last keystroke so typing never hides the cursor.
    #[must_use]
    pub fn blink_on(&self) -> bool {
        let elapsed = self.blink_epoch.elapsed().as_millis() as u64;
        (elapsed / BLINK_MS) % 2 == 0
    }

    /// `true` while `scrollbar: "auto"` should still paint.
    #[must_use]
    pub fn pointer_recently_moved(&self) -> bool {
        self.last_pointer_move
            .is_some_and(|at| at.elapsed() < Duration::from_millis(SCROLLBAR_FADE_MS))
    }

    /// Milliseconds since the element was created, the debouncer's clock.
    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    // ---------------------------------------------------------------- events

    fn emit(&self, event_type: &str, build: impl FnOnce(&mut EventPayload)) {
        if !self.declared_events.contains(event_type) {
            return;
        }
        let Some(emitter) = &self.emitter else {
            return;
        };
        let mut payload = EventPayload {
            element_id: self.id as f64,
            event_type: event_type.to_string(),
            ..EventPayload::default()
        };
        build(&mut payload);
        emitter(payload);
    }

    /// Emitted from the painter when the grid size changes.
    pub fn emit_resize(&self, cols: u16, rows: u16) {
        self.emit("resize", |payload| {
            payload.start_index = Some(f64::from(cols));
            payload.end_index = Some(f64::from(rows));
        });
    }

    /// Emits everything that changed since the previous frame.
    fn emit_changes(&mut self) {
        if self.emitted.title.as_deref() != Some(self.title.as_str()) {
            self.emitted.title = Some(self.title.clone());
            let title = self.title.clone();
            self.emit("title", |payload| payload.value = Some(title));
        }
        if self.emitted.modes != Some(self.modes) {
            self.emitted.modes = Some(self.modes);
            let modes = self.modes;
            self.emit("modes", |payload| {
                payload.hovered = Some(modes.contains(Modes::ALT_SCREEN));
                payload.precise = Some(modes.contains(Modes::BRACKETED_PASTE));
                payload.is_held = Some(modes.mouse_reporting());
            });
        }
        let scroll = (self.scroll_offset, self.content_lines);
        if self.emitted.scroll != Some(scroll) {
            self.emitted.scroll = Some(scroll);
            let rows = self.stats.rows;
            self.emit("scroll", |payload| {
                payload.start_index = Some(scroll.0 as f64);
                payload.end_index = Some(scroll.1 as f64);
                payload.delta_y = Some(f64::from(rows));
            });
        }
        let has_selection = self
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_empty());
        if self.emitted.has_selection != Some(has_selection) {
            self.emitted.has_selection = Some(has_selection);
            self.emit("selection", |payload| payload.hovered = Some(has_selection));
        }
    }

    /// Turns queued Data Plane events into React events.
    fn drain_plane_events(&mut self) {
        let Some(plane) = self.plane.clone() else {
            return;
        };
        plane.pump();
        for event in plane.take_events_for(self.surface()) {
            match event {
                DataPlaneEvent::Bell(_) => self.emit("bell", |_| {}),
                DataPlaneEvent::Exited { status, .. } => self.emit("exited", |payload| {
                    payload.start_index = status.code.map(f64::from);
                    payload.end_index = status.signal.map(f64::from);
                }),
                DataPlaneEvent::Detached { .. } => self.attached = None,
                DataPlaneEvent::Connected { .. } => {
                    self.connect_error = None;
                    // A fresh handshake re-attaches inside st-client-core, but
                    // the Server has no View State from us yet.
                    self.debouncer.reset();
                }
                DataPlaneEvent::Disconnected { reason } => {
                    self.connect_error = Some(reason);
                    self.attached = None;
                }
                DataPlaneEvent::Rejected(reject) => {
                    self.connect_error = Some(format!("{:?}", reject.reason));
                }
                // `st-client-core` requests the Snapshot itself; nothing to do.
                DataPlaneEvent::Gap(_) => {}
                DataPlaneEvent::Error(error) => {
                    self.connect_error = Some(error.message);
                }
            }
        }
    }

    // ------------------------------------------------------------ connection

    /// Opens (or reuses) the socket and attaches, on the first frame where
    /// both `surfaceId` and `socketPath` are set (04 §5).
    fn ensure_connection(&mut self) {
        let (Some(surface), Some(path)) = (self.surface(), self.props.socket_path.clone()) else {
            self.teardown();
            return;
        };

        if self.connected_path.as_deref() != Some(path.as_str()) {
            self.teardown();
            match crate::conn::open(&path, &self.props.build_id) {
                Ok(plane) => {
                    if let Some(waker) = &self.waker {
                        plane.register(self.id, waker.clone());
                    }
                    self.handle = Some(plane.handle());
                    self.plane = Some(plane);
                    self.connected_path = Some(path);
                    self.connect_error = None;
                }
                Err(error) => {
                    self.connect_error = Some(error);
                    return;
                }
            }
        }

        if self.attached != Some(surface) {
            if let Some(previous) = self.attached.take() {
                if let Some(handle) = &self.handle {
                    let _ = handle.detach(previous);
                }
            }
            let Some(handle) = &self.handle else {
                return;
            };
            if handle.attach(surface, self.props.attach_mode).is_ok() {
                self.attached = Some(surface);
                self.selection = None;
                self.scroll_offset = 0;
                self.last_sent_size = None;
                self.pending_history = None;
                self.debouncer.reset();
            }
        }
    }

    /// Detaches and drops our reference to the socket.
    fn teardown(&mut self) {
        if let (Some(handle), Some(surface)) = (&self.handle, self.attached.take()) {
            let _ = handle.detach(surface);
        }
        if let Some(plane) = &self.plane {
            plane.unregister(self.id);
        }
        self.plane = None;
        self.handle = None;
        self.connected_path = None;
    }

    // -------------------------------------------------------------- commands

    /// Runs the one-shot commands `set_prop` queued (04 §3, §9).
    fn run_commands(&mut self, window: &mut Window, cx: &mut gpui::App) {
        if std::mem::take(&mut self.pending_focus) {
            if let Some(handle) = self.focus.clone() {
                handle.focus(window, cx);
            }
        }
        for command in std::mem::take(&mut self.pending_commands) {
            match command.name.as_str() {
                "copy" => self.copy(cx),
                "paste" => self.paste(command.text.as_deref(), cx),
                "clearScrollback" => self.clear_scrollback(),
                "scrollToBottom" => self.scroll_to(0),
                "selectAll" => self.select_all(),
                "clearSelection" => {
                    self.selection = None;
                    self.report_view_state(Trigger::SelectionEnd);
                }
                "focus" => {
                    if let Some(handle) = self.focus.clone() {
                        handle.focus(window, cx);
                    }
                }
                other => tracing_warn(other),
            }
        }
    }

    fn selection_text(&self) -> String {
        let (Some(handle), Some(surface), Some(selection)) =
            (&self.handle, self.surface(), &self.selection)
        else {
            return String::new();
        };
        handle
            .with_replica(surface, |replica| {
                selection.text(replica, &self.props.selection_config)
            })
            .unwrap_or_default()
    }

    fn copy(&mut self, cx: &mut gpui::App) {
        let text = self.selection_text();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    /// Grilling Q48: on Linux the primary selection is written when a
    /// selection is made, so middle-click paste works. Never on macOS, where
    /// there is no primary selection and copy is explicit (Q24).
    fn write_primary(&self, cx: &mut gpui::App) {
        if !cfg!(target_os = "linux") {
            return;
        }
        let text = self.selection_text();
        if text.is_empty() {
            return;
        }
        cx.write_to_primary(gpui::ClipboardItem::new_string(text));
    }

    fn paste(&mut self, text: Option<&str>, cx: &mut gpui::App) {
        let text = match text {
            Some(text) => text.to_string(),
            None => match cx.read_from_clipboard().and_then(|item| item.text()) {
                Some(text) => text,
                None => return,
            },
        };
        // `prepare_paste` normalises newlines, brackets when the program asked
        // for it and strips an embedded ESC[201~ (04 §9); `send_input` splits
        // at MAX_INPUT_BYTES = 64 KiB.
        let bytes = prepare_paste(&text, self.modes, &self.props.key_config);
        self.send(&bytes);
        self.scroll_to(0);
    }

    fn clear_scrollback(&mut self) {
        let (Some(handle), Some(surface)) = (&self.handle, self.surface()) else {
            return;
        };
        handle.with_replica_mut(surface, |replica| replica.shrink_history_to(0));
        self.scroll_offset = 0;
        self.selection = None;
    }

    fn select_all(&mut self) {
        let (Some(handle), Some(surface)) = (&self.handle, self.surface()) else {
            return;
        };
        let Some((first, last, cols)) = handle.with_replica(surface, |replica| {
            (
                replica.history_base().get(),
                replica.first_visible_line().get() + u64::from(replica.rows()).saturating_sub(1),
                replica.cols(),
            )
        }) else {
            return;
        };
        let mut selection = Selection::new(
            AbsPoint {
                line: AbsLine::new(first),
                col: 0,
            },
            SelectionMode::Char,
        );
        selection.extend_to(AbsPoint {
            line: AbsLine::new(last),
            col: cols.saturating_sub(1),
        });
        self.selection = Some(selection);
        self.report_view_state(Trigger::SelectionEnd);
    }

    // ----------------------------------------------------------------- input

    /// Writes bytes to the PTY and jumps to the bottom (Q25).
    fn send(&mut self, bytes: &[u8]) {
        let (Some(handle), Some(surface)) = (&self.handle, self.surface()) else {
            return;
        };
        let _ = handle.send_input(surface, bytes);
    }

    fn scroll_to(&mut self, offset: u64) {
        let max = self
            .handle
            .as_ref()
            .zip(self.surface())
            .and_then(|(handle, surface)| {
                handle.with_replica(surface, st_client_core::Replica::max_scroll_offset)
            })
            .unwrap_or(0);
        let next = offset.min(max);
        if next != self.scroll_offset {
            self.scroll_offset = next;
            self.report_view_state(Trigger::Scroll);
        }
    }

    fn scroll_by_lines(&mut self, lines: i32) {
        let max = self
            .handle
            .as_ref()
            .zip(self.surface())
            .and_then(|(handle, surface)| {
                handle.with_replica(surface, st_client_core::Replica::max_scroll_offset)
            })
            .unwrap_or(0);
        let next = crate::geometry::scroll_by(self.scroll_offset, i64::from(lines), max);
        if next != self.scroll_offset {
            self.scroll_offset = next;
            self.report_view_state(Trigger::Scroll);
        }
    }

    /// Reports scroll offset and selection to the Server (Q43/Q49).
    fn report_view_state(&mut self, trigger: Trigger) {
        let Some(surface) = self.surface() else {
            return;
        };
        let message = viewstate::message(
            surface,
            AbsLine::new(self.viewport_top),
            self.selection.as_ref(),
        );
        if self.debouncer.should_send(self.now_ms(), &message, trigger) {
            if let Err(error) = viewstate::sink().send(message) {
                tracing::debug!(%error, "SetViewState was not delivered");
            }
        }
    }

    // ----------------------------------------------------------------- mouse

    /// Element-local pixel coordinates for a window position.
    fn local(&self, position: gpui::Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.bounds?;
        Some((
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        ))
    }

    /// The cell a pointer is inside.
    fn cell_at(&self, x: f32, y: f32) -> AbsPoint {
        hit_test(
            x,
            y,
            &self.geometry.hit_metrics(),
            AbsLine::new(self.viewport_top),
            self.geometry.cols.max(1),
            self.geometry.rows.max(1),
        )
    }

    /// The cell *boundary* nearest a pointer, for dragging.
    fn edge_at(&self, x: f32, y: f32) -> AbsPoint {
        hit_edge(
            x,
            y,
            &self.geometry.hit_metrics(),
            AbsLine::new(self.viewport_top),
            self.geometry.cols.max(1),
            self.geometry.rows.max(1),
        )
    }

    /// Grid coordinates a mouse report uses: 1-based, viewport-relative.
    fn report_cell(&self, x: f32, y: f32) -> (u16, u16) {
        let point = self.cell_at(x, y);
        let row = point
            .line
            .get()
            .saturating_sub(self.viewport_top)
            .min(u64::from(self.geometry.rows.saturating_sub(1))) as u16;
        (point.col + 1, row + 1)
    }

    /// Reports a wheel notch as button 64/65, one press per line, capped so a
    /// flung trackpad cannot put a hundred frames of input on the socket.
    fn report_wheel(&mut self, x: f32, y: f32, lines: i32, mods: Mods) {
        let cell = self.report_cell(x, y);
        let button = st_client_core::mouse::wheel_button(lines);
        let (protocol, encoding) = self.mouse_protocol();
        let count = (lines.unsigned_abs() as usize).min(MAX_WHEEL_REPORTS);
        let mut bytes = Vec::new();
        for _ in 0..count {
            if let Some(report) =
                encode_mouse(&MouseEvent::press(button, cell, mods), protocol, encoding)
            {
                bytes.extend_from_slice(&report);
            }
        }
        self.send(&bytes);
    }

    /// Scrolls while a selection drag sits against the top or bottom edge
    /// (04 §8). gpui only delivers `on_mouse_move` while the pointer is inside
    /// the element, so this is an edge *band* rather than true past-the-edge
    /// tracking; a pointer dragged right out of the window stops scrolling.
    fn autoscroll_drag(&mut self, y: f32) {
        let height = self
            .bounds
            .map_or(0.0, |bounds| f32::from(bounds.size.height));
        let step = autoscroll_step(y, height, self.geometry.cell.height);
        if step != 0 {
            self.scroll_by_lines(step);
        }
    }

    fn mouse_protocol(&self) -> (MouseProtocol, MouseEncoding) {
        (
            MouseProtocol::from_modes(self.modes),
            MouseEncoding::from_modes(self.modes),
        )
    }

    fn snap_selection(&mut self) {
        let (Some(handle), Some(surface), Some(selection)) =
            (&self.handle, self.surface(), self.selection.as_mut())
        else {
            return;
        };
        let config = &self.props.selection_config;
        let _ = handle.with_replica(surface, |replica| selection.snap(replica, config));
    }

    // ------------------------------------------------------------- publishing

    /// Publishes the read-back snapshot for `st_read_prop` (04 §3).
    fn publish(&self) {
        let (hits, misses) = self.run_cache.counters();
        crate::registry::publish(GridSnapshot {
            element_id: self.id,
            surface: self.surface(),
            handle: self.handle.clone(),
            selection: self.selection,
            selection_config: self.props.selection_config.clone(),
            scroll_offset: self.scroll_offset,
            content_lines: self.content_lines,
            viewport_top: self.viewport_top,
            title: self.title.clone(),
            cols: self.geometry.cols,
            rows: self.geometry.rows,
            cell_width: self.geometry.cell.width,
            cell_height: self.geometry.cell.height,
            connected: self
                .plane
                .as_ref()
                .is_some_and(|plane| plane.is_connected()),
            attached: self.attached.is_some(),
            modes: self.modes,
            stats: StatsSnapshot::of(&self.stats, hits, misses, self.run_cache.len()),
        });
    }
}

/// Lines to scroll for a drag at `y` inside an element `height` px tall:
/// `+1` towards history in the top band, `-1` towards the bottom in the bottom
/// band, `0` in between. The band is one cell tall.
#[must_use]
fn autoscroll_step(y: f32, height: f32, cell_height: f32) -> i32 {
    if height <= 0.0 || !y.is_finite() {
        return 0;
    }
    let band = cell_height.max(1.0);
    if y <= band {
        1
    } else if y >= height - band {
        -1
    } else {
        0
    }
}

fn tracing_warn(command: &str) {
    tracing::warn!(command, "unknown <terminal-grid> command");
}

/// The factory registered through gpuix's patched hook.
pub struct TerminalGridFactory;

impl CustomElementFactory for TerminalGridFactory {
    fn element_type(&self) -> &str {
        ELEMENT_TYPE
    }

    fn create(&self, id: u64) -> Box<dyn CustomElement> {
        Box::new(TerminalGridElement::new(id))
    }
}

/// The element itself: a handle onto [`GridState`].
pub struct TerminalGridElement {
    state: Rc<RefCell<GridState>>,
}

impl TerminalGridElement {
    /// A fresh element with the given gpuix id.
    #[must_use]
    pub fn new(id: u64) -> Self {
        Self {
            state: Rc::new(RefCell::new(GridState {
                id,
                ..GridState::default()
            })),
        }
    }

    /// The shared state, for tests.
    #[must_use]
    pub fn state(&self) -> &Rc<RefCell<GridState>> {
        &self.state
    }
}

impl CustomElement for TerminalGridElement {
    fn render(
        &mut self,
        ctx: CustomRenderContext,
        window: &mut Window,
        cx: &mut gpui::Context<GpuixView>,
    ) -> gpui::AnyElement {
        use gpui::prelude::*;

        let focus = {
            let mut state = self.state.borrow_mut();
            state.id = ctx.id;
            state.emitter = ctx.event_callback.clone();
            state.declared_events = ctx.events.clone();

            // Prefer gpuix's handle so `renderer.focusElement(id)` works; fall
            // back to our own when React declared no keyboard listeners.
            match ctx.focus_handle {
                Some(handle) if !state.focus_is_gpuix || state.focus.is_none() => {
                    state.focus = Some(handle.clone());
                    state.focus_is_gpuix = true;
                }
                None if state.focus.is_none() => {
                    state.focus = Some(cx.focus_handle());
                    state.focus_is_gpuix = false;
                }
                _ => {}
            }

            ensure_wake_task(&mut state, cx);
            state.ensure_connection();
            state.drain_plane_events();
            state.run_commands(window, cx);
            state.focus.clone().expect("a focus handle was just set")
        };

        let focused = focus.is_focused(window);
        ensure_blink_task(&self.state, focused, cx);

        let background = crate::theme::rgba(self.state.borrow().props.palette.bg);
        let mut element = gpui::div()
            .id(gpui::SharedString::from(format!(
                "__st_terminal_grid_{}",
                ctx.id
            )))
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&focus)
            .bg(background)
            .cursor(gpui::CursorStyle::IBeam);

        if let Some(style) = ctx.style {
            element = gpuix_native::apply_interactive_styles(element, style);
        }

        element = install_listeners(element, &self.state, &focus);
        element = element.child(canvas_for(&self.state));
        gpuix_native::automation::track_own_bounds(element, ctx.id).into_any_element()
    }

    fn set_prop(&mut self, key: &str, value: serde_json::Value) {
        let mut state = self.state.borrow_mut();
        let (effect, command) = state.props.set(key, &value);
        if effect.metrics {
            state.cell = None;
            state.cell_font_key = None;
            state.run_cache.clear();
            state.last_sent_size = None;
        }
        if effect.theme {
            // Colour is part of the shaped line's decoration runs.
            state.run_cache.clear();
        }
        if let Some(command) = command {
            state.pending_commands.push(command);
        }
        if effect.focus && state.props.focused == Some(true) {
            // `set_prop` has no `Window`, so the actual `focus()` happens at
            // the top of the next frame.
            state.pending_focus = true;
        }
        if !effect.is_noop() {
            if let Some(waker) = &state.waker {
                waker.wake();
            }
        }
        drop(state);
        self.state.borrow().publish();
    }

    fn supported_props(&self) -> &'static [&'static str] {
        SUPPORTED_PROPS
    }

    fn supported_events(&self) -> &'static [&'static str] {
        SUPPORTED_EVENTS
    }

    fn destroy(&mut self) {
        let mut state = self.state.borrow_mut();
        let id = state.id;
        state.teardown();
        state.wake_task = None;
        state.blink_task = None;
        state.run_cache.clear();
        crate::registry::retire(id);
    }
}

/// Starts the task that turns a Data Plane wake into a GPUI repaint (04 §5).
fn ensure_wake_task(state: &mut GridState, cx: &mut gpui::Context<GpuixView>) {
    if state.wake_task.is_some() {
        return;
    }
    let (waker, mut receiver) = crate::wake::channel();
    state.waker = Some(waker.clone());
    if let Some(plane) = &state.plane {
        plane.register(state.id, waker.clone());
    }
    state.wake_task = Some(cx.spawn(async move |view, cx| {
        use futures::StreamExt;
        while receiver.next().await.is_some() {
            // Clear the coalescing flag *before* notifying, so a Delta landing
            // during the frame schedules the next one (Q27).
            waker.armed();
            if view.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }
    }));
}

/// Starts or stops the 530 ms blink ticker (04 §6 step 6).
fn ensure_blink_task(
    state: &Rc<RefCell<GridState>>,
    focused: bool,
    cx: &mut gpui::Context<GpuixView>,
) {
    let want = focused && state.borrow().props.cursor_blink;
    if !want {
        state.borrow_mut().blink_task = None;
        return;
    }
    if state.borrow().blink_task.is_some() {
        return;
    }
    let task = cx.spawn(async move |view, cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(BLINK_MS))
            .await;
        if view.update(cx, |_, cx| cx.notify()).is_err() {
            break;
        }
    });
    state.borrow_mut().blink_task = Some(task);
}

/// The canvas that paints the grid.
fn canvas_for(state: &Rc<RefCell<GridState>>) -> impl gpui::IntoElement {
    use gpui::prelude::*;
    let state = Rc::clone(state);
    gpui::canvas(
        |_bounds, _window, _cx| (),
        move |bounds, (), window, cx| {
            let mut state = state.borrow_mut();
            if paint_frame(&mut state, bounds, window, cx) {
                state.emit_changes();
            }
            state.publish();
        },
    )
    .absolute()
    .size_full()
}

/// Every key and pointer listener the element needs.
fn install_listeners<E>(
    mut element: E,
    state: &Rc<RefCell<GridState>>,
    focus: &gpui::FocusHandle,
) -> E
where
    E: gpui::StatefulInteractiveElement,
{
    // ── keyboard ────────────────────────────────────────────────────
    {
        let state = Rc::clone(state);
        element = element.on_key_down(move |event, window, cx| {
            let mut grid = state.borrow_mut();
            let mods = mods_from_gpui(event.keystroke.modifiers);
            let key = event.keystroke.key.clone();
            let outcome = handle_key(
                &key,
                event.keystroke.key_char.as_deref(),
                mods,
                &grid.props.passthrough,
                grid.modes,
                &grid.props.key_config,
            );
            match outcome {
                KeyOutcome::Send(bytes) => {
                    grid.blink_epoch = Instant::now();
                    grid.send(&bytes);
                    // Q25: any key that produces input jumps to the bottom.
                    grid.scroll_to(0);
                    drop(grid);
                    // Consuming the event is what keeps GPUI from bubbling it
                    // to the React app root.
                    cx.stop_propagation();
                    window.refresh();
                }
                KeyOutcome::Passthrough => {
                    // HANDOVER V5, **verified on Linux/WSLg 2026-08-31** by
                    // `tests/passthrough-keys.tsx`: declining the event (no
                    // `stop_propagation`) does reach a React ancestor's
                    // `onKeyDown` — GPUI bubbles up the focus chain and
                    // gpuix's own `on_key_down` on the ancestor `div` fires.
                    // That is the mechanism in use. The `shortcut` event below
                    // is a redundant second channel: it is delivered, but
                    // `@gpuix/react`'s `EVENT_PROPS` has no `shortcut` entry,
                    // so JSX `onShortcut` is inert until upstream adds one and
                    // the listener has to be registered by hand today.
                    let chord = chord_string(mods, &key);
                    grid.emit("shortcut", |payload| {
                        payload.key = Some(key);
                        payload.value = Some(chord);
                        payload.modifiers = Some(gpuix_native::EventModifiers {
                            shift: mods.contains(Mods::SHIFT),
                            ctrl: mods.contains(Mods::CTRL),
                            alt: mods.contains(Mods::ALT),
                            cmd: mods.contains(Mods::SUPER),
                        });
                    });
                    // No `stop_propagation`: GPUI keeps bubbling up the focus
                    // chain to the React ancestor's `onKeyDown` (Q23).
                }
                KeyOutcome::Ignore => {}
            }
        });
    }

    // ── mouse down ──────────────────────────────────────────────────
    {
        let state = Rc::clone(state);
        let focus = focus.clone();
        element = element.on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
            focus.focus(window, cx);
            let mut grid = state.borrow_mut();
            let Some((x, y)) = grid.local(event.position) else {
                return;
            };
            grid.last_pointer_move = Some(Instant::now());
            let mods = mods_from_gpui(event.modifiers);

            let zone = hit_zone(
                x,
                y,
                grid.geometry.scrollbar_x(),
                grid.props.scrollbar != ScrollbarMode::Never
                    && grid.content_lines > u64::from(grid.geometry.rows),
                grid.scrollbar_thumb,
            );
            match zone {
                HitZone::ScrollbarThumb => {
                    grid.drag = Some(Drag::Scrollbar);
                }
                HitZone::ScrollbarTrack => {
                    let page = i32::from(grid.geometry.rows);
                    let above = grid.scrollbar_thumb.is_none_or(|thumb| y < thumb.y);
                    grid.scroll_by_lines(if above { page } else { -page });
                }
                HitZone::Cell => {
                    if reports_to_program(grid.modes, mods) {
                        let cell = grid.report_cell(x, y);
                        let (protocol, encoding) = grid.mouse_protocol();
                        if let Some(bytes) = encode_mouse(
                            &MouseEvent::press(
                                st_client_core::mouse::MouseButton::Left,
                                cell,
                                mods,
                            ),
                            protocol,
                            encoding,
                        ) {
                            grid.send(&bytes);
                        }
                    } else {
                        let mode = selection_mode_for(event.click_count, mods.contains(Mods::ALT));
                        let point = grid.cell_at(x, y);
                        grid.selection = Some(Selection::new(point, mode));
                        grid.snap_selection();
                        grid.drag = Some(Drag::Selection);
                    }
                }
            }
            drop(grid);
            window.refresh();
        });
    }

    // ── mouse move ──────────────────────────────────────────────────
    {
        let state = Rc::clone(state);
        element = element.on_mouse_move(move |event, window, _cx| {
            let mut grid = state.borrow_mut();
            let Some((x, y)) = grid.local(event.position) else {
                return;
            };
            grid.last_pointer_move = Some(Instant::now());
            grid.scrollbar_hover = x >= grid.geometry.scrollbar_x();

            match grid.drag {
                Some(Drag::Selection) => {
                    let head = grid.edge_at(x, y);
                    if let Some(selection) = grid.selection.as_mut() {
                        selection.extend_to(head);
                    }
                    grid.snap_selection();
                    grid.autoscroll_drag(y);
                }
                Some(Drag::Scrollbar) => {
                    let height = grid.bounds.map_or(0.0, |b| f32::from(b.size.height));
                    let thumb_height = grid.scrollbar_thumb.map_or(0.0, |thumb| thumb.height);
                    let offset = crate::geometry::scroll_offset_for_thumb(
                        height,
                        thumb_height,
                        grid.geometry.rows,
                        grid.content_lines,
                        y - thumb_height / 2.0,
                    );
                    grid.scroll_to(offset);
                }
                None => {
                    let mods = mods_from_gpui(event.modifiers);
                    if reports_to_program(grid.modes, mods) {
                        let cell = grid.report_cell(x, y);
                        if grid.motion.should_report(cell) {
                            let button = event
                                .pressed_button
                                .and_then(crate::mouse::button_from_gpui)
                                .unwrap_or(st_client_core::mouse::MouseButton::None);
                            let (protocol, encoding) = grid.mouse_protocol();
                            if let Some(bytes) = encode_mouse(
                                &MouseEvent::motion(button, cell, mods),
                                protocol,
                                encoding,
                            ) {
                                grid.send(&bytes);
                            }
                        }
                    }
                }
            }
            drop(grid);
            window.refresh();
        });
    }

    // ── mouse up, inside and outside ────────────────────────────────
    for outside in [false, true] {
        let state = Rc::clone(state);
        let handler = move |event: &gpui::MouseUpEvent, window: &mut Window, cx: &mut gpui::App| {
            let mut grid = state.borrow_mut();
            let drag = grid.drag.take();
            grid.motion.reset();
            if let Some((x, y)) = grid.local(event.position) {
                let mods = mods_from_gpui(event.modifiers);
                if drag.is_none() && reports_to_program(grid.modes, mods) {
                    let cell = grid.report_cell(x, y);
                    let (protocol, encoding) = grid.mouse_protocol();
                    if let Some(bytes) = encode_mouse(
                        &MouseEvent::release(st_client_core::mouse::MouseButton::Left, cell, mods),
                        protocol,
                        encoding,
                    ) {
                        grid.send(&bytes);
                    }
                }
            }
            if drag == Some(Drag::Selection) {
                // Q48: the Linux primary selection is written on select.
                grid.write_primary(cx);
                grid.report_view_state(Trigger::SelectionEnd);
            }
            drop(grid);
            window.refresh();
        };
        element = if outside {
            element.on_mouse_up_out(gpui::MouseButton::Left, handler)
        } else {
            element.on_mouse_up(gpui::MouseButton::Left, handler)
        };
    }

    // ── wheel ───────────────────────────────────────────────────────
    {
        let state = Rc::clone(state);
        element = element.on_scroll_wheel(move |event, window, _cx| {
            let mut grid = state.borrow_mut();
            let line_height = grid.geometry.cell.height;
            let lines_per_notch = grid.props.wheel_config.lines_per_notch;
            let lines = match event.delta {
                gpui::ScrollDelta::Pixels(delta) => {
                    grid.wheel.push_pixels(f32::from(delta.y), line_height)
                }
                gpui::ScrollDelta::Lines(delta) => grid.wheel.push_lines(delta.y, lines_per_notch),
            };
            if lines == 0 {
                return;
            }
            let mods = mods_from_gpui(event.modifiers);
            let config = grid.props.wheel_config;
            match handle_wheel(lines, mods, grid.modes, &config) {
                WheelAction::Scroll(lines) => grid.scroll_by_lines(lines),
                WheelAction::Send(bytes) => grid.send(&bytes),
                // `handle_wheel` deliberately declines when the program is
                // reading the mouse: only the renderer knows which cell the
                // pointer is over, so the wheel report is built here (04 §8).
                WheelAction::None => {
                    if reports_to_program(grid.modes, mods) {
                        if let Some((x, y)) = grid.local(event.position) {
                            grid.report_wheel(x, y, lines, mods);
                        }
                    }
                }
            }
            drop(grid);
            window.refresh();
        });
    }

    element
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_factory_answers_to_the_documented_element_type() {
        assert_eq!(TerminalGridFactory.element_type(), "terminal-grid");
        assert_eq!(ELEMENT_TYPE, "terminal-grid");
    }

    #[test]
    fn a_fresh_element_publishes_nothing_and_supports_the_documented_props() {
        let element = TerminalGridElement::new(7);
        assert_eq!(element.supported_props(), SUPPORTED_PROPS);
        assert_eq!(element.supported_events(), SUPPORTED_EVENTS);
        assert_eq!(element.state().borrow().id, 7);
    }

    #[test]
    fn props_reach_the_state_through_set_prop() {
        let mut element = TerminalGridElement::new(1);
        element.set_prop("fontSize", json!(18));
        element.set_prop("lineHeight", json!(1.5));
        element.set_prop("theme", json!({ "bg": "#000102" }));
        element.set_prop("passthroughKeys", json!(["cmd-t"]));
        let state = element.state().borrow();
        assert_eq!(state.props.font_size, 18.0);
        assert_eq!(state.props.line_height, 1.5);
        assert_eq!(state.props.palette.bg, (0, 1, 2));
        assert!(state.props.passthrough.contains(Mods::SUPER, "t"));
    }

    #[test]
    fn a_metrics_change_drops_the_shaped_run_cache() {
        let mut element = TerminalGridElement::new(2);
        element
            .state()
            .borrow_mut()
            .run_cache
            .insert(dummy_key(), gpui::ShapedLine::default());
        assert_eq!(element.state().borrow().run_cache.len(), 1);
        element.set_prop("fontSize", json!(22));
        assert!(element.state().borrow().run_cache.is_empty());
    }

    #[test]
    fn a_theme_change_drops_the_shaped_run_cache_too() {
        let mut element = TerminalGridElement::new(3);
        element
            .state()
            .borrow_mut()
            .run_cache
            .insert(dummy_key(), gpui::ShapedLine::default());
        element.set_prop("theme", json!({ "fg": "#abcdef" }));
        assert!(element.state().borrow().run_cache.is_empty());
    }

    #[test]
    fn commands_queue_until_a_frame_can_run_them() {
        let mut element = TerminalGridElement::new(4);
        element.set_prop("command", json!({ "seq": 1, "name": "copy" }));
        assert_eq!(element.state().borrow().pending_commands.len(), 1);
        element.set_prop("command", json!({ "seq": 1, "name": "copy" }));
        assert_eq!(element.state().borrow().pending_commands.len(), 1);
        element.set_prop("command", json!({ "seq": 2, "name": "scrollToBottom" }));
        assert_eq!(element.state().borrow().pending_commands.len(), 2);
    }

    #[test]
    fn destroying_an_element_retires_its_snapshot() {
        // Surface ids are unique per test: the registry is process-global and
        // `cargo test` runs these in parallel.
        let mut element = TerminalGridElement::new(5);
        element.set_prop("surfaceId", json!(4242));
        assert!(crate::registry::snapshot_for_surface(SurfaceId(4242)).is_some());
        element.destroy();
        assert!(crate::registry::snapshot_for_surface(SurfaceId(4242)).is_none());
    }

    #[test]
    fn the_blink_phase_flips_every_period() {
        let mut state = GridState::default();
        assert!(state.blink_on());
        state.blink_epoch = Instant::now() - Duration::from_millis(BLINK_MS + 10);
        assert!(!state.blink_on());
        state.blink_epoch = Instant::now() - Duration::from_millis(BLINK_MS * 2 + 10);
        assert!(state.blink_on());
    }

    #[test]
    fn the_auto_scrollbar_fades_after_the_pointer_stops() {
        let mut state = GridState::default();
        assert!(!state.pointer_recently_moved());
        state.last_pointer_move = Some(Instant::now());
        assert!(state.pointer_recently_moved());
        state.last_pointer_move =
            Some(Instant::now() - Duration::from_millis(SCROLLBAR_FADE_MS + 100));
        assert!(!state.pointer_recently_moved());
    }

    #[test]
    fn a_grid_with_no_socket_reports_no_selection_text_instead_of_panicking() {
        let state = GridState::default();
        assert_eq!(state.selection_text(), "");
        assert_eq!(state.surface(), None);
    }

    #[test]
    fn report_cells_are_one_based() {
        let state = GridState {
            geometry: GridGeometry::fit(
                800.0,
                400.0,
                crate::geometry::CellSize::new(8.0, 16.0),
                crate::props::Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                },
            ),
            ..GridState::default()
        };
        assert_eq!(state.report_cell(0.0, 0.0), (1, 1));
        assert_eq!(state.report_cell(8.0, 16.0), (2, 2));
        assert_eq!(state.report_cell(23.9, 16.0), (3, 2));
    }

    #[test]
    fn a_drag_against_an_edge_scrolls_and_the_middle_does_not() {
        // 400 px tall, 16 px cells: the top and bottom 16 px are the bands.
        assert_eq!(autoscroll_step(0.0, 400.0, 16.0), 1);
        assert_eq!(autoscroll_step(16.0, 400.0, 16.0), 1);
        assert_eq!(autoscroll_step(17.0, 400.0, 16.0), 0);
        assert_eq!(autoscroll_step(200.0, 400.0, 16.0), 0);
        assert_eq!(autoscroll_step(383.0, 400.0, 16.0), 0);
        assert_eq!(autoscroll_step(384.0, 400.0, 16.0), -1);
        assert_eq!(autoscroll_step(400.0, 400.0, 16.0), -1);
    }

    #[test]
    fn autoscroll_is_inert_before_the_first_paint() {
        assert_eq!(autoscroll_step(10.0, 0.0, 16.0), 0);
        assert_eq!(autoscroll_step(f32::NAN, 400.0, 16.0), 0);
    }

    fn dummy_key() -> RunKey {
        RunKey {
            text: "x".to_string(),
            style: crate::runs::StyleKey {
                fg: (0, 0, 0),
                underline_color: (0, 0, 0),
                bold: false,
                italic: false,
                underline: 0,
                strike: false,
            },
            forced: true,
        }
    }
}
