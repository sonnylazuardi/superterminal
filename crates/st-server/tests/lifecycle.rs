//! Process lifecycle end to end — `docs/plan/03-server.md` §2, grilling
//! Q30/Q37/Q42.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{spawn_spec, Harness};
use serde_json::json;
use st_config::Config;
use st_proto::DATA_MAGIC;
use st_server::lifecycle::{LockError, ServerBuilder};
use st_server::workspace::NullSpawner;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

fn config_with_idle(minutes: f64) -> Config {
    let mut config = Config::default();
    config.server.idle_exit_minutes = minutes;
    config
}

#[tokio::test]
async fn a_second_daemon_cannot_take_the_lock() {
    let harness = Harness::start().await;

    let error = ServerBuilder::new(harness.paths.clone(), Config::default())
        .spawner(Arc::new(NullSpawner::new()))
        .start()
        .await
        .expect_err("the second start must fail");

    match error.downcast_ref::<LockError>() {
        Some(LockError::AlreadyRunning { pid, path }) => {
            assert_eq!(*pid, Some(std::process::id()));
            assert_eq!(path, &harness.paths.lock_path());
        }
        other => panic!("expected AlreadyRunning, got {other:?}"),
    }

    // The first daemon is untouched.
    let mut client = harness.client().await;
    client.ok(json!({ "t": "workspace.get" })).await;
}

#[tokio::test]
async fn the_lock_is_released_when_the_daemon_stops() {
    let mut harness = Harness::start().await;
    harness.stop().await;

    let second = ServerBuilder::new(harness.paths.clone(), Config::default())
        .spawner(Arc::new(NullSpawner::new()))
        .start()
        .await
        .expect("the lock is free again");
    second.stop("done").await.unwrap();
}

#[tokio::test]
async fn an_idle_daemon_with_only_pristine_surfaces_exits() {
    // 0.05 minutes = 3 s.
    let mut harness = Harness::start_with(config_with_idle(0.05)).await;
    let server = harness.server.take().expect("running");

    let reason = tokio::time::timeout(Duration::from_secs(20), server.wait())
        .await
        .expect("the idle timer fires (grilling Q42: pristine surfaces count as zero)")
        .expect("a clean shutdown");

    assert!(reason.contains("idle"), "unexpected reason: {reason}");
    assert!(!harness.socket().exists());
}

#[tokio::test]
async fn a_non_pristine_surface_keeps_the_daemon_alive() {
    let mut harness = Harness::start_with(config_with_idle(0.05)).await;
    {
        // A Surface a client asked for is not pristine, so it counts.
        let mut client = harness.client().await;
        let snapshot = client.ok(json!({ "t": "workspace.get" })).await;
        let session = snapshot["workspace"]["sessions"][0]["id"].as_u64().unwrap();
        client
            .ok(json!({ "t": "tab.create", "session": session, "spawn": spawn_spec() }))
            .await;
        client.close().await;
    }

    let server = harness.server.take().expect("running");
    let outcome = tokio::time::timeout(Duration::from_secs(8), server.wait()).await;
    assert!(
        outcome.is_err(),
        "the daemon must stay up while real work is running"
    );
}

#[tokio::test]
async fn input_to_a_seeded_surface_ends_its_pristineness() {
    let harness = Harness::start_with(config_with_idle(0.05)).await;
    let workspace = harness.server.as_ref().unwrap().workspace().clone();

    let before = workspace.stats().await.unwrap();
    assert_eq!(before.live_surfaces, 1);
    assert_eq!(before.busy_surfaces, 0, "the seeded shell is pristine");

    workspace
        .surface_event(st_server::workspace::SurfaceEvent::Input {
            surface: st_proto::SurfaceId(1),
        })
        .await
        .unwrap();

    let after = workspace.stats().await.unwrap();
    assert_eq!(after.busy_surfaces, 1, "grilling Q42");
}

#[tokio::test]
async fn a_connection_opening_with_the_data_magic_is_not_control() {
    let harness = Harness::start().await;

    let mut stream = UnixStream::connect(harness.socket()).await.unwrap();
    stream.write_all(&DATA_MAGIC).await.unwrap();
    stream.write_all(&[0x08, 0x00, 0x00, 0x00]).await.unwrap();
    stream.flush().await.unwrap();

    // Whatever the data plane does with it, it is never answered with NDJSON.
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
    )
    .await;
    if let Ok(Ok(n)) = read {
        assert!(
            n == 0 || buf[0] != b'{',
            "a DATA connection must never receive a control-plane line: {:?}",
            &buf[..n]
        );
    }

    // A control connection on the same socket still works.
    let mut client = harness.client().await;
    client.ok(json!({ "t": "workspace.get" })).await;
}

#[tokio::test]
async fn a_connection_with_neither_marker_is_closed() {
    let harness = Harness::start().await;

    let mut stream = UnixStream::connect(harness.socket()).await.unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();

    let mut buf = [0u8; 32];
    let read = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
    )
    .await
    .expect("the server closes the connection promptly");
    match read {
        // A clean EOF, or an RST because the unread request bytes were still
        // in flight when the server closed: either way, nothing was answered.
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("the server must say nothing and hang up, got {other:?}"),
    }
}

#[tokio::test]
async fn a_client_that_never_says_hello_is_dropped_after_the_timeout() {
    let harness = Harness::start().await;

    // A CONTROL connection (first byte `{`) that then goes silent must be
    // closed after the 5 s handshake budget (§2 rule 1).
    let mut stream = UnixStream::connect(harness.socket()).await.unwrap();
    stream.write_all(b"{").await.unwrap();

    let mut buf = [0u8; 16];
    let read = tokio::time::timeout(
        Duration::from_secs(12),
        tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
    )
    .await
    .expect("the server gives up on the handshake");
    assert!(
        matches!(read, Ok(0))
            || matches!(&read, Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset),
        "expected the socket to be closed, got {read:?}"
    );

    let mut good = harness.client().await;
    good.ok(json!({ "t": "workspace.get" })).await;
}
