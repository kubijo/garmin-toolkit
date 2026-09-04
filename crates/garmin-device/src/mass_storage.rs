use crate::manifest::{MAX_MANIFEST_BYTES, ManifestError, parse_manifest};
use crate::{
    DeviceInventory, DeviceManifest, DevicePathInspection, DevicePathState, SafeRelativePath,
    TransportKind,
};
use crate::{DeviceProbeReport, UploadCompletion};
use garmin_progress::{OperationStage, ProgressReporter};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum MassStorageError {
    #[error("unable to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("no GarminDevice.xml below {0}")]
    NotGarmin(PathBuf),
    #[error("unsafe symbolic link or special file in device path: {0}")]
    UnsafeDevicePath(PathBuf),
    #[error("device probe I/O failed: {0}")]
    ProbeIo(std::io::Error),
    #[error("device probe size mismatch: expected {expected}, observed {actual}")]
    ProbeSize { expected: u64, actual: u64 },
    #[error("device probe read-back checksum did not match the source")]
    ProbeChecksum,
    #[error("device probe cancelled")]
    Cancelled,
    #[error("device probe failed ({operation}); disposable-file cleanup failed ({cleanup})")]
    ProbeCleanup {
        operation: String,
        cleanup: std::io::Error,
    },
    #[error("disposable device file still exists after deletion")]
    ProbeNotRemoved,
}

/// Read a manifest from a real mounted directory without following symlinks.
/// # Errors
/// Unsafe root or absent, oversized, unreadable, or invalid manifest.
pub async fn open_mass_storage(root: &Path) -> Result<DeviceManifest, MassStorageError> {
    require_real_directory(root).await?;
    let garmin_directory = root.join("Garmin");
    match tokio::fs::symlink_metadata(&garmin_directory).await {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err(MassStorageError::UnsafeDevicePath(garmin_directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(MassStorageError::Read {
                path: garmin_directory,
                source,
            });
        }
    }
    let candidates = [
        root.join("Garmin/GarminDevice.xml"),
        root.join("GarminDevice.xml"),
    ];
    let mut manifest_path = None;
    for candidate in candidates {
        match tokio::fs::symlink_metadata(&candidate).await {
            Ok(metadata) if metadata.file_type().is_file() => {
                manifest_path = Some(candidate);
                break;
            }
            Ok(_) => return Err(MassStorageError::UnsafeDevicePath(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(MassStorageError::Read {
                    path: candidate,
                    source,
                });
            }
        }
    }
    let manifest_path =
        manifest_path.ok_or_else(|| MassStorageError::NotGarmin(root.to_path_buf()))?;
    let metadata = tokio::fs::metadata(&manifest_path)
        .await
        .map_err(|source| MassStorageError::Read {
            path: manifest_path.clone(),
            source,
        })?;
    if metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(ManifestError::TooLarge.into());
    }
    let xml = tokio::fs::read_to_string(&manifest_path)
        .await
        .map_err(|source| MassStorageError::Read {
            path: manifest_path,
            source,
        })?;
    parse_manifest(&xml, TransportKind::MassStorage, root.display().to_string()).map_err(Into::into)
}

pub async fn discover_mass_storage() -> Vec<DeviceManifest> {
    let mut roots = BTreeSet::new();
    if let Ok(mounts) = std::fs::read_to_string("/proc/self/mountinfo") {
        for line in mounts.lines() {
            if let Some(encoded) = line.split_whitespace().nth(4) {
                roots.insert(PathBuf::from(unescape_mount(encoded)));
            }
        }
    }

    let mut devices = Vec::new();
    for root in roots {
        if let Ok(device) = open_mass_storage(&root).await {
            devices.push(device);
        }
    }
    devices
}

/// Inspect declared device-relative paths without reading or changing content.
///
/// Symbolic links and special objects are reported as `Other` and never followed.
/// Missing paths remain explicit in the result.
/// # Errors
/// Device-root or path-metadata inspection failure.
pub async fn inventory_mass_storage(
    root: &Path,
    paths: &[SafeRelativePath],
) -> Result<DeviceInventory, MassStorageError> {
    require_real_directory(root).await?;
    let mut inspected = Vec::with_capacity(paths.len());
    for path in paths {
        let (state, size) = inspect_mass_storage_path(root, path).await?;
        inspected.push(DevicePathInspection {
            storage_id: "primary".to_owned(),
            storage_label: "Device storage".to_owned(),
            path: path.clone(),
            state,
            size,
        });
    }
    Ok(DeviceInventory {
        transport: TransportKind::MassStorage,
        paths: inspected,
    })
}

async fn inspect_mass_storage_path(
    root: &Path,
    relative: &SafeRelativePath,
) -> Result<(DevicePathState, Option<u64>), MassStorageError> {
    let mut current = root.to_owned();
    let component_count = relative.as_path().components().count();
    for (index, component) in relative.as_path().components().enumerate() {
        current.push(component.as_os_str());
        let metadata = match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((DevicePathState::Missing, None));
            }
            Err(source) => {
                return Err(MassStorageError::Read {
                    path: current,
                    source,
                });
            }
        };
        let is_last = index + 1 == component_count;
        if !is_last && !metadata.file_type().is_dir() {
            return Ok((DevicePathState::Other, None));
        }
        if is_last {
            return Ok(if metadata.file_type().is_file() {
                (DevicePathState::RegularFile, Some(metadata.len()))
            } else if metadata.file_type().is_dir() {
                (DevicePathState::Directory, None)
            } else {
                (DevicePathState::Other, None)
            });
        }
    }
    Ok((DevicePathState::Other, None))
}

