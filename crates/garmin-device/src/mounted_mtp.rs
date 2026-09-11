use crate::manifest::{MAX_MANIFEST_BYTES, ManifestError, parse_manifest};
use crate::storage::BackupDestination;
use crate::storage::DeviceDirectoryEntry;
use crate::system::{
    MountedMtpBackupProgress, MountedMtpProbeFailure, MountedMtpUploadProgress,
    MountedMtpVerifyProgress,
};
use crate::{
    DeviceInventory, DeviceManifest, DevicePathInspection, DevicePathState, SafeRelativePath,
    TransportKind,
};
use crate::{DeviceProbeReport, UploadCompletion};
use garmin_progress::{CancellationToken, OperationStage, ProgressReporter};
use gio::prelude::{CancellableExt, FileEnumeratorExt, FileExt, InputStreamExtManual};
use gio::{File, FileCopyFlags, FileType};
use sha2::{Digest, Sha256};
use std::fs::File as StdFile;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

const CHILD_ATTRIBUTES: &str = "standard::name,standard::type,standard::size";
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(40);
const REQUIRED_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

struct CancellableWatcher {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl CancellableWatcher {
    fn new(cancellation: CancellationToken, cancellable: &gio::Cancellable) -> Self {
        Self::new_with_timeout(cancellation, cancellable, None)
    }

    fn with_timeout(
        cancellation: CancellationToken,
        cancellable: &gio::Cancellable,
        timeout: Duration,
    ) -> Self {
        Self::new_with_timeout(cancellation, cancellable, Some(timeout))
    }

    fn new_with_timeout(
        cancellation: CancellationToken,
        cancellable: &gio::Cancellable,
        timeout: Option<Duration>,
    ) -> Self {
        if cancellation.is_cancelled() {
            cancellable.cancel();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let cancellable = cancellable.clone();
        let worker = thread::spawn(move || {
            let started = Instant::now();
            while !worker_stop.load(Ordering::Acquire) {
                if cancellation.is_cancelled()
                    || timeout.is_some_and(|timeout| started.elapsed() >= timeout)
                {
                    cancellable.cancel();
                    break;
                }
                thread::park_timeout(CANCELLATION_POLL_INTERVAL);
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for CancellableWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn cancellation_result<T>(
    result: Result<T, MountedMtpError>,
    progress: &ProgressReporter,
) -> Result<T, MountedMtpError> {
    if progress.is_cancelled() {
        Err(MountedMtpError::Cancelled)
    } else {
        result
    }
}

fn run_cancellable<T>(
    progress: &ProgressReporter,
    operation: impl FnOnce(&gio::Cancellable) -> Result<T, MountedMtpError>,
) -> Result<T, MountedMtpError> {
    let cancellable = gio::Cancellable::new();
    let _cancellation = CancellableWatcher::new(progress.cancellation_token(), &cancellable);
    cancellation_result(operation(&cancellable), progress)
}

pub async fn mounted_device_state(
    mount_id: &str,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    mounted_device_state_with_progress(mount_id, &ProgressReporter::default()).await
}

pub async fn mounted_device_state_with_progress(
    mount_id: &str,
    progress: &ProgressReporter,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let progress = progress.clone();
    tokio::task::spawn_blocking(move || {
        run_cancellable(&progress, |cancellable| {
            mounted_device_state_blocking_with_cancellable(&mount_id, Some(cancellable))
        })
    })
    .await
    .map_err(MountedMtpError::Task)?
}

pub async fn read_bounded_mounted_mtp_file(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    limit: u64,
) -> Result<Option<Vec<u8>>, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        let root = mounted_root(&mount_id)?;
        let storage = storage_for_id_with_cancellable(&root, &storage_id, None)?;
        read_bounded_mounted_file(&storage, &path, limit)
    })
    .await
    .map_err(MountedMtpError::Task)?
}

fn read_bounded_mounted_file(
    storage: &File,
    path: &SafeRelativePath,
    limit: u64,
) -> Result<Option<Vec<u8>>, MountedMtpError> {
    let (parent, name) = match mounted_parent_with_cancellable(storage, path, None) {
        Ok(parent) => parent,
        Err(MountedMtpError::RemovalObjectState {
            state: DevicePathState::Missing,
            ..
        }) => return Ok(None),
        Err(MountedMtpError::RemovalObjectState { state, .. }) => {
            return Err(MountedMtpError::MetadataPathState {
                path: path.clone(),
                state,
            });
        }
        Err(error) => return Err(error),
    };
    let file = match child_metadata_with_cancellable(&parent, &name, None)? {
        ChildMetadata::Missing => return Ok(None),
        ChildMetadata::Ambiguous => {
            return Err(MountedMtpError::MetadataPathState {
                path: path.clone(),
                state: DevicePathState::Ambiguous,
            });
        }
        ChildMetadata::Unique {
            file,
            file_type: FileType::Regular,
            size,
        } => {
            let size =
                u64::try_from(size).map_err(|_| MountedMtpError::NegativeObjectSize(size))?;
            if size > limit {
                return Err(MountedMtpError::MetadataTooLarge {
                    path: path.clone(),
                    limit,
                    size,
                });
            }
            file
        }
        ChildMetadata::Unique { .. } => {
            return Err(MountedMtpError::MetadataPathState {
                path: path.clone(),
                state: DevicePathState::Other,
            });
        }
    };
    let stream = file.read(gio::Cancellable::NONE)?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = stream.read(&mut buffer, gio::Cancellable::NONE)?;
        if count == 0 {
            break;
        }
        if u64::try_from(bytes.len().saturating_add(count)).unwrap_or(u64::MAX) > limit {
            return Err(MountedMtpError::MetadataTooLarge {
                path: path.clone(),
                limit,
                size: u64::try_from(bytes.len().saturating_add(count)).unwrap_or(u64::MAX),
            });
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(Some(bytes))
}

pub async fn list_mounted_mtp_directory(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
) -> Result<Vec<DeviceDirectoryEntry>, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        let root = mounted_root(&mount_id)?;
        let storage = storage_for_id_with_cancellable(&root, &storage_id, None)?;
        let directory = mounted_directory_with_cancellable(&storage, &path, None)?;
        let enumerator = directory.enumerate_children(
            CHILD_ATTRIBUTES,
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )?;
        let mut names = std::collections::BTreeSet::new();
        let mut entries = Vec::new();
        while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
            let name = info.name().to_string_lossy().into_owned();
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(MountedMtpError::MetadataPathState {
                    path,
                    state: DevicePathState::Ambiguous,
                });
            }
            let (state, size) = match info.file_type() {
                FileType::Regular => (
                    DevicePathState::RegularFile,
                    Some(
                        u64::try_from(info.size())
                            .map_err(|_| MountedMtpError::NegativeObjectSize(info.size()))?,
                    ),
                ),
                FileType::Directory => (DevicePathState::Directory, None),
                _ => (DevicePathState::Other, None),
            };
            entries.push(DeviceDirectoryEntry { name, state, size });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    })
    .await
    .map_err(MountedMtpError::Task)?
}

pub async fn ensure_mounted_mtp_directory(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        let root = mounted_root(&mount_id)?;
        let mut parent = storage_for_id_with_cancellable(&root, &storage_id, None)?;
        for component in path.as_path().components() {
            let name = component.as_os_str().to_string_lossy().into_owned();
            parent = match child_metadata_with_cancellable(&parent, &name, None)? {
                ChildMetadata::Unique {
                    file,
                    file_type: FileType::Directory,
                    ..
                } => file,
                ChildMetadata::Missing => {
                    let directory = parent.child(&name);
                    directory.make_directory(gio::Cancellable::NONE)?;
                    match child_metadata_with_cancellable(&parent, &name, None)? {
                        ChildMetadata::Unique {
                            file,
                            file_type: FileType::Directory,
                            ..
                        } => file,
                        ChildMetadata::Ambiguous => {
                            return Err(MountedMtpError::MetadataPathState {
                                path,
                                state: DevicePathState::Ambiguous,
                            });
                        }
                        _ => {
                            return Err(MountedMtpError::MetadataPathState {
                                path,
                                state: DevicePathState::Other,
                            });
                        }
                    }
                }
                ChildMetadata::Ambiguous => {
                    return Err(MountedMtpError::MetadataPathState {
                        path,
                        state: DevicePathState::Ambiguous,
                    });
                }
                ChildMetadata::Unique { .. } => {
                    return Err(MountedMtpError::MetadataPathState {
                        path,
                        state: DevicePathState::Other,
                    });
                }
            };
        }
        Ok(())
    })
    .await
    .map_err(MountedMtpError::Task)?
}

pub async fn create_verified_mounted_mtp_file(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    bytes: &[u8],
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut temporary = tempfile::NamedTempFile::new()?;
        temporary.write_all(&bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let size = u64::try_from(bytes.len()).map_err(|_| MountedMtpError::MetadataTooLarge {
            path: path.clone(),
            limit: u64::MAX,
            size: u64::MAX,
        })?;
        let root = mounted_root(&mount_id)?;
        let storage = storage_for_id_with_cancellable(&root, &storage_id, None)?;
        upload_to_storage_blocking(
            &storage,
            &path,
            temporary.path(),
            size,
            &sha256,
            &MountedMtpUploadProgress {
                reporter: ProgressReporter::default(),
                completed_before: 0,
                total: size,
            },
        )
    })
    .await
    .map_err(MountedMtpError::Task)?
}

