//! Bounded device contents and explicit explorer operations.

use std::{
    collections::HashSet,
    io::{Read as _, Write},
    path::Path,
};

use camino::{Utf8Path, Utf8PathBuf};
use garmin_progress::ProgressReporter;
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use tokio::io::AsyncReadExt as _;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
    DevicePathStatus, PathSafetyError, SafeRelativePath,
    storage::{DeviceIoError, DeviceRead, DeviceWrite},
};

pub const MAX_BROWSER_TRANSFER_BYTES: u64 = 512 * 1024 * 1024;
const TOOLKIT_ROOT: &str = "GARMIN-TOOLKIT";

/// One device file-system snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCatalog {
    pub storages: Vec<DeviceCatalogStorage>,
}

/// One separately mounted device storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCatalogStorage {
    pub id: String,
    pub label: String,
    pub entries: Vec<DeviceCatalogEntry>,
}

/// One device-relative entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCatalogEntry {
    pub path: Utf8PathBuf,
    pub kind: DeviceCatalogEntryKind,
    pub size: Option<u64>,
}

/// File-system entry kind retained by the browser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceCatalogEntryKind {
    Directory,
    File,
}

/// One catalog entry selected for an explicit explorer operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceBrowserTarget {
    pub storage_id: String,
    pub path: Utf8PathBuf,
    pub kind: DeviceCatalogEntryKind,
}

/// One validated browser upload backed by a native host file.
pub struct BrowserFileUpload<'a> {
    pub storage_id: &'a str,
    pub directory: &'a Utf8Path,
    pub file_name: &'a str,
    pub source: &'a Path,
    pub size: u64,
}

/// A bounded device download retained in a temporary file.
///
/// Keeping the payload outside the process heap lets frontends stream it to their final
/// destination without cloning a potentially large transfer.
#[derive(Debug)]
pub struct PreparedDeviceBrowserDownload {
    pub file_name: String,
    pub size: u64,
    contents: tempfile::TempPath,
}

