//! One Data Plane connection per socket, shared by every `<terminal-grid>`.
//!
//! 04 §5 puts a *single* socket in the client and `st-client-core` owns the
//! thread behind it, so the element must not open one per instance: with
//! grilling Q44 only the visible Tab is mounted, but a Session switch mounts
//! the next one before the last unmounts, and two sockets would mean two
//! `Hello` handshakes and two sets of attachments for the same client.
//!
//! This module is therefore a small process-global pool keyed on the socket
//! path. It also fans the single [`WakeFn`] out to every mounted element and
//! files `DataPlaneEvent`s per Surface, because `take_events` drains the whole
//! connection and one element must not eat another's `bell`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use st_client_core::{
    DataPlaneConnection, DataPlaneEvent, DataPlaneHandle, DataPlaneOptions, WakeFn,
};
use st_proto::SurfaceId;

use crate::wake::Waker;

/// A live connection plus the bookkeeping the elements share.
pub struct SharedDataPlane {
    path: String,
    connection: DataPlaneConnection,
    wakers: Arc<Mutex<Vec<(u64, Waker)>>>,
    inbox: Mutex<HashMap<Option<SurfaceId>, Vec<DataPlaneEvent>>>,
}

impl std::fmt::Debug for SharedDataPlane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedDataPlane")
            .field("path", &self.path)
            .field("connected", &self.connection.is_connected())
            .finish()
    }
}

impl SharedDataPlane {
    /// The socket this connection is on.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// A cloneable handle for attach/input/resize/history.
    #[must_use]
    pub fn handle(&self) -> DataPlaneHandle {
        self.connection.handle()
    }

    /// Registers an element's waker. Replaces any waker already under `id`, so
    /// a re-render after a window rebuild does not leave a dead sender behind.
    pub fn register(&self, id: u64, waker: Waker) {
        let mut wakers = self.wakers.lock().unwrap_or_else(|e| e.into_inner());
        wakers.retain(|(existing, _)| *existing != id);
        wakers.push((id, waker));
    }

    /// Forgets an element's waker on `destroy()`.
    pub fn unregister(&self, id: u64) {
        let mut wakers = self.wakers.lock().unwrap_or_else(|e| e.into_inner());
        wakers.retain(|(existing, _)| *existing != id);
    }

    /// Moves everything the connection queued into the per-Surface inboxes.
    ///
    /// Called once per frame by whichever element renders first; the rest read
    /// their own inbox. Events with no Surface (connect, disconnect, reject)
    /// are filed under `None` and delivered to every element.
    pub fn pump(&self) {
        let events = self.connection.handle().take_events();
        if events.is_empty() {
            return;
        }
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        for event in events {
            let key = surface_of(&event);
            let queue = inbox.entry(key).or_default();
            // A disconnected client can queue events faster than an unmounted
            // element reads them; the newest are the ones that matter.
            if queue.len() >= 256 {
                queue.remove(0);
            }
            queue.push(event);
        }
    }

    /// Takes this Surface's events, plus the connection-wide ones.
    ///
    /// Connection-wide events are *copied*, not moved, so every mounted
    /// element sees the disconnect; they are dropped when the last element
    /// with a Surface reads them or when 256 pile up.
    #[must_use]
    pub fn take_events_for(&self, surface: Option<SurfaceId>) -> Vec<DataPlaneEvent> {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = inbox.remove(&None).unwrap_or_default();
        if let Some(surface) = surface {
            out.extend(inbox.remove(&Some(surface)).unwrap_or_default());
        }
        out
    }

    /// `true` once the socket is up.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connection.is_connected()
    }
}

/// Which Surface an event belongs to, if any.
fn surface_of(event: &DataPlaneEvent) -> Option<SurfaceId> {
    match event {
        DataPlaneEvent::Bell(surface_id) => Some(*surface_id),
        DataPlaneEvent::Exited { surface_id, .. } | DataPlaneEvent::Detached { surface_id, .. } => {
            Some(*surface_id)
        }
        DataPlaneEvent::Gap(gap) => Some(gap.surface_id),
        DataPlaneEvent::Error(error) => error.surface_id,
        DataPlaneEvent::Connected { .. }
        | DataPlaneEvent::Rejected(_)
        | DataPlaneEvent::Disconnected { .. } => None,
    }
}

