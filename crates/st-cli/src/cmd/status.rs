//! `st status` — `docs/plan/03-server.md` §11.
//!
//! Sends `server.status` and `workspace.get` on the CONTROL plane and prints
//! what came back. The Session and Tab counts are *not* in `ServerStatus`
//! (§3.3 only gives `surfaces`), so they are derived from the workspace
//! document.
//!
//! **Metrics.** `03-server.md` §11 says the server keeps `pty_bytes_in`,
//! `pty_bytes_out`, `deltas_sent` and friends, but `02-protocol.md` §3.3 does
//! not put them in the `ServerStatus` result. Rather than guess a shape, this
//! command reads the raw `result` object and prints any of those counters it
//! finds under an optional `metrics` key (or at the top level), and prints
//! nothing about them otherwise — the "if available" in the M1 brief.

use std::io::Write;

use serde_json::{json, Value};
use st_proto::control::{Req, ServerStatus, WorkspaceSnapshot};

use crate::cmd::{format_bytes, format_uptime};
use crate::control::ControlClient;
use crate::exit::{CliError, Result};
use crate::transport::Connector;

/// Counters `03-server.md` §11 defines, when the server exposes them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// Bytes read from PTYs.
    pub pty_bytes_in: Option<u64>,
    /// Bytes written to PTYs.
    pub pty_bytes_out: Option<u64>,
    /// Frames written to data connections.
    pub frames_out: Option<u64>,
    /// Deltas sent.
    pub deltas_sent: Option<u64>,
    /// Snapshots sent.
    pub snapshots_sent: Option<u64>,
}

impl Metrics {
    /// Picks the counters out of a `server.status` result, looking first in a
    /// `metrics` sub-object and then at the top level.
    #[must_use]
    pub fn from_status_value(value: &Value) -> Self {
        let scope = value.get("metrics").unwrap_or(value);
        let get = |key: &str| scope.get(key).and_then(Value::as_u64);
        Self {
            pty_bytes_in: get("pty_bytes_in"),
            pty_bytes_out: get("pty_bytes_out"),
            frames_out: get("frames_out"),
            deltas_sent: get("deltas_sent"),
            snapshots_sent: get("snapshots_sent"),
        }
    }