impl PreparedDeviceBrowserDownload {
    /// Adopt an owning temporary file as a prepared download.
    /// # Errors
    /// The temporary path must still name a regular file.
    pub fn from_temporary(
        file_name: String,
        contents: tempfile::TempPath,
    ) -> Result<Self, std::io::Error> {
        let metadata =
            std::fs::symlink_metadata(<tempfile::TempPath as AsRef<Path>>::as_ref(&contents))?;
        if !metadata.file_type().is_file() {
            return Err(std::io::Error::other(
                "prepared download is not a regular file",
            ));
        }
        Ok(Self {
            file_name,
            size: metadata.len(),
            contents,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.contents.as_ref()
    }
}

/// Failure from a bounded explorer operation.
#[derive(Debug, Error)]
pub enum DeviceBrowserOperationError {
    #[error("unknown device storage: {0}")]
    Storage(String),
    #[error("the selected device item changed; refresh the file browser and try again: {0}")]
    StaleTarget(String),
    #[error("the device storage root cannot be changed or removed")]
    RootMutation,
    #[error("invalid device item selection: {0}")]
    InvalidSelection(String),
    #[error("GARMIN-TOOLKIT is managed by Garmin Toolkit and is browse/download-only")]
    ToolkitManaged,
    #[error("invalid device file name: {0}")]
    FileName(String),
    #[error("the device transfer exceeds the 512 MiB browser limit")]
    TransferLimit,
    #[error("device file is missing its size: {0}")]
    MissingSize(String),
    #[error("could not restore {path} after replacement failed ({transfer}): {restore}")]
    ReplacementRecovery {
        path: String,
        transfer: String,
        restore: String,
    },
    #[error(transparent)]
    Path(#[from] PathSafetyError),
    #[error(transparent)]
    Device(#[from] DeviceIoError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}

/// Prepare one validated file, or one directory as a ZIP archive, in bounded temporary storage.
/// # Errors
/// Unknown, stale, unsafe, oversized, cancelled, or unreadable targets are rejected.
pub async fn prepare_browser_download<R>(
    device: &R,
    catalog: &DeviceCatalog,
    target: &DeviceBrowserTarget,
    progress: &ProgressReporter,
) -> Result<PreparedDeviceBrowserDownload, DeviceBrowserOperationError>
where
    R: DeviceRead + ?Sized,
{
    let storage = validate_target(catalog, target)?;
    if target.kind == DeviceCatalogEntryKind::File {
        let entry = exact_entry(storage, &target.path, target.kind)?;
        let size = entry
            .size
            .ok_or_else(|| DeviceBrowserOperationError::MissingSize(entry.path.to_string()))?;
        let contents = backup_entry(device, &target.storage_id, entry, 0, size, progress).await?;
        return Ok(PreparedDeviceBrowserDownload {
            file_name: entry_name(&target.path).to_owned(),
            size,
            contents,
        });
    }

    let prefix = descendant_prefix(&target.path);
    let mut descendants = storage
        .entries
        .iter()
        .filter(|entry| target.path.as_str().is_empty() || entry.path.as_str().starts_with(&prefix))
        .collect::<Vec<_>>();
    descendants.sort_by(|left, right| left.path.cmp(&right.path));
    let total = descendants
        .iter()
        .filter(|entry| entry.kind == DeviceCatalogEntryKind::File)
        .try_fold(0_u64, |total, entry| {
            total
                .checked_add(entry.size.ok_or_else(|| {
                    DeviceBrowserOperationError::MissingSize(entry.path.to_string())
                })?)
                .ok_or(DeviceBrowserOperationError::TransferLimit)
        })?;
    if total > MAX_BROWSER_TRANSFER_BYTES {
        return Err(DeviceBrowserOperationError::TransferLimit);
    }

    let archive = tempfile::NamedTempFile::new()?;
    let archive_file = archive.reopen()?;
    let mut writer = ZipWriter::new(archive_file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    let directory_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o755);
    let archive_root = (!target.path.as_str().is_empty()).then(|| entry_name(&target.path));
    if let Some(root) = archive_root {
        writer.add_directory(format!("{root}/"), directory_options)?;
    }
    for entry in descendants
        .iter()
        .filter(|entry| entry.kind == DeviceCatalogEntryKind::Directory)
    {
        writer.add_directory(
            format!("{}/", archive_path(&entry.path, &target.path, archive_root)),
            directory_options,
        )?;
    }
    let mut completed = 0_u64;
    for entry in descendants
        .into_iter()
        .filter(|entry| entry.kind == DeviceCatalogEntryKind::File)
    {
        if progress.is_cancelled() {
            return Err(DeviceBrowserOperationError::Device(
                DeviceIoError::Cancelled,
            ));
        }
        let size = entry
            .size
            .ok_or_else(|| DeviceBrowserOperationError::MissingSize(entry.path.to_string()))?;
        let contents = backup_entry(
            device,
            &target.storage_id,
            entry,
            completed,
            total,
            progress,
        )
        .await?;
        writer.start_file(
            archive_path(&entry.path, &target.path, archive_root),
            options,
        )?;
        copy_local_file(contents.as_ref(), &mut writer, progress)?;
        completed = completed.saturating_add(size);
    }
    let archive_file = writer.finish()?;
    archive_file.sync_all()?;
    let size = archive_file.metadata()?.len();
    if size > MAX_BROWSER_TRANSFER_BYTES {
        return Err(DeviceBrowserOperationError::TransferLimit);
    }
    drop(archive_file);
    Ok(PreparedDeviceBrowserDownload {
        file_name: archive_name(&target.path, &storage.label),
        size,
        contents: archive.into_temp_path(),
    })
}

/// Upload and verify one file from a bounded host path without loading it into memory.
/// # Errors
/// Unsafe, protected, duplicate, oversized, changed, cancelled, or unverifiable writes are
/// rejected.
pub async fn upload_browser_file_from_path<W>(
    device: &W,
    catalog: &DeviceCatalog,
    upload: BrowserFileUpload<'_>,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    let BrowserFileUpload {
        storage_id,
        directory,
        file_name,
        source,
        size,
    } = upload;
    if size > MAX_BROWSER_TRANSFER_BYTES {
        return Err(DeviceBrowserOperationError::TransferLimit);
    }
    let metadata = tokio::fs::symlink_metadata(source).await?;
    if !metadata.file_type().is_file() || metadata.len() != size {
        return Err(DeviceBrowserOperationError::Device(
            DeviceIoError::Verification(source.display().to_string()),
        ));
    }
    let storage = validate_directory(catalog, storage_id, directory)?;
    ensure_mutable(directory)?;
    validate_name(file_name)?;
    let destination = joined_path(directory, file_name);
    ensure_available(storage, &destination)?;
    let destination = SafeRelativePath::parse(destination)?;
    let sha256 = hash_local_file(source, size, progress).await?;
    device
        .upload(
            storage_id,
            &destination,
            source,
            size,
            &sha256,
            crate::MountedMtpUploadProgress {
                reporter: progress.clone(),
                completed_before: 0,
                total: size,
            },
        )
        .await?;
    if device
        .inspect_with_progress(storage_id, &destination, progress)
        .await?
        != (DevicePathStatus::RegularFile { size })
    {
        return Err(DeviceBrowserOperationError::Device(
            DeviceIoError::Verification(destination.to_string()),
        ));
    }
    Ok(())
}

/// Create and verify one direct child directory.
/// # Errors
/// Unsafe, protected, duplicate, or unverifiable writes are rejected.
pub async fn create_browser_directory<W>(
    device: &W,
    catalog: &DeviceCatalog,
    storage_id: &str,
    parent: impl AsRef<Utf8Path>,
    name: &str,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    let parent = parent.as_ref();
    create_browser_directory_with_progress(
        device,
        catalog,
        storage_id,
        parent,
        name,
        &ProgressReporter::default(),
    )
    .await
}

/// Create and verify one direct child directory while honoring cancellation.
/// # Errors
/// Unsafe, protected, duplicate, cancelled, or unverifiable writes are rejected.
pub async fn create_browser_directory_with_progress<W>(
    device: &W,
    catalog: &DeviceCatalog,
    storage_id: &str,
    parent: impl AsRef<Utf8Path>,
    name: &str,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    let parent = parent.as_ref();
    let storage = validate_directory(catalog, storage_id, parent)?;
    ensure_mutable(parent)?;
    validate_name(name)?;
    let path = joined_path(parent, name);
    ensure_available(storage, &path)?;
    let path = SafeRelativePath::parse(path)?;
    device
        .ensure_directory_with_progress(storage_id, &path, progress)
        .await?;
    if device
        .inspect_with_progress(storage_id, &path, progress)
        .await?
        != DevicePathStatus::Directory
    {
        return Err(DeviceBrowserOperationError::Device(
            DeviceIoError::Verification(path.to_string()),
        ));
    }
    Ok(())
}

/// Recursively remove one validated file or directory and verify each deletion.
/// # Errors
/// Storage roots, toolkit-managed paths, stale entries, or failed deletions are rejected.
pub async fn remove_browser_target<W>(
    device: &W,
    catalog: &DeviceCatalog,
    target: &DeviceBrowserTarget,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    remove_browser_target_with_progress(device, catalog, target, &ProgressReporter::default()).await
}

/// Recursively remove one validated target while honoring cancellation.
/// # Errors
/// Storage roots, managed paths, stale entries, cancellation, or failed deletions are rejected.
pub async fn remove_browser_target_with_progress<W>(
    device: &W,
    catalog: &DeviceCatalog,
    target: &DeviceBrowserTarget,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    let storage = validate_target(catalog, target)?;
    if target.path.as_str().is_empty() {
        return Err(DeviceBrowserOperationError::RootMutation);
    }
    ensure_mutable(&target.path)?;
    if target.kind == DeviceCatalogEntryKind::File {
        let entry = exact_entry(storage, &target.path, target.kind)?;
        remove_file(device, &target.storage_id, entry, progress).await?;
        return Ok(());
    }

    let prefix = descendant_prefix(&target.path);
    let mut descendants = storage
        .entries
        .iter()
        .filter(|entry| entry.path.as_str().starts_with(&prefix))
        .collect::<Vec<_>>();
    descendants.sort_by_key(|entry| std::cmp::Reverse(path_depth(&entry.path)));
    for entry in descendants
        .iter()
        .copied()
        .filter(|entry| entry.kind == DeviceCatalogEntryKind::File)
    {
        remove_file(device, &target.storage_id, entry, progress).await?;
    }
    for entry in descendants
        .into_iter()
        .filter(|entry| entry.kind == DeviceCatalogEntryKind::Directory)
    {
        let path = SafeRelativePath::parse(&entry.path)?;
        device
            .remove_empty_directory_with_progress(&target.storage_id, &path, progress)
            .await?;
    }
    let path = SafeRelativePath::parse(&target.path)?;
    device
        .remove_empty_directory_with_progress(&target.storage_id, &path, progress)
        .await?;
    Ok(())
}

/// Remove a reviewed selection using one catalog snapshot.
/// # Errors
/// Empty, overlapping, protected, stale, or unsafe selections are rejected before deletion.
/// An I/O failure can leave earlier disjoint items removed; refresh the catalog before retrying.
pub async fn remove_browser_targets_with_progress<W>(
    device: &W,
    catalog: &DeviceCatalog,
    targets: &[DeviceBrowserTarget],
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    validate_disjoint_targets(catalog, targets)?;
    for target in targets {
        if progress.is_cancelled() {
            return Err(DeviceBrowserOperationError::Device(
                DeviceIoError::Cancelled,
            ));
        }
        remove_browser_target_with_progress(device, catalog, target, progress).await?;
    }
    Ok(())
}

/// Copy or move selected device items into an existing directory.
///
/// Every destination is checked before writing. A move removes its sources only after all copies
/// have been uploaded and verified. A failed copy can leave new destination items to review.
/// # Errors
/// Unsafe, protected, overlapping, stale, duplicate, oversized, or failed transfers are rejected.
pub async fn transfer_browser_targets_with_progress<W>(
    device: &W,
    catalog: &DeviceCatalog,
    targets: &[DeviceBrowserTarget],
    destination_storage_id: &str,
    destination: &Utf8Path,
    copy: bool,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    transfer_browser_targets_named_with_progress(
        device,
        catalog,
        targets,
        BrowserTransferDestination {
            storage_id: destination_storage_id,
            directory: destination,
            root_name: None,
            copy,
        },
        progress,
    )
    .await
}

/// Rename one file or folder by copying it to the new name, verifying the copy, then removing the original.
/// # Errors
/// Rejects invalid names, protected or stale targets, duplicates, and transfers over the browser limit.
pub async fn rename_browser_target_with_progress<W>(
    device: &W,
    catalog: &DeviceCatalog,
    target: &DeviceBrowserTarget,
    name: &str,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    validate_name(name)?;
    if name.eq_ignore_ascii_case(entry_name(&target.path)) {
        return Err(DeviceBrowserOperationError::InvalidSelection(
            target.path.to_string(),
        ));
    }
    let parent = target.path.parent().unwrap_or(Utf8Path::new(""));
    transfer_browser_targets_named_with_progress(
        device,
        catalog,
        std::slice::from_ref(target),
        BrowserTransferDestination {
            storage_id: &target.storage_id,
            directory: parent,
            root_name: Some(name),
            copy: false,
        },
        progress,
    )
    .await
}

struct BrowserTransferDestination<'a> {
    storage_id: &'a str,
    directory: &'a Utf8Path,
    root_name: Option<&'a str>,
    copy: bool,
}

async fn transfer_browser_targets_named_with_progress<W>(
    device: &W,
    catalog: &DeviceCatalog,
    targets: &[DeviceBrowserTarget],
    destination: BrowserTransferDestination<'_>,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    let (mut planned, total) = plan_browser_transfer(
        catalog,
        targets,
        destination.storage_id,
        destination.directory,
        destination.root_name,
    )?;
    planned.sort_by_key(|(_, entry, path)| {
        (entry.kind == DeviceCatalogEntryKind::File, path_depth(path))
    });
    let transfer_total = total.saturating_mul(2);
    let mut completed = 0_u64;
    for (storage_id, entry, destination_path) in planned {
        let path = SafeRelativePath::parse(&destination_path)?;
        if entry.kind == DeviceCatalogEntryKind::Directory {
            device
                .ensure_directory_with_progress(destination.storage_id, &path, progress)
                .await?;
            if device
                .inspect_with_progress(destination.storage_id, &path, progress)
                .await?
                != DevicePathStatus::Directory
            {
                return Err(DeviceBrowserOperationError::Device(
                    DeviceIoError::Verification(path.to_string()),
                ));
            }
            continue;
        }
        let size = entry
            .size
            .ok_or_else(|| DeviceBrowserOperationError::MissingSize(entry.path.to_string()))?;
        let contents = backup_entry(
            device,
            &storage_id,
            &entry,
            completed,
            transfer_total,
            progress,
        )
        .await?;
        let digest = hash_local_file(contents.as_ref(), size, progress).await?;
        device
            .upload(
                destination.storage_id,
                &path,
                contents.as_ref(),
                size,
                &digest,
                crate::MountedMtpUploadProgress {
                    reporter: progress.clone(),
                    completed_before: completed.saturating_add(size),
                    total: transfer_total,
                },
            )
            .await?;
        if device
            .inspect_with_progress(destination.storage_id, &path, progress)
            .await?
            != (DevicePathStatus::RegularFile { size })
        {
            return Err(DeviceBrowserOperationError::Device(
                DeviceIoError::Verification(path.to_string()),
            ));
        }
        completed = completed.saturating_add(size.saturating_mul(2));
    }
    if !destination.copy {
        remove_browser_targets_with_progress(device, catalog, targets, progress).await?;
    }
    Ok(())
}

struct BrowserReplacement<'a> {
    source: &'a DeviceCatalogEntry,
    existing: &'a DeviceCatalogEntry,
    path: SafeRelativePath,
    source_size: u64,
    existing_size: u64,
}

fn validate_browser_replacement<'a>(
    catalog: &'a DeviceCatalog,
    target: &DeviceBrowserTarget,
    destination_storage_id: &str,
    destination: &Utf8Path,
) -> Result<BrowserReplacement<'a>, DeviceBrowserOperationError> {
    if target.kind != DeviceCatalogEntryKind::File {
        return Err(DeviceBrowserOperationError::InvalidSelection(
            target.path.to_string(),
        ));
    }
    let source_storage = validate_target(catalog, target)?;
    let source = exact_entry(source_storage, &target.path, DeviceCatalogEntryKind::File)?;
    let destination_storage = validate_directory(catalog, destination_storage_id, destination)?;
    let destination_path = joined_path(destination, entry_name(&target.path));
    ensure_mutable(&target.path)?;
    ensure_mutable(destination)?;
    ensure_mutable(&destination_path)?;
    if target.storage_id == destination_storage_id
        && target
            .path
            .as_str()
            .eq_ignore_ascii_case(destination_path.as_str())
    {
        return Err(DeviceBrowserOperationError::InvalidSelection(
            target.path.to_string(),
        ));
    }
    let existing = destination_storage
        .entries
        .iter()
        .find(|entry| {
            entry
                .path
                .as_str()
                .eq_ignore_ascii_case(destination_path.as_str())
        })
        .filter(|entry| entry.kind == DeviceCatalogEntryKind::File)
        .ok_or_else(|| DeviceBrowserOperationError::StaleTarget(destination_path.to_string()))?;
    let source_size = source
        .size
        .ok_or_else(|| DeviceBrowserOperationError::MissingSize(source.path.to_string()))?;
    let existing_size = existing
        .size
        .ok_or_else(|| DeviceBrowserOperationError::MissingSize(existing.path.to_string()))?;
    if source_size > MAX_BROWSER_TRANSFER_BYTES || existing_size > MAX_BROWSER_TRANSFER_BYTES {
        return Err(DeviceBrowserOperationError::TransferLimit);
    }
    Ok(BrowserReplacement {
        source,
        existing,
        path: SafeRelativePath::parse(&existing.path)?,
        source_size,
        existing_size,
    })
}

