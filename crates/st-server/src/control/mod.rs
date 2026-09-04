//! The accept loop and the plane sniffer — `docs/plan/03-server.md` §7,
//! `02-protocol.md` §1.2.
//!
//! One task per connection. Before anything is read, the peer's uid is checked
//! against our own (`SO_PEERCRED`, §10: defence in depth over the `0600`
//! socket). Then the first byte decides the plane for the connection's whole
//! life:
//!
//! | First byte | Plane | Handled by |
//! |---|---|---|
//! | `{` | CONTROL | [`conn::serve_control`] — NDJSON |
//! | `0xFF` + `"STD"` | DATA | the [`DataAcceptor`] the data plane installs |
//! | anything else | — | closed immediately (`02-protocol.md` §1.2) |
//!
//! The hand-off is one field: [`crate::ServerContext::data`] is an
//! `Option<DataAcceptor>`, set to `Some(crate::data::acceptor(..))` when the
//! daemon is built. A build that leaves it `None` — a control-only test
//! harness, say — logs each DATA connection and closes it, and nothing in this
//! module changes either way.

pub mod conn;
pub mod handlers;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use st_proto::frame::{ConnectionKind, DATA_MAGIC, HANDSHAKE_TIMEOUT_SECS};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

use crate::workspace::ClientId;
use crate::ServerContext;

/// Either side of the Windows/WSL boundary: a Unix socket at home, loopback
/// TCP across it (`superterminald --tcp 127.0.0.1:PORT`). The first-byte plane
/// sniffing works on both because it only ever reads bytes.
#[derive(Debug)]
pub enum Conn {
    /// The at-home transport.
    Unix(UnixStream),
    /// The Windows-client/WSL-server transport.
    Tcp(TcpStream),
}

impl From<UnixStream> for Conn {
    fn from(stream: UnixStream) -> Self {
        Conn::Unix(stream)
    }
}

impl From<TcpStream> for Conn {
    fn from(stream: TcpStream) -> Self {
        Conn::Tcp(stream)
    }
}

