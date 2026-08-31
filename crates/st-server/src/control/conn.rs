//! One CONTROL connection — `02-protocol.md` §1.2, §2, §3.
//!
//! Newline-delimited JSON, UTF-8, at most [`MAX_CONTROL_LINE`] bytes per line.
//! The connection is three tasks' worth of work:
//!
//! * the **reader** (this function) parses one line at a time and dispatches
//!   it through [`handlers::handle`], so requests from one connection are
//!   applied in order;
//! * the **writer** owns the write half and serialises every outbound line,
//!   so responses and events can be produced concurrently without interleaving;
//! * the **event pump** forwards `ev.*` from the actor's broadcast, but only
//!   after `workspace.subscribe` (§3.1) and never the connection's own
//!   view-only echo (§3.3).

use std::sync::Arc;
use std::time::Duration;

use st_proto::control::{Handshake, Revision};
use st_proto::frame::{HelloAck, Reject, RejectReason, HANDSHAKE_TIMEOUT_SECS};
use st_proto::{ClientKind, MAX_CONTROL_LINE, PROTO_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::control::handlers;
use crate::workspace::{ClientId, EventEnvelope, WorkspaceHandle};
use crate::ServerContext;

/// Outbound line queue depth. A client that cannot keep up with this many
/// pending lines is not reading its socket at all, and gets disconnected.
const OUTBOUND_CAPACITY: usize = 256;

/// Serves one CONTROL connection to completion.
///
/// `first_byte` is the `{` the sniffer consumed; it is put back in front of
/// the stream so the first line parses.
pub async fn serve_control(
    stream: UnixStream,
    ctx: Arc<ServerContext>,
    client: ClientId,
    first_byte: u8,
) {
    ctx.metrics.control_clients.inc();
    let result = run(stream, &ctx, client, first_byte).await;
    ctx.metrics.control_clients.dec();

    match result {
        Ok(()) => tracing::debug!(client = client.0, "control connection closed"),
        Err(e) => tracing::debug!(client = client.0, error = %e, "control connection ended"),
    }
}

async fn run(
    stream: UnixStream,
    ctx: &Arc<ServerContext>,
    client: ClientId,
    first_byte: u8,
) -> std::io::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = LineReader::new(read_half, &[first_byte]);

    let (out_tx, out_rx) = mpsc::channel::<String>(OUTBOUND_CAPACITY);
    let writer = tokio::spawn(write_lines(write_half, out_rx, Arc::clone(ctx)));

    // The broadcast receiver is created *before* the handshake so that nothing
    // published between `workspace.subscribe` being answered and the pump
    // starting can be lost; the pump drops what the snapshot already covered.
    let events = ctx.workspace.subscribe();
    let (subscribe_tx, subscribe_rx) = oneshot::channel::<Revision>();
    let pump = tokio::spawn(pump_events(
        events,
        subscribe_rx,
        out_tx.clone(),
        client,
        ctx.workspace.clone(),
    ));

    let outcome = converse(&mut reader, ctx, client, &out_tx, subscribe_tx).await;

    drop(out_tx);
    pump.abort();
    let _ = writer.await;
    outcome
}

async fn converse(
    reader: &mut LineReader,
    ctx: &Arc<ServerContext>,
    client: ClientId,
    out: &mpsc::Sender<String>,
    subscribe_tx: oneshot::Sender<Revision>,
) -> std::io::Result<()> {
    // §2 rule 1: a connection that has not said `hello` within the budget is
    // closed. The budget covers the whole handshake, not just the first byte
    // the plane sniffer already read.
    let handshaken = tokio::time::timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        handshake(reader, ctx, client, out),
    )
    .await;
    match handshaken {
        Ok(result) => {
            if !result? {
                return Ok(());
            }
        }
        Err(_) => {
            tracing::debug!(client = client.0, "handshake timed out; closing");
            ctx.metrics.connections_refused.inc();
            return Ok(());
        }
    }

    let mut subscribe_tx = Some(subscribe_tx);
    loop {
        let line = match reader.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(LineError::TooLong) => {
                reject(out, RejectReason::LineTooLong, "control line exceeds 4 MiB").await;
                return Ok(());
            }
            Err(LineError::Io(e)) => return Err(e),
        };
        ctx.metrics.control_bytes_in.add(line.len() as u64 + 1);

        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let handled = match handlers::parse_request(&line) {
            Ok(req) => {
                tracing::trace!(client = client.0, tag = req.tag(), "request");
                handlers::handle(ctx, client, req).await
            }
            Err(res) => handlers::Handled {
                res,
                subscribe_at: None,
            },
        };

        if let (Some(at), Some(tx)) = (handled.subscribe_at, subscribe_tx.take()) {
            // Arm the pump *before* the response goes out, so an event that
            // races the response is still ordered after it by the writer.
            let _ = tx.send(at);
        } else if handled.subscribe_at.is_some() {
            tracing::debug!(client = client.0, "workspace.subscribe repeated; ignoring");
        }

        if send_json(out, &handled.res).await.is_err() {
            return Ok(());
        }
    }
}

