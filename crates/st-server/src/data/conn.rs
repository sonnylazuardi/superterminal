//! One DATA connection — `02-protocol.md` §1.3, §2, §4, §9 and
//! `03-server.md` §7.
//!
//! The control plane's accept loop sniffs the first byte, verifies the 4-byte
//! magic `0xFF "STD"` (grilling Q37) and hands the stream here. This module
//! then owns the connection for its whole life:
//!
//! 1. `Hello` → `HelloAck`, or `Reject` on a major-version gap;
//! 2. loop over frames, dispatching to the Surface supervisor;
//! 3. on close, detach from every Surface the connection had attached.
//!
//! Outbound frames never block the reader: everything the connection is owed
//! — including frames the 120 Hz pump produces from another task — goes
//! through a bounded channel drained by a dedicated writer task.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::control::Conn;
use st_core::publisher::ClientId as CoreClientId;
use st_core::surface::SurfaceStatus;
use st_proto::data::SetViewState;
use st_proto::{
    Ack, Attach, CodecError, DataError, DataMsg, Detach, Detached, FetchHistory, FrameDecoder,
    FrameError, Hello, HelloAck, Input, Reject, RejectReason, Resize, SurfaceId, ViewState,
    DATA_ERR_BAD_REQUEST, DATA_ERR_NOT_ATTACHED, DATA_ERR_SURFACE_EXITED, MAX_HISTORY_COUNT,
    MAX_INPUT_BYTES, PROTO_VERSION,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::{mpsc, Notify};

use crate::supervisor::{SurfaceSupervisor, Upcall};
use crate::workspace::{ClientId, SurfaceEvent};

/// Bytes read from the socket in one `read(2)`.
const READ_CHUNK: usize = 64 * 1024;

/// Everything a DATA connection needs from the rest of the daemon.
///
/// Cheap to clone; [`crate::data::acceptor`] builds one per connection.
#[derive(Clone)]
pub struct DataCtx {
    /// Owns the Surfaces and the connection table.
    pub supervisor: Arc<SurfaceSupervisor>,
    /// Reported in `HelloAck.server_build_id`.
    pub build_id: String,
    /// Reported in `HelloAck.server_pid`.
    pub server_pid: u32,
    /// Reported in `HelloAck.workspace_revision`.
    pub workspace_revision: u64,
    /// Budget for the `Hello` frame (`02-protocol.md` §2 rule 1).
    pub handshake_timeout: Duration,
}

impl std::fmt::Debug for DataCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataCtx")
            .field("build_id", &self.build_id)
            .field("server_pid", &self.server_pid)
            .field("workspace_revision", &self.workspace_revision)
            .finish_non_exhaustive()
    }
}

impl DataCtx {
    /// A context around a supervisor, with the defaults from §2.
    #[must_use]
    pub fn new(supervisor: Arc<SurfaceSupervisor>) -> Self {
        Self {
            supervisor,
            build_id: String::new(),
            server_pid: std::process::id(),
            workspace_revision: 0,
            handshake_timeout: Duration::from_secs(st_proto::frame::HANDSHAKE_TIMEOUT_SECS),
        }
    }

    /// Sets the build id echoed in `HelloAck`.
    #[must_use]
    pub fn with_build_id(mut self, build_id: impl Into<String>) -> Self {
        self.build_id = build_id.into();
        self
    }

    /// Sets the Workspace revision echoed in `HelloAck`.
    #[must_use]
    pub fn with_workspace_revision(mut self, revision: u64) -> Self {
        self.workspace_revision = revision;
        self
    }
}

/// Serves one DATA connection whose magic the accept loop already consumed.
///
/// This is the shape [`crate::control::DataAcceptor`] promises. Errors are
/// logged rather than propagated: a broken client must never take the daemon
/// down.
pub async fn accept(stream: Conn, ctx: DataCtx, client: ClientId) {
    serve_connection(stream, Vec::new(), ctx, client).await;
}

