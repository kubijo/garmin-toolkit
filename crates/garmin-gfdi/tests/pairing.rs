//! First-pairing transcripts and rejection cases; no physical Bluetooth state.

use std::num::NonZeroU64;
use std::time::Duration;

use garmin_gfdi::download::DownloadError;
use garmin_gfdi::pairing::PairingSession;
use garmin_gfdi::{FrameDecoder, LinkMode, Message, encode_frame, encode_message};

const NOW: Duration = Duration::ZERO;
const LIMIT: Duration = Duration::from_secs(10);

fn session() -> PairingSession {
    let (mut session, request) =
        PairingSession::begin(NonZeroU64::new(1).unwrap(), LinkMode::Plain, LIMIT);
    assert_eq!(request, [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0]);
    session
        .receive(&[0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0x86, 0], NOW)
        .unwrap();
    session
}

fn receive(
    session: &mut PairingSession,
    kind: u16,
    payload: &[u8],
) -> Result<Vec<(u16, Vec<u8>)>, DownloadError> {
    let mut outgoing = Vec::new();
    for chunk in encode_frame(&encode_message(kind, payload).unwrap()).chunks(19) {
        let mut packet = vec![0x86];
        packet.extend(chunk);
        outgoing.extend(session.receive(&packet, NOW)?);
    }
    let mut decoder = FrameDecoder::new(10_000);
    let mut messages = Vec::new();
    for packet in outgoing {
        assert_eq!(packet[0], 0x86);
        for byte in &packet[1..] {
            if let Some(frame) = decoder.push(*byte) {
                let frame = frame.unwrap();
                let message = Message::decode(&frame).unwrap();
                messages.push((message.raw_type(), message.payload().to_vec()));
            }
        }
    }
    Ok(messages)
}

fn identify(session: &mut PairingSession) {
    let mut identity = vec![0; 12];
    identity[..2].copy_from_slice(&150_u16.to_le_bytes());
    identity[10..12].copy_from_slice(&375_u16.to_le_bytes());
    identity.extend([0, 0, 0]);
    let replies = receive(session, 5024, &identity).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].0, 5000);
    assert_eq!(replies[0].1[..3], [0xa0, 0x13, 0]);
    assert!(
        replies[0]
            .1
            .windows(14)
            .any(|name| name == b"Garmin Toolkit")
    );
}

fn configured() -> PairingSession {
    let mut session = session();
    identify(&mut session);
    let replies = receive(&mut session, 5050, &[2, 0xff, 0xff]).unwrap();
    assert_eq!(replies[0], (5000, vec![0xba, 0x13, 0]));
    assert_eq!(
        replies[1],
        (0x8032, vec![10, 1, 0, 0, 0, 0, 0, 0, 0, 0, 16])
    );
    session
}

#[test]
fn pairing_requires_configuration_and_three_correlated_event_acks() {
    let mut session = configured();
    assert!(session.completed().is_none());
    assert_eq!(
        receive(&mut session, 0x8000, &[0xba, 0x13, 0]).unwrap(),
        [(0x811e, vec![4, 0])]
    );
    assert_eq!(
        receive(&mut session, 0x8100, &[0xa6, 0x13, 0]).unwrap(),
        [(0x821e, vec![0, 0])]
    );
    assert!(session.completed().is_none());
    assert_eq!(
        receive(&mut session, 0x8200, &[0xa6, 0x13, 0]).unwrap(),
        [(0x831e, vec![14, 0])]
    );
    assert!(session.completed().is_none());
    assert_eq!(
        receive(&mut session, 0x8300, &[0xa6, 0x13, 0]).unwrap(),
        [] as [(u16, Vec<u8>); 0]
    );
    assert!(session.completed().is_some());
    assert_eq!(
        session.close_request().unwrap(),
        [0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0x86]
    );
}

#[test]
fn stale_unsequenced_or_unrelated_acks_cannot_complete_setup() {
    let mut session = configured();
    for (kind, payload) in [
        (5000, [0xba, 0x13, 0]),
        (0x8100, [0xba, 0x13, 0]),
        (0x8000, [0xa6, 0x13, 0]),
    ] {
        assert_eq!(
            receive(&mut session, kind, &payload).unwrap(),
            [] as [(u16, Vec<u8>); 0]
        );
    }
    receive(&mut session, 0x8000, &[0xba, 0x13, 0]).unwrap();
    receive(&mut session, 0x8100, &[0xa6, 0x13, 0]).unwrap();
    assert_eq!(
        receive(&mut session, 0x8100, &[0xa6, 0x13, 0]).unwrap(),
        [] as [(u16, Vec<u8>); 0]
    );
    assert!(session.completed().is_none());
}

#[test]
fn negotiation_echoes_only_the_upstream_envelope_and_never_runs_crypto() {
    let mut session = session();
    assert_eq!(
        receive(&mut session, 0x8465, &[7, 3, 0, 0, 0]).unwrap(),
        [(0x8400, vec![0xed, 0x13, 0, 0, 7, 3, 0, 0, 0])]
    );
    assert_eq!(
        receive(&mut session, 5052, &[]).unwrap(),
        [(5000, vec![0xbc, 0x13, 2])]
    );
    assert!(receive(&mut session, 5102, &[]).is_err());
    assert!(session.poll(NOW).is_err());
}

#[test]
fn malformed_or_rejected_handshakes_stop_and_keep_owned_cleanup() {
    for (kind, payload) in [
        (5050, vec![]),
        (5050, vec![2, 1]),
        (5050, vec![0, 1]),
        (5101, vec![0; 4]),
        (0x8000, vec![0xba, 0x13, 1]),
        (0x8000, vec![0xba]),
    ] {
        let mut session = if kind == 0x8000 {
            configured()
        } else {
            let mut session = session();
            identify(&mut session);
            session
        };
        assert!(receive(&mut session, kind, &payload).is_err());
        assert!(session.completed().is_none());
        assert!(session.poll(NOW).is_err());
        assert!(session.close_request().is_some());
    }
}

#[test]
fn timeout_peer_close_and_invalid_notifications_never_report_success() {
    let mut timed = configured();
    assert!(timed.poll(LIMIT).is_err());
    assert!(timed.poll(NOW).is_err());
    for packet in [
        vec![],
        vec![0; 513],
        vec![0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0x86, 0],
    ] {
        let mut session = session();
        assert!(session.receive(&packet, NOW).is_err());
        assert!(session.completed().is_none());
    }
}
