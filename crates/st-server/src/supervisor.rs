//! The Surface supervisor — the data plane's half of `03-server.md` §4.
//!
//! One [`SurfaceSupervisor`] owns every live [`st_core::Surface`] in the
//! daemon, keyed by [`SurfaceId`], plus the table of data-plane connections
//! that frames are fanned out to. It is the concrete
//! [`SurfaceSpawner`](crate::workspace::SurfaceSpawner) the Workspace actor
//! drives, so `surface.create` really does open a PTY and `surface.kill`
//! really does `killpg`.
//!
//! ```text
//!   reader thread ──64 KiB──► mpsc(16) ──► feed task ──► Surface::feed
//!        ▲                                     │
//!        │                                     └──► take_pty_replies ──┐
//!   PTY master                                                         │
//!        ▼                                                             │
//!   writer thread ◄────────────── mpsc ◄── Input from a Client ◄───────┘
//!
//!   pump (120 Hz) ──► Surface::flush ──► ClientFrame ──► conn out channel
//! ```
//!
//! **Threading.** Each Surface gets two blocking `std::thread`s (a reader and
//! a writer, as `03-server.md` §4 prescribes) and one tokio task that owns the
//! feed loop. The child is reaped by [`Surface::poll_exit`] from the pump
//! rather than by a third blocking thread: `portable_pty::Child::wait` needs
//! `&mut Pty`, which lives inside the `Surface`, and a `try_wait` driven by
//! the reader's EOF is both simpler and race-free.
//!
//! **Locking.** A Surface is behind a `std::sync::Mutex`; every critical
//! section is synchronous and short (feed a chunk, build frames), and no lock
//! is ever held across an `.await`. The connection registry is a second mutex,
//! always taken *after* the Surface lock is released — never nested — so the
//! two can never deadlock.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use st_core::pty::{Pty, PtyConfig};
use st_core::publisher::{ClientId, PublisherConfig};
use st_core::surface::{Surface, SurfaceConfig, SurfaceStatus};
use st_core::vt::alacritty::EngineConfig;
use st_proto::control::KillSignal;
use st_proto::{DataMsg, DetachReason, Detached, SurfaceId, ViewState};
use tokio::sync::{mpsc, Notify};

use crate::metrics::Metrics;
use crate::workspace::spawn::{SpawnError, SpawnSpec, SpawnedSurface, SurfaceSpawner};
use crate::workspace::{SurfaceEvent, WorkspaceHandle};

/// Bytes read from a PTY in one `read(2)` (`03-server.md` §4).
pub const PTY_READ_CHUNK: usize = 64 * 1024;

/// Chunks the reader thread may queue before the kernel PTY buffer becomes the
/// backpressure (`03-server.md` §4: bounded `mpsc`, cap 16).
pub const PTY_QUEUE_DEPTH: usize = 16;

/// Frames one connection may have queued for its socket before it is judged
/// unable to keep up. The ack window already bounds state frames to four per
/// Surface, so reaching this means the socket itself has stopped draining.
pub const DEFAULT_OUTBOUND_CAPACITY: usize = 256;

// --------------------------------------------------------------------- events

/// Something the data plane must report to the Workspace actor.
///
/// [`SurfaceEvent`] covers title, cwd, resize, foreground child, input and
/// exit; View State is separate because grilling Q43/Q49 routes it through
/// [`WorkspaceHandle::set_view_state`], the same command the control plane's
/// `view.set` uses, so both planes bump the revision identically.
#[derive(Debug, Clone)]
pub enum Upcall {
    /// A plain Surface change.
    Surface(SurfaceEvent),
    /// A View State edit that must be echoed on `ev.workspace`.
    ViewState {
        /// Which Surface.
        surface: SurfaceId,
        /// The stored View State.
        view: ViewState,
        /// The connection that caused it, so the actor can skip echoing to it.
        origin: Option<crate::workspace::ClientId>,
    },
}

