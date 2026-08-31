//! Atomic counters, sampled into the log and reported by `server.status`
//! (`docs/plan/03-server.md` §11).
//!
//! Every field is an [`AtomicU64`] behind an `Arc<Metrics>` so any task may
//! bump it without coordination. Nothing here is on a hot path that needs
//! anything stronger than [`Ordering::Relaxed`]: the counters are diagnostics,
//! not synchronisation.
//!
//! The data-plane agent owns the PTY-side counters ([`Metrics::pty_bytes_in`],
//! [`Metrics::pty_bytes_out`], [`Metrics::frames_out`]); they are declared here
//! so `server.status` has a stable shape from the first release.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// One counter. Thin wrapper so the call sites read as prose.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Adds one.
    #[inline]
    pub fn inc(&self) {
        self.add(1);
    }

    /// Adds `n`.
    #[inline]
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    /// Subtracts one, saturating at zero (used for the "currently connected"
    /// gauges).
    #[inline]
    pub fn dec(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
    }

    /// Reads the current value.
    #[inline]
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Every counter the daemon keeps.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Connections accepted, both planes, since startup.
    pub connections_accepted: Counter,
    /// Connections turned away (bad magic, bad uid, `Reject`, over the limit).
    pub connections_refused: Counter,
    /// Control connections currently open.
    pub control_clients: Counter,
    /// Data connections currently open.
    pub data_clients: Counter,
    /// Control requests received.
    pub requests: Counter,
    /// Control requests answered with an `err` envelope.
    pub request_errors: Counter,
    /// Bytes read from control connections.
    pub control_bytes_in: Counter,
    /// Bytes written to control connections.
    pub control_bytes_out: Counter,
    /// Workspace revisions produced, i.e. successful mutations.
    pub revisions: Counter,
    /// Surfaces spawned through the [`SurfaceSpawner`](crate::workspace::SurfaceSpawner).
    pub surfaces_spawned: Counter,
    /// Surfaces whose process ended.
    pub surfaces_exited: Counter,
    /// `workspace.json` writes that reached the filesystem.
    pub persist_writes: Counter,
    /// `workspace.json` writes that failed.
    pub persist_errors: Counter,
    /// PTY bytes read (data plane; owned by `src/supervisor.rs`).
    pub pty_bytes_in: Counter,
    /// PTY bytes written (data plane).
    pub pty_bytes_out: Counter,
    /// Data-plane frames written (data plane).
    pub frames_out: Counter,
    /// `Delta` messages sent (data plane).
    pub deltas_sent: Counter,
    /// `Snapshot` messages sent (data plane).
    pub snapshots_sent: Counter,
    /// Damage events observed (data plane); `damage_events / deltas_sent` is
    /// the coalesce ratio §11 asks for.
    pub damage_events: Counter,
}

impl Metrics {
    /// A fresh, all-zero set of counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The counters as the JSON object embedded in the `server.status` result.
    ///
    /// `02-protocol.md` §3.3 fixes the named fields of `ServerStatus`; adding
    /// an object beside them is a minor, backwards-compatible change (§10),
    /// which is why the metrics live under one `"metrics"` key rather than
    /// being spliced in at the top level. `st status` reads exactly this
    /// object (`crates/st-cli/src/cmd/status.rs`), so the key names are the
    /// ones from `03-server.md` §11 and must not drift.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let deltas = self.deltas_sent.get();
        let damage = self.damage_events.get();
        serde_json::json!({
            "pty_bytes_in": self.pty_bytes_in.get(),
            "pty_bytes_out": self.pty_bytes_out.get(),
            "frames_out": self.frames_out.get(),
            "deltas_sent": deltas,
            "snapshots_sent": self.snapshots_sent.get(),
            "damage_events": damage,
            "coalesce_ratio": if deltas == 0 { 0.0 } else { damage as f64 / deltas as f64 },
            "connections_control": self.control_clients.get(),
            "connections_data": self.data_clients.get(),
            "connections_accepted": self.connections_accepted.get(),
            "connections_refused": self.connections_refused.get(),
            "requests_handled": self.requests.get(),
            "request_errors": self.request_errors.get(),
            "control_bytes_in": self.control_bytes_in.get(),
            "control_bytes_out": self.control_bytes_out.get(),
            "revisions": self.revisions.get(),
            "surfaces_spawned": self.surfaces_spawned.get(),
            "surfaces_exited": self.surfaces_exited.get(),
            "persist_writes": self.persist_writes.get(),
            "persist_errors": self.persist_errors.get(),
        })
    }
}

/// Wall-clock uptime source, so `server.status` and the idle timer agree.
#[derive(Debug, Clone, Copy)]
pub struct Uptime(Instant);

impl Uptime {
    /// Starts the clock now.
    #[must_use]
    pub fn start() -> Self {
        Self(Instant::now())
    }

    /// Time since [`Uptime::start`].
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    /// Whole seconds since [`Uptime::start`], as `server.status` reports them.
    #[must_use]
    pub fn secs(&self) -> u64 {
        self.0.elapsed().as_secs()
    }
}

impl Default for Uptime {
    fn default() -> Self {
        Self::start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_add_and_saturate() {
        let m = Metrics::new();
        m.requests.inc();
        m.requests.add(4);
        assert_eq!(m.requests.get(), 5);
        m.control_clients.dec();
        assert_eq!(m.control_clients.get(), 0, "gauges never wrap");
    }

    #[test]
    fn json_uses_the_key_names_st_cli_reads() {
        let m = Metrics::new();
        m.revisions.add(3);
        m.requests.add(9);
        m.control_clients.inc();
        let json = m.to_json();
        assert_eq!(json["revisions"], 3);
        assert_eq!(json["requests_handled"], 9);
        assert_eq!(json["connections_control"], 1);
        // `st status` looks these up by name; missing keys silently degrade
        // its output to "not reported by this server build".
        for key in [
            "pty_bytes_in",
            "pty_bytes_out",
            "frames_out",
            "deltas_sent",
            "snapshots_sent",
            "coalesce_ratio",
            "connections_control",
            "connections_data",
            "requests_handled",
            "revisions",
            "surfaces_spawned",
            "surfaces_exited",
        ] {
            assert!(json.get(key).is_some(), "missing metric {key}");
        }
    }

    #[test]
    fn the_coalesce_ratio_never_divides_by_zero() {
        let m = Metrics::new();
        assert_eq!(m.to_json()["coalesce_ratio"], 0.0);
        m.deltas_sent.add(2);
        m.damage_events.add(9);
        assert_eq!(m.to_json()["coalesce_ratio"], 4.5);
    }
}
