//! Synthetic receive-side MLR and identity-observation transcripts.

use std::num::{NonZeroU8, NonZeroU64};
use std::time::Duration;

use garmin_gfdi::{
    DeviceInformation, IdentityError, IdentityProbe, LinkMode, MlrError, MlrReceiver, ProbeError,
    RouteKind, encode_frame, encode_message,
};

const CLIENT: NonZeroU64 = NonZeroU64::new(0x0807_0605_0403_0201).unwrap();
const HANDLE: NonZeroU8 = NonZeroU8::new(0x86).unwrap();
const NOW: Duration = Duration::from_millis(10);
const DEADLINE: Duration = Duration::from_secs(1);
const NO_ACKS: [[u8; 2]; 0] = [];

fn identity_payload() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend(150_u16.to_le_bytes());
    data.extend(0x1234_u16.to_le_bytes());
    data.extend(0x0102_0304_u32.to_le_bytes());
    data.extend(1234_u16.to_le_bytes());
    data.extend(375_u16.to_le_bytes());
    for value in ["Demo Garmin", "Demo vélo", "Synthetic tracker"] {
        data.push(u8::try_from(value.len()).unwrap());
        data.extend(value.as_bytes());
    }
    data
}

fn identity_frame() -> Vec<u8> {
    encode_frame(&encode_message(5024, &identity_payload()).unwrap())
}

fn packet(seq: u8, peer_ack: u8, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xe0 | (peer_ack >> 2), ((peer_ack & 3) << 6) | seq];
    bytes.extend(data);
    bytes
}

fn probe(mode: LinkMode) -> IdentityProbe {
    let (mut probe, request) =
        IdentityProbe::begin(RouteKind::MultiLink, CLIENT, mode, 20, DEADLINE);
    assert_eq!(request.unwrap()[..2], [0, 0]);
    let mut reply = hex::decode("000101020304050607080100008601").unwrap();
    if mode == LinkMode::Plain {
        reply[14] = 0;
    }
    assert_eq!(probe.receive(&reply, NOW).unwrap(), NO_ACKS);
    probe
}

#[test]
fn identity_fields_are_bounded_and_extensions_are_preserved() {
    let mut payload = identity_payload();
    let identity = DeviceInformation::decode(&payload).unwrap();
    assert_eq!(identity.protocol_version, 150);
    assert_eq!(identity.product_number, 0x1234);
    assert_eq!(identity.unit_number, 0x0102_0304);
    assert_eq!(identity.software_version, 1234);
    assert_eq!(identity.max_packet_size, 375);
    assert_eq!(identity.bluetooth_name, "Demo Garmin");
    assert_eq!(identity.device_name, "Demo vélo");
    assert_eq!(identity.model, "Synthetic tracker");
    assert_eq!(identity.extension, []);
    payload.extend([1, 2, 3]);
    assert_eq!(
        DeviceInformation::decode(&payload).unwrap().extension,
        [1, 2, 3]
    );
    for length in 0..identity_payload().len() {
        assert_eq!(
            DeviceInformation::decode(&identity_payload()[..length]),
            Err(IdentityError::Truncated)
        );
    }
    payload[13] = 255;
    assert_eq!(
        DeviceInformation::decode(&payload),
        Err(IdentityError::InvalidText)
    );
    let empty = [0; 15];
    assert_eq!(DeviceInformation::decode(&empty).unwrap().model, "");
}

#[test]
fn mlr_header_and_ack_timer_match_the_wire_layout() {
    let mut receiver = MlrReceiver::new(HANDLE, 20).unwrap();
    assert_eq!(receiver.poll(NOW).unwrap(), None);
    assert_eq!(receiver.flush_ack().unwrap(), None);
    let bytes = packet(0, 63, &[7, 8]);
    assert_eq!(&bytes[..2], &[0xef, 0xc0]);
    let update = receiver.receive(&bytes, NOW).unwrap();
    assert_eq!(update.peer_ack, 63);
    assert_eq!(update.data, Some([7, 8].as_slice()));
    assert_eq!(update.ack, None);
    assert_eq!(receiver.poll(Duration::from_millis(9)).unwrap(), None);
    assert_eq!(receiver.poll(Duration::from_millis(29)).unwrap(), None);
    assert_eq!(
        receiver.poll(Duration::from_millis(30)).unwrap(),
        Some([0xe0, 0x40])
    );
    assert_eq!(receiver.poll(Duration::from_millis(99)).unwrap(), None);
}

#[test]
fn mlr_duplicates_and_reordering_are_not_delivered() {
    let mut receiver = MlrReceiver::new(HANDLE, 20).unwrap();
    let second = packet(1, 0, &[2]);
    let out_of_order = receiver.receive(&second, NOW).unwrap();
    assert_eq!(out_of_order.data, None);
    assert_eq!(out_of_order.ack, Some([0xe0, 0]));
    let first = packet(0, 0, &[1]);
    assert_eq!(
        receiver.receive(&first, NOW).unwrap().data,
        Some([1].as_slice())
    );
    let duplicate = receiver.receive(&first, NOW).unwrap();
    assert_eq!(duplicate.data, None);
    assert_eq!(duplicate.ack, Some([0xe0, 0x40]));
    assert_eq!(
        receiver.receive(&second, NOW).unwrap().data,
        Some([2].as_slice())
    );
    assert_eq!(receiver.flush_ack().unwrap(), Some([0xe0, 0x80]));
}

