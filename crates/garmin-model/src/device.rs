//! Storage metadata shared by device views and transaction checks.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageCapacity {
    Available { total_bytes: u64, free_bytes: u64 },
    Unavailable { reason: String },
}

impl StorageCapacity {
    #[must_use]
    pub fn new(total_bytes: u64, free_bytes: u64) -> Self {
        if free_bytes > total_bytes || total_bytes == u64::MAX || free_bytes == u64::MAX {
            return Self::unavailable("The device reported invalid storage totals");
        }
        Self::Available {
            total_bytes,
            free_bytes,
        }
    }

    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    /// Validated total and free bytes.
    #[must_use]
    pub const fn bytes(&self) -> Option<(u64, u64)> {
        match *self {
            Self::Available {
                total_bytes,
                free_bytes,
            } if free_bytes <= total_bytes && total_bytes != u64::MAX => {
                Some((total_bytes, free_bytes))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStorageState {
    pub id: String,
    pub label: String,
    pub capacity: StorageCapacity,
    pub writable: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStateSnapshot {
    pub storages: Vec<DeviceStorageState>,
}

#[cfg(test)]
mod tests {
    use super::{DeviceStateSnapshot, DeviceStorageState, StorageCapacity};

    #[test]
    fn device_state_round_trips_through_postcard() {
        let state = DeviceStateSnapshot {
            storages: vec![
                DeviceStorageState {
                    id: "internal".to_owned(),
                    label: "Internal storage".to_owned(),
                    capacity: StorageCapacity::new(32_000_000_000, 8_600_000_000),
                    writable: Some(true),
                },
                DeviceStorageState {
                    id: "card".to_owned(),
                    label: "Memory card".to_owned(),
                    capacity: StorageCapacity::unavailable("The mount did not report capacity"),
                    writable: None,
                },
            ],
        };

        let encoded = postcard::to_stdvec(&state).unwrap();
        let decoded = postcard::from_bytes::<DeviceStateSnapshot>(&encoded).unwrap();

        assert_eq!(decoded, state);
    }
}
