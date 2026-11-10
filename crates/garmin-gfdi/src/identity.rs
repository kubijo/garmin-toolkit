//! Device-initiated GFDI message 5024; adapted from Gadgetbridge `DeviceInformationMessage`.

use thiserror::Error;

use crate::{Message, MessageError, encode_frame, encode_message};

pub(super) fn host_information(
    request: Message<'_>,
    identity: &DeviceInformation,
) -> Result<Vec<u8>, MessageError> {
    let mut host = 5024_u16.to_le_bytes().to_vec();
    host.push(0);
    host.extend(150_u16.to_le_bytes());
    host.extend(u16::MAX.to_le_bytes());
    host.extend(u32::MAX.to_le_bytes());
    host.extend(0_u16.to_le_bytes());
    host.extend(u16::MAX.to_le_bytes());
    for (length, name) in [(14, "Garmin Toolkit"), (7, "Toolkit"), (5, "Linux")] {
        host.push(length);
        host.extend(name.as_bytes());
    }
    host.push(u8::from(identity.protocol_version / 100 == 1));
    let kind = request
        .sequence()
        .map_or(5000, |sequence| 0x8000 | u16::from(sequence) << 8);
    Ok(encode_frame(&encode_message(kind, &host)?))
}

/// Invalid or incomplete device-information fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum IdentityError {
    /// A fixed field or length-prefixed string extends past the payload.
    #[error("truncated GFDI device information")]
    Truncated,
    /// Names are UTF-8, as specified by the upstream reader.
    #[error("invalid UTF-8 in GFDI device information")]
    InvalidText,
}

/// Device-provided identity, without any host registration response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInformation {
    /// Raw device protocol version.
    pub protocol_version: u16,
    /// Garmin product number.
    pub product_number: u16,
    /// Device-specific unit identifier; keep it out of publishable fixtures.
    pub unit_number: u32,
    /// Firmware version in hundredths.
    pub software_version: u16,
    /// Device-reported GFDI packet size, distinct from the ATT MTU.
    pub max_packet_size: u16,
    /// Device-reported Bluetooth name.
    pub bluetooth_name: String,
    /// Device-reported name.
    pub device_name: String,
    /// Device-reported model.
    pub model: String,
    /// Uninterpreted trailing fields, which may include addresses.
    pub extension: Vec<u8>,
}

impl DeviceInformation {
    /// Decode the payload of a validated GFDI message with ID 5024.
    ///
    /// # Errors
    /// Rejects truncated fixed fields, truncated strings, and invalid UTF-8.
    pub fn decode(payload: &[u8]) -> Result<Self, IdentityError> {
        let fixed = payload.get(..12).ok_or(IdentityError::Truncated)?;
        let mut remainder = &payload[12..];
        let bluetooth_name = take_text(&mut remainder)?;
        let device_name = take_text(&mut remainder)?;
        let model = take_text(&mut remainder)?;
        Ok(Self {
            protocol_version: u16::from_le_bytes([fixed[0], fixed[1]]),
            product_number: u16::from_le_bytes([fixed[2], fixed[3]]),
            unit_number: u32::from_le_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]),
            software_version: u16::from_le_bytes([fixed[8], fixed[9]]),
            max_packet_size: u16::from_le_bytes([fixed[10], fixed[11]]),
            bluetooth_name,
            device_name,
            model,
            extension: remainder.to_vec(),
        })
    }
}

fn take_text(input: &mut &[u8]) -> Result<String, IdentityError> {
    let (&length, rest) = input.split_first().ok_or(IdentityError::Truncated)?;
    let bytes = rest
        .get(..usize::from(length))
        .ok_or(IdentityError::Truncated)?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| IdentityError::InvalidText)?
        .to_owned();
    *input = &rest[usize::from(length)..];
    Ok(text)
}