/// Copy a local file to a unique disposable name, verify its size, and delete it.
/// # Errors
/// [`MassStorageError`] when the source or device transfer fails.
pub async fn probe_mass_storage_file(
    root: &Path,
    source: &Path,
    progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MassStorageError> {
    require_real_directory(root).await?;
    require_real_directory(&root.join("Garmin")).await?;
    let bytes = tokio::fs::metadata(source)
        .await
        .map_err(MassStorageError::ProbeIo)?
        .len();
    let destination = root
        .join("Garmin")
        .join(format!(".garmin-cli-probe-{}.bin", Uuid::new_v4()));
    let expected_sha256 = hash_probe_file(source, bytes, None).await?;
    let (upload_seconds, device_finalize_seconds) =
        copy_probe_file(source, &destination, bytes, progress).await?;
    let read_back_seconds =
        match verify_probe_file(&destination, bytes, &expected_sha256, progress).await {
            Ok(elapsed) => elapsed,
            Err(error) => return Err(cleanup_probe_error(&destination, error).await),
        };
    let cleanup_seconds = delete_probe_file(&destination, progress).await?;

    Ok(DeviceProbeReport {
        transport: TransportKind::MassStorage,
        bytes,
        upload_seconds,
        device_finalize_seconds,
        upload_completion: UploadCompletion::Acknowledged,
        read_back_seconds,
        cleanup_seconds,
        content_verified: true,
        temporary_file_removed: true,
    })
}

async fn require_real_directory(path: &Path) -> Result<(), MassStorageError> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(MassStorageError::ProbeIo)?;
    if !metadata.file_type().is_dir() {
        return Err(MassStorageError::UnsafeDevicePath(path.to_owned()));
    }
    Ok(())
}

async fn copy_probe_file(
    source: &Path,
    destination: &Path,
    bytes: u64,
    progress: &ProgressReporter,
) -> Result<(f64, f64), MassStorageError> {
    let mut input = tokio::fs::File::open(source)
        .await
        .map_err(MassStorageError::ProbeIo)?;
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await
        .map_err(MassStorageError::ProbeIo)?;
    let label = "Copying disposable map file";
    progress.started(OperationStage::Upload, label, Some(bytes));
    let started = Instant::now();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    let transfer = async {
        loop {
            if progress.is_cancelled() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "device probe cancelled",
                ));
            }
            let count = input.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count]).await?;
            copied = copied
                .checked_add(u64::try_from(count).map_err(std::io::Error::other)?)
                .ok_or_else(|| std::io::Error::other("probe byte count overflow"))?;
            progress.advanced(OperationStage::Upload, label, copied, Some(bytes));
        }
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(error) = transfer {
        progress.failed(OperationStage::Upload, "Device copy failed");
        drop(output);
        let operation = if progress.is_cancelled() {
            MassStorageError::Cancelled
        } else {
            MassStorageError::ProbeIo(error)
        };
        return Err(cleanup_probe_error(destination, operation).await);
    }
    let upload_seconds = started.elapsed().as_secs_f64();
    progress.completed(OperationStage::Upload, label, copied, Some(bytes));
    progress.started(
        OperationStage::DeviceFinalize,
        "Synchronizing disposable device file",
        None,
    );
    let finalize_started = Instant::now();
    if let Err(error) = async {
        output.flush().await?;
        output.sync_all().await
    }
    .await
    {
        progress.failed(
            OperationStage::DeviceFinalize,
            "Device file synchronization failed",
        );
        drop(output);
        return Err(cleanup_probe_error(destination, MassStorageError::ProbeIo(error)).await);
    }
    let finalize_seconds = finalize_started.elapsed().as_secs_f64();
    progress.completed(
        OperationStage::DeviceFinalize,
        "Disposable device file synchronized",
        1,
        Some(1),
    );
    Ok((upload_seconds, finalize_seconds))
}

async fn verify_probe_file(
    destination: &Path,
    expected: u64,
    expected_sha256: &str,
    progress: &ProgressReporter,
) -> Result<f64, MassStorageError> {
    progress.started(
        OperationStage::DeviceVerify,
        "Reading device file back",
        Some(expected),
    );
    let started = Instant::now();
    let actual_sha256 = match hash_probe_file(destination, expected, Some(progress)).await {
        Ok(hash) => hash,
        Err(error) => {
            progress.failed(OperationStage::DeviceVerify, "Device read-back failed");
            return Err(error);
        }
    };
    if actual_sha256 != expected_sha256 {
        progress.failed(
            OperationStage::DeviceVerify,
            "Device read-back checksum mismatch",
        );
        return Err(MassStorageError::ProbeChecksum);
    }
    progress.completed(
        OperationStage::DeviceVerify,
        "Read back and SHA-256 verified",
        expected,
        Some(expected),
    );
    Ok(started.elapsed().as_secs_f64())
}

