//! Transaction I/O, independent of transport and execution mode.

use std::{
    fs::File,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    DeviceInventory, DevicePathStatus, DeviceProbeReport, DeviceStateSnapshot, MassStorageError,
    MountedMtpBackupProgress, MountedMtpDevice, MountedMtpError, MountedMtpUploadProgress,
    MountedMtpVerifyProgress, MtpError, SafeRelativePath,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDirectoryEntry {
    pub name: String,
    pub state: crate::DevicePathState,
    pub size: Option<u64>,
}

mod directory;
pub(crate) mod raw_mtp;

pub use directory::DirectoryDevice;
pub use raw_mtp::MtpStorageDevice;

#[derive(Debug, Clone)]
pub struct DeviceProbeRequest {
    pub source: PathBuf,
    pub recovery_root: PathBuf,
    pub device_identity: String,
}

#[derive(Debug, Clone)]
pub struct DeviceLinkCapacity {
    pub state: DeviceStateSnapshot,
    pub storage_id: String,
}

#[async_trait::async_trait]
pub trait DeviceLink: Send + Sync {
    async fn capacity(&self) -> Result<Option<DeviceLinkCapacity>, DeviceLinkError>;
    async fn probe(
        &self,
        request: &DeviceProbeRequest,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<DeviceProbeReport, DeviceLinkError>;
}

#[derive(Debug, Error)]
pub enum DeviceLinkError {
    #[error(transparent)]
    Device(#[from] DeviceIoError),
    #[error(transparent)]
    MassStorage(#[from] MassStorageError),
    #[error(transparent)]
    MountedMtp(#[from] MountedMtpError),
    #[error(transparent)]
    Mtp(#[from] MtpError),
}

/// Exclusively created local file reserved for one device backup.
#[derive(Debug)]
pub struct BackupDestination {
    path: PathBuf,
    file: File,
}

impl BackupDestination {
    /// Bind an open file to the path used to create it.
    /// # Errors
    /// The path must resolve directly to a regular file.
    pub fn new(path: PathBuf, file: File) -> Result<Self, DeviceIoError> {
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || !file.metadata()?.is_file() {
            return Err(DeviceIoError::UnsafePath(path.display().to_string()));
        }
        Ok(Self { path, file })
    }

    #[must_use]
    pub fn into_file(self) -> File {
        self.file
    }

    #[must_use]
    pub fn into_parts(self) -> (PathBuf, File) {
        (self.path, self.file)
    }
}

/// Reads available to snapshot preparation; no mutation capability is exposed.
#[async_trait::async_trait]
pub trait DeviceRead: Send + Sync {
    fn execution_target(&self) -> Option<&str>;
    async fn state(&self) -> Result<crate::DeviceStateSnapshot, DeviceIoError>;
    async fn state_with_progress(
        &self,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<crate::DeviceStateSnapshot, DeviceIoError> {
        require_running(progress)?;
        let state = self.state().await?;
        require_running(progress)?;
        Ok(state)
    }
    async fn inventory(&self, paths: &[SafeRelativePath])
    -> Result<DeviceInventory, DeviceIoError>;
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError>;
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<DevicePathStatus, DeviceIoError>;
    async fn inspect_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<DevicePathStatus, DeviceIoError> {
        require_running(progress)?;
        let inspection = self.inspect(storage, path).await?;
        require_running(progress)?;
        Ok(inspection)
    }
    async fn backup(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError>;
    async fn verify(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError>;
    async fn verify_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
        progress: MountedMtpVerifyProgress,
    ) -> Result<(), DeviceIoError> {
        if progress.reporter.is_cancelled() {
            return Err(DeviceIoError::Cancelled);
        }
        self.verify(storage, path, size, sha256).await?;
        if progress.reporter.is_cancelled() {
            return Err(DeviceIoError::Cancelled);
        }
        progress.advanced(path, size);
        Ok(())
    }

    /// Read a bounded regular file.
    async fn read_bounded_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceIoError> {
        let status = self.inspect(storage, path).await?;
        match status {
            DevicePathStatus::Missing => Ok(None),
            DevicePathStatus::RegularFile { size } if size <= limit => {
                Err(DeviceIoError::Transport(format!(
                    "bounded reads are unsupported for {storage}:{path} ({size} bytes)"
                )))
            }
            DevicePathStatus::RegularFile { .. } => {
                Err(DeviceIoError::LimitExceeded(path.to_string()))
            }
            _ => Err(DeviceIoError::UnsafePath(path.to_string())),
        }
    }

    /// List direct children.
    async fn list_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<Vec<DeviceDirectoryEntry>, DeviceIoError> {
        Err(DeviceIoError::Transport(format!(
            "directory listing is unsupported for {storage}:{path}"
        )))
    }
}

/// Mutation authority granted only to the transaction destination.
#[async_trait::async_trait]
pub trait DeviceWrite: DeviceRead {
    /// Create missing directories.
    async fn ensure_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        Err(DeviceIoError::Transport(format!(
            "directory creation is unsupported for {storage}:{path}"
        )))
    }

    /// Create and verify a small file.
    async fn create_verified_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), DeviceIoError> {
        Err(DeviceIoError::Transport(format!(
            "small verified writes are unsupported for {storage}:{path} ({} bytes)",
            bytes.len()
        )))
    }

    /// Remove an empty directory.
    async fn remove_empty_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        Err(DeviceIoError::Transport(format!(
            "directory removal is unsupported for {storage}:{path}"
        )))
    }

    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError>;
    async fn delete_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<(), DeviceIoError> {
        require_running(progress)?;
        self.delete(storage, path, size, sha256).await?;
        require_running(progress)
    }
    /// Delete an exact path after size validation without reading its contents.
    ///
    /// Delete a transaction-owned path after checking its size.
    async fn delete_size_checked(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        Err(DeviceIoError::Transport(format!(
            "size-checked deletion is unsupported for {storage}:{path} ({size} bytes)"
        )))
    }
    async fn delete_size_checked_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<(), DeviceIoError> {
        require_running(progress)?;
        self.delete_size_checked(storage, path, size).await?;
        require_running(progress)
    }
    async fn upload(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        source: &Path,
        size: u64,
        sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> Result<(), DeviceIoError>;
    async fn restore(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> Result<(), DeviceIoError>;
    async fn restore_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<(), DeviceIoError> {
        require_running(progress)?;
        self.restore(storage, path, size, backup, sha256).await?;
        require_running(progress)
    }
}

fn require_running(progress: &garmin_progress::ProgressReporter) -> Result<(), DeviceIoError> {
    if progress.is_cancelled() {
        Err(DeviceIoError::Cancelled)
    } else {
        Ok(())
    }
}

fn path_status(
    (state, size): (crate::DevicePathState, Option<u64>),
) -> Result<DevicePathStatus, DeviceIoError> {
    DevicePathStatus::from_parts(state, size).ok_or_else(|| {
        DeviceIoError::Transport(format!(
            "device returned inconsistent path metadata: {state:?}, size {size:?}"
        ))
    })
}

#[async_trait::async_trait]
impl DeviceRead for MountedMtpDevice {
    fn execution_target(&self) -> Option<&str> {
        None
    }
    async fn state(&self) -> Result<crate::DeviceStateSnapshot, DeviceIoError> {
        Ok(self.state_snapshot().await?)
    }
    async fn state_with_progress(
        &self,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<crate::DeviceStateSnapshot, DeviceIoError> {
        Ok(self.state_snapshot_with_progress(progress).await?)
    }
    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, DeviceIoError> {
        Ok(self.backend_inventory(paths).await?)
    }
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        Ok(self.backend_primary_storage_id().await?)
    }
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<DevicePathStatus, DeviceIoError> {
        path_status(self.backend_inspect(storage, path).await?)
    }
    async fn inspect_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<DevicePathStatus, DeviceIoError> {
        path_status(
            self.backend_inspect_with_progress(storage, path, progress)
                .await?,
        )
    }
    async fn backup(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError> {
        Ok(self
            .backend_backup(storage, path, size, destination, progress)
            .await?)
    }
    async fn verify(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        Ok(self.backend_verify(storage, path, size, sha256).await?)
    }
    async fn verify_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
        progress: MountedMtpVerifyProgress,
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_verify_with_progress(storage, path, size, sha256, progress)
            .await?)
    }

    async fn read_bounded_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, DeviceIoError> {
        Ok(self.backend_read_bounded_file(storage, path, limit).await?)
    }

    async fn list_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<Vec<DeviceDirectoryEntry>, DeviceIoError> {
        Ok(self.backend_list_directory(storage, path).await?)
    }
}

#[async_trait::async_trait]
impl DeviceWrite for MountedMtpDevice {
    async fn ensure_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        Ok(self.backend_ensure_directory(storage, path).await?)
    }

    async fn create_verified_file(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_create_verified_file(storage, path, bytes)
            .await?)
    }

    async fn remove_empty_directory(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(), DeviceIoError> {
        Ok(self.backend_remove_empty_directory(storage, path).await?)
    }

    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        Ok(self.backend_delete(storage, path, size, sha256).await?)
    }
    async fn delete_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_delete_with_progress(storage, path, size, sha256, progress)
            .await?)
    }
    async fn delete_size_checked(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_delete_size_checked(storage, path, size)
            .await?)
    }
    async fn delete_size_checked_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_delete_size_checked_with_progress(storage, path, size, progress)
            .await?)
    }
    async fn upload(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        source: &Path,
        size: u64,
        sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_upload(storage, path, source, size, sha256, progress)
            .await?)
    }
    async fn restore(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_restore(storage, path, size, backup, sha256)
            .await?)
    }
    async fn restore_with_progress(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<(), DeviceIoError> {
        Ok(self
            .backend_restore_with_progress(storage, path, size, backup, sha256, progress)
            .await?)
    }
}

