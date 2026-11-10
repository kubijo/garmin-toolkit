//! Independent wire fixtures for generated proto2 types and bounded Buffa decoding.

use buffa::{DecodeOptions, Message};
use garmin_gfdi::proto::{Smart, data_types, file_access};

#[test]
fn smart_field_43_and_file_listing_field_9_match_known_wire_tags() {
    let message = Smart {
        file_access_service: file_access::Service {
            item_list_request: file_access::ItemListRequest::default().into(),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    };
    assert_eq!(message.encode_to_vec(), hex::decode("da02024a00").unwrap());
    let decoded = Smart::decode_from_slice(&hex::decode("da02044a021864").unwrap()).unwrap();
    let listing = decoded
        .file_access_service
        .item_list_request
        .as_option()
        .unwrap();
    assert_eq!(listing.max_item_count, Some(100));
    assert_eq!(listing.session_id, None);
    assert_eq!(listing.transaction_id, None);
    assert_eq!(listing.exclude_all_flags, []);
}

#[test]
fn explicit_zero_result_is_distinct_from_absent_proto2_result() {
    let missing = file_access::PullItemDataResponse::decode_from_slice(&[]).unwrap();
    let success = file_access::PullItemDataResponse::decode_from_slice(&[8, 0]).unwrap();
    assert_eq!(missing.result, None);
    assert_eq!(success.result, Some(file_access::ItemAccessResult::SUCCESS));
    assert_eq!(missing.encode_to_vec(), []);
    assert_eq!(success.encode_to_vec(), [8, 0]);
}

#[test]
fn opaque_identifiers_preserve_unsigned_fixed64_bits() {
    let bytes = hex::decode("09ffffffffffffffff110807060504030201").unwrap();
    let identifier = data_types::UUID::decode_from_slice(&bytes).unwrap();
    assert_eq!(identifier.most_significant, u64::MAX);
    assert_eq!(identifier.least_significant, 0x0102_0304_0506_0708);
    assert_eq!(identifier.encode_to_vec(), bytes);
}

#[test]
fn unknown_fields_round_trip_without_becoming_success() {
    // Unknown enum result 99 is preserved as an unknown field, never coerced to SUCCESS.
    let response = file_access::PullItemDataResponse::decode_from_slice(&[8, 99]).unwrap();
    assert_eq!(response.result, None);
    assert_eq!(response.encode_to_vec(), [8, 99]);
    let unknown = Smart::decode_from_slice(&[8, 123]).unwrap();
    assert!(unknown.file_access_service.is_unset());
    assert_eq!(unknown.encode_to_vec(), [8, 123]);
}

#[test]
fn malformed_messages_and_decode_limits_are_enforced() {
    for bytes in [&[0xda, 2, 255][..], &[0][..], &[0xda, 2, 1, 0xff][..]] {
        assert!(Smart::decode_from_slice(bytes).is_err());
    }
    assert!(
        DecodeOptions::new()
            .with_max_message_size(2)
            .decode_from_slice::<Smart>(&[0xda, 2, 0])
            .is_err()
    );
    assert!(
        DecodeOptions::new()
            .with_unknown_field_limit(0)
            .decode_from_slice::<Smart>(&[8, 1])
            .is_err()
    );
}
