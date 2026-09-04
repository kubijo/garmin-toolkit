//! Transaction I/O, independent of transport and execution mode.

use std::{
    fs::File,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    DeviceInventory, DevicePathState, DeviceProbeReport, DeviceStateSnapshot, MassStorageError,
    MountedMtpBackupProgress, MountedMtpDevice, MountedMtpDeviceAdapter, MountedMtpError,
    MountedMtpUploadProgress, MtpError, SafeRelativePath,
};

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
    async fn inventory(&self, paths: &[SafeRelativePath])
    -> Result<DeviceInventory, DeviceIoError>;
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError>;
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(DevicePathState, Option<u64>), DeviceIoError>;
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
}

/// Mutation authority granted only to the transaction destination.
#[async_trait::async_trait]
pub trait DeviceWrite: DeviceRead {
    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError>;
    /// Delete an exact path after size validation without reading its contents.
    ///
    /// This is reserved for explicitly unprotected transactions where no
    /// recovery backup or content digest is available.
    async fn delete_unverified(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        Err(DeviceIoError::Transport(format!(
            "unverified deletion is unsupported for {storage}:{path} ({size} bytes)"
        )))
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
}

#[async_trait::async_trait]
impl DeviceRead for MountedMtpDevice {
    fn execution_target(&self) -> Option<&str> {
        None
    }
    async fn state(&self) -> Result<crate::DeviceStateSnapshot, DeviceIoError> {
        Ok(self.state_snapshot().await?)
    }
    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, DeviceIoError> {
        Ok(MountedMtpDeviceAdapter::inventory(self, paths).await?)
    }
    async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
        Ok(MountedMtpDeviceAdapter::primary_storage_id(self).await?)
    }
    async fn inspect(
        &self,
        storage: &str,
        path: &SafeRelativePath,
    ) -> Result<(DevicePathState, Option<u64>), DeviceIoError> {
        Ok(MountedMtpDeviceAdapter::inspect(self, storage, path).await?)
    }
    async fn backup(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, DeviceIoError> {
        Ok(
            MountedMtpDeviceAdapter::backup(self, storage, path, size, destination, progress)
                .await?,
        )
    }
    async fn verify(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        Ok(MountedMtpDeviceAdapter::verify(self, storage, path, size, sha256).await?)
    }
}

#[async_trait::async_trait]
impl DeviceWrite for MountedMtpDevice {
    async fn delete(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        Ok(MountedMtpDeviceAdapter::delete(self, storage, path, size, sha256).await?)
    }
    async fn delete_unverified(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
    ) -> Result<(), DeviceIoError> {
        Ok(MountedMtpDeviceAdapter::delete_unverified(self, storage, path, size).await?)
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
        Ok(
            MountedMtpDeviceAdapter::upload(self, storage, path, source, size, sha256, progress)
                .await?,
        )
    }
    async fn restore(
        &self,
        storage: &str,
        path: &SafeRelativePath,
        size: u64,
        backup: &Path,
        sha256: &str,
    ) -> Result<(), DeviceIoError> {
        Ok(MountedMtpDeviceAdapter::restore(self, storage, path, size, backup, sha256).await?)
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
        Ok(MountedMtpDeviceAdapter::probe(self, &request.source, progress).await?)
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
    #[error("device file failed size or checksum verification: {0}")]
    Verification(String),
    #[error("refusing to overwrite device file: {0}")]
    Occupied(String),
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
        if error.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Transport(error.to_string())
        }
    }
}

impl From<mtp_rs::Error> for DeviceIoError {
    fn from(error: mtp_rs::Error) -> Self {
        Self::Transport(error.to_string())
    }
}
