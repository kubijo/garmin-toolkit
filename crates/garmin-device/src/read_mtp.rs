//! Read-only Garmin MTP access.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    io::Write,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use futures_lite::Stream;
use mtp_rs::{
    ByteRange, DEFAULT_CANCEL_TIMEOUT, DateTime, FileDownload, MtpDevice, ObjectHandle, ObjectInfo,
    Storage,
    mtp::{DeviceWatch, DeviceWatchBuilder, HotplugEvent},
};
use thiserror::Error;

use crate::capabilities::{
    DataType, Error as ManifestError, Manifest, TransferDirection, parse, paths_equal,
};
use crate::mtp::{GARMIN_USB_VENDOR_ID, KNOWN_GARMIN_MTP};

const GARMIN_DEVICE_MANIFEST: &str = "GARMIN/GarminDevice.xml";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// An attached Garmin MTP candidate.
#[derive(Clone, Eq, PartialEq)]
pub struct Candidate {
    vendor_id: u16,
    product_id: u16,
    manufacturer: Option<String>,
    product: Option<String>,
    serial_number: Option<String>,
    location_id: u64,
}

impl Candidate {
    #[must_use]
    pub const fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    #[must_use]
    pub const fn product_id(&self) -> u16 {
        self.product_id
    }

    #[must_use]
    pub fn manufacturer(&self) -> Option<&str> {
        self.manufacturer.as_deref()
    }

    #[must_use]
    pub fn product(&self) -> Option<&str> {
        self.product.as_deref()
    }

    #[must_use]
    pub fn serial_number(&self) -> Option<&str> {
        self.serial_number.as_deref()
    }

    /// Opens this candidate after scan consent.
    /// # Errors
    /// Missing, busy, or non-MTP device.
    pub async fn connect(&self) -> Result<Device, Error> {
        let unchanged = MtpDevice::list_devices_with_known(KNOWN_GARMIN_MTP)?
            .into_iter()
            .any(|candidate| {
                candidate.location_id == self.location_id
                    && candidate.vendor_id == self.vendor_id
                    && candidate.product_id == self.product_id
                    && candidate.serial_number == self.serial_number
            });
        if !unchanged {
            return Err(Error::CandidateChanged);
        }
        let inner = MtpDevice::builder()
            .known_devices(KNOWN_GARMIN_MTP)
            .open_by_location(self.location_id)
            .await
            .map_err(connection_error)?;
        Ok(Device { inner })
    }
}

impl fmt::Debug for Candidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Candidate")
            .field("vendor_id", &self.vendor_id)
            .field("product_id", &self.product_id)
            .field("manufacturer", &self.manufacturer)
            .field("product", &self.product)
            .finish_non_exhaustive()
    }
}

/// Lists attached Garmin MTP candidates without opening their files.
/// # Errors
/// USB enumeration failure.
pub fn candidates() -> Result<Vec<Candidate>, Error> {
    Ok(MtpDevice::list_devices_with_known(KNOWN_GARMIN_MTP)?
        .into_iter()
        .filter(|candidate| candidate.vendor_id == GARMIN_USB_VENDOR_ID)
        .map(candidate_from_info)
        .collect())
}

/// Starts a Garmin attachment watch.
///
/// Reports the initial snapshot, then arrivals and departures, without opening
/// a device. Consumers must periodically reconcile against [`candidates`]
/// because host events are not durable.
/// # Errors
/// USB notification setup failure.
pub fn watch_candidates() -> Result<CandidateWatch, Error> {
    let inner = DeviceWatchBuilder::new()
        .known_devices(KNOWN_GARMIN_MTP)
        .watch()?;
    Ok(CandidateWatch { inner })
}

/// Garmin attachment events, including candidates present when watching begins.
pub struct CandidateWatch {
    inner: DeviceWatch,
}

impl fmt::Debug for CandidateWatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CandidateWatch")
            .finish_non_exhaustive()
    }
}

