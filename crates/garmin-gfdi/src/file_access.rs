//! Typed `FileAccess` requests and bounded responses; no Bluetooth or file-index fallback.

use std::num::NonZeroU32;

use buffa::{DecodeOptions, Message as _};
use thiserror::Error;

use crate::proto::{Smart, data_types, file_access as wire};

/// Rejected `FileAccess` input.
#[derive(Debug, Error)]
pub enum FileAccessError {
    /// Protobuf decoding failed, including configured allocation/recursion limits.
    #[error(transparent)]
    Decode(#[from] buffa::DecodeError),
    /// An operation requires an explicitly present field, not its default value.
    #[error("FileAccess requires {0}")]
    MissingField(&'static str),
    /// An advertised file exceeds the caller's explicit retained-size bound.
    #[error("FileAccess item size exceeds the download limit")]
    FileTooLarge,
    /// The envelope contains another `Smart` service or has no `FileAccess` service.
    #[error("unexpected Protobuf service")]
    UnexpectedService,
    /// Multiple, unknown, or unsupported `FileAccess` operations were present.
    #[error("unexpected FileAccess operation")]
    UnexpectedOperation,
}

/// A recognized incoming `FileAccess` operation. Missing/unknown status stays absent.
#[derive(Debug, PartialEq)]
pub enum FileAccessResponse {
    /// A page of item references, with cursor and continuation metadata.
    List(wire::ItemListResponse),
    /// A pull result and transfer-pipe handle.
    Pull(wire::PullItemDataResponse),
    /// Peer-reported transfer completion, cancellation, or failure.
    TransferStatus(wire::TransferStatusRequest),
}

/// Build an unfiltered history listing or continuation within a supplied cursor.
/// No synced-item exclusion, persistence flags, or mutation request is generated.
#[must_use]
pub fn list_request(session_id: Option<u32>, max_items: NonZeroU32) -> Vec<u8> {
    envelope(wire::Service {
        item_list_request: wire::ItemListRequest {
            session_id,
            max_item_count: Some(max_items.get()),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    })
}

/// Request one explicitly selected item over a `MultiLink` transfer pipe at offset zero.
/// Echo only known identity/type/size fields, dropping response flags and unknown fields.
/// Compression is negotiated with a fifteen-bit window; callers must validate the response.
///
/// # Errors
/// Rejects missing identity/type/size fields and advertised sizes beyond `max_file_bytes`.
pub fn pull_request(
    item: &wire::ItemReference,
    max_file_bytes: u64,
) -> Result<Vec<u8>, FileAccessError> {
    let uid = item
        .uid
        .as_option()
        .ok_or(FileAccessError::MissingField("item UID"))?;
    let kind = item
        .data_type
        .as_option()
        .ok_or(FileAccessError::MissingField("item type"))?;
    let size = item
        .data_size
        .ok_or(FileAccessError::MissingField("item size"))?;
    if size > max_file_bytes {
        return Err(FileAccessError::FileTooLarge);
    }
    let item = wire::ItemReference {
        uid: data_types::UUID {
            most_significant: uid.most_significant,
            least_significant: uid.least_significant,
            ..Default::default()
        }
        .into(),
        data_type: wire::DataType {
            format: kind.format,
            gdxml_string: kind.gdxml_string.clone(),
            gdxml_string_key: kind.gdxml_string_key,
            gdxml_file_identifier: kind.gdxml_file_identifier.clone(),
            software_update_path: kind.software_update_path.clone(),
            special_case: kind.special_case,
            update_part_number: kind.update_part_number.clone(),
            ..Default::default()
        }
        .into(),
        data_size: Some(size),
        ..Default::default()
    };
    Ok(envelope(wire::Service {
        pull_item_request: wire::PullItemDataRequest {
            item: item.into(),
            priority: Some(24),
            supported_transports: vec![wire::TransportProtocol::MULTILINK_TRANSPORT_PIPE],
            offset: Some(0),
            requested_compression_window: Some(15),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    }))
}

/// Decode one complete, reassembled `Smart` message with explicit hostile-input bounds.
/// This does not reassemble GFDI fragments or correlate request IDs.
///
/// # Errors
/// Rejects messages over 64 KiB, excessive allocation/recursion/unknown fields, and
/// envelopes containing unsupported or ambiguous operations.
pub fn decode_response(bytes: &[u8]) -> Result<FileAccessResponse, FileAccessError> {
    let mut smart: Smart = DecodeOptions::new()
        .with_max_message_size(64 * 1024)
        .with_element_memory_limit(1024 * 1024)
        .with_recursion_limit(16)
        .with_unknown_field_limit(256)
        .decode_from_slice(bytes)?;
    let mut service = smart
        .file_access_service
        .take()
        .ok_or(FileAccessError::UnexpectedService)?;
    if smart.encoded_len() != 0 {
        return Err(FileAccessError::UnexpectedService);
    }
    let response = if let Some(page) = service.item_list_response.take() {
        FileAccessResponse::List(page)
    } else if let Some(result) = service.pull_item_response.take() {
        FileAccessResponse::Pull(result)
    } else if let Some(status) = service.transfer_status_request.take() {
        FileAccessResponse::TransferStatus(status)
    } else {
        return Err(FileAccessError::UnexpectedOperation);
    };
    // MessageField equality treats an explicitly present default message as unset.
    // Encoded size preserves presence, including empty unsupported operations.
    if service.encoded_len() != 0 {
        return Err(FileAccessError::UnexpectedOperation);
    }
    Ok(response)
}

fn envelope(service: wire::Service) -> Vec<u8> {
    Smart {
        file_access_service: service.into(),
        ..Default::default()
    }
    .encode_to_vec()
}

pub(crate) fn transfer_status_response() -> Vec<u8> {
    envelope(wire::Service {
        transfer_status_response: wire::TransferStatusResponse {
            next_transfer_priority: Some(1),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    })
}