/// Serves one DATA connection that still has its 4-byte magic in the stream.
///
/// Only the tests and standalone tooling need this; the daemon's accept loop
/// consumes the magic while sniffing the plane.
pub async fn accept_with_magic(stream: Conn, ctx: DataCtx, client: ClientId) {
    serve_connection_expecting_magic(stream, ctx, client).await;
}

async fn serve_connection_expecting_magic(stream: Conn, ctx: DataCtx, client: ClientId) {
    serve_inner(stream, ctx, client, true, Vec::new()).await;
}

async fn serve_connection(stream: Conn, prefix: Vec<u8>, ctx: DataCtx, client: ClientId) {
    serve_inner(stream, ctx, client, false, prefix).await;
}

async fn serve_inner(
    stream: Conn,
    ctx: DataCtx,
    client: ClientId,
    expect_magic: bool,
    prefix: Vec<u8>,
) {
    ctx.supervisor.ensure_pump();
    let core_client = CoreClientId::new(client.0);

    let (reader, writer) = tokio::io::split(stream);
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(ctx.supervisor.config().outbound_capacity);
    let shutdown = Arc::new(Notify::new());
    ctx.supervisor
        .register_client(core_client, out_tx, Arc::clone(&shutdown));
    let writer_task = tokio::spawn(pump_socket(writer, out_rx));

    let span = tracing::info_span!("conn", id = client.0, plane = "data");
    let _guard = span.enter();

    let mut decoder = if expect_magic {
        FrameDecoder::expecting_magic()
    } else {
        FrameDecoder::new()
    };
    decoder.push(&prefix);

    if let Err(err) = serve(reader, decoder, &ctx, core_client, &shutdown).await {
        tracing::debug!(%err, "data connection ended");
    }

    ctx.supervisor.unregister_client(core_client);
    // Dropping the registry's sender closes the channel; awaiting the writer
    // guarantees a final `Reject` or `Detached` actually reaches the socket.
    let _ = writer_task.await;
}

/// Why a connection stopped. All of these are ordinary, not bugs.
#[derive(Debug, thiserror::Error)]
enum ConnError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing: {0}")]
    Frame(#[from] FrameError),
    #[error("rejected: {0:?}")]
    Rejected(RejectReason),
    #[error("handshake timed out")]
    HandshakeTimeout,
    #[error("closed by the server")]
    Closed,
    #[error("end of stream")]
    Eof,
}

async fn serve(
    mut reader: ReadHalf<Conn>,
    mut decoder: FrameDecoder,
    ctx: &DataCtx,
    client: CoreClientId,
    shutdown: &Notify,
) -> Result<(), ConnError> {
    let mut buf = vec![0u8; READ_CHUNK];

    // ---- handshake (§2) -------------------------------------------------
    let hello = tokio::time::timeout(
        ctx.handshake_timeout,
        next_incoming(&mut reader, &mut decoder, &mut buf),
    )
    .await
    .map_err(|_| ConnError::HandshakeTimeout)?;

    match hello {
        Ok(Incoming::Msg(DataMsg::Hello(hello))) => greet(ctx, client, &hello)?,
        Ok(Incoming::Eof) => return Err(ConnError::Eof),
        Ok(_) => {
            reject(ctx, client, RejectReason::NotHello, "expected Hello first");
            return Err(ConnError::Rejected(RejectReason::NotHello));
        }
        Err(ConnError::Frame(err)) => {
            let reason = err.reject_reason();
            reject(ctx, client, reason, &err.to_string());
            return Err(ConnError::Rejected(reason));
        }
        Err(err) => return Err(err),
    }

    // ---- steady state (§4) ----------------------------------------------
    let mut attached: BTreeSet<SurfaceId> = BTreeSet::new();
    let result = loop {
        let incoming = tokio::select! {
            () = shutdown.notified() => break Err(ConnError::Closed),
            incoming = next_incoming(&mut reader, &mut decoder, &mut buf) => incoming,
        };
        match incoming {
            Ok(Incoming::Msg(msg)) => handle(ctx, client, &mut attached, msg),
            Ok(Incoming::Undecodable(msg_type, err)) => {
                tracing::warn!(msg_type, %err, "undecodable data frame");
                send_error(
                    ctx,
                    client,
                    None,
                    DATA_ERR_BAD_REQUEST,
                    format!("cannot decode msg_type 0x{msg_type:04X}: {err}"),
                );
            }
            Ok(Incoming::Eof) => break Err(ConnError::Eof),
            Err(err) => break Err(err),
        }
    };

    for surface in attached {
        if let Some(slot) = ctx.supervisor.slot(surface) {
            slot.lock().detach(client);
        }
    }
    result
}