/// The `Hello` / `HelloAck` / `Reject` exchange (§2). Returns `false` when the
/// connection was rejected and must be closed.
async fn handshake(
    reader: &mut LineReader,
    ctx: &Arc<ServerContext>,
    client: ClientId,
    out: &mpsc::Sender<String>,
) -> std::io::Result<bool> {
    let line = match reader.next_line().await {
        Ok(Some(line)) => line,
        Ok(None) => return Ok(false),
        Err(LineError::TooLong) => {
            reject(out, RejectReason::LineTooLong, "control line exceeds 4 MiB").await;
            return Ok(false);
        }
        Err(LineError::Io(e)) => return Err(e),
    };
    ctx.metrics.control_bytes_in.add(line.len() as u64 + 1);

    let hello = match serde_json::from_slice::<Handshake>(&line) {
        Ok(Handshake::Hello(hello)) => hello,
        Ok(_) => {
            reject(
                out,
                RejectReason::NotHello,
                "the first message must be `hello`",
            )
            .await;
            ctx.metrics.connections_refused.inc();
            return Ok(false);
        }
        Err(e) => {
            reject(
                out,
                RejectReason::NotHello,
                format!("the first message must be `hello`: {e}"),
            )
            .await;
            ctx.metrics.connections_refused.inc();
            return Ok(false);
        }
    };

    let Some(negotiated) = PROTO_VERSION.negotiate(hello.proto_version) else {
        reject(
            out,
            RejectReason::MajorMismatch,
            format!(
                "client speaks protocol {}, this server speaks {PROTO_VERSION}",
                hello.proto_version
            ),
        )
        .await;
        ctx.metrics.connections_refused.inc();
        return Ok(false);
    };

    if hello.client_kind == ClientKind::Data {
        // §2: the native client's DATA connection never speaks NDJSON.
        reject(
            out,
            RejectReason::NotHello,
            "client_kind `data` belongs on a data-plane connection",
        )
        .await;
        ctx.metrics.connections_refused.inc();
        return Ok(false);
    }

    let revision = ctx
        .workspace
        .stats()
        .await
        .map(|s| s.revision)
        .unwrap_or_default();

    tracing::info!(
        client = client.0,
        build = %hello.build_id,
        kind = ?hello.client_kind,
        version = %negotiated,
        "control client connected"
    );

    let ack = Handshake::HelloAck(HelloAck {
        proto_version: negotiated,
        server_build_id: ctx.build_id.clone(),
        workspace_revision: revision,
        server_pid: std::process::id(),
    });
    Ok(send_json(out, &ack).await.is_ok())
}

async fn reject(out: &mpsc::Sender<String>, reason: RejectReason, message: impl Into<String>) {
    let reject = Handshake::Reject(Reject {
        reason,
        message: message.into(),
        server_version: PROTO_VERSION,
    });
    let _ = send_json(out, &reject).await;
    // Give the writer a moment to flush before the caller drops the socket.
    tokio::task::yield_now().await;
}

async fn send_json<T: serde::Serialize>(out: &mpsc::Sender<String>, value: &T) -> Result<(), ()> {
    let line = serde_json::to_string(value).map_err(|_| ())?;
    out.send(line).await.map_err(|_| ())
}

/// Owns the write half so nothing else can interleave a partial line.
async fn write_lines(
    mut write_half: OwnedWriteHalf,
    mut rx: mpsc::Receiver<String>,
    ctx: Arc<ServerContext>,
) {
    while let Some(line) = rx.recv().await {
        let bytes = line.len() + 1;
        if write_half.write_all(line.as_bytes()).await.is_err()
            || write_half.write_all(b"\n").await.is_err()
        {
            return;
        }
        ctx.metrics.control_bytes_out.add(bytes as u64);
    }
    let _ = write_half.shutdown().await;
}

