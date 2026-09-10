//! Fallback host adapter for systems without desktop-mounted MTP integration.

use std::path::{Path, PathBuf};

use garmin_progress::ProgressReporter;
use serde::Serialize;
use thiserror::Error;

use crate::attachments;
use crate::storage::BackupDestination;
use crate::system::{MountedMtpBackupProgress, MountedMtpUploadProgress, MountedMtpVerifyProgress};
use crate::{
    DeviceInventory, DeviceManifest, DevicePathState, DeviceProbeReport, SafeRelativePath,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MountedMtpCandidate {
    pub mount_id: String,
    pub name: String,
}

impl attachments::Candidate for MountedMtpCandidate {
    fn key(&self) -> &str {
        &self.mount_id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn inspect(&self) -> Result<attachments::Metadata, String> {
        Err(MountedMtpError::Unsupported.to_string())
    }
}

#[derive(Debug, Default)]
pub struct MountedMtpMonitor;

impl MountedMtpMonitor {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl attachments::Backend for MountedMtpMonitor {
    type Candidate = MountedMtpCandidate;

    fn poll_changed(&self) -> bool {
        false
    }

    fn candidates(&self) -> Vec<Self::Candidate> {
        Vec::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GarminUsbDevice {
    pub sysfs_path: PathBuf,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: Option<String>,
}

#[derive(Debug, Error)]
pub enum MountedMtpError {
    #[error("desktop-mounted MTP is not supported on this operating system")]
    Unsupported,
}

impl MountedMtpError {
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        let Self::Unsupported = self;
        false
    }

    #[must_use]
    pub const fn probe_failure(&self) -> Option<crate::system::MountedMtpProbeFailure> {
        let Self::Unsupported = self;
        None
    }
}

async fn unsupported<T>() -> Result<T, MountedMtpError> {
    std::future::ready(Err(MountedMtpError::Unsupported)).await
}

#[must_use]
pub fn discover_mounted_mtp() -> Vec<MountedMtpCandidate> {
    Vec::new()
}

#[must_use]
pub fn discover_garmin_usb_sysfs() -> Vec<GarminUsbDevice> {
    Vec::new()
}

pub async fn open_mounted_mtp(_mount_id: &str) -> Result<DeviceManifest, MountedMtpError> {
    unsupported().await
}

pub async fn mounted_device_state(
    _mount_id: &str,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    unsupported().await
}

pub async fn mounted_device_state_with_progress(
    _mount_id: &str,
    _progress: &ProgressReporter,
) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
    unsupported().await
}

pub async fn inventory_mounted_mtp(
    _mount_id: &str,
    _paths: &[SafeRelativePath],
) -> Result<DeviceInventory, MountedMtpError> {
    unsupported().await
}

pub async fn primary_mounted_mtp_storage_id(_mount_id: &str) -> Result<String, MountedMtpError> {
    unsupported().await
}

pub async fn inspect_mounted_mtp_object(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
    unsupported().await
}

pub async fn inspect_mounted_mtp_object_with_progress(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _progress: &ProgressReporter,
) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
    unsupported().await
}

pub async fn read_bounded_mounted_mtp_file(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _limit: u64,
) -> Result<Option<Vec<u8>>, MountedMtpError> {
    unsupported().await
}

pub async fn list_mounted_mtp_directory(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
) -> Result<Vec<crate::storage::DeviceDirectoryEntry>, MountedMtpError> {
    unsupported().await
}

pub async fn ensure_mounted_mtp_directory(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn create_verified_mounted_mtp_file(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _bytes: &[u8],
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn remove_empty_mounted_mtp_directory(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn backup_mounted_mtp_object(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _destination: BackupDestination,
    _progress: MountedMtpBackupProgress,
) -> Result<String, MountedMtpError> {
    unsupported().await
}

pub async fn delete_mounted_mtp_object(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn delete_mounted_mtp_object_with_progress(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _expected_sha256: &str,
    _progress: &ProgressReporter,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn delete_size_checked_mounted_mtp_object(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn delete_size_checked_mounted_mtp_object_with_progress(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _progress: &ProgressReporter,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn verify_mounted_mtp_object(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn verify_mounted_mtp_object_with_progress(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _expected_sha256: &str,
    _progress: MountedMtpVerifyProgress,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn restore_mounted_mtp_object(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _backup: &Path,
    _expected_sha256: &str,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn restore_mounted_mtp_object_with_progress(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _expected_size: u64,
    _backup: &Path,
    _expected_sha256: &str,
    _progress: &ProgressReporter,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn upload_mounted_mtp_object(
    _mount_id: &str,
    _storage_id: &str,
    _path: &SafeRelativePath,
    _source: &Path,
    _expected_size: u64,
    _expected_sha256: &str,
    _progress: MountedMtpUploadProgress,
) -> Result<(), MountedMtpError> {
    unsupported().await
}

pub async fn probe_mounted_mtp_file(
    _mount_id: &str,
    _source: &Path,
    _progress: &ProgressReporter,
) -> Result<DeviceProbeReport, MountedMtpError> {
    unsupported().await
}