fn greet(ctx: &DataCtx, client: CoreClientId, hello: &Hello) -> Result<(), ConnError> {
    if !PROTO_VERSION.compatible_with(hello.proto_version) {
        let message = format!(
            "protocol major mismatch: this server speaks {PROTO_VERSION}, the client speaks {}",
            hello.proto_version
        );
        reject(ctx, client, RejectReason::MajorMismatch, &message);
        return Err(ConnError::Rejected(RejectReason::MajorMismatch));
    }
    let negotiated = PROTO_VERSION
        .negotiate(hello.proto_version)
        .unwrap_or(PROTO_VERSION);
    tracing::debug!(
        version = %negotiated,
        kind = ?hello.client_kind,
        build = %hello.build_id,
        "data handshake"
    );
    ctx.supervisor.send(
        client,
        &DataMsg::HelloAck(HelloAck {
            proto_version: negotiated,
            server_build_id: ctx.build_id.clone(),
            workspace_revision: ctx.workspace_revision,
            server_pid: ctx.server_pid,
        }),
    );
    Ok(())
}

fn reject(ctx: &DataCtx, client: CoreClientId, reason: RejectReason, message: &str) {
    tracing::info!(?reason, message, "rejecting a data connection");
    ctx.supervisor.send(
        client,
        &DataMsg::Reject(Reject {
            reason,
            message: message.to_owned(),
            server_version: PROTO_VERSION,
        }),
    );
}

// ------------------------------------------------------------- dispatching

fn handle(ctx: &DataCtx, client: CoreClientId, attached: &mut BTreeSet<SurfaceId>, msg: DataMsg) {
    match msg {
        DataMsg::Attach(attach) => on_attach(ctx, client, attached, &attach),
        DataMsg::Detach(detach) => on_detach(ctx, client, attached, &detach),
        DataMsg::Input(input) => on_input(ctx, client, input),
        DataMsg::Resize(resize) => on_resize(ctx, client, &resize),
        DataMsg::FetchHistory(fetch) => on_fetch_history(ctx, client, &fetch),
        DataMsg::Ack(ack) => on_ack(ctx, client, &ack),
        DataMsg::SetViewState(view) => on_set_view_state(ctx, client, &view),
        DataMsg::Hello(_) => send_error(
            ctx,
            client,
            None,
            DATA_ERR_BAD_REQUEST,
            "the handshake is already complete".to_owned(),
        ),
        other => send_error(
            ctx,
            client,
            None,
            DATA_ERR_BAD_REQUEST,
            format!(
                "msg_type 0x{:04X} is server → client only",
                other.msg_type()
            ),
        ),
    }
}

/// `Attach` (§6). The first frame is always a Snapshot, which is a superset of
/// what `want_snapshot` / `known_seq` ask for.
fn on_attach(
    ctx: &DataCtx,
    client: CoreClientId,
    attached: &mut BTreeSet<SurfaceId>,
    msg: &Attach,
) {
    let Some(slot) = ctx.supervisor.slot(msg.surface_id) else {
        return unknown_surface(ctx, client, msg.surface_id);
    };
    let accepted = slot.lock().attach(client, msg.mode, Instant::now());
    if !accepted {
        return send_error(
            ctx,
            client,
            Some(msg.surface_id),
            DATA_ERR_BAD_REQUEST,
            format!("already attached to surface {}", msg.surface_id),
        );
    }
    attached.insert(msg.surface_id);
    tracing::debug!(
        surface = %msg.surface_id,
        mode = ?msg.mode,
        want_snapshot = msg.want_snapshot,
        known_seq = %msg.known_seq,
        "attached"
    );
}

