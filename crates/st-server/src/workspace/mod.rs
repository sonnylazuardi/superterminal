//! The Workspace: the domain model, the single-writer actor and the Surface
//! spawning seam (`docs/plan/03-server.md` §3).
//!
//! * [`model`] — pure `Workspace`/`Session`/`Tab`/`Surface` data and the
//!   grilling-Q21 shape rules. No async, no I/O.
//! * [`actor`] — the one task that owns a [`model::Workspace`], the
//!   [`actor::WorkspaceCommand`] channel and the `ev.workspace` broadcast.
//! * [`spawn`] — [`SurfaceSpawner`], the trait the data plane implements so
//!   the control plane never sees a PTY.

pub mod actor;
pub mod model;
pub mod spawn;

pub use actor::{
    reseed, ActorConfig, ClientId, EventEnvelope, SpawnDefaults, Stats, SurfaceEvent,
    WorkspaceActor, WorkspaceCommand, WorkspaceHandle,
};
pub use model::{Session, Surface, SurfaceStatus, Tab, Workspace, DEFAULT_SESSION_NAME};
pub use spawn::{NullSpawner, SpawnError, SpawnSpec, SpawnedSurface, SurfaceSpawner};