#[test]
fn mlr_window_wraps_and_ack_timer_does_not_slide_with_new_data() {
    let mut receiver = MlrReceiver::new(HANDLE, 20).unwrap();
    for index in 0..130_u16 {
        let seq = u8::try_from(index & 63).unwrap();
        let bytes = packet(seq, 0, &[seq]);
        let update = receiver.receive(&bytes, NOW).unwrap();
        assert_eq!(update.data, Some([seq].as_slice()));
        let expected = u8::try_from((index + 1) & 63).unwrap();
        let ack = ((index + 1) % 5 == 0).then_some([0xe0 | (expected >> 2), (expected & 3) << 6]);
        assert_eq!(update.ack, ack);
    }
    assert_eq!(receiver.flush_ack().unwrap(), None);
    let mut receiver = MlrReceiver::new(HANDLE, 20).unwrap();
    receiver.receive(&packet(0, 0, &[1]), NOW).unwrap();
    receiver
        .receive(&packet(1, 0, &[2]), Duration::from_millis(29))
        .unwrap();
    assert_eq!(
        receiver.poll(Duration::from_millis(30)).unwrap(),
        Some([0xe0, 0x80])
    );
}

#[test]
fn ack_only_packets_do_not_advance_data_sequence_or_cause_ack_loops() {
    let mut receiver = MlrReceiver::new(HANDLE, 20).unwrap();
    let bytes = packet(42, 31, &[]);
    let update = receiver.receive(&bytes, NOW).unwrap();
    assert_eq!(update.peer_ack, 31);
    assert_eq!(update.data, None);
    assert_eq!(update.ack, None);
    assert_eq!(receiver.poll(Duration::from_secs(10)).unwrap(), None);
    assert_eq!(
        receiver.receive(&packet(0, 0, &[1]), NOW).unwrap().data,
        Some([1].as_slice())
    );
}

#[test]
fn invalid_mlr_input_is_bounded_and_closed_receivers_stay_closed() {
    assert!(matches!(
        MlrReceiver::new(HANDLE, 1),
        Err(MlrError::TooShort)
    ));
    let mut receiver = MlrReceiver::new(HANDLE, 3).unwrap();
    assert_eq!(receiver.receive(&[], NOW), Err(MlrError::TooShort));
    assert_eq!(receiver.receive(&[0xe0], NOW), Err(MlrError::TooShort));
    assert_eq!(
        receiver.receive(&[0xe0, 0, 1, 2], NOW),
        Err(MlrError::TooLong)
    );
    assert_eq!(
        receiver.receive(&[0x60, 0, 1], NOW),
        Err(MlrError::NotReliable)
    );
    assert_eq!(
        receiver.receive(&[0xd0, 0, 1], NOW),
        Err(MlrError::WrongHandle)
    );
    receiver.close();
    receiver.close();
    assert_eq!(receiver.receive(&[0xe0, 0, 1], NOW), Err(MlrError::Closed));
    assert_eq!(receiver.poll(NOW), Err(MlrError::Closed));
    assert_eq!(receiver.flush_ack(), Err(MlrError::Closed));
}

#[test]
fn identity_is_observed_on_direct_plain_and_reliable_routes() {
    for route in [
        RouteKind::DirectV0,
        RouteKind::DirectV1,
        RouteKind::MultiLink,
    ] {
        let mut probe = if route == RouteKind::MultiLink {
            probe(LinkMode::Plain)
        } else {
            let (probe, request) =
                IdentityProbe::begin(route, CLIENT, LinkMode::Plain, 20, DEADLINE);
            assert_eq!(request, None);
            probe
        };
        assert_eq!(probe.identity(), None);
        assert_eq!(probe.poll(NOW).unwrap(), None);
        for chunk in identity_frame().chunks(18) {
            let mut bytes = Vec::new();
            if route == RouteKind::MultiLink {
                bytes.push(0x86);
            }
            bytes.extend(chunk);
            assert_eq!(probe.receive(&bytes, NOW).unwrap(), NO_ACKS);
        }
        assert_eq!(probe.identity().unwrap().model, "Synthetic tracker");
        assert!(matches!(probe.poll(NOW), Err(ProbeError::Complete)));
        assert_eq!(
            probe.close_request().is_some(),
            route == RouteKind::MultiLink
        );
    }
    let mut probe = probe(LinkMode::Reliable);
    let mut ack_count = 0;
    for (index, chunk) in identity_frame().chunks(18).enumerate() {
        let outgoing = probe
            .receive(&packet(u8::try_from(index).unwrap(), 0, chunk), NOW)
            .unwrap();
        ack_count += outgoing.len();
    }
    assert_eq!(probe.identity().unwrap().model, "Synthetic tracker");
    assert!(ack_count > 0);
    assert_eq!(
        hex::encode(probe.close_request().unwrap()),
        "00020102030405060708010086"
    );
}