pub async fn remove_empty_mounted_mtp_directory(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        let root = mounted_root(&mount_id)?;
        let storage = storage_for_id_with_cancellable(&root, &storage_id, None)?;
        let directory = mounted_directory_with_cancellable(&storage, &path, None)?;
        directory.delete(gio::Cancellable::NONE)?;
        if directory.query_exists(gio::Cancellable::NONE) {
            return Err(MountedMtpError::MetadataPathState {
                path,
                state: DevicePathState::Directory,
            });
        }
        Ok(())
    })
    .await
    .map_err(MountedMtpError::Task)?
}

fn mounted_device_state_blocking_with_cancellable(
    mount_id: &str,
    cancellable: Option<&gio::Cancellable>,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    let root = mounted_root(mount_id)?;
    mounted_device_state_for_root(&root, cancellable)
}

fn mounted_device_state_for_root(
    root: &File,
    cancellable: Option<&gio::Cancellable>,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    let mut storages = Vec::new();
    for (index, storage) in storage_roots_with_cancellable(root, cancellable)?
        .into_iter()
        .enumerate()
    {
        let info = storage.query_filesystem_info(
            "filesystem::size,filesystem::free,filesystem::readonly",
            cancellable,
        );
        let (capacity, writable) = match info {
            Ok(info) => {
                let capacity = if info.has_attribute("filesystem::size")
                    && info.has_attribute("filesystem::free")
                {
                    crate::StorageCapacity::new(
                        info.attribute_uint64("filesystem::size"),
                        info.attribute_uint64("filesystem::free"),
                    )
                } else {
                    crate::StorageCapacity::unavailable("The mount did not report capacity")
                };
                let writable = info
                    .has_attribute("filesystem::readonly")
                    .then(|| !info.boolean("filesystem::readonly"));
                (capacity, writable)
            }
            Err(error) => (crate::StorageCapacity::unavailable(error.to_string()), None),
        };
        storages.push(crate::DeviceStorageState {
            id: mount_id_for_uri(&storage.uri()),
            label: storage_label_with_cancellable(&storage, index, cancellable),
            capacity,
            writable,
        });
    }
    Ok(crate::DeviceStateSnapshot { storages })
}

use crate::system::mounted_catalog::MountedMtpCandidate;

#[must_use]
pub fn discover_mounted_mtp() -> Vec<MountedMtpCandidate> {
    crate::system::mounted_catalog::mounted_candidates()
}

/// Read `GarminDevice.xml` through an existing `GVfs` MTP session.
///
/// Uses GIO without taking the USB interface from `GVfs`.
/// # Errors
/// Mount or GIO failure; missing, ambiguous, oversized, or invalid manifest.
pub async fn open_mounted_mtp(mount_id: &str) -> Result<DeviceManifest, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    tokio::task::spawn_blocking(move || open_mounted_mtp_blocking(&mount_id))
        .await
        .map_err(MountedMtpError::Task)?
}

/// Inspect declared paths through the desktop-owned MTP session.
///
/// Every Garmin storage is checked independently.
/// Returned storage IDs remain opaque and do not expose private GIO URIs.
/// # Errors
/// [`MountedMtpError`] if the mount disappears.
/// Failed object-tree enumeration also returns an error.
pub async fn inventory_mounted_mtp(
    mount_id: &str,
    paths: &[SafeRelativePath],
) -> Result<DeviceInventory, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let paths = paths.to_vec();
    tokio::task::spawn_blocking(move || inventory_mounted_mtp_blocking(&mount_id, &paths))
        .await
        .map_err(MountedMtpError::Task)?
}

/// Return the opaque ID of the storage that owns `GarminDevice.xml`.
///
/// This canonical storage receives paths absent from every mounted storage.
/// # Errors
/// Missing and ambiguous canonical manifests return [`MountedMtpError`].
pub async fn primary_mounted_mtp_storage_id(mount_id: &str) -> Result<String, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    tokio::task::spawn_blocking(move || {
        let root = mounted_root(&mount_id)?;
        let mut owners = Vec::new();
        for (storage, garmin) in garmin_storages(&root)? {
            if child_named(&garmin, "GarminDevice.xml", FileType::Regular)?.is_some() {
                owners.push(mount_id_for_uri(&storage.uri()));
            }
        }
        match owners.as_slice() {
            [] => Err(MountedMtpError::ManifestMissing),
            [storage_id] => Ok(storage_id.clone()),
            _ => Err(MountedMtpError::AmbiguousManifest),
        }
    })
    .await
    .map_err(MountedMtpError::Task)?
}

fn inventory_mounted_mtp_blocking(
    mount_id: &str,
    paths: &[SafeRelativePath],
) -> Result<DeviceInventory, MountedMtpError> {
    let root = mounted_root(mount_id)?;
    let storages = garmin_storages(&root)?;
    if storages.is_empty() {
        return Err(MountedMtpError::GarminDirectoryMissing);
    }
    let mut inspected = Vec::with_capacity(paths.len().saturating_mul(storages.len()));
    for (storage_index, (storage, _garmin)) in storages.iter().enumerate() {
        let storage_id = mount_id_for_uri(&storage.uri());
        let storage_label = storage_label(storage, storage_index);
        for path in paths {
            let (state, size) = inspect_mounted_path(storage, path)?;
            inspected.push(DevicePathInspection {
                storage_id: storage_id.clone(),
                storage_label: storage_label.clone(),
                path: path.clone(),
                state,
                size,
            });
        }
    }
    Ok(DeviceInventory {
        transport: TransportKind::MountedMtp,
        paths: inspected,
    })
}

fn inspect_mounted_path(
    storage: &File,
    relative: &SafeRelativePath,
) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
    inspect_mounted_path_with_cancellable(storage, relative, None)
}

fn inspect_mounted_path_with_cancellable(
    storage: &File,
    relative: &SafeRelativePath,
    cancellable: Option<&gio::Cancellable>,
) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
    let components = relative
        .as_path()
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut parent = storage.clone();
    for (index, component) in components.iter().enumerate() {
        let (child, file_type, size) =
            match child_metadata_with_cancellable(&parent, component, cancellable)? {
                ChildMetadata::Missing => return Ok((DevicePathState::Missing, None)),
                ChildMetadata::Ambiguous => return Ok((DevicePathState::Ambiguous, None)),
                ChildMetadata::Unique {
                    file,
                    file_type,
                    size,
                } => (file, file_type, size),
            };
        let is_last = index + 1 == components.len();
        if !is_last && file_type != FileType::Directory {
            return Ok((DevicePathState::Other, None));
        }
        if is_last {
            return Ok(match file_type {
                FileType::Regular => {
                    let size = u64::try_from(size)
                        .map_err(|_| MountedMtpError::NegativeObjectSize(size))?;
                    (DevicePathState::RegularFile, Some(size))
                }
                FileType::Directory => (DevicePathState::Directory, None),
                _ => (DevicePathState::Other, None),
            });
        }
        parent = child;
    }
    Ok((DevicePathState::Other, None))
}

fn child_metadata_with_cancellable(
    parent: &File,
    expected: &str,
    cancellable: Option<&gio::Cancellable>,
) -> Result<ChildMetadata, MountedMtpError> {
    let enumerator = parent.enumerate_children(
        CHILD_ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        cancellable,
    )?;
    let mut found = None;
    while let Some(info) = enumerator.next_file(cancellable)? {
        if info.name().to_string_lossy().eq_ignore_ascii_case(expected) {
            if found.is_some() {
                return Ok(ChildMetadata::Ambiguous);
            }
            found = Some((enumerator.child(&info), info.file_type(), info.size()));
        }
    }
    Ok(
        found.map_or(ChildMetadata::Missing, |(file, file_type, size)| {
            ChildMetadata::Unique {
                file,
                file_type,
                size,
            }
        }),
    )
}

enum ChildMetadata {
    Missing,
    Unique {
        file: File,
        file_type: FileType,
        size: i64,
    },
    Ambiguous,
}

fn open_mounted_mtp_blocking(mount_id: &str) -> Result<DeviceManifest, MountedMtpError> {
    let root = mounted_root(mount_id)?;
    open_mounted_mtp_root_blocking(&root, mount_id)
}

fn open_mounted_mtp_root_blocking(
    root: &File,
    mount_id: &str,
) -> Result<DeviceManifest, MountedMtpError> {
    let manifests = mounted_mtp_manifests_for_root(root, mount_id)?;
    match manifests.as_slice() {
        [manifest] => Ok(manifest.clone()),
        _ => Err(MountedMtpError::AmbiguousManifest),
    }
}

