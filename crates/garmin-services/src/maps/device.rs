//! Host-selected device capabilities.
//! Clients never supply a transport or filesystem path.

use anyhow::Result;
use async_trait::async_trait;
use garmin_device::{DeviceManifest, storage::DeviceWrite};

pub struct Connection {
    pub manifest: DeviceManifest,
    pub device: Box<dyn DeviceWrite>,
}

/// Resolves an attachment again before inspection,
/// planning, approval, and recovery.
///
/// Implementations select an existing transport;
/// transaction behavior stays in this crate.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self) -> Result<Connection>;
}

/// Mounted device selected by the host's attachment discovery.
pub struct MountedConnector(pub String);

#[async_trait]
impl Connector for MountedConnector {
    async fn connect(&self) -> Result<Connection> {
        Ok(Connection {
            manifest: garmin_device::MountedMtpDevice::new(&self.0).open().await?,
            device: Box::new(garmin_device::MountedMtpDevice::new(&self.0)),
        })
    }
}

/// Directory transport selected by the host, including persistent demo devices.
pub struct DirectoryConnector {
    pub root: std::path::PathBuf,
    pub storage_id: String,
    pub storage_label: String,
}

#[async_trait]
impl Connector for DirectoryConnector {
    async fn connect(&self) -> Result<Connection> {
        Ok(Connection {
            manifest: garmin_device::open_mass_storage(&self.root).await?,
            device: Box::new(garmin_device::storage::DirectoryDevice::with_storage(
                self.root.clone(),
                self.storage_id.clone(),
                self.storage_label.clone(),
            )),
        })
    }
}
