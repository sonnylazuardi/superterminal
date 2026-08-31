//! End-to-end CONTROL plane tests — `docs/plan/02-protocol.md` §2–§3.
//!
//! A whole daemon runs in-process on a temporary socket with a `NullSpawner`,
//! and a raw NDJSON client drives it. Every assertion is against the JSON on
//! the wire, not against internal types.

mod common;

use std::time::Duration;

use common::{spawn_spec, subscribe, Client, Harness};
use serde_json::json;

#[tokio::test]
async fn the_handshake_acks_a_matching_major() {
    let harness = Harness::start().await;
    let mut client = Client::connect(&harness.socket()).await;

    let ack = client.hello("1.0").await;
    assert_eq!(ack["t"], "hello.ack");
    assert_eq!(ack["proto_version"], "1.0");
    assert_eq!(ack["server_build_id"], "test-build");
    assert_eq!(ack["workspace_revision"], 0);
    assert_eq!(ack["server_pid"], json!(std::process::id()));
}

#[tokio::test]
async fn a_higher_client_minor_negotiates_down() {
    let harness = Harness::start().await;
    let mut client = Client::connect(&harness.socket()).await;

    let ack = client.hello("1.9").await;
    assert_eq!(ack["t"], "hello.ack");
    assert_eq!(ack["proto_version"], "1.0", "negotiated minor is the lower");
}

#[tokio::test]
async fn a_major_mismatch_is_rejected() {
    let harness = Harness::start().await;
    let mut client = Client::connect(&harness.socket()).await;

    let reject = client.hello("2.0").await;
    assert_eq!(reject["t"], "reject");
    assert_eq!(reject["reason"], "major_mismatch");
    assert_eq!(reject["server_version"], "1.0");
    assert!(reject["message"].as_str().unwrap().contains("2.0"));
    assert!(client.read_line().await.is_none(), "and the socket closes");
}

#[tokio::test]
async fn the_first_message_must_be_hello() {
    let harness = Harness::start().await;
    let mut client = Client::connect(&harness.socket()).await;

    client.send(json!({ "t": "workspace.get", "id": 1 })).await;
    let reject = client.read_line().await.expect("a reject");
    assert_eq!(reject["t"], "reject");
    assert_eq!(reject["reason"], "not_hello");
}

