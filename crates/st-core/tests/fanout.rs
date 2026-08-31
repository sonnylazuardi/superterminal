//! Surface + Publisher end to end: what two Clients actually receive.

mod common;

use std::time::{Duration, Instant};

use common::{fixture, row_text};
use st_core::ClientId;
use st_proto::{AttachMode, DataMsg, Seq};

const ACTIVE: ClientId = ClientId(1);
const PASSIVE: ClientId = ClientId(2);

fn snapshot_of(frames: &[st_core::ClientFrame], client: ClientId) -> &st_proto::Snapshot {
    match &frames.iter().find(|f| f.client == client).unwrap().msg {
        DataMsg::Snapshot(s) => s,
        other => panic!("expected a Snapshot for {client}, got {other:?}"),
    }
}

fn delta_of(frames: &[st_core::ClientFrame], client: ClientId) -> &st_proto::Delta {
    match &frames.iter().find(|f| f.client == client).unwrap().msg {
        DataMsg::Delta(d) => d,
        other => panic!("expected a Delta for {client}, got {other:?}"),
    }
}

#[test]
fn attach_gets_a_snapshot_then_deltas() {
    let mut s = fixture(20, 4);
    let t0 = Instant::now();
    s.feed(b"before attach");

    assert!(s.attach(ACTIVE, AttachMode::Active, t0));
    assert!(!s.attach(ACTIVE, AttachMode::Active, t0), "double attach");
    assert!(s.should_flush(t0));

    let frames = s.flush(t0);
    let snapshot = snapshot_of(&frames, ACTIVE);
    assert_eq!(snapshot.surface_id, s.id());
    assert_eq!(snapshot.grid.len(), 4);
    assert_eq!(row_text(&snapshot.grid[0]), "before attach");
    assert_eq!(snapshot.styles[0], st_proto::Style::DEFAULT);
    s.ack(ACTIVE, snapshot.seq, t0);

    let t1 = t0 + Duration::from_millis(20);
    s.feed(b"\r\nsecond line");
    let frames = s.flush(t1);
    let delta = delta_of(&frames, ACTIVE);
    assert_eq!(delta.since_seq, snapshot.seq);
    assert!(delta.rows.iter().any(|r| row_text(&r.row) == "second line"));
    assert_eq!(delta.history_len, s.history_len());
    assert_eq!(delta.history_base, s.history_base());
}

#[test]
fn passive_clients_get_metadata_but_no_rows() {
    let mut s = fixture(20, 4);
    let t0 = Instant::now();
    s.attach(ACTIVE, AttachMode::Active, t0);
    s.attach(PASSIVE, AttachMode::Passive, t0);

    let frames = s.flush(t0);
    assert_eq!(frames.len(), 2);
    let seq = snapshot_of(&frames, ACTIVE).seq;
    s.ack(ACTIVE, seq, t0);
    s.ack(PASSIVE, seq, t0);

    let t1 = t0 + Duration::from_millis(20);
    s.feed(b"\x1b]0;new title\x07text on screen");
    let frames = s.flush(t1);
    assert_eq!(frames.len(), 2);

    let active = delta_of(&frames, ACTIVE);
    assert!(!active.rows.is_empty());

    let passive = delta_of(&frames, PASSIVE);
    assert!(passive.rows.is_empty(), "Q44: no rows for a Passive attach");
    assert_eq!(passive.title.as_deref(), Some("new title"));
    assert_eq!(passive.history_len, active.history_len);
}

#[test]
fn a_client_that_never_acks_is_given_a_snapshot_and_then_reported() {
    let mut s = fixture(20, 4);
    let mut now = Instant::now();
    s.attach(ACTIVE, AttachMode::Active, now);
    let seq = snapshot_of(&s.flush(now), ACTIVE).seq;
    s.ack(ACTIVE, seq, now);

    // Four Deltas fill the ack window.
    for i in 0..4 {
        now += Duration::from_millis(10);
        s.feed(format!("\r\nline {i}").as_bytes());
        assert_eq!(s.flush(now).len(), 1, "delta {i}");
    }

    // Now the window is full: nothing goes out, but nothing is lost either.
    now += Duration::from_millis(10);
    s.feed(b"\r\nwhile blocked");
    assert!(s.flush(now).is_empty(), "the ack window blocks");

    // After 3 s of being blocked the Client is resynced with a Snapshot.
    now += Duration::from_secs(4);
    s.feed(b" more");
    let frames = s.flush(now);
    let snapshot = snapshot_of(&frames, ACTIVE);
    assert!(
        snapshot
            .grid
            .iter()
            .any(|r| row_text(r).contains("while blocked more")),
        "the Snapshot carries the coalesced state"
    );

    // Still silent 30 s later ⇒ the Server is told to drop the connection.
    let much_later = now + Duration::from_secs(31);
    assert_eq!(s.publisher().silent_clients(much_later), vec![ACTIVE]);
}

#[test]
fn detaching_stops_the_frames() {
    let mut s = fixture(20, 4);
    let t0 = Instant::now();
    s.attach(ACTIVE, AttachMode::Active, t0);
    s.flush(t0);
    assert!(s.detach(ACTIVE));

    s.feed(b"nobody is listening");
    assert!(s.flush(t0 + Duration::from_millis(20)).is_empty());
    assert_eq!(s.publisher().len(), 0);
}

#[test]
fn a_resize_reaches_every_client_with_the_new_size() {
    let mut s = fixture(20, 4);
    let t0 = Instant::now();
    s.attach(ACTIVE, AttachMode::Active, t0);
    s.attach(PASSIVE, AttachMode::Passive, t0);
    let seq = snapshot_of(&s.flush(t0), ACTIVE).seq;
    s.ack(ACTIVE, seq, t0);
    s.ack(PASSIVE, seq, t0);

    s.resize(30, 8).unwrap();
    let frames = s.flush(t0 + Duration::from_millis(20));
    assert_eq!(delta_of(&frames, ACTIVE).resized, Some((30, 8)));
    assert_eq!(delta_of(&frames, ACTIVE).rows.len(), 8);
    assert_eq!(delta_of(&frames, PASSIVE).resized, Some((30, 8)));
    assert!(delta_of(&frames, PASSIVE).rows.is_empty());
}

#[test]
fn every_frame_round_trips_through_the_wire_codec() {
    let mut s = fixture(24, 5);
    let t0 = Instant::now();
    s.attach(ACTIVE, AttachMode::Active, t0);
    s.feed(b"\x1b[1;31mcolour\x1b[0m\r\n\x07plain");

    let mut all = s.flush(t0);
    s.ack(ACTIVE, s.seq(), t0);
    s.feed(b"\r\nmore");
    all.extend(s.flush(t0 + Duration::from_millis(20)));
    assert!(all.len() >= 2);

    for frame in &all {
        let payload = frame.msg.to_payload().expect("encodes");
        let decoded = DataMsg::from_frame(frame.msg.msg_type(), &payload).expect("decodes");
        assert_eq!(decoded, frame.msg);
    }
}

#[test]
fn acks_beyond_what_was_sent_are_ignored() {
    let mut s = fixture(10, 2);
    let t0 = Instant::now();
    s.attach(ACTIVE, AttachMode::Active, t0);
    s.flush(t0);
    s.ack(ACTIVE, Seq::new(9_999), t0);
    let sub = s.publisher().subscription(ACTIVE).unwrap();
    assert_eq!(sub.last_acked_seq(), sub.last_sent_seq());
}