/// The seam between the data plane and the Workspace actor.
///
/// The supervisor never awaits the actor: it reports upward through this
/// trait, which must not block. [`HandleNotifier`] is the production wiring;
/// tests use [`NullNotifier`] or [`RecordingNotifier`].
pub trait WorkspaceNotifier: Send + Sync + 'static {
    /// Reports one change. Must return immediately.
    fn notify(&self, upcall: Upcall);
}

/// A notifier that throws everything away — for a supervisor with no actor.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullNotifier;

impl WorkspaceNotifier for NullNotifier {
    fn notify(&self, _upcall: Upcall) {}
}

/// Forwards every [`Upcall`] into the Workspace actor.
///
/// The forwarding task exists so the pump and the connection tasks stay
/// synchronous: they push into an unbounded queue, one task awaits the actor's
/// bounded command channel.
#[derive(Debug, Clone)]
pub struct HandleNotifier(mpsc::UnboundedSender<Upcall>);

impl HandleNotifier {
    /// Starts the forwarding task. Must be called inside a tokio runtime.
    #[must_use]
    pub fn new(handle: WorkspaceHandle) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Upcall>();
        tokio::spawn(async move {
            while let Some(upcall) = rx.recv().await {
                // An error here only happens once the actor has stopped.
                if forward(&handle, upcall).await.is_err() {
                    break;
                }
            }
        });
        Self(tx)
    }
}

impl WorkspaceNotifier for HandleNotifier {
    fn notify(&self, upcall: Upcall) {
        let _ = self.0.send(upcall);
    }
}

/// A notifier that buffers until the Workspace actor exists.
///
/// Start-up is circular: [`ServerBuilder`](crate::lifecycle::ServerBuilder)
/// needs the supervisor as its [`SurfaceSpawner`] *before* it can build the
/// actor whose handle the supervisor reports to. This breaks the cycle —
/// upcalls made during re-seed queue up and are delivered the moment
/// [`DeferredNotifier::attach`] runs.
#[derive(Debug)]
pub struct DeferredNotifier {
    tx: mpsc::UnboundedSender<Upcall>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<Upcall>>>,
}

impl Default for DeferredNotifier {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }
}

impl DeferredNotifier {
    /// An unattached notifier.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Starts forwarding into `handle`, replaying anything buffered so far.
    /// A second call is a no-op.
    pub fn attach(&self, handle: WorkspaceHandle) {
        let Some(mut rx) = self.rx.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        tokio::spawn(async move {
            while let Some(upcall) = rx.recv().await {
                if forward(&handle, upcall).await.is_err() {
                    break;
                }
            }
        });
    }
}

impl WorkspaceNotifier for DeferredNotifier {
    fn notify(&self, upcall: Upcall) {
        let _ = self.tx.send(upcall);
    }
}

/// Applies one [`Upcall`] to the Workspace actor.
async fn forward(
    handle: &WorkspaceHandle,
    upcall: Upcall,
) -> Result<(), crate::workspace::actor::ActorGone> {
    match upcall {
        Upcall::Surface(event) => handle.surface_event(event).await,
        Upcall::ViewState {
            surface,
            view,
            origin,
        } => {
            if let Err(err) = handle
                .set_view_state(
                    surface,
                    Some(view.scroll_offset),
                    Some(view.selection),
                    origin,
                )
                .await
            {
                tracing::debug!(%surface, ?err, "view.set from the data plane failed");
            }
            Ok(())
        }
    }
}

/// Keeps every upcall in a vector, for tests.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    upcalls: Mutex<Vec<Upcall>>,
}

