//! The Data Plane client — `docs/plan/04-client-native.md` §5.
//!
//! # Shape
//!
//! Two layers, so the protocol logic is testable without a socket:
//!
//! * [`DataPlaneCore`] — **runtime-agnostic**. Bytes in
//!   ([`feed`](DataPlaneCore::feed)), frames out; it owns the
//!   [`FrameDecoder`], decodes [`DataMsg`]s and applies them to the Replica
//!   map. It never touches a file descriptor, so every protocol behaviour
//!   below has a unit test that is just a `Vec<u8>`.
//! * [`DataPlaneHandle`] — a cheap, cloneable handle over an
//!   `Arc<`[`Shared`]`>`, backed by one dedicated OS thread doing blocking
//!   I/O on the Unix socket.
//!
//! # Why a thread and `std::os::unix::net`, not tokio
//!
//! `04-client-native.md` §5 sketches "a dedicated OS thread running a tokio
//! current-thread runtime". The runtime buys nothing here and costs a large
//! dependency:
//!
//! * There is exactly **one** socket per client and exactly **two** things to
//!   do with it — read frames, write frames. There is no concurrency to
//!   multiplex, so a `select!` loop has nothing to select between.
//! * Reads happen on the I/O thread and writes happen on the caller's thread
//!   through a `try_clone`d write half under a [`parking_lot::Mutex`]. The
//!   kernel serialises them; a Unix-socket write of a ≤64 KiB frame does not
//!   block in practice, and if it does, blocking the caller is the correct
//!   backpressure.
//! * Inbound messages are applied **on the I/O thread**, directly into the
//!   Replica map (§5). That is microseconds of work and keeps it off the frame
//!   budget, which is the whole point of the design — an async runtime would
//!   not change where the work happens.
//! * Invariant I8 keeps `st-proto` dependency-light; there is no reason for
//!   the client half to be heavier. Dropping tokio removes ~40 transitive
//!   crates from a build that already pays for GPUI.
//!
//! Shutdown is a flag plus `UnixStream::shutdown`, which unblocks the reader
//! immediately, so the thread never needs a wakeup channel.
//!
//! # Waking the renderer
//!
//! A [`WakeFn`] is invoked whenever a Replica changes, coalesced per Surface
//! by a `pending_paint` flag: N Deltas between two frames cost **one** wake
//! (grilling Q27). The GPUI layer's callback schedules a repaint; this crate
//! never learns what a frame is.
//!
//! # Gaps
//!
//! When [`Replica::apply_delta`] reports a [`Gap`], the core immediately
//! queues an `Attach { want_snapshot: true, known_seq: 0 }` for that Surface
//! and emits [`DataPlaneEvent::Gap`]. The delta is dropped, never buffered.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use st_proto::{
    encode_frame, AbsLine, Ack, Attach, AttachMode, ClientKind, CodecError, DataError, DataMsg,
    Detach, DetachReason, ExitStatus, FetchHistory, FrameDecoder, FrameError, Hello, Input,
    ProtoVersion, Reject, Resize, Seq, SetViewState, SurfaceId, DATA_MAGIC, MAX_INPUT_BYTES,
    PROTO_VERSION,
};

use crate::replica::{Gap, Replica, ReplicaConfig};

/// Called when a Replica changes or an event is queued, from the I/O thread.
///
/// Must be cheap and must not block: the GPUI layer only schedules a repaint
/// (`AsyncApp::update` → `cx.notify()`, §5).
pub type WakeFn = Box<dyn Fn() + Send + Sync>;

/// Something out-of-band the renderer or the app layer should know about.
///
/// Replica *content* changes are **not** events — they are signalled through
/// the [`WakeFn`] and read back with [`DataPlaneHandle::take_dirty`], so a
/// burst of Deltas cannot pile up an unbounded queue.
#[derive(Debug, Clone, PartialEq)]
pub enum DataPlaneEvent {
    /// The handshake completed.
    Connected {
        /// The negotiated protocol version.
        proto_version: ProtoVersion,
        /// The server's build id, for `server.status`.
        server_build_id: String,
        /// The daemon's pid.
        server_pid: u32,
    },
    /// The server refused the connection (§2).
    Rejected(Box<Reject>),
    /// The socket closed or errored; the thread will reconnect if configured.
    Disconnected {
        /// Why, for the banner and the log.
        reason: String,
    },
    /// The program rang the bell.
    Bell(SurfaceId),
    /// The Surface's process ended.
    Exited {
        /// Which Surface.
        surface_id: SurfaceId,
        /// How it ended.
        status: ExitStatus,
    },
    /// The server dropped this connection's attachment.
    Detached {
        /// Which Surface.
        surface_id: SurfaceId,
        /// Why.
        reason: DetachReason,
    },
    /// A Delta did not build on what we hold; a Snapshot has been requested.
    Gap(Gap),
    /// A per-message error (grilling Q48). Never fatal.
    Error(DataError),
}

/// Options for a Data Plane connection.
#[derive(Debug, Clone)]
pub struct DataPlaneOptions {
    /// `Hello.build_id`: git sha + dirty flag. Informational only.
    pub build_id: String,
    /// Cache sizing for every Replica this connection creates.
    pub replica: ReplicaConfig,
    /// Reconnect after the socket drops, re-attaching every known Surface.
    pub reconnect: bool,
    /// First reconnect delay; it doubles up to
    /// [`max_reconnect_delay`](DataPlaneOptions::max_reconnect_delay).
    pub reconnect_delay: Duration,
    /// Ceiling for the reconnect backoff.
    pub max_reconnect_delay: Duration,
    /// Acknowledge every applied state change automatically (§6.5). Turn it
    /// off only if the caller drives [`DataPlaneHandle::ack`] itself.
    pub auto_ack: bool,
}

impl Default for DataPlaneOptions {
    fn default() -> Self {
        Self {
            build_id: String::new(),
            replica: ReplicaConfig::default(),
            reconnect: true,
            reconnect_delay: Duration::from_millis(100),
            max_reconnect_delay: Duration::from_secs(5),
            auto_ack: true,
        }
    }
}

/// Anything that went wrong on the Data Plane.
#[derive(Debug, thiserror::Error)]
pub enum DataPlaneError {
    /// Socket I/O failed.
    #[error("data-plane I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Framing failed. Always fatal for the connection.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// A message could not be encoded or decoded.
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// Nothing is connected, so the message could not be sent.
    #[error("the data plane is not connected")]
    NotConnected,
    /// The server sent a `Reject` (§2).
    #[error("the server rejected the connection: {} ({:?})", .0.message, .0.reason)]
    Rejected(Box<Reject>),
    /// The server sent a message only a client may send.
    #[error("the server sent a client-to-server message (msg_type 0x{0:04X})")]
    UnexpectedMessage(u16),
}

