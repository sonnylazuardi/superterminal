//! Round-trip tests for both planes.
//!
//! * Every data-plane message survives postcard → frame → postcard.
//! * Every control-plane message survives a JSON NDJSON line.
//! * The two "every" claims are checked for real: the strategies are sampled
//!   until every `msg_type` and every request tag has been seen.

use std::collections::BTreeSet;

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, TestRunner};
use st_proto::control::*;
use st_proto::data::*;
use st_proto::*;

mod strategies;
use strategies as s;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// postcard → payload → postcard, keyed by the frame's `msg_type`.
    #[test]
    fn data_message_survives_postcard(msg in s::data_msg()) {
        let payload = msg.to_payload().expect("encode");
        let back = DataMsg::from_frame(msg.msg_type(), &payload).expect("decode");
        prop_assert_eq!(back, msg);
    }

    /// The same, but through the framing layer, several messages at a time.
    #[test]
    fn data_messages_survive_framing(msgs in proptest::collection::vec(s::data_msg(), 1..8)) {
        let mut wire = Vec::new();
        for msg in &msgs {
            msg.encode_to(&mut wire).expect("encode");
        }
        let mut decoder = FrameDecoder::new();
        decoder.push(&wire);
        for expected in &msgs {
            let frame = decoder.next_frame().expect("no framing error").expect("a frame");
            let got = DataMsg::from_frame(frame.msg_type, &frame.payload).expect("decode");
            prop_assert_eq!(&got, expected);
        }
        prop_assert!(decoder.next_frame().unwrap().is_none());
    }

    /// A decoded payload re-encodes to exactly the same bytes: postcard is
    /// canonical, which is what makes the golden fixtures of §10 meaningful.
    #[test]
    fn data_encoding_is_canonical(msg in s::data_msg()) {
        let first = msg.to_payload().expect("encode");
        let decoded = DataMsg::from_frame(msg.msg_type(), &first).expect("decode");
        prop_assert_eq!(decoded.to_payload().expect("re-encode"), first);
    }

    #[test]
    fn request_survives_json(req in s::req()) {
        let line = serde_json::to_string(&req).expect("encode");
        prop_assert!(!line.contains('\n'), "NDJSON lines must not embed newlines");
        let back: Req = serde_json::from_str(&line).expect("decode");
        prop_assert_eq!(back, req);
    }

    #[test]
    fn event_survives_json(ev in s::ev()) {
        let line = serde_json::to_string(&ev).expect("encode");
        let back: Ev = serde_json::from_str(&line).expect("decode");
        prop_assert_eq!(back, ev);
    }

    #[test]
    fn ok_response_survives_json(id in any::<u32>(), rev in any::<u64>()) {
        let res: Res<RevisionResult> = Res::Ok { id, result: RevisionResult { revision: rev } };
        let line = serde_json::to_string(&res).expect("encode");
        prop_assert_eq!(serde_json::from_str::<Res<RevisionResult>>(&line).expect("decode"), res);
    }

    #[test]
    fn err_response_survives_json(id in any::<u32>(), error in s::error_body()) {
        let res: AnyRes = Res::Err { id, error };
        let line = serde_json::to_string(&res).expect("encode");
        prop_assert_eq!(serde_json::from_str::<AnyRes>(&line).expect("decode"), res);
    }

    /// Any control message parses back through the untagged `ControlMsg` union
    /// as the same variant.
    #[test]
    fn control_union_survives_json(req in s::req(), ev in s::ev()) {
        for msg in [ControlMsg::Req(Box::new(req)), ControlMsg::Ev(Box::new(ev))] {
            let line = msg.to_line().expect("encode");
            prop_assert_eq!(ControlMsg::from_line(&line).expect("decode"), msg);
        }
    }
}