impl RecordingNotifier {
    /// An empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything recorded so far, in order.
    #[must_use]
    pub fn upcalls(&self) -> Vec<Upcall> {
        self.upcalls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl WorkspaceNotifier for RecordingNotifier {
    fn notify(&self, upcall: Upcall) {
        self.upcalls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(upcall);
    }
}

// --------------------------------------------------------------------- config

/// Tunables the supervisor takes from `[server]` and `[shell]`.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Retained history lines per Surface.
    pub scrollback_lines: usize,
    /// Fan-out tunables handed to every Surface's `Publisher`.
    pub publisher: PublisherConfig,
    /// Exported to children as `TERM_PROGRAM_VERSION`.
    pub build_id: String,
    /// Exported to children as `SUPERTERMINAL_SOCKET`.
    pub socket_path: Option<PathBuf>,
    /// How long a Surface may take to die after `SIGHUP` before `SIGKILL`
    /// (`03-server.md` §2).
    pub kill_grace: Duration,
    /// How often the `/proc` cwd probe and the foreground-pgid sample run.
    pub sample_interval: Duration,
    /// Frames one connection may have queued for its socket.
    pub outbound_capacity: usize,
}

impl SupervisorConfig {
    /// Reads the tunables the supervisor cares about out of `config.toml`
    /// (`03-server.md` §2: the daemon reads `[server]`, `[shell]` and
    /// `[terminal]`).
    #[must_use]
    pub fn from_config(
        config: &st_config::Config,
        build_id: impl Into<String>,
        socket_path: Option<PathBuf>,
    ) -> Self {
        Self {
            scrollback_lines: config.terminal.scrollback_lines,
            build_id: build_id.into(),
            socket_path,
            ..Self::default()
        }
    }
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            publisher: PublisherConfig::default(),
            build_id: String::new(),
            socket_path: None,
            kill_grace: Duration::from_secs(2),
            sample_interval: Duration::from_secs(1),
            outbound_capacity: DEFAULT_OUTBOUND_CAPACITY,
        }
    }
}

// ----------------------------------------------------------------------- slot

/// One live Surface plus the handles the async world needs to reach it.
pub struct SurfaceSlot {
    id: SurfaceId,
    surface: Mutex<Surface>,
    input: mpsc::UnboundedSender<Vec<u8>>,
    /// Set by the feed task when the PTY reaches EOF: the child is gone, so
    /// the pump reaps it on the very next tick instead of at the next sample.
    exit_hint: AtomicBool,
    /// Set once the exit has been observed and fanned out.
    exit_reported: AtomicBool,
    /// The last title reported upward, so the pump only notifies on change.
    last_title: Mutex<String>,
    /// The last `has_foreground_child` reported upward.
    last_busy: AtomicBool,
}

impl std::fmt::Debug for SurfaceSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceSlot").field("id", &self.id).finish()
    }
}

impl SurfaceSlot {
    /// The Surface's id.
    #[must_use]
    pub fn id(&self) -> SurfaceId {
        self.id
    }

    /// Locks the Surface. The guard must never be held across an `.await`.
    ///
    /// A poisoned lock is recovered rather than propagated: a panic in one
    /// frame builder must not take the whole Surface table down.
    pub fn lock(&self) -> MutexGuard<'_, Surface> {
        self.surface.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues bytes for the PTY writer thread. `false` when there is no PTY.
    pub fn write_pty(&self, bytes: Vec<u8>) -> bool {
        !bytes.is_empty() && self.input.send(bytes).is_ok()
    }

    /// `true` once the PTY reader has seen EOF.
    #[must_use]
    pub fn exit_hint(&self) -> bool {
        self.exit_hint.load(Ordering::Relaxed)
    }

    fn set_exit_hint(&self) {
        self.exit_hint.store(true, Ordering::Relaxed);
    }

