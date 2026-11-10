//! Synthetic `MultiLink` vectors adapted from `CommunicatorV2`;
//! see ../README.md.

use std::num::{NonZeroU8, NonZeroU64};
use std::time::Duration;

use garmin_gfdi::{
    Channel, Characteristic, LinkMode, Registration, RegistrationError, RegistrationState,
    RouteKind, select_route,
};

const CLIENT: NonZeroU64 = NonZeroU64::new(0x0807_0605_0403_0201).unwrap();
const DEADLINE: Duration = Duration::from_secs(10);
const NOW: Duration = Duration::from_secs(1);
const ML_SERVICE: u128 = 0x6a4e_2800_667b_11e3_949a_0800_200c_9a66;
const ML_RX: u128 = 0x6a4e_2810_667b_11e3_949a_0800_200c_9a66;
const ML_TX: u128 = 0x6a4e_2820_667b_11e3_949a_0800_200c_9a66;

fn reply() -> Vec<u8> {
    hex::decode("000101020304050607080100008601").unwrap()
}

fn pair(service: u128, receive: u128, send: u128) -> [Characteristic; 2] {
    [
        Characteristic {
            service,
            uuid: receive,
            notify: true,
            write: false,
        },
        Characteristic {
            service,
            uuid: send,
            notify: false,
            write: true,
        },
    ]
}

#[test]
fn registration_request_matches_upstream_layout_without_close_all() {
    for (mode, flag) in [(LinkMode::Plain, "00"), (LinkMode::Reliable, "02")] {
        let (attempt, request) = Registration::begin(CLIENT, mode, DEADLINE);
        assert_eq!(
            hex::encode(request),
            "000001020304050607080100".to_owned() + flag
        );
        assert_eq!(attempt.state(), RegistrationState::Pending);
    }
}

#[test]
fn matching_reply_registers_the_explicit_mode() {
    for (mode, flag) in [(LinkMode::Plain, 0), (LinkMode::Reliable, 1)] {
        for extension in [None, Some(17)] {
            let (mut attempt, _) = Registration::begin(CLIENT, mode, DEADLINE);
            let mut bytes = reply();
            bytes[14] = flag;
            bytes.extend(extension);
            let expected = Channel {
                handle: NonZeroU8::new(0x86).unwrap(),
                mode,
                extension,
            };
            assert_eq!(attempt.receive(&bytes, NOW), Ok(Some(expected)));
            assert_eq!(attempt.state(), RegistrationState::Registered(expected));
            assert_eq!(
                attempt.receive(&bytes, NOW),
                Err(RegistrationError::NotPending)
            );
            attempt.cancel();
            assert_eq!(attempt.state(), RegistrationState::Registered(expected));
        }
    }
}

#[test]
fn unrelated_replies_and_data_do_not_change_the_attempt() {
    let (mut attempt, _) = Registration::begin(CLIENT, LinkMode::Reliable, DEADLINE);
    let mut other_client = reply();
    other_client[2] ^= 1;
    let mut other_service = reply();
    other_service[10] = 4;
    for bytes in [vec![5, 0, 0], vec![0, 3], other_client, other_service] {
        assert_eq!(attempt.receive(&bytes, NOW), Ok(None));
        assert_eq!(attempt.state(), RegistrationState::Pending);
    }
    assert_eq!(attempt.poll(DEADLINE), Err(RegistrationError::Timeout));
    assert_eq!(attempt.state(), RegistrationState::Failed);
}

#[test]
fn timeout_and_cancellation_cannot_be_revived_by_late_replies() {
    for now in [DEADLINE, DEADLINE + Duration::from_nanos(1)] {
        let (mut attempt, _) = Registration::begin(CLIENT, LinkMode::Reliable, DEADLINE);
        assert_eq!(
            attempt.receive(&reply(), now),
            Err(RegistrationError::Timeout)
        );
        assert_eq!(
            attempt.receive(&reply(), NOW),
            Err(RegistrationError::NotPending)
        );
        attempt.cancel();
        assert_eq!(attempt.state(), RegistrationState::Failed);
    }
    let (mut attempt, _) = Registration::begin(CLIENT, LinkMode::Reliable, DEADLINE);
    attempt.cancel();
    attempt.cancel();
    assert_eq!(attempt.state(), RegistrationState::Cancelled);
    assert_eq!(
        attempt.receive(&reply(), NOW),
        Err(RegistrationError::NotPending)
    );
}

#[test]
fn fresh_client_identity_rejects_an_earlier_attempts_reply() {
    let new_client = NonZeroU64::new(CLIENT.get() + 1).unwrap();
    let (mut attempt, _) = Registration::begin(new_client, LinkMode::Reliable, DEADLINE);
    assert_eq!(attempt.receive(&reply(), NOW), Ok(None));
    let mut bytes = reply();
    bytes[2] += 1;
    assert!(attempt.receive(&bytes, NOW).unwrap().is_some());
}