fn on_detach(
    ctx: &DataCtx,
    client: CoreClientId,
    attached: &mut BTreeSet<SurfaceId>,
    msg: &Detach,
) {
    let Some(slot) = ctx.supervisor.slot(msg.surface_id) else {
        return unknown_surface(ctx, client, msg.surface_id);
    };
    if !slot.lock().detach(client) {
        return send_error(
            ctx,
            client,
            Some(msg.surface_id),
            DATA_ERR_NOT_ATTACHED,
            format!("not attached to surface {}", msg.surface_id),
        );
    }
    attached.remove(&msg.surface_id);
    ctx.supervisor.send(
        client,
        &DataMsg::Detached(Detached {
            surface_id: msg.surface_id,
            reason: st_proto::DetachReason::Requested,
        }),
    );
}

/// `Input` (§9). Bytes go to the PTY verbatim. Writing to an exited Surface is
/// a per-message `DataError`, never fatal (grilling Q48).
fn on_input(ctx: &DataCtx, client: CoreClientId, msg: Input) {
    let Some(slot) = ctx.supervisor.slot(msg.surface_id) else {
        return unknown_surface(ctx, client, msg.surface_id);
    };
    if msg.bytes.len() > MAX_INPUT_BYTES {
        return send_error(
            ctx,
            client,
            Some(msg.surface_id),
            DATA_ERR_BAD_REQUEST,
            format!(
                "Input carries {} bytes, over the {MAX_INPUT_BYTES} byte limit",
                msg.bytes.len()
            ),
        );
    }
    if matches!(slot.lock().status(), SurfaceStatus::Exited(_)) {
        return send_error(
            ctx,
            client,
            Some(msg.surface_id),
            DATA_ERR_SURFACE_EXITED,
            format!("surface {} has exited", msg.surface_id),
        );
    }
    if msg.bytes.is_empty() {
        return;
    }
    if !slot.write_pty(msg.bytes) {
        return send_error(
            ctx,
            client,
            Some(msg.surface_id),
            DATA_ERR_SURFACE_EXITED,
            format!("surface {} has no writable pty", msg.surface_id),
        );
    }
    // Q42: a Surface that has been typed into is no longer pristine.
    ctx.supervisor.notify(Upcall::Surface(SurfaceEvent::Input {
        surface: msg.surface_id,
    }));
}

/// `Resize` (§9, Q40). Last writer wins, the selection is cleared and the
/// cleared View State is broadcast.
fn on_resize(ctx: &DataCtx, client: CoreClientId, msg: &Resize) {
    let Some(slot) = ctx.supervisor.slot(msg.surface_id) else {
        return unknown_surface(ctx, client, msg.surface_id);
    };
    if msg.cols == 0 || msg.rows == 0 {
        return send_error(
            ctx,
            client,
            Some(msg.surface_id),
            DATA_ERR_BAD_REQUEST,
            "Resize must be at least 1×1".to_owned(),
        );
    }
    let view = {
        let mut surface = slot.lock();
        match surface.resize(msg.cols, msg.rows) {
            Ok(()) => surface.view_state().clone(),
            Err(err) => {
                drop(surface);
                return send_error(
                    ctx,
                    client,
                    Some(msg.surface_id),
                    DATA_ERR_BAD_REQUEST,
                    format!("resize failed: {err}"),
                );
            }
        }
    };
    ctx.supervisor
        .notify(Upcall::Surface(SurfaceEvent::Resized {
            surface: msg.surface_id,
            cols: msg.cols,
            rows: msg.rows,
        }));
    ctx.supervisor.notify(Upcall::ViewState {
        surface: msg.surface_id,
        view,
        origin: None,
    });
}