    /// `true` when the exit has already been fanned out.
    #[must_use]
    pub fn exit_reported(&self) -> bool {
        self.exit_reported.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_exit_reported(&self) {
        self.exit_reported.store(true, Ordering::Relaxed);
    }

    pub(crate) fn take_title_change(&self, title: &str) -> Option<String> {
        let mut last = self.last_title.lock().unwrap_or_else(|e| e.into_inner());
        if *last == title {
            return None;
        }
        title.clone_into(&mut last);
        Some(last.clone())
    }

    pub(crate) fn take_busy_change(&self, busy: bool) -> Option<bool> {
        if self.last_busy.swap(busy, Ordering::Relaxed) == busy {
            None
        } else {
            Some(busy)
        }
    }
}

// --------------------------------------------------------------- connections

struct ConnHandle {
    out: mpsc::Sender<Vec<u8>>,
    shutdown: Arc<Notify>,
}

#[derive(Default)]
struct Registry {
    surfaces: BTreeMap<SurfaceId, Arc<SurfaceSlot>>,
    conns: BTreeMap<ClientId, ConnHandle>,
}

// ----------------------------------------------------------------- supervisor

/// Owns every live Surface and every data-plane connection.
pub struct SurfaceSupervisor {
    registry: Mutex<Registry>,
    config: SupervisorConfig,
    notifier: Arc<dyn WorkspaceNotifier>,
    metrics: Arc<std::sync::OnceLock<Arc<Metrics>>>,
    next_surface: AtomicU32,
    runtime: Option<tokio::runtime::Handle>,
    pump_started: AtomicBool,
}

impl std::fmt::Debug for SurfaceSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceSupervisor")
            .field("surfaces", &self.surface_ids())
            .field("clients", &self.client_count())
            .finish()
    }
}

impl SurfaceSupervisor {
    /// Builds a supervisor. Call it from inside the tokio runtime that will
    /// own the Surface tasks; the handle is captured here, so
    /// [`SurfaceSpawner::spawn`] works afterwards from the actor task or from
    /// any blocking thread.
    #[must_use]
    pub fn new(config: SupervisorConfig, notifier: Arc<dyn WorkspaceNotifier>) -> Self {
        Self {
            registry: Mutex::new(Registry::default()),
            config,
            notifier,
            metrics: Arc::new(std::sync::OnceLock::new()),
            next_surface: AtomicU32::new(1),
            runtime: tokio::runtime::Handle::try_current().ok(),
            pump_started: AtomicBool::new(false),
        }
    }

    /// Shares the daemon's counters, so `server.status` sees PTY and frame
    /// traffic (§11).
    #[must_use]
    pub fn with_metrics(self, metrics: Arc<Metrics>) -> Self {
        self.install_metrics(metrics);
        self
    }

    /// Installs the counters after construction.
    ///
    /// [`ServerBuilder`](crate::lifecycle::ServerBuilder) creates the
    /// [`Metrics`] itself, and it needs the supervisor as its spawner first,
    /// so the daemon installs them once the server is up. Surfaces started
    /// before that — the `workspace.json` re-seed — pick them up too. A second
    /// call is a no-op.
    pub fn install_metrics(&self, metrics: Arc<Metrics>) {
        let _ = self.metrics.set(metrics);
    }

    /// A supervisor with default tunables and no upward reporting.
    #[must_use]
    pub fn for_tests() -> Arc<Self> {
        Arc::new(Self::new(
            SupervisorConfig::default(),
            Arc::new(NullNotifier),
        ))
    }

    /// The tunables in force.
    #[must_use]
    pub fn config(&self) -> &SupervisorConfig {
        &self.config
    }

    /// Where Surface changes are reported.
    #[must_use]
    pub fn notifier(&self) -> &Arc<dyn WorkspaceNotifier> {
        &self.notifier
    }

    /// The shared counters, when the daemon installed them.
    #[must_use]
    pub fn metrics(&self) -> Option<&Arc<Metrics>> {
        self.metrics.get()
    }

