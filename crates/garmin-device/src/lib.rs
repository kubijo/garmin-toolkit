//! Garmin device discovery and transport adapters.

pub mod attachments;
pub mod capabilities;
mod domain;
mod manifest;
mod mass_storage;
mod mtp;
pub mod read_mounted;
pub mod read_mtp;
mod state;
pub mod storage;
mod system;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
pub(crate) mod test_support;

pub use domain::{
    DeviceInventory, DeviceManifest, DevicePathInspection, DevicePathState, DevicePathStatus,
    DeviceSummary, PathSafetyError, SafeRelativePath, TransportKind,
};
pub use manifest::{ManifestError, parse_manifest};
pub use mass_storage::{
    MassStorageError, discover_mass_storage, inventory_mass_storage, open_mass_storage,
    probe_mass_storage_file,
};
pub use mtp::{
    GARMIN_USB_VENDOR_ID, KNOWN_GARMIN_MTP, MtpCandidate, MtpError, ProbeRecoveryReason,
    RawMtpLink, RawMtpSession, discover_mtp_candidates, inventory_mtp,
    mtp_usb_reset_known_ineffective, open_mtp, reset_mtp_transport,
};
pub use state::{DeviceStateSnapshot, DeviceStorageState, StorageCapacity, filesystem_capacity};
pub use storage::{BackupDestination, DeviceDirectoryEntry};
pub use system::{
    GarminUsbDevice, MountedMtpBackupProgress, MountedMtpCandidate, MountedMtpDevice,
    MountedMtpError, MountedMtpMonitor, MountedMtpProbeFailure, MountedMtpUploadProgress,
    MountedMtpVerifyProgress, discover_garmin_usb_sysfs, discover_mounted_mtp,
};

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
    pub bytes: u64,
    pub upload_seconds: f64,
    pub device_finalize_seconds: f64,
    pub upload_completion: UploadCompletion,
    pub read_back_seconds: f64,
    pub cleanup_seconds: f64,
    pub content_verified: bool,
    pub temporary_file_removed: bool,
}

impl DeviceProbeReport {
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "an approximate transfer rate is intentionally represented as f64"
    )]
    pub fn upload_bytes_per_second(&self) -> f64 {
        if self.upload_seconds == 0.0 {
            0.0
        } else {
            self.bytes as f64 / self.upload_seconds
        }
    }

    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "an approximate transfer rate is intentionally represented as f64"
    )]
    pub fn read_back_bytes_per_second(&self) -> f64 {
        if self.read_back_seconds == 0.0 {
            0.0
        } else {
            self.bytes as f64 / self.read_back_seconds
        }
    }
}
