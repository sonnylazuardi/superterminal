//! `st status` and `st ls` against a fake server that speaks real NDJSON.

mod common;

use common::{code, stderr, stdout, FakeServer, Seen};
use serde_json::{json, Value};

fn status_result() -> Value {
    json!({
        "build_id": "cafe1234-dirty",
        "proto_version": "1.0",
        "pid": 4242,
        "uptime_s": 3_661,
        "surfaces": 2,
        "control_clients": 1,
        "data_clients": 3,
        "workspace_file": "/state/superterminal/workspace.json",
        "metrics": {
            "pty_bytes_in": 1_572_864,
            "pty_bytes_out": 302,
            "deltas_sent": 7_322,
            "snapshots_sent": 11,
        },
    })
}

fn workspace_result() -> Value {
    json!({
        "workspace": {
            "revision": 42,
            "active_session": 1,
            "sessions": [
                {
                    "id": 1,
                    "name": "Default",
                    "active_tab": 12,
                    "tabs": [{"id": 12, "surface": 9}, {"id": 13, "surface": 10}]
                },
                {"id": 2, "name": "notes", "active_tab": null, "tabs": []}
            ]
        },
        "surfaces": [
            {
                "id": 9, "title": "zsh", "user_title": null,
                "cwd": "/home/sonny/projects/superterminal",
                "cols": 200, "rows": 60, "has_foreground_child": true,
                "state": {"kind": "running"},
                "view_state": {"scroll_offset": 0, "selection": null},
                "pid": 5150
            },
            {
                "id": 10, "title": "vim", "user_title": "editor",
                "cwd": null,
                "cols": 80, "rows": 24, "has_foreground_child": false,
                "state": {"kind": "exited", "code": 1, "signal": null},
                "view_state": {"scroll_offset": 0, "selection": null}
            }
        ]
    })
}

fn server() -> FakeServer {
    FakeServer::builder()
        .control(|req| match req["t"].as_str() {
            Some("server.status") => status_result(),
            Some("workspace.get") => workspace_result(),
            other => json!({"__err": {"code": "bad_request", "message": format!("no {other:?}")}}),
        })
        .start()
}

#[test]
fn status_prints_build_uptime_and_counts() {
    let server = server();
    let out = server.run(&["status"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));

    let text = stdout(&out);
    assert!(text.contains("build       cafe1234-dirty\n"), "{text}");
    assert!(text.contains("protocol    1.0\n"), "{text}");
    assert!(text.contains("pid         4242\n"), "{text}");
    assert!(text.contains("uptime      1h 01m 01s\n"), "{text}");
    assert!(text.contains("sessions    2\n"), "{text}");
    assert!(text.contains("tabs        2\n"), "{text}");
    assert!(text.contains("surfaces    2\n"), "{text}");
    assert!(text.contains("clients     1 control, 3 data\n"), "{text}");
    assert!(text.contains("pty in/out  1.5 MiB / 302 B\n"), "{text}");
    assert!(text.contains("deltas      7322 (2.0/s)\n"), "{text}");
    assert!(text.contains("snapshots   11\n"), "{text}");
    assert!(
        text.contains(&format!("socket      {}\n", server.socket().display())),
        "{text}"
    );
}

#[test]
fn status_sends_a_tool_hello_then_two_requests() {
    let server = server();
    assert_eq!(code(&server.run(&["status"])), 0);

    let seen: Vec<Value> = server
        .wait_seen(3)
        .into_iter()
        .map(|s| match s {
            Seen::Control(v) => v,
            Seen::Data(d) => panic!("unexpected data message {d}"),
        })
        .collect();

    assert_eq!(seen[0]["t"], "hello");
    assert_eq!(seen[0]["client_kind"], "tool");
    assert_eq!(seen[0]["proto_version"], "1.0");
    assert!(seen[0]["build_id"].is_string());

    assert_eq!(seen[1]["t"], "server.status");
    assert_eq!(seen[2]["t"], "workspace.get");
    // §3.1: ids are client-chosen and unique while outstanding.
    assert_ne!(seen[1]["id"], seen[2]["id"]);
}

#[test]
fn status_json_keeps_the_servers_own_document() {
    let out = server().run(&["status", "--json"]);
    assert_eq!(code(&out), 0);
    let doc: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["status"], status_result());
    assert_eq!(doc["sessions"], 2);
    assert_eq!(doc["tabs"], 2);
    assert_eq!(doc["revision"], 42);
}