static POOL: Mutex<Option<HashMap<String, Weak<SharedDataPlane>>>> = Mutex::new(None);
/// Connections opened by `stConnectDataPlane` before any element mounted.
/// Without a strong reference the pool's `Weak` would be dead by the time the
/// first `<terminal-grid>` asks for it, and the pre-warm would be a no-op.
static PINNED: Mutex<Option<Vec<Arc<SharedDataPlane>>>> = Mutex::new(None);

/// Opens the socket, or returns the connection already open on it.
///
/// The pool holds `Weak`s: when the last `<terminal-grid>` on a socket is
/// destroyed the `Arc` drops, `DataPlaneConnection::drop` shuts the thread
/// down, and the next mount reconnects.
pub fn open(path: &str, build_id: &str) -> Result<Arc<SharedDataPlane>, String> {
    let mut guard = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let pool = guard.get_or_insert_with(HashMap::new);
    pool.retain(|_, weak| weak.strong_count() > 0);
    if let Some(existing) = pool.get(path).and_then(Weak::upgrade) {
        return Ok(existing);
    }

    let wakers: Arc<Mutex<Vec<(u64, Waker)>>> = Arc::new(Mutex::new(Vec::new()));
    let wake: WakeFn = {
        let wakers = Arc::clone(&wakers);
        Box::new(move || {
            for (_, waker) in wakers.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                waker.wake();
            }
        })
    };

    let options = DataPlaneOptions {
        build_id: build_id.to_string(),
        ..DataPlaneOptions::default()
    };
    let connection = DataPlaneConnection::connect(path, options, wake)
        .map_err(|error| format!("{path}: {error}"))?;

    let shared = Arc::new(SharedDataPlane {
        path: path.to_string(),
        connection,
        wakers,
        inbox: Mutex::new(HashMap::new()),
    });
    pool.insert(path.to_string(), Arc::downgrade(&shared));
    Ok(shared)
}

/// Keeps a connection alive with no element mounted on it (`stConnectDataPlane`).
pub fn pin(plane: &Arc<SharedDataPlane>) {
    let mut guard = PINNED.lock().unwrap_or_else(|e| e.into_inner());
    let pinned = guard.get_or_insert_with(Vec::new);
    if !pinned.iter().any(|existing| Arc::ptr_eq(existing, plane)) {
        pinned.push(Arc::clone(plane));
    }
}

/// Sockets with a live connection, for `stListGrids` and tests.
#[must_use]
pub fn open_paths() -> Vec<String> {
    let mut guard = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let pool = guard.get_or_insert_with(HashMap::new);
    pool.retain(|_, weak| weak.strong_count() > 0);
    let mut paths: Vec<String> = pool.keys().cloned().collect();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_client_core::replica::Gap;
    use st_proto::{DataError, DetachReason, ExitStatus, Seq};

    #[test]
    fn events_are_routed_to_the_surface_they_belong_to() {
        assert_eq!(
            surface_of(&DataPlaneEvent::Bell(SurfaceId(4))),
            Some(SurfaceId(4))
        );
        assert_eq!(
            surface_of(&DataPlaneEvent::Exited {
                surface_id: SurfaceId(9),
                status: ExitStatus {
                    code: Some(0),
                    signal: None
                },
            }),
            Some(SurfaceId(9))
        );
        assert_eq!(
            surface_of(&DataPlaneEvent::Detached {
                surface_id: SurfaceId(2),
                reason: DetachReason::SurfaceDestroyed,
            }),
            Some(SurfaceId(2))
        );
        assert_eq!(
            surface_of(&DataPlaneEvent::Gap(Gap {
                surface_id: SurfaceId(3),
                have: Seq::ZERO,
                since: Seq::ZERO,
                got: Seq::FIRST,
            })),
            Some(SurfaceId(3))
        );
        assert_eq!(
            surface_of(&DataPlaneEvent::Error(DataError {
                surface_id: Some(SurfaceId(1)),
                code: 1,
                message: String::new(),
            })),
            Some(SurfaceId(1))
        );
        assert_eq!(
            surface_of(&DataPlaneEvent::Disconnected {
                reason: String::new()
            }),
            None
        );
        assert_eq!(
            surface_of(&DataPlaneEvent::Connected {
                proto_version: st_proto::PROTO_VERSION,
                server_build_id: String::new(),
                server_pid: 1,
            }),
            None
        );
    }

    #[test]
    fn opening_a_socket_that_is_not_there_is_an_error_not_a_panic() {
        let error = open("/nonexistent/superterminal/server.sock", "test").unwrap_err();
        assert!(error.contains("/nonexistent/"), "{error}");
        assert!(open_paths().is_empty());
    }
}
