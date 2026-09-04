//! Per-storage peak demand in transaction order.

use std::{collections::BTreeMap, path::Path};

use garmin_device::{DeviceStateSnapshot, StorageCapacity, filesystem_capacity};
use serde::Serialize;
use thiserror::Error;

/// A replacement deletes its original before writing the new object.
pub struct StorageChange<'a> {
    pub storage_id: &'a str,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageSpaceRequirement {
    pub storage_id: String,
    pub required_free_bytes: u64,
}

/// Computes peak additional bytes without crediting later removals.
/// # Errors
/// Byte counts exceed the supported range.
pub fn storage_requirements<'a>(
    changes: impl IntoIterator<Item = StorageChange<'a>>,
) -> Result<Vec<StorageSpaceRequirement>, SpaceError> {
    let mut usage = BTreeMap::<String, (i128, i128)>::new();
    for change in changes {
        let (current, peak) = usage.entry(change.storage_id.to_owned()).or_default();
        *current = current
            .checked_sub(i128::from(change.before_bytes))
            .and_then(|value| value.checked_add(i128::from(change.after_bytes)))
            .ok_or(SpaceError::Overflow)?;
        *peak = (*peak).max(*current);
    }
    usage
        .into_iter()
        .map(|(storage_id, (_, peak))| {
            Ok(StorageSpaceRequirement {
                storage_id,
                required_free_bytes: u64::try_from(peak).map_err(|_| SpaceError::Overflow)?,
            })
        })
        .collect()
}

/// Checks capacity for each affected storage.
/// # Errors
/// Any affected volume cannot satisfy its own peak demand.
pub fn check_device_space(
    snapshot: &DeviceStateSnapshot,
    requirements: &[StorageSpaceRequirement],
) -> Result<(), SpaceError> {
    for requirement in requirements {
        let mut matches = snapshot
            .storages
            .iter()
            .filter(|storage| storage.id == requirement.storage_id);
        let storage = matches.next().ok_or_else(|| SpaceError::Unavailable {
            label: requirement.storage_id.clone(),
            reason: "Storage is missing".to_owned(),
        })?;
        if matches.next().is_some() {
            return Err(SpaceError::Unavailable {
                label: storage.label.clone(),
                reason: "Storage identity is ambiguous".to_owned(),
            });
        }
        if storage.writable == Some(false) {
            return Err(SpaceError::ReadOnly(storage.label.clone()));
        }
        check_capacity(
            &storage.label,
            &storage.capacity,
            requirement.required_free_bytes,
        )?;
    }
    Ok(())
}

/// Checks that each target storage is writable.
/// # Errors
/// An affected storage cannot be identified or reports read-only access.
pub fn check_device_writable<'a>(
    snapshot: &DeviceStateSnapshot,
    storage_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), SpaceError> {
    for storage_id in storage_ids {
        let mut matches = snapshot
            .storages
            .iter()
            .filter(|storage| storage.id == storage_id);
        let storage = matches.next().ok_or_else(|| SpaceError::Unavailable {
            label: storage_id.to_owned(),
            reason: "Storage is missing".to_owned(),
        })?;
        if matches.next().is_some() {
            return Err(SpaceError::Unavailable {
                label: storage.label.clone(),
                reason: "Storage identity is ambiguous".to_owned(),
            });
        }
        if storage.writable == Some(false) {
            return Err(SpaceError::ReadOnly(storage.label.clone()));
        }
    }
    Ok(())
}

fn check_capacity(
    label: &str,
    capacity: &StorageCapacity,
    required: u64,
) -> Result<(), SpaceError> {
    let Some((_, available)) = capacity.bytes() else {
        return Err(SpaceError::Unavailable {
            label: label.to_owned(),
            reason: match capacity {
                StorageCapacity::Unavailable { reason } => reason.clone(),
                StorageCapacity::Available { .. } => "Inconsistent capacity".to_owned(),
            },
        });
    };
    if required > available {
        return Err(SpaceError::Insufficient {
            label: label.to_owned(),
            required,
            available,
        });
    }
    Ok(())
}

/// Checks host space with transaction headroom.
/// # Errors
/// Host capacity is unknown or insufficient.
pub async fn check_host_space(path: &Path, bytes: u64) -> Result<(), SpaceError> {
    let required = bytes.checked_add(1024 * 1024).ok_or(SpaceError::Overflow)?;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        check_capacity(
            &format!("Host storage at {}", path.display()),
            &filesystem_capacity(&path),
            required,
        )
    })
    .await
    .map_err(|error| SpaceError::Unavailable {
        label: "Host storage".to_owned(),
        reason: error.to_string(),
    })?
}

fn size(bytes: u64) -> String {
    format!(
        "{:.2}",
        byte_unit::Byte::from_u64(bytes).get_appropriate_unit(byte_unit::UnitType::Decimal)
    )
}

#[derive(Debug, Error)]
pub enum SpaceError {
    #[error("{label} needs {}; only {} is free", size(*required), size(*available))]
    Insufficient {
        label: String,
        required: u64,
        available: u64,
    },
    #[error("{0} is read-only")]
    ReadOnly(String),
    #[error("Cannot determine free space on {label}: {reason}; writing is blocked")]
    Unavailable { label: String, reason: String },
    #[error("storage byte count overflow")]
    Overflow,
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_device::DeviceStorageState;

    #[test]
    fn peak_demand_preserves_write_order_and_storage_boundaries() {
        let changes = [("a", 100, 20), ("b", 0, 90), ("a", 0, 140), ("a", 80, 20)];
        let requirements = storage_requirements(changes.into_iter().map(
            |(storage_id, before_bytes, after_bytes)| StorageChange {
                storage_id,
                before_bytes,
                after_bytes,
            },
        ))
        .unwrap();
        assert_eq!(
            requirements
                .iter()
                .map(|r| (r.storage_id.as_str(), r.required_free_bytes))
                .collect::<Vec<_>>(),
            [("a", 60), ("b", 90)]
        );
        let mut snapshot = DeviceStateSnapshot {
            storages: vec![
                DeviceStorageState {
                    id: "a".to_owned(),
                    label: "Internal".to_owned(),
                    capacity: StorageCapacity::new(1000, 60),
                    writable: Some(true),
                },
                DeviceStorageState {
                    id: "b".to_owned(),
                    label: "Card".to_owned(),
                    capacity: StorageCapacity::new(1000, 89),
                    writable: Some(true),
                },
            ],
        };
        assert!(matches!(
            check_device_space(&snapshot, &requirements),
            Err(SpaceError::Insufficient {
                required: 90,
                available: 89,
                ..
            })
        ));
        snapshot.storages[1].capacity = StorageCapacity::new(1000, 90);
        check_device_space(&snapshot, &requirements).unwrap();
        snapshot.storages[1].writable = Some(false);
        assert!(matches!(
            check_device_space(&snapshot, &requirements),
            Err(SpaceError::ReadOnly(_))
        ));
        snapshot.storages[1].writable = None;
        snapshot.storages[1].capacity = StorageCapacity::new(100, 101);
        assert!(matches!(
            check_device_space(&snapshot, &requirements),
            Err(SpaceError::Unavailable { .. })
        ));
        snapshot.storages.push(snapshot.storages[0].clone());
        assert!(matches!(
            check_device_space(&snapshot, &requirements),
            Err(SpaceError::Unavailable { .. })
        ));
    }
}
