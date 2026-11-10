use std::num::NonZeroU32;

use buffa::Message as _;
use garmin_gfdi::file_access::{
    FileAccessError, FileAccessResponse, decode_response, list_request, pull_request,
};
use garmin_gfdi::proto::{Smart, data_types, file_access as wire};

fn encoded(service: wire::Service) -> Vec<u8> {
    Smart {
        file_access_service: service.into(),
        ..Default::default()
    }
    .encode_to_vec()
}

fn item() -> wire::ItemReference {
    wire::ItemReference {
        uid: data_types::UUID {
            most_significant: 1,
            least_significant: 2,
            ..Default::default()
        }
        .into(),
        data_type: wire::DataType {
            gdxml_string: Some("FIT_TYPE_4".into()),
            ..Default::default()
        }
        .into(),
        data_size: Some(123),
        requested_flags: Some(u32::MAX),
        transaction_id: Some(99),
        ..Default::default()
    }
}

#[test]
fn listing_includes_history_and_preserves_explicit_cursor_zero() {
    assert_eq!(
        list_request(None, NonZeroU32::new(100).unwrap()),
        hex::decode("da02044a021864").unwrap()
    );
    let mut service =
        Smart::decode_from_slice(&list_request(Some(0), NonZeroU32::new(10).unwrap()))
            .unwrap()
            .file_access_service
            .take()
            .unwrap();
    let request = service.item_list_request.take().unwrap();
    assert_eq!(request.session_id, Some(0));
    assert_eq!(request.max_item_count, Some(10));
    assert_eq!(request.exclude_all_flags, []);
    assert_eq!(request.get_flags, []);
    assert_eq!(service.encode_to_vec(), []);
}

#[test]
fn pull_uses_typed_transport_and_drops_incoming_flags_and_extensions() {
    let mut source = item();
    // Unknown peer fields must not be echoed into an outgoing request.
    source.merge_from_slice(&[0xf8, 0x07, 1]).unwrap();
    let mut service = Smart::decode_from_slice(&pull_request(&source, 123).unwrap())
        .unwrap()
        .file_access_service
        .take()
        .unwrap();
    let request = service.pull_item_request.take().unwrap();
    assert_eq!(request.priority, Some(24));
    assert_eq!(request.offset, Some(0));
    assert_eq!(request.requested_compression_window, Some(15));
    assert_eq!(
        request.supported_transports,
        [wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE]
    );
    let selected = request.item.as_option().unwrap();
    assert_eq!(selected.uid, source.uid);
    assert_eq!(selected.data_size, Some(123));
    assert_eq!(selected.data_type, source.data_type);
    assert_eq!(selected.requested_flags, None);
    assert_eq!(selected.transaction_id, None);
    assert_eq!(
        selected.__buffa_unknown_fields,
        buffa::UnknownFields::default()
    );
    assert_eq!(service.encode_to_vec(), []);
}

#[test]
fn pull_requires_identity_type_size_and_a_bounded_advertised_file() {
    assert!(matches!(
        pull_request(&item(), 122),
        Err(FileAccessError::FileTooLarge)
    ));
    let mut source = item();
    source.uid.take();
    assert!(matches!(
        pull_request(&source, 1000),
        Err(FileAccessError::MissingField("item UID"))
    ));
    let mut source = item();
    source.data_type.take();
    assert!(matches!(
        pull_request(&source, 1000),
        Err(FileAccessError::MissingField("item type"))
    ));
    let mut source = item();
    source.data_size = None;
    assert!(matches!(
        pull_request(&source, 1000),
        Err(FileAccessError::MissingField("item size"))
    ));
}

#[test]
fn list_pull_and_transfer_status_are_decoded_without_defaulting_results() {
    let list = wire::ItemListResponse {
        items: vec![item()],
        ..Default::default()
    };
    assert_eq!(
        decode_response(&encoded(wire::Service {
            item_list_response: list.clone().into(),
            ..Default::default()
        }))
        .unwrap(),
        FileAccessResponse::List(list)
    );
    let pull = wire::PullItemDataResponse {
        transfer_handle: Some(7),
        ..Default::default()
    };
    assert_eq!(
        decode_response(&encoded(wire::Service {
            pull_item_response: pull.clone().into(),
            ..Default::default()
        }))
        .unwrap(),
        FileAccessResponse::Pull(pull)
    );
    let status = wire::TransferStatusRequest {
        transfer_handle: Some(7),
        ..Default::default()
    };
    assert_eq!(
        decode_response(&encoded(wire::Service {
            transfer_status_request: status.clone().into(),
            ..Default::default()
        }))
        .unwrap(),
        FileAccessResponse::TransferStatus(status)
    );
}

#[test]
fn oversized_malformed_unknown_and_ambiguous_messages_are_rejected() {
    assert!(matches!(
        decode_response(&vec![0; 65537]),
        Err(FileAccessError::Decode(_))
    ));
    assert!(matches!(
        decode_response(&[0xff]),
        Err(FileAccessError::Decode(_))
    ));
    assert!(matches!(
        decode_response(&[]),
        Err(FileAccessError::UnexpectedService)
    ));
    assert!(matches!(
        decode_response(&encoded(wire::Service::default())),
        Err(FileAccessError::UnexpectedOperation)
    ));
    let mut message = encoded(wire::Service {
        item_list_response: wire::ItemListResponse::default().into(),
        ..Default::default()
    });
    message.extend([8, 1]);
    assert!(matches!(
        decode_response(&message),
        Err(FileAccessError::UnexpectedService)
    ));
    let both = wire::Service {
        item_list_response: wire::ItemListResponse::default().into(),
        pull_item_response: wire::PullItemDataResponse::default().into(),
        ..Default::default()
    };
    assert!(matches!(
        decode_response(&encoded(both)),
        Err(FileAccessError::UnexpectedOperation)
    ));
    let mut unknown = wire::Service {
        pull_item_response: wire::PullItemDataResponse::default().into(),
        ..Default::default()
    };
    unknown.merge_from_slice(&[0xf8, 0x07, 1]).unwrap();
    assert!(matches!(
        decode_response(&encoded(unknown)),
        Err(FileAccessError::UnexpectedOperation)
    ));
}