async fn hash_probe_file(
    path: &Path,
    expected: u64,
    progress: Option<&ProgressReporter>,
) -> Result<String, MassStorageError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(MassStorageError::ProbeIo)?;
    let mut hash = Sha256::new();
    let mut actual = 0_u64;
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    loop {
        if progress.is_some_and(ProgressReporter::is_cancelled) {
            return Err(MassStorageError::Cancelled);
        }
        let count = file
            .read(&mut buffer)
            .await
            .map_err(MassStorageError::ProbeIo)?;
        if count == 0 {
            break;
        }
        let count_u64 = u64::try_from(count)
            .map_err(std::io::Error::other)
            .map_err(MassStorageError::ProbeIo)?;
        actual = actual
            .checked_add(count_u64)
            .ok_or_else(|| std::io::Error::other("probe byte count overflow"))
            .map_err(MassStorageError::ProbeIo)?;
        if actual > expected {
            return Err(MassStorageError::ProbeSize { expected, actual });
        }
        hash.update(&buffer[..count]);
        if let Some(progress) = progress {
            progress.advanced(
                OperationStage::DeviceVerify,
                "Reading device file back",
                actual,
                Some(expected),
            );
        }
    }
    if actual != expected {
        return Err(MassStorageError::ProbeSize { expected, actual });
    }
    Ok(hex::encode(hash.finalize()))
}

async fn cleanup_probe_error(destination: &Path, operation: MassStorageError) -> MassStorageError {
    match tokio::fs::remove_file(destination).await {
        Ok(()) => operation,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => operation,
        Err(cleanup) => MassStorageError::ProbeCleanup {
            operation: operation.to_string(),
            cleanup,
        },
    }
}

async fn delete_probe_file(
    destination: &Path,
    progress: &ProgressReporter,
) -> Result<f64, MassStorageError> {
    progress.started(
        OperationStage::Delete,
        "Removing disposable device file",
        None,
    );
    let started = Instant::now();
    tokio::fs::remove_file(destination)
        .await
        .map_err(MassStorageError::ProbeIo)?;
    if destination.exists() {
        progress.failed(
            OperationStage::Delete,
            "Disposable device file still exists",
        );
        return Err(MassStorageError::ProbeNotRemoved);
    }
    progress.completed(
        OperationStage::Delete,
        "Disposable device file removed",
        1,
        Some(1),
    );
    Ok(started.elapsed().as_secs_f64())
}

fn unescape_mount(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use garmin_progress::ProgressState;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn refuses_a_manifest_below_a_symlinked_directory() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        let outside = fixture.path().join("outside");
        tokio::fs::create_dir(&device).await.unwrap();
        tokio::fs::create_dir(&outside).await.unwrap();
        tokio::fs::write(outside.join("GarminDevice.xml"), b"outside")
            .await
            .unwrap();
        symlink(&outside, device.join("Garmin")).unwrap();

        let result = open_mass_storage(&device).await;

        assert!(matches!(result, Err(MassStorageError::UnsafeDevicePath(_))));
    }

    #[tokio::test]
    async fn probe_reads_back_every_byte_before_removing_the_device_file() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        let source = fixture.path().join("source.bin");
        tokio::fs::create_dir_all(device.join("Garmin"))
            .await
            .unwrap();
        tokio::fs::write(&source, vec![0x5a; 5 * 1024 * 1024])
            .await
            .unwrap();
        let (progress, receiver) = ProgressReporter::channel();

        let report = probe_mass_storage_file(&device, &source, &progress)
            .await
            .unwrap();
        let events = receiver.try_iter().collect::<Vec<_>>();

        assert!(report.content_verified);
        assert!(report.read_back_seconds > 0.0);
        assert!(report.temporary_file_removed);
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
        assert!(events.iter().any(|event| {
            event.stage == OperationStage::DeviceVerify
                && event.state == ProgressState::Advanced
                && event.completed == report.bytes
        }));
        assert!(
            std::fs::read_dir(device.join("Garmin"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn cancelled_probe_removes_the_partial_device_file() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        let source = fixture.path().join("source.bin");
        tokio::fs::create_dir_all(device.join("Garmin"))
            .await
            .unwrap();
        tokio::fs::write(&source, vec![0x5a; 5 * 1024 * 1024])
            .await
            .unwrap();
        let progress = ProgressReporter::default();
        progress.cancellation_token().cancel();

        let result = probe_mass_storage_file(&device, &source, &progress).await;

        assert!(matches!(result, Err(MassStorageError::Cancelled)));
        assert!(
            std::fs::read_dir(device.join("Garmin"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}
