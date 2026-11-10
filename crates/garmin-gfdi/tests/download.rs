//! Synthetic `FileAccess` listings, pull responses and owned transfer-pipe transcripts.

use std::io::Write as _;
use std::num::NonZeroU64;
use std::time::Duration;

use buffa::Message as _;
use flate2::{Compression, write::ZlibEncoder};
use garmin_gfdi::download::{DownloadError, DownloadSession};
use garmin_gfdi::proto::{Smart, data_types, file_access as wire};
use garmin_gfdi::rpc::encode_rpc_request;
use garmin_gfdi::{FrameDecoder, LinkMode, Message, encode_frame, encode_message};

const NOW: Duration = Duration::ZERO;
const DEADLINE: Duration = Duration::from_secs(10);

fn registration(service: u16, handle: u8, reliable: bool) -> Vec<u8> {
    let mut packet = vec![0, 1];
    packet.extend(1_u64.to_le_bytes());
    packet.extend(service.to_le_bytes());
    packet.extend([0, handle, u8::from(reliable)]);
    packet
}

fn close(service: u16, handle: u8) -> Vec<u8> {
    let mut packet = vec![0, 3];
    packet.extend(1_u64.to_le_bytes());
    packet.extend(service.to_le_bytes());
    packet.extend([handle, 0]);
    packet
}

fn new() -> DownloadSession {
    let (mut session, request) =
        DownloadSession::begin(NonZeroU64::new(1).unwrap(), LinkMode::Plain, DEADLINE, 1024);
    assert_eq!(request[10..13], [1, 0, 0]);
    session.receive(&registration(1, 0x86, false), NOW).unwrap();
    session
}

fn gfdi(
    session: &mut DownloadSession,
    kind: u16,
    payload: &[u8],
) -> Result<Vec<Vec<u8>>, DownloadError> {
    let mut outgoing = Vec::new();
    for chunk in encode_frame(&encode_message(kind, payload).unwrap()).chunks(19) {
        let mut packet = vec![0x86];
        packet.extend(chunk);
        outgoing.extend(session.receive(&packet, NOW)?);
    }
    Ok(outgoing)
}

fn identity(session: &mut DownloadSession) -> Result<Vec<Vec<u8>>, DownloadError> {
    let mut payload = vec![0; 12];
    payload[..2].copy_from_slice(&150_u16.to_le_bytes());
    payload[10..12].copy_from_slice(&375_u16.to_le_bytes());
    payload.extend([0, 0, 0]);
    gfdi(session, 5024, &payload)
}

fn rpc(
    session: &mut DownloadSession,
    service: wire::Service,
) -> Result<Vec<Vec<u8>>, DownloadError> {
    let bytes = Smart {
        file_access_service: service.into(),
        ..Default::default()
    }
    .encode_to_vec();
    if bytes.len() <= 4096 {
        let frame = encode_rpc_request(77, &bytes, 4096).unwrap();
        let message = Message::decode(&frame).unwrap();
        return gfdi(session, message.message_id(), message.payload());
    }
    let mut outgoing = Vec::new();
    for (index, chunk) in bytes.chunks(512).enumerate() {
        let mut payload = 77_u16.to_le_bytes().to_vec();
        payload.extend(u32::try_from(index * 512).unwrap().to_le_bytes());
        payload.extend(u32::try_from(bytes.len()).unwrap().to_le_bytes());
        payload.extend(u32::try_from(chunk.len()).unwrap().to_le_bytes());
        payload.extend(chunk);
        outgoing.extend(gfdi(session, 5043, &payload)?);
    }
    Ok(outgoing)
}

fn item(name: &str, size: u64) -> wire::ItemReference {
    wire::ItemReference {
        uid: data_types::UUID {
            most_significant: 1,
            least_significant: 2,
            ..Default::default()
        }
        .into(),
        data_type: wire::DataType {
            gdxml_string: Some(name.into()),
            ..Default::default()
        }
        .into(),
        data_size: Some(size),
        ..Default::default()
    }
}

fn listed(
    session: &mut DownloadSession,
    items: Vec<wire::ItemReference>,
    cursor: Option<u32>,
) -> Result<Vec<Vec<u8>>, DownloadError> {
    rpc(
        session,
        wire::Service {
            item_list_response: wire::ItemListResponse {
                status: Some(wire::item_list_response::Status::SUCCESS),
                session_id: cursor,
                items,
                ..Default::default()
            }
            .into(),
            ..Default::default()
        },
    )
}

