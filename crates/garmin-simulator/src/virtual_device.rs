//! Explicit virtual registration; never broadens hardware discovery.

use std::{path::PathBuf, time::Duration};

use mtp_rs::{
    VirtualDeviceConfig, VirtualStorageConfig, register_virtual_device, unregister_virtual_device,
};

use garmin_device::storage::{DeviceIoError, MtpStorageDevice};

pub struct VirtualVolume {
    pub key: String,
    pub label: String,
    pub directory: PathBuf,
    pub capacity: u64,
    pub read_only: bool,
}

/// Registration lifetime only; dropping it never deletes backing files.
pub struct VirtualDevice {
    location: u64,
    device: MtpStorageDevice,
}

impl VirtualDevice {
    /// Register retained backing directories without accessing USB.
    ///
    /// # Errors
    /// Invalid virtual configuration.
    pub fn register(
        id: String,
        primary: String,
        volumes: Vec<VirtualVolume>,
    ) -> Result<Self, DeviceIoError> {
        let keys = volumes
            .iter()
            .map(|volume| volume.key.clone())
            .collect::<Vec<_>>();
        if !keys.contains(&primary)
            || keys.iter().collect::<std::collections::BTreeSet<_>>().len() != keys.len()
        {
            return Err(DeviceIoError::Storage(primary));
        }
        let location = register_virtual_device(&VirtualDeviceConfig {
            serial: id.clone(),
            storages: volumes
                .into_iter()
                .map(|volume| VirtualStorageConfig {
                    description: volume.label,
                    capacity: volume.capacity,
                    backing_dir: volume.directory,
                    read_only: volume.read_only,
                })
                .collect(),
            event_poll_interval: Duration::ZERO,
            watch_backing_dirs: false,
            ..Default::default()
        })
        .location_id;
        Ok(Self {
            location,
            device: MtpStorageDevice::new(location, Some(id), keys, primary),
        })
    }

    #[must_use]
    pub const fn device(&self) -> &MtpStorageDevice {
        &self.device
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        unregister_virtual_device(self.location);
    }
}
