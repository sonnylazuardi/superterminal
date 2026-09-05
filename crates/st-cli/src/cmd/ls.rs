//! `st ls` — the Workspace document as a tree.
//!
//! `workspace.get` returns `{workspace, surfaces}` (`02-protocol.md` §3.2), so
//! the tree is Session → Tab → Surface with the Surface's metadata looked up
//! by id.
//!
//! **On `pid`:** the M1 brief asks for a pid per surface, but `SurfaceMeta`
//! (§3.2) has no such field — only `03-server.md` §11's `status` document
//! mentions one. The raw JSON is therefore scanned for an optional `pid` key
//! per surface and printed when the server provides it; a v1.0 server that
//! follows §3.2 to the letter simply shows no pid.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use serde_json::Value;
use st_proto::control::{Layout, Req, SplitAxis, SurfaceMeta, SurfaceState, WorkspaceSnapshot};
use st_proto::SurfaceId;

use crate::control::ControlClient;
use crate::exit::{CliError, Result};
use crate::transport::Connector;

/// Runs the command.
pub fn run(connector: &dyn Connector, json: bool, out: &mut dyn Write) -> Result<()> {
    let mut client = ControlClient::connect(connector)?;
    let raw = client.request_raw(|id| Req::WorkspaceGet { id })?;
    let snapshot: WorkspaceSnapshot = serde_json::from_value(raw.clone()).map_err(|err| {
        CliError::protocol(format!(
            "workspace.get result is not a WorkspaceSnapshot: {err}"
        ))
    })?;

    let text = if json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&raw).unwrap_or_else(|_| "{}".into())
        )
    } else {
        render_tree(&snapshot, &pids(&raw))
    };
    out.write_all(text.as_bytes())
        .map_err(|e| CliError::failure(format!("cannot write to stdout: {e}")))
}

