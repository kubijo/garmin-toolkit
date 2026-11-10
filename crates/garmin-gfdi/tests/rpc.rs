use std::time::Duration;

use garmin_gfdi::rpc::{RpcError, RpcReassembly, encode_rpc_request};
use garmin_gfdi::{Message, encode_message};

const NOW: Duration = Duration::ZERO;
const DEADLINE: Duration = Duration::from_secs(1);

fn fragment(kind: u16, request: u16, offset: u32, total: u32, data: &[u8]) -> Vec<u8> {
    let mut payload = request.to_le_bytes().to_vec();
    payload.extend(offset.to_le_bytes());
    payload.extend(total.to_le_bytes());
    payload.extend(u32::try_from(data.len()).unwrap().to_le_bytes());
    payload.extend(data);
    encode_message(kind, &payload).unwrap()
}

#[test]
fn complete_request_and_short_ack_have_independent_wire_layouts() {
    let request = encode_rpc_request(0x1234, &[1, 2], 2).unwrap();
    let message = Message::decode(&request).unwrap();
    assert_eq!(message.message_id(), 5043);
    assert_eq!(
        message.payload(),
        hex::decode("34120000000002000000020000000102").unwrap()
    );
    let mut assembly = RpcReassembly::new(2, DEADLINE);
    let result = assembly.receive(message, NOW).unwrap();
    assert_eq!(result.request_id, 0x1234);
    assert_eq!(result.complete, Some(vec![1, 2]));
    let acknowledgement = Message::decode(&result.acknowledgement).unwrap();
    assert_eq!(acknowledgement.raw_type(), 5000);
    assert_eq!(acknowledgement.payload(), [0xb3, 0x13, 0]);
    assert!(matches!(
        assembly.receive(message, NOW),
        Err(RpcError::Stopped)
    ));
}

#[test]
fn fragmented_response_preserves_ack_sequence_and_accepts_exact_replays() {
    let mut assembly = RpcReassembly::new(4, DEADLINE);
    let first = fragment(0x852c, 9, 0, 4, &[1, 2]); // Sequenced message 5044, sequence 5.
    let first_message = Message::decode(&first).unwrap();
    let accepted = assembly.receive(first_message, NOW).unwrap();
    assert_eq!(accepted.complete, None);
    let acknowledgement = Message::decode(&accepted.acknowledgement).unwrap();
    assert_eq!(acknowledgement.raw_type(), 0x8500);
    assert_eq!(
        acknowledgement.payload(),
        [0xb4, 0x13, 0, 9, 0, 0, 0, 0, 0, 0, 0]
    );
    let replay = assembly.receive(first_message, NOW).unwrap();
    assert_eq!(replay.complete, None);
    assert_eq!(replay.acknowledgement, accepted.acknowledgement);
    let final_chunk = fragment(5044, 9, 2, 4, &[3, 4]);
    let result = assembly
        .receive(Message::decode(&final_chunk).unwrap(), NOW)
        .unwrap();
    assert_eq!(result.complete, Some(vec![1, 2, 3, 4]));
    let acknowledgement = Message::decode(&result.acknowledgement).unwrap();
    assert_eq!(
        acknowledgement.payload(),
        [0xb4, 0x13, 0, 9, 0, 2, 0, 0, 0, 0, 0]
    );
}

#[test]
fn identity_size_offset_and_replay_conflicts_fail_closed() {
    for (kind, request, offset, total, data, expected) in [
        (5044, 10, 2, 4, vec![3, 4], "correlation"),
        (5043, 9, 2, 4, vec![3, 4], "correlation"),
        (5044, 9, 2, 5, vec![3, 4], "correlation"),
        (5044, 9, 3, 4, vec![3], "offset"),
        (5044, 9, 0, 4, vec![1, 9], "offset"),
    ] {
        let mut assembly = RpcReassembly::new(10, DEADLINE);
        let first = fragment(5044, 9, 0, 4, &[1, 2]);
        assembly
            .receive(Message::decode(&first).unwrap(), NOW)
            .unwrap();
        let bytes = fragment(kind, request, offset, total, &data);
        let error = assembly
            .receive(Message::decode(&bytes).unwrap(), NOW)
            .unwrap_err();
        match expected {
            "correlation" => assert!(matches!(error, RpcError::Correlation)),
            _ => assert!(matches!(error, RpcError::Offset)),
        }
        assert!(matches!(
            assembly.receive(Message::decode(&first).unwrap(), NOW),
            Err(RpcError::Stopped)
        ));
    }
}

#[test]
fn malformed_fragments_limits_and_timeouts_never_produce_acks() {
    let mut cases = vec![
        encode_message(5024, &[0; 14]).unwrap(),
        encode_message(5044, &[0; 13]).unwrap(),
        fragment(5044, 1, 4, 3, &[1]),
        fragment(5044, 1, 2, 3, &[1, 2]),
        fragment(5044, 1, 0, 1, &[]),
    ];
    let mut payload = vec![0; 14];
    payload[10] = 1;
    cases.push(encode_message(5044, &payload).unwrap());
    for bytes in cases {
        assert!(matches!(
            RpcReassembly::new(100, DEADLINE).receive(Message::decode(&bytes).unwrap(), NOW),
            Err(RpcError::Fragment)
        ));
    }
    let bytes = fragment(5044, 1, 0, 11, &[1]);
    assert!(matches!(
        RpcReassembly::new(10, DEADLINE).receive(Message::decode(&bytes).unwrap(), NOW),
        Err(RpcError::Limit)
    ));
    let bytes = fragment(5044, 1, 1, 2, &[1]);
    assert!(matches!(
        RpcReassembly::new(10, DEADLINE).receive(Message::decode(&bytes).unwrap(), NOW),
        Err(RpcError::Offset)
    ));
    let bytes = fragment(5044, 1, 0, 1, &[1]);
    assert!(matches!(
        RpcReassembly::new(10, DEADLINE).receive(Message::decode(&bytes).unwrap(), DEADLINE),
        Err(RpcError::Timeout)
    ));
    assert!(matches!(
        encode_rpc_request(1, &[1], 0),
        Err(RpcError::Limit)
    ));
    assert!(matches!(
        encode_rpc_request(1, &vec![0; 65536], usize::MAX),
        Err(RpcError::Message(_))
    ));
}

#[test]
fn empty_payload_and_explicit_cancellation_are_terminal() {
    let bytes = fragment(5044, 0, 0, 0, &[]);
    let mut assembly = RpcReassembly::new(0, DEADLINE);
    assert_eq!(
        assembly
            .receive(Message::decode(&bytes).unwrap(), NOW)
            .unwrap()
            .complete,
        Some(vec![])
    );
    let mut assembly = RpcReassembly::new(10, DEADLINE);
    assembly.cancel();
    assert!(matches!(
        assembly.receive(Message::decode(&bytes).unwrap(), NOW),
        Err(RpcError::Stopped)
    ));
}