fn mounted_mtp_manifests_for_root(
    root: &File,
    mount_id: &str,
) -> Result<Vec<DeviceManifest>, MountedMtpError> {
    let mut manifests = Vec::new();
    for garmin in garmin_directories(root)? {
        if let Some(manifest) = child_named(&garmin, "GarminDevice.xml", FileType::Regular)? {
            manifests.push(manifest);
        }
    }
    if manifests.is_empty() {
        return Err(MountedMtpError::ManifestMissing);
    }
    manifests
        .iter()
        .map(|manifest| {
            let xml = read_bounded(manifest)?;
            parse_manifest(
                &xml,
                TransportKind::MountedMtp,
                format!("mounted-mtp:{mount_id}"),
            )
            .map_err(Into::into)
        })
        .collect()
}

pub(super) fn inspect_mounted_attachment_blocking(
    root: &File,
    mount_id: &str,
) -> Result<(Vec<DeviceManifest>, crate::DeviceStateSnapshot), MountedMtpError> {
    let manifests = mounted_mtp_manifests_for_root(root, mount_id)?;
    let state = mounted_device_state_for_root(root, None)?;
    Ok((manifests, state))
}

/// Copy one previously inventoried mounted-MTP object to a new local backup.
///
/// The adapter resolves the device object by opaque storage ID.
/// Path matching is case-insensitive, and size is rechecked before copying.
/// The local copy is hashed before returning its SHA-256.
/// # Errors
/// Target changes and ambiguity return [`MountedMtpError`].
/// Read failures do too.
/// Backup creation and verification failures are also returned.
pub async fn backup_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    destination: BackupDestination,
    progress: MountedMtpBackupProgress,
) -> Result<String, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        let result = backup_mounted_mtp_object_blocking(
            &mount_id,
            &storage_id,
            &path,
            expected_size,
            destination,
            &progress,
        );
        cancellation_result(result, &progress.reporter)
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Reinspect one object on a previously identified mounted-MTP storage.
/// # Errors
/// Missing mounts and storage return [`MountedMtpError`]. Unsafe path
/// enumeration also returns an error.
pub async fn inspect_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
    inspect_mounted_mtp_object_with_progress(
        mount_id,
        storage_id,
        path,
        &ProgressReporter::default(),
    )
    .await
}

pub async fn inspect_mounted_mtp_object_with_progress(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    progress: &ProgressReporter,
) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let progress = progress.clone();
    tokio::task::spawn_blocking(move || {
        run_cancellable(&progress, |cancellable| {
            let root = mounted_root(&mount_id)?;
            let storage = storage_for_id_with_cancellable(&root, &storage_id, Some(cancellable))?;
            inspect_mounted_path_with_cancellable(&storage, &path, Some(cancellable))
        })
    })
    .await
    .map_err(MountedMtpError::Task)?
}

fn backup_mounted_mtp_object_blocking(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    destination: BackupDestination,
    progress: &MountedMtpBackupProgress,
) -> Result<String, MountedMtpError> {
    let cancellable = gio::Cancellable::new();
    let _cancellation =
        CancellableWatcher::new(progress.reporter.cancellation_token(), &cancellable);
    let source = mounted_regular_file_with_cancellable(
        mount_id,
        storage_id,
        path,
        expected_size,
        Some(&cancellable),
    )?;
    let (destination_path, reserved) = destination.into_parts();
    if reserved.metadata()?.len() != 0 {
        return Err(MountedMtpError::BackupSinkVerification(path.clone()));
    }
    drop(reserved);
    std::fs::remove_file(&destination_path)?;
    let destination = File::for_path(&destination_path);
    let callback_cancellable = cancellable.clone();
    let mut copy_progress = |current: i64, _total: i64| {
        if progress.reporter.is_cancelled() {
            callback_cancellable.cancel();
        }
        let Ok(copied) = u64::try_from(current) else {
            return;
        };
        progress.advanced(storage_id, path, copied, expected_size);
    };
    if let Err(error) = source.copy(
        &destination,
        FileCopyFlags::NONE,
        Some(&cancellable),
        Some(&mut copy_progress),
    ) {
        return Err(if progress.reporter.is_cancelled() {
            MountedMtpError::Cancelled
        } else {
            MountedMtpError::Gio(error)
        });
    }
    let (copied, copied_hash) = hash_local_file(&destination_path)?;
    if copied != expected_size {
        return Err(MountedMtpError::RemovalObjectSize {
            path: path.clone(),
            expected: expected_size,
            actual: copied,
        });
    }
    progress.completed(storage_id, path, expected_size);
    Ok(copied_hash)
}

/// Delete one exact mounted-MTP object and verify that it is absent.
/// # Errors
/// Changed objects and deletion failures return [`MountedMtpError`]. Objects
/// that remain visible after deletion also return an error.
pub async fn delete_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    delete_mounted_mtp_object_with_progress(
        mount_id,
        storage_id,
        path,
        expected_size,
        expected_sha256,
        &ProgressReporter::default(),
    )
    .await
}

