use crate::manifest::{MAX_MANIFEST_BYTES, ManifestError, parse_manifest};
use crate::storage::BackupDestination;
use crate::system::{MountedMtpBackupProgress, MountedMtpProbeFailure, MountedMtpUploadProgress};
use crate::{
    DeviceInventory, DeviceManifest, DevicePathInspection, DevicePathState, SafeRelativePath,
    TransportKind,
};
use crate::{DeviceProbeReport, UploadCompletion};
use garmin_progress::{OperationStage, ProgressReporter};
use gio::prelude::{CancellableExt, FileEnumeratorExt, FileExt, InputStreamExtManual};
use gio::{File, FileCopyFlags, FileType};
use sha2::{Digest, Sha256};
use std::fs::File as StdFile;
use std::io::Read as _;
use std::path::Path;
use std::time::Instant;
use thiserror::Error;
use uuid::Uuid;

const CHILD_ATTRIBUTES: &str = "standard::name,standard::type,standard::size";

pub async fn mounted_device_state(
    mount_id: &str,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    let mount_id = mount_id.to_owned();
    tokio::task::spawn_blocking(move || mounted_device_state_blocking(&mount_id))
        .await
        .map_err(MountedMtpError::Task)?
}

pub(super) fn mounted_device_state_blocking(
    mount_id: &str,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    let root = mounted_root(mount_id)?;
    let mut storages = Vec::new();
    for (index, storage) in storage_roots(&root)?.into_iter().enumerate() {
        let info = storage.query_filesystem_info(
            "filesystem::size,filesystem::free,filesystem::readonly",
            gio::Cancellable::NONE,
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
            label: storage_label(&storage, index),
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
    let components = relative
        .as_path()
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut parent = storage.clone();
    for (index, component) in components.iter().enumerate() {
        let (child, file_type, size) = match child_metadata(&parent, component)? {
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

fn child_metadata(parent: &File, expected: &str) -> Result<ChildMetadata, MountedMtpError> {
    let enumerator = parent.enumerate_children(
        CHILD_ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    )?;
    let mut found = None;
    while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
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
    let mut manifests = Vec::new();
    for garmin in garmin_directories(&root)? {
        if let Some(manifest) = child_named(&garmin, "GarminDevice.xml", FileType::Regular)? {
            manifests.push(manifest);
        }
    }
    let manifest = match manifests.as_slice() {
        [] => return Err(MountedMtpError::ManifestMissing),
        [manifest] => manifest,
        _ => return Err(MountedMtpError::AmbiguousManifest),
    };
    let xml = read_bounded(manifest)?;
    parse_manifest(
        &xml,
        TransportKind::MountedMtp,
        format!("mounted-mtp:{mount_id}"),
    )
    .map_err(Into::into)
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
        backup_mounted_mtp_object_blocking(
            &mount_id,
            &storage_id,
            &path,
            expected_size,
            destination,
            &progress,
        )
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
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        let root = mounted_root(&mount_id)?;
        let storage = storage_for_id(&root, &storage_id)?;
        inspect_mounted_path(&storage, &path)
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
    let source = mounted_regular_file(mount_id, storage_id, path, expected_size)?;
    let (destination_path, reserved) = destination.into_parts();
    if reserved.metadata()?.len() != 0 {
        return Err(MountedMtpError::BackupSinkVerification(path.clone()));
    }
    drop(reserved);
    std::fs::remove_file(&destination_path)?;
    let destination = File::for_path(&destination_path);
    let cancellable = gio::Cancellable::new();
    if progress.reporter.is_cancelled() {
        cancellable.cancel();
    }
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
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let expected_sha256 = expected_sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        let target = mounted_regular_file(&mount_id, &storage_id, &path, expected_size)?;
        let (actual_size, actual_sha256) = hash_mounted_file(&target)?;
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
        target.delete(gio::Cancellable::NONE)?;
        if target.query_exists(gio::Cancellable::NONE) {
            return Err(MountedMtpError::RemovalObjectNotRemoved(path));
        }
        Ok(())
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Delete one exact mounted-MTP object after metadata-size validation only.
/// # Errors
/// Unsafe paths, changed sizes, deletion failures, and objects that remain
/// visible after deletion return [`MountedMtpError`].
pub async fn delete_unverified_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        let target = mounted_regular_file(&mount_id, &storage_id, &path, expected_size)?;
        target.delete(gio::Cancellable::NONE)?;
        if target.query_exists(gio::Cancellable::NONE) {
            return Err(MountedMtpError::RemovalObjectNotRemoved(path));
        }
        Ok(())
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Verify one mounted-MTP object against its expected size and SHA-256.
/// # Errors
/// [`MountedMtpError`] if the object changed or cannot be read.
pub async fn verify_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let expected_sha256 = expected_sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        let target = mounted_regular_file(&mount_id, &storage_id, &path, expected_size)?;
        let (actual_size, actual_sha256) = hash_mounted_file(&target)?;
        if actual_size != expected_size {
            return Err(MountedMtpError::RemovalObjectSize {
                path: path.clone(),
                expected: expected_size,
                actual: actual_size,
            });
        }
        if actual_sha256 != expected_sha256 {
            return Err(MountedMtpError::RemovalObjectChecksum(path));
        }
        Ok(())
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Restore one exact mounted-MTP object from a verified local backup.
/// # Errors
/// [`MountedMtpError`] if the backup is invalid, the destination
/// already exists, upload fails, or restored verification fails.
pub async fn restore_mounted_mtp_object(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    backup: &Path,
    expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    let mount_id = mount_id.to_owned();
    let storage_id = storage_id.to_owned();
    let path = path.clone();
    let backup = backup.to_owned();
    let expected_sha256 = expected_sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        restore_mounted_mtp_object_blocking(
            &mount_id,
            &storage_id,
            &path,
            expected_size,
            &backup,
            &expected_sha256,
        )
    })
    .await
    .map_err(MountedMtpError::Task)?
}

/// Upload one verified local file to an absent mounted-MTP destination.
///
/// Checks the source before copying, then reads and verifies the device object.
/// A failed upload is removed when possible.
/// # Errors
/// Source change, existing destination, copy failure, or failed readback.
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
        upload_mounted_mtp_object_blocking(
            &mount_id,
            &storage_id,
            &path,
            &source,
            expected_size,
            &expected_sha256,
            &progress,
        )
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
    let root = mounted_root(mount_id)?;
    let storage = storage_for_id(&root, storage_id)?;
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
    let (source_size, source_sha256) = hash_local_file(source)?;
    if source_size != expected_size || source_sha256 != expected_sha256 {
        return Err(MountedMtpError::UploadSourceVerification(source.to_owned()));
    }
    let (parent, name) = mounted_parent(storage, path)?;
    let destination = parent.child(name);
    if destination.query_exists(gio::Cancellable::NONE) {
        return Err(MountedMtpError::UploadDestinationExists(path.clone()));
    }

    let cancellable = gio::Cancellable::new();
    if progress.reporter.is_cancelled() {
        cancellable.cancel();
    }
    let callback_cancellable = cancellable.clone();
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
        }
    };
    if let Err(upload) = File::for_path(source).copy(
        &destination,
        FileCopyFlags::NONE,
        Some(&cancellable),
        Some(&mut copy_progress),
    ) {
        let cleanup = discard_partial_probe(&destination);
        if progress.reporter.is_cancelled() && cleanup.is_ok() {
            return Err(MountedMtpError::Cancelled);
        }
        return match cleanup {
            Ok(()) => Err(MountedMtpError::Upload(upload)),
            Err(cleanup) => Err(MountedMtpError::UploadCleanup { upload, cleanup }),
        };
    }

    let verification = (|| {
        let info = destination.query_info(
            "standard::type,standard::size",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )?;
        if info.file_type() != FileType::Regular {
            return Err(MountedMtpError::UploadVerification(path.clone()));
        }
        let reported_size = u64::try_from(info.size())
            .map_err(|_| MountedMtpError::NegativeObjectSize(info.size()))?;
        if reported_size != expected_size {
            return Err(MountedMtpError::UploadVerification(path.clone()));
        }
        let (actual_size, actual_sha256) = hash_mounted_file(&destination)?;
        if actual_size != expected_size || actual_sha256 != expected_sha256 {
            return Err(MountedMtpError::UploadVerification(path.clone()));
        }
        Ok(())
    })();
    if let Err(error) = verification {
        return Err(cleanup_failed_upload(&destination, error));
    }
    Ok(())
}

fn cleanup_failed_upload(destination: &File, operation: MountedMtpError) -> MountedMtpError {
    if !destination.query_exists(gio::Cancellable::NONE) {
        return operation;
    }
    match destination.delete(gio::Cancellable::NONE) {
        Ok(()) => operation,
        Err(cleanup) => MountedMtpError::UploadVerificationCleanup {
            operation: operation.to_string(),
            cleanup,
        },
    }
}

fn restore_mounted_mtp_object_blocking(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
    backup: &Path,
    expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    let (backup_size, backup_sha256) = hash_local_file(backup)?;
    if backup_size != expected_size || backup_sha256 != expected_sha256 {
        return Err(MountedMtpError::BackupVerification(backup.to_owned()));
    }
    let root = mounted_root(mount_id)?;
    let storage = storage_for_id(&root, storage_id)?;
    let (parent, name) = mounted_parent(&storage, path)?;
    let destination = parent.child(name);
    if destination.query_exists(gio::Cancellable::NONE) {
        return Err(MountedMtpError::RemovalRestoreExists(path.clone()));
    }
    if let Err(upload) = File::for_path(backup).copy(
        &destination,
        FileCopyFlags::NONE,
        gio::Cancellable::NONE,
        None,
    ) {
        return Err(cleanup_failed_restore(
            &destination,
            MountedMtpError::Gio(upload),
        ));
    }
    let verification = (|| {
        let restored = mounted_regular_file(mount_id, storage_id, path, expected_size)?;
        let (actual, restored_sha256) = hash_mounted_file(&restored)?;
        if actual != expected_size {
            return Err(MountedMtpError::RemovalObjectSize {
                path: path.clone(),
                expected: expected_size,
                actual,
            });
        }
        if restored_sha256 != expected_sha256 {
            return Err(MountedMtpError::RemovalRestoreChecksum(path.clone()));
        }
        Ok(())
    })();
    if let Err(error) = verification {
        return Err(cleanup_failed_restore(&destination, error));
    }
    Ok(())
}

fn cleanup_failed_restore(destination: &File, operation: MountedMtpError) -> MountedMtpError {
    if !destination.query_exists(gio::Cancellable::NONE) {
        return operation;
    }
    match destination.delete(gio::Cancellable::NONE) {
        Ok(()) => operation,
        Err(cleanup) => MountedMtpError::RemovalRestoreCleanup {
            operation: operation.to_string(),
            cleanup,
        },
    }
}

fn mounted_regular_file(
    mount_id: &str,
    storage_id: &str,
    path: &SafeRelativePath,
    expected_size: u64,
) -> Result<File, MountedMtpError> {
    let root = mounted_root(mount_id)?;
    let storage = storage_for_id(&root, storage_id)?;
    let (parent, name) = mounted_parent(&storage, path)?;
    match child_metadata(&parent, &name)? {
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

fn storage_for_id(root: &File, storage_id: &str) -> Result<File, MountedMtpError> {
    garmin_storages(root)?
        .into_iter()
        .map(|(storage, _)| storage)
        .find(|storage| mount_id_for_uri(&storage.uri()) == storage_id)
        .ok_or_else(|| MountedMtpError::StorageNotFound(storage_id.to_owned()))
}

fn mounted_parent(
    storage: &File,
    path: &SafeRelativePath,
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
        parent = match child_metadata(&parent, &component)? {
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

fn hash_local_file(path: &Path) -> Result<(u64, String), MountedMtpError> {
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(MountedMtpError::BackupVerification(path.to_owned()));
    }
    let mut file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(MountedMtpError::BackupVerification(path.to_owned()));
    }
    hash_local_reader(&mut file)
}

fn hash_local_reader(file: &mut StdFile) -> Result<(u64, String), MountedMtpError> {
    let mut size = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(count).map_err(|_| MountedMtpError::SizeOverflow)?)
            .ok_or(MountedMtpError::SizeOverflow)?;
        hash.update(&buffer[..count]);
    }
    Ok((size, hex::encode(hash.finalize())))
}

fn hash_mounted_file(file: &File) -> Result<(u64, String), MountedMtpError> {
    let stream = file.read(gio::Cancellable::NONE)?;
    let mut size = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = stream.read(&mut buffer, gio::Cancellable::NONE)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(count).map_err(|_| MountedMtpError::SizeOverflow)?)
            .ok_or(MountedMtpError::SizeOverflow)?;
        hash.update(&buffer[..count]);
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
                return match discard_partial_probe(&destination) {
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
        let cleanup = discard_partial_probe(destination);
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

fn discard_partial_probe(destination: &File) -> Result<(), gio::glib::Error> {
    match destination.delete(gio::Cancellable::NONE) {
        Ok(()) => Ok(()),
        Err(error) if error.kind::<gio::IOErrorEnum>() == Some(gio::IOErrorEnum::NotFound) => {
            Ok(())
        }
        Err(error) => Err(error),
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
    progress.started(
        OperationStage::Delete,
        "Removing disposable device object",
        Some(1),
    );
    let deletion_started = Instant::now();
    if let Err(error) = destination.delete(gio::Cancellable::NONE) {
        progress.failed(OperationStage::Delete, "Disposable object deletion failed");
        return Err(error.into());
    }
    let removed = !destination.query_exists(gio::Cancellable::NONE);
    let deletion_elapsed = deletion_started.elapsed();
    if !removed {
        progress.failed(OperationStage::Delete, "Disposable object still exists");
        return Err(MountedMtpError::ProbeNotRemoved);
    }
    progress.completed(
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
    storage_roots(root)?
        .into_iter()
        .filter_map(
            |storage| match child_named(&storage, "Garmin", FileType::Directory) {
                Ok(Some(garmin)) => Some(Ok((storage, garmin))),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

fn storage_roots(root: &File) -> Result<Vec<File>, MountedMtpError> {
    if child_named(root, "Garmin", FileType::Directory)?.is_some() {
        return Ok(vec![root.clone()]);
    }
    let children = child_directories(root)?;
    Ok(if children.is_empty() {
        vec![root.clone()]
    } else {
        children
    })
}

fn storage_label(storage: &File, index: usize) -> String {
    storage
        .query_info(
            "standard::display-name",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
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

fn child_directories(parent: &File) -> Result<Vec<File>, MountedMtpError> {
    let enumerator = parent.enumerate_children(
        CHILD_ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    )?;
    let mut directories = Vec::new();
    while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
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
    let enumerator = parent.enumerate_children(
        CHILD_ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    )?;
    while let Some(info) = enumerator.next_file(gio::Cancellable::NONE)? {
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
    #[error("restored mounted MTP object {0} failed checksum verification")]
    RemovalRestoreChecksum(SafeRelativePath),
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
    #[error("uploaded mounted MTP object {0} failed read-back verification")]
    UploadVerification(SafeRelativePath),
    #[error("upload verification failed ({operation}); invalid-object cleanup failed ({cleanup})")]
    UploadVerificationCleanup {
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
        MountedMtpError, MountedMtpUploadProgress, mount_id_for_uri, storage_roots,
        upload_probe_file, upload_to_storage_blocking, verify_probe_file,
    };
    use crate::SafeRelativePath;
    use garmin_progress::{OperationStage, ProgressReporter, ProgressState};
    use gio::File;
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
    fn uploads_and_reads_back_a_verified_object() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = fixture.path().join("storage");
        let source = fixture.path().join("source.img");
        std::fs::create_dir_all(storage.join("Garmin")).unwrap();
        std::fs::write(&source, b"verified map payload").unwrap();
        let path = SafeRelativePath::parse("Garmin/map.img").unwrap();
        let sha256 = hex::encode(Sha256::digest(b"verified map payload"));

        upload_to_storage_blocking(
            &File::for_path(&storage),
            &path,
            &source,
            20,
            &sha256,
            &MountedMtpUploadProgress {
                reporter: ProgressReporter::default(),
                completed_before: 0,
                total: 20,
            },
        )
        .unwrap();

        assert_eq!(
            std::fs::read(storage.join("Garmin/map.img")).unwrap(),
            b"verified map payload"
        );
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
