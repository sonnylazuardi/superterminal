//! `superterminald`, the Superterminal daemon — control plane.
//!
//! The daemon owns the Workspace (Sessions → Tabs → Surfaces), the Unix socket
//! both planes share, `workspace.json`, and its own lifecycle. This crate is
//! the async shell described in `docs/plan/03-server.md`; the terminal engine
//! itself lives in `st-core` and the wire types in `st-proto`.
//!
//! # Layout
//!
//! | Module | Spec | What it owns |
//! |---|---|---|
//! | [`lifecycle`] | §2 | lock file, socket, signals, logging, idle exit, graceful shutdown |
//! | [`workspace`] | §3 | the domain model, the single-writer actor, the [`workspace::SurfaceSpawner`] seam |
//! | [`persist`] | §8 | `workspace.json` v1: debounce, atomic write, corrupt-file recovery |
//! | [`control`] | §7 | accept loop, plane sniffing, NDJSON connections |
//! | [`metrics`] | §11 | counters, reported by `server.status` |
//!
//! # The two planes
//!
//! Every connection is CONTROL or DATA for its whole life, decided by its
//! first byte (grilling Q37). [`control`] implements CONTROL end to end and
//! sniffs DATA connections, handing them to the [`control::DataAcceptor`] that
//! [`data`] installs; a build with no acceptor logs and closes them. The seams
//! between the two halves are:
//!
//! * [`workspace::SurfaceSpawner`] — start and signal Surface processes;
//! * [`workspace::WorkspaceHandle::surface_event`] — report title, cwd, resize,
//!   foreground child, input and exit upward;
//! * [`workspace::WorkspaceHandle::set_view_state`] — apply a data-plane
//!   `SetViewState` through the *same* actor command as control-plane
//!   `view.set` (grilling Q43/Q49);
//! * [`control::DataAcceptor`] — take over a sniffed DATA connection.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod control;
pub mod data;
pub mod lifecycle;
pub mod metrics;
pub mod persist;
pub mod supervisor;
pub mod workspace;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::control::DataAcceptor;
use crate::lifecycle::Shutdown;
use crate::metrics::{Metrics, Uptime};
use crate::workspace::{ClientId, WorkspaceHandle};

/// The `build_id` this daemon reports in `HelloAck` and `server.status`.
///
/// `SUPERTERMINAL_BUILD_ID` is baked in by the build when available; otherwise
/// the crate version stands in. Informational only (`02-protocol.md` §2,
/// rule 4).
#[must_use]
pub fn build_id() -> String {
    option_env!("SUPERTERMINAL_BUILD_ID").map_or_else(
        || format!("superterminald {}", env!("CARGO_PKG_VERSION")),
        Into::into,
    )
}

/// Everything a connection task needs, shared behind an `Arc`.
pub struct ServerContext {
    /// The single writer of the Workspace.
    pub workspace: WorkspaceHandle,
    /// Counters (§11).
    pub metrics: Arc<Metrics>,
    /// This build's id.
    pub build_id: String,
    /// When the daemon started.
    pub uptime: Uptime,
    /// The uid a peer must have (§10). `None` disables the check, which only
    /// happens on platforms where the credentials cannot be read.
    pub allowed_uid: Option<u32>,
    /// Installed by `src/data/mod.rs`; `None` in a control-only build.
    pub data: Option<DataAcceptor>,
    /// The graceful-shutdown trigger.
    pub shutdown: Shutdown,
    next_client_id: AtomicU64,
    last_activity: std::sync::Mutex<Instant>,
}

impl std::fmt::Debug for ServerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerContext")
            .field("build_id", &self.build_id)
            .field("allowed_uid", &self.allowed_uid)
            .field("data_plane", &self.data.is_some())
            .finish_non_exhaustive()
    }
}

impl ServerContext {
    /// Builds a context. Connection ids start at 1.
    #[must_use]
    pub fn new(
        workspace: WorkspaceHandle,
        metrics: Arc<Metrics>,
        build_id: String,
        uptime: Uptime,
        allowed_uid: Option<u32>,
        shutdown: Shutdown,
    ) -> Self {
        Self {
            workspace,
            metrics,
            build_id,
            uptime,
            allowed_uid,
            data: None,
            shutdown,
            next_client_id: AtomicU64::new(1),
            last_activity: std::sync::Mutex::new(Instant::now()),
        }
    }

    /// Allocates the next per-connection id and counts the connection as
    /// activity for the idle timer (§2).
    pub fn next_client_id(&self) -> ClientId {
        self.mark_active();
        ClientId(self.next_client_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Resets the idle countdown.
    pub fn mark_active(&self) {
        if let Ok(mut at) = self.last_activity.lock() {
            *at = Instant::now();
        }
    }

    /// How long the daemon has had nothing to do.
    #[must_use]
    pub fn idle_for(&self) -> std::time::Duration {
        self.last_activity
            .lock()
            .map_or_else(|_| std::time::Duration::ZERO, |at| at.elapsed())
    }

    /// Connections currently open on both planes.
    #[must_use]
    pub fn open_connections(&self) -> u64 {
        self.metrics.control_clients.get() + self.metrics.data_clients.get()
    }
}
