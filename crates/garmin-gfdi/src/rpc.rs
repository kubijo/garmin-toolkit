//! GFDI Protobuf fragment envelopes and bounded reassembly, independent of Bluetooth.

use std::time::Duration;

use thiserror::Error;

use crate::{Message, MessageError, encode_message};

/// Rejected Protobuf envelope or fragment sequence.
#[derive(Debug, Error)]
pub enum RpcError {
    /// A generated GFDI envelope exceeded its wire limit.
    #[error(transparent)]
    Message(#[from] MessageError),
    /// Unexpected message type or truncated/inconsistent fragment metadata.
    #[error("invalid GFDI Protobuf fragment")]
    Fragment,
    /// The configured total payload bound was exceeded.
    #[error("GFDI Protobuf payload limit exceeded")]
    Limit,
    /// A different request/type/size arrived while reassembly was active.
    #[error("GFDI Protobuf fragment changed request identity or total length")]
    Correlation,
    /// Fragments must be consecutive; exact retransmissions are acknowledged again.
    #[error("GFDI Protobuf fragment has an invalid offset or conflicting data")]
    Offset,
    /// The caller's whole-operation deadline expired.
    #[error("GFDI Protobuf reassembly deadline expired")]
    Timeout,
    /// Completion, cancellation, and errors permanently stop an instance.
    #[error("GFDI Protobuf reassembly is stopped")]
    Stopped,
}

/// One accepted incoming fragment and the GFDI acknowledgement it requires.
#[derive(Debug)]
pub struct RpcUpdate {
    /// Peer request ID; independent of transport and GFDI sequence numbers.
    pub request_id: u16,
    /// Complete GFDI ACK envelope; the caller separately COBS-frames and sends it.
    pub acknowledgement: Vec<u8>,
    /// Reassembled Protobuf bytes only after all advertised data has arrived.
    pub complete: Option<Vec<u8>>,
}

/// Reassemble one peer Protobuf message under explicit size and monotonic time limits.
/// The caller dispatches separate peer request IDs into separately bounded instances;
/// a peer request ID is not assumed to equal the preceding host request ID.
#[derive(Debug)]
pub struct RpcReassembly {
    max_bytes: usize,
    deadline: Duration,
    identity: Option<(u16, u16, usize)>,
    data: Vec<u8>,
    stopped: bool,
}

impl RpcReassembly {
    /// Start an empty bounded assembly. Deadline and timestamps share one origin.
    #[must_use]
    pub const fn new(max_bytes: usize, deadline: Duration) -> Self {
        Self {
            max_bytes,
            deadline,
            identity: None,
            data: Vec::new(),
            stopped: false,
        }
    }

    /// Accept a validated GFDI envelope of message type 5043 or 5044.
    /// No application acknowledgement is produced for malformed data.
    ///
    /// # Errors
    /// Rejects malformed, oversized, miscorrelated, reordered, expired, or stopped input.
    pub fn receive(&mut self, message: Message<'_>, now: Duration) -> Result<RpcUpdate, RpcError> {
        let result = self.receive_inner(message, now);
        if result.is_err() {
            self.cancel();
        }
        result
    }

    fn receive_inner(
        &mut self,
        message: Message<'_>,
        now: Duration,
    ) -> Result<RpcUpdate, RpcError> {
        if self.stopped {
            return Err(RpcError::Stopped);
        }
        if now >= self.deadline {
            return Err(RpcError::Timeout);
        }
        if !matches!(message.message_id(), 5043 | 5044) || message.payload().len() < 14 {
            return Err(RpcError::Fragment);
        }
        let bytes = message.payload();
        let request_id = u16::from_le_bytes([bytes[0], bytes[1]]);
        let offset = read_u32(&bytes[2..6]);
        let total = read_u32(&bytes[6..10]);
        let length = read_u32(&bytes[10..14]);
        let data = &bytes[14..];
        if total > self.max_bytes {
            return Err(RpcError::Limit);
        }
        if length != data.len()
            || offset > total
            || length > total - offset
            || (length == 0 && total != 0)
        {
            return Err(RpcError::Fragment);
        }
        let identity = (request_id, message.message_id(), total);
        if self.identity.is_some_and(|expected| expected != identity) {
            return Err(RpcError::Correlation);
        }
        if offset == self.data.len() {
            self.data.extend_from_slice(data);
        } else if self.data.get(offset..offset + length) != Some(data) {
            return Err(RpcError::Offset);
        }
        self.identity = Some(identity);
        let mut acknowledgement = message.message_id().to_le_bytes().to_vec();
        acknowledgement.push(0);
        if offset != 0 || length != total {
            acknowledgement.extend(request_id.to_le_bytes());
            acknowledgement.extend_from_slice(&bytes[2..6]);
            acknowledgement.extend([0, 0]); // KEPT, NO_ERROR.
        }
        let response_type = message
            .sequence()
            .map_or(5000, |sequence| 0x8000 | u16::from(sequence) << 8);
        let acknowledgement = encode_message(response_type, &acknowledgement)?;
        let complete = if self.data.len() == total {
            self.stopped = true;
            Some(std::mem::take(&mut self.data))
        } else {
            None
        };
        Ok(RpcUpdate {
            request_id,
            acknowledgement,
            complete,
        })
    }

    /// Stop locally and drop partial bytes; no peer cancellation is sent.
    pub fn cancel(&mut self) {
        self.stopped = true;
        self.data.clear();
    }
}

/// Wrap a complete small outgoing Protobuf request in GFDI message 5043.
/// Larger outbound messages require an application-level chunk/ACK state machine.
///
/// # Errors
/// Rejects requests above the explicit chunk bound or the GFDI length limit.
pub fn encode_rpc_request(
    request_id: u16,
    protobuf: &[u8],
    max_chunk_bytes: usize,
) -> Result<Vec<u8>, RpcError> {
    encode_rpc(5043, request_id, protobuf, max_chunk_bytes)
}

/// Wrap a complete small Protobuf response, preserving the peer's request ID.
///
/// # Errors
/// Rejects payloads above the explicit chunk bound or the GFDI length limit.
pub fn encode_rpc_response(
    request_id: u16,
    protobuf: &[u8],
    max_chunk_bytes: usize,
) -> Result<Vec<u8>, RpcError> {
    encode_rpc(5044, request_id, protobuf, max_chunk_bytes)
}

fn encode_rpc(
    kind: u16,
    request_id: u16,
    protobuf: &[u8],
    max_chunk_bytes: usize,
) -> Result<Vec<u8>, RpcError> {
    if protobuf.len() > max_chunk_bytes {
        return Err(RpcError::Limit);
    }
    let length = u32::try_from(protobuf.len()).map_err(|_| RpcError::Limit)?;
    let mut payload = request_id.to_le_bytes().to_vec();
    payload.extend(0_u32.to_le_bytes());
    payload.extend(length.to_le_bytes());
    payload.extend(length.to_le_bytes());
    payload.extend(protobuf);
    Ok(encode_message(kind, &payload)?)
}

fn read_u32(bytes: &[u8]) -> usize {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
}