#[test]
fn ls_renders_the_session_tab_surface_tree() {
    let out = server().run(&["ls"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "session 1 \"Default\" (active)\n\
         \x20 tab 12 (active)\n\
         \x20   surface 9  \"zsh\"  running  200x60  \
         cwd=/home/sonny/projects/superterminal  pid=5150  fg\n\
         \x20 tab 13\n\
         \x20   surface 10  \"editor\"  exited code=1  80x24  cwd=-  title=\"vim\"\n\
         session 2 \"notes\"\n\
         \x20 (no tabs)\n"
    );
}

#[test]
fn ls_json_is_the_raw_workspace_document() {
    let out = server().run(&["ls", "--json"]);
    assert_eq!(code(&out), 0);
    let doc: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc, workspace_result());
}

#[test]
fn the_socket_environment_variable_is_honoured() {
    let server = server();
    let out = server.run_via_env(&["ls"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).starts_with("session 1 \"Default\""));
}

#[test]
fn the_socket_flag_beats_the_environment_variable() {
    let server = server();
    let out = common::run_st_env(
        Some(server.socket()),
        &[("SUPERTERMINAL_SOCKET", "/definitely/not/a/socket")],
        &["ls"],
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("session 1"));
}

#[test]
fn a_missing_socket_exits_three_with_a_helpful_message() {
    let out = common::run_st(
        Some(std::path::Path::new("/nonexistent/st.sock")),
        &["status"],
    );
    assert_eq!(code(&out), 3);
    assert!(stdout(&out).is_empty());
    let err = stderr(&out);
    assert!(
        err.contains("no server socket at /nonexistent/st.sock"),
        "{err}"
    );
    assert!(err.contains("--socket"), "{err}");
}

#[test]
fn a_rejected_handshake_exits_four() {
    let server = FakeServer::builder().rejecting().start();
    let out = server.run(&["status"]);
    assert_eq!(code(&out), 4);
    let err = stderr(&out);
    assert!(err.contains("server rejected the connection"), "{err}");
    assert!(err.contains("MajorMismatch"), "{err}");
}

#[test]
fn a_server_that_says_nothing_exits_three() {
    let server = FakeServer::builder().silent().start();
    let out = server.run(&["ls"]);
    assert_eq!(code(&out), 3);
    assert!(
        stderr(&out).contains("closed the control connection"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_not_found_error_exits_five() {
    let server = FakeServer::builder()
        .control(|_| json!({"__err": {"code": "not_found", "message": "no such thing"}}))
        .start();
    let out = server.run(&["ls"]);
    assert_eq!(code(&out), 5);
    assert!(
        stderr(&out).contains("workspace.get failed: no such thing"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn other_server_errors_exit_six() {
    let server = FakeServer::builder()
        .control(|_| json!({"__err": {"code": "internal", "message": "boom"}}))
        .start();
    assert_eq!(code(&server.run(&["ls"])), 6);
}

#[test]
fn kill_server_asks_the_control_plane_and_reports_the_pid() {
    let server = FakeServer::builder()
        .control(|req| {
            assert_eq!(req["t"], "server.shutdown");
            assert!(
                req.get("force").is_none(),
                "no --force means no force field"
            );
            json!({})
        })
        .start();
    let out = server.run(&["kill-server"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "server 4242 is shutting down\n");
}

#[test]
fn kill_server_force_sets_the_force_field() {
    let server = FakeServer::builder()
        .control(|req| {
            assert_eq!(req["force"], true);
            json!({})
        })
        .start();
    assert_eq!(code(&server.run(&["kill-server", "--force"])), 0);
}

#[test]
fn a_refused_shutdown_suggests_force() {
    let server = FakeServer::builder()
        .control(|_| json!({"__err": {"code": "conflict", "message": "2 surfaces are live"}}))
        .start();
    let out = server.run(&["kill-server"]);
    assert_eq!(code(&out), 6);
    let err = stderr(&out);
    assert!(err.contains("2 surfaces are live"), "{err}");
    assert!(err.contains("--force"), "{err}");
}

#[test]
fn kill_server_force_falls_back_to_the_lockfile_pid() {
    // No server at all, but a lockfile beside the socket path.
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("server.sock");
    std::fs::write(dir.path().join(st_config::LOCK_FILE_NAME), "999999\n").unwrap();

    let out = common::run_st(Some(&socket), &["kill-server", "--force"]);
    // pid 999999 does not exist, so kill(1) fails — but the fallback ran,
    // which is what this test is about.
    assert_ne!(
        code(&out),
        3,
        "should not report 'no server': {}",
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("sent SIGTERM to pid 999999")
            || stderr(&out).contains("kill -TERM 999999 failed"),
        "stdout={} stderr={}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn kill_server_without_force_does_not_touch_the_lockfile() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("server.sock");
    std::fs::write(dir.path().join(st_config::LOCK_FILE_NAME), "999999\n").unwrap();

    let out = common::run_st(Some(&socket), &["kill-server"]);
    assert_eq!(code(&out), 3);
    assert!(
        stderr(&out).contains("no server socket"),
        "{}",
        stderr(&out)
    );
}