/// One Replica plus its repaint-coalescing flag (§5).
#[derive(Debug)]
struct ReplicaSlot {
    replica: Replica,
    pending_paint: AtomicBool,
}

/// The write half of a live connection, plus a way to unblock the reader.
struct Conn {
    write: Box<dyn Write + Send>,
    close: Box<dyn Fn() + Send + Sync>,
}

/// State shared between the I/O thread and every [`DataPlaneHandle`].
pub struct Shared {
    replicas: Mutex<HashMap<SurfaceId, ReplicaSlot>>,
    attachments: Mutex<HashMap<SurfaceId, AttachMode>>,
    conn: Mutex<Option<Conn>>,
    events: Mutex<Vec<DataPlaneEvent>>,
    connected: AtomicBool,
    shutdown: AtomicBool,
    wake: WakeFn,
    options: DataPlaneOptions,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("surfaces", &self.replicas.lock().len())
            .field("connected", &self.connected.load(Ordering::Acquire))
            .field("shutdown", &self.shutdown.load(Ordering::Acquire))
            .finish()
    }
}

impl Shared {
    /// Builds the shared state. Nothing is connected yet.
    #[must_use]
    pub fn new(options: DataPlaneOptions, wake: WakeFn) -> Arc<Self> {
        Arc::new(Self {
            replicas: Mutex::new(HashMap::new()),
            attachments: Mutex::new(HashMap::new()),
            conn: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            connected: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            wake,
            options,
        })
    }

    /// Queues an event and wakes the renderer.
    fn push_event(&self, event: DataPlaneEvent) {
        self.events.lock().push(event);
        (self.wake)();
    }

    /// Marks a Surface dirty, waking the renderer at most once per frame.
    fn mark_dirty(&self, slot: &ReplicaSlot) {
        if !slot.pending_paint.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }

    /// Writes raw bytes to the socket, if one is up.
    fn write_all(&self, bytes: &[u8]) -> Result<(), DataPlaneError> {
        let mut guard = self.conn.lock();
        let conn = guard.as_mut().ok_or(DataPlaneError::NotConnected)?;
        conn.write.write_all(bytes)?;
        conn.write.flush()?;
        Ok(())
    }

    /// Encodes and sends one message.
    fn send(&self, msg: &DataMsg) -> Result<(), DataPlaneError> {
        let mut buf = Vec::new();
        msg.encode_to(&mut buf)?;
        self.write_all(&buf)
    }

    /// Installs a live connection, replacing any previous one.
    fn set_conn(&self, conn: Option<Conn>) {
        let connected = conn.is_some();
        *self.conn.lock() = conn;
        self.connected.store(connected, Ordering::Release);
    }
}

// ------------------------------------------------------------------- the core

/// The transport-free half of the Data Plane client.
///
/// Feed it the bytes a socket produced; it decodes frames, applies state to
/// the Replica map and answers what the protocol requires (acks, and a
/// Snapshot request after a [`Gap`]) through the same [`Shared`] the handle
/// writes with.
pub struct DataPlaneCore {
    shared: Arc<Shared>,
    decoder: FrameDecoder,
}

impl std::fmt::Debug for DataPlaneCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataPlaneCore")
            .field("buffered", &self.decoder.buffered_len())
            .finish()
    }
}

impl DataPlaneCore {
    /// A core over the given shared state.
    #[must_use]
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            decoder: FrameDecoder::new(),
        }
    }

    /// The bytes a client sends first: the 4-byte magic `0xFF"STD"` followed
    /// by a framed [`Hello`] (grilling Q37, §2).
    #[must_use]
    pub fn handshake_bytes(build_id: &str) -> Vec<u8> {
        let hello = DataMsg::Hello(Hello {
            proto_version: PROTO_VERSION,
            client_kind: ClientKind::Data,
            build_id: build_id.to_string(),
        });
        let payload = hello.to_payload().expect("Hello always encodes");
        let mut out = Vec::with_capacity(DATA_MAGIC.len() + payload.len() + 6);
        out.extend_from_slice(&DATA_MAGIC);
        encode_frame(hello.msg_type(), &payload, &mut out).expect("Hello always fits in a frame");
        out
    }

    /// Decodes and applies everything complete in `bytes`.
    ///
    /// A framing error is fatal for the connection and is returned; a decode
    /// error for one message is logged and skipped, because a `msg_type` from
    /// a newer minor must not kill the link (§10).
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), DataPlaneError> {
        self.decoder.push(bytes);
        while let Some(frame) = self.decoder.next_frame()? {
            match DataMsg::from_frame(frame.msg_type, &frame.payload) {
                Ok(msg) => self.dispatch(msg)?,
                Err(CodecError::UnknownMsgType(t)) => {
                    tracing::debug!(msg_type = t, "ignoring an unknown data-plane message");
                }
                Err(err) => {
                    tracing::warn!(msg_type = frame.msg_type, %err, "undecodable data-plane frame");
                }
            }
        }
        Ok(())
    }

    /// Applies one decoded message.
    fn dispatch(&mut self, msg: DataMsg) -> Result<(), DataPlaneError> {
        // `is_client_to_server` splits at 0x0100, which puts the handshake
        // answers (`HelloAck`, `Reject`) on the client side; every genuinely
        // client-only message falls through to the `other` arm below.
        match msg {
            DataMsg::HelloAck(ack) => {
                self.shared.push_event(DataPlaneEvent::Connected {
                    proto_version: ack.proto_version,
                    server_build_id: ack.server_build_id,
                    server_pid: ack.server_pid,
                });
            }
            DataMsg::Reject(reject) => {
                let reject = Box::new(reject);
                self.shared
                    .push_event(DataPlaneEvent::Rejected(reject.clone()));
                return Err(DataPlaneError::Rejected(reject));
            }
            DataMsg::Snapshot(snap) => {
                let (surface_id, seq) = (snap.surface_id, snap.seq);
                self.with_slot(surface_id, |shared, slot| {
                    slot.replica.apply_snapshot(&snap);
                    shared.mark_dirty(slot);
                });
                self.auto_ack(surface_id, seq);
            }
            DataMsg::Delta(delta) => {
                let (surface_id, seq) = (delta.surface_id, delta.seq);
                let gap = self.with_slot(surface_id, |shared, slot| {
                    match slot.replica.apply_delta(&delta) {
                        Ok(()) => {
                            shared.mark_dirty(slot);
                            None
                        }
                        Err(gap) => Some(gap),
                    }
                });
                if let Some(gap) = gap {
                    tracing::warn!(%gap, "delta gap; requesting a snapshot");
                    self.shared.push_event(DataPlaneEvent::Gap(gap));
                    self.request_snapshot(surface_id);
                } else {
                    self.auto_ack(surface_id, seq);
                }
            }
            DataMsg::History(page) => {
                let surface_id = page.surface_id;
                self.with_slot(surface_id, |shared, slot| {
                    slot.replica.apply_history_page(&page);
                    shared.mark_dirty(slot);
                });
            }
            DataMsg::SurfaceExited(exited) => {
                self.with_slot(exited.surface_id, |shared, slot| {
                    slot.replica.apply_exited(exited.seq, exited.status);
                    shared.mark_dirty(slot);
                });
                self.shared.push_event(DataPlaneEvent::Exited {
                    surface_id: exited.surface_id,
                    status: exited.status,
                });
                self.auto_ack(exited.surface_id, exited.seq);
            }
            DataMsg::Bell(bell) => {
                self.shared
                    .push_event(DataPlaneEvent::Bell(bell.surface_id));
            }
            DataMsg::Detached(detached) => {
                self.shared.attachments.lock().remove(&detached.surface_id);
                self.shared.push_event(DataPlaneEvent::Detached {
                    surface_id: detached.surface_id,
                    reason: detached.reason,
                });
            }
            DataMsg::DataError(err) => {
                tracing::warn!(code = err.code, message = %err.message, "data-plane error");
                self.shared.push_event(DataPlaneEvent::Error(err));
            }
            // `Hello`, `Attach`, `Input`, … are client→server only.
            other => return Err(DataPlaneError::UnexpectedMessage(other.msg_type())),
        }
        Ok(())
    }

    /// Runs `f` against the Replica for `surface_id`, creating it if this is
    /// the first message about that Surface.
    fn with_slot<R>(
        &self,
        surface_id: SurfaceId,
        f: impl FnOnce(&Shared, &mut ReplicaSlot) -> R,
    ) -> R {
        let mut replicas = self.shared.replicas.lock();
        let slot = replicas.entry(surface_id).or_insert_with(|| ReplicaSlot {
            replica: Replica::with_config(surface_id, self.shared.options.replica),
            pending_paint: AtomicBool::new(false),
        });
        f(&self.shared, slot)
    }

    /// Sends an [`Ack`] when configured to (§6.5).
    fn auto_ack(&self, surface_id: SurfaceId, seq: Seq) {
        if !self.shared.options.auto_ack {
            return;
        }
        if let Err(err) = self.shared.send(&DataMsg::Ack(Ack { surface_id, seq })) {
            tracing::debug!(%err, "could not ack");
        }
    }

    /// Re-`Attach`es with `want_snapshot: true` after a gap.
    fn request_snapshot(&self, surface_id: SurfaceId) {
        let mode = self
            .shared
            .attachments
            .lock()
            .get(&surface_id)
            .copied()
            .unwrap_or(AttachMode::Active);
        let attach = DataMsg::Attach(Attach {
            surface_id,
            mode,
            want_snapshot: true,
            known_seq: Seq::ZERO,
        });
        if let Err(err) = self.shared.send(&attach) {
            tracing::debug!(%err, "could not request a snapshot");
        }
    }
}

