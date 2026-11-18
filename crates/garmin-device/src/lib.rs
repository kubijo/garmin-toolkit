//! Garmin device discovery and transport adapters.

pub mod attachments;
mod browser;
mod domain;
pub mod manifest;
mod mass_storage;
mod mtp;
mod raw_catalog;
pub mod read_mounted;
pub mod read_mtp;
mod state;
pub mod storage;
mod system;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
pub(crate) mod test_support;

pub use browser::{
    BrowserFileUpload, DeviceBrowserOperationError, DeviceBrowserTarget, DeviceCatalog,
    DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogStorage, MAX_BROWSER_TRANSFER_BYTES,
    PreparedDeviceBrowserDownload, create_browser_directory,
    create_browser_directory_with_progress, prepare_browser_download, remove_browser_target,
    remove_browser_target_with_progress, remove_browser_targets_with_progress,
    rename_browser_target_with_progress, replace_browser_file_with_progress,
    transfer_browser_targets_with_progress, upload_browser_file_from_path,
};
pub use domain::{
    DeviceInventory, DeviceManifest, DevicePathInspection, DevicePathState, DevicePathStatus,
    DeviceSummary, PathSafetyError, SafeRelativePath, TransportKind,
};
pub use manifest::{
    DataType, DeviceId, DeviceModel, FileCapability, ManifestError, ManifestFormat, ManifestQuirk,
    OperationHandle, ParsedManifest, SoftwareVersion, TransferDirection, parse_manifest,
};
pub use mass_storage::{
    MassStorageError, discover_mass_storage, inventory_mass_storage, open_mass_storage,
    probe_mass_storage_file,
};
pub use mtp::{
    GARMIN_USB_VENDOR_ID, KNOWN_GARMIN_MTP, MtpCandidate, MtpError, ProbeRecoveryReason,
    RawMtpLink, RawMtpSession, discover_mtp_candidates, inventory_mtp,
    mtp_usb_reset_known_ineffective, open_mtp, reset_mtp_transport,
};
pub use raw_catalog::{RawMtpCandidate, RawMtpMonitor};
pub use state::{DeviceStateSnapshot, DeviceStorageState, StorageCapacity, filesystem_capacity};
pub use storage::{BackupDestination, DeviceDirectoryEntry};
pub use system::{
    GarminUsbDevice, MountedMtpBackupProgress, MountedMtpCandidate, MountedMtpDevice,
    MountedMtpError, MountedMtpMonitor, MountedMtpProbeFailure, MountedMtpUploadProgress,
    MountedMtpVerifyProgress, discover_garmin_usb_sysfs, discover_mounted_mtp,
};

use std::time::Duration;

use garmin_model::artifact::{ByteCount, ByteRate};
use garmin_model::value::duration_seconds;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadCompletion {
    Acknowledged,
    ReconciledAfterTimeout,
}

/// Discover readable Garmin devices and collapse duplicate transport views.
///
/// Desktop-owned MTP sessions take precedence over raw USB access. Devices
/// exposed through more than one readable transport are matched by identity.
#[must_use]
pub async fn discover_readable_devices() -> Vec<DeviceManifest> {
    let mut devices = discover_mass_storage().await;
    let mounted_mtp = discover_mounted_mtp();
    let raw_mtp_available = mounted_mtp.is_empty();
    for candidate in mounted_mtp {
        match MountedMtpDevice::new(candidate.mount_id.clone())
            .open()
            .await
        {
            Ok(device) => push_distinct_device(&mut devices, device),
            Err(error) => tracing::debug!(
                mount = %candidate.mount_id,
                error = %error,
                "desktop-mounted MTP device could not be inspected"
            ),
        }
    }
    if raw_mtp_available {
        for candidate in discover_mtp_candidates() {
            match open_mtp(candidate.location_id).await {
                Ok(device) => push_distinct_device(&mut devices, device),
                Err(error) => tracing::debug!(
                    location = candidate.location_id,
                    error = %error,
                    "raw MTP device could not be inspected"
                ),
            }
        }
    }
    devices
}

fn push_distinct_device(devices: &mut Vec<DeviceManifest>, candidate: DeviceManifest) {
    let identity = candidate.identity_digest();
    if devices
        .iter()
        .all(|device| device.identity_digest() != identity)
    {
        devices.push(candidate);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceProbeReport {
    pub transport: TransportKind,
    pub bytes: ByteCount,
    #[serde(
        rename = "upload_seconds",
        serialize_with = "duration_seconds::serialize"
    )]
    pub upload_elapsed: Duration,
    #[serde(
        rename = "device_finalize_seconds",
        serialize_with = "duration_seconds::serialize"
    )]
    pub device_finalize_elapsed: Duration,
    pub upload_completion: UploadCompletion,
    #[serde(
        rename = "read_back_seconds",
        serialize_with = "duration_seconds::serialize"
    )]
    pub read_back_elapsed: Duration,
    #[serde(
        rename = "cleanup_seconds",
        serialize_with = "duration_seconds::serialize"
    )]
    pub cleanup_elapsed: Duration,
    pub content_verified: bool,
    pub temporary_file_removed: bool,
}

impl DeviceProbeReport {
    #[must_use]
    pub fn upload_rate(&self) -> ByteRate {
        ByteRate::from_transfer(self.bytes, self.upload_elapsed)
    }

    #[must_use]
    pub fn read_back_rate(&self) -> ByteRate {
        ByteRate::from_transfer(self.bytes, self.read_back_elapsed)
    }
}
