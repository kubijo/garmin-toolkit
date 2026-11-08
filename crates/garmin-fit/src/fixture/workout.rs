//! Independently encoded workout following Garmin's required message order:
//! <https://developer.garmin.com/fit/file-types/workout>.

use super::integrity::{Header, START, envelope};

/// A two-step running workout: 60 seconds active, 30 seconds rest, both with open targets.
/// # Panics
/// If the fixed payload exceeds the FIT data-size field.
#[must_use]
pub fn bytes(header: Header) -> Vec<u8> {
    // file_id: workout type (5), fake manufacturer (255), product (1), serial (1), creation time.
    let mut data = vec![
        0x40, 0, 0, 0, 0, 5, 0, 1, 0, 1, 2, 0x84, 2, 2, 0x84, 3, 4, 0x8C, 4, 4, 0x86, 0, 5, 255, 0,
        1, 0,
    ];
    data.extend(1_u32.to_le_bytes());
    data.extend(START.to_le_bytes());
    // workout (26): running sport (4), step count (6), name (8).
    data.extend([
        0x41, 0, 0, 26, 0, 3, 4, 1, 0, 6, 2, 0x84, 8, 5, 7, 1, 1, 2, 0,
    ]);
    data.extend(b"Test\0");
    // workout_step (27): index, duration type/value, target type/value, intensity.
    data.extend([
        0x42, 0, 0, 27, 0, 6, 254, 2, 0x84, 1, 1, 0, 2, 4, 0x86, 3, 1, 0, 4, 4, 0x86, 7, 1, 0,
    ]);
    for (index, duration, intensity) in [(0_u16, 60_000_u32, 0), (1, 30_000, 1)] {
        data.push(2);
        data.extend(index.to_le_bytes());
        data.push(0); // timed duration in milliseconds
        data.extend(duration.to_le_bytes());
        data.push(2); // open target
        data.extend(0_u32.to_le_bytes());
        data.push(intensity);
    }
    envelope(header, &data)
}