// ----------------------------------------------------------------- the handle

/// A cheap, cloneable handle to a Data Plane connection.
///
/// Every method is safe to call from any thread and from any state: sends on a
/// dead connection return [`DataPlaneError::NotConnected`] rather than
/// blocking, and the attachment set is remembered so a reconnect restores it.
#[derive(Clone, Debug)]
pub struct DataPlaneHandle {
    shared: Arc<Shared>,
}

impl DataPlaneHandle {
    /// Wraps existing shared state without any I/O. Useful for tests and for
    /// driving a [`DataPlaneCore`] by hand.
    #[must_use]
    pub fn from_shared(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// The shared state, for constructing a [`DataPlaneCore`].
    #[must_use]
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// `true` once the socket is up (the handshake may still be in flight).
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    /// Subscribes to a Surface (§4.2, grilling Q44).
    ///
    /// The mode is remembered, so a reconnect re-attaches with it. When the
    /// Replica already holds state, `known_seq` lets the server answer with
    /// Deltas instead of a full Snapshot.
    pub fn attach(&self, surface_id: SurfaceId, mode: AttachMode) -> Result<(), DataPlaneError> {
        self.shared.attachments.lock().insert(surface_id, mode);
        let known_seq = self
            .with_replica(surface_id, Replica::seq)
            .unwrap_or(Seq::ZERO);
        self.shared.send(&DataMsg::Attach(Attach {
            surface_id,
            mode,
            want_snapshot: known_seq == Seq::ZERO,
            known_seq,
        }))
    }

    /// Unsubscribes from a Surface. The Replica is kept, so re-attaching
    /// applies a Snapshot into an existing allocation (grilling Q44).
    pub fn detach(&self, surface_id: SurfaceId) -> Result<(), DataPlaneError> {
        self.shared.attachments.lock().remove(&surface_id);
        self.shared.send(&DataMsg::Detach(Detach { surface_id }))
    }

    /// Drops a Surface's Replica entirely, freeing its memory.
    pub fn forget(&self, surface_id: SurfaceId) {
        self.shared.attachments.lock().remove(&surface_id);
        self.shared.replicas.lock().remove(&surface_id);
    }

    /// Reports the user's View State (selection, scroll offset) for a Surface.
    ///
    /// Grilling Q43/Q49: selection and scroll offset are produced by the
    /// renderer from the [`Replica`](crate::replica::Replica), so they travel
    /// on the Data Plane rather than making a round trip through the control
    /// plane. The Server stores them on the Surface — which is what makes a
    /// selection survive a Client relaunch — and echoes them to control-plane
    /// subscribers in `ev.workspace`.
    ///
    /// Callers should debounce this: send on mouse-up after a selection drag,
    /// and at most a few times a second while scrolling.
    pub fn set_view_state(&self, msg: SetViewState) -> Result<(), DataPlaneError> {
        self.shared.send(&DataMsg::SetViewState(msg))
    }

    /// Writes bytes to a Surface's PTY (§9).
    ///
    /// Long pastes are split into consecutive frames of at most
    /// [`MAX_INPUT_BYTES`]; the server writes them in order.
    pub fn send_input(&self, surface_id: SurfaceId, bytes: &[u8]) -> Result<(), DataPlaneError> {
        if bytes.is_empty() {
            return Ok(());
        }
        for chunk in bytes.chunks(MAX_INPUT_BYTES) {
            self.shared.send(&DataMsg::Input(Input {
                surface_id,
                bytes: chunk.to_vec(),
            }))?;
        }
        Ok(())
    }

    /// Asks the server to resize a Surface's grid. Last writer wins
    /// (grilling Q40); the client does not reflow locally.
    pub fn resize(
        &self,
        surface_id: SurfaceId,
        cols: u16,
        rows: u16,
    ) -> Result<(), DataPlaneError> {
        self.shared.send(&DataMsg::Resize(Resize {
            surface_id,
            cols,
            rows,
        }))
    }

    /// Requests a page of scrollback (§8). `count` is clamped to
    /// [`st_proto::MAX_HISTORY_COUNT`].
    pub fn fetch_history(
        &self,
        surface_id: SurfaceId,
        from_line: AbsLine,
        count: u16,
    ) -> Result<(), DataPlaneError> {
        self.shared.send(&DataMsg::FetchHistory(FetchHistory {
            surface_id,
            from_line,
            count: count.min(st_proto::MAX_HISTORY_COUNT),
        }))
    }

    /// Acknowledges everything applied up to `seq` (§6.5).
    ///
    /// The core does this automatically unless
    /// [`DataPlaneOptions::auto_ack`] is off.
    pub fn ack(&self, surface_id: SurfaceId, seq: Seq) -> Result<(), DataPlaneError> {
        self.shared.send(&DataMsg::Ack(Ack { surface_id, seq }))
    }

    /// Runs `f` against a Surface's Replica under the lock.
    ///
    /// Hold it for as short as possible — the I/O thread applies Deltas under
    /// the same lock. The renderer's pattern is "lock, copy the visible rows,
    /// unlock" (§6).
    #[must_use]
    pub fn with_replica<R>(
        &self,
        surface_id: SurfaceId,
        f: impl FnOnce(&Replica) -> R,
    ) -> Option<R> {
        let replicas = self.shared.replicas.lock();
        replicas.get(&surface_id).map(|slot| f(&slot.replica))
    }

    /// Runs `f` against a Surface's Replica mutably, creating it if needed.
    ///
    /// For local-only edits the server does not own, e.g. trimming a hidden
    /// Tab's cache with [`Replica::shrink_history_to`].
    pub fn with_replica_mut<R>(
        &self,
        surface_id: SurfaceId,
        f: impl FnOnce(&mut Replica) -> R,
    ) -> R {
        let mut replicas = self.shared.replicas.lock();
        let slot = replicas.entry(surface_id).or_insert_with(|| ReplicaSlot {
            replica: Replica::with_config(surface_id, self.shared.options.replica),
            pending_paint: AtomicBool::new(false),
        });
        f(&mut slot.replica)
    }

    /// Every Surface with a Replica.
    #[must_use]
    pub fn surfaces(&self) -> Vec<SurfaceId> {
        let mut ids: Vec<SurfaceId> = self.shared.replicas.lock().keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// The Surfaces whose Replica changed since the last call, clearing their
    /// `pending_paint` flags (§5, grilling Q27).
    ///
    /// Call this *after* copying the rows out, so a Delta that lands mid-paint
    /// schedules the next frame instead of being lost.
    #[must_use]
    pub fn take_dirty(&self) -> Vec<SurfaceId> {
        let replicas = self.shared.replicas.lock();
        let mut dirty: Vec<SurfaceId> = replicas
            .iter()
            .filter(|(_, slot)| slot.pending_paint.swap(false, Ordering::AcqRel))
            .map(|(id, _)| *id)
            .collect();
        dirty.sort_unstable();
        dirty
    }

    /// Drains the out-of-band event queue.
    #[must_use]
    pub fn take_events(&self) -> Vec<DataPlaneEvent> {
        std::mem::take(&mut *self.shared.events.lock())
    }

    /// Closes the connection and stops the I/O thread.
    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(conn) = self.shared.conn.lock().as_ref() {
            (conn.close)();
        }
        self.shared.connected.store(false, Ordering::Release);
    }
}

// --------------------------------------------------------------- the I/O thread

#[cfg(unix)]
mod io_thread {
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use super::*;

