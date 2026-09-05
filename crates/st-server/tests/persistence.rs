//! `workspace.json` end to end — `docs/plan/03-server.md` §8, grilling Q18.

mod common;

use std::time::Duration;

use common::{spawn_spec, Harness};
use serde_json::json;
use st_config::Config;

/// The harness runs with an 80 ms debounce; anything longer is a real write.
const SETTLE: Duration = Duration::from_millis(1500);

#[tokio::test]
async fn a_mutation_is_written_after_the_debounce() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    assert!(
        harness.saved().is_none(),
        "nothing is written before a change"
    );

    let session = client
        .ok(json!({ "t": "session.create", "name": "work" }))
        .await["session"]
        .as_u64()
        .unwrap();
    client
        .ok(json!({ "t": "tab.create", "session": session, "spawn": spawn_spec() }))
        .await;

    assert!(
        harness
            .wait_until(SETTLE, || harness.saved().is_some())
            .await,
        "the debounced write lands"
    );

    let saved = harness.saved().unwrap();
    assert_eq!(saved["version"], 1);
    assert!(saved["saved_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(saved["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(saved["sessions"][1]["name"], "work");

    let tab = &saved["sessions"][1]["tabs"][0];
    assert_eq!(saved["sessions"][1]["active_tab"], tab["id"]);
    assert!(tab["surface"]["shell"].is_array(), "the argv is persisted");
    assert!(tab["surface"]["cwd"].is_string());

    let text = saved.to_string();
    assert!(
        !text.contains("scroll_offset"),
        "view state is not persisted"
    );
    assert!(
        !text.contains("\"state\""),
        "surface status is not persisted"
    );
    assert!(!text.contains("\"cols\""), "the grid size is not persisted");

    assert!(
        !harness.workspace_file().with_extension("json.tmp").exists(),
        "the atomic write leaves no temp file behind"
    );
}

#[tokio::test]
async fn a_restart_reseeds_the_saved_shape() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let session = client
        .ok(json!({ "t": "session.create", "name": "work" }))
        .await["session"]
        .as_u64()
        .unwrap();
    let created = client
        .ok(json!({ "t": "tab.create", "session": session, "spawn": spawn_spec() }))
        .await;
    let old_surface = created["surface"].as_u64().unwrap();
    client
        .ok(json!({ "t": "surface.rename", "surface": old_surface, "user_title": "keep me" }))
        .await;
    client
        .ok(json!({ "t": "session.set_active", "session": session }))
        .await;

    assert!(
        harness
            .wait_until(SETTLE, || harness.saved().is_some())
            .await
    );
    let before = harness.saved().unwrap();
    client.close().await;

    let harness = harness.restart().await;
    let mut client = harness.client().await;
    let after = client.ok(json!({ "t": "workspace.get" })).await;

    assert_eq!(
        after["workspace"]["revision"], 0,
        "a restart starts at revision 0"
    );
    assert_eq!(after["workspace"]["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(after["workspace"]["sessions"][1]["name"], "work");
    assert_eq!(
        after["workspace"]["sessions"][1]["id"], before["sessions"][1]["id"],
        "session ids survive"
    );
    assert_eq!(
        after["workspace"]["sessions"][1]["tabs"][0]["id"], before["sessions"][1]["tabs"][0]["id"],
        "tab ids survive"
    );
    assert_eq!(
        after["workspace"]["active_session"], before["active_session"],
        "the active session survives"
    );

    let reseeded = after["workspace"]["sessions"][1]["tabs"][0]["surface"]
        .as_u64()
        .unwrap();
    // Surface ids are allocated by the spawner and are unique for the life of
    // *one* daemon; a re-seed starts a new process, so the id in the file is
    // informational only and a fresh one is handed out.
    let mut ids: Vec<u64> = after["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_u64().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![1, 2],
        "the restarted daemon allocated both surfaces afresh"
    );
    let _ = old_surface;
    let meta = after["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == json!(reseeded))
        .unwrap();
    assert_eq!(meta["user_title"], "keep me", "a user title survives");
    assert_eq!(meta["state"]["kind"], "running");
}

#[tokio::test]
async fn a_corrupt_file_is_moved_aside_and_the_daemon_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let paths = common::paths_in(dir.path());
    let file = paths.ensure_state_dir().unwrap().join("workspace.json");
    std::fs::write(&file, b"{ this is not a workspace document").unwrap();

    let harness = Harness::start_in(dir, Config::default(), Duration::from_millis(80)).await;
    let mut client = harness.client().await;

    let bad = file.with_file_name("workspace.json.bad");
    assert!(bad.exists(), "the corrupt file is renamed to .bad");
    assert!(
        std::fs::read_to_string(&bad).unwrap().starts_with("{ this"),
        "and keeps its contents for a post-mortem"
    );

    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(
        snapshot["workspace"]["sessions"].as_array().unwrap().len(),
        1
    );
    assert_eq!(snapshot["workspace"]["sessions"][0]["name"], "Default");
}

#[tokio::test]
async fn an_unknown_schema_version_is_treated_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let paths = common::paths_in(dir.path());
    let file = paths.ensure_state_dir().unwrap().join("workspace.json");
    std::fs::write(
        &file,
        br#"{"version":7,"saved_at":"2026-01-01T00:00:00Z","next_id":9,
             "active_session":1,"sessions":[]}"#,
    )
    .unwrap();

    let harness = Harness::start_in(dir, Config::default(), Duration::from_millis(80)).await;
    let mut client = harness.client().await;

    assert!(file.with_file_name("workspace.json.bad").exists());
    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    assert_eq!(snapshot["workspace"]["sessions"][0]["name"], "Default");
}