#[test]
fn denials_are_terminal_and_preserve_unknown_status_codes() {
    for status in [1, 2, 3, 4, 255] {
        let (mut attempt, _) = Registration::begin(CLIENT, LinkMode::Reliable, DEADLINE);
        let mut bytes = reply();
        bytes.truncate(13);
        bytes[12] = status;
        assert_eq!(
            attempt.receive(&bytes, NOW),
            Err(RegistrationError::Denied { status })
        );
        assert_eq!(attempt.state(), RegistrationState::Failed);
        assert_eq!(
            attempt.receive(&reply(), NOW),
            Err(RegistrationError::NotPending)
        );
    }
}

#[test]
fn mode_changes_fail_without_silent_fallback_and_retain_the_channel() {
    for (requested, accepted, flag) in [
        (LinkMode::Plain, LinkMode::Reliable, 1),
        (LinkMode::Reliable, LinkMode::Plain, 0),
    ] {
        let (mut attempt, _) = Registration::begin(CLIENT, requested, DEADLINE);
        let mut bytes = reply();
        bytes[14] = flag;
        assert_eq!(
            attempt.receive(&bytes, NOW),
            Err(RegistrationError::ModeMismatch {
                requested,
                channel: Channel {
                    handle: NonZeroU8::new(0x86).unwrap(),
                    mode: accepted,
                    extension: None
                },
            })
        );
        assert_eq!(attempt.state(), RegistrationState::Failed);
    }
}

#[test]
fn truncated_or_extended_replies_never_panic_or_extend_deadlines() {
    for length in 0..reply().len() {
        let (mut attempt, _) = Registration::begin(CLIENT, LinkMode::Reliable, DEADLINE);
        assert_eq!(
            attempt.receive(&reply()[..length], NOW),
            Err(RegistrationError::Malformed)
        );
        assert_eq!(attempt.state(), RegistrationState::Pending);
        assert_eq!(attempt.poll(DEADLINE), Err(RegistrationError::Timeout));
    }
    for (offset, value) in [(13, 0), (14, 2)] {
        let (mut attempt, _) = Registration::begin(CLIENT, LinkMode::Reliable, DEADLINE);
        let mut bytes = reply();
        bytes[offset] = value;
        assert_eq!(
            attempt.receive(&bytes, NOW),
            Err(RegistrationError::Malformed)
        );
        assert!(attempt.receive(&reply(), NOW).unwrap().is_some());
    }
    let (mut attempt, _) = Registration::begin(CLIENT, LinkMode::Reliable, DEADLINE);
    let mut bytes = reply();
    bytes.extend([0, 0]);
    assert_eq!(
        attempt.receive(&bytes, NOW),
        Err(RegistrationError::Malformed)
    );
}

#[test]
fn all_five_multilink_slots_are_selected_only_as_complete_pairs() {
    for slot in 0..5_u128 {
        let offset = slot << 96;
        let inventory = pair(ML_SERVICE, ML_RX + offset, ML_TX + offset);
        let route = select_route(&inventory).unwrap();
        assert_eq!(route.kind, RouteKind::MultiLink);
        assert_eq!(route.service, ML_SERVICE);
        assert_eq!(route.receive, inventory[0].uuid);
        assert_eq!(route.send, inventory[1].uuid);
    }
}

#[test]
fn route_selection_requires_matching_services_slots_and_properties() {
    assert_eq!(select_route(&[]), None);
    for broken in 0..4 {
        let mut inventory = pair(ML_SERVICE, ML_RX, ML_TX);
        match broken {
            0 => inventory[1].uuid += 1 << 96,
            1 => inventory[1].service += 1,
            2 => inventory[0].notify = false,
            _ => inventory[1].write = false,
        }
        assert_eq!(select_route(&inventory), None);
    }
    assert_eq!(
        select_route(&pair(ML_SERVICE, ML_RX + (5 << 96), ML_TX + (5 << 96))),
        None
    );
}

#[test]
fn direct_routes_and_multilink_preference_match_upstream() {
    let v0 = pair(
        0x9b01_2401_bc30_ce9a_e111_0f67_e491_abde,
        0x4acb_cd28_7425_868e_f447_915c_8f00_d0cb,
        0xdf33_4c80_e6a7_d082_274d_78fc_66f8_5e16,
    );
    let v1 = pair(
        0x6a4e_2401_667b_11e3_949a_0800_200c_9a66,
        0x6a4e_cd28_667b_11e3_949a_0800_200c_9a66,
        0x6a4e_4c80_667b_11e3_949a_0800_200c_9a66,
    );
    assert_eq!(select_route(&v0).unwrap().kind, RouteKind::DirectV0);
    let mut inventory = v0.to_vec();
    inventory.extend(v1);
    assert_eq!(select_route(&inventory).unwrap().kind, RouteKind::DirectV1);
    inventory.extend(pair(ML_SERVICE, ML_RX + (1 << 96), ML_TX + (1 << 96)));
    inventory.extend(pair(ML_SERVICE, ML_RX, ML_TX));
    assert_eq!(select_route(&inventory).unwrap().receive, ML_RX);
}
