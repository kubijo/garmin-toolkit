//! Host-selected adapters for desktop-owned Garmin device access.

use std::path::Path;

use garmin_progress::{OperationStage, ProgressReporter};

use crate::storage::BackupDestination;
use crate::{
    DeviceInventory, DeviceManifest, DevicePathState, DeviceProbeReport, SafeRelativePath,
};

#[cfg(target_os = "linux")]
#[path = "mounted_catalog.rs"]
mod mounted_catalog;

#[cfg(target_os = "linux")]
#[path = "mounted_mtp.rs"]
mod mounted_mtp;

#[cfg(target_os = "linux")]
#[path = "sysfs.rs"]
mod sysfs;

#[cfg(target_os = "linux")]
mod implementation {
    pub use super::mounted_catalog::{MountedMtpCandidate, MountedMtpMonitor};
    pub use super::mounted_mtp::{
        MountedMtpError, backup_mounted_mtp_object, delete_mounted_mtp_object,
        delete_unverified_mounted_mtp_object, discover_mounted_mtp, inspect_mounted_mtp_object,
        inventory_mounted_mtp, mounted_device_state, open_mounted_mtp,
        primary_mounted_mtp_storage_id, probe_mounted_mtp_file, restore_mounted_mtp_object,
        upload_mounted_mtp_object, verify_mounted_mtp_object,
    };
    pub use super::sysfs::{GarminUsbDevice, discover_garmin_usb_sysfs};
}

#[cfg(not(target_os = "linux"))]
#[path = "system_unsupported.rs"]
mod implementation;

#[cfg(test)]
#[expect(
    dead_code,
    reason = "compile-check the inactive system adapter on the Linux test host"
)]
#[path = "system_unsupported.rs"]
mod unsupported_compile;

pub use implementation::{
    GarminUsbDevice, MountedMtpCandidate, MountedMtpError, MountedMtpMonitor,
};

/// Progress context for one device-to-capture backup.
#[derive(Debug, Clone)]
pub struct MountedMtpBackupProgress {
    pub reporter: ProgressReporter,
    pub completed_before: u64,
    pub total: u64,
}

impl MountedMtpBackupProgress {
    pub fn advanced(&self, storage_id: &str, path: &SafeRelativePath, completed: u64, total: u64) {
        let display_path = path.to_string();
        self.reporter.advanced_with_path(
            OperationStage::Backup,
            "Backing up selected device file",
            &display_path,
            self.completed_before.saturating_add(completed),
            Some(self.total),
        );
        self.reporter
            .for_item(format!("backup:{storage_id}:{display_path}"))
            .advanced_with_path(
                OperationStage::Backup,
                "Backing up device file",
                display_path,
                completed,
                Some(total),
            );
    }

    pub fn completed(&self, storage_id: &str, path: &SafeRelativePath, total: u64) {
        let display_path = path.to_string();
        self.reporter
            .for_item(format!("backup:{storage_id}:{display_path}"))
            .completed_with_path(
                OperationStage::Backup,
                "Device file backed up",
                display_path,
                total,
                Some(total),
            );
    }
}

/// Progress context for one capture-to-device upload.
#[derive(Debug, Clone)]
pub struct MountedMtpUploadProgress {
    pub reporter: ProgressReporter,
    pub completed_before: u64,
    pub total: u64,
}

/// Structured details for a disposable mounted-MTP probe failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountedMtpProbeFailure {
    Upload { reason: String },
    UploadCleanup { upload: String, cleanup: String },
    ReadBack { reason: String },
    Size { expected: u64, actual: u64 },
    Checksum,
    NotRemoved,
}

/// One mounted-MTP attachment selected by its opaque host identifier.
#[derive(Debug, Clone)]
pub struct MountedMtpDevice {
    mount_id: String,
}

