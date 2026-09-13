//! Disposable directory-backed Garmin device used by demo deployments.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};

use garmin_device::{
    DeviceCatalog, DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogStorage,
    DeviceStateSnapshot, DeviceStorageState, SafeRelativePath, StorageCapacity,
    attachments::{Capability, Metadata},
    storage::DirectoryDevice,
};
use garmin_fit::fixture::ActivityCase;
use thiserror::Error;

pub const KEY: &str = "demo:watch-o-matic-9000";
pub const NAME: &str = "Mock Watch-o-Matic 9000";
pub const STORAGE_ID: &str = "internal";
pub const STORAGE_LABEL: &str = "Mock internal storage";
const MAX_DEPTH: usize = 32;
const MAX_ENTRIES: usize = 4_096;
const ROOT_MARKER: &str = ".garmin-toolkit-demo-device";
const ROOT_MARKER_CONTENTS: &[u8] = b"garmin-toolkit demo device v1\n";

/// A safely owned fake device tree.
#[derive(Clone, Debug)]
pub struct Device {
    root: PathBuf,
}

impl Device {
    /// Recreates the complete disposable device tree.
    /// # Errors
    /// The target cannot be safely replaced or the fixture cannot be encoded or written.
    pub fn recreate(root: PathBuf) -> Result<Self, DeviceError> {
        reset(&root)?;
        for directory in [
            "Garmin/Activity/History/2026",
            "Garmin/Courses",
            "Garmin/Workouts",
            "GARMIN-TOOLKIT/transactions",
            "Music",
            "Podcasts",
        ] {
            let path = root.join(directory);
            std::fs::create_dir_all(&path).map_err(|source| io_error("create", path, source))?;
        }
        let activity = root.join("Garmin/Activity/History/2026/made-up-morning-ride.fit");
        std::fs::write(&activity, ActivityCase::RecoveryRide.encode()?)
            .map_err(|source| io_error("write", activity, source))?;
        let description = root.join("Garmin/GarminDevice.xml");
        std::fs::write(
            &description,
            br#"<?xml version="1.0" encoding="UTF-8"?><Device><Description>Mock Watch-o-Matic 9000</Description></Device>"#,
        )
        .map_err(|source| io_error("write", description, source))?;
        Ok(Self { root })
    }

    /// Whether the backing directory is still attached.
    /// # Errors
    /// A present path is not the fixture-owned directory.
    pub fn presence(&self) -> Result<Presence, DeviceError> {
        inspect_root(&self.root)
    }

    /// Takes a bounded catalog snapshot of the current tree.
    /// # Errors
    /// The device is detached, unsafe, ambiguous, unreadable, or exceeds its bounds.
    pub fn catalog(&self) -> Result<DeviceCatalog, String> {
        scan_catalog(&self.root)
    }

    #[must_use]
    pub fn transport(&self) -> DirectoryDevice {
        DirectoryDevice::with_storage(self.root.clone(), STORAGE_ID, STORAGE_LABEL)
    }
}

#[must_use]
pub fn state() -> DeviceStateSnapshot {
    DeviceStateSnapshot {
        storages: vec![DeviceStorageState {
            id: STORAGE_ID.to_owned(),
            label: "Internal storage".to_owned(),
            capacity: StorageCapacity::new(32_000_000_000, 8_600_000_000),
            writable: Some(true),
        }],
    }
}

#[must_use]
pub fn metadata() -> Metadata {
    Metadata {
        id: garmin_device::DeviceId::from_u32(42_530_200),
        name: NAME.to_owned(),
        software_version: garmin_device::SoftwareVersion::from_hundredths(1_870),
        capabilities: vec![
            Capability::new(
                garmin_device::DataType::Activity,
                garmin_device::TransferDirection::OutputFromUnit,
            ),
            Capability::new(
                garmin_device::DataType::Workout,
                garmin_device::TransferDirection::InputOutput,
            ),
            Capability::new(
                garmin_device::DataType::Course,
                garmin_device::TransferDirection::InputOutput,
            ),
        ],
        storage: state(),
    }
}

/// Presence of the directory-backed attachment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Presence {
    Missing,
    Present,
}

/// Failure while managing the disposable device tree.
#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("could not {operation} mock device path {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Encoding(#[from] garmin_fit::fixture::EncodingError),
    #[error("refusing to replace unowned mock device root {}: {reason}", path.display())]
    UnownedRoot { path: PathBuf, reason: &'static str },
}

fn reset(root: &Path) -> Result<(), DeviceError> {
    if inspect_root(root)? == Presence::Present {
        std::fs::remove_dir_all(root)
            .map_err(|source| io_error("remove", root.to_owned(), source))?;
    }
    std::fs::create_dir(root).map_err(|source| io_error("create", root.to_owned(), source))?;
    let marker = root.join(ROOT_MARKER);
    std::fs::write(&marker, ROOT_MARKER_CONTENTS)
        .map_err(|source| io_error("write", marker, source))
}