/// Samples the data strategy until every `msg_type` of §4.1 has been produced,
/// so "every data message round-trips" is a claim about all sixteen of them.
#[test]
fn the_strategy_covers_every_msg_type() {
    let expected: BTreeSet<u16> = [
        msg_type::HELLO,
        msg_type::HELLO_ACK,
        msg_type::REJECT,
        msg_type::ATTACH,
        msg_type::DETACH,
        msg_type::INPUT,
        msg_type::RESIZE,
        msg_type::FETCH_HISTORY,
        msg_type::ACK,
        msg_type::SNAPSHOT,
        msg_type::DELTA,
        msg_type::HISTORY,
        msg_type::SURFACE_EXITED,
        msg_type::BELL,
        msg_type::DETACHED,
        msg_type::DATA_ERROR,
    ]
    .into_iter()
    .collect();

    let mut runner = TestRunner::new(Config::default());
    let strategy = s::data_msg();
    let mut seen = BTreeSet::new();
    for _ in 0..2000 {
        let msg = strategy.new_tree(&mut runner).unwrap().current();
        // Round-trip each sample too — cheap, and it makes the coverage claim
        // and the round-trip claim the same test data.
        let payload = msg.to_payload().unwrap();
        assert_eq!(
            DataMsg::from_frame(msg.msg_type(), &payload).unwrap(),
            msg,
            "round trip failed for msg_type 0x{:04X}",
            msg.msg_type()
        );
        seen.insert(msg.msg_type());
        if seen == expected {
            return;
        }
    }
    panic!("missing msg_types: {:?}", &expected - &seen);
}

/// The same, for the eighteen requests of §3.3.
#[test]
fn the_strategy_covers_every_request() {
    let expected: BTreeSet<&str> = [
        "workspace.get",
        "workspace.subscribe",
        "session.create",
        "session.rename",
        "session.delete",
        "session.list",
        "session.set_active",
        "tab.create",
        "tab.close",
        "tab.reorder",
        "tab.move",
        "tab.set_active",
        "surface.create",
        "surface.kill",
        "surface.rename",
        "view.set",
        "server.status",
        "server.shutdown",
    ]
    .into_iter()
    .collect();

    let mut runner = TestRunner::new(Config::default());
    let strategy = s::req();
    let mut seen = BTreeSet::new();
    for _ in 0..2000 {
        let req = strategy.new_tree(&mut runner).unwrap().current();
        let line = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<Req>(&line).unwrap(), req);
        seen.insert(req.tag());
        if seen == expected {
            return;
        }
    }
    panic!("missing request tags: {:?}", &expected - &seen);
}

/// A `Snapshot` of a realistic 200×60 dense grid stays far below the frame cap
/// and inside the §11 budget.
#[test]
fn dense_snapshot_size_budget() {
    let mut styles = vec![Style::DEFAULT];
    for i in 1..200u16 {
        styles.push(Style {
            fg: Color::Indexed((i % 255) as u8),
            bg: Color::Default,
            underline_color: Color::Default,
            attrs: Attrs::BOLD,
        });
    }
    let grid: Vec<Row> = (0..60)
        .map(|r| Row {
            cells: (0..200)
                .map(|c| {
                    PackedCell::from_char(
                        (b'a' + ((r + c) % 26) as u8) as char,
                        StyleIdx::new((c % 120) as u16),
                    )
                })
                .collect(),
            extras: Vec::new(),
            wrapped: false,
        })
        .collect();
    let snapshot = Snapshot {
        surface_id: SurfaceId(1),
        seq: Seq(1),
        cols: 200,
        rows: 60,
        styles,
        grid,
        cursor: Cursor::default(),
        modes: Modes::LINE_WRAP,
        title: "btop".into(),
        history_base: AbsLine(0),
        history_len: 0,
        view_state: ViewState::default(),
        exited: None,
    };
    let bytes = postcard::to_stdvec(&snapshot).unwrap();
    assert!(
        (30_000..50_000).contains(&bytes.len()),
        "dense snapshot was {} bytes, §11 predicts ≈39 KB",
        bytes.len()
    );
    assert!(bytes.len() < MAX_FRAME);
}