    /// A running Data Plane connection: the handle plus its I/O thread.
    ///
    /// Dropping this shuts the connection down and joins the thread, so a
    /// client that forgets to call [`DataPlaneHandle::shutdown`] still leaves
    /// no thread behind.
    #[derive(Debug)]
    pub struct DataPlaneConnection {
        handle: DataPlaneHandle,
        thread: Option<JoinHandle<()>>,
    }

    impl DataPlaneConnection {
        /// Connects to `path` (`$XDG_RUNTIME_DIR/superterminal/data.sock`) and
        /// starts the I/O thread.
        ///
        /// The first connection attempt is synchronous, so a bad path is an
        /// error rather than a silent retry loop; every *later* attempt is the
        /// thread's business.
        pub fn connect(
            path: impl AsRef<Path>,
            options: DataPlaneOptions,
            wake: WakeFn,
        ) -> Result<Self, DataPlaneError> {
            let path = path.as_ref().to_path_buf();
            let stream = UnixStream::connect(&path)?;
            let shared = Shared::new(options, wake);
            Ok(Self::spawn(shared, stream, Some(path)))
        }

        /// Starts the I/O thread on an already-connected stream.
        ///
        /// This is what the tests use with [`UnixStream::pair`], and what a
        /// caller with its own socket setup (an inherited fd, say) wants.
        /// Without a path there is nothing to reconnect to, so the thread
        /// stops when the stream closes.
        #[must_use]
        pub fn from_stream(stream: UnixStream, options: DataPlaneOptions, wake: WakeFn) -> Self {
            let shared = Shared::new(options, wake);
            Self::spawn(shared, stream, None)
        }

        fn spawn(shared: Arc<Shared>, stream: UnixStream, path: Option<PathBuf>) -> Self {
            let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
            let thread = std::thread::Builder::new()
                .name("st-dataplane".into())
                .spawn(move || run(shared, stream, path))
                .expect("spawning the st-dataplane thread");
            Self {
                handle,
                thread: Some(thread),
            }
        }

        /// The handle callers use. Clone it freely.
        #[must_use]
        pub fn handle(&self) -> DataPlaneHandle {
            self.handle.clone()
        }

        /// Shuts the connection down and joins the I/O thread.
        pub fn shutdown(mut self) {
            self.stop();
        }