/// Replace one existing device file after an explicit overwrite confirmation.
/// The old file is backed up before deletion and restored if the new upload fails.
/// # Errors
/// Rejects stale, unsafe, protected, or non-file targets; reports a recovery error if rollback fails.
pub async fn replace_browser_file_with_progress<W>(
    device: &W,
    catalog: &DeviceCatalog,
    targets: &[DeviceBrowserTarget],
    destination_storage_id: &str,
    destination: &Utf8Path,
    copy: bool,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    let [target] = targets else {
        return Err(DeviceBrowserOperationError::InvalidSelection(
            "replacement requires one file".to_owned(),
        ));
    };
    let BrowserReplacement {
        source,
        existing,
        path,
        source_size,
        existing_size,
    } = validate_browser_replacement(catalog, target, destination_storage_id, destination)?;
    let source_backup =
        backup_entry(device, &target.storage_id, source, 0, source_size, progress).await?;
    let source_digest = hash_local_file(source_backup.as_ref(), source_size, progress).await?;
    let existing_backup = backup_entry(
        device,
        destination_storage_id,
        existing,
        0,
        existing_size,
        progress,
    )
    .await?;
    let existing_digest =
        hash_local_file(existing_backup.as_ref(), existing_size, progress).await?;
    if device.inspect(destination_storage_id, &path).await?
        != (DevicePathStatus::RegularFile {
            size: existing_size,
        })
    {
        return Err(DeviceBrowserOperationError::StaleTarget(
            existing.path.to_string(),
        ));
    }
    if progress.is_cancelled() {
        return Err(DeviceBrowserOperationError::Device(
            DeviceIoError::Cancelled,
        ));
    }
    delete_replaced_file(
        device,
        destination_storage_id,
        &path,
        existing_size,
        existing_backup.as_ref(),
        &existing_digest,
    )
    .await?;
    let replacement = async {
        device
            .upload(
                destination_storage_id,
                &path,
                source_backup.as_ref(),
                source_size,
                &source_digest,
                crate::MountedMtpUploadProgress {
                    reporter: progress.clone(),
                    completed_before: source_size,
                    total: source_size.saturating_mul(2),
                },
            )
            .await?;
        if device.inspect(destination_storage_id, &path).await?
            != (DevicePathStatus::RegularFile { size: source_size })
        {
            return Err(DeviceBrowserOperationError::Device(
                DeviceIoError::Verification(path.to_string()),
            ));
        }
        Ok::<(), DeviceBrowserOperationError>(())
    }
    .await;
    if let Err(error) = replacement {
        let recovery = restore_browser_file(
            device,
            destination_storage_id,
            &path,
            existing_size,
            existing_backup.as_ref(),
            &existing_digest,
        )
        .await;
        return match recovery {
            Ok(()) => Err(error),
            Err(restore) => Err(DeviceBrowserOperationError::ReplacementRecovery {
                path: path.to_string(),
                transfer: error.to_string(),
                restore: restore.to_string(),
            }),
        };
    }
    if !copy {
        remove_browser_target_with_progress(device, catalog, target, progress).await?;
    }
    Ok(())
}