pub async fn delete_mounted_mtp_object_with_progress(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    expected_sha256: &str,
    progress: &ProgressReporter,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let expected_sha256 = expected_sha256.to_owned();
    let progress = progress.clone();
    tokio::task::spawn_blocking(move || {
        run_cancellable(&progress, |cancellable| {
            let target = mounted_regular_file_with_cancellable(
                &mount_id,
                &storage_id,
                &path,
                expected_size,
                Some(cancellable),
            )?;
            let (actual_size, actual_sha256) =
                hash_mounted_file_with_progress(&target, Some(&progress), |_| {})?;
            if actual_size != expected_size {
                return Err(MountedMtpError::RemovalObjectSize {
                    path: path.clone(),
                    expected: expected_size,
                    actual: actual_size,
                });
            }
            if actual_sha256 != expected_sha256 {
                return Err(MountedMtpError::RemovalObjectChecksum(path.clone()));
            }
            target.delete(Some(cancellable))?;
            if target.query_exists(Some(cancellable)) {
                return Err(MountedMtpError::RemovalObjectNotRemoved(path.clone()));
            }
            Ok(())
        })
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Delete one exact mounted-MTP object after metadata-size validation only.
/// # Errors
/// Unsafe paths, changed sizes, deletion failures, and objects that remain
/// visible after deletion return [`MountedMtpError`].
pub async fn delete_size_checked_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
) -> Result<(), MountedMtpError> {
    delete_size_checked_mounted_mtp_object_with_progress(
        mount_id,
        storage_id,
        path,
        expected_size,
        &ProgressReporter::default(),
    )
    .await
}

pub async fn delete_size_checked_mounted_mtp_object_with_progress(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    progress: &ProgressReporter,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let progress = progress.clone();
    tokio::task::spawn_blocking(move || {
        run_cancellable(&progress, |cancellable| {
            let target = mounted_regular_file_with_cancellable(
                &mount_id,
                &storage_id,
                &path,
                expected_size,
                Some(cancellable),
            )?;
            target.delete(Some(cancellable))?;
            if target.query_exists(Some(cancellable)) {
                return Err(MountedMtpError::RemovalObjectNotRemoved(path.clone()));
            }
            Ok(())
        })
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Verify a mounted-MTP object's size and SHA-256.
/// # Errors
/// The object must be readable and match.
pub async fn verify_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    verify_mounted_mtp_object_with_progress(
        mount_id,
        storage_id,
        path,
        expected_size,
        expected_sha256,
        MountedMtpVerifyProgress {
            reporter: ProgressReporter::default(),
            completed_before: 0,
            total: expected_size,
        },
    )
    .await
}

/// Verify with cancellable read progress.
/// # Errors
/// The object must be readable and match; cancellation also fails.
pub async fn verify_mounted_mtp_object_with_progress(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    expected_sha256: &str,
    progress: MountedMtpVerifyProgress,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let expected_sha256 = expected_sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        let reporter = progress.reporter.clone();
        run_cancellable(&reporter, |cancellable| {
            let target = mounted_regular_file_with_cancellable(
                &mount_id,
                &storage_id,
                &path,
                expected_size,
                Some(cancellable),
            )?;
            let (actual_size, actual_sha256) =
                hash_mounted_file_with_progress(&target, Some(&progress.reporter), |current| {
                    progress.advanced(&path, current);
                })?;
            if actual_size != expected_size {
                return Err(MountedMtpError::RemovalObjectSize {
                    path: path.clone(),
                    expected: expected_size,
                    actual: actual_size,
                });
            }
            if actual_sha256 != expected_sha256 {
                return Err(MountedMtpError::RemovalObjectChecksum(path.clone()));
            }
            Ok(())
        })
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Restore one exact mounted-MTP object from a verified local backup.
/// # Errors
/// Backup, destination, upload, and metadata failures are returned.
pub async fn restore_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    backup: &Path,
    expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    restore_mounted_mtp_object_with_progress(
        mount_id,
        storage_id,
        path,
        expected_size,
        backup,
        expected_sha256,
        &ProgressReporter::default(),
    )
    .await
}

pub async fn restore_mounted_mtp_object_with_progress(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    backup: &Path,
    expected_sha256: &str,
    progress: &ProgressReporter,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let backup = backup.to_owned();
    let expected_sha256 = expected_sha256.to_owned();
    let progress = progress.clone();
    tokio::task::spawn_blocking(move || {
        run_cancellable(&progress, |cancellable| {
            restore_mounted_mtp_object_blocking_with_cancellable(
                &mount_id,
                &storage_id,
                &path,
                expected_size,
                &backup,
                &expected_sha256,
                Some(cancellable),
                Some(&progress),
            )
        })
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Upload a locally verified file, finalize it, and check destination metadata.
/// # Errors
/// Source, destination, copy, or metadata failure.
pub async fn upload_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    source: &Path,
    expected_size: u64,
    expected_sha256: &str,
    progress: MountedMtpUploadProgress,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let source = source.to_owned();
    let expected_sha256 = expected_sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        let result = upload_mounted_mtp_object_blocking(
            &mount_id,
            &storage_id,
            &path,
            &source,
            expected_size,
            &expected_sha256,
            &progress,
        );
        cancellation_result(result, &progress.reporter)
    })
    .await
    .map_err(MountedMtpError::Task)?
}

fn upload_mounted_mtp_object_blocking(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    source: &Path,
    expected_size: u64,
    expected_sha256: &str,
    progress: &MountedMtpUploadProgress,
) -> Result<(), MountedMtpError> {
    let cancellable = gio::Cancellable::new();
    let _cancellation =
        CancellableWatcher::new(progress.reporter.cancellation_token(), &cancellable);
    let root = mounted_root(mount_id)?;
    let storage = storage_for_id_with_cancellable(&root, storage_id, Some(&cancellable))?;
    upload_to_storage_blocking(
        &storage,
        path,
        source,
        expected_size,
        expected_sha256,
        progress,
    )
}

fn upload_to_storage_blocking(
    storage: &File,
    path: &SafeRelativePath,
    source: &Path,
    expected_size: u64,
    expected_sha256: &str,
    progress: &MountedMtpUploadProgress,
) -> Result<(), MountedMtpError> {
    let (source_size, source_sha256) =
        hash_local_file_with_cancellation(source, Some(&progress.reporter))?;
    if source_size != expected_size || source_sha256 != expected_sha256 {
        return Err(MountedMtpError::UploadSourceVerification(source.to_owned()));
    }
    let cancellable = gio::Cancellable::new();
    let _cancellation =
        CancellableWatcher::new(progress.reporter.cancellation_token(), &cancellable);
    let (parent, name) = mounted_parent_with_cancellable(storage, path, Some(&cancellable))?;
    let destination = parent.child(name);
    if destination.query_exists(Some(&cancellable)) {
        return Err(MountedMtpError::UploadDestinationExists(path.clone()));
    }

    copy_upload_to_device(&destination, path, source, expected_size, progress)?;
    if let Err(error) = verify_uploaded_object_metadata(&destination, path, expected_size, progress)
    {
        progress.reporter.failed(
            OperationStage::DeviceVerify,
            "Uploaded file metadata check failed",
        );
        return Err(cleanup_failed_upload(
            &destination,
            path,
            error,
            &progress.reporter,
        ));
    }
    Ok(())
}

fn copy_upload_to_device(
    destination: &File,
    path: &SafeRelativePath,
    source: &Path,
    expected_size: u64,
    progress: &MountedMtpUploadProgress,
) -> Result<(), MountedMtpError> {
    let cancellable = gio::Cancellable::new();
    let _cancellation =
        CancellableWatcher::new(progress.reporter.cancellation_token(), &cancellable);
    let callback_cancellable = cancellable.clone();
    let mut payload_sent = false;
    let mut copy_progress = |current: i64, _total: i64| {
        if progress.reporter.is_cancelled() {
            callback_cancellable.cancel();
        }
        if let Ok(current) = u64::try_from(current) {
            progress.reporter.advanced_with_path(
                OperationStage::Commit,
                "Writing verified file to device",
                path.to_string(),
                progress.completed_before.saturating_add(current),
                Some(progress.total),
            );
            if current >= expected_size && !payload_sent {
                payload_sent = true;
                progress.reporter.started_with_path(
                    OperationStage::DeviceFinalize,
                    "Waiting for the desktop MTP mount to finalize the upload",
                    path.to_string(),
                    None,
                );
            }
        }
    };
    if let Err(upload) = File::for_path(source).copy(
        destination,
        FileCopyFlags::NONE,
        Some(&cancellable),
        Some(&mut copy_progress),
    ) {
        let cleanup = discard_partial_object(destination, &path.to_string(), &progress.reporter);
        progress.reporter.failed(
            if payload_sent {
                OperationStage::DeviceFinalize
            } else {
                OperationStage::Commit
            },
            "Desktop MTP upload failed",
        );
        if progress.reporter.is_cancelled() && cleanup.is_ok() {
            return Err(MountedMtpError::Cancelled);
        }
        return match cleanup {
            Ok(()) => Err(MountedMtpError::Upload(upload)),
            Err(cleanup) => Err(MountedMtpError::UploadCleanup { upload, cleanup }),
        };
    }

    if !payload_sent {
        progress.reporter.started_with_path(
            OperationStage::DeviceFinalize,
            "Waiting for the desktop MTP mount to finalize the upload",
            path.to_string(),
            None,
        );
    }
    progress.reporter.completed_with_path(
        OperationStage::DeviceFinalize,
        "Desktop MTP upload finalized",
        path.to_string(),
        1,
        Some(1),
    );
    Ok(())
}

fn verify_uploaded_object_metadata(
    destination: &File,
    path: &SafeRelativePath,
    expected_size: u64,
    progress: &MountedMtpUploadProgress,
) -> Result<(), MountedMtpError> {
    progress.reporter.started_operations_with_path(
        OperationStage::DeviceVerify,
        "Checking uploaded file path and size",
        path.to_string(),
        Some(1),
    );
    let cancellable = gio::Cancellable::new();
    let _cancellation =
        CancellableWatcher::new(progress.reporter.cancellation_token(), &cancellable);
    let info = destination.query_info(
        "standard::type,standard::size",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        Some(&cancellable),
    )?;
    if info.file_type() != FileType::Regular {
        return Err(MountedMtpError::UploadMetadata(path.clone()));
    }
    let reported_size =
        u64::try_from(info.size()).map_err(|_| MountedMtpError::NegativeObjectSize(info.size()))?;
    if reported_size != expected_size {
        return Err(MountedMtpError::UploadMetadata(path.clone()));
    }
    progress.reporter.completed_operations_with_path(
        OperationStage::DeviceVerify,
        "Uploaded file path and size checked",
        path.to_string(),
        1,
        Some(1),
    );
    Ok(())
}

fn cleanup_failed_upload(
    destination: &File,
    path: &SafeRelativePath,
    operation: MountedMtpError,
    progress: &ProgressReporter,
) -> MountedMtpError {
    match discard_partial_object(destination, &path.to_string(), progress) {
        Ok(()) => operation,
        Err(cleanup) => MountedMtpError::UploadMetadataCleanup {
            operation: operation.to_string(),
            cleanup,
        },
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one exact restore contract plus its cancellation context"
)]
fn restore_mounted_mtp_object_blocking_with_cancellable(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    backup: &Path,
    expected_sha256: &str,
    cancellable: Option<&gio::Cancellable>,
    progress: Option<&ProgressReporter>,
) -> Result<(), MountedMtpError> {
    let (backup_size, backup_sha256) = hash_local_file_with_cancellation(backup, progress)?;
    if backup_size != expected_size || backup_sha256 != expected_sha256 {
        return Err(MountedMtpError::BackupVerification(backup.to_owned()));
    }
    let root = mounted_root(mount_id)?;
    let storage = storage_for_id_with_cancellable(&root, storage_id, cancellable)?;
    let (parent, name) = mounted_parent_with_cancellable(&storage, path, cancellable)?;
    let destination = parent.child(name);
    if destination.query_exists(cancellable) {
        return Err(MountedMtpError::RemovalRestoreExists(path.clone()));
    }
    if let Err(upload) =
        File::for_path(backup).copy(&destination, FileCopyFlags::NONE, cancellable, None)
    {
        return Err(cleanup_failed_restore(
            &destination,
            path,
            MountedMtpError::Gio(upload),
            progress,
        ));
    }
    let metadata_check = (|| {
        if let Some(progress) = progress {
            progress.started_operations_with_path(
                OperationStage::DeviceVerify,
                "Checking restored file path and size",
                path.to_string(),
                Some(1),
            );
        }
        mounted_regular_file_with_cancellable(
            mount_id,
            storage_id,
            path,
            expected_size,
            cancellable,
        )?;
        if let Some(progress) = progress {
            progress.completed_operations_with_path(
                OperationStage::DeviceVerify,
                "Restored file path and size checked",
                path.to_string(),
                1,
                Some(1),
            );
        }
        Ok(())
    })();
    if let Err(error) = metadata_check {
        return Err(cleanup_failed_restore(&destination, path, error, progress));
    }
    Ok(())
}

fn cleanup_failed_restore(
    destination: &File,
    path: &SafeRelativePath,
    operation: MountedMtpError,
    progress: Option<&ProgressReporter>,
) -> MountedMtpError {
    let default_progress = ProgressReporter::default();
    let progress = progress.unwrap_or(&default_progress);
    match discard_partial_object(destination, &path.to_string(), progress) {
        Ok(()) => operation,
        Err(cleanup) => MountedMtpError::RemovalRestoreCleanup {
            operation: operation.to_string(),
            cleanup,
        },
    }
}

fn mounted_regular_file_with_cancellable(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    cancellable: Option<&gio::Cancellable>,
) -> Result<File, MountedMtpError> {
    let root = mounted_root(mount_id)?;
    let storage = storage_for_id_with_cancellable(&root, storage_id, cancellable)?;
    let (parent, name) = mounted_parent_with_cancellable(&storage, path, cancellable)?;
    match child_metadata_with_cancellable(&parent, &name, cancellable)? {
        ChildMetadata::Unique {
            file,
            file_type: FileType::Regular,
            size,
        } => {
            let actual =
                u64::try_from(size).map_err(|_| MountedMtpError::NegativeObjectSize(size))?;
            if actual != expected_size {
                return Err(MountedMtpError::RemovalObjectSize {
                    path: path.clone(),
                    expected: expected_size,
                    actual,
                });
            }
            Ok(file)
        }
        ChildMetadata::Missing => Err(MountedMtpError::RemovalObjectState {
            path: path.clone(),
            state: DevicePathState::Missing,
        }),
        ChildMetadata::Ambiguous => Err(MountedMtpError::RemovalObjectState {
            path: path.clone(),
            state: DevicePathState::Ambiguous,
        }),
        ChildMetadata::Unique { .. } => Err(MountedMtpError::RemovalObjectState {
            path: path.clone(),
            state: DevicePathState::Other,
        }),
    }
}

fn storage_for_id_with_cancellable(
    root: &File,
    storage_id: &str,
    cancellable: Option<&gio::Cancellable>,
) -> Result<File, MountedMtpError> {
    garmin_storages_with_cancellable(root, cancellable)?
        .into_iter()
        .map(|(storage, _)| storage)
        .find(|storage| mount_id_for_uri(&storage.uri()) == storage_id)
        .ok_or_else(|| MountedMtpError::StorageNotFound(storage_id.to_owned()))
}

fn mounted_parent_with_cancellable(
    storage: &File,
    path: &SafeRelativePath,
    cancellable: Option<&gio::Cancellable>,
) -> Result<(File, String), MountedMtpError> {
    let mut components = path
        .as_path()
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let name = components
        .pop()
        .ok_or_else(|| MountedMtpError::RemovalObjectState {
            path: path.clone(),
            state: DevicePathState::Other,
        })?;
    let mut parent = storage.clone();
    for component in components {
        parent = match child_metadata_with_cancellable(&parent, &component, cancellable)? {
            ChildMetadata::Unique {
                file,
                file_type: FileType::Directory,
                ..
            } => file,
            ChildMetadata::Missing => {
                return Err(MountedMtpError::RemovalObjectState {
                    path: path.clone(),
                    state: DevicePathState::Missing,
                });
            }
            ChildMetadata::Ambiguous => {
                return Err(MountedMtpError::RemovalObjectState {
                    path: path.clone(),
                    state: DevicePathState::Ambiguous,
                });
            }
            ChildMetadata::Unique { .. } => {
                return Err(MountedMtpError::RemovalObjectState {
                    path: path.clone(),
                    state: DevicePathState::Other,
                });
            }
        };
    }
    Ok((parent, name))
}

fn mounted_directory_with_cancellable(
    storage: &File,
    path: &SafeRelativePath,
    cancellable: Option<&gio::Cancellable>,
) -> Result<File, MountedMtpError> {
    let mut directory = storage.clone();
    for component in path.as_path().components() {
        let name = component.as_os_str().to_string_lossy();
        directory = match child_metadata_with_cancellable(&directory, &name, cancellable)? {
            ChildMetadata::Unique {
                file,
                file_type: FileType::Directory,
                ..
            } => file,
            ChildMetadata::Missing => {
                return Err(MountedMtpError::MetadataPathState {
                    path: path.clone(),
                    state: DevicePathState::Missing,
                });
            }
            ChildMetadata::Ambiguous => {
                return Err(MountedMtpError::MetadataPathState {
                    path: path.clone(),
                    state: DevicePathState::Ambiguous,
                });
            }
            ChildMetadata::Unique { .. } => {
                return Err(MountedMtpError::MetadataPathState {
                    path: path.clone(),
                    state: DevicePathState::Other,
                });
            }
        };
    }
    Ok(directory)
}

fn hash_local_file(path: &Path) -> Result<(u64, String), MountedMtpError> {
    hash_local_file_with_cancellation(path, None)
}

fn hash_local_file_with_cancellation(
    path: &Path,
    progress: Option<&ProgressReporter>,
) -> Result<(u64, String), MountedMtpError> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(MountedMtpError::BackupVerification(path.to_owned()));
    }
    let mut file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(MountedMtpError::BackupVerification(path.to_owned()));
    }
    hash_local_reader(&mut file, progress)
}

fn hash_local_reader(
    file: &mut StdFile,
    progress: Option<&ProgressReporter>,
) -> Result<(u64, String), MountedMtpError> {
    let mut size = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if progress.is_some_and(ProgressReporter::is_cancelled) {
            return Err(MountedMtpError::Cancelled);
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(count).map_err(|_| MountedMtpError::SizeOverflow)?)
            .ok_or(MountedMtpError::SizeOverflow)?;
        hash.update(&buffer[..count]);
    }
    if progress.is_some_and(ProgressReporter::is_cancelled) {
        return Err(MountedMtpError::Cancelled);
    }
    Ok((size, hex::encode(hash.finalize())))
}