/// `FetchHistory` (§8). Answered immediately, outside the ack window.
fn on_fetch_history(ctx: &DataCtx, client: CoreClientId, msg: &FetchHistory) {
    let Some(slot) = ctx.supervisor.slot(msg.surface_id) else {
        return unknown_surface(ctx, client, msg.surface_id);
    };
    let count = u32::from(msg.count.min(MAX_HISTORY_COUNT));
    let page = slot.lock().history(msg.from_line, count);
    super::pump::send(&ctx.supervisor, client, DataMsg::History(Box::new(page)));
}

fn on_ack(ctx: &DataCtx, client: CoreClientId, msg: &Ack) {
    let Some(slot) = ctx.supervisor.slot(msg.surface_id) else {
        return unknown_surface(ctx, client, msg.surface_id);
    };
    slot.lock().ack(client, msg.seq, Instant::now());
}

/// `SetViewState` (grilling Q43/Q49). Stored on the Surface *and* pushed
/// through the Workspace actor, which echoes it on `ev.workspace`.
fn on_set_view_state(ctx: &DataCtx, client: CoreClientId, msg: &SetViewState) {
    let Some(slot) = ctx.supervisor.slot(msg.surface) else {
        return unknown_surface(ctx, client, msg.surface);
    };
    let view = {
        let mut surface = slot.lock();
        // The wire carries the absolute id of the top visible line; the stored
        // form is the distance from the bottom, which stays correct as new
        // output arrives (`st-client-core`'s `Replica::viewport_range`).
        let first_visible = surface.history_base().get() + surface.history_len();
        let scroll_offset = msg.scroll_offset.map_or(0, |top| {
            u32::try_from(first_visible.saturating_sub(top.get())).unwrap_or(u32::MAX)
        });
        let view = ViewState {
            scroll_offset,
            selection: msg.selection,
        };
        surface.set_view_state(view.clone());
        view
    };
    ctx.supervisor.notify(Upcall::ViewState {
        surface: msg.surface,
        view,
        origin: Some(ClientId(client.get())),
    });
}

fn unknown_surface(ctx: &DataCtx, client: CoreClientId, surface: SurfaceId) {
    send_error(
        ctx,
        client,
        Some(surface),
        DATA_ERR_BAD_REQUEST,
        format!("no such surface: {surface}"),
    );
}

fn send_error(
    ctx: &DataCtx,
    client: CoreClientId,
    surface_id: Option<SurfaceId>,
    code: u16,
    message: String,
) {
    ctx.supervisor.send(
        client,
        &DataMsg::DataError(DataError {
            surface_id,
            code,
            message,
        }),
    );
}

// ------------------------------------------------------------------- socket

/// What one read produced.
enum Incoming {
    /// A decoded message.
    Msg(DataMsg),
    /// A well-framed message this version cannot decode; per-message error.
    Undecodable(u16, CodecError),
    /// The peer closed its write half.
    Eof,
}

async fn next_incoming(
    reader: &mut ReadHalf<Conn>,
    decoder: &mut FrameDecoder,
    buf: &mut [u8],
) -> Result<Incoming, ConnError> {
    loop {
        if let Some(frame) = decoder.next_frame()? {
            return Ok(match DataMsg::from_frame(frame.msg_type, &frame.payload) {
                Ok(msg) => Incoming::Msg(msg),
                Err(err) => Incoming::Undecodable(frame.msg_type, err),
            });
        }
        let read = reader.read(buf).await?;
        if read == 0 {
            return Ok(Incoming::Eof);
        }
        decoder.push(&buf[..read]);
    }
}

/// Drains the outbound channel to the socket. Ends when the connection is
/// unregistered, which drops the sender.
async fn pump_socket(mut writer: WriteHalf<Conn>, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(frame) = rx.recv().await {
        if writer.write_all(&frame).await.is_err() {
            break;
        }
    }
    let _ = writer.flush().await;
    let _ = writer.shutdown().await;
}