fn pending_start(size: u64, compression: u32) -> DownloadSession {
    let mut session = new();
    identity(&mut session).unwrap();
    listed(&mut session, vec![item("FIT_TYPE_4", size)], None).unwrap();
    rpc(
        &mut session,
        wire::Service {
            pull_item_response: wire::PullItemDataResponse {
                result: Some(wire::ItemAccessResult::SUCCESS),
                transport: Some(wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE),
                transfer_handle: Some(7),
                compression_window: Some(compression),
                ..Default::default()
            }
            .into(),
            ..Default::default()
        },
    )
    .unwrap();
    session
        .receive(&registration(0x2018, 0x82, false), NOW)
        .unwrap();
    session
}

fn prepared(size: u64, compression: u32) -> DownloadSession {
    let mut session = pending_start(size, compression);
    session.receive(&[0x82, 0, 0, 0], NOW).unwrap();
    session
}

#[test]
fn transfer_status_response_preserves_peer_request_id_and_rejects_unknown_failures() {
    let mut session = prepared(3, 0);
    let status = wire::TransferStatusRequest {
        transfer_handle: Some(7),
        uid: item("FIT_TYPE_4", 3).uid,
        ..Default::default()
    };
    let packets = rpc(
        &mut session,
        wire::Service {
            transfer_status_request: status.into(),
            ..Default::default()
        },
    )
    .unwrap();
    let frames = outgoing_messages(&packets);
    let response = Message::decode(frames.last().unwrap()).unwrap();
    assert_eq!(response.message_id(), 5044);
    assert_eq!(response.payload()[..2], [77, 0]);
    let mut smart = Smart::decode_from_slice(&response.payload()[14..]).unwrap();
    let mut service = smart.file_access_service.take().unwrap();
    assert_eq!(
        service
            .transfer_status_response
            .take()
            .unwrap()
            .next_transfer_priority,
        Some(1)
    );
    assert_eq!(service.encode_to_vec(), []);

    let mut unknown = wire::TransferStatusRequest {
        transfer_handle: Some(7),
        ..Default::default()
    };
    unknown.merge_from_slice(&[24, 99]).unwrap();
    for status in [
        wire::TransferStatusRequest::default(),
        wire::TransferStatusRequest {
            transfer_handle: Some(8),
            ..Default::default()
        },
        wire::TransferStatusRequest {
            transfer_handle: Some(7),
            uid: data_types::UUID::default().into(),
            ..Default::default()
        },
        wire::TransferStatusRequest {
            transfer_handle: Some(7),
            failure_reason: Some(wire::transfer_status_request::FailureReason::TRANSPORT_FAILED),
            ..Default::default()
        },
        unknown,
    ] {
        let mut session = prepared(3, 0);
        assert!(
            rpc(
                &mut session,
                wire::Service {
                    transfer_status_request: status.into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(session.close_requests().len(), 2);
    }
}

#[test]
fn transfer_start_and_payload_bounds_are_explicit() {
    let mut session = pending_start(3, 0);
    assert!(session.receive(&[0x82, 9, 9, 9], NOW).is_err());
    let mut session = pending_start(3, 0);
    assert!(session.receive(&close(0x2018, 0x82), NOW).is_err());
    let mut session = prepared(3, 0);
    let mut packet = vec![0; 501];
    packet[0] = 0x82;
    for _ in 0..135 {
        session.receive(&packet, NOW).unwrap();
    }
    assert!(session.receive(&packet, NOW).is_err());
    assert!(session.take_file().is_none());
}

#[test]
fn first_candidate_waits_for_listing_end_and_type_key_changes_are_rejected() {
    let mut session = new();
    identity(&mut session).unwrap();
    let frames =
        outgoing_messages(&listed(&mut session, vec![item("FIT_TYPE_4", 3)], Some(42)).unwrap());
    let request = Message::decode(frames.last().unwrap()).unwrap();
    let smart = Smart::decode_from_slice(&request.payload()[14..]).unwrap();
    assert!(smart.file_access_service.item_list_request.is_set());
    assert!(smart.file_access_service.pull_item_request.is_unset());
    let frames = outgoing_messages(&listed(&mut session, Vec::new(), None).unwrap());
    let request = Message::decode(frames.last().unwrap()).unwrap();
    assert!(
        Smart::decode_from_slice(&request.payload()[14..])
            .unwrap()
            .file_access_service
            .pull_item_request
            .is_set()
    );

    let mut session = new();
    identity(&mut session).unwrap();
    let mut reference = item("OTHER", 3);
    reference.data_type.get_or_insert_default().gdxml_string_key = Some(1);
    listed(&mut session, vec![reference.clone()], Some(42)).unwrap();
    reference.data_type.get_or_insert_default().gdxml_string = Some("FIT_TYPE_4".into());
    assert!(listed(&mut session, vec![reference], None).is_err());
}

fn outgoing_messages(packets: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut decoder = FrameDecoder::new(10000);
    let mut messages = Vec::new();
    for packet in packets.iter().filter(|packet| packet[0] == 0x86) {
        for byte in &packet[1..] {
            if let Some(frame) = decoder.push(*byte) {
                messages.push(frame.unwrap());
            }
        }
    }
    messages
}

#[test]
fn host_handshake_and_listing_do_not_send_time_sync_or_legacy_commands() {
    let mut session = new();
    assert_eq!(session.poll(NOW).unwrap(), Vec::<Vec<u8>>::new());
    assert!(session.take_file().is_none());
    assert!(session.identity().is_none());
    let frames = outgoing_messages(&identity(&mut session).unwrap());
    assert_eq!(frames.len(), 2);
    assert_eq!(Message::decode(&frames[0]).unwrap().message_id(), 5000);
    assert_eq!(
        Message::decode(&frames[0]).unwrap().payload()[..3],
        [0xa0, 0x13, 0]
    );
    let request = Message::decode(&frames[1]).unwrap();
    assert_eq!(request.message_id(), 5043);
    let smart = Smart::decode_from_slice(&request.payload()[14..]).unwrap();
    assert_eq!(
        smart
            .file_access_service
            .item_list_request
            .exclude_all_flags,
        []
    );
    assert!(session.identity().is_some());
}

#[test]
fn plain_compressed_and_uncompressed_files_preserve_exact_original_bytes() {
    let original: Vec<u8> = (0..200).collect();
    for compression in [0, 15] {
        let mut session = prepared(original.len() as u64, compression);
        let bytes = if compression == 15 {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&original).unwrap();
            encoder.finish().unwrap()
        } else {
            original.clone()
        };
        for chunk in bytes.chunks(100) {
            let mut packet = vec![0x82];
            packet.extend(chunk);
            session.receive(&packet, NOW).unwrap();
        }
        assert!(session.take_file().is_none());
        session.receive(&close(0x2018, 0x82), NOW).unwrap();
        let file = session.take_file().unwrap();
        assert_eq!(file.bytes, original);
        assert_eq!(file.item.uid.most_significant, 1);
        assert_eq!(session.close_requests().len(), 1);
        assert!(session.receive(&[0x86], NOW).is_err());
        assert!(session.take_file().is_none());
    }
}

#[test]
fn continuation_uses_cursor_and_resolves_type_names_without_excluding_history() {
    let mut session = new();
    identity(&mut session).unwrap();
    let continuation = listed(&mut session, vec![item("FIT_TYPE_32", 1)], Some(9)).unwrap();
    let frames = outgoing_messages(&continuation);
    let request = Message::decode(frames.last().unwrap()).unwrap();
    let smart = Smart::decode_from_slice(&request.payload()[14..]).unwrap();
    assert_eq!(
        smart.file_access_service.item_list_request.session_id,
        Some(9)
    );
    assert_eq!(
        smart
            .file_access_service
            .item_list_request
            .exclude_all_flags,
        []
    );
    let mut reference = item("FIT_TYPE_4", 100);
    reference.data_type.get_or_insert_default().gdxml_string_key = Some(42);
    listed(&mut session, vec![reference], None).unwrap();
    assert_eq!(session.close_requests().len(), 1);
}

#[test]
fn invalid_listing_and_pull_statuses_stop_without_allocating_transfer_pipe() {
    for service in [
        wire::Service {
            item_list_response: wire::ItemListResponse::default().into(),
            ..Default::default()
        },
        wire::Service {
            item_list_response: wire::ItemListResponse {
                status: Some(wire::item_list_response::Status::FAIL_OTHER),
                ..Default::default()
            }
            .into(),
            ..Default::default()
        },
        wire::Service {
            pull_item_response: wire::PullItemDataResponse::default().into(),
            ..Default::default()
        },
    ] {
        let mut session = new();
        identity(&mut session).unwrap();
        assert!(rpc(&mut session, service).is_err());
        assert_eq!(session.close_requests().len(), 1);
        assert!(session.poll(NOW).is_err());
    }
    for result in [
        wire::PullItemDataResponse::default(),
        wire::PullItemDataResponse {
            result: Some(wire::ItemAccessResult::SUCCESS),
            transport: Some(wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE),
            ..Default::default()
        },
        wire::PullItemDataResponse {
            result: Some(wire::ItemAccessResult::SUCCESS),
            transport: Some(wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE),
            transfer_handle: Some(65536),
            ..Default::default()
        },
        wire::PullItemDataResponse {
            result: Some(wire::ItemAccessResult::SUCCESS),
            transport: Some(wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE),
            transfer_handle: Some(7),
            compression_window: Some(10),
            ..Default::default()
        },
    ] {
        let mut session = new();
        identity(&mut session).unwrap();
        listed(&mut session, vec![item("FIT_TYPE_4", 100)], None).unwrap();
        assert!(
            rpc(
                &mut session,
                wire::Service {
                    pull_item_response: result.into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert_eq!(session.close_requests().len(), 1);
    }
}

#[test]
fn missing_item_fields_oversized_files_and_stalled_listing_fail_closed() {
    for reference in [
        wire::ItemReference::default(),
        wire::ItemReference {
            uid: data_types::UUID::default().into(),
            ..Default::default()
        },
        item("FIT_TYPE_4", 1025),
    ] {
        let mut session = new();
        identity(&mut session).unwrap();
        assert!(listed(&mut session, vec![reference], None).is_err());
    }
    let mut session = new();
    identity(&mut session).unwrap();
    assert!(listed(&mut session, Vec::new(), Some(1)).is_err());
    let mut session = new();
    identity(&mut session).unwrap();
    assert!(listed(&mut session, Vec::new(), None).is_err());
    let mut session = new();
    identity(&mut session).unwrap();
    assert!(listed(&mut session, vec![item("OTHER", 1); 1001], Some(1)).is_err());
}

#[test]
fn corrupt_truncated_trailing_and_wrong_size_payloads_never_complete() {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&[1, 2, 3]).unwrap();
    let valid = encoder.finish().unwrap();
    for bytes in [vec![1, 2, 3], valid[..valid.len() - 1].to_vec(), {
        let mut bytes = valid.clone();
        bytes.push(0);
        bytes
    }] {
        let mut session = prepared(3, 15);
        let mut packet = vec![0x82];
        packet.extend(bytes);
        session.receive(&packet, NOW).unwrap();
        assert!(session.receive(&close(0x2018, 0x82), NOW).is_err());
        assert!(session.take_file().is_none());
    }
    let mut session = prepared(4, 0);
    session.receive(&[0x82, 1, 2, 3], NOW).unwrap();
    assert!(session.receive(&close(0x2018, 0x82), NOW).is_err());
    let mut session = prepared(2, 15);
    let mut packet = vec![0x82];
    packet.extend(valid);
    session.receive(&packet, NOW).unwrap();
    assert!(session.receive(&close(0x2018, 0x82), NOW).is_err());
}

#[test]
fn malformed_transport_authentication_and_expired_sessions_keep_cleanup_available() {
    for packet in [Vec::new(), vec![0; 513]] {
        let mut session = new();
        assert!(session.receive(&packet, NOW).is_err());
        assert_eq!(session.close_requests().len(), 1);
    }
    let mut session = new();
    assert!(gfdi(&mut session, 5101, &[]).is_err());
    let mut session = new();
    assert!(session.poll(DEADLINE).is_err());
    assert!(session.poll(NOW).is_err());
    let mut session = new();
    session.cancel();
    assert!(session.receive(&[0x86], NOW).is_err());
    let mut session = new();
    assert!(session.receive(&close(1, 0x86), NOW).is_err());
    let mut session = new();
    identity(&mut session).unwrap();
    listed(&mut session, vec![item("FIT_TYPE_4", 1)], None).unwrap();
    rpc(
        &mut session,
        wire::Service {
            pull_item_response: wire::PullItemDataResponse {
                result: Some(wire::ItemAccessResult::SUCCESS),
                transport: Some(wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE),
                transfer_handle: Some(7),
                compression_window: Some(0),
                ..Default::default()
            }
            .into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        session
            .receive(&registration(0x2018, 0x86, false), NOW)
            .is_err()
    );
    assert_eq!(session.close_requests().len(), 2);
}