        fn stop(&mut self) {
            self.handle.shutdown();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    impl Drop for DataPlaneConnection {
        fn drop(&mut self) {
            self.stop();
        }
    }

    impl std::ops::Deref for DataPlaneConnection {
        type Target = DataPlaneHandle;

        fn deref(&self) -> &Self::Target {
            &self.handle
        }
    }

    /// The I/O thread: serve the stream we were handed, then reconnect for as
    /// long as we are configured to and not shutting down.
    fn run(shared: Arc<Shared>, stream: UnixStream, path: Option<PathBuf>) {
        let mut stream = Some(stream);
        let mut delay = shared.options.reconnect_delay;

        loop {
            let next = match stream.take() {
                Some(s) => Some(s),
                None => match path.as_ref() {
                    Some(path) => match UnixStream::connect(path) {
                        Ok(s) => Some(s),
                        Err(err) => {
                            tracing::debug!(%err, "data-plane reconnect failed");
                            None
                        }
                    },
                    None => break,
                },
            };

            if let Some(stream) = next {
                let reason = serve(&shared, stream);
                shared.set_conn(None);
                if shared.shutdown.load(Ordering::Acquire) {
                    break;
                }
                shared.push_event(DataPlaneEvent::Disconnected { reason });
                delay = shared.options.reconnect_delay;
            }

            if shared.shutdown.load(Ordering::Acquire)
                || !shared.options.reconnect
                || path.is_none()
            {
                break;
            }
            std::thread::sleep(delay);
            delay = (delay * 2).min(shared.options.max_reconnect_delay);
        }
        shared.set_conn(None);
    }

    /// Handshake, re-attach, then read until the stream ends. Returns why it
    /// ended.
    fn serve(shared: &Arc<Shared>, stream: UnixStream) -> String {
        let Ok(writer) = stream.try_clone() else {
            return "could not duplicate the socket".to_string();
        };
        let closer = match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return "could not duplicate the socket".to_string(),
        };
        shared.set_conn(Some(Conn {
            write: Box::new(writer),
            close: Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            }),
        }));

        if let Err(err) =
            shared.write_all(&DataPlaneCore::handshake_bytes(&shared.options.build_id))
        {
            return format!("handshake write failed: {err}");
        }
        reattach_all(shared);

        let mut core = DataPlaneCore::new(Arc::clone(shared));
        let mut stream = stream;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => return "the server closed the connection".to_string(),
                Ok(n) => {
                    if let Err(err) = core.feed(&buf[..n]) {
                        return err.to_string();
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => return err.to_string(),
            }
            if shared.shutdown.load(Ordering::Acquire) {
                return "shutting down".to_string();
            }
        }
    }

    /// Re-sends an `Attach` for every Surface the client believes it is
    /// attached to, so a reconnect restores the subscription set.
    fn reattach_all(shared: &Arc<Shared>) {
        let attachments: Vec<(SurfaceId, AttachMode)> = shared
            .attachments
            .lock()
            .iter()
            .map(|(id, mode)| (*id, *mode))
            .collect();
        for (surface_id, mode) in attachments {
            let known_seq = shared
                .replicas
                .lock()
                .get(&surface_id)
                .map_or(Seq::ZERO, |slot| slot.replica.seq());
            let attach = DataMsg::Attach(Attach {
                surface_id,
                mode,
                want_snapshot: known_seq == Seq::ZERO,
                known_seq,
            });
            if let Err(err) = shared.send(&attach) {
                tracing::debug!(%err, "could not re-attach after reconnecting");
            }
        }
    }
}