impl AsyncRead for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Unix(stream) => Pin::new(stream).poll_read(cx, buf),
            Conn::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match self.get_mut() {
            Conn::Unix(stream) => Pin::new(stream).poll_write(cx, buf),
            Conn::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Conn::Unix(stream) => Pin::new(stream).poll_flush(cx),
            Conn::Tcp(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match self.get_mut() {
            Conn::Unix(stream) => Pin::new(stream).poll_shutdown(cx),
            Conn::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// A boxed future, as returned by a [`DataAcceptor`].
pub type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// How the data plane plugs into the accept loop.
///
/// The stream handed over has already had its 4-byte [`DATA_MAGIC`] consumed
/// and its peer uid checked; the next bytes on it are the `Hello` frame
/// (`msg_type` `0x0001`). The acceptor owns the connection from that point,
/// including decrementing [`crate::metrics::Metrics::data_clients`] when it
/// closes.
pub type DataAcceptor = Arc<dyn Fn(Conn, Arc<ServerContext>, ClientId) -> BoxFuture + Send + Sync>;

/// Runs the accept loop until `shutdown` fires.
///
/// Each accepted connection becomes its own task, so a slow handshake never
/// blocks the next client.
pub async fn accept_loop(listener: UnixListener, ctx: Arc<ServerContext>) {
    let mut stop = ctx.shutdown.subscribe();
    loop {
        tokio::select! {
            biased;
            _ = stop.wait() => {
                tracing::debug!("accept loop stopping");
                return;
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _addr)) => {
                    let id = ctx.next_client_id();
                    let ctx = Arc::clone(&ctx);
                    tokio::spawn(async move {
                        serve(Conn::Unix(stream), ctx, id).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    // A transient accept error must not spin the loop.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
        }
    }
}

/// The TCP twin of [`accept_loop`], for `--tcp` (the Windows-client/WSL-server
/// transport). Same sniffing, same hand-off; only the peer check differs (see
/// [`check_peer`]).
pub async fn accept_loop_tcp(listener: TcpListener, ctx: Arc<ServerContext>) {
    let mut stop = ctx.shutdown.subscribe();
    loop {
        tokio::select! {
            biased;
            _ = stop.wait() => {
                tracing::debug!("TCP accept loop stopping");
                return;
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    tracing::debug!(%addr, "TCP connection accepted");
                    let id = ctx.next_client_id();
                    let ctx = Arc::clone(&ctx);
                    tokio::spawn(async move {
                        serve(Conn::Tcp(stream), ctx, id).await;
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "TCP accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
        }
    }
}

/// Serves one accepted connection: uid check, plane sniff, hand-off.
pub async fn serve(stream: Conn, ctx: Arc<ServerContext>, id: ClientId) {
    ctx.metrics.connections_accepted.inc();

    if let Err(reason) = check_peer(&stream, &ctx) {
        ctx.metrics.connections_refused.inc();
        tracing::warn!(client = id.0, reason, "refusing connection");
        return;
    }

    let deadline = Duration::from_secs(HANDSHAKE_TIMEOUT_SECS);
    let sniffed = match tokio::time::timeout(deadline, sniff(stream)).await {
        Ok(Ok(sniffed)) => sniffed,
        Ok(Err(e)) => {
            ctx.metrics.connections_refused.inc();
            tracing::debug!(client = id.0, error = %e, "connection closed before it chose a plane");
            return;
        }
        Err(_) => {
            ctx.metrics.connections_refused.inc();
            tracing::debug!(client = id.0, "handshake timed out before the first byte");
            return;
        }
    };

    match sniffed {
        Sniffed::Control { stream, first_byte } => {
            let span = tracing::info_span!("conn", id = id.0, plane = "control");
            let _guard = span.enter();
            drop(_guard);
            conn::serve_control(stream, ctx, id, first_byte).await;
        }
        Sniffed::Data { stream } => match ctx.data.clone() {
            Some(acceptor) => {
                ctx.metrics.data_clients.inc();
                tracing::debug!(client = id.0, "data connection accepted");
                acceptor(stream, ctx, id).await;
            }
            None => {
                ctx.metrics.connections_refused.inc();
                tracing::warn!(
                    client = id.0,
                    "data plane not yet implemented: this build of superterminald serves the \
                     control plane only, so the connection is closed (see src/data/mod.rs)"
                );
            }
        },
        Sniffed::Unknown { first_byte } => {
            ctx.metrics.connections_refused.inc();
            tracing::warn!(
                client = id.0,
                first_byte = format!("0x{first_byte:02X}"),
                "connection opened with neither `{{` nor the DATA magic; closing"
            );
        }
    }
}

/// The outcome of reading a connection's first bytes.
enum Sniffed {
    /// A CONTROL connection; `first_byte` is the `{` that must be put back.
    Control {
        /// The stream, positioned after the sniffed byte.
        stream: Conn,
        /// The byte already consumed.
        first_byte: u8,
    },
    /// A DATA connection whose 4-byte magic has been consumed and verified.
    Data {
        /// The stream, positioned after the magic.
        stream: Conn,
    },
    /// Neither.
    Unknown {
        /// What we saw instead.
        first_byte: u8,
    },
}

async fn sniff(mut stream: Conn) -> std::io::Result<Sniffed> {
    let mut first = [0u8; 1];
    stream.read_exact(&mut first).await?;

    match st_proto::detect_connection_kind(first[0]) {
        Some(ConnectionKind::Control) => Ok(Sniffed::Control {
            stream,
            first_byte: first[0],
        }),
        Some(ConnectionKind::Data) => {
            let mut rest = [0u8; 3];
            stream.read_exact(&mut rest).await?;
            if rest != DATA_MAGIC[1..] {
                return Ok(Sniffed::Unknown {
                    first_byte: first[0],
                });
            }
            Ok(Sniffed::Data { stream })
        }
        None => Ok(Sniffed::Unknown {
            first_byte: first[0],
        }),
    }
}

/// `SO_PEERCRED` uid check (`03-server.md` §10).
///
/// On Linux and macOS `tokio` exposes the peer's uid directly. Elsewhere the
/// check is skipped and the `0700` directory plus the `0600` socket carry the
/// whole burden; the daemon only ever runs on those two platforms in v1.
///
/// TCP connections (`--tcp`) skip the check: there is no peer credential on a
/// TCP socket. The listener is meant to bind loopback only (the Windows client
/// reaches it through WSL's shared localhost), so the exposure is one machine.
/// Binding a non-loopback address is refused at start-up (see
/// [`crate::lifecycle`]).
fn check_peer(stream: &Conn, ctx: &ServerContext) -> Result<(), &'static str> {
    if matches!(stream, Conn::Tcp(_)) {
        tracing::debug!("TCP peer accepted without a uid check; the listener is loopback-only");
        return Ok(());
    }
    let Some(expected) = ctx.allowed_uid else {
        return Ok(());
    };

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let Conn::Unix(stream) = stream else {
            return Ok(());
        };
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == expected => Ok(()),
            Ok(_) => Err("peer uid does not match the daemon's"),
            Err(_) => Err("cannot read the peer's credentials"),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (stream, expected);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_first_byte_chooses_the_plane_over_tcp_too() {
        for (bytes, expect_control, expect_data) in [
            (b"{\"t\":\"hello\"}".to_vec(), true, false),
            ([DATA_MAGIC.to_vec(), vec![0, 1, 2]].concat(), false, true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let payload = bytes.clone();
            let writer = tokio::spawn(async move {
                let mut stream = TcpStream::connect(addr).await.unwrap();
                tokio::io::AsyncWriteExt::write_all(&mut stream, &payload)
                    .await
                    .unwrap();
            });
            let (server, _) = listener.accept().await.unwrap();
            let sniffed = sniff(Conn::from(server)).await.unwrap();
            assert_eq!(
                matches!(sniffed, Sniffed::Control { .. }),
                expect_control,
                "control detection for {bytes:?}"
            );
            assert_eq!(
                matches!(sniffed, Sniffed::Data { .. }),
                expect_data,
                "data detection for {bytes:?}"
            );
            writer.await.unwrap();
        }
    }

    #[tokio::test]
    async fn the_first_byte_chooses_the_plane() {
        for (bytes, expect_control, expect_data) in [
            (b"{\"t\":\"hello\"}".to_vec(), true, false),
            ([DATA_MAGIC.to_vec(), vec![0, 1, 2]].concat(), false, true),
            (b"GET / HTTP/1.1".to_vec(), false, false),
            (vec![0xFF, b'X', b'X', b'X'], false, false),
        ] {
            let (mut client, server) = UnixStream::pair().unwrap();
            let server = Conn::from(server);
            tokio::io::AsyncWriteExt::write_all(&mut client, &bytes)
                .await
                .unwrap();
            let sniffed = sniff(server).await.unwrap();
            assert_eq!(
                matches!(sniffed, Sniffed::Control { .. }),
                expect_control,
                "control detection for {bytes:?}"
            );
            assert_eq!(
                matches!(sniffed, Sniffed::Data { .. }),
                expect_data,
                "data detection for {bytes:?}"
            );
        }
    }
}