    /// `true` when the server reported nothing at all.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// Everything `st status` gathers, so the renderer can be tested on its own.
#[derive(Debug)]
pub struct StatusReport {
    /// The socket the report came from.
    pub socket: String,
    /// The typed part of the `server.status` result.
    pub status: ServerStatus,
    /// The raw result, kept so unknown fields survive `--json`.
    pub raw: Value,
    /// Optional counters.
    pub metrics: Metrics,
    /// Number of Sessions in the workspace document.
    pub sessions: usize,
    /// Number of Tabs across all Sessions.
    pub tabs: usize,
    /// The workspace revision at the time of the call.
    pub revision: u64,
}

/// Runs the command.
pub fn run(connector: &dyn Connector, json: bool, out: &mut dyn Write) -> Result<()> {
    let mut client = ControlClient::connect(connector)?;
    let raw = client.request_raw(|id| Req::ServerStatus { id })?;
    let status: ServerStatus = serde_json::from_value(raw.clone()).map_err(|err| {
        CliError::protocol(format!("server.status result is not a ServerStatus: {err}"))
    })?;
    let workspace: WorkspaceSnapshot = client.request(|id| Req::WorkspaceGet { id })?;

    let report = StatusReport {
        socket: connector.describe().display().to_string(),
        metrics: Metrics::from_status_value(&raw),
        raw,
        sessions: workspace.workspace.sessions.len(),
        tabs: workspace
            .workspace
            .sessions
            .iter()
            .map(|s| s.tabs.len())
            .sum(),
        revision: workspace.workspace.revision,
        status,
    };

    let text = if json {
        format!("{}\n", render_json(&report))
    } else {
        render_text(&report)
    };
    out.write_all(text.as_bytes())
        .map_err(|e| CliError::failure(format!("cannot write to stdout: {e}")))
}

/// The human-readable report.
#[must_use]
pub fn render_text(report: &StatusReport) -> String {
    let s = &report.status;
    let mut lines = vec![
        ("socket", report.socket.clone()),
        ("build", s.build_id.clone()),
        ("protocol", s.proto_version.clone()),
        ("pid", s.pid.to_string()),
        ("uptime", format_uptime(s.uptime_s)),
        (
            "workspace",
            format!("{} (revision {})", s.workspace_file, report.revision),
        ),
        ("sessions", report.sessions.to_string()),
        ("tabs", report.tabs.to_string()),
        ("surfaces", s.surfaces.to_string()),
        (
            "clients",
            format!("{} control, {} data", s.control_clients, s.data_clients),
        ),
    ];

    let m = report.metrics;
    if let (Some(in_), Some(out)) = (m.pty_bytes_in, m.pty_bytes_out) {
        lines.push((
            "pty in/out",
            format!("{} / {}", format_bytes(in_), format_bytes(out)),
        ));
    }
    if let Some(deltas) = m.deltas_sent {
        lines.push(("deltas", format!("{deltas}{}", per_sec(deltas, s.uptime_s))));
    }
    if let Some(snapshots) = m.snapshots_sent {
        lines.push(("snapshots", snapshots.to_string()));
    }
    if let Some(frames) = m.frames_out {
        lines.push((
            "frames out",
            format!("{frames}{}", per_sec(frames, s.uptime_s)),
        ));
    }
    if m.is_empty() {
        lines.push(("metrics", "not reported by this server build".to_string()));
    }

    let width = lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut text = String::new();
    for (key, value) in lines {
        text.push_str(&format!("{key:width$}  {value}\n"));
    }
    text
}

fn per_sec(count: u64, uptime_s: u64) -> String {
    if uptime_s == 0 {
        return String::new();
    }
    format!(" ({:.1}/s)", count as f64 / uptime_s as f64)
}

/// The `--json` document: the server's raw result plus what `st` derived.
#[must_use]
pub fn render_json(report: &StatusReport) -> String {
    let doc = json!({
        "socket": report.socket,
        "status": report.raw,
        "sessions": report.sessions,
        "tabs": report.tabs,
        "surfaces": report.status.surfaces,
        "revision": report.revision,
    });
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(uptime_s: u64, raw: Value) -> StatusReport {
        let status: ServerStatus = serde_json::from_value(raw.clone()).unwrap();
        StatusReport {
            socket: "/run/st.sock".into(),
            metrics: Metrics::from_status_value(&raw),
            raw,
            sessions: 2,
            tabs: 3,
            revision: 42,
            status: ServerStatus { uptime_s, ..status },
        }
    }

    fn base() -> Value {
        json!({
            "build_id": "abc123-dirty",
            "proto_version": "1.0",
            "pid": 4242,
            "uptime_s": 3_661,
            "surfaces": 3,
            "control_clients": 1,
            "data_clients": 2,
            "workspace_file": "/state/workspace.json",
        })
    }

    #[test]
    fn text_report_without_metrics() {
        let text = render_text(&report(3_661, base()));
        assert_eq!(
            text,
            "socket     /run/st.sock\n\
             build      abc123-dirty\n\
             protocol   1.0\n\
             pid        4242\n\
             uptime     1h 01m 01s\n\
             workspace  /state/workspace.json (revision 42)\n\
             sessions   2\n\
             tabs       3\n\
             surfaces   3\n\
             clients    1 control, 2 data\n\
             metrics    not reported by this server build\n"
        );
    }

    #[test]
    fn metrics_are_picked_up_from_a_nested_object() {
        let mut raw = base();
        raw["metrics"] = json!({
            "pty_bytes_in": 1_572_864u64,
            "pty_bytes_out": 302u64,
            "deltas_sent": 1_200u64,
            "snapshots_sent": 4u64,
            "frames_out": 1_300u64,
        });
        let text = render_text(&report(100, raw));
        assert!(text.contains("pty in/out  1.5 MiB / 302 B\n"));
        assert!(text.contains("deltas      1200 (12.0/s)\n"));
        assert!(text.contains("snapshots   4\n"));
        assert!(text.contains("frames out  1300 (13.0/s)\n"));
        assert!(!text.contains("not reported"));
    }

    #[test]
    fn metrics_are_also_read_from_the_top_level() {
        let mut raw = base();
        raw["deltas_sent"] = json!(10u64);
        let m = Metrics::from_status_value(&raw);
        assert_eq!(m.deltas_sent, Some(10));
        assert_eq!(m.pty_bytes_in, None);
        assert!(!m.is_empty());
    }

    #[test]
    fn a_zero_uptime_does_not_divide_by_zero() {
        let mut raw = base();
        raw["metrics"] = json!({ "deltas_sent": 5u64 });
        let text = render_text(&report(0, raw));
        assert!(text.contains("deltas     5\n"), "{text}");
        assert!(text.contains("uptime     0s\n"));
    }

    #[test]
    fn json_keeps_unknown_server_fields() {
        let mut raw = base();
        raw["future_field"] = json!("kept");
        let doc: Value = serde_json::from_str(&render_json(&report(1, raw))).unwrap();
        assert_eq!(doc["status"]["future_field"], "kept");
        assert_eq!(doc["sessions"], 2);
        assert_eq!(doc["tabs"], 3);
        assert_eq!(doc["socket"], "/run/st.sock");
    }
}