#[cfg(unix)]
pub use io_thread::DataPlaneConnection;

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{
        Bell, Cursor, Delta, Detached, DirtyRow, HelloAck, Modes, PackedCell, RejectReason, Row,
        Snapshot, Style, StyleIdx, SurfaceExited, ViewState,
    };
    use std::sync::atomic::AtomicUsize;

    /// Counts wake calls, so tests can assert on repaint coalescing.
    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl WakeCounter {
        fn get(&self) -> usize {
            self.0.load(Ordering::Acquire)
        }
    }

    fn wired(options: DataPlaneOptions) -> (Arc<Shared>, Arc<WakeCounter>) {
        let counter = Arc::new(WakeCounter::default());
        let for_wake = Arc::clone(&counter);
        let shared = Shared::new(
            options,
            Box::new(move || {
                for_wake.0.fetch_add(1, Ordering::AcqRel);
            }),
        );
        (shared, counter)
    }

    /// A writable sink standing in for the socket, so the core's outbound
    /// bytes can be inspected.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Sink {
        fn messages(&self) -> Vec<DataMsg> {
            let bytes = self.0.lock().clone();
            let mut decoder = FrameDecoder::new();
            decoder.push(&bytes);
            let mut out = Vec::new();
            while let Some(frame) = decoder.next_frame().expect("well-framed") {
                out.push(DataMsg::from_frame(frame.msg_type, &frame.payload).expect("decodable"));
            }
            out
        }
    }

    fn attach_sink(shared: &Arc<Shared>) -> Sink {
        let sink = Sink::default();
        shared.set_conn(Some(Conn {
            write: Box::new(sink.clone()),
            close: Box::new(|| {}),
        }));
        sink
    }

    fn row_of(text: &str) -> Row {
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

    fn snapshot(seq: u64, lines: &[&str]) -> Snapshot {
        Snapshot {
            surface_id: SurfaceId(3),
            seq: Seq(seq),
            cols: 20,
            rows: lines.len() as u16,
            styles: vec![Style::DEFAULT],
            grid: lines.iter().map(|l| row_of(l)).collect(),
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: "sh".into(),
            history_base: AbsLine(0),
            history_len: 0,
            view_state: ViewState::default(),
            exited: None,
        }
    }

    fn delta(seq: u64, since: u64, rows: Vec<DirtyRow>) -> Delta {
        Delta {
            surface_id: SurfaceId(3),
            seq: Seq(seq),
            since_seq: Seq(since),
            history_base: AbsLine(0),
            history_len: 0,
            resized: None,
            new_styles: Vec::new(),
            rows,
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: None,
        }
    }

    fn wire(msgs: &[DataMsg]) -> Vec<u8> {
        let mut out = Vec::new();
        for msg in msgs {
            msg.encode_to(&mut out).expect("encodable");
        }
        out
    }

    #[test]
    fn the_handshake_is_the_magic_then_a_framed_hello() {
        let bytes = DataPlaneCore::handshake_bytes("deadbeef");
        assert_eq!(&bytes[..4], &DATA_MAGIC);

        let mut decoder = FrameDecoder::expecting_magic();
        decoder.push(&bytes);
        let frame = decoder.next_frame().unwrap().unwrap();
        assert_eq!(frame.msg_type, st_proto::msg_type::HELLO);
        let DataMsg::Hello(hello) = DataMsg::from_frame(frame.msg_type, &frame.payload).unwrap()
        else {
            panic!("expected a Hello");
        };
        assert_eq!(hello.client_kind, ClientKind::Data);
        assert_eq!(hello.proto_version, PROTO_VERSION);
        assert_eq!(hello.build_id, "deadbeef");
        assert_eq!(decoder.next_frame().unwrap(), None);
    }

    #[test]
    fn a_snapshot_lands_in_the_replica_map_and_wakes_once() {
        let (shared, wakes) = wired(DataPlaneOptions::default());
        let sink = attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));

        core.feed(&wire(&[DataMsg::Snapshot(Box::new(snapshot(
            1,
            &["hello"],
        )))]))
        .unwrap();

        assert_eq!(handle.surfaces(), vec![SurfaceId(3)]);
        let title = handle
            .with_replica(SurfaceId(3), |r| {
                assert_eq!(r.seq(), Seq(1));
                assert_eq!(r.cols(), 20);
                r.title().to_string()
            })
            .unwrap();
        assert_eq!(title, "sh");
        assert_eq!(wakes.get(), 1);
        assert_eq!(handle.take_dirty(), vec![SurfaceId(3)]);
        // Draining resets the flag.
        assert!(handle.take_dirty().is_empty());
        // And an Ack went out.
        assert_eq!(
            sink.messages(),
            vec![DataMsg::Ack(Ack {
                surface_id: SurfaceId(3),
                seq: Seq(1)
            })]
        );
    }

    #[test]
    fn deltas_between_frames_coalesce_into_one_wake() {
        let (shared, wakes) = wired(DataPlaneOptions {
            auto_ack: false,
            ..DataPlaneOptions::default()
        });
        attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));

        let mut msgs = vec![DataMsg::Snapshot(Box::new(snapshot(1, &["a", "b"])))];
        for seq in 2..=6u64 {
            msgs.push(DataMsg::Delta(Box::new(delta(
                seq,
                seq - 1,
                vec![DirtyRow {
                    index: 0,
                    row: row_of(&format!("row{seq}")),
                }],
            ))));
        }
        core.feed(&wire(&msgs)).unwrap();

        // One wake for the whole burst.
        assert_eq!(wakes.get(), 1);
        assert_eq!(handle.take_dirty(), vec![SurfaceId(3)]);
        handle
            .with_replica(SurfaceId(3), |r| {
                assert_eq!(r.seq(), Seq(6));
                assert_eq!(r.row(0).unwrap(), &row_of("row6"));
            })
            .unwrap();

        // After a paint, the next delta wakes again.
        core.feed(&wire(&[DataMsg::Delta(Box::new(delta(7, 6, vec![])))]))
            .unwrap();
        assert_eq!(wakes.get(), 2);
    }

    #[test]
    fn a_gap_requests_a_snapshot_and_reports_an_event() {
        let (shared, _) = wired(DataPlaneOptions::default());
        let sink = attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));
        handle.attach(SurfaceId(3), AttachMode::Active).unwrap();

        core.feed(&wire(&[
            DataMsg::Snapshot(Box::new(snapshot(1, &["a"]))),
            // since_seq 4 does not match our seq 1.
            DataMsg::Delta(Box::new(delta(
                5,
                4,
                vec![DirtyRow {
                    index: 0,
                    row: row_of("dropped"),
                }],
            ))),
        ]))
        .unwrap();

        // The delta was dropped.
        handle
            .with_replica(SurfaceId(3), |r| {
                assert_eq!(r.seq(), Seq(1));
                assert_eq!(r.row(0).unwrap(), &row_of("a"));
            })
            .unwrap();

        let events = handle.take_events();
        let gap = events
            .iter()
            .find_map(|e| match e {
                DataPlaneEvent::Gap(gap) => Some(*gap),
                _ => None,
            })
            .expect("a Gap event");
        assert_eq!(gap.have, Seq(1));
        assert_eq!(gap.since, Seq(4));

        // The last message out is a re-Attach asking for a Snapshot.
        let sent = sink.messages();
        assert_eq!(
            sent.last(),
            Some(&DataMsg::Attach(Attach {
                surface_id: SurfaceId(3),
                mode: AttachMode::Active,
                want_snapshot: true,
                known_seq: Seq::ZERO,
            }))
        );
    }

    #[test]
    fn out_of_band_messages_become_events() {
        let (shared, _) = wired(DataPlaneOptions::default());
        attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));
        handle.attach(SurfaceId(3), AttachMode::Passive).unwrap();

        let status = ExitStatus {
            code: Some(2),
            signal: None,
        };
        core.feed(&wire(&[
            DataMsg::HelloAck(HelloAck {
                proto_version: PROTO_VERSION,
                server_build_id: "cafe".into(),
                workspace_revision: 1,
                server_pid: 42,
            }),
            DataMsg::Bell(Bell {
                surface_id: SurfaceId(3),
            }),
            DataMsg::DataError(DataError {
                surface_id: Some(SurfaceId(3)),
                code: st_proto::DATA_ERR_SURFACE_EXITED,
                message: "gone".into(),
            }),
            DataMsg::SurfaceExited(SurfaceExited {
                surface_id: SurfaceId(3),
                seq: Seq(9),
                status,
            }),
            DataMsg::Detached(Detached {
                surface_id: SurfaceId(3),
                reason: DetachReason::SurfaceDestroyed,
            }),
        ]))
        .unwrap();

        let events = handle.take_events();
        assert!(matches!(
            events[0],
            DataPlaneEvent::Connected { server_pid: 42, .. }
        ));
        assert_eq!(events[1], DataPlaneEvent::Bell(SurfaceId(3)));
        assert!(matches!(events[2], DataPlaneEvent::Error(_)));
        assert_eq!(
            events[3],
            DataPlaneEvent::Exited {
                surface_id: SurfaceId(3),
                status
            }
        );
        assert_eq!(
            events[4],
            DataPlaneEvent::Detached {
                surface_id: SurfaceId(3),
                reason: DetachReason::SurfaceDestroyed
            }
        );
        // A Detached forgets the attachment so a reconnect does not restore it.
        assert!(shared.attachments.lock().is_empty());
        handle
            .with_replica(SurfaceId(3), |r| {
                assert_eq!(r.exited(), Some(status));
                assert_eq!(r.seq(), Seq(9));
            })
            .unwrap();
    }

    #[test]
    fn a_reject_is_fatal() {
        let (shared, _) = wired(DataPlaneOptions::default());
        attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));

        let err = core
            .feed(&wire(&[DataMsg::Reject(Reject {
                reason: RejectReason::MajorMismatch,
                message: "server speaks 2.x".into(),
                server_version: ProtoVersion::new(2, 0),
            })]))
            .unwrap_err();
        assert!(matches!(err, DataPlaneError::Rejected(_)));
        assert!(err.to_string().contains("server speaks 2.x"));
        assert!(matches!(
            handle.take_events().first(),
            Some(DataPlaneEvent::Rejected(_))
        ));
    }

    #[test]
    fn a_client_to_server_message_from_the_server_is_an_error() {
        let (shared, _) = wired(DataPlaneOptions::default());
        let mut core = DataPlaneCore::new(shared);
        let err = core
            .feed(&wire(&[DataMsg::Detach(Detach {
                surface_id: SurfaceId(1),
            })]))
            .unwrap_err();
        assert!(matches!(
            err,
            DataPlaneError::UnexpectedMessage(st_proto::msg_type::DETACH)
        ));
    }

    #[test]
    fn an_unknown_message_type_is_skipped_not_fatal() {
        let (shared, _) = wired(DataPlaneOptions::default());
        attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));

        let mut bytes = Vec::new();
        // 0x0180 is unallocated in 1.0.
        encode_frame(0x0180, b"whatever", &mut bytes).unwrap();
        bytes.extend_from_slice(&wire(&[DataMsg::Snapshot(Box::new(snapshot(1, &["ok"])))]));

        core.feed(&bytes).unwrap();
        assert_eq!(handle.surfaces(), vec![SurfaceId(3)]);
    }

    #[test]
    fn frames_split_across_reads_are_reassembled() {
        let (shared, _) = wired(DataPlaneOptions::default());
        attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));

        let bytes = wire(&[
            DataMsg::Snapshot(Box::new(snapshot(1, &["split"]))),
            DataMsg::Delta(Box::new(delta(
                2,
                1,
                vec![DirtyRow {
                    index: 0,
                    row: row_of("joined"),
                }],
            ))),
        ]);
        for byte in &bytes {
            core.feed(std::slice::from_ref(byte)).unwrap();
        }
        handle
            .with_replica(SurfaceId(3), |r| {
                assert_eq!(r.seq(), Seq(2));
                assert_eq!(r.row(0).unwrap(), &row_of("joined"));
            })
            .unwrap();
    }

    #[test]
    fn outbound_messages_have_the_right_shape() {
        let (shared, _) = wired(DataPlaneOptions::default());
        let sink = attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));

        handle.attach(SurfaceId(1), AttachMode::Active).unwrap();
        handle.resize(SurfaceId(1), 120, 40).unwrap();
        handle.send_input(SurfaceId(1), b"ls\r").unwrap();
        handle.send_input(SurfaceId(1), b"").unwrap();
        handle
            .fetch_history(SurfaceId(1), AbsLine(500), 9999)
            .unwrap();
        handle.ack(SurfaceId(1), Seq(4)).unwrap();
        handle.detach(SurfaceId(1)).unwrap();

        assert_eq!(
            sink.messages(),
            vec![
                DataMsg::Attach(Attach {
                    surface_id: SurfaceId(1),
                    mode: AttachMode::Active,
                    want_snapshot: true,
                    known_seq: Seq::ZERO,
                }),
                DataMsg::Resize(Resize {
                    surface_id: SurfaceId(1),
                    cols: 120,
                    rows: 40
                }),
                DataMsg::Input(Input {
                    surface_id: SurfaceId(1),
                    bytes: b"ls\r".to_vec()
                }),
                DataMsg::FetchHistory(FetchHistory {
                    surface_id: SurfaceId(1),
                    from_line: AbsLine(500),
                    // Clamped to MAX_HISTORY_COUNT.
                    count: st_proto::MAX_HISTORY_COUNT,
                }),
                DataMsg::Ack(Ack {
                    surface_id: SurfaceId(1),
                    seq: Seq(4)
                }),
                DataMsg::Detach(Detach {
                    surface_id: SurfaceId(1)
                }),
            ]
        );
        assert!(shared.attachments.lock().is_empty());
    }

    #[test]
    fn a_second_attach_carries_the_known_seq() {
        let (shared, _) = wired(DataPlaneOptions::default());
        let sink = attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));

        handle.attach(SurfaceId(3), AttachMode::Active).unwrap();
        core.feed(&wire(&[DataMsg::Snapshot(Box::new(snapshot(11, &["x"])))]))
            .unwrap();
        handle.attach(SurfaceId(3), AttachMode::Passive).unwrap();

        let sent = sink.messages();
        assert_eq!(
            sent.last(),
            Some(&DataMsg::Attach(Attach {
                surface_id: SurfaceId(3),
                mode: AttachMode::Passive,
                want_snapshot: false,
                known_seq: Seq(11),
            }))
        );
    }

    #[test]
    fn a_long_paste_is_chunked() {
        let (shared, _) = wired(DataPlaneOptions::default());
        let sink = attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(shared);

        let big = vec![b'x'; MAX_INPUT_BYTES * 2 + 7];
        handle.send_input(SurfaceId(1), &big).unwrap();

        let sent = sink.messages();
        assert_eq!(sent.len(), 3);
        let lens: Vec<usize> = sent
            .iter()
            .map(|m| match m {
                DataMsg::Input(i) => i.bytes.len(),
                other => panic!("expected Input, got {other:?}"),
            })
            .collect();
        assert_eq!(lens, vec![MAX_INPUT_BYTES, MAX_INPUT_BYTES, 7]);
    }

    #[test]
    fn sends_without_a_connection_are_a_clean_error() {
        let (shared, _) = wired(DataPlaneOptions::default());
        let handle = DataPlaneHandle::from_shared(shared);
        assert!(!handle.is_connected());
        assert!(matches!(
            handle.attach(SurfaceId(1), AttachMode::Active),
            Err(DataPlaneError::NotConnected)
        ));
        assert!(matches!(
            handle.send_input(SurfaceId(1), b"x"),
            Err(DataPlaneError::NotConnected)
        ));
        // The attachment is still remembered, so a later connect re-attaches.
        assert_eq!(handle.shared().attachments.lock().len(), 1);
    }

    #[test]
    fn replicas_can_be_read_written_and_forgotten() {
        let (shared, _) = wired(DataPlaneOptions::default());
        attach_sink(&shared);
        let handle = DataPlaneHandle::from_shared(Arc::clone(&shared));
        let mut core = DataPlaneCore::new(Arc::clone(&shared));

        core.feed(&wire(&[DataMsg::Snapshot(Box::new(snapshot(1, &["a"])))]))
            .unwrap();
        assert!(handle.with_replica(SurfaceId(99), |_| ()).is_none());

        handle.with_replica_mut(SurfaceId(3), |r| r.shrink_history_to(0));
        assert_eq!(
            handle.with_replica(SurfaceId(3), Replica::cached_history_len),
            Some(0)
        );

        handle.forget(SurfaceId(3));
        assert!(handle.surfaces().is_empty());
    }

    #[test]
    fn auto_ack_can_be_turned_off() {
        let (shared, _) = wired(DataPlaneOptions {
            auto_ack: false,
            ..DataPlaneOptions::default()
        });
        let sink = attach_sink(&shared);
        let mut core = DataPlaneCore::new(Arc::clone(&shared));
        core.feed(&wire(&[DataMsg::Snapshot(Box::new(snapshot(1, &["a"])))]))
            .unwrap();
        assert!(sink.messages().is_empty());
    }
}

