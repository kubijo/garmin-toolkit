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
        MountedMtpError, backup_mounted_mtp_object, create_verified_mounted_mtp_file,
        delete_mounted_mtp_object, delete_mounted_mtp_object_with_progress,
        delete_size_checked_mounted_mtp_object,
        delete_size_checked_mounted_mtp_object_with_progress, discover_mounted_mtp,
        ensure_mounted_mtp_directory, inspect_mounted_mtp_object,
        inspect_mounted_mtp_object_with_progress, inventory_mounted_mtp,
        list_mounted_mtp_directory, mounted_device_state, mounted_device_state_with_progress,
        open_mounted_mtp, primary_mounted_mtp_storage_id, probe_mounted_mtp_file,
        read_bounded_mounted_mtp_file, remove_empty_mounted_mtp_directory,
        restore_mounted_mtp_object, restore_mounted_mtp_object_with_progress,
        upload_mounted_mtp_object, verify_mounted_mtp_object,
        verify_mounted_mtp_object_with_progress,
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

#[derive(Debug, Clone)]
pub struct MountedMtpVerifyProgress {
    pub reporter: ProgressReporter,
    pub completed_before: u64,
    pub total: u64,
}

impl MountedMtpVerifyProgress {
    pub fn advanced(&self, path: &SafeRelativePath, completed: u64) {
        self.reporter.advanced_with_path(
            OperationStage::DeviceVerify,
            "Verifying existing device file",
            path.to_string(),
            self.completed_before.saturating_add(completed),
            Some(self.total),
        );
    }
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

    /// Read capacity with cancellation.
    /// # Errors
    /// Mount, listing, and cancellation failures are returned.
    pub async fn state_snapshot_with_progress(
        &self,
        progress: &ProgressReporter,
    ) -> Result<crate::DeviceStateSnapshot, MountedMtpError> {
        implementation::mounted_device_state_with_progress(&self.mount_id, progress).await
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

impl MountedMtpDevice {
    /// Read `GarminDevice.xml`.
    /// # Errors
    /// The mount and manifest must be valid.
    pub async fn open(&self) -> Result<DeviceManifest, MountedMtpError> {
        implementation::open_mounted_mtp(&self.mount_id).await
    }

    pub(crate) async fn backend_inventory(
        &self,
        paths: &[SafeRelativePath],
    ) -> Result<DeviceInventory, MountedMtpError> {
        implementation::inventory_mounted_mtp(&self.mount_id, paths).await
    }

    pub(crate) async fn backend_primary_storage_id(&self) -> Result<String, MountedMtpError> {
        implementation::primary_mounted_mtp_storage_id(&self.mount_id).await
    }

    pub(crate) async fn backend_inspect(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
    ) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
        implementation::inspect_mounted_mtp_object(&self.mount_id, storage_id, path).await
    }

    pub(crate) async fn backend_inspect_with_progress(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        progress: &ProgressReporter,
    ) -> Result<(DevicePathState, Option<u64>), MountedMtpError> {
        implementation::inspect_mounted_mtp_object_with_progress(
            &self.mount_id,
            storage_id,
            path,
            progress,
        )
        .await
    }

    pub(crate) async fn backend_read_bounded_file(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, MountedMtpError> {
        implementation::read_bounded_mounted_mtp_file(&self.mount_id, storage_id, path, limit).await
    }

    pub(crate) async fn backend_list_directory(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
    ) -> Result<Vec<crate::storage::DeviceDirectoryEntry>, MountedMtpError> {
        implementation::list_mounted_mtp_directory(&self.mount_id, storage_id, path).await
    }

    pub(crate) async fn backend_ensure_directory(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
    ) -> Result<(), MountedMtpError> {
        implementation::ensure_mounted_mtp_directory(&self.mount_id, storage_id, path).await
    }

    pub(crate) async fn backend_create_verified_file(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        bytes: &[u8],
    ) -> Result<(), MountedMtpError> {
        implementation::create_verified_mounted_mtp_file(&self.mount_id, storage_id, path, bytes)
            .await
    }

    pub(crate) async fn backend_remove_empty_directory(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
    ) -> Result<(), MountedMtpError> {
        implementation::remove_empty_mounted_mtp_directory(&self.mount_id, storage_id, path).await
    }

    pub(crate) async fn backend_backup(
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

    pub(crate) async fn backend_delete(
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

    pub(crate) async fn backend_delete_with_progress(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        expected_sha256: &str,
        progress: &ProgressReporter,
    ) -> Result<(), MountedMtpError> {
        implementation::delete_mounted_mtp_object_with_progress(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            expected_sha256,
            progress,
        )
        .await
    }

    pub(crate) async fn backend_delete_size_checked(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
    ) -> Result<(), MountedMtpError> {
        implementation::delete_size_checked_mounted_mtp_object(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
        )
        .await
    }

    pub(crate) async fn backend_delete_size_checked_with_progress(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        progress: &ProgressReporter,
    ) -> Result<(), MountedMtpError> {
        implementation::delete_size_checked_mounted_mtp_object_with_progress(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            progress,
        )
        .await
    }

    pub(crate) async fn backend_verify(
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

    pub(crate) async fn backend_verify_with_progress(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        expected_sha256: &str,
        progress: MountedMtpVerifyProgress,
    ) -> Result<(), MountedMtpError> {
        implementation::verify_mounted_mtp_object_with_progress(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            expected_sha256,
            progress,
        )
        .await
    }

    pub(crate) async fn backend_restore(
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

    pub(crate) async fn backend_restore_with_progress(
        &self,
        storage_id: &str,
        path: &SafeRelativePath,
        expected_size: u64,
        backup: &Path,
        expected_sha256: &str,
        progress: &ProgressReporter,
    ) -> Result<(), MountedMtpError> {
        implementation::restore_mounted_mtp_object_with_progress(
            &self.mount_id,
            storage_id,
            path,
            expected_size,
            backup,
            expected_sha256,
            progress,
        )
        .await
    }

    pub(crate) async fn backend_upload(
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

    pub(crate) async fn backend_probe(
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
