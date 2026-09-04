//! Read-only access through an existing filesystem mount.

use std::{
    collections::HashSet,
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use thiserror::Error;
use walkdir::WalkDir;

use crate::capabilities::{
    DataType, Error as ManifestError, Manifest, TransferDirection, parse, paths_equal,
};

const GARMIN_DEVICE_MANIFEST: &str = "GARMIN/GarminDevice.xml";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// A consented, already-mounted device root.
pub struct Device {
    root: PathBuf,
}

impl Device {
    /// Opens a host-provided mount root without reading it.
    /// The caller owns mount discovery and consent.
    #[must_use]
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Discovers the validated manifest and its readable files.
    /// # Errors
    /// Missing, ambiguous, invalid, or inaccessible device data.
    pub fn scan(&self) -> Result<Catalog, Error> {
        let entries = entries(&self.root)?;
        let manifests = entries
            .iter()
            .filter(|entry| {
                entry.is_file && paths_equal(&entry.relative, Path::new(GARMIN_DEVICE_MANIFEST))
            })
            .collect::<Vec<_>>();
        let manifest_entry = match manifests.as_slice() {
            [] => return Err(Error::ManifestMissing),
            [manifest] => *manifest,
            _ => return Err(Error::AmbiguousManifest),
        };
        let manifest = read_manifest(&manifest_entry.absolute)?;
        let manifest = parse(std::str::from_utf8(&manifest)?)?;
        let mut files = Vec::new();
        let mut seen = HashSet::new();

        for capability in manifest.capabilities().iter().filter(|capability| {
            matches!(
                capability.direction(),
                TransferDirection::OutputFromUnit | TransferDirection::InputOutput
            )
        }) {
            for entry in entries.iter().filter(|entry| entry.is_file) {
                if capability.handle().matches(&entry.relative)
                    && seen.insert((entry.relative.clone(), capability.data_type()))
                {
                    files.push(DeviceFile {
                        source: entry.absolute.clone(),
                        relative: entry.relative.clone(),
                        data_type: capability.data_type(),
                        direction: capability.direction(),
                        declared_size: entry.size,
                        modified: entry.modified,
                    });
                }
            }
        }
        files.sort_by(file_order);

        Ok(Catalog { manifest, files })
    }
}

impl fmt::Debug for Device {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Device").finish_non_exhaustive()
    }
}

/// A validated manifest and its readable mounted files.
pub struct Catalog {
    manifest: Manifest,
    files: Vec<DeviceFile>,
}

impl Catalog {
    #[must_use]
    pub const fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    #[must_use]
    pub fn files(&self) -> &[DeviceFile] {
        &self.files
    }
}

/// One readable file selected through a validated manifest capability.
pub struct DeviceFile {
    source: PathBuf,
    relative: PathBuf,
    data_type: DataType,
    direction: TransferDirection,
    declared_size: u64,
    modified: Option<SystemTime>,
}

impl DeviceFile {
    #[must_use]
    pub const fn data_type(&self) -> DataType {
        self.data_type
    }

    #[must_use]
    pub const fn direction(&self) -> TransferDirection {
        self.direction
    }

    #[must_use]
    pub const fn declared_size(&self) -> u64 {
        self.declared_size
    }

    /// Streams this complete file into caller-owned staging.
    /// Any error may leave an unpublished prefix in the target; discard it.
    /// # Errors
    /// Source, target, or length failures.
    pub fn copy_to<W>(&self, target: &mut W) -> Result<CopyOutcome, Error>
    where
        W: Write + ?Sized,
    {
        let mut source = fs::File::open(&self.source).map_err(Error::Source)?;
        let current_size = source.metadata().map_err(Error::Source)?.len();
        if current_size != self.declared_size {
            return Err(Error::SizeMismatch {
                declared: self.declared_size,
                received: current_size,
            });
        }

        let mut buffer = vec![0_u8; 64 * 1024];
        let mut received = 0_u64;
        loop {
            let count = source.read(&mut buffer).map_err(Error::Source)?;
            if count == 0 {
                break;
            }
            received = received
                .checked_add(u64::try_from(count).unwrap_or(u64::MAX))
                .ok_or(Error::SizeMismatch {
                    declared: self.declared_size,
                    received: u64::MAX,
                })?;
            if received > self.declared_size {
                return Err(Error::SizeMismatch {
                    declared: self.declared_size,
                    received,
                });
            }
            target.write_all(&buffer[..count]).map_err(Error::Target)?;
        }
        if received != self.declared_size {
            return Err(Error::SizeMismatch {
                declared: self.declared_size,
                received,
            });
        }
        Ok(CopyOutcome { bytes: received })
    }
}

