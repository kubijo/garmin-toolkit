//! Independent developer-field cases shadowing muktihari/fit's metadata regressions.
//! No upstream bytes or encoder code are used.
//! <https://github.com/muktihari/fit/blob/7addcbcbb7a3d99e88db5568bf691308ff4d66cb/decoder/decoder_test.go>

use super::integrity;
use super::integrity::{Header, START, envelope};

/// Metadata available for a custom uint32 record field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Metadata {
    Plain,
    Described,
    MissingDescription,
    MissingDeveloperId,
}

impl Metadata {
    pub const ALL: [Self; 4] = [
        Self::Plain,
        Self::Described,
        Self::MissingDescription,
        Self::MissingDeveloperId,
    ];
}

/// Append a native record with a conflicting custom distance value to a synthetic activity.
/// `Plain` provides the same native record without custom data as a semantic control.
/// # Panics
/// If the fixed activity scaffold is truncated or the payload exceeds the FIT data-size field.
#[must_use]
pub fn activity(header: Header, metadata: Metadata) -> Vec<u8> {
    let base = integrity::activity(header);
    let mut data = base[usize::from(header.size())..base.len() - 2].to_vec();
    if matches!(metadata, Metadata::Described | Metadata::MissingDescription) {
        // developer_data_id (207): synthetic application ID and developer index 0.
        data.extend([0x45, 0, 0, 207, 0, 2, 1, 16, 2, 3, 1, 2, 5]);
        data.extend([0xA5; 16]);
        data.push(0);
    }
    if matches!(metadata, Metadata::Described | Metadata::MissingDeveloperId) {
        // field_description (206): index, field number 5, uint32, name, native record distance.
        data.extend([
            0x46, 0, 0, 206, 0, 6, 0, 1, 2, 1, 1, 2, 2, 1, 2, 3, 7, 7, 14, 2, 0x84, 15, 1, 2, 6, 0,
            5, 0x86,
        ]);
        data.extend(b"custom\0");
        data.extend([20, 0, 5]);
    }
    // record local 7: native timestamp and distance, optionally one custom field.
    let definition = if metadata == Metadata::Plain {
        0x47
    } else {
        0x67
    };
    data.extend([definition, 0, 0, 20, 0, 2, 253, 4, 0x86, 5, 4, 0x86]);
    if metadata != Metadata::Plain {
        data.extend([1, 5, 4, 0]);
    }
    data.push(7);
    data.extend((START + 30).to_le_bytes());
    data.extend(50_000_u32.to_le_bytes());
    if metadata != Metadata::Plain {
        data.extend(9_999_999_u32.to_le_bytes());
    }
    envelope(header, &data)
}
