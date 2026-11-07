//! Independent FIT wire fixtures, without the parser's encoder or CRC implementation.
//!
//! Layout: <https://developer.garmin.com/fit/articles/fit-protocol/fit_protocol.html>.
//! The fake recording is a 30-second, 500-metre run with one lap and one track point.

/// Synthetic FIT timestamp (seconds since the FIT epoch).
pub const START: u32 = 1_100_000_000;

/// Header forms permitted by the FIT protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Header {
    Legacy,
    Checksummed,
    ZeroCrc,
}

impl Header {
    pub const ALL: [Self; 3] = [Self::Legacy, Self::Checksummed, Self::ZeroCrc];

    #[must_use]
    pub const fn size(self) -> u8 {
        match self {
            Self::Legacy => 12,
            Self::Checksummed | Self::ZeroCrc => 14,
        }
    }
}

/// Encode a minimal activity directly from message and field numbers in the global profile.
/// # Panics
/// If the fixed recording exceeds the FIT data-size field.
#[must_use]
pub fn activity(header: Header) -> Vec<u8> {
    // file_id (0): activity type (0), development manufacturer (1), creation time (4).
    let mut data = vec![
        0x40, 0, 0, 0, 0, 3, 0, 1, 0, 1, 2, 0x84, 4, 4, 0x86, 0, 4, 255, 0,
    ];
    data.extend(START.to_le_bytes());
    summary(&mut data, 1, 18, true); // session
    summary(&mut data, 2, 19, false); // lap
    // record (20): timestamp (253), distance (5), uint32 centimetres.
    data.extend([0x43, 0, 0, 20, 0, 2, 253, 4, 0x86, 5, 4, 0x86, 3]);
    data.extend((START + 30).to_le_bytes());
    data.extend(50_000_u32.to_le_bytes());
    // activity (34): timestamp (253), total timer milliseconds (0), session count (1).
    data.extend([
        0x44, 0, 0, 34, 0, 3, 253, 4, 0x86, 0, 4, 0x86, 1, 2, 0x84, 4,
    ]);
    data.extend((START + 30).to_le_bytes());
    data.extend(30_000_u32.to_le_bytes());
    data.extend(1_u16.to_le_bytes());

    let mut bytes = vec![header.size(), 0x20, 0, 8]; // protocol 2.0, profile 20.48
    bytes.extend(
        u32::try_from(data.len())
            .expect("fixed synthetic recording fits u32")
            .to_le_bytes(),
    );
    bytes.extend(b".FIT");
    if header != Header::Legacy {
        let checksum = if header == Header::Checksummed {
            crc(&bytes)
        } else {
            0
        };
        bytes.extend(checksum.to_le_bytes());
    }
    bytes.extend(data);
    let checksum = crc(&bytes);
    bytes.extend(checksum.to_le_bytes());
    bytes
}

fn summary(data: &mut Vec<u8>, local: u8, global: u16, sport: bool) {
    data.extend([0x40 | local, 0, 0]);
    data.extend(global.to_le_bytes());
    data.push(if sport { 6 } else { 5 });
    // timestamp, start_time, elapsed milliseconds, timer milliseconds, distance centimetres.
    for field in [253, 2, 7, 8, 9] {
        data.extend([field, 4, 0x86]);
    }
    if sport {
        data.extend([5, 1, 0]);
    }
    data.push(local);
    for value in [START + 30, START, 30_000, 30_000, 50_000] {
        data.extend(value.to_le_bytes());
    }
    if sport {
        data.push(1); // running
    }
}

/// Every proper prefix, every single-bit mutation, and independently repaired integrity failures.
/// # Panics
/// If the fixed fixture's layout or checksum invariants no longer hold.
#[must_use]
pub fn corruptions(header: Header) -> Vec<(String, Vec<u8>)> {
    let bytes = activity(header);
    let mut cases = Vec::new();
    for length in 0..bytes.len() {
        cases.push((format!("truncated at {length}"), bytes[..length].to_vec()));
    }
    for offset in 0..bytes.len() {
        for bit in 0..8 {
            let mut changed = bytes.clone();
            changed[offset] ^= 1 << bit;
            cases.push((format!("byte {offset}, bit {bit}"), changed));
        }
    }
    if header == Header::Checksummed {
        for bit in 0..16 {
            let mut changed = bytes.clone();
            changed[12 + bit / 8] ^= 1 << (bit % 8);
            // Zero is a legal omitted header checksum, not a corrupt header checksum.
            if changed[12..14] != [0, 0] {
                repair_file_crc(&mut changed);
                cases.push((format!("header CRC bit {bit}, repaired file CRC"), changed));
            }
        }
    }
    if header == Header::ZeroCrc {
        let mut changed = bytes.clone();
        let end = changed.len() - 2;
        let wrong = crc(&changed[usize::from(header.size())..end]);
        changed[end..].copy_from_slice(&wrong.to_le_bytes());
        assert_ne!(
            crc(&changed),
            0,
            "fixture must distinguish body and file CRC"
        );
        cases.push(("body-only CRC with zero header CRC".to_owned(), changed));
    }
    for delta in [-1_i32, 1] {
        let changed = misdeclared_length(&bytes, delta);
        cases.push((
            format!("declared data length {delta:+}, repaired CRCs"),
            changed,
        ));
    }
    cases
}

pub(crate) fn misdeclared_length(bytes: &[u8], delta: i32) -> Vec<u8> {
    let mut changed = bytes.to_vec();
    let size = u32::from_le_bytes(changed[4..8].try_into().expect("fixed data-size field"));
    changed[4..8].copy_from_slice(
        &size
            .checked_add_signed(delta)
            .expect("small fixed fixture")
            .to_le_bytes(),
    );
    if changed[0] == 14 && changed[12..14] != [0, 0] {
        let checksum = crc(&changed[..12]);
        changed[12..14].copy_from_slice(&checksum.to_le_bytes());
    }
    repair_file_crc(&mut changed);
    changed
}

fn repair_file_crc(bytes: &mut [u8]) {
    let end = bytes.len() - 2;
    let checksum = crc(&bytes[..end]);
    bytes[end..].copy_from_slice(&checksum.to_le_bytes());
}

// Bitwise CRC-16/ARC (initial zero, reflected polynomial 0xA001), independent of rustyfit.
fn crc(bytes: &[u8]) -> u16 {
    let mut value = 0_u16;
    for byte in bytes {
        value ^= u16::from(*byte);
        for _ in 0..8 {
            value = if value & 1 == 0 {
                value >> 1
            } else {
                (value >> 1) ^ 0xA001
            };
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::{Header, activity, crc};

    #[test]
    fn checksum_matches_a_known_vector_and_complete_files_have_zero_residue() {
        assert_eq!(crc(b"123456789"), 0xBB3D);
        for header in Header::ALL {
            let bytes = activity(header);
            assert_eq!(crc(&bytes), 0, "{header:?}");
            if header == Header::Checksummed {
                assert_eq!(crc(&bytes[..14]), 0);
            }
        }
    }
}