fn hash_mounted_file_with_progress(
    file: &File,
    progress: Option<&ProgressReporter>,
    mut report: impl FnMut(u64),
) -> Result<(u64, String), MountedMtpError> {
    if progress.is_some_and(ProgressReporter::is_cancelled) {
        return Err(MountedMtpError::Cancelled);
    }
    let cancellable = gio::Cancellable::new();
    let _cancellation = progress
        .map(|progress| CancellableWatcher::new(progress.cancellation_token(), &cancellable));
    let stream = file.read(Some(&cancellable))?;
    let mut size = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if progress.is_some_and(ProgressReporter::is_cancelled) {
            cancellable.cancel();
            return Err(MountedMtpError::Cancelled);
        }
        let count = match stream.read(&mut buffer, Some(&cancellable)) {
            Ok(count) => count,
            Err(_) if progress.is_some_and(ProgressReporter::is_cancelled) => {
                return Err(MountedMtpError::Cancelled);
            }
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(count).map_err(|_| MountedMtpError::SizeOverflow)?)
            .ok_or(MountedMtpError::SizeOverflow)?;
        hash.update(&buffer[..count]);
        report(size);
    }
    Ok((size, hex::encode(hash.finalize())))
}

/// Copies a download to mounted MTP, verifies its size, then removes it.
/// # Errors
/// Unreadable source, unavailable or ambiguous mount, or transfer failure.
pub async fn probe_mounted_mtp_file(
    mount_id: &str,
    source: &Path,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let source = source.to_owned();
    let progress = progress.clone();
    tokio::task::spawn_blocking(move || {
        probe_mounted_mtp_file_blocking(&mount_id, &source, &progress)
    })
    .await
    .map_err(MountedMtpError::Task)?
}