impl MountedMtpDevice {
    /// Read capacity through the system-owned mount.
    /// # Errors
    /// The mount disappeared or its storage listing failed.
    pub async fn state_snapshot(&self) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
        implementation::mounted_device_state(&self.mount_id).await
    }

    #[must_use]
    pub fn new(mount_id: impl Into<String>) -> Self {
        Self {
            mount_id: mount_id.into(),
        }
    }

    #[must_use]
    pub fn mount_id(&self) -> &str {
        &self.mount_id
    }
}

/// Platform-neutral operations supplied by the host mounted-MTP adapter.
#[expect(
    async_fn_in_trait,
    reason = "the adapter is an internal generic contract and is not object-safe"
)]
pub trait MountedMtpDeviceAdapter {
    async fn open(&self) -> Result<DeviceManifest, MountedMtpError>;

    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, MountedMtpError>;

    async fn primary_storage_id(&self) -> Result<String, MountedMtpError>;

    async fn inspect(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
    ) -> Result<(DevicePathState, Option<u64>), MountedMtpError>;

    async fn backup(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, MountedMtpError>;

    async fn delete(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<(), MountedMtpError>;

    async fn delete_unverified(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
    ) -> Result<(), MountedMtpError>;

    async fn verify(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<(), MountedMtpError>;

    async fn restore(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        backup: &Path,
        expected_sha256: &str,
    ) -> Result<(), MountedMtpError>;

    async fn upload(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        source: &Path,
        expected_size: u64,
        expected_sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> Result<(), MountedMtpError>;

    async fn probe(
        &self,
        source: &Path,
        progress: &ProgressReporter,
    ) -> Result<DeviceProbeReport, MountedMtpError>;
}

impl MountedMtpDeviceAdapter for MountedMtpDevice {
    async fn open(&self) -> Result<DeviceManifest, MountedMtpError> {
        implementation::open_mounted_mtp(&self.mount_id).await
    }

    async fn inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, MountedMtpError> {
        implementation::inventory_mounted_mtp(&self.mount_id, paths).await
    }

    async fn primary_storage_id(&self) -> Result<String, MountedMtpError> {
        implementation::primary_mounted_mtp_storage_id(&self.mount_id).await
    }

    async fn inspect(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
    ) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
        implementation::inspect_mounted_mtp_object(&self.mount_id, storage_id, path).await
    }

    async fn backup(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        destination: BackupDestination,
        progress: MountedMtpBackupProgress,
    ) -> Result<String, MountedMtpError> {
        implementation::backup_mounted_mtp_object(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            destination,
            progress,
        )
        .await
    }

    async fn delete(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<(), MountedMtpError> {
        implementation::delete_mounted_mtp_object(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            expected_sha256,
        )
        .await
    }

    async fn delete_unverified(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
    ) -> Result<(), MountedMtpError> {
        implementation::delete_unverified_mounted_mtp_object(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
        )
        .await
    }

    async fn verify(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        expected_sha256: &str,
    ) -> Result<(), MountedMtpError> {
        implementation::verify_mounted_mtp_object(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            expected_sha256,
        )
        .await
    }

    async fn restore(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        backup: &Path,
        expected_sha256: &str,
    ) -> Result<(), MountedMtpError> {
        implementation::restore_mounted_mtp_object(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            backup,
            expected_sha256,
        )
        .await
    }

    async fn upload(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        source: &Path,
        expected_size: u64,
        expected_sha256: &str,
        progress: MountedMtpUploadProgress,
    ) -> Result<(), MountedMtpError> {
        implementation::upload_mounted_mtp_object(
            &self.mount_id,
            storage_id,
            path,
            source,
            expected_size,
            expected_sha256,
            progress,
        )
        .await
    }

    async fn probe(
        &self,
        source: &Path,
        progress: &ProgressReporter,
    ) -> Result<DeviceProbeReport, MountedMtpError> {
        implementation::probe_mounted_mtp_file(&self.mount_id, source, progress).await
    }
}

#[must_use]
pub fn discover_mounted_mtp() -> Vec<MountedMtpCandidate> {
    implementation::discover_mounted_mtp()
}

#[must_use]
pub fn discover_garmin_usb_sysfs() -> Vec<GarminUsbDevice> {
    implementation::discover_garmin_usb_sysfs()
}
