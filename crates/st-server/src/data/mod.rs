//! The DATA plane — `02-protocol.md` §1.3, §2, §4, §6–§9.
//!
//! Two halves:
//!
//! * [`conn`] — one task per connection: handshake, then a dispatch loop over
//!   `Attach` / `Detach` / `Input` / `Resize` / `FetchHistory` / `Ack` /
//!   `SetViewState`.
//! * [`pump`] — one task per daemon: the 120 Hz emit loop that turns each
//!   Surface's `Publisher` output into `Snapshot` / `Delta` / `Bell` /
//!   `SurfaceExited` frames on the right connections.
//!
//! # Wiring
//!
//! Building the daemon's [`ServerContext`](crate::ServerContext) is the only
//! place that has to mention this module:
//!
//! ```ignore
//! let supervisor = Arc::new(SurfaceSupervisor::new(cfg, notifier).with_metrics(metrics.clone()));
//! let mut ctx = ServerContext::new(workspace, metrics, build_id, uptime, uid, shutdown);
//! ctx.data = Some(st_server::data::acceptor(Arc::clone(&supervisor)));
//! ```
//!
//! The same `supervisor` is the [`SurfaceSpawner`](crate::workspace::SurfaceSpawner)
//! handed to [`ActorConfig`](crate::workspace::ActorConfig), so the control
//! plane creates real PTYs and the data plane serves them. The 120 Hz pump
//! starts itself on the first accepted DATA connection.

pub mod conn;
pub mod pump;

use std::sync::Arc;

pub use conn::{accept, accept_with_magic, DataCtx};

use crate::control::{BoxFuture, DataAcceptor};
use crate::supervisor::SurfaceSupervisor;

/// Builds the [`DataAcceptor`] the control plane's accept loop calls once it
/// has sniffed and consumed the DATA magic (grilling Q37).
///
/// It owns the connection from the `Hello` frame onward and decrements
/// `metrics.data_clients` when the connection closes, as
/// [`DataAcceptor`]'s contract requires.
#[must_use]
pub fn acceptor(supervisor: Arc<SurfaceSupervisor>) -> DataAcceptor {
    Arc::new(move |stream, server, id| -> BoxFuture {
        let supervisor = Arc::clone(&supervisor);
        Box::pin(async move {
            let revision = server
                .workspace
                .stats()
                .await
                .map_or(0, |stats| stats.revision);
            let ctx = DataCtx::new(supervisor)
                .with_build_id(server.build_id.clone())
                .with_workspace_revision(revision);
            accept(stream, ctx, id).await;
            server.metrics.data_clients.dec();
            server.mark_active();
        })
    })
}