    /// Reports one change to the Workspace actor.
    pub fn notify(&self, upcall: Upcall) {
        self.notifier.notify(upcall);
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn handle(&self) -> Result<tokio::runtime::Handle, SpawnError> {
        self.runtime
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
            .ok_or_else(|| {
                SpawnError::Other("no tokio runtime to drive the Surface task".to_owned())
            })
    }

    // ------------------------------------------------------------- surfaces

    /// Looks a Surface up.
    #[must_use]
    pub fn slot(&self, id: SurfaceId) -> Option<Arc<SurfaceSlot>> {
        self.registry().surfaces.get(&id).cloned()
    }

    /// Every live Surface, in id order.
    #[must_use]
    pub fn surfaces(&self) -> Vec<Arc<SurfaceSlot>> {
        self.registry().surfaces.values().cloned().collect()
    }

    /// Every live Surface's id, in order.
    #[must_use]
    pub fn surface_ids(&self) -> Vec<SurfaceId> {
        self.registry().surfaces.keys().copied().collect()
    }

    /// Number of live Surfaces.
    #[must_use]
    pub fn surface_count(&self) -> usize {
        self.registry().surfaces.len()
    }

    /// Registers an already-built Surface, starting its I/O threads and task.
    ///
    /// Exposed so tests (and a future replay mode) can supervise a Surface
    /// that was not produced by [`SurfaceSpawner::spawn`]. A Surface with no
    /// PTY gets no threads: whoever built it feeds it.
    pub fn insert_surface(&self, surface: Surface) -> Result<Arc<SurfaceSlot>, SpawnError> {
        let id = surface.id();
        let title = surface.title().to_owned();

        let reader = match surface.pty() {
            Some(pty) => Some(
                pty.reader()
                    .map_err(|e| SpawnError::Other(format!("pty reader: {e}")))?,
            ),
            None => None,
        };
        let writer = match surface.pty() {
            Some(pty) => Some(
                pty.writer()
                    .map_err(|e| SpawnError::Other(format!("pty writer: {e}")))?,
            ),
            None => None,
        };
        let handle = reader.is_some().then(|| self.handle()).transpose()?;

        let (input_tx, input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let slot = Arc::new(SurfaceSlot {
            id,
            surface: Mutex::new(surface),
            input: input_tx,
            exit_hint: AtomicBool::new(reader.is_none()),
            exit_reported: AtomicBool::new(false),
            last_title: Mutex::new(title),
            last_busy: AtomicBool::new(false),
        });

        if let Some(writer) = writer {
            spawn_writer_thread(id, writer, input_rx, Arc::clone(&self.metrics));
        }
        if let (Some(reader), Some(handle)) = (reader, handle) {
            let (pty_tx, pty_rx) = mpsc::channel::<Vec<u8>>(PTY_QUEUE_DEPTH);
            spawn_reader_thread(id, reader, pty_tx);
            handle.spawn(feed_loop(
                Arc::clone(&slot),
                pty_rx,
                Arc::clone(&self.metrics),
            ));
        }

        self.registry().surfaces.insert(id, Arc::clone(&slot));
        Ok(slot)
    }

    /// Forgets a Surface, telling every attached Client why.
    ///
    /// The child is *not* signalled here — the Workspace actor calls
    /// [`SurfaceSpawner::kill`] first when it wants the process gone.
    pub fn remove(&self, id: SurfaceId) -> bool {
        let removed = self.registry().surfaces.remove(&id);
        let Some(slot) = removed else {
            return false;
        };
        let clients: Vec<ClientId> = slot.lock().publisher().clients().collect();
        for client in clients {
            self.send(
                client,
                &DataMsg::Detached(Detached {
                    surface_id: id,
                    reason: DetachReason::SurfaceDestroyed,
                }),
            );
        }
        true
    }

    /// Allocates the next Surface id. Ids are never reused.
    pub fn alloc_surface_id(&self) -> SurfaceId {
        SurfaceId(self.next_surface.fetch_add(1, Ordering::Relaxed))
    }

    /// Makes sure the next allocated id is at least `next_id`, so a Workspace
    /// restored from `workspace.json` never collides with it.
    pub fn reserve_ids_below(&self, next_id: u32) {
        self.next_surface.fetch_max(next_id, Ordering::Relaxed);
    }

    // -------------------------------------------------------------- clients

    /// Registers a data-plane connection under the id the accept loop
    /// allocated, so the control and data planes agree on connection numbers.
    pub fn register_client(
        &self,
        client: ClientId,
        out: mpsc::Sender<Vec<u8>>,
        shutdown: Arc<Notify>,
    ) {
        self.registry()
            .conns
            .insert(client, ConnHandle { out, shutdown });
    }

    /// Drops a connection and detaches it from every Surface (§7).
    pub fn unregister_client(&self, client: ClientId) {
        let (surfaces, handle) = {
            let mut registry = self.registry();
            let handle = registry.conns.remove(&client);
            let surfaces: Vec<Arc<SurfaceSlot>> = registry.surfaces.values().cloned().collect();
            (surfaces, handle)
        };
        for slot in surfaces {
            slot.lock().detach(client);
        }
        drop(handle);
    }

    /// Number of live data-plane connections.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.registry().conns.len()
    }

    /// Asks a connection to close itself (slow client, shutdown).
    pub fn close_client(&self, client: ClientId) {
        if let Some(handle) = self.registry().conns.get(&client) {
            handle.shutdown.notify_waiters();
        }
    }

    /// Encodes and queues one message for one connection.
    ///
    /// Returns `false` when the client is gone or its socket queue is full —
    /// the caller decides whether that is fatal.
    pub fn send(&self, client: ClientId, msg: &DataMsg) -> bool {
        let mut wire = Vec::new();
        if let Err(err) = msg.encode_to(&mut wire) {
            tracing::error!(%client, msg_type = msg.msg_type(), %err, "cannot encode a data frame");
            return false;
        }
        self.count_frame(msg.msg_type());
        self.send_bytes(client, wire)
    }

    /// Queues an already-encoded frame.
    pub fn send_bytes(&self, client: ClientId, wire: Vec<u8>) -> bool {
        let registry = self.registry();
        let Some(handle) = registry.conns.get(&client) else {
            return false;
        };
        match handle.out.try_send(wire) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(%client, "data connection is not draining; closing it");
                handle.shutdown.notify_waiters();
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    pub(crate) fn count_frame(&self, msg_type: u16) {
        let Some(metrics) = self.metrics.get() else {
            return;
        };
        metrics.frames_out.inc();
        match msg_type {
            st_proto::msg_type::SNAPSHOT => metrics.snapshots_sent.inc(),
            st_proto::msg_type::DELTA => metrics.deltas_sent.inc(),
            _ => {}
        }
    }

    // ------------------------------------------------------------- shutdown

    /// Signals every Surface's process group and every connection, then waits
    /// up to [`SupervisorConfig::kill_grace`] for the children to die
    /// (`03-server.md` §2).
    pub async fn shutdown(&self) {
        let clients: Vec<ClientId> = self.registry().conns.keys().copied().collect();
        let surfaces = self.surfaces();
        for slot in &surfaces {
            let attached: BTreeSet<ClientId> = slot.lock().publisher().clients().collect();
            for client in attached {
                self.send(
                    client,
                    &DataMsg::Detached(Detached {
                        surface_id: slot.id(),
                        reason: DetachReason::ServerShutdown,
                    }),
                );
            }
            let surface = slot.lock();
            if let Some(pty) = surface.pty() {
                pty.hangup();
            }
        }

        let deadline = std::time::Instant::now() + self.config.kill_grace;
        loop {
            let alive = surfaces
                .iter()
                .filter(|slot| {
                    let mut surface = slot.lock();
                    surface.poll_exit();
                    surface.status().is_running()
                })
                .count();
            if alive == 0 || std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        for slot in &surfaces {
            let mut surface = slot.lock();
            if surface.status().is_running() {
                if let Some(pty) = surface.pty_mut() {
                    pty.kill();
                }
            }
        }
        for client in clients {
            self.close_client(client);
        }
    }

    // ----------------------------------------------------------------- pump

    /// Starts the 120 Hz emit loop, at most once per supervisor.
    ///
    /// Called from [`crate::data::accept`], so nothing in `main.rs` has to
    /// know the pump exists.
    pub fn ensure_pump(self: &Arc<Self>) {
        if self.pump_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(handle) = self.handle() else {
            self.pump_started.store(false, Ordering::SeqCst);
            return;
        };
        handle.spawn(crate::data::pump::run(Arc::downgrade(self)));
    }
}

// -------------------------------------------------------------- the spawner

impl SurfaceSpawner for SurfaceSupervisor {
    fn spawn(&self, spec: &SpawnSpec) -> Result<SpawnedSurface, SpawnError> {
        let id = self.alloc_surface_id();
        let cols = spec.cols.max(1);
        let rows = spec.rows.max(1);

        let engine = EngineConfig {
            cols,
            rows,
            scrollback_lines: self.config.scrollback_lines,
            default_title: spec.program_name(),
            kitty_keyboard: true,
        };
        let pty = PtyConfig {
            surface_id: id,
            cols,
            rows,
            program: spec.shell.first().map(PathBuf::from),
            args: spec.shell.iter().skip(1).cloned().collect(),
            // The Workspace actor already resolved argv, `-l` included (§9).
            login: false,
            cwd: Some(spec.cwd.clone()),
            env: spec
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            build_id: self.config.build_id.clone(),
            socket_path: self.config.socket_path.clone(),
        };
        let surface = Surface::new(SurfaceConfig {
            id,
            engine,
            pty: Some(pty),
            spawn_cwd: spec.cwd.clone(),
            publisher: self.config.publisher,
        })
        .map_err(|err| SpawnError::Spawn {
            program: spec.program_name(),
            source: std::io::Error::other(err.to_string()),
        })?;

        let pid = surface.pty().and_then(Pty::pid);
        let title = surface.title().to_owned();
        self.insert_surface(surface)?;
        if let Some(metrics) = self.metrics.get() {
            metrics.surfaces_spawned.inc();
        }
        tracing::info!(surface = %id, ?pid, "spawned a Surface");

        Ok(SpawnedSurface {
            id,
            pid,
            title: (!title.is_empty()).then_some(title),
        })
    }

    fn kill(&self, id: SurfaceId, signal: KillSignal) -> Result<(), SpawnError> {
        let Some(slot) = self.slot(id) else {
            // Idempotent: the actor may signal a Surface we have forgotten.
            tracing::debug!(surface = %id, "kill for an unknown Surface, ignored");
            return Ok(());
        };
        let mut surface = slot.lock();
        if matches!(surface.status(), SurfaceStatus::Exited(_)) {
            return Ok(());
        }
        let Some(pid) = surface.pty().and_then(Pty::pid) else {
            return Ok(());
        };
        tracing::info!(surface = %id, ?signal, pid, "signalling a Surface's process group");
        match signal {
            // `Pty::hangup` is exactly `killpg(pgid, SIGHUP)` (Q21).
            KillSignal::Hup => {
                let _ = surface.pty().is_some_and(Pty::hangup);
            }
            KillSignal::Term => {
                if !kill_group(pid, rustix::process::Signal::TERM) {
                    let _ = surface.pty().is_some_and(Pty::hangup);
                }
            }
            KillSignal::Kill => {
                if let Some(pty) = surface.pty_mut() {
                    pty.kill();
                }
            }
        }
        Ok(())
    }

    fn destroy(&self, id: SurfaceId) {
        // `remove` detaches every subscriber with `SurfaceDestroyed` and drops
        // the engine + PTY. Idempotent: it returns false for unknown ids.
        if self.remove(id) {
            tracing::debug!(surface = %id, "destroyed a Surface");
        }
    }
}

/// `killpg(pgid, sig)`; `false` when the group is already gone.
fn kill_group(pid: u32, signal: rustix::process::Signal) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    rustix::process::kill_process_group(pid, signal).is_ok()
}

// ----------------------------------------------------------------- PTY I/O

fn spawn_reader_thread(id: SurfaceId, mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<Vec<u8>>) {
    let _ = std::thread::Builder::new()
        .name(format!("st-pty-read-{id}"))
        .spawn(move || {
            let mut buf = vec![0u8; PTY_READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    // `EIO` on Linux is the normal "the child closed the
                    // slave" signal, so it is an EOF, not a failure.
                    Err(err) => {
                        tracing::debug!(surface = %id, %err, "pty reader finished");
                        break;
                    }
                }
            }
        });
}

fn spawn_writer_thread(
    id: SurfaceId,
    mut writer: Box<dyn Write + Send>,
    mut rx: mpsc::UnboundedReceiver<Vec<u8>>,
    metrics: Arc<std::sync::OnceLock<Arc<Metrics>>>,
) {
    let _ = std::thread::Builder::new()
        .name(format!("st-pty-write-{id}"))
        .spawn(move || {
            while let Some(bytes) = rx.blocking_recv() {
                if let Err(err) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                    tracing::debug!(surface = %id, %err, "pty writer finished");
                    break;
                }
                if let Some(metrics) = metrics.get() {
                    metrics.pty_bytes_out.add(bytes.len() as u64);
                }
            }
        });
}