#[cfg(all(test, unix))]
mod socket_tests {
    //! Round trip against a fake in-process server that speaks the real frame
    //! codec over a real Unix socket.

    use super::tests_support::*;
    use super::*;
    use st_proto::HelloAck;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// Blocks until `f` is true or a second goes by.
    fn wait_for(mut f: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        f()
    }

    #[test]
    fn handshake_attach_and_snapshot_over_a_real_socket() {
        let dir = TempDir::new("st-dataplane-roundtrip");
        let path = dir.path().join("data.sock");
        let listener = UnixListener::bind(&path).expect("bind");

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut decoder = FrameDecoder::expecting_magic();
            let mut buf = [0u8; 4096];
            let mut got = Vec::new();

            // Read until we have both the Hello and the Attach.
            while got.len() < 2 {
                let n = stream.read(&mut buf).expect("read");
                assert!(n > 0, "client closed early");
                decoder.push(&buf[..n]);
                while let Some(frame) = decoder.next_frame().expect("framing") {
                    got.push(DataMsg::from_frame(frame.msg_type, &frame.payload).expect("decode"));
                }
                // Answer the Hello as soon as we see it.
                if got.len() == 1 {
                    let ack = DataMsg::HelloAck(HelloAck {
                        proto_version: PROTO_VERSION,
                        server_build_id: "fake-server".into(),
                        workspace_revision: 7,
                        server_pid: 1234,
                    });
                    let mut out = Vec::new();
                    ack.encode_to(&mut out).unwrap();
                    stream.write_all(&out).expect("write ack");
                }
            }

            // Answer the Attach with a Snapshot.
            let mut out = Vec::new();
            DataMsg::Snapshot(Box::new(snapshot_for(
                SurfaceId(5),
                1,
                &["from the server"],
            )))
            .encode_to(&mut out)
            .unwrap();
            DataMsg::Delta(Box::new(delta_for(SurfaceId(5), 2, 1, "second line")))
                .encode_to(&mut out)
                .unwrap();
            stream.write_all(&out).expect("write snapshot");

            // Keep reading so the client's acks do not block, until it closes.
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            got
        });

        let wakes = Arc::new(AtomicUsize::new(0));
        let for_wake = Arc::clone(&wakes);
        let conn = DataPlaneConnection::connect(
            &path,
            DataPlaneOptions {
                build_id: "test-build".into(),
                reconnect: false,
                ..DataPlaneOptions::default()
            },
            Box::new(move || {
                for_wake.fetch_add(1, Ordering::AcqRel);
            }),
        )
        .expect("connect");

        assert!(
            wait_for(|| conn.is_connected()),
            "the I/O thread should connect"
        );
        conn.attach(SurfaceId(5), AttachMode::Active)
            .expect("attach");

        assert!(
            wait_for(|| conn.with_replica(SurfaceId(5), |r| r.seq()) == Some(Seq(2))),
            "the snapshot and delta should have landed"
        );

        // The wake callback fired.
        assert!(wakes.load(Ordering::Acquire) > 0);
        assert_eq!(conn.take_dirty(), vec![SurfaceId(5)]);

        let rows = conn
            .with_replica(SurfaceId(5), |r| {
                (
                    r.row(0).map(|row| row_text(row, r.cols())),
                    r.row(1).map(|row| row_text(row, r.cols())),
                    r.cols(),
                )
            })
            .unwrap();
        assert_eq!(rows.0.as_deref(), Some("from the server"));
        assert_eq!(rows.1.as_deref(), Some("second line"));
        assert_eq!(rows.2, 20);

        // The Connected event carries the server's identity.
        let events = conn.take_events();
        assert!(events.iter().any(|e| matches!(
            e,
            DataPlaneEvent::Connected {
                server_pid: 1234,
                ..
            }
        )));

        conn.shutdown();
        let received = server.join().expect("server thread");
        assert!(matches!(
            &received[0],
            DataMsg::Hello(h) if h.client_kind == ClientKind::Data && h.build_id == "test-build"
        ));
        assert_eq!(
            received[1],
            DataMsg::Attach(Attach {
                surface_id: SurfaceId(5),
                mode: AttachMode::Active,
                want_snapshot: true,
                known_seq: Seq::ZERO,
            })
        );
    }

    #[test]
    fn a_closed_socket_reports_a_disconnect() {
        let (client, server) = std::os::unix::net::UnixStream::pair().expect("pair");
        let conn = DataPlaneConnection::from_stream(
            client,
            DataPlaneOptions {
                reconnect: false,
                ..DataPlaneOptions::default()
            },
            Box::new(|| {}),
        );
        assert!(wait_for(|| conn.is_connected()));
        drop(server);
        assert!(wait_for(|| conn
            .take_events()
            .iter()
            .any(|e| matches!(e, DataPlaneEvent::Disconnected { .. }))
            || !conn.is_connected()));
        conn.shutdown();
    }

    #[test]
    fn connecting_to_a_missing_socket_fails_immediately() {
        let dir = TempDir::new("st-dataplane-missing");
        let err = DataPlaneConnection::connect(
            dir.path().join("nope.sock"),
            DataPlaneOptions::default(),
            Box::new(|| {}),
        )
        .unwrap_err();
        assert!(matches!(err, DataPlaneError::Io(_)));
    }
}

