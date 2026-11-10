//! Synthetic conformance cases;
//! upstream provenance is recorded in ../README.md.

use garmin_gfdi::{FrameDecoder, FrameError, Message, MessageError, encode_frame, encode_message};

fn feed(decoder: &mut FrameDecoder, bytes: &[u8]) -> Vec<Result<Vec<u8>, FrameError>> {
    bytes
        .iter()
        .filter_map(|&byte| decoder.push(byte))
        .collect()
}

#[test]
fn garmin_delimiters_and_zero_runs_match_known_encodings() {
    for (payload, expected) in [
        (vec![], vec![0, 1, 0]),
        (vec![1], vec![0, 2, 1, 0]),
        (vec![0], vec![0, 1, 1, 0]),
        (vec![1, 0, 0], vec![0, 2, 1, 1, 1, 0]),
        (vec![0, 1, 2, 0], vec![0, 1, 3, 1, 2, 1, 0]),
    ] {
        assert_eq!(encode_frame(&payload), expected);
        assert_eq!(
            feed(&mut FrameDecoder::new(16), &expected),
            vec![Ok(payload)]
        );
    }
}

#[test]
fn full_nonzero_blocks_retain_garmins_terminal_empty_block() {
    for blocks in [1, 2, 3] {
        let payload = vec![0x55; 254 * blocks];
        let mut expected = vec![0];
        for _ in 0..blocks {
            expected.push(255);
            expected.extend([0x55; 254]);
        }
        expected.extend([1, 0]);
        assert_eq!(encode_frame(&payload), expected);
        assert_eq!(
            feed(&mut FrameDecoder::new(1024), &expected),
            vec![Ok(payload)]
        );
    }
}

#[test]
fn chunk_boundaries_do_not_change_decoding() {
    for length in [0, 1, 2, 253, 254, 255, 256, 508, 509, 4096] {
        for payload in [
            vec![0; length],
            vec![0x55; length],
            (0..=u8::MAX).cycle().take(length).collect(),
        ] {
            let frame = encode_frame(&payload);
            for chunk_size in [1, 20, 229, 514] {
                let mut decoder = FrameDecoder::new(frame.len() - 2);
                let actual: Vec<_> = frame
                    .chunks(chunk_size)
                    .flat_map(|chunk| feed(&mut decoder, chunk))
                    .collect();
                assert_eq!(
                    actual,
                    vec![Ok(payload.clone())],
                    "length={length}, chunk={chunk_size}"
                );
            }
            for split in 0..=frame.len().min(260) {
                let mut decoder = FrameDecoder::new(frame.len() - 2);
                let mut actual = feed(&mut decoder, &frame[..split]);
                actual.extend(feed(&mut decoder, &frame[split..]));
                assert_eq!(
                    actual,
                    vec![Ok(payload.clone())],
                    "length={length}, split={split}"
                );
            }
        }
    }
}

#[test]
fn coalesced_frames_are_not_lost_or_overwritten() {
    let mut bytes = encode_frame(&[1, 0, 2]);
    bytes.extend(encode_frame(&[3, 4]));
    bytes.extend(encode_frame(&[]));
    assert_eq!(
        feed(&mut FrameDecoder::new(8), &bytes),
        vec![Ok(vec![1, 0, 2]), Ok(vec![3, 4]), Ok(vec![])]
    );
}

#[test]
fn truncated_frame_does_not_wedge_following_frames() {
    let mut decoder = FrameDecoder::new(16);
    assert_eq!(feed(&mut decoder, &[0, 5, 1, 2]), Vec::new());
    assert_eq!(feed(&mut decoder, &[0]), vec![Err(FrameError::Malformed)]);
    assert_eq!(
        feed(&mut decoder, &encode_frame(&[7, 0, 8])),
        vec![Ok(vec![7, 0, 8])]
    );
}

#[test]
fn lost_middle_chunk_recovers_at_next_frame_start() {
    let mut bytes = vec![0, 255, 0x55, 0x55];
    bytes.extend(encode_frame(&[7, 8, 9]));
    assert_eq!(
        feed(&mut FrameDecoder::new(32), &bytes),
        vec![Err(FrameError::Malformed), Ok(vec![7, 8, 9])]
    );
}

#[test]
fn noise_and_duplicate_delimiters_do_not_emit_messages() {
    let mut decoder = FrameDecoder::new(16);
    assert_eq!(feed(&mut decoder, &[99, 88, 77, 0, 0, 0]), Vec::new());
    assert_eq!(
        feed(&mut decoder, &[2, 7, 0, 2, 8, 0]),
        vec![Ok(vec![7]), Ok(vec![8])]
    );
}