#[tokio::test]
async fn the_daemon_starts_with_one_default_session_and_one_tab() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let result = client.ok(json!({ "t": "workspace.get" })).await;
    let workspace = &result["workspace"];
    assert_eq!(workspace["revision"], 0);
    assert_eq!(workspace["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(workspace["sessions"][0]["name"], "Default");
    assert_eq!(
        workspace["sessions"][0]["tabs"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        workspace["active_session"], workspace["sessions"][0]["id"],
        "the only session is the active one"
    );
    assert_eq!(result["surfaces"].as_array().unwrap().len(), 1);
    assert_eq!(result["surfaces"][0]["state"]["kind"], "running");
    assert_eq!(result["surfaces"][0]["view_state"]["scroll_offset"], 0);
}

#[tokio::test]
async fn sessions_are_created_renamed_listed_and_deleted() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.ok(subscribe()).await;

    let created = client
        .ok(json!({ "t": "session.create", "name": "work" }))
        .await;
    let session = created["session"].as_u64().unwrap();
    assert_eq!(created["revision"], 1);

    let event = client.next_workspace_event().await;
    assert_eq!(event["revision"], 1);
    assert_eq!(event["workspace"]["sessions"][1]["name"], "work");

    let renamed = client
        .ok(json!({ "t": "session.rename", "session": session, "name": "play" }))
        .await;
    assert_eq!(renamed["revision"], 2);

    let listed = client.ok(json!({ "t": "session.list" })).await;
    let names: Vec<&str> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Default", "play"]);

    let activated = client
        .ok(json!({ "t": "session.set_active", "session": session }))
        .await;
    assert_eq!(activated["revision"], 3);
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(after["workspace"]["active_session"], json!(session));

    client
        .ok(json!({ "t": "session.delete", "session": session }))
        .await;
    let listed = client.ok(json!({ "t": "session.list" })).await;
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn tabs_are_created_reordered_moved_activated_and_closed() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.ok(subscribe()).await;

    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    let default = snapshot["workspace"]["sessions"][0]["id"].as_u64().unwrap();
    let first_tab = snapshot["workspace"]["sessions"][0]["tabs"][0]["id"]
        .as_u64()
        .unwrap();

    let created = client
        .ok(json!({ "t": "tab.create", "session": default, "spawn": spawn_spec() }))
        .await;
    let tab = created["tab"].as_u64().unwrap();
    let surface = created["surface"].as_u64().unwrap();
    assert_eq!(created["revision"], 1);

    let event = client.next_workspace_event().await;
    assert_eq!(
        event["workspace"]["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(event["workspace"]["sessions"][0]["active_tab"], json!(tab));
    let meta = event["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == json!(surface))
        .expect("the new surface is in the document");
    assert_eq!(meta["cols"], 100);
    assert_eq!(meta["rows"], 30);

    client
        .ok(json!({ "t": "tab.reorder", "tab": tab, "index": 0 }))
        .await;
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(
        after["workspace"]["sessions"][0]["tabs"][0]["id"],
        json!(tab)
    );

    client
        .ok(json!({ "t": "tab.set_active", "tab": first_tab }))
        .await;
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(
        after["workspace"]["sessions"][0]["active_tab"],
        json!(first_tab)
    );

    let other = client
        .ok(json!({ "t": "session.create", "name": "other" }))
        .await["session"]
        .as_u64()
        .unwrap();
    client
        .ok(json!({ "t": "tab.move", "tab": tab, "to_session": other }))
        .await;
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(
        after["workspace"]["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        after["workspace"]["sessions"][1]["tabs"][0]["id"],
        json!(tab)
    );

    client.ok(json!({ "t": "tab.close", "tab": tab })).await;
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(
        after["workspace"]["sessions"].as_array().unwrap().len(),
        1,
        "closing the last tab of a session deletes it (grilling Q21)"
    );
    assert!(
        !after["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == json!(surface)),
        "and its surface is gone"
    );
}

#[tokio::test]
async fn closing_the_very_last_tab_reseeds_a_default_session() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    let tab = snapshot["workspace"]["sessions"][0]["tabs"][0]["id"]
        .as_u64()
        .unwrap();

    client.ok(json!({ "t": "tab.close", "tab": tab })).await;

    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(after["workspace"]["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(after["workspace"]["sessions"][0]["name"], "Default");
    assert_eq!(
        after["workspace"]["sessions"][0]["tabs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_ne!(
        after["workspace"]["sessions"][0]["tabs"][0]["id"],
        json!(tab),
        "the re-seeded tab is a new one"
    );
}

#[tokio::test]
async fn a_detached_surface_can_be_created_and_adopted() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let created = client
        .ok(json!({ "t": "surface.create", "spawn": spawn_spec() }))
        .await;
    let surface = created["surface"].as_u64().unwrap();
    assert!(
        created.get("revision").is_none(),
        "surface.create returns only the id"
    );

    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    let session = snapshot["workspace"]["sessions"][0]["id"].as_u64().unwrap();
    assert!(
        snapshot["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == json!(surface)),
        "a detached surface is still in the document"
    );

    let tab = client
        .ok(json!({ "t": "tab.create", "session": session, "surface": surface }))
        .await;
    assert_eq!(tab["surface"], json!(surface));

    // Adopting it twice is a bad request.
    let code = client
        .err(json!({ "t": "tab.create", "session": session, "surface": surface }))
        .await;
    assert_eq!(code, "bad_request");
}

#[tokio::test]
async fn tab_create_needs_exactly_one_of_spawn_and_surface() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    let session = snapshot["workspace"]["sessions"][0]["id"].as_u64().unwrap();

    assert_eq!(
        client
            .err(json!({ "t": "tab.create", "session": session }))
            .await,
        "bad_request"
    );
    assert_eq!(
        client
            .err(json!({
                "t": "tab.create", "session": session,
                "spawn": spawn_spec(), "surface": 1
            }))
            .await,
        "bad_request"
    );
}

#[tokio::test]
async fn surfaces_are_renamed_and_killed() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    let surface = snapshot["surfaces"][0]["id"].as_u64().unwrap();

    let renamed = client
        .ok(json!({ "t": "surface.rename", "surface": surface, "user_title": "build" }))
        .await;
    assert_eq!(renamed["revision"], 1);
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(after["surfaces"][0]["user_title"], "build");

    let cleared = client
        .ok(json!({ "t": "surface.rename", "surface": surface, "user_title": null }))
        .await;
    assert_eq!(cleared["revision"], 2);
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(after["surfaces"][0]["user_title"], json!(null));

    let killed = client
        .ok(json!({ "t": "surface.kill", "surface": surface, "signal": "TERM" }))
        .await;
    assert_eq!(killed, json!({}));

    assert_eq!(
        client
            .err(json!({ "t": "surface.kill", "surface": 9999 }))
            .await,
        "not_found"
    );
}

#[tokio::test]
async fn view_set_is_echoed_to_others_but_not_to_its_author() {
    let harness = Harness::start().await;
    let mut author = harness.client().await;
    let mut watcher = harness.client().await;

    let snapshot = author.ok(subscribe()).await;
    watcher.ok(subscribe()).await;
    let surface = snapshot["surfaces"][0]["id"].as_u64().unwrap();

    let result = author
        .ok(json!({
            "t": "view.set",
            "surface": surface,
            "scroll_offset": 42,
            "selection": {
                "kind": "lines",
                "anchor": { "line": 10, "col": 0 },
                "head": { "line": 11, "col": 7 }
            }
        }))
        .await;
    assert_eq!(result["revision"], 1);

    let event = watcher.next_workspace_event().await;
    assert_eq!(event["revision"], 1);
    assert_eq!(event["surfaces"][0]["view_state"]["scroll_offset"], 42);
    assert_eq!(
        event["surfaces"][0]["view_state"]["selection"]["kind"],
        "lines"
    );

    assert!(
        author.is_quiet_for(Duration::from_millis(250)).await,
        "the author of a view-only change gets no echo (§3.3)"
    );

    // A `null` selection clears it; an absent field leaves it alone.
    author
        .ok(json!({ "t": "view.set", "surface": surface, "selection": null }))
        .await;
    let after = author.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(after["surfaces"][0]["view_state"]["selection"], json!(null));
    assert_eq!(
        after["surfaces"][0]["view_state"]["scroll_offset"], 42,
        "an absent field is left alone"
    );
}

#[tokio::test]
async fn a_stale_if_revision_is_a_conflict() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    client
        .ok(json!({ "t": "session.create", "name": "one", "if_revision": 0 }))
        .await;

    let res = client
        .request(json!({ "t": "session.create", "name": "two", "if_revision": 0 }))
        .await;
    assert_eq!(res["t"], "err");
    assert_eq!(res["error"]["code"], "conflict");
    assert_eq!(res["error"]["data"]["revision"], 1);

    // The current revision is accepted.
    client
        .ok(json!({ "t": "session.create", "name": "two", "if_revision": 1 }))
        .await;
}

#[tokio::test]
async fn unknown_ids_and_unknown_requests_are_reported() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    assert_eq!(
        client
            .err(json!({ "t": "session.rename", "session": 999, "name": "x" }))
            .await,
        "not_found"
    );
    assert_eq!(
        client.err(json!({ "t": "tab.close", "tab": 999 })).await,
        "not_found"
    );
    assert_eq!(client.err(json!({ "t": "nope.nope" })).await, "bad_request");

    // A line that is not even JSON still gets one response.
    client.send_raw(b"this is not json\n").await;
    let res = client.read_line().await.expect("an error envelope");
    assert_eq!(res["t"], "err");
    assert_eq!(res["id"], 0);
    assert_eq!(res["error"]["code"], "bad_request");

    // ...and the connection survives it.
    client.ok(json!({ "t": "workspace.get" })).await;
}

#[tokio::test]
async fn events_only_flow_after_subscribe() {
    let harness = Harness::start().await;
    let mut quiet = harness.client().await;
    let mut loud = harness.client().await;
    loud.ok(subscribe()).await;

    loud.ok(json!({ "t": "session.create", "name": "noise" }))
        .await;
    let event = loud.next_workspace_event().await;
    assert_eq!(event["revision"], 1);

    assert!(
        quiet.is_quiet_for(Duration::from_millis(250)).await,
        "a connection that never subscribed receives no events (§3.1)"
    );
}

#[tokio::test]
async fn server_status_reports_the_daemon_and_its_metrics() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let status = client.ok(json!({ "t": "server.status" })).await;
    assert_eq!(status["build_id"], "test-build");
    assert_eq!(status["proto_version"], "1.0");
    assert_eq!(status["pid"], json!(std::process::id()));
    assert_eq!(status["surfaces"], 1);
    assert_eq!(status["control_clients"], 1);
    assert_eq!(status["data_clients"], 0);
    assert_eq!(
        status["workspace_file"],
        json!(harness.workspace_file().display().to_string())
    );

    let metrics = &status["metrics"];
    assert!(metrics["requests_handled"].as_u64().unwrap() >= 1);
    assert_eq!(metrics["connections_control"], 1);
    for key in ["pty_bytes_in", "deltas_sent", "coalesce_ratio", "revisions"] {
        assert!(metrics.get(key).is_some(), "`st status` reads {key}");
    }
}

#[tokio::test]
async fn server_shutdown_refuses_while_surfaces_run_unless_forced() {
    let mut harness = Harness::start().await;
    let mut client = harness.client().await;

    let res = client.request(json!({ "t": "server.shutdown" })).await;
    assert_eq!(res["t"], "err");
    assert_eq!(res["error"]["code"], "conflict");
    assert_eq!(res["error"]["data"]["surfaces"], 1);

    client.ok(subscribe()).await;
    let forced = client
        .ok(json!({ "t": "server.shutdown", "force": true }))
        .await;
    assert_eq!(forced, json!({}));

    let server = harness.server.take().expect("running");
    let reason = tokio::time::timeout(Duration::from_secs(5), server.wait())
        .await
        .expect("the daemon stops")
        .expect("cleanly");
    assert_eq!(reason, "server.shutdown");
    assert!(!harness.socket().exists(), "the socket is unlinked");
}

#[tokio::test]
async fn a_shutdown_notice_reaches_subscribers() {
    let mut harness = Harness::start().await;
    let mut client = harness.client().await;
    client.ok(subscribe()).await;

    let server = harness.server.take().expect("running");
    tokio::spawn(async move {
        let _ = server.stop("test wants out").await;
    });

    let event = client.next_event().await;
    assert_eq!(event["t"], "ev.server_shutting_down");
    assert_eq!(event["reason"], "test wants out");
}