async fn delete_replaced_file<W>(
    device: &W,
    storage_id: &str,
    path: &SafeRelativePath,
    size: u64,
    backup: &Path,
    digest: &str,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    if let Err(error) = device.delete_size_checked(storage_id, path, size).await {
        if matches!(
            device.inspect(storage_id, path).await,
            Ok(DevicePathStatus::RegularFile { size: actual }) if actual == size
        ) {
            return Err(error.into());
        }
        return match restore_browser_file(device, storage_id, path, size, backup, digest).await {
            Ok(()) => Err(error.into()),
            Err(restore) => Err(DeviceBrowserOperationError::ReplacementRecovery {
                path: path.to_string(),
                transfer: error.to_string(),
                restore: restore.to_string(),
            }),
        };
    }
    Ok(())
}

async fn restore_browser_file<W>(
    device: &W,
    storage_id: &str,
    path: &SafeRelativePath,
    size: u64,
    backup: &Path,
    digest: &str,
) -> Result<(), DeviceIoError>
where
    W: DeviceWrite + ?Sized,
{
    match device.inspect(storage_id, path).await? {
        DevicePathStatus::Missing => {}
        DevicePathStatus::RegularFile { size } => {
            device.delete_size_checked(storage_id, path, size).await?;
        }
        DevicePathStatus::Directory | DevicePathStatus::Other | DevicePathStatus::Ambiguous => {
            return Err(DeviceIoError::Verification(path.to_string()));
        }
    }
    device
        .restore(storage_id, path, size, backup, digest)
        .await?;
    if device.inspect(storage_id, path).await? != (DevicePathStatus::RegularFile { size }) {
        return Err(DeviceIoError::Verification(path.to_string()));
    }
    Ok(())
}

