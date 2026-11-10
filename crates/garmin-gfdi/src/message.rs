//! GFDI little-endian length/type envelope and CRC-16/ARC.

use crc::{CRC_16_ARC, Crc};
use thiserror::Error;

const CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_ARC);
const OVERHEAD: usize = 6;

/// A decoded message that failed envelope validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MessageError {
    /// The length, type, and checksum fields cannot all be present.
    #[error("GFDI message is shorter than six bytes")]
    TooShort,
    /// The declared total length differs from the decoded frame length.
    #[error("GFDI message declares {declared} bytes but contains {actual}")]
    LengthMismatch {
        /// Wire length including header and checksum.
        declared: usize,
        /// Actual decoded frame length.
        actual: usize,
    },
    /// The wire checksum does not match the header and payload.
    #[error("GFDI checksum mismatch")]
    Checksum,
    /// The envelope's 16-bit total-length field cannot represent the payload.
    #[error("GFDI message exceeds the 16-bit total-length field")]
    TooLong,
}

/// A validated, borrowed GFDI envelope; unknown message types are preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message<'a> {
    raw_type: u16,
    payload: &'a [u8],
}

impl<'a> Message<'a> {
    /// Validate a COBS-decoded frame before exposing its type or payload.
    ///
    /// # Errors
    /// Rejects truncated headers, inconsistent lengths, and invalid checksums.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, MessageError> {
        if bytes.len() < OVERHEAD {
            return Err(MessageError::TooShort);
        }
        let declared = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
        if declared != bytes.len() {
            return Err(MessageError::LengthMismatch {
                declared,
                actual: bytes.len(),
            });
        }
        if CRC.checksum(bytes) != 0 {
            return Err(MessageError::Checksum);
        }
        Ok(Self {
            raw_type: u16::from_le_bytes([bytes[2], bytes[3]]),
            payload: &bytes[4..bytes.len() - 2],
        })
    }

    /// Original type field, including sequence bits when present.
    #[must_use]
    pub const fn raw_type(self) -> u16 {
        self.raw_type
    }

    /// Normalize sequenced types to their GFDI message ID.
    #[must_use]
    pub const fn message_id(self) -> u16 {
        if self.raw_type & 0x8000 != 0 {
            5000 + (self.raw_type & 0xff)
        } else {
            self.raw_type
        }
    }

    /// Seven-bit sequence carried by a sequenced type;
    /// absent on plain types.
    #[must_use]
    pub fn sequence(self) -> Option<u8> {
        (self.raw_type & 0x8000 != 0).then_some(((self.raw_type >> 8) & 0x7f) as u8)
    }

    /// Payload excluding the length, type, and checksum fields.
    #[must_use]
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }
}

/// Encode a raw type field and payload into a GFDI envelope, before COBS framing.
/// This is a wire primitive, not authorization to send any particular command.
///
/// # Errors
/// Returns [`MessageError::TooLong`] if the total length cannot fit in 16 bits.
pub fn encode_message(raw_type: u16, payload: &[u8]) -> Result<Vec<u8>, MessageError> {
    let length = payload
        .len()
        .checked_add(OVERHEAD)
        .ok_or(MessageError::TooLong)?;
    let wire_length = u16::try_from(length).map_err(|_| MessageError::TooLong)?;
    let mut bytes = Vec::with_capacity(length);
    bytes.extend(wire_length.to_le_bytes());
    bytes.extend(raw_type.to_le_bytes());
    bytes.extend(payload);
    bytes.extend(CRC.checksum(&bytes).to_le_bytes());
    Ok(bytes)
}