impl fmt::Debug for DeviceFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceFile")
            .field("data_type", &self.data_type)
            .field("direction", &self.direction)
            .field("declared_size", &self.declared_size)
            .finish_non_exhaustive()
    }
}

/// A completed mount-to-staging copy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyOutcome {
    bytes: u64,
}

impl CopyOutcome {
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// A mounted-device read failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("mounted device listing failed: {0}")]
    Listing(#[from] walkdir::Error),
    #[error("GarminDevice.xml was not found")]
    ManifestMissing,
    #[error("multiple GarminDevice.xml files occupied the canonical path")]
    AmbiguousManifest,
    #[error("GarminDevice.xml exceeded {MAX_MANIFEST_BYTES} bytes")]
    ManifestTooLarge,
    #[error("GarminDevice.xml was not UTF-8: {0}")]
    ManifestUtf8(#[from] std::str::Utf8Error),
    #[error("GarminDevice.xml was rejected: {0}")]
    Manifest(#[from] ManifestError),
    #[error("mounted source read failed: {0}")]
    Source(std::io::Error),
    #[error("staging write failed: {0}")]
    Target(std::io::Error),
    #[error("mounted source size mismatch: declared {declared}, received {received}")]
    SizeMismatch {
        /// Discovery size.
        declared: u64,
        /// Transferred or current size.
        received: u64,
    },
}

struct Entry {
    absolute: PathBuf,
    relative: PathBuf,
    is_file: bool,
    size: u64,
    modified: Option<SystemTime>,
}

fn entries(root: &Path) -> Result<Vec<Entry>, Error> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .map(|entry| {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| Error::Source(std::io::Error::other(error)))?
                .to_path_buf();
            Ok(Entry {
                absolute: entry.path().to_path_buf(),
                relative,
                is_file: metadata.is_file(),
                size: metadata.len(),
                modified: metadata.modified().ok(),
            })
        })
        .collect()
}

fn read_manifest(path: &Path) -> Result<Vec<u8>, Error> {
    let file = fs::File::open(path).map_err(Error::Source)?;
    let size = file.metadata().map_err(Error::Source)?.len();
    if size > MAX_MANIFEST_BYTES {
        return Err(Error::ManifestTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(Error::Source)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MANIFEST_BYTES {
        return Err(Error::ManifestTooLarge);
    }
    Ok(bytes)
}

fn file_order(left: &DeviceFile, right: &DeviceFile) -> std::cmp::Ordering {
    right
        .modified
        .cmp(&left.modified)
        .then_with(|| left.relative.cmp(&right.relative))
}

#[cfg(test)]
mod tests {
    use std::{error::Error as StdError, fs};

    use tempfile::tempdir;

    use super::{Device, Error};
    use crate::{capabilities::DataType, test_support};

    const NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";

    #[test]
    fn scans_and_streams_manifest_declared_files() -> Result<(), Box<dyn StdError>> {
        let mount = tempdir()?;
        let garmin = mount.path().join("GARMIN");
        let activities = garmin.join("ACTIVITY");
        fs::create_dir_all(&activities)?;
        fs::write(
            garmin.join("GarminDevice.xml"),
            test_support::manifest(
                NAMESPACE,
                &[("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit")],
            )?,
        )?;
        fs::write(activities.join("activity.fit"), [1, 2, 3, 4])?;
        fs::write(activities.join("ignored.txt"), [5])?;

        let catalog = Device::open(mount.path()).scan()?;
        assert_eq!(catalog.manifest().id().as_u32(), 123_456);
        assert_eq!(catalog.manifest().model().description(), "Synthetic Garmin");
        let [file] = catalog.files() else {
            return Err("expected one readable file".into());
        };
        assert_eq!(file.data_type(), DataType::Activity);
        let mut bytes = Vec::new();
        let outcome = file.copy_to(&mut bytes)?;
        assert_eq!(outcome.bytes(), 4);
        assert_eq!(bytes, [1, 2, 3, 4]);
        Ok(())
    }

    #[test]
    fn detects_source_changes_after_discovery() -> Result<(), Box<dyn StdError>> {
        let mount = tempdir()?;
        let garmin = mount.path().join("GARMIN");
        let activities = garmin.join("ACTIVITY");
        fs::create_dir_all(&activities)?;
        fs::write(
            garmin.join("GarminDevice.xml"),
            test_support::manifest(
                NAMESPACE,
                &[("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit")],
            )?,
        )?;
        let source = activities.join("activity.fit");
        fs::write(&source, [1, 2, 3, 4])?;

        let catalog = Device::open(mount.path()).scan()?;
        fs::write(source, [1, 2])?;
        let result = catalog.files()[0].copy_to(&mut Vec::new());
        assert!(matches!(result, Err(Error::SizeMismatch { .. })));
        Ok(())
    }
}
