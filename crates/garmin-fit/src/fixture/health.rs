//! Independent synthetic health messages in monitoring envelopes; no device provenance is claimed.
//! Numeric wire definitions follow Garmin's global profile:
//! <https://github.com/garmin/fit-python-sdk/blob/main/garmin_fit_sdk/profile.py>.

use super::integrity::{Header, START, envelope};

/// Minimal examples of health data that the activity importer does not normalize.
#[derive(Clone, Copy, Debug)]
pub enum Health {
    Wellness,
    Sleep,
    Hrv,
}

impl Health {
    pub const ALL: [Self; 3] = [Self::Wellness, Self::Sleep, Self::Hrv];

    #[must_use]
    pub const fn file_type(self) -> u8 {
        match self {
            Self::Wellness => 15,          // monitoring_a
            Self::Sleep | Self::Hrv => 32, // monitoring_b
        }
    }

    /// Encode independently of the production parser and encoder.
    /// # Panics
    /// If the fixed payload exceeds the FIT data-size field.
    #[must_use]
    pub fn bytes(self, header: Header) -> Vec<u8> {
        // file_id: type, development manufacturer, creation time.
        let mut data = vec![0x40, 0, 0, 0, 0, 3, 0, 1, 0, 1, 2, 0x84, 4, 4, 0x86, 0];
        data.extend([self.file_type(), 255, 0]);
        data.extend(START.to_le_bytes());
        match self {
            Self::Wellness => {
                // monitoring (55): timestamp and heart_rate (27), 62 bpm.
                data.extend([0x41, 0, 0, 55, 0, 2, 253, 4, 0x86, 27, 1, 2, 1]);
                data.extend(START.to_le_bytes());
                data.push(62);
            }
            Self::Sleep => {
                // sleep_level (275): timestamp and enum level (0), light then deep.
                data.extend([0x41, 0, 0, 0x13, 1, 2, 253, 4, 0x86, 0, 1, 0, 1]);
                data.extend(START.to_le_bytes());
                data.extend([2, 1]);
                data.extend((START + 60).to_le_bytes());
                data.push(3);
            }
            Self::Hrv => {
                // hrv (78): time (0), uint16 array in milliseconds, including missing sentinel.
                data.extend([0x41, 0, 0, 78, 0, 1, 0, 6, 0x84, 1]);
                for interval in [800_u16, 810, u16::MAX] {
                    data.extend(interval.to_le_bytes());
                }
            }
        }
        envelope(header, &data)
    }
}