impl Stream for CandidateWatch {
    type Item = CandidateEvent;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let event = match Pin::new(&mut self.inner).poll_next(context) {
                Poll::Ready(Some(event)) => event,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            };
            let event = match event {
                HotplugEvent::Arrived(candidate) if candidate.vendor_id == GARMIN_USB_VENDOR_ID => {
                    CandidateEvent::Arrived(candidate_from_info(candidate))
                }
                HotplugEvent::Left(candidate) if candidate.vendor_id == GARMIN_USB_VENDOR_ID => {
                    CandidateEvent::Left(candidate_from_info(candidate))
                }
                HotplugEvent::Arrived(_) | HotplugEvent::Left(_) => continue,
            };
            return Poll::Ready(Some(event));
        }
    }
}

/// A change in the attached Garmin candidate set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateEvent {
    Arrived(Candidate),
    Left(Candidate),
}

/// An open, consented MTP device.
pub struct Device {
    inner: MtpDevice,
}

impl Device {
    /// Discovers validated manifests and their readable files.
    /// # Errors
    /// Incomplete metadata, invalid manifest, unsafe hierarchy, or transport failure.
    pub async fn scan(&self) -> Result<Vec<Catalog>, Error> {
        let mut catalogs = Vec::new();
        for storage in self.inner.storages().await? {
            if let Some(catalog) = scan_storage(storage).await? {
                catalogs.push(catalog);
            }
        }
        if catalogs.is_empty() {
            return Err(Error::ManifestMissing);
        }
        Ok(catalogs)
    }
}