#[async_trait::async_trait]
impl DeviceLink for MountedMtpDevice {
    async fn capacity(&self) -> Result<Option<DeviceLinkCapacity>, DeviceLinkError> {
        Ok(Some(DeviceLinkCapacity {
            state: self.state().await?,
            storage_id: DeviceRead::primary_storage_id(self).await?,
        }))
    }

    async fn probe(
        &self,
        request: &DeviceProbeRequest,
        progress: &garmin_progress::ProgressReporter,
    ) -> Result<DeviceProbeReport, DeviceLinkError> {
        Ok(self.backend_probe(&request.source, progress).await?)
    }
}

#[derive(Debug, Error)]
pub enum DeviceIoError {
    #[error("device operation cancelled")]
    Cancelled,
    #[error("device storage is unavailable: {0}")]
    Storage(String),
    #[error("unsafe or ambiguous device path: {0}")]
    UnsafePath(String),
    #[error("device object failed a required metadata or content check: {0}")]
    Verification(String),
    #[error("refusing to overwrite device file: {0}")]
    Occupied(String),
    #[error("device metadata file exceeds its configured limit: {0}")]
    LimitExceeded(String),
    #[error("device I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("device transport failed: {0}")]
    Transport(String),
}

impl DeviceIoError {
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

impl From<MountedMtpError> for DeviceIoError {
    fn from(error: MountedMtpError) -> Self {
        match error {
            MountedMtpError::Cancelled => Self::Cancelled,
            error @ (MountedMtpError::RemovalObjectSize { .. }
            | MountedMtpError::RemovalObjectChecksum(_)
            | MountedMtpError::UploadMetadata(_)) => Self::Verification(error.to_string()),
            error => Self::Transport(error.to_string()),
        }
    }
}

impl From<mtp_rs::Error> for DeviceIoError {
    fn from(error: mtp_rs::Error) -> Self {
        Self::Transport(error.to_string())
    }
}