type PlannedTransfer = (String, DeviceCatalogEntry, Utf8PathBuf);

fn plan_browser_transfer(
    catalog: &DeviceCatalog,
    targets: &[DeviceBrowserTarget],
    destination_storage_id: &str,
    destination: &Utf8Path,
    root_name: Option<&str>,
) -> Result<(Vec<PlannedTransfer>, u64), DeviceBrowserOperationError> {
    let destination_storage = validate_directory(catalog, destination_storage_id, destination)?;
    ensure_mutable(destination)?;
    validate_disjoint_targets(catalog, targets)?;
    let mut planned = Vec::new();
    let mut names = HashSet::new();
    let mut total = 0_u64;
    for target in targets {
        if target.storage_id == destination_storage_id
            && target.kind == DeviceCatalogEntryKind::Directory
            && destination.starts_with(&target.path)
        {
            return Err(DeviceBrowserOperationError::InvalidSelection(
                target.path.to_string(),
            ));
        }
        let source_storage = validate_target(catalog, target)?;
        let root = joined_path(destination, root_name.unwrap_or(entry_name(&target.path)));
        let prefix = descendant_prefix(&target.path);
        for entry in source_storage.entries.iter().filter(|entry| {
            entry.path == target.path
                || target.kind == DeviceCatalogEntryKind::Directory
                    && entry.path.as_str().starts_with(&prefix)
        }) {
            let relative = entry.path.strip_prefix(&target.path).map_err(|_| {
                DeviceBrowserOperationError::InvalidSelection(entry.path.to_string())
            })?;
            let path = if relative.as_str().is_empty() {
                root.clone()
            } else {
                root.join(relative)
            };
            ensure_mutable(&path)?;
            ensure_available(destination_storage, &path)?;
            if !names.insert(path.as_str().to_ascii_lowercase()) {
                return Err(DeviceBrowserOperationError::InvalidSelection(
                    path.to_string(),
                ));
            }
            if entry.kind == DeviceCatalogEntryKind::File {
                total = total
                    .checked_add(entry.size.ok_or_else(|| {
                        DeviceBrowserOperationError::MissingSize(entry.path.to_string())
                    })?)
                    .ok_or(DeviceBrowserOperationError::TransferLimit)?;
            }
            planned.push((target.storage_id.clone(), entry.clone(), path));
        }
    }
    if total > MAX_BROWSER_TRANSFER_BYTES {
        return Err(DeviceBrowserOperationError::TransferLimit);
    }
    Ok((planned, total))
}

fn validate_disjoint_targets(
    catalog: &DeviceCatalog,
    targets: &[DeviceBrowserTarget],
) -> Result<(), DeviceBrowserOperationError> {
    if targets.is_empty() {
        return Err(DeviceBrowserOperationError::InvalidSelection(
            "no items selected".to_owned(),
        ));
    }
    for (index, target) in targets.iter().enumerate() {
        validate_target(catalog, target)?;
        if target.path.as_str().is_empty() {
            return Err(DeviceBrowserOperationError::RootMutation);
        }
        ensure_mutable(&target.path)?;
        if targets[..index].iter().any(|previous| {
            previous.storage_id == target.storage_id
                && (previous.path == target.path
                    || previous.kind == DeviceCatalogEntryKind::Directory
                        && target.path.starts_with(&previous.path)
                    || target.kind == DeviceCatalogEntryKind::Directory
                        && previous.path.starts_with(&target.path))
        }) {
            return Err(DeviceBrowserOperationError::InvalidSelection(
                target.path.to_string(),
            ));
        }
    }
    Ok(())
}