#[test]
fn overflow_is_reported_once_and_next_frame_survives() {
    let mut decoder = FrameDecoder::new(3);
    assert_eq!(feed(&mut decoder, &[0, 5, 1, 2]), Vec::new());
    assert_eq!(
        feed(&mut decoder, &[3]),
        vec![Err(FrameError::TooLong { limit: 3 })]
    );
    assert_eq!(feed(&mut decoder, &[4, 5, 6, 7]), Vec::new());
    assert_eq!(feed(&mut decoder, &encode_frame(&[9])), vec![Ok(vec![9])]);
}

#[test]
fn zero_limit_rejects_every_body_without_panicking() {
    assert_eq!(
        feed(&mut FrameDecoder::new(0), &[0, 1, 0]),
        vec![Err(FrameError::TooLong { limit: 0 })]
    );
}

#[test]
fn disconnect_reset_discards_partial_input() {
    let mut decoder = FrameDecoder::new(16);
    assert_eq!(feed(&mut decoder, &[0, 3, 1]), Vec::new());
    decoder.reset();
    assert_eq!(feed(&mut decoder, &[2, 0]), Vec::new());
    assert_eq!(feed(&mut decoder, &encode_frame(&[9])), vec![Ok(vec![9])]);
}

#[test]
fn known_upstream_status_frame_checks_length_crc_and_payload() {
    let wire = hex::decode("11008813b41300370300000000000037e4").unwrap();
    let message = Message::decode(&wire).unwrap();
    assert_eq!(message.raw_type(), 5000);
    assert_eq!(message.message_id(), 5000);
    assert_eq!(message.sequence(), None);
    assert_eq!(
        message.payload(),
        hex::decode("b413003703000000000000").unwrap()
    );
    assert_eq!(
        encode_message(message.raw_type(), message.payload()).unwrap(),
        wire
    );
}

#[test]
fn sequenced_and_unknown_type_fields_are_preserved() {
    for (raw_type, message_id, sequence) in [
        (0xb788, 5136, Some(55)),
        (0xffff, 5255, Some(127)),
        (1234, 1234, None),
    ] {
        let bytes = encode_message(raw_type, &[0, 1, 0]).unwrap();
        let message = Message::decode(&bytes).unwrap();
        assert_eq!(message.raw_type(), raw_type);
        assert_eq!(message.message_id(), message_id);
        assert_eq!(message.sequence(), sequence);
        assert_eq!(message.payload(), &[0, 1, 0]);
    }
}

#[test]
fn invalid_envelopes_are_rejected_before_dispatch() {
    for length in 0..6 {
        assert_eq!(
            Message::decode(&vec![0; length]),
            Err(MessageError::TooShort)
        );
    }
    let valid = encode_message(5000, &[1, 2, 3]).unwrap();
    let mut wrong_length = valid.clone();
    wrong_length[0] -= 1;
    assert_eq!(
        Message::decode(&wrong_length),
        Err(MessageError::LengthMismatch {
            declared: 8,
            actual: 9
        })
    );
    let mut appended = valid.clone();
    appended.push(0);
    assert_eq!(
        Message::decode(&appended),
        Err(MessageError::LengthMismatch {
            declared: 9,
            actual: 10
        })
    );
    for index in 2..valid.len() {
        let mut corrupted = valid.clone();
        corrupted[index] ^= 1;
        assert_eq!(Message::decode(&corrupted), Err(MessageError::Checksum));
    }
}

#[test]
fn maximum_message_length_is_explicit() {
    let payload = vec![0x55; usize::from(u16::MAX) - 6];
    let wire = encode_message(5000, &payload).unwrap();
    assert_eq!(wire.len(), usize::from(u16::MAX));
    assert_eq!(Message::decode(&wire).unwrap().payload(), payload);
    assert_eq!(
        encode_message(5000, &vec![0; payload.len() + 1]),
        Err(MessageError::TooLong)
    );
}

#[test]
fn framed_messages_survive_fragmented_delivery() {
    let message = encode_message(5000, &[0, 1, 0, 2]).unwrap();
    let mut decoder = FrameDecoder::new(64);
    let frames: Vec<_> = encode_frame(&message)
        .chunks(3)
        .flat_map(|chunk| feed(&mut decoder, chunk))
        .collect();
    assert_eq!(frames, vec![Ok(message.clone())]);
    assert_eq!(Message::decode(&message).unwrap().payload(), &[0, 1, 0, 2]);
}