/// Drains the reader channel into the engine, batching up to
/// [`PTY_READ_CHUNK`] bytes per `feed` so a `cat` of a large file costs one
/// parser pass instead of sixteen.
async fn feed_loop(
    slot: Arc<SurfaceSlot>,
    mut rx: mpsc::Receiver<Vec<u8>>,
    metrics: Arc<std::sync::OnceLock<Arc<Metrics>>>,
) {
    while let Some(first) = rx.recv().await {
        let mut batch = first;
        while batch.len() < PTY_READ_CHUNK {
            match rx.try_recv() {
                Ok(more) => batch.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        if let Some(metrics) = metrics.get() {
            metrics.pty_bytes_in.add(batch.len() as u64);
        }
        let replies = {
            let mut surface = slot.lock();
            surface.feed(&batch);
            surface.take_pty_replies()
        };
        if !replies.is_empty() {
            slot.write_pty(replies);
        }
    }
    slot.set_exit_hint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_notifier_keeps_order() {
        let notifier = RecordingNotifier::new();
        notifier.notify(Upcall::Surface(SurfaceEvent::Input {
            surface: SurfaceId(1),
        }));
        notifier.notify(Upcall::Surface(SurfaceEvent::Input {
            surface: SurfaceId(2),
        }));
        assert_eq!(notifier.upcalls().len(), 2);
    }

    #[tokio::test]
    async fn ids_are_allocated_upward_and_never_reused() {
        let sup = SurfaceSupervisor::for_tests();
        assert_eq!(sup.alloc_surface_id(), SurfaceId(1));
        assert_eq!(sup.alloc_surface_id(), SurfaceId(2));
        sup.reserve_ids_below(50);
        assert_eq!(sup.alloc_surface_id(), SurfaceId(50));
        sup.reserve_ids_below(10);
        assert_eq!(sup.alloc_surface_id(), SurfaceId(51));
    }

    #[tokio::test]
    async fn killing_an_unknown_surface_is_not_an_error() {
        let sup = SurfaceSupervisor::for_tests();
        assert!(sup.kill(SurfaceId(404), KillSignal::Hup).is_ok());
        assert!(!sup.remove(SurfaceId(404)));
    }
}