async fn backup_entry<R>(
    device: &R,
    storage_id: &str,
    entry: &DeviceCatalogEntry,
    completed_before: u64,
    total: u64,
    progress: &ProgressReporter,
) -> Result<tempfile::TempPath, DeviceBrowserOperationError>
where
    R: DeviceRead + ?Sized,
{
    let size = entry
        .size
        .ok_or_else(|| DeviceBrowserOperationError::MissingSize(entry.path.to_string()))?;
    if size > MAX_BROWSER_TRANSFER_BYTES {
        return Err(DeviceBrowserOperationError::TransferLimit);
    }
    if progress.is_cancelled() {
        return Err(DeviceBrowserOperationError::Device(
            DeviceIoError::Cancelled,
        ));
    }
    let path = SafeRelativePath::parse(&entry.path)?;
    let temporary = tempfile::NamedTempFile::new()?;
    let destination =
        crate::storage::BackupDestination::new(temporary.path().to_owned(), temporary.reopen()?)?;
    device
        .backup(
            storage_id,
            &path,
            size,
            destination,
            crate::MountedMtpBackupProgress {
                reporter: progress.clone(),
                completed_before,
                total,
            },
        )
        .await?;
    if progress.is_cancelled() {
        return Err(DeviceBrowserOperationError::Device(
            DeviceIoError::Cancelled,
        ));
    }
    Ok(temporary.into_temp_path())
}

fn copy_local_file(
    source: &Path,
    destination: &mut impl Write,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError> {
    let mut source = std::fs::File::open(source)?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if progress.is_cancelled() {
            return Err(DeviceBrowserOperationError::Device(
                DeviceIoError::Cancelled,
            ));
        }
        let count = source.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        destination.write_all(&buffer[..count])?;
    }
}

async fn hash_local_file(
    source_path: &Path,
    expected_size: u64,
    progress: &ProgressReporter,
) -> Result<String, DeviceBrowserOperationError> {
    let mut source = tokio::fs::File::open(source_path).await?;
    let mut hash = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if progress.is_cancelled() {
            return Err(DeviceBrowserOperationError::Device(
                DeviceIoError::Cancelled,
            ));
        }
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        observed = observed
            .checked_add(
                u64::try_from(count).map_err(|_| DeviceBrowserOperationError::TransferLimit)?,
            )
            .ok_or(DeviceBrowserOperationError::TransferLimit)?;
        if observed > expected_size {
            return Err(DeviceBrowserOperationError::Device(
                DeviceIoError::Verification(source_path.display().to_string()),
            ));
        }
        hash.update(&buffer[..count]);
    }
    if observed != expected_size {
        return Err(DeviceBrowserOperationError::Device(
            DeviceIoError::Verification(source_path.display().to_string()),
        ));
    }
    Ok(hex::encode(hash.finalize()))
}

async fn remove_file<W>(
    device: &W,
    storage_id: &str,
    entry: &DeviceCatalogEntry,
    progress: &ProgressReporter,
) -> Result<(), DeviceBrowserOperationError>
where
    W: DeviceWrite + ?Sized,
{
    let size = entry
        .size
        .ok_or_else(|| DeviceBrowserOperationError::MissingSize(entry.path.to_string()))?;
    let path = SafeRelativePath::parse(&entry.path)?;
    device
        .delete_size_checked_with_progress(storage_id, &path, size, progress)
        .await?;
    Ok(())
}

fn validate_target<'a>(
    catalog: &'a DeviceCatalog,
    target: &DeviceBrowserTarget,
) -> Result<&'a DeviceCatalogStorage, DeviceBrowserOperationError> {
    let storage = catalog
        .storages
        .iter()
        .find(|storage| storage.id == target.storage_id)
        .ok_or_else(|| DeviceBrowserOperationError::Storage(target.storage_id.clone()))?;
    if target.path.as_str().is_empty() {
        if target.kind != DeviceCatalogEntryKind::Directory {
            return Err(DeviceBrowserOperationError::StaleTarget(
                target.path.to_string(),
            ));
        }
    } else {
        let _ = SafeRelativePath::parse(&target.path)?;
        let _ = exact_entry(storage, &target.path, target.kind)?;
    }
    Ok(storage)
}

fn validate_directory<'a>(
    catalog: &'a DeviceCatalog,
    storage_id: &str,
    path: &Utf8Path,
) -> Result<&'a DeviceCatalogStorage, DeviceBrowserOperationError> {
    let target = DeviceBrowserTarget {
        storage_id: storage_id.to_owned(),
        path: path.to_owned(),
        kind: DeviceCatalogEntryKind::Directory,
    };
    validate_target(catalog, &target)
}

fn exact_entry<'a>(
    storage: &'a DeviceCatalogStorage,
    path: &Utf8Path,
    kind: DeviceCatalogEntryKind,
) -> Result<&'a DeviceCatalogEntry, DeviceBrowserOperationError> {
    storage
        .entries
        .iter()
        .find(|entry| entry.path == path && entry.kind == kind)
        .ok_or_else(|| DeviceBrowserOperationError::StaleTarget(path.to_string()))
}

fn ensure_available(
    storage: &DeviceCatalogStorage,
    path: &Utf8Path,
) -> Result<(), DeviceBrowserOperationError> {
    if storage
        .entries
        .iter()
        .any(|entry| entry.path.as_str().eq_ignore_ascii_case(path.as_str()))
    {
        Err(DeviceBrowserOperationError::StaleTarget(path.to_string()))
    } else {
        Ok(())
    }
}

fn ensure_mutable(path: &Utf8Path) -> Result<(), DeviceBrowserOperationError> {
    if path
        .as_str()
        .split('/')
        .next()
        .is_some_and(|component| component.eq_ignore_ascii_case(TOOLKIT_ROOT))
    {
        Err(DeviceBrowserOperationError::ToolkitManaged)
    } else {
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), DeviceBrowserOperationError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed != name
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
        || matches!(name, "." | "..")
    {
        Err(DeviceBrowserOperationError::FileName(name.to_owned()))
    } else {
        let _ = SafeRelativePath::parse(name)?;
        Ok(())
    }
}

fn joined_path(parent: &Utf8Path, name: &str) -> Utf8PathBuf {
    if parent.as_str().is_empty() {
        name.into()
    } else {
        parent.join(name)
    }
}

