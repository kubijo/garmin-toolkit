use garmin_capture::{CaptureError, SessionCapture};
use garmin_progress::{OperationStage, ProgressReporter};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone, Serialize)]
pub struct BackupReport {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub bytes: u64,
    pub files: Vec<BackupFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

struct SourceFile {
    source: PathBuf,
    relative: PathBuf,
    bytes: u64,
}

/// Copy and independently verify every regular file on a mounted device.
/// # Errors
/// Nested destination, unsafe file, insufficient space, source change, or I/O failure.
pub async fn backup_mass_storage(
    source: &Path,
    capture: &SessionCapture,
    destination: &Path,
    progress: &ProgressReporter,
) -> Result<BackupReport, BackupError> {
    let source = tokio::fs::canonicalize(source).await?;
    let capture_root = tokio::fs::canonicalize(capture.root()).await?;
    let destination_path = capture.artifact_path(destination)?;
    if capture_root.starts_with(&source) {
        return Err(BackupError::DestinationInsideDevice(destination.to_owned()));
    }
    if tokio::fs::symlink_metadata(&destination_path).await.is_ok() {
        return Err(BackupError::DestinationExists(destination_path));
    }

    let (directories, files, total) = inventory(&source).await?;
    let available = fs2::available_space(&capture_root)?;
    if available < total {
        return Err(BackupError::InsufficientSpace {
            required: total,
            available,
        });
    }
    capture.create_directory(destination).await?;
    for relative in directories {
        capture
            .create_directory(&destination.join(relative))
            .await?;
    }

    progress.started(
        OperationStage::Backup,
        "Copying the complete device before installation",
        Some(total),
    );
    let mut completed = 0_u64;
    let mut backed_up = Vec::with_capacity(files.len());
    for file in files {
        if progress.is_cancelled() {
            return Err(BackupError::Cancelled);
        }
        let target = destination.join(&file.relative);
        let sha256 = copy_and_verify(&file.source, capture, &target, file.bytes, progress).await?;
        completed = completed
            .checked_add(file.bytes)
            .ok_or(BackupError::SizeOverflow)?;
        progress.advanced_with_path(
            OperationStage::Backup,
            "Backed up device file",
            file.relative.display().to_string(),
            completed,
            Some(total),
        );
        backed_up.push(BackupFile {
            path: file.relative,
            bytes: file.bytes,
            sha256,
        });
    }
    progress.completed(
        OperationStage::Backup,
        "Complete device backup copied and verified",
        total,
        Some(total),
    );
    Ok(BackupReport {
        source,
        destination: destination_path,
        bytes: total,
        files: backed_up,
    })
}

async fn inventory(root: &Path) -> Result<(Vec<PathBuf>, Vec<SourceFile>, u64), BackupError> {
    let mut pending = VecDeque::from([PathBuf::new()]);
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut total = 0_u64;
    while let Some(relative_directory) = pending.pop_front() {
        let mut entries = tokio::fs::read_dir(root.join(&relative_directory)).await?;
        while let Some(entry) = entries.next_entry().await? {
            let relative = relative_directory.join(entry.file_name());
            let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
            if metadata.file_type().is_symlink() {
                return Err(BackupError::UnsupportedEntry(entry.path()));
            }
            if metadata.is_dir() {
                directories.push(relative.clone());
                pending.push_back(relative);
            } else if metadata.is_file() {
                total = total
                    .checked_add(metadata.len())
                    .ok_or(BackupError::SizeOverflow)?;
                files.push(SourceFile {
                    source: entry.path(),
                    relative,
                    bytes: metadata.len(),
                });
            } else {
                return Err(BackupError::UnsupportedEntry(entry.path()));
            }
        }
    }
    directories.sort();
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok((directories, files, total))
}

async fn copy_and_verify(
    source: &Path,
    capture: &SessionCapture,
    destination: &Path,
    expected_bytes: u64,
    progress: &ProgressReporter,
) -> Result<String, BackupError> {
    let mut input = tokio::fs::File::open(source).await?;
    if input.metadata().await?.len() != expected_bytes {
        return Err(BackupError::SourceChanged(source.to_owned()));
    }
    let destination_path = capture.artifact_path(destination)?;
    let mut output = capture.create_file(destination).await?;
    let mut source_hash = Sha256::new();
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    let mut copied = 0_u64;
    loop {
        if progress.is_cancelled() {
            return Err(BackupError::Cancelled);
        }
        let count = input.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(u64::try_from(count).map_err(|_| BackupError::SizeOverflow)?)
            .ok_or(BackupError::SizeOverflow)?;
        if copied > expected_bytes {
            return Err(BackupError::SourceChanged(source.to_owned()));
        }
        source_hash.update(&buffer[..count]);
        output.write_all(&buffer[..count]).await?;
    }
    if copied != expected_bytes {
        return Err(BackupError::SourceChanged(source.to_owned()));
    }
    output.flush().await?;
    output.sync_all().await?;
    drop(output);

    let expected_hash = source_hash.finalize();
    let mut backup = tokio::fs::File::open(&destination_path).await?;
    let mut backup_hash = Sha256::new();
    loop {
        if progress.is_cancelled() {
            return Err(BackupError::Cancelled);
        }
        let count = backup.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        backup_hash.update(&buffer[..count]);
    }
    let actual_hash = backup_hash.finalize();
    if expected_hash != actual_hash {
        return Err(BackupError::Verification(destination_path));
    }
    Ok(hex::encode(expected_hash))
}

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("device backup cancelled")]
    Cancelled,
    #[error("backup destination has no safe parent: {0}")]
    UnsafeDestination(PathBuf),
    #[error("backup destination is inside the device: {0}")]
    DestinationInsideDevice(PathBuf),
    #[error("backup destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("device backup does not support a symbolic link or special file: {0}")]
    UnsupportedEntry(PathBuf),
    #[error("device file changed while it was being backed up: {0}")]
    SourceChanged(PathBuf),
    #[error("backup verification failed: {0}")]
    Verification(PathBuf),
    #[error("backup requires {required} bytes but only {available} are available")]
    InsufficientSpace { required: u64, available: u64 },
    #[error("backup byte count exceeds u64")]
    SizeOverflow,
    #[error("device backup I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("device backup capture failed: {0}")]
    Capture(#[from] CaptureError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn copies_and_independently_verifies_the_complete_tree() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("device");
        let capture_root = temporary.path().join("capture");
        let destination = capture_root.join("device-backup");
        tokio::fs::create_dir_all(source.join("Garmin/maps"))
            .await
            .unwrap();
        let capture = SessionCapture::create(&capture_root).unwrap();
        tokio::fs::write(source.join("GarminDevice.xml"), b"manifest")
            .await
            .unwrap();
        tokio::fs::write(source.join("Garmin/maps/map.img"), b"map bytes")
            .await
            .unwrap();

        let report = backup_mass_storage(
            &source,
            &capture,
            Path::new("device-backup"),
            &ProgressReporter::default(),
        )
        .await
        .unwrap();

        assert_eq!(report.files.len(), 2);
        assert_eq!(report.bytes, 17);
        assert_eq!(
            tokio::fs::read(destination.join("GarminDevice.xml"))
                .await
                .unwrap(),
            b"manifest"
        );
        assert_eq!(
            tokio::fs::read(destination.join("Garmin/maps/map.img"))
                .await
                .unwrap(),
            b"map bytes"
        );
        assert!(report.files.iter().all(|file| file.sha256.len() == 64));
    }

    #[tokio::test]
    async fn cancellation_stops_before_copying_a_file() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("device");
        let capture_root = temporary.path().join("capture");
        let destination = capture_root.join("device-backup");
        tokio::fs::create_dir(&source).await.unwrap();
        let capture = SessionCapture::create(&capture_root).unwrap();
        tokio::fs::write(source.join("map.img"), b"map")
            .await
            .unwrap();
        let progress = ProgressReporter::default();
        progress.cancellation_token().cancel();

        let result =
            backup_mass_storage(&source, &capture, Path::new("device-backup"), &progress).await;

        assert!(matches!(result, Err(BackupError::Cancelled)));
        assert!(!destination.join("map.img").exists());
    }
}
