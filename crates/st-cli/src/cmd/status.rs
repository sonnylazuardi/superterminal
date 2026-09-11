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
//!
//! **Scrollback.** The server may also add a `scrollback` array beside
//! `metrics`, one entry per Surface (`scrollback_rows`, `scrollback_bytes`;
//! handover B.2 step 7). It is printed as a per-Surface block and kept in
//! `--json` under `status.scrollback`; an older server simply omits it.

use std::collections::BTreeMap;
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
    /// Repeated Attaches answered with a Snapshot (Resyncs).
    pub resyncs: Option<u64>,
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
            resyncs: get("resyncs"),
        }
    }

    /// `true` when the server reported nothing at all.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// One Surface's scrollback footprint, parsed from the optional `scrollback`
/// array in the `server.status` result (handover B.2 step 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scrollback {
    /// Surface id.
    pub surface: u32,
    /// Retained History rows.
    pub rows: u64,
    /// Approximate bytes those rows occupy.
    pub bytes: u64,
}

impl Scrollback {
    /// Parses the array, dropping entries with a missing or non-numeric field,
    /// so a future server shape degrades to "not reported" rather than an
    /// error. An absent key yields an empty vector.
    #[must_use]
    pub fn from_status_value(value: &Value) -> Vec<Self> {
        value
            .get("scrollback")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Some(Self {
                            surface: u32::try_from(entry.get("surface")?.as_u64()?).ok()?,
                            rows: entry.get("scrollback_rows")?.as_u64()?,
                            bytes: entry.get("scrollback_bytes")?.as_u64()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
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
    /// Per-Surface scrollback; empty when the server does not report it.
    pub scrollback: Vec<Scrollback>,
    /// Surface ids to display titles, for the scrollback block.
    pub surface_titles: BTreeMap<u32, String>,
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

    let surface_titles = workspace
        .surfaces
        .iter()
        .map(|meta| {
            let title = meta
                .user_title
                .clone()
                .unwrap_or_else(|| meta.title.clone());
            (meta.id.get(), title)
        })
        .collect();

    let report = StatusReport {
        socket: connector.describe(),
        metrics: Metrics::from_status_value(&raw),
        scrollback: Scrollback::from_status_value(&raw),
        surface_titles,
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
    if let Some(resyncs) = m.resyncs {
        lines.push(("resyncs", resyncs.to_string()));
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
    if !report.scrollback.is_empty() {
        let total_rows: u64 = report.scrollback.iter().map(|s| s.rows).sum();
        let total_bytes: u64 = report.scrollback.iter().map(|s| s.bytes).sum();
        lines.push((
            "scrollback",
            format!(
                "{} rows / {} across {}",
                total_rows,
                format_bytes(total_bytes),
                surface_count(report.scrollback.len()),
            ),
        ));
    }

    let width = lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut text = String::new();
    for (key, value) in lines {
        text.push_str(&format!("{key:width$}  {value}\n"));
    }
    for entry in &report.scrollback {
        text.push_str(&format!(
            "  {}\n",
            describe_scrollback(entry, &report.surface_titles)
        ));
    }
    text
}

/// `1 surface` / `2 surfaces`, for the scrollback total line.
fn surface_count(n: usize) -> String {
    if n == 1 {
        "1 surface".to_string()
    } else {
        format!("{n} surfaces")
    }
}

/// One per-Surface scrollback line: `surface 9 "zsh": 8421 rows, 32.9 MiB`.
fn describe_scrollback(entry: &Scrollback, titles: &BTreeMap<u32, String>) -> String {
    let mut line = format!("surface {}", entry.surface);
    if let Some(title) = titles.get(&entry.surface) {
        line.push_str(&format!(" {title:?}"));
    }
    line.push_str(&format!(
        ": {} rows, {}",
        entry.rows,
        format_bytes(entry.bytes)
    ));
    line
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
            scrollback: Scrollback::from_status_value(&raw),
            surface_titles: BTreeMap::new(),
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
            "resyncs": 2u64,
            "frames_out": 1_300u64,
        });
        let text = render_text(&report(100, raw));
        assert!(text.contains("pty in/out  1.5 MiB / 302 B\n"));
        assert!(text.contains("deltas      1200 (12.0/s)\n"));
        assert!(text.contains("snapshots   4\n"));
        assert!(text.contains("resyncs     2\n"));
        assert!(text.contains("frames out  1300 (13.0/s)\n"));
        assert!(!text.contains("not reported"));
    }

    #[test]
    fn resyncs_appears_and_defaults_to_zero() {
        let mut raw = base();
        raw["metrics"] = json!({ "snapshots_sent": 1u64, "resyncs": 0u64 });
        let metrics = Metrics::from_status_value(&raw);
        assert_eq!(metrics.resyncs, Some(0), "a reported zero is not 'absent'");
        let text = render_text(&report(10, raw));
        assert!(
            text.lines()
                .any(|line| line.starts_with("resyncs") && line.trim_end().ends_with('0')),
            "{text}"
        );

        // An older server that does not report the counter prints nothing.
        let mut without = base();
        without["metrics"] = json!({ "snapshots_sent": 1u64 });
        assert_eq!(Metrics::from_status_value(&without).resyncs, None);
        assert!(!render_text(&report(10, without)).contains("resyncs"));
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
    fn scrollback_is_printed_per_surface() {
        let mut raw = base();
        raw["scrollback"] = json!([
            {"surface": 9, "scrollback_rows": 8_421u64, "scrollback_bytes": 34_521_088u64},
            {"surface": 10, "scrollback_rows": 0u64, "scrollback_bytes": 0u64},
        ]);
        let mut r = report(10, raw);
        r.surface_titles = BTreeMap::from([(9, "zsh".to_string()), (10, "editor".to_string())]);
        let text = render_text(&r);
        assert!(
            text.contains("scrollback  8421 rows / 32.9 MiB across 2 surfaces\n"),
            "{text}"
        );
        assert!(
            text.contains("  surface 9 \"zsh\": 8421 rows, 32.9 MiB\n"),
            "{text}"
        );
        assert!(
            text.contains("  surface 10 \"editor\": 0 rows, 0 B\n"),
            "{text}"
        );
    }

    #[test]
    fn scrollback_lines_are_absent_when_the_server_does_not_report_them() {
        let text = render_text(&report(10, base()));
        assert!(!text.contains("scrollback"), "{text}");
    }

    #[test]
    fn scrollback_parser_skips_malformed_entries() {
        let raw = json!({
            "scrollback": [
                {"surface": 9, "scrollback_rows": 12, "scrollback_bytes": 2_304},
                {"surface": "ten", "scrollback_rows": 1, "scrollback_bytes": 24},
                {"surface": 11, "scrollback_rows": 3},
                {"surface": 12, "scrollback_rows": 4, "scrollback_bytes": 96},
            ]
        });
        assert_eq!(
            Scrollback::from_status_value(&raw),
            vec![
                Scrollback {
                    surface: 9,
                    rows: 12,
                    bytes: 2_304
                },
                Scrollback {
                    surface: 12,
                    rows: 4,
                    bytes: 96
                },
            ]
        );
        assert!(Scrollback::from_status_value(&base()).is_empty());
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
        raw["scrollback"] = json!([
            {"surface": 9, "scrollback_rows": 12u64, "scrollback_bytes": 2_304u64},
        ]);
        let doc: Value = serde_json::from_str(&render_json(&report(1, raw))).unwrap();
        assert_eq!(doc["status"]["future_field"], "kept");
        assert_eq!(doc["status"]["scrollback"][0]["scrollback_bytes"], 2_304);
        assert_eq!(doc["sessions"], 2);
        assert_eq!(doc["tabs"], 3);
        assert_eq!(doc["socket"], "/run/st.sock");
    }
}
