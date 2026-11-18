use garmin_model::artifact::ByteCount;
use std::path::Path;

pub use garmin_model::device::{DeviceStateSnapshot, DeviceStorageState, StorageCapacity};

/// Query filesystem metadata without walking its files.
#[must_use]
pub fn filesystem_capacity(path: &Path) -> StorageCapacity {
    match (fs2::total_space(path), fs2::available_space(path)) {
        (Ok(total), Ok(free)) => {
            StorageCapacity::new(ByteCount::from_bytes(total), ByteCount::from_bytes(free))
        }
        (Err(error), _) | (_, Err(error)) => StorageCapacity::unavailable(error.to_string()),
    }
}