fn descendant_prefix(path: &Utf8Path) -> String {
    if path.as_str().is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

fn entry_name(path: &Utf8Path) -> &str {
    path.file_name().unwrap_or(path.as_str())
}

fn archive_name(path: &Utf8Path, storage_label: &str) -> String {
    let base = if path.as_str().is_empty() {
        storage_label
    } else {
        entry_name(path)
    };
    let safe = base
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character => character,
        })
        .collect::<String>();
    format!("{safe}.zip")
}

fn archive_path(path: &Utf8Path, selected: &Utf8Path, archive_root: Option<&str>) -> String {
    let path = path.as_str();
    let selected = selected.as_str();
    let relative = if selected.is_empty() {
        path
    } else {
        path.strip_prefix(&format!("{selected}/")).unwrap_or(path)
    };
    match archive_root {
        Some(root) if relative.is_empty() => root.to_owned(),
        Some(root) => format!("{root}/{relative}"),
        None => relative.to_owned(),
    }
}

fn path_depth(path: &Utf8Path) -> usize {
    path.as_str().split('/').count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::DirectoryDevice;

    fn catalog() -> DeviceCatalog {
        DeviceCatalog {
            storages: vec![DeviceCatalogStorage {
                id: "primary".to_owned(),
                label: "Mock storage".to_owned(),
                entries: vec![
                    DeviceCatalogEntry {
                        path: "Garmin".into(),
                        kind: DeviceCatalogEntryKind::Directory,
                        size: None,
                    },
                    DeviceCatalogEntry {
                        path: "Garmin/Activity".into(),
                        kind: DeviceCatalogEntryKind::Directory,
                        size: None,
                    },
                    DeviceCatalogEntry {
                        path: "Garmin/Activity/ride.fit".into(),
                        kind: DeviceCatalogEntryKind::File,
                        size: Some(4),
                    },
                    DeviceCatalogEntry {
                        path: "GARMIN-TOOLKIT".into(),
                        kind: DeviceCatalogEntryKind::Directory,
                        size: None,
                    },
                ],
            }],
        }
    }

    async fn fixture() -> (tempfile::TempDir, DirectoryDevice) {
        let root = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(root.path().join("Garmin/Activity"))
            .await
            .unwrap();
        tokio::fs::create_dir(root.path().join("GARMIN-TOOLKIT"))
            .await
            .unwrap();
        tokio::fs::write(root.path().join("Garmin/Activity/ride.fit"), b"ride")
            .await
            .unwrap();
        let device = DirectoryDevice::new(root.path().to_owned());
        (root, device)
    }

    #[tokio::test]
    async fn downloads_directories_as_stored_zips() {
        let (_root, device) = fixture().await;
        for (path, file_name, entries) in [
            (
                "Garmin/Activity",
                "Activity.zip",
                &["Activity/", "Activity/ride.fit"][..],
            ),
            (
                "",
                "Mock storage.zip",
                &[
                    "GARMIN-TOOLKIT/",
                    "Garmin/",
                    "Garmin/Activity/",
                    "Garmin/Activity/ride.fit",
                ][..],
            ),
            (
                "GARMIN-TOOLKIT",
                "GARMIN-TOOLKIT.zip",
                &["GARMIN-TOOLKIT/"][..],
            ),
        ] {
            let download = prepare_browser_download(
                &device,
                &catalog(),
                &DeviceBrowserTarget {
                    storage_id: "primary".to_owned(),
                    path: path.into(),
                    kind: DeviceCatalogEntryKind::Directory,
                },
                &ProgressReporter::default(),
            )
            .await
            .unwrap();
            assert_eq!(download.file_name, file_name);
            let file = std::fs::File::open(download.path()).unwrap();
            assert_eq!(download.size, file.metadata().unwrap().len());
            let mut archive = zip::ZipArchive::new(file).unwrap();
            assert_eq!(
                archive.file_names().collect::<Result<Vec<_>, _>>().unwrap(),
                entries,
            );
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).unwrap();
                assert_eq!(entry.compression(), CompressionMethod::Stored);
                let permissions = if entry.is_dir() { 0o755 } else { 0o644 };
                assert_eq!(entry.unix_mode().unwrap() & 0o7777, permissions);
                if !entry.is_dir() {
                    let mut bytes = Vec::new();
                    entry.read_to_end(&mut bytes).unwrap();
                    assert_eq!(bytes, b"ride");
                }
            }
        }
    }

    #[tokio::test]
    async fn uploads_creates_and_recursively_removes_with_verification() {
        let (root, device) = fixture().await;
        let mut current = catalog();
        create_browser_directory(&device, &current, "primary", "Garmin", "New")
            .await
            .unwrap();
        current.storages[0].entries.push(DeviceCatalogEntry {
            path: "Garmin/New".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        });
        let upload = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(upload.path(), b"payload").await.unwrap();
        upload_browser_file_from_path(
            &device,
            &current,
            BrowserFileUpload {
                storage_id: "primary",
                directory: Utf8Path::new("Garmin/New"),
                file_name: "payload.bin",
                source: upload.path(),
                size: 7,
            },
            &ProgressReporter::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("Garmin/New/payload.bin"))
                .await
                .unwrap(),
            b"payload"
        );
        current.storages[0].entries.push(DeviceCatalogEntry {
            path: "Garmin/New/payload.bin".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(7),
        });
        remove_browser_target(
            &device,
            &current,
            &DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "Garmin/New".into(),
                kind: DeviceCatalogEntryKind::Directory,
            },
        )
        .await
        .unwrap();
        assert!(!root.path().join("Garmin/New").exists());
    }

    #[tokio::test]
    async fn copies_without_removing_source_and_moves_after_copy() {
        let (root, device) = fixture().await;
        let target = DeviceBrowserTarget {
            storage_id: "primary".to_owned(),
            path: "Garmin/Activity/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
        };
        transfer_browser_targets_with_progress(
            &device,
            &catalog(),
            std::slice::from_ref(&target),
            "primary",
            Utf8Path::new("Garmin"),
            true,
            &ProgressReporter::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("Garmin/ride.fit"))
                .await
                .unwrap(),
            b"ride"
        );
        assert!(root.path().join("Garmin/Activity/ride.fit").exists());

        let mut current = catalog();
        current.storages[0].entries.push(DeviceCatalogEntry {
            path: "Garmin/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(4),
        });
        transfer_browser_targets_with_progress(
            &device,
            &current,
            &[target],
            "primary",
            Utf8Path::new(""),
            false,
            &ProgressReporter::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("ride.fit")).await.unwrap(),
            b"ride"
        );
        assert!(!root.path().join("Garmin/Activity/ride.fit").exists());
    }

    #[tokio::test]
    async fn confirmed_replacement_overwrites_file_and_then_removes_moved_source() {
        let (root, device) = fixture().await;
        tokio::fs::write(root.path().join("Garmin/ride.fit"), b"old!")
            .await
            .unwrap();
        let mut current = catalog();
        current.storages[0].entries.push(DeviceCatalogEntry {
            path: "Garmin/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(4),
        });
        replace_browser_file_with_progress(
            &device,
            &current,
            &[DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "Garmin/Activity/ride.fit".into(),
                kind: DeviceCatalogEntryKind::File,
            }],
            "primary",
            Utf8Path::new("Garmin"),
            false,
            &ProgressReporter::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("Garmin/ride.fit"))
                .await
                .unwrap(),
            b"ride"
        );
        assert!(!root.path().join("Garmin/Activity/ride.fit").exists());
    }

    #[tokio::test]
    async fn confirmed_replacement_copy_preserves_source() {
        let (root, device) = fixture().await;
        tokio::fs::write(root.path().join("Garmin/ride.fit"), b"old!")
            .await
            .unwrap();
        let mut current = catalog();
        current.storages[0].entries.push(DeviceCatalogEntry {
            path: "Garmin/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(4),
        });
        replace_browser_file_with_progress(
            &device,
            &current,
            &[DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "Garmin/Activity/ride.fit".into(),
                kind: DeviceCatalogEntryKind::File,
            }],
            "primary",
            Utf8Path::new("Garmin"),
            true,
            &ProgressReporter::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("Garmin/ride.fit"))
                .await
                .unwrap(),
            b"ride"
        );
        assert_eq!(
            tokio::fs::read(root.path().join("Garmin/Activity/ride.fit"))
                .await
                .unwrap(),
            b"ride"
        );
    }

    #[tokio::test]
    async fn renames_a_file_after_verifying_the_new_copy() {
        let (root, device) = fixture().await;
        rename_browser_target_with_progress(
            &device,
            &catalog(),
            &DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "Garmin/Activity/ride.fit".into(),
                kind: DeviceCatalogEntryKind::File,
            },
            "morning.fit",
            &ProgressReporter::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("Garmin/Activity/morning.fit"))
                .await
                .unwrap(),
            b"ride"
        );
        assert!(!root.path().join("Garmin/Activity/ride.fit").exists());
    }

    #[tokio::test]
    async fn renames_a_folder_with_its_contents() {
        let (root, device) = fixture().await;
        rename_browser_target_with_progress(
            &device,
            &catalog(),
            &DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "Garmin/Activity".into(),
                kind: DeviceCatalogEntryKind::Directory,
            },
            "Activities",
            &ProgressReporter::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("Garmin/Activities/ride.fit"))
                .await
                .unwrap(),
            b"ride"
        );
        assert!(!root.path().join("Garmin/Activity").exists());
    }

    #[tokio::test]
    async fn replacement_recovery_restores_the_previous_file() {
        let (root, device) = fixture().await;
        let destination = root.path().join("Garmin/ride.fit");
        tokio::fs::write(&destination, b"new!").await.unwrap();
        let backup = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(backup.path(), b"old!").await.unwrap();
        let digest = hash_local_file(backup.path(), 4, &ProgressReporter::default())
            .await
            .unwrap();
        restore_browser_file(
            &device,
            "primary",
            &SafeRelativePath::parse(Utf8Path::new("Garmin/ride.fit")).unwrap(),
            4,
            backup.path(),
            &digest,
        )
        .await
        .unwrap();
        assert_eq!(tokio::fs::read(destination).await.unwrap(), b"old!");
    }

    #[tokio::test]
    async fn rejects_a_transfer_into_its_own_directory_before_writing() {
        let (root, device) = fixture().await;
        let error = transfer_browser_targets_with_progress(
            &device,
            &catalog(),
            &[DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "Garmin".into(),
                kind: DeviceCatalogEntryKind::Directory,
            }],
            "primary",
            Utf8Path::new("Garmin/Activity"),
            false,
            &ProgressReporter::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            DeviceBrowserOperationError::InvalidSelection(_)
        ));
        assert!(root.path().join("Garmin/Activity/ride.fit").exists());
    }

    #[tokio::test]
    async fn rejects_protected_batch_before_removing_any_file() {
        let (root, device) = fixture().await;
        let targets = [
            DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "Garmin/Activity/ride.fit".into(),
                kind: DeviceCatalogEntryKind::File,
            },
            DeviceBrowserTarget {
                storage_id: "primary".to_owned(),
                path: "GARMIN-TOOLKIT".into(),
                kind: DeviceCatalogEntryKind::Directory,
            },
        ];
        let error = remove_browser_targets_with_progress(
            &device,
            &catalog(),
            &targets,
            &ProgressReporter::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DeviceBrowserOperationError::ToolkitManaged));
        assert!(root.path().join("Garmin/Activity/ride.fit").exists());
    }

    #[tokio::test]
    async fn toolkit_namespace_rejects_mutation() {
        let (_root, device) = fixture().await;
        let error =
            create_browser_directory(&device, &catalog(), "primary", "GARMIN-TOOLKIT", "unsafe")
                .await
                .unwrap_err();
        assert!(matches!(error, DeviceBrowserOperationError::ToolkitManaged));
    }
}