/// Forwards `ev.*` once the connection has subscribed (§3.1).
async fn pump_events(
    mut events: broadcast::Receiver<EventEnvelope>,
    subscribed: oneshot::Receiver<Revision>,
    out: mpsc::Sender<String>,
    client: ClientId,
    workspace: WorkspaceHandle,
) {
    let Ok(from) = subscribed.await else {
        return;
    };

    loop {
        match events.recv().await {
            Ok(envelope) => {
                if envelope.suppress == Some(client) {
                    // §3.3: a `view.set` is not echoed to its own author.
                    continue;
                }
                if envelope.revision.is_some_and(|r| r <= from) {
                    continue;
                }
                if out.send(envelope.json.to_string()).await.is_err() {
                    return;
                }
            }
            Err(RecvError::Lagged(missed)) => {
                tracing::debug!(client = client.0, missed, "event stream lagged; resyncing");
                let Ok(snapshot) = workspace.snapshot().await else {
                    return;
                };
                let event = handlers::workspace_event_from_snapshot(&snapshot);
                if out.send(event.to_string()).await.is_err() {
                    return;
                }
            }
            Err(RecvError::Closed) => return,
        }
    }
}

// ------------------------------------------------------------ line framing

/// Why reading a line failed.
#[derive(Debug, thiserror::Error)]
enum LineError {
    /// The line exceeded [`MAX_CONTROL_LINE`] (§1.2).
    #[error("control line exceeds {MAX_CONTROL_LINE} bytes")]
    TooLong,
    /// The socket failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reads `\n`-terminated lines with a hard size cap.
///
/// `tokio`'s `read_until` has no cap, and a 4 MiB limit that is only checked
/// after the allocation is not a limit, so the loop is written by hand.
struct LineReader {
    inner: BufReader<OwnedReadHalf>,
    buf: Vec<u8>,
    prefix: Vec<u8>,
}

impl LineReader {
    fn new(read_half: OwnedReadHalf, prefix: &[u8]) -> Self {
        Self {
            inner: BufReader::new(read_half),
            buf: Vec::with_capacity(1024),
            prefix: prefix.to_vec(),
        }
    }

    /// The next line without its terminator, or `None` at a clean EOF.
    async fn next_line(&mut self) -> Result<Option<Vec<u8>>, LineError> {
        self.buf.clear();
        self.buf.append(&mut self.prefix);

        loop {
            let consumed;
            let mut complete = false;
            {
                let available = self.inner.fill_buf().await?;
                if available.is_empty() {
                    return if self.buf.is_empty() {
                        Ok(None)
                    } else {
                        // A final line without a trailing newline still counts.
                        Ok(Some(std::mem::take(&mut self.buf)))
                    };
                }
                match available.iter().position(|&b| b == b'\n') {
                    Some(at) => {
                        self.buf.extend_from_slice(&available[..at]);
                        consumed = at + 1;
                        complete = true;
                    }
                    None => {
                        self.buf.extend_from_slice(available);
                        consumed = available.len();
                    }
                }
            }
            self.inner.consume(consumed);

            if self.buf.len() > MAX_CONTROL_LINE {
                return Err(LineError::TooLong);
            }
            if complete {
                let mut line = std::mem::take(&mut self.buf);
                // `\r` is not allowed inside a line (§1.2), but tolerating a
                // trailing CR costs nothing and makes `socat` friendlier.
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    async fn reader_over(bytes: &[u8], prefix: &[u8]) -> LineReader {
        let (mut client, server) = UnixStream::pair().unwrap();
        let bytes = bytes.to_vec();
        // Written from a task: a payload larger than the socket buffer would
        // otherwise deadlock against a reader that has not started yet.
        tokio::spawn(async move {
            let _ = client.write_all(&bytes).await;
            let _ = client.shutdown().await;
        });
        LineReader::new(server.into_split().0, prefix)
    }

    #[tokio::test]
    async fn lines_are_split_and_the_prefix_is_restored() {
        let mut reader = reader_over(b"\"a\":1}\n{\"b\":2}\n", b"{").await;
        assert_eq!(reader.next_line().await.unwrap().unwrap(), br#"{"a":1}"#);
        assert_eq!(reader.next_line().await.unwrap().unwrap(), br#"{"b":2}"#);
        assert!(reader.next_line().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_trailing_carriage_return_is_dropped() {
        let mut reader = reader_over(b"{\"a\":1}\r\n", b"").await;
        assert_eq!(reader.next_line().await.unwrap().unwrap(), br#"{"a":1}"#);
    }

    #[tokio::test]
    async fn an_unterminated_final_line_is_still_delivered() {
        let mut reader = reader_over(b"{\"a\":1}", b"").await;
        assert_eq!(reader.next_line().await.unwrap().unwrap(), br#"{"a":1}"#);
        assert!(reader.next_line().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_oversized_line_is_refused() {
        let huge = vec![b'x'; MAX_CONTROL_LINE + 16];
        let mut reader = reader_over(&huge, b"").await;
        assert!(matches!(reader.next_line().await, Err(LineError::TooLong)));
    }
}
