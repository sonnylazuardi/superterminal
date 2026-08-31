//! Framing-layer tests: arbitrary chunk boundaries, the magic, and the
//! [`MAX_FRAME`] cap (`docs/plan/02-protocol.md` §1.2–§1.3).

use proptest::prelude::*;
use st_proto::data::msg_type;
use st_proto::*;

mod strategies;
use strategies as s;

/// Feeds `wire` to a decoder in the given chunk sizes and returns every frame.
fn decode_in_chunks(wire: &[u8], chunks: &[usize]) -> Vec<Frame> {
    let mut decoder = FrameDecoder::new();
    let mut out = Vec::new();
    let mut offset = 0;
    let mut i = 0;
    while offset < wire.len() {
        let take = chunks[i % chunks.len()].clamp(1, wire.len() - offset);
        decoder.push(&wire[offset..offset + take]);
        offset += take;
        i += 1;
        while let Some(frame) = decoder.next_frame().expect("no framing error") {
            out.push(frame);
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Arbitrary frames survive being split at arbitrary chunk boundaries.
    #[test]
    fn arbitrary_chunking(
        frames in proptest::collection::vec(
            (any::<u16>(), proptest::collection::vec(any::<u8>(), 0..300)),
            1..12,
        ),
        chunks in proptest::collection::vec(1usize..64, 1..8),
    ) {
        let mut wire = Vec::new();
        for (msg_type, payload) in &frames {
            encode_frame(*msg_type, payload, &mut wire).expect("encode");
        }
        let decoded = decode_in_chunks(&wire, &chunks);
        prop_assert_eq!(decoded.len(), frames.len());
        for (got, (msg_type, payload)) in decoded.iter().zip(&frames) {
            prop_assert_eq!(got.msg_type, *msg_type);
            prop_assert_eq!(&got.payload, payload);
        }
    }

    /// Byte-by-byte feeding never loses or duplicates a frame.
    #[test]
    fn byte_by_byte(msgs in proptest::collection::vec(s::data_msg(), 1..6)) {
        let mut wire = Vec::new();
        for msg in &msgs {
            msg.encode_to(&mut wire).expect("encode");
        }
        let mut decoder = FrameDecoder::new();
        let mut decoded = Vec::new();
        for byte in &wire {
            decoder.push(std::slice::from_ref(byte));
            while let Some(frame) = decoder.next_frame().expect("no framing error") {
                decoded.push(DataMsg::from_frame(frame.msg_type, &frame.payload).expect("decode"));
            }
        }
        prop_assert_eq!(decoded, msgs);
    }

    /// A DATA stream that starts with the magic decodes the same way, however
    /// the magic itself is split.
    #[test]
    fn magic_across_chunk_boundaries(split in 0usize..=4) {
        let mut wire = DATA_MAGIC.to_vec();
        encode_frame(msg_type::BELL, &[7], &mut wire).expect("encode");

        let mut decoder = FrameDecoder::expecting_magic();
        decoder.push(&wire[..split]);
        prop_assert!(decoder.next_frame().expect("no framing error").is_none());
        decoder.push(&wire[split..]);
        let frame = decoder.next_frame().expect("no framing error").expect("a frame");
        prop_assert_eq!(frame.msg_type, msg_type::BELL);
        prop_assert_eq!(frame.payload, vec![7]);
    }

    /// Any first byte other than `{` or `0xFF` is refused outright (§1.2).
    #[test]
    fn only_two_connection_kinds(byte in any::<u8>()) {
        let kind = detect_connection_kind(byte);
        match byte {
            CONTROL_FIRST_BYTE => prop_assert_eq!(kind, Some(ConnectionKind::Control)),
            0xFF => prop_assert_eq!(kind, Some(ConnectionKind::Data)),
            _ => prop_assert_eq!(kind, None),
        }
    }
}

#[test]
fn frames_at_and_above_the_cap() {
    // Exactly at the cap: accepted.
    let payload = vec![0u8; MAX_PAYLOAD];
    let mut wire = Vec::new();
    encode_frame(msg_type::SNAPSHOT, &payload, &mut wire).unwrap();
    assert_eq!(wire.len(), MAX_FRAME + 4);
    let mut decoder = FrameDecoder::new();
    decoder.push(&wire);
    assert_eq!(
        decoder.next_frame().unwrap().unwrap().payload.len(),
        MAX_PAYLOAD
    );

    // One byte over: the encoder refuses …
    let err = encode_frame(
        msg_type::SNAPSHOT,
        &vec![0u8; MAX_PAYLOAD + 1],
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(matches!(err, FrameError::PayloadTooLarge { .. }));

    // … and so does the decoder, from the header alone, before buffering the
    // body — which is what lets the server close the connection immediately.
    let mut header = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
    header.extend_from_slice(&msg_type::SNAPSHOT.to_le_bytes());
    let mut decoder = FrameDecoder::new();
    decoder.push(&header);
    assert_eq!(
        decoder.next_frame(),
        Err(FrameError::FrameTooLarge {
            len: MAX_FRAME + 1,
            max: MAX_FRAME
        })
    );
    assert!(decoder.is_poisoned());
    assert_eq!(decoder.next_frame(), Err(FrameError::Poisoned));
    assert_eq!(
        FrameError::FrameTooLarge {
            len: MAX_FRAME + 1,
            max: MAX_FRAME
        }
        .reject_reason(),
        RejectReason::FrameTooLarge
    );
}

#[test]
fn a_control_line_pretending_to_be_data_fails_on_the_magic() {
    let mut decoder = FrameDecoder::expecting_magic();
    decoder.push(b"{\"t\":\"hello\"}\n");
    assert_eq!(decoder.next_frame(), Err(FrameError::BadMagic));
    assert_eq!(FrameError::BadMagic.reject_reason(), RejectReason::BadMagic);
}

#[test]
fn handshake_travels_as_frames_on_the_data_plane() {
    let hello = DataMsg::Hello(Hello {
        proto_version: PROTO_VERSION,
        client_kind: ClientKind::Data,
        build_id: "abc".into(),
    });
    let mut wire = DATA_MAGIC.to_vec();
    hello.encode_to(&mut wire).unwrap();

    let mut decoder = FrameDecoder::expecting_magic();
    decoder.push(&wire);
    let frame = decoder.next_frame().unwrap().unwrap();
    assert_eq!(frame.msg_type, msg_type::HELLO);
    assert_eq!(
        DataMsg::from_frame(frame.msg_type, &frame.payload).unwrap(),
        hello
    );
}