fn probe_mounted_mtp_file_blocking(
    mount_id: &str,
    source: &Path,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MountedMtpError> {
    let root = mounted_root(mount_id)?;
    let garmin = exactly_one_garmin_directory(&root)?;
    let bytes = source.metadata()?.len();
    let (source_size, expected_sha256) = hash_local_file(source)?;
    if source_size != bytes {
        return Err(MountedMtpError::ProbeSize {
            expected: bytes,
            actual: source_size,
        });
    }
    let destination = garmin.child(format!(".garmin-cli-probe-{}.bin", Uuid::new_v4()));
    let readback = source.with_file_name(format!(".garmin-cli-readback-{}.bin", Uuid::new_v4()));

    let (upload_elapsed, finalize_elapsed) =
        upload_probe_file(source, &destination, bytes, progress)?;
    let read_back_elapsed =
        match verify_probe_file(&destination, &readback, bytes, &expected_sha256, progress) {
            Ok(elapsed) => elapsed,
            Err(operation) => {
                let destination_name = destination.basename().map_or_else(
                    || "temporary probe".into(),
                    |name| name.to_string_lossy().into_owned(),
                );
                return match discard_partial_object(&destination, &destination_name, progress) {
                    Ok(()) => Err(operation),
                    Err(cleanup) => Err(MountedMtpError::ProbeVerificationCleanup {
                        operation: operation.to_string(),
                        cleanup,
                    }),
                };
            }
        };
    let cleanup_elapsed = delete_probe_file(&destination, progress)?;

    Ok(DeviceProbeReport {
        transport: TransportKind::MountedMtp,
        bytes,
        upload_seconds: upload_elapsed.as_secs_f64(),
        device_finalize_seconds: finalize_elapsed.as_secs_f64(),
        upload_completion: UploadCompletion::Acknowledged,
        read_back_seconds: read_back_elapsed.as_secs_f64(),
        cleanup_seconds: cleanup_elapsed.as_secs_f64(),
        content_verified: true,
        temporary_file_removed: true,
    })
}

fn upload_probe_file(
    source: &Path,
    destination: &File,
    bytes: u64,
    progress: &ProgressReporter,
) -> Result<(std::time::Duration, std::time::Duration), MountedMtpError> {
    progress.started(
        OperationStage::Upload,
        "Writing through the desktop MTP mount",
        Some(bytes),
    );
    let upload_started = Instant::now();
    let source = File::for_path(source);
    let cancellable = gio::Cancellable::new();
    if progress.is_cancelled() {
        cancellable.cancel();
    }
    let callback_cancellable = cancellable.clone();
    let mut payload_elapsed = None;
    let mut copy_progress = |current: i64, _total: i64| {
        if progress.is_cancelled() {
            callback_cancellable.cancel();
        }
        if let Ok(current) = u64::try_from(current) {
            progress.advanced(
                OperationStage::Upload,
                "Writing through the desktop MTP mount",
                current,
                Some(bytes),
            );
            if current >= bytes && payload_elapsed.is_none() {
                payload_elapsed = Some(upload_started.elapsed());
                progress.completed(
                    OperationStage::Upload,
                    "Payload sent through the desktop MTP mount",
                    bytes,
                    Some(bytes),
                );
                progress.started(
                    OperationStage::DeviceFinalize,
                    "Waiting for the desktop MTP mount to finalize the upload",
                    None,
                );
            }
        }
    };
    if let Err(error) = source.copy(
        destination,
        FileCopyFlags::NONE,
        Some(&cancellable),
        Some(&mut copy_progress),
    ) {
        let destination_name = destination.basename().map_or_else(
            || "temporary probe".into(),
            |name| name.to_string_lossy().into_owned(),
        );
        let cleanup = discard_partial_object(destination, &destination_name, progress);
        progress.failed(
            if payload_elapsed.is_some() {
                OperationStage::DeviceFinalize
            } else {
                OperationStage::Upload
            },
            "Desktop MTP upload failed",
        );
        if progress.is_cancelled() {
            if let Err(cleanup) = cleanup {
                return Err(MountedMtpError::ProbeUploadCleanup {
                    upload: error,
                    cleanup,
                });
            }
            return Err(MountedMtpError::Cancelled);
        }
        if let Err(cleanup) = cleanup {
            return Err(MountedMtpError::ProbeUploadCleanup {
                upload: error,
                cleanup,
            });
        }
        return Err(MountedMtpError::ProbeUpload(error));
    }
    let payload_elapsed = payload_elapsed.unwrap_or_else(|| {
        let elapsed = upload_started.elapsed();
        progress.completed(
            OperationStage::Upload,
            "Payload sent through the desktop MTP mount",
            bytes,
            Some(bytes),
        );
        progress.started(
            OperationStage::DeviceFinalize,
            "Waiting for the desktop MTP mount to finalize the upload",
            None,
        );
        elapsed
    });
    let finalize_elapsed = upload_started.elapsed().saturating_sub(payload_elapsed);
    progress.completed(
        OperationStage::DeviceFinalize,
        "Desktop MTP upload finalized",
        1,
        Some(1),
    );
    Ok((payload_elapsed, finalize_elapsed))
}

fn discard_partial_object(
    destination: &File,
    path: &str,
    progress: &ProgressReporter,
) -> Result<(), gio::glib::Error> {
    let cleanup = progress
        .clone()
        .for_required_cleanup()
        .for_item(format!("discard-partial:{path}"));
    cleanup.started_operations_with_path(
        OperationStage::Cleanup,
        "Removing incomplete device upload",
        path,
        Some(1),
    );
    let cancellable = gio::Cancellable::new();
    let _deadline = CancellableWatcher::with_timeout(
        cleanup.cancellation_token(),
        &cancellable,
        REQUIRED_CLEANUP_TIMEOUT,
    );
    match destination.delete(Some(&cancellable)) {
        Ok(()) => {
            cleanup.completed_operations_with_path(
                OperationStage::Cleanup,
                "Removed incomplete device upload",
                path,
                1,
                Some(1),
            );
            Ok(())
        }
        Err(error) if error.kind::<gio::IOErrorEnum>() == Some(gio::IOErrorEnum::NotFound) => {
            cleanup.completed_operations_with_path(
                OperationStage::Cleanup,
                "No incomplete device upload remained",
                path,
                1,
                Some(1),
            );
            Ok(())
        }
        Err(error) => {
            cleanup.failed_with_path(
                OperationStage::Cleanup,
                "Incomplete device upload cleanup failed or timed out",
                path,
            );
            Err(error)
        }
    }
}

fn verify_probe_file(
    destination: &File,
    readback_path: &Path,
    bytes: u64,
    expected_sha256: &str,
    progress: &ProgressReporter,
) -> Result<std::time::Duration, MountedMtpError> {
    progress.started(
        OperationStage::DeviceVerify,
        "Reading desktop MTP object back",
        Some(bytes),
    );
    let verification_started = Instant::now();
    let readback = File::for_path(readback_path);
    let cancellable = gio::Cancellable::new();
    let callback_cancellable = cancellable.clone();
    let mut copy_progress = |current: i64, _total: i64| {
        if progress.is_cancelled() {
            callback_cancellable.cancel();
        }
        if let Ok(current) = u64::try_from(current) {
            progress.advanced(
                OperationStage::DeviceVerify,
                "Reading desktop MTP object back",
                current,
                Some(bytes),
            );
        }
    };
    if let Err(error) = destination.copy(
        &readback,
        FileCopyFlags::NONE,
        Some(&cancellable),
        Some(&mut copy_progress),
    ) {
        progress.failed(OperationStage::DeviceVerify, "Desktop MTP read-back failed");
        let operation = if progress.is_cancelled() {
            MountedMtpError::Cancelled
        } else {
            MountedMtpError::ProbeReadBack(error)
        };
        return remove_readback_after_error(readback_path, operation);
    }
    let readback_hash = hash_local_file(readback_path);
    let cleanup = remove_local_readback(readback_path);
    let (actual, actual_sha256) = match (readback_hash, cleanup) {
        (Ok(result), Ok(())) => result,
        (Err(operation), Ok(())) => {
            progress.failed(OperationStage::DeviceVerify, "Desktop MTP read-back failed");
            return Err(operation);
        }
        (Ok(_), Err(cleanup)) => {
            progress.failed(OperationStage::DeviceVerify, "Read-back cleanup failed");
            return Err(cleanup.into());
        }
        (Err(operation), Err(cleanup)) => {
            progress.failed(OperationStage::DeviceVerify, "Desktop MTP read-back failed");
            return Err(MountedMtpError::ProbeReadBackCleanup {
                operation: operation.to_string(),
                cleanup,
            });
        }
    };
    if actual != bytes {
        progress.failed(
            OperationStage::DeviceVerify,
            "Desktop MTP read-back size mismatch",
        );
        return Err(MountedMtpError::ProbeSize {
            expected: bytes,
            actual,
        });
    }
    if actual_sha256 != expected_sha256 {
        progress.failed(
            OperationStage::DeviceVerify,
            "Desktop MTP read-back checksum mismatch",
        );
        return Err(MountedMtpError::ProbeChecksum);
    }
    let verification_elapsed = verification_started.elapsed();
    progress.completed(
        OperationStage::DeviceVerify,
        "Read back and SHA-256 verified",
        actual,
        Some(bytes),
    );
    Ok(verification_elapsed)
}

fn remove_readback_after_error(
    readback_path: &Path,
    operation: MountedMtpError,
) -> Result<std::time::Duration, MountedMtpError> {
    match remove_local_readback(readback_path) {
        Ok(()) => Err(operation),
        Err(cleanup) => Err(MountedMtpError::ProbeReadBackCleanup {
            operation: operation.to_string(),
            cleanup,
        }),
    }
}

fn remove_local_readback(path: &Path) -> Result<(), std::io::Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn delete_probe_file(
    destination: &File,
    progress: &ProgressReporter,
) -> Result<std::time::Duration, MountedMtpError> {
    let cleanup = progress.clone().for_required_cleanup();
    cleanup.started(
        OperationStage::Delete,
        "Removing disposable device object",
        Some(1),
    );
    let deletion_started = Instant::now();
    let cancellable = gio::Cancellable::new();
    let _deadline = CancellableWatcher::with_timeout(
        cleanup.cancellation_token(),
        &cancellable,
        REQUIRED_CLEANUP_TIMEOUT,
    );
    if let Err(error) = destination.delete(Some(&cancellable)) {
        cleanup.failed(
            OperationStage::Delete,
            "Disposable object deletion failed or timed out",
        );
        return Err(error.into());
    }
    let removed = !destination.query_exists(Some(&cancellable));
    let deletion_elapsed = deletion_started.elapsed();
    if !removed {
        cleanup.failed(OperationStage::Delete, "Disposable object still exists");
        return Err(MountedMtpError::ProbeNotRemoved);
    }
    cleanup.completed(
        OperationStage::Delete,
        "Disposable object removed",
        1,
        Some(1),
    );
    Ok(deletion_elapsed)
}

fn mounted_root(mount_id: &str) -> Result<File, MountedMtpError> {
    mounted_roots()
        .into_iter()
        .find(|(candidate, _)| candidate.mount_id == mount_id)
        .map(|(_, root)| root)
        .ok_or_else(|| MountedMtpError::NotFound(mount_id.to_owned()))
}

fn garmin_directories(root: &File) -> Result<Vec<File>, MountedMtpError> {
    Ok(garmin_storages(root)?
        .into_iter()
        .map(|(_, garmin)| garmin)
        .collect())
}

fn garmin_storages(root: &File) -> Result<Vec<(File, File)>, MountedMtpError> {
    garmin_storages_with_cancellable(root, None)
}

fn garmin_storages_with_cancellable(
    root: &File,
    cancellable: Option<&gio::Cancellable>,
) -> Result<Vec<(File, File)>, MountedMtpError> {
    storage_roots_with_cancellable(root, cancellable)?
        .into_iter()
        .filter_map(|storage| {
            match child_named_with_cancellable(&storage, "Garmin", FileType::Directory, cancellable)
            {
                Ok(Some(garmin)) => Some(Ok((storage, garmin))),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect()
}

#[cfg(test)]
fn storage_roots(root: &File) -> Result<Vec<File>, MountedMtpError> {
    storage_roots_with_cancellable(root, None)
}

fn storage_roots_with_cancellable(
    root: &File,
    cancellable: Option<&gio::Cancellable>,
) -> Result<Vec<File>, MountedMtpError> {
    if child_named_with_cancellable(root, "Garmin", FileType::Directory, cancellable)?.is_some() {
        return Ok(vec![root.clone()]);
    }
    let children = child_directories_with_cancellable(root, cancellable)?;
    Ok(if children.is_empty() {
        vec![root.clone()]
    } else {
        children
    })
}

fn storage_label(storage: &File, index: usize) -> String {
    storage_label_with_cancellable(storage, index, None)
}

fn storage_label_with_cancellable(
    storage: &File,
    index: usize,
    cancellable: Option<&gio::Cancellable>,
) -> String {
    storage
        .query_info(
            "standard::display-name",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            cancellable,
        )
        .ok()
        .map(|info| info.display_name().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| format!("Device storage {}", index + 1))
}

fn exactly_one_garmin_directory(root: &File) -> Result<File, MountedMtpError> {
    let directories = garmin_directories(root)?;
    match directories.as_slice() {
        [] => Err(MountedMtpError::GarminDirectoryMissing),
        [directory] => Ok(directory.clone()),
        _ => Err(MountedMtpError::AmbiguousGarminDirectory),
    }
}

fn mounted_roots() -> Vec<(MountedMtpCandidate, File)> {
    crate::system::mounted_catalog::mounted_candidates()
        .into_iter()
        .map(|candidate| {
            let root = candidate.root().clone();
            (candidate, root)
        })
        .collect()
}

pub(crate) fn mount_id_for_uri(uri: &str) -> String {
    let digest = Sha256::digest(uri.as_bytes());
    hex::encode(&digest[..8])
}

fn child_directories_with_cancellable(
    parent: &File,
    cancellable: Option<&gio::Cancellable>,
) -> Result<Vec<File>, MountedMtpError> {
    let enumerator = parent.enumerate_children(
        CHILD_ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        cancellable,
    )?;
    let mut directories = Vec::new();
    while let Some(info) = enumerator.next_file(cancellable)? {
        if info.file_type() == FileType::Directory {
            directories.push(enumerator.child(&info));
        }
    }
    directories.sort_by_key(FileExt::uri);
    Ok(directories)
}

fn child_named(
    parent: &File,
    expected: &str,
    expected_type: FileType,
) -> Result<Option<File>, MountedMtpError> {
    child_named_with_cancellable(parent, expected, expected_type, None)
}

fn child_named_with_cancellable(
    parent: &File,
    expected: &str,
    expected_type: FileType,
    cancellable: Option<&gio::Cancellable>,
) -> Result<Option<File>, MountedMtpError> {
    let enumerator = parent.enumerate_children(
        CHILD_ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        cancellable,
    )?;
    while let Some(info) = enumerator.next_file(cancellable)? {
        if info.file_type() == expected_type
            && info.name().to_string_lossy().eq_ignore_ascii_case(expected)
        {
            return Ok(Some(enumerator.child(&info)));
        }
    }
    Ok(None)
}

fn read_bounded(source: &File) -> Result<String, MountedMtpError> {
    let stream = source.read(gio::Cancellable::NONE)?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = stream.read(&mut buffer, gio::Cancellable::NONE)?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) > MAX_MANIFEST_BYTES {
            return Err(MountedMtpError::ManifestTooLarge);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(bytes).map_err(Into::into)
}

#[derive(Debug, Error)]
pub enum MountedMtpError {
    #[error("mounted MTP operation failed: {0}")]
    Gio(#[from] gio::glib::Error),
    #[error("mounted MTP device {0} is no longer available")]
    NotFound(String),
    #[error("mounted MTP metadata path {path} has unsafe state {state:?}")]
    MetadataPathState {
        path: SafeRelativePath,
        state: DevicePathState,
    },
    #[error("mounted MTP metadata file {path} is {size} bytes; limit is {limit} bytes")]
    MetadataTooLarge {
        path: SafeRelativePath,
        limit: u64,
        size: u64,
    },
    #[error("GarminDevice.xml was not found on the mounted MTP device")]
    ManifestMissing,
    #[error("multiple GarminDevice.xml files occupied canonical locations")]
    AmbiguousManifest,
    #[error("a Garmin directory was not found on the mounted MTP device")]
    GarminDirectoryMissing,
    #[error("multiple Garmin directories were found on the mounted MTP device")]
    AmbiguousGarminDirectory,
    #[error("GarminDevice.xml exceeded {MAX_MANIFEST_BYTES} bytes")]
    ManifestTooLarge,
    #[error("GarminDevice.xml was not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("local file I/O failed: {0}")]
    LocalIo(#[from] std::io::Error),
    #[error("desktop MTP upload failed: {0}")]
    ProbeUpload(#[source] gio::glib::Error),
    #[error("desktop MTP upload failed ({upload}); partial-object cleanup failed ({cleanup})")]
    ProbeUploadCleanup {
        upload: gio::glib::Error,
        cleanup: gio::glib::Error,
    },
    #[error("desktop MTP verification failed ({operation}); object cleanup failed ({cleanup})")]
    ProbeVerificationCleanup {
        operation: String,
        cleanup: gio::glib::Error,
    },
    #[error("desktop MTP upload cancelled")]
    Cancelled,
    #[error("desktop MTP read-back failed: {0}")]
    ProbeReadBack(#[source] gio::glib::Error),
    #[error("desktop MTP read-back failed ({operation}); local cleanup failed ({cleanup})")]
    ProbeReadBackCleanup {
        operation: String,
        cleanup: std::io::Error,
    },
    #[error("device reported a negative object size: {0}")]
    NegativeObjectSize(i64),
    #[error("device-side size mismatch: expected {expected}, found {actual}")]
    ProbeSize { expected: u64, actual: u64 },
    #[error("desktop MTP read-back checksum did not match the source")]
    ProbeChecksum,
    #[error("disposable device object still exists after deletion")]
    ProbeNotRemoved,
    #[error("mounted MTP storage {0} is no longer available")]
    StorageNotFound(String),
    #[error("mounted MTP object {path} has unsafe state {state:?}")]
    RemovalObjectState {
        path: SafeRelativePath,
        state: DevicePathState,
    },
    #[error("mounted MTP object {path} changed size: expected {expected}, found {actual}")]
    RemovalObjectSize {
        path: SafeRelativePath,
        expected: u64,
        actual: u64,
    },
    #[error("mounted MTP object {0} changed contents after backup")]
    RemovalObjectChecksum(SafeRelativePath),
    #[error("removal backup could not be independently verified: {0}")]
    BackupVerification(std::path::PathBuf),
    #[error("mounted MTP backup sink failed verification for {0}")]
    BackupSinkVerification(SafeRelativePath),
    #[error("mounted MTP object still exists after removal: {0}")]
    RemovalObjectNotRemoved(SafeRelativePath),
    #[error("refusing to overwrite an object while restoring {0}")]
    RemovalRestoreExists(SafeRelativePath),
    #[error("restoration failed ({operation}); partial-object cleanup failed ({cleanup})")]
    RemovalRestoreCleanup {
        operation: String,
        cleanup: gio::glib::Error,
    },
    #[error("update payload could not be independently verified: {0}")]
    UploadSourceVerification(std::path::PathBuf),
    #[error("refusing to overwrite mounted MTP object {0}")]
    UploadDestinationExists(SafeRelativePath),
    #[error("mounted MTP update upload failed: {0}")]
    Upload(#[source] gio::glib::Error),
    #[error(
        "mounted MTP update upload failed ({upload}); partial-object cleanup failed ({cleanup})"
    )]
    UploadCleanup {
        upload: gio::glib::Error,
        cleanup: gio::glib::Error,
    },
    #[error("uploaded mounted MTP object {0} has invalid destination metadata")]
    UploadMetadata(SafeRelativePath),
    #[error(
        "upload metadata check failed ({operation}); invalid-object cleanup failed ({cleanup})"
    )]
    UploadMetadataCleanup {
        operation: String,
        cleanup: gio::glib::Error,
    },
    #[error("mounted MTP byte count exceeds the supported range")]
    SizeOverflow,
    #[error("mounted MTP worker failed: {0}")]
    Task(#[source] tokio::task::JoinError),
}

impl MountedMtpError {
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    #[must_use]
    pub fn probe_failure(&self) -> Option<MountedMtpProbeFailure> {
        match self {
            Self::ProbeUpload(source) => Some(MountedMtpProbeFailure::Upload {
                reason: source.to_string(),
            }),
            Self::ProbeUploadCleanup { upload, cleanup } => {
                Some(MountedMtpProbeFailure::UploadCleanup {
                    upload: upload.to_string(),
                    cleanup: cleanup.to_string(),
                })
            }
            Self::ProbeReadBack(source) => Some(MountedMtpProbeFailure::ReadBack {
                reason: source.to_string(),
            }),
            Self::ProbeVerificationCleanup { .. } | Self::ProbeReadBackCleanup { .. } => {
                Some(MountedMtpProbeFailure::ReadBack {
                    reason: self.to_string(),
                })
            }
            Self::ProbeSize { expected, actual } => Some(MountedMtpProbeFailure::Size {
                expected: *expected,
                actual: *actual,
            }),
            Self::ProbeChecksum => Some(MountedMtpProbeFailure::Checksum),
            Self::ProbeNotRemoved => Some(MountedMtpProbeFailure::NotRemoved),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CancellableWatcher, MountedMtpError, MountedMtpUploadProgress, cleanup_failed_upload,
        hash_mounted_file_with_progress, mount_id_for_uri, read_bounded_mounted_file,
        storage_roots, upload_probe_file, upload_to_storage_blocking, verify_probe_file,
    };
    use crate::SafeRelativePath;
    use garmin_progress::{OperationStage, ProgressReporter, ProgressState};
    use gio::File;
    use gio::prelude::CancellableExt;
    use gio::prelude::FileExt as _;
    use sha2::{Digest as _, Sha256};

    #[test]
    fn mount_ids_are_stable_and_do_not_expose_the_uri() {
        let uri = "mtp://Garmin_Device_private-identifier/";
        assert_eq!(mount_id_for_uri(uri), mount_id_for_uri(uri));
        assert!(!mount_id_for_uri(uri).contains("private"));
    }

    #[test]
    fn capacity_enumeration_keeps_an_empty_storage_volume() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(fixture.path().join("Internal/Garmin")).unwrap();
        std::fs::create_dir(fixture.path().join("Memory card")).unwrap();
        let roots = storage_roots(&File::for_path(fixture.path())).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(
            roots
                .iter()
                .any(|root| root.uri().ends_with("/Memory%20card"))
        );
    }

    #[test]
    fn bounded_read_treats_a_missing_parent_directory_as_an_absent_file() {
        let fixture = tempfile::tempdir().unwrap();
        let path = SafeRelativePath::parse("GARMIN-TOOLKIT/manifest.toml").unwrap();

        let bytes = read_bounded_mounted_file(&File::for_path(fixture.path()), &path, 1024)
            .expect("missing metadata is not an unsafe object");

        assert_eq!(bytes, None);
    }

    #[test]
    fn mounted_hash_stops_when_verification_is_cancelled() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("map.img");
        std::fs::write(&path, b"verified map payload").unwrap();
        let progress = ProgressReporter::default();
        progress.cancellation_token().cancel();

        let result =
            hash_mounted_file_with_progress(&File::for_path(path), Some(&progress), |_| {});

        assert!(matches!(result, Err(MountedMtpError::Cancelled)));
    }

    #[test]
    fn cancellation_watcher_interrupts_a_blocked_gio_operation() {
        let progress = ProgressReporter::default();
        let cancellation = progress.cancellation_token();
        let cancellable = gio::Cancellable::new();
        let _watcher = CancellableWatcher::new(cancellation.clone(), &cancellable);

        cancellation.cancel();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !cancellable.is_cancelled() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }

        assert!(cancellable.is_cancelled());
    }

    #[test]
    fn cancellation_watcher_enforces_a_cleanup_deadline() {
        let cancellable = gio::Cancellable::new();
        let _watcher = CancellableWatcher::with_timeout(
            ProgressReporter::default().cancellation_token(),
            &cancellable,
            std::time::Duration::from_millis(1),
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !cancellable.is_cancelled() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }

        assert!(cancellable.is_cancelled());
    }

    #[test]
    fn uploads_a_locally_verified_object_and_checks_device_metadata() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = fixture.path().join("storage");
        let source = fixture.path().join("source.img");
        std::fs::create_dir_all(storage.join("Garmin")).unwrap();
        std::fs::write(&source, b"verified map payload").unwrap();
        let path = SafeRelativePath::parse("Garmin/map.img").unwrap();
        let sha256 = hex::encode(Sha256::digest(b"verified map payload"));
        let (progress, receiver) = ProgressReporter::channel();

        upload_to_storage_blocking(
            &File::for_path(&storage),
            &path,
            &source,
            20,
            &sha256,
            &MountedMtpUploadProgress {
                reporter: progress,
                completed_before: 0,
                total: 20,
            },
        )
        .unwrap();

        assert_eq!(
            std::fs::read(storage.join("Garmin/map.img")).unwrap(),
            b"verified map payload"
        );
        let events = receiver.try_iter().collect::<Vec<_>>();
        let position = |stage, state| {
            events
                .iter()
                .position(|event| event.stage == stage && event.state == state)
                .unwrap()
        };
        let finalize_started = position(OperationStage::DeviceFinalize, ProgressState::Started);
        let finalize_completed = position(OperationStage::DeviceFinalize, ProgressState::Completed);
        let verify_completed = position(OperationStage::DeviceVerify, ProgressState::Completed);
        assert!(finalize_started < finalize_completed);
        assert!(finalize_completed < verify_completed);
        assert_eq!(
            events[verify_completed].unit,
            garmin_progress::ProgressUnit::Operations
        );
        assert_eq!(
            events[verify_completed].label,
            "Uploaded file path and size checked"
        );
    }

    #[test]
    fn failed_upload_cleanup_removes_the_object_and_reports_progress() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = fixture.path().join("storage");
        std::fs::create_dir_all(storage.join("Garmin")).unwrap();
        std::fs::write(storage.join("Garmin/map.img"), b"invalid map payload").unwrap();
        let path = SafeRelativePath::parse("Garmin/map.img").unwrap();
        let (progress, receiver) = ProgressReporter::channel();

        let result = cleanup_failed_upload(
            &File::for_path(storage.join("Garmin/map.img")),
            &path,
            MountedMtpError::UploadMetadata(path.clone()),
            &progress,
        );

        assert!(matches!(result, MountedMtpError::UploadMetadata(_)));
        assert!(!storage.join("Garmin/map.img").exists());
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(events.iter().any(|event| {
            event.stage == OperationStage::Cleanup
                && event.state == ProgressState::Started
                && event.label == "Removing incomplete device upload"
                && event.path.as_deref() == Some("Garmin/map.img")
        }));
        assert!(events.iter().any(|event| {
            event.stage == OperationStage::Cleanup
                && event.state == ProgressState::Completed
                && event.label == "Removed incomplete device upload"
                && event.path.as_deref() == Some("Garmin/map.img")
        }));
    }

    #[test]
    fn mounted_probe_reports_payload_before_device_finalization() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.img");
        let destination = fixture.path().join("device.img");
        std::fs::write(&source, b"verified map payload").unwrap();
        let (progress, receiver) = ProgressReporter::channel();

        upload_probe_file(&source, &File::for_path(&destination), 20, &progress).unwrap();
        let events = receiver.try_iter().collect::<Vec<_>>();
        let upload_complete = events
            .iter()
            .position(|event| {
                event.stage == OperationStage::Upload && event.state == ProgressState::Completed
            })
            .unwrap();
        let finalize_started = events
            .iter()
            .position(|event| {
                event.stage == OperationStage::DeviceFinalize
                    && event.state == ProgressState::Started
            })
            .unwrap();

        assert!(upload_complete < finalize_started);
    }

    #[test]
    fn probe_read_back_leaves_device_cleanup_to_its_caller() {
        let fixture = tempfile::tempdir().unwrap();
        let device_file = fixture.path().join("device.bin");
        let readback = fixture.path().join("readback.bin");
        std::fs::write(&device_file, b"other-data").unwrap();
        let expected = hex::encode(Sha256::digest(b"source-map"));

        let result = verify_probe_file(
            &File::for_path(&device_file),
            &readback,
            10,
            &expected,
            &ProgressReporter::default(),
        );

        assert!(matches!(result, Err(MountedMtpError::ProbeChecksum)));
        assert!(device_file.exists());
        assert!(!readback.exists());
    }
}