#[cfg(test)]
mod tests_support {
    //! Fixtures shared between the in-memory and the socket tests.

    use st_proto::{
        AbsLine, Cursor, Delta, DirtyRow, Modes, PackedCell, Row, Seq, Snapshot, Style, StyleIdx,
        SurfaceId, ViewState,
    };

    /// A minimal directory that removes itself, so the socket tests need no
    /// `tempfile` dependency.
    pub struct TempDir(std::path::PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let unique = format!(
                "{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn row_of(text: &str) -> Row {
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

    pub fn row_text(row: &Row, cols: u16) -> String {
        (0..cols)
            .map(|c| char::from_u32(row.cell_at(c as usize).codepoint).unwrap_or(' '))
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    pub fn snapshot_for(surface_id: SurfaceId, seq: u64, lines: &[&str]) -> Snapshot {
        Snapshot {
            surface_id,
            seq: Seq(seq),
            cols: 20,
            rows: 2,
            styles: vec![Style::DEFAULT],
            grid: lines.iter().map(|l| row_of(l)).collect(),
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: "sh".into(),
            history_base: AbsLine(0),
            history_len: 0,
            view_state: ViewState::default(),
            exited: None,
        }
    }

    pub fn delta_for(surface_id: SurfaceId, seq: u64, since: u64, text: &str) -> Delta {
        Delta {
            surface_id,
            seq: Seq(seq),
            since_seq: Seq(since),
            history_base: AbsLine(0),
            history_len: 0,
            resized: None,
            new_styles: Vec::new(),
            rows: vec![DirtyRow {
                index: 1,
                row: row_of(text),
            }],
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: None,
        }
    }
}