#[test]
fn identity_probe_does_not_reply_to_time_configuration_or_unknown_messages() {
    let mut probe = probe(LinkMode::Plain);
    for id in [5052, 5050, 6000] {
        let mut bytes = vec![0x86];
        bytes.extend(encode_frame(&encode_message(id, &[]).unwrap()));
        assert_eq!(probe.receive(&bytes, NOW).unwrap(), NO_ACKS);
        assert_eq!(probe.identity(), None);
    }
    assert_eq!(probe.receive(&[9, 1, 2], NOW).unwrap(), NO_ACKS);
    assert_eq!(probe.receive(&[0, 6], NOW).unwrap(), NO_ACKS);
    let mut close = probe.close_request().unwrap().to_vec();
    close[1] = 3;
    close[2] ^= 1;
    assert_eq!(probe.receive(&close, NOW).unwrap(), NO_ACKS);
    close[2] ^= 1;
    assert!(matches!(
        probe.receive(&close, NOW),
        Err(ProbeError::PeerClosed)
    ));
    assert!(matches!(probe.poll(NOW), Err(ProbeError::Stopped)));
    assert!(probe.close_request().is_some());
}

#[test]
fn authentication_crc_and_notification_errors_stop_without_application_replies() {
    let mut auth_probe = probe(LinkMode::Plain);
    let mut auth = vec![0x86];
    auth.extend(encode_frame(&encode_message(5101, &[]).unwrap()));
    assert!(matches!(
        auth_probe.receive(&auth, NOW),
        Err(ProbeError::AuthenticationRequired)
    ));
    let (mut direct, _) =
        IdentityProbe::begin(RouteKind::DirectV1, CLIENT, LinkMode::Plain, 512, DEADLINE);
    let mut corrupted = encode_message(5024, &identity_payload()).unwrap();
    corrupted[6] ^= 1;
    assert!(matches!(
        direct.receive(&encode_frame(&corrupted), NOW),
        Err(ProbeError::Message(_))
    ));
    for bytes in [vec![], vec![1; 21]] {
        let mut probe = probe(LinkMode::Reliable);
        assert!(matches!(
            probe.receive(&bytes, NOW),
            Err(ProbeError::NotificationSize)
        ));
    }
}

#[test]
fn reliable_probe_checks_peer_ack_and_drives_ack_deadlines() {
    let mut probe = probe(LinkMode::Reliable);
    assert_eq!(probe.receive(&packet(0, 0, &[0, 2]), NOW).unwrap(), NO_ACKS);
    assert_eq!(
        probe.poll(Duration::from_millis(30)).unwrap(),
        Some([0xe0, 0x40])
    );
    assert_eq!(probe.receive(&[0xd0, 0, 1], NOW).unwrap(), NO_ACKS);
    assert!(matches!(
        probe.receive(&packet(1, 1, &[]), NOW),
        Err(ProbeError::UnexpectedPeerAck)
    ));
}

#[test]
fn cancelled_and_timed_out_probes_preserve_only_known_cleanup() {
    let (mut pending, _) = IdentityProbe::begin(
        RouteKind::MultiLink,
        CLIENT,
        LinkMode::Reliable,
        20,
        DEADLINE,
    );
    assert_eq!(pending.close_request(), None);
    pending.cancel();
    assert!(matches!(pending.poll(NOW), Err(ProbeError::Stopped)));
    let mut registered = probe(LinkMode::Reliable);
    assert!(matches!(
        registered.poll(DEADLINE),
        Err(ProbeError::Timeout)
    ));
    assert!(registered.close_request().is_some());
    assert!(matches!(
        registered.receive(&[0xe0, 0], NOW),
        Err(ProbeError::Stopped)
    ));
}

#[test]
fn pending_registration_and_malformed_frames_are_terminal_for_the_probe() {
    let (mut probe, _) = IdentityProbe::begin(
        RouteKind::MultiLink,
        CLIENT,
        LinkMode::Reliable,
        20,
        DEADLINE,
    );
    assert_eq!(probe.receive(&[9, 0, 0], NOW).unwrap(), NO_ACKS);
    assert!(matches!(
        probe.receive(&[], NOW),
        Err(ProbeError::NotificationSize)
    ));
    let (mut direct, _) =
        IdentityProbe::begin(RouteKind::DirectV1, CLIENT, LinkMode::Plain, 20, DEADLINE);
    assert!(matches!(
        direct.receive(&[0, 5, 1, 0], NOW),
        Err(ProbeError::Frame(_))
    ));
    let (mut direct, _) =
        IdentityProbe::begin(RouteKind::DirectV1, CLIENT, LinkMode::Plain, 20, DEADLINE);
    let bytes = encode_frame(&encode_message(5024, &[]).unwrap());
    assert!(matches!(
        direct.receive(&bytes, NOW),
        Err(ProbeError::Identity(_))
    ));
}
