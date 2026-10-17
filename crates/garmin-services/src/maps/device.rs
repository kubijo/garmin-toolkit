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

/// Raw MTP device selected by a host that does not use a desktop mount session.
pub struct RawMtpConnector(pub u64);

#[async_trait]
impl Connector for RawMtpConnector {
    async fn connect(&self) -> Result<Connection> {
        Ok(Connection {
            manifest: garmin_device::open_mtp(self.0).await?,
            device: Box::new(garmin_device::storage::MtpStorageDevice::discover(self.0).await?),
        })
    }
}

/// Directory transport selected by the host, including persistent demo devices.
pub struct DirectoryConnector {
    pub root: std::path::PathBuf,
    pub storage_id: String,
    pub storage_label: String,
}

impl DirectoryConnector {
    /// Return an opt-in connector whose directory writes and checks can be observed by a demo UI.
    #[must_use]
    pub fn with_transfer_delays(
        self,
        upload: std::time::Duration,
        verify: std::time::Duration,
    ) -> DelayedDirectoryConnector {
        DelayedDirectoryConnector {
            inner: self,
            upload,
            verify,
            remove_uploaded_before_verify: false,
            truncate_uploaded_before_verify: false,
        }
    }
}

pub struct DelayedDirectoryConnector {
    inner: DirectoryConnector,
    upload: std::time::Duration,
    verify: std::time::Duration,
    remove_uploaded_before_verify: bool,
    truncate_uploaded_before_verify: bool,
}

impl DelayedDirectoryConnector {
    /// Inject a missing just-uploaded file at verification in a disposable demo device.
    #[must_use]
    pub fn with_missing_uploaded_file_on_verify(mut self, enabled: bool) -> Self {
        self.remove_uploaded_before_verify = enabled;
        self
    }

    /// Inject a partial just-uploaded file at verification in a disposable demo device.
    #[must_use]
    pub fn with_partial_uploaded_file_on_verify(mut self, enabled: bool) -> Self {
        self.truncate_uploaded_before_verify = enabled;
        self
    }
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

#[async_trait]
impl Connector for DelayedDirectoryConnector {
    async fn connect(&self) -> Result<Connection> {
        Ok(Connection {
            manifest: garmin_device::open_mass_storage(&self.inner.root).await?,
            device: Box::new(
                garmin_device::storage::DirectoryDevice::with_storage(
                    self.inner.root.clone(),
                    self.inner.storage_id.clone(),
                    self.inner.storage_label.clone(),
                )
                .with_transfer_delays(self.upload, self.verify)
                .with_missing_uploaded_file_on_verify(self.remove_uploaded_before_verify)
                .with_partial_uploaded_file_on_verify(self.truncate_uploaded_before_verify),
            ),
        })
    }
}