#[tokio::test]
async fn shutdown_flushes_the_document_immediately() {
    let mut harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok(json!({ "t": "session.create", "name": "flushed" }))
        .await;

    harness.stop().await;

    let saved = harness
        .saved()
        .expect("the shutdown flush bypasses the debounce");
    assert_eq!(saved["sessions"][1]["name"], "flushed");
}

#[tokio::test]
async fn a_restart_reseeds_every_pane_of_a_split_tab() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
    let tab = snapshot["workspace"]["sessions"][0]["tabs"][0]["id"]
        .as_u64()
        .unwrap();
    let first = snapshot["workspace"]["sessions"][0]["tabs"][0]["surface"]
        .as_u64()
        .unwrap();
    let second = client
        .ok(json!({
            "t": "tab.split", "tab": tab, "pane": first, "axis": "row", "spawn": spawn_spec()
        }))
        .await["surface"]
        .as_u64()
        .unwrap();
    client
        .ok(json!({
            "t": "tab.split", "tab": tab, "pane": second, "axis": "column", "spawn": spawn_spec()
        }))
        .await;
    client
        .ok(json!({ "t": "tab.set_ratio", "tab": tab, "path": [1], "ratio": 0.25 }))
        .await;
    client
        .ok(json!({ "t": "surface.rename", "surface": second, "user_title": "middle" }))
        .await;

    assert!(
        harness
            .wait_until(SETTLE, || {
                harness.saved().is_some_and(|s| {
                    s["sessions"][0]["tabs"][0]["layout"]["second"]["ratio"] == 0.25
                })
            })
            .await
    );
    let saved = harness.saved().unwrap();
    let saved_tab = &saved["sessions"][0]["tabs"][0];
    assert_eq!(
        saved_tab["surface"]["id"],
        json!(first),
        "surface is the first leaf"
    );
    assert_eq!(saved_tab["layout"]["kind"], "split");
    assert_eq!(saved_tab["layout"]["axis"], "row");
    assert_eq!(
        saved_tab["layout"]["second"]["first"]["surface"]["user_title"],
        "middle"
    );
    client.close().await;

    let harness = harness.restart().await;
    let mut client = harness.client().await;
    let after = client.ok(json!({ "t": "workspace.get" })).await;
    let tab_doc = &after["workspace"]["sessions"][0]["tabs"][0];
    assert_eq!(tab_doc["id"], json!(tab), "tab ids survive");
    let layout = &tab_doc["layout"];
    assert_eq!(layout["kind"], "split");
    assert_eq!(layout["axis"], "row");
    assert_eq!(layout["second"]["axis"], "column");
    assert_eq!(layout["second"]["ratio"], 0.25, "ratios survive");
    assert_eq!(
        tab_doc["surface"], layout["first"]["surface"],
        "surface is still the first leaf"
    );
    assert_eq!(
        after["surfaces"].as_array().unwrap().len(),
        3,
        "every Pane was respawned"
    );
    let middle = after["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == layout["second"]["first"]["surface"])
        .unwrap();
    assert_eq!(middle["user_title"], "middle");
}