/// Extracts the optional per-surface `pid` from the raw result.
#[must_use]
pub fn pids(raw: &Value) -> BTreeMap<u32, u64> {
    raw.get("surfaces")
        .and_then(Value::as_array)
        .map(|surfaces| {
            surfaces
                .iter()
                .filter_map(|s| {
                    let id = u32::try_from(s.get("id")?.as_u64()?).ok()?;
                    Some((id, s.get("pid")?.as_u64()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Renders the tree. Pure, so the shape is unit-testable.
#[must_use]
pub fn render_tree(snapshot: &WorkspaceSnapshot, pids: &BTreeMap<u32, u64>) -> String {
    let by_id: BTreeMap<SurfaceId, &SurfaceMeta> =
        snapshot.surfaces.iter().map(|s| (s.id, s)).collect();
    let mut attached: BTreeSet<SurfaceId> = BTreeSet::new();

    let mut out = String::new();
    if snapshot.workspace.sessions.is_empty() {
        out.push_str("no sessions\n");
    }

    for session in &snapshot.workspace.sessions {
        let active = if session.id == snapshot.workspace.active_session {
            " (active)"
        } else {
            ""
        };
        out.push_str(&format!(
            "session {} {:?}{active}\n",
            session.id, session.name
        ));
        if session.tabs.is_empty() {
            out.push_str("  (no tabs)\n");
        }
        for tab in &session.tabs {
            let active = if Some(tab.id) == session.active_tab {
                " (active)"
            } else {
                ""
            };
            out.push_str(&format!("  tab {}{active}\n", tab.id));
            render_layout(&tab.layout, 2, &by_id, pids, &mut attached, &mut out);
        }
    }

    let detached: Vec<_> = snapshot
        .surfaces
        .iter()
        .filter(|s| !attached.contains(&s.id))
        .collect();
    if !detached.is_empty() {
        out.push_str("detached surfaces\n");
        for meta in detached {
            out.push_str(&format!(
                "  {}\n",
                surface_line(meta, pids.get(&meta.id.get()).copied())
            ));
        }
    }

    out
}

/// Renders a Tab's Panes: a single Pane is one indented surface line, a Split
/// is a `split <axis> <ratio>` line with its children indented under it.
fn render_layout(
    layout: &Layout,
    depth: usize,
    by_id: &BTreeMap<SurfaceId, &SurfaceMeta>,
    pids: &BTreeMap<u32, u64>,
    attached: &mut BTreeSet<SurfaceId>,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    match layout {
        Layout::Leaf { surface } => {
            attached.insert(*surface);
            let line = match by_id.get(surface) {
                Some(meta) => surface_line(meta, pids.get(&surface.get()).copied()),
                None => format!("surface {surface} <not in the surfaces list>"),
            };
            out.push_str(&format!("{indent}{line}\n"));
        }
        Layout::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let axis = match axis {
                SplitAxis::Row => "row",
                SplitAxis::Column => "column",
            };
            out.push_str(&format!("{indent}split {axis} {:.2}\n", ratio.as_f32()));
            render_layout(first, depth + 1, by_id, pids, attached, out);
            render_layout(second, depth + 1, by_id, pids, attached, out);
        }
    }
}

/// One surface's line: id, title, state, size, cwd and whatever else is known.
#[must_use]
pub fn surface_line(meta: &SurfaceMeta, pid: Option<u64>) -> String {
    let mut line = format!(
        "surface {}  {:?}  {}  {}x{}  cwd={}",
        meta.id,
        meta.user_title.as_ref().unwrap_or(&meta.title),
        describe_state(&meta.state),
        meta.cols,
        meta.rows,
        meta.cwd.as_deref().unwrap_or("-"),
    );
    if let Some(user_title) = &meta.user_title {
        if user_title != &meta.title {
            line.push_str(&format!("  title={:?}", meta.title));
        }
    }
    if let Some(pid) = pid {
        line.push_str(&format!("  pid={pid}"));
    }
    if meta.has_foreground_child {
        line.push_str("  fg");
    }
    line
}

/// `running`, `exited code=0`, `exited signal=SIGTERM`, or a bare `exited`.
#[must_use]
pub fn describe_state(state: &SurfaceState) -> String {
    match state {
        SurfaceState::Running => "running".into(),
        SurfaceState::Exited {
            code: Some(code), ..
        } => format!("exited code={code}"),
        SurfaceState::Exited {
            signal: Some(signal),
            ..
        } => format!("exited signal={signal}"),
        SurfaceState::Exited { .. } => "exited".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::control::{Session, SplitRatio, Tab, ViewState, Workspace};
    use st_proto::{SessionId, TabId};

    fn meta(id: u32, title: &str, cwd: Option<&str>, state: SurfaceState) -> SurfaceMeta {
        SurfaceMeta {
            id: SurfaceId(id),
            title: title.into(),
            user_title: None,
            cwd: cwd.map(Into::into),
            cols: 200,
            rows: 60,
            has_foreground_child: false,
            state,
            view_state: ViewState::default(),
        }
    }

    fn snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            workspace: Workspace {
                revision: 42,
                active_session: SessionId(1),
                sessions: vec![
                    Session {
                        id: SessionId(1),
                        name: "Default".into(),
                        active_tab: Some(TabId(12)),
                        tabs: vec![
                            Tab::leaf(TabId(12), SurfaceId(9)),
                            Tab::leaf(TabId(13), SurfaceId(10)),
                        ],
                    },
                    Session {
                        id: SessionId(2),
                        name: "notes".into(),
                        active_tab: None,
                        tabs: Vec::new(),
                    },
                ],
            },
            surfaces: vec![
                meta(9, "zsh", Some("/home/sonny"), SurfaceState::Running),
                meta(
                    10,
                    "vim",
                    None,
                    SurfaceState::Exited {
                        code: Some(1),
                        signal: None,
                    },
                ),
            ],
        }
    }

    #[test]
    fn the_tree_has_the_expected_shape() {
        assert_eq!(
            render_tree(&snapshot(), &BTreeMap::new()),
            "session 1 \"Default\" (active)\n\
             \x20 tab 12 (active)\n\
             \x20   surface 9  \"zsh\"  running  200x60  cwd=/home/sonny\n\
             \x20 tab 13\n\
             \x20   surface 10  \"vim\"  exited code=1  200x60  cwd=-\n\
             session 2 \"notes\"\n\
             \x20 (no tabs)\n"
        );
    }

    #[test]
    fn a_split_tab_lists_its_panes_under_the_splits() {
        let mut snap = snapshot();
        snap.surfaces
            .push(meta(11, "top", Some("/tmp"), SurfaceState::Running));
        snap.workspace.sessions[0].tabs[0] = Tab::with_layout(
            TabId(12),
            Layout::Split {
                axis: SplitAxis::Row,
                ratio: SplitRatio::HALF,
                first: Box::new(Layout::leaf(SurfaceId(9))),
                second: Box::new(Layout::Split {
                    axis: SplitAxis::Column,
                    ratio: SplitRatio::from_f32(0.25),
                    first: Box::new(Layout::leaf(SurfaceId(11))),
                    second: Box::new(Layout::leaf(SurfaceId(10))),
                }),
            },
        );
        snap.workspace.sessions[0].tabs.pop();
        assert_eq!(
            render_tree(&snap, &BTreeMap::new()),
            "session 1 \"Default\" (active)\n\
             \x20 tab 12 (active)\n\
             \x20   split row 0.50\n\
             \x20     surface 9  \"zsh\"  running  200x60  cwd=/home/sonny\n\
             \x20     split column 0.25\n\
             \x20       surface 11  \"top\"  running  200x60  cwd=/tmp\n\
             \x20       surface 10  \"vim\"  exited code=1  200x60  cwd=-\n\
             session 2 \"notes\"\n\
             \x20 (no tabs)\n"
        );
    }

    #[test]
    fn a_pid_and_a_foreground_child_are_shown_when_known() {
        let mut snap = snapshot();
        snap.surfaces[0].has_foreground_child = true;
        let pids = BTreeMap::from([(9u32, 4242u64)]);
        let tree = render_tree(&snap, &pids);
        assert!(tree.contains("cwd=/home/sonny  pid=4242  fg\n"), "{tree}");
    }

    #[test]
    fn a_user_title_wins_and_the_program_title_is_kept() {
        let mut m = meta(9, "zsh", None, SurfaceState::Running);
        m.user_title = Some("build".into());
        assert_eq!(
            surface_line(&m, None),
            "surface 9  \"build\"  running  200x60  cwd=-  title=\"zsh\""
        );
    }

    #[test]
    fn surfaces_no_tab_points_at_are_listed_separately() {
        let mut snap = snapshot();
        snap.surfaces
            .push(meta(11, "detached", None, SurfaceState::Running));
        let tree = render_tree(&snap, &BTreeMap::new());
        assert!(tree
            .ends_with("detached surfaces\n  surface 11  \"detached\"  running  200x60  cwd=-\n"));
    }

    #[test]
    fn a_tab_pointing_at_a_missing_surface_still_renders() {
        let mut snap = snapshot();
        snap.surfaces.remove(0);
        let tree = render_tree(&snap, &BTreeMap::new());
        assert!(tree.contains("surface 9 <not in the surfaces list>"));
    }

    #[test]
    fn an_empty_workspace_says_so() {
        let snap = WorkspaceSnapshot::default();
        assert_eq!(render_tree(&snap, &BTreeMap::new()), "no sessions\n");
    }

    #[test]
    fn exit_states_are_spelled_out() {
        assert_eq!(describe_state(&SurfaceState::Running), "running");
        assert_eq!(
            describe_state(&SurfaceState::Exited {
                code: Some(0),
                signal: None
            }),
            "exited code=0"
        );
        assert_eq!(
            describe_state(&SurfaceState::Exited {
                code: None,
                signal: Some("SIGTERM".into())
            }),
            "exited signal=SIGTERM"
        );
        assert_eq!(
            describe_state(&SurfaceState::Exited {
                code: None,
                signal: None
            }),
            "exited"
        );
    }

    #[test]
    fn pids_are_read_out_of_the_raw_result() {
        let raw = serde_json::json!({
            "surfaces": [
                {"id": 9, "pid": 4242},
                {"id": 10},
            ]
        });
        assert_eq!(pids(&raw), BTreeMap::from([(9u32, 4242u64)]));
        assert!(pids(&serde_json::json!({})).is_empty());
    }
}