fn inspect_root(root: &Path) -> Result<Presence, DeviceError> {
    match std::fs::symlink_metadata(root) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Presence::Missing),
        Err(source) => return Err(io_error("inspect", root.to_owned(), source)),
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(DeviceError::UnownedRoot {
                path: root.to_owned(),
                reason: "it is not a directory",
            });
        }
    }

    let marker = root.join(ROOT_MARKER);
    match std::fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(DeviceError::UnownedRoot {
                path: root.to_owned(),
                reason: "its ownership marker is not a regular file",
            });
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(DeviceError::UnownedRoot {
                path: root.to_owned(),
                reason: "its ownership marker is missing",
            });
        }
        Err(source) => return Err(io_error("inspect", marker, source)),
    }
    let contents = std::fs::read(&marker).map_err(|source| io_error("read", marker, source))?;
    if contents != ROOT_MARKER_CONTENTS {
        return Err(DeviceError::UnownedRoot {
            path: root.to_owned(),
            reason: "its ownership marker is invalid",
        });
    }
    Ok(Presence::Present)
}

fn scan_catalog(root: &Path) -> Result<DeviceCatalog, String> {
    match inspect_root(root).map_err(|error| error.to_string())? {
        Presence::Present => {}
        Presence::Missing => return Err("the mock device is unavailable".to_owned()),
    }
    let mut pending = vec![(root.to_owned(), PathBuf::new(), 0_usize)];
    let mut remaining = MAX_ENTRIES;
    let mut entries = Vec::new();
    while let Some((directory, parent, depth)) = pending.pop() {
        let mut children = std::fs::read_dir(directory)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        children.sort_by_key(std::fs::DirEntry::file_name);
        let mut names = BTreeSet::new();
        for child in children {
            if parent.as_os_str().is_empty() && child.file_name() == OsStr::new(ROOT_MARKER) {
                continue;
            }
            if remaining == 0 {
                return Err(format!(
                    "mock device browser exceeded its {MAX_ENTRIES}-entry limit"
                ));
            }
            remaining -= 1;
            let name = child
                .file_name()
                .into_string()
                .map_err(|_| "mock device browser found a non-Unicode filename".to_owned())?;
            if !names.insert(name.to_lowercase()) {
                return Err(format!(
                    "mock device directory {} is ambiguous",
                    parent.display()
                ));
            }
            let relative = parent.join(&name);
            let safe = SafeRelativePath::parse(&relative).map_err(|error| error.to_string())?;
            let metadata =
                std::fs::symlink_metadata(child.path()).map_err(|error| error.to_string())?;
            let (kind, size) = if metadata.file_type().is_dir() {
                if depth >= MAX_DEPTH {
                    return Err(format!(
                        "mock device browser exceeded its {MAX_DEPTH}-level depth limit"
                    ));
                }
                pending.push((child.path(), relative, depth + 1));
                (DeviceCatalogEntryKind::Directory, None)
            } else if metadata.file_type().is_file() {
                (DeviceCatalogEntryKind::File, Some(metadata.len()))
            } else {
                continue;
            };
            entries.push(DeviceCatalogEntry {
                path: safe.into_utf8_path_buf(),
                kind,
                size,
            });
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(DeviceCatalog {
        storages: vec![DeviceCatalogStorage {
            id: STORAGE_ID.to_owned(),
            label: STORAGE_LABEL.to_owned(),
            entries,
        }],
    })
}

fn io_error(operation: &'static str, path: PathBuf, source: io::Error) -> DeviceError {
    DeviceError::Io {
        operation,
        path,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_start_recreates_the_complete_demo_device() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("device");
        Device::recreate(root.clone()).unwrap();
        std::fs::write(root.join("Music/uploaded.mp3"), b"previous run").unwrap();
        std::fs::write(root.join("Garmin/GarminDevice.xml"), b"modified").unwrap();

        let device = Device::recreate(root.clone()).unwrap();

        assert!(!root.join("Music/uploaded.mp3").exists());
        assert_ne!(
            std::fs::read(root.join("Garmin/GarminDevice.xml")).unwrap(),
            b"modified"
        );
        assert!(
            device.catalog().unwrap().storages[0]
                .entries
                .iter()
                .all(|entry| entry.path != ROOT_MARKER)
        );
        assert_eq!(device.presence().unwrap(), Presence::Present);
    }

    #[cfg(unix)]
    #[test]
    fn reset_does_not_follow_links_from_a_previous_demo_tree() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("device");
        let outside = directory.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), b"keep").unwrap();
        Device::recreate(root.clone()).unwrap();
        std::fs::remove_dir_all(root.join("Garmin")).unwrap();
        symlink(&outside, root.join("Garmin")).unwrap();

        Device::recreate(root.clone()).unwrap();

        assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"keep");
        assert!(!outside.join("Activity").exists());
        let garmin = std::fs::symlink_metadata(root.join("Garmin")).unwrap();
        assert!(garmin.file_type().is_dir());
        assert!(!garmin.file_type().is_symlink());
    }

    #[test]
    fn reset_refuses_to_replace_an_unowned_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("device");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("sentinel"), b"keep").unwrap();

        let error = Device::recreate(root.clone()).unwrap_err();

        assert!(matches!(error, DeviceError::UnownedRoot { .. }));
        assert_eq!(std::fs::read(root.join("sentinel")).unwrap(), b"keep");
    }
}