/// A validated manifest and the readable files it declares on one storage.
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
    storage: Arc<Storage>,
    handle: ObjectHandle,
    source_path: PathBuf,
    data_type: DataType,
    direction: TransferDirection,
    declared_size: u64,
    timestamp: Option<DateTime>,
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

    /// Streams this complete file into a caller-owned staging writer.
    ///
    /// The source path and MTP handle remain private.
    /// A failed target write cancels the MTP stream before returning.
    /// Any error may leave an unpublished prefix in the target; discard it.
    /// # Errors
    /// Transport, target, cancellation, or length failures.
    pub async fn copy_to<W>(&self, target: &mut W) -> Result<CopyOutcome, Error>
    where
        W: Write + ?Sized,
    {
        let mut download = self.storage.download(self.handle, ByteRange::Full).await?;
        if download.size() != self.declared_size {
            cancel(&mut download, "object size changed before transfer").await?;
            return Err(Error::SizeMismatch {
                declared: self.declared_size,
                received: download.size(),
            });
        }

        while let Some(result) = download.next_chunk().await {
            let chunk = match result {
                Ok(chunk) => chunk,
                Err(transfer) => {
                    return match download.cancel(DEFAULT_CANCEL_TIMEOUT).await {
                        Ok(()) => Err(Error::Mtp(transfer)),
                        Err(cancel) => Err(Error::TransferAndCancel { transfer, cancel }),
                    };
                }
            };
            if let Err(target_error) = target.write_all(&chunk) {
                return match download.cancel(DEFAULT_CANCEL_TIMEOUT).await {
                    Ok(()) => Err(Error::Target(target_error)),
                    Err(cancel) => Err(Error::TargetAndCancel {
                        target: target_error,
                        cancel,
                    }),
                };
            }
        }

        let received = download.bytes_received();
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

/// A completed device-to-staging copy.
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

/// A read-only MTP failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("MTP operation failed: {0}")]
    Mtp(#[from] mtp_rs::Error),
    #[error("Garmin device is mounted or held by another process")]
    DeviceBusy,
    #[error("host permissions do not allow Garmin MTP access")]
    PermissionDenied,
    #[error("MTP listing omitted {0} unreadable objects")]
    IncompleteListing(usize),
    #[error("GarminDevice.xml was not found")]
    ManifestMissing,
    #[error("the selected Garmin candidate disappeared or changed")]
    CandidateChanged,
    #[error("multiple GarminDevice.xml objects occupied the canonical path")]
    AmbiguousManifest,
    #[error("GarminDevice.xml exceeded {MAX_MANIFEST_BYTES} bytes")]
    ManifestTooLarge,
    #[error("GarminDevice.xml was not UTF-8: {0}")]
    ManifestUtf8(#[from] std::str::Utf8Error),
    #[error("GarminDevice.xml was rejected: {0}")]
    Manifest(#[from] ManifestError),
    #[error("MTP listing repeated an object handle")]
    DuplicateHandle,
    #[error("MTP object referenced a missing parent")]
    MissingParent,
    #[error("MTP object hierarchy contained a cycle")]
    ObjectCycle,
    #[error("MTP object hierarchy contained an unsafe filename component")]
    UnsafeObjectName,
    #[error("MTP size mismatch: declared {declared}, received {received}")]
    SizeMismatch {
        /// Metadata size.
        declared: u64,
        /// Transferred size.
        received: u64,
    },
    #[error("staging write failed: {0}")]
    Target(std::io::Error),
    #[error("MTP transfer failed ({transfer}); cancellation also failed ({cancel})")]
    TransferAndCancel {
        /// Original transfer error.
        transfer: mtp_rs::Error,
        /// Cancellation error.
        cancel: mtp_rs::Error,
    },
    #[error("staging write failed ({target}); MTP cancellation also failed ({cancel})")]
    TargetAndCancel {
        /// Original target error.
        target: std::io::Error,
        /// Cancellation error.
        cancel: mtp_rs::Error,
    },
    #[error("MTP cancellation failed after {context}: {source}")]
    Cancel {
        /// Rejection that required cancellation.
        context: &'static str,
        /// Cancellation error.
        source: mtp_rs::Error,
    },
}

async fn scan_storage(storage: Storage) -> Result<Option<Catalog>, Error> {
    let collection = storage.collect_objects_recursive(None).await?;
    if !collection.skipped.is_empty() {
        return Err(Error::IncompleteListing(collection.skipped.len()));
    }
    let objects = collection.objects;
    let by_handle = index_objects(&objects)?;
    let paths = objects
        .iter()
        .map(|object| Ok((object.handle, object_path(object, &by_handle)?)))
        .collect::<Result<HashMap<_, _>, Error>>()?;

    let manifests = objects
        .iter()
        .filter(|object| {
            object.is_file()
                && paths
                    .get(&object.handle)
                    .is_some_and(|path| paths_equal(path, Path::new(GARMIN_DEVICE_MANIFEST)))
        })
        .collect::<Vec<_>>();
    let manifest_object = match manifests.as_slice() {
        [] => return Ok(None),
        [manifest] => *manifest,
        _ => return Err(Error::AmbiguousManifest),
    };
    let manifest_bytes = read_manifest(&storage, manifest_object).await?;
    let manifest = parse(std::str::from_utf8(&manifest_bytes)?)?;
    let storage = Arc::new(storage);
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    for capability in manifest.capabilities().iter().filter(|capability| {
        matches!(
            capability.direction(),
            TransferDirection::OutputFromUnit | TransferDirection::InputOutput
        )
    }) {
        for object in objects.iter().filter(|object| object.is_file()) {
            let Some(path) = paths.get(&object.handle) else {
                continue;
            };
            if capability.handle().matches(path)
                && seen.insert((object.handle, capability.data_type()))
            {
                files.push(DeviceFile {
                    storage: Arc::clone(&storage),
                    handle: object.handle,
                    source_path: path.clone(),
                    data_type: capability.data_type(),
                    direction: capability.direction(),
                    declared_size: object.size,
                    timestamp: object.modified.or(object.created),
                });
            }
        }
    }
    files.sort_by(file_order);
    Ok(Some(Catalog { manifest, files }))
}

fn candidate_from_info(candidate: mtp_rs::mtp::MtpDeviceInfo) -> Candidate {
    Candidate {
        vendor_id: candidate.vendor_id,
        product_id: candidate.product_id,
        manufacturer: candidate.manufacturer,
        product: candidate.product,
        serial_number: candidate.serial_number,
        location_id: candidate.location_id,
    }
}

fn connection_error(error: mtp_rs::Error) -> Error {
    if error.is_exclusive_access() {
        Error::DeviceBusy
    } else if error.is_permission_denied() {
        Error::PermissionDenied
    } else {
        Error::Mtp(error)
    }
}

async fn read_manifest(storage: &Storage, object: &ObjectInfo) -> Result<Vec<u8>, Error> {
    if object.size > MAX_MANIFEST_BYTES {
        return Err(Error::ManifestTooLarge);
    }
    let mut download = storage.download(object.handle, ByteRange::Full).await?;
    if download.size() != object.size {
        cancel(&mut download, "manifest size changed before transfer").await?;
        return Err(Error::SizeMismatch {
            declared: object.size,
            received: download.size(),
        });
    }

    let mut bytes = Vec::new();
    while let Some(result) = download.next_chunk().await {
        let chunk = match result {
            Ok(chunk) => chunk,
            Err(transfer) => {
                return match download.cancel(DEFAULT_CANCEL_TIMEOUT).await {
                    Ok(()) => Err(Error::Mtp(transfer)),
                    Err(cancel) => Err(Error::TransferAndCancel { transfer, cancel }),
                };
            }
        };
        let chunk_size = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        let bounded = u64::try_from(bytes.len())
            .ok()
            .and_then(|received| received.checked_add(chunk_size))
            .is_some_and(|received| received <= MAX_MANIFEST_BYTES);
        if !bounded {
            cancel(&mut download, "manifest exceeded its size bound").await?;
            return Err(Error::ManifestTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }

    let received = download.bytes_received();
    if received != object.size {
        return Err(Error::SizeMismatch {
            declared: object.size,
            received,
        });
    }
    Ok(bytes)
}

fn index_objects(objects: &[ObjectInfo]) -> Result<HashMap<ObjectHandle, &ObjectInfo>, Error> {
    let mut indexed = HashMap::with_capacity(objects.len());
    for object in objects {
        if indexed.insert(object.handle, object).is_some() {
            return Err(Error::DuplicateHandle);
        }
    }
    Ok(indexed)
}

fn object_path(
    object: &ObjectInfo,
    objects: &HashMap<ObjectHandle, &ObjectInfo>,
) -> Result<PathBuf, Error> {
    validate_object_name(&object.filename)?;
    let mut components = vec![object.filename.as_str()];
    let mut parent = object.parent;
    let mut visited = HashSet::new();
    while parent != ObjectHandle::ROOT {
        if !visited.insert(parent) {
            return Err(Error::ObjectCycle);
        }
        let ancestor = objects.get(&parent).ok_or(Error::MissingParent)?;
        validate_object_name(&ancestor.filename)?;
        components.push(ancestor.filename.as_str());
        parent = ancestor.parent;
    }
    components.reverse();

    let mut path = PathBuf::new();
    for component in components {
        path.push(component);
    }
    Ok(path)
}

fn validate_object_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', ':'])
        || Path::new(name).is_absolute()
    {
        return Err(Error::UnsafeObjectName);
    }
    Ok(())
}

fn file_order(left: &DeviceFile, right: &DeviceFile) -> std::cmp::Ordering {
    right
        .timestamp
        .map(timestamp_key)
        .cmp(&left.timestamp.map(timestamp_key))
        .then_with(|| left.source_path.cmp(&right.source_path))
}

const fn timestamp_key(value: DateTime) -> (u16, u8, u8, u8, u8, u8) {
    (
        value.year,
        value.month,
        value.day,
        value.hour,
        value.minute,
        value.second,
    )
}

async fn cancel(download: &mut FileDownload, context: &'static str) -> Result<(), Error> {
    download
        .cancel(DEFAULT_CANCEL_TIMEOUT)
        .await
        .map_err(|source| Error::Cancel { context, source })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        error::Error as StdError,
        fs,
        io::{Error as IoError, Result as IoResult, Write},
        time::Duration,
    };

    use futures_lite::future::block_on;
    use mtp_rs::{MtpDevice, ObjectInfo, VirtualDeviceConfig, VirtualStorageConfig};
    use tempfile::tempdir;

    use super::{Device, Error, connection_error, object_path};
    use crate::{capabilities::DataType, test_support};

    const NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";

    struct RejectingWriter;

    impl Write for RejectingWriter {
        fn write(&mut self, _buffer: &[u8]) -> IoResult<usize> {
            Err(IoError::other("synthetic staging failure"))
        }

        fn flush(&mut self) -> IoResult<()> {
            Ok(())
        }
    }

    #[test]
    fn scans_and_streams_manifest_declared_files() -> Result<(), Box<dyn StdError>> {
        block_on(async {
            let backing = tempdir()?;
            let garmin = backing.path().join("GARMIN");
            let activities = garmin.join("ACTIVITY");
            fs::create_dir_all(&activities)?;
            fs::write(
                garmin.join("GarminDevice.xml"),
                test_support::manifest(
                    NAMESPACE,
                    &[("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit")],
                )?,
            )?;
            fs::write(activities.join("first.fit"), [1, 2, 3])?;
            fs::write(activities.join("second.FIT"), [4, 5, 6, 7])?;
            fs::write(activities.join("ignored.txt"), [8])?;

            let inner = MtpDevice::builder()
                .open_virtual(VirtualDeviceConfig {
                    serial: "garmin-toolkit-mtp-read".to_owned(),
                    storages: vec![VirtualStorageConfig {
                        description: "Internal Storage".to_owned(),
                        capacity: 1_048_576,
                        backing_dir: backing.path().to_path_buf(),
                        read_only: true,
                    }],
                    event_poll_interval: Duration::ZERO,
                    watch_backing_dirs: false,
                    ..VirtualDeviceConfig::default()
                })
                .await?;
            let catalogs = Device { inner }.scan().await?;
            let [catalog] = catalogs.as_slice() else {
                return Err("expected one manifest-bearing storage".into());
            };
            assert_eq!(catalog.manifest().id().as_u32(), 123_456);
            assert_eq!(catalog.manifest().model().description(), "Synthetic Garmin");
            assert_eq!(catalog.files().len(), 2);
            assert!(
                catalog
                    .files()
                    .iter()
                    .all(|file| file.data_type() == DataType::Activity)
            );

            for file in catalog.files() {
                let mut bytes = Vec::new();
                let copied = file.copy_to(&mut bytes).await?;
                assert_eq!(copied.bytes(), file.declared_size());
                assert!(matches!(bytes.as_slice(), [1, 2, 3] | [4, 5, 6, 7]));
            }

            let first = catalog
                .files()
                .first()
                .ok_or("manifest declared no readable files")?;
            assert!(matches!(
                first.copy_to(&mut RejectingWriter).await,
                Err(Error::Target(_))
            ));
            let mut retry = Vec::new();
            first.copy_to(&mut retry).await?;
            assert!(!retry.is_empty());
            Ok(())
        })
    }

    #[test]
    fn rejects_unsafe_object_names_before_path_construction() {
        let mut object = ObjectInfo::default();
        object.filename = "..".to_owned();
        assert!(matches!(
            object_path(&object, &HashMap::new()),
            Err(Error::UnsafeObjectName)
        ));
    }

    #[test]
    fn classifies_connection_failures_for_host_guidance() {
        assert!(matches!(
            connection_error(mtp_rs::Error::ExclusiveAccess),
            Error::DeviceBusy
        ));
        assert!(matches!(
            connection_error(mtp_rs::Error::PermissionDenied),
            Error::PermissionDenied
        ));
    }
}
