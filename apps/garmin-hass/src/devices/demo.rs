use std::path::{Path, PathBuf};

use garmin_fixtures::device::{self as fixture, Device, Presence};
use garmin_progress::ProgressReporter;
use garmin_service_api::{
    DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload, DeviceCapability,
    DeviceCatalogSnapshot, DeviceDataType, DeviceSnapshot, InspectionState, TransferDirection,
};

use super::{
    Source, SourceOpenError, SourceProvider, browser_device_key, device_catalog,
    execute_browser_download, execute_browser_operation, execute_browser_upload,
};

pub(super) const DEVICE_KEY: &str = fixture::KEY;
#[cfg(test)]
pub(super) const STORAGE_ID: &str = fixture::STORAGE_ID;

pub(crate) struct Provider;

impl SourceProvider for Provider {
    fn open(
        self,
        data_root: &Path,
        runtime: tokio::runtime::Handle,
    ) -> Result<Box<dyn Source>, SourceOpenError> {
        DemoSource::new(data_root.join("device"), runtime)
            .map(|source| Box::new(source) as Box<dyn Source>)
            .map_err(|error| Box::new(error) as SourceOpenError)
    }
}

pub(crate) struct DemoSource {
    device: Device,
    runtime: tokio::runtime::Handle,
}

impl DemoSource {
    pub(crate) fn new(
        root: PathBuf,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, fixture::DeviceError> {
        Ok(Self {
            device: Device::recreate(root)?,
            runtime,
        })
    }

    fn native_catalog(&self) -> Result<garmin_device::DeviceCatalog, String> {
        self.device.catalog()
    }
}

impl Source for DemoSource {
    fn snapshot(&mut self) -> Vec<DeviceSnapshot> {
        match self.device.presence() {
            Ok(Presence::Missing) => Vec::new(),
            Ok(Presence::Present) => vec![device_snapshot(InspectionState::Ready, None)],
            Err(error) => vec![device_snapshot(
                InspectionState::Failed,
                Some(error.to_string()),
            )],
        }
    }

    fn catalog(
        &mut self,
        device_key: &str,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String> {
        ensure_running(progress)?;
        ensure_device(device_key)?;
        let catalog = device_catalog(device_key.to_owned(), self.native_catalog()?);
        ensure_running(progress)?;
        Ok(catalog)
    }

    fn browser(
        &mut self,
        request: DeviceBrowserRequest,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String> {
        ensure_device(browser_device_key(&request))?;
        ensure_running(progress)?;
        let catalog = self.native_catalog()?;
        self.runtime.block_on(execute_browser_operation(
            &self.device.transport(),
            &catalog,
            request,
            progress,
        ))?;
        ensure_running(progress)?;
        self.native_catalog()
            .map(|catalog| device_catalog(DEVICE_KEY.to_owned(), catalog))
    }

    fn download(
        &mut self,
        device_key: &str,
        target: DeviceBrowserTarget,
        progress: &ProgressReporter,
    ) -> Result<garmin_device::PreparedDeviceBrowserDownload, String> {
        ensure_device(device_key)?;
        ensure_running(progress)?;
        let catalog = self.native_catalog()?;
        self.runtime.block_on(execute_browser_download(
            &self.device.transport(),
            &catalog,
            target,
            progress,
        ))
    }

    fn upload(
        &mut self,
        request: DeviceBrowserUpload,
        contents: tempfile::TempPath,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String> {
        ensure_device(&request.device_key)?;
        ensure_running(progress)?;
        let catalog = self.native_catalog()?;
        self.runtime.block_on(execute_browser_upload(
            &self.device.transport(),
            &catalog,
            &request,
            contents.as_ref(),
            progress,
        ))?;
        ensure_running(progress)?;
        self.native_catalog()
            .map(|catalog| device_catalog(DEVICE_KEY.to_owned(), catalog))
    }
}

fn ensure_running(progress: &ProgressReporter) -> Result<(), String> {
    if progress.is_cancelled() {
        Err("device browser operation was cancelled".to_owned())
    } else {
        Ok(())
    }
}

fn ensure_device(device_key: &str) -> Result<(), String> {
    if device_key == DEVICE_KEY {
        Ok(())
    } else {
        Err("the selected mock device is unavailable".to_owned())
    }
}

fn device_snapshot(
    inspection: InspectionState,
    inspection_error: Option<String>,
) -> DeviceSnapshot {
    let ready = inspection == InspectionState::Ready;
    let metadata = ready.then(fixture::metadata);
    let (identifier, software_version, capabilities, storages) = if let Some(metadata) = metadata {
        (
            Some(metadata.id.into_u32()),
            Some(metadata.software_version.into_hundredths()),
            metadata
                .capabilities
                .into_iter()
                .filter_map(service_capability)
                .collect(),
            metadata.storage.storages,
        )
    } else {
        (None, None, Vec::new(), Vec::new())
    };
    DeviceSnapshot {
        key: DEVICE_KEY.to_owned(),
        name: fixture::NAME.to_owned(),
        identifier,
        software_version,
        inspection,
        inspection_error,
        capabilities,
        storages,
    }
}

fn service_capability(
    capability: garmin_device::attachments::Capability,
) -> Option<DeviceCapability> {
    let data_type = match capability.data_type() {
        garmin_device::DataType::Activity => DeviceDataType::Activity,
        garmin_device::DataType::Workout => DeviceDataType::Workout,
        garmin_device::DataType::Course => DeviceDataType::Course,
        _ => return None,
    };
    Some(DeviceCapability {
        data_type,
        direction: match capability.direction() {
            garmin_device::TransferDirection::OutputFromUnit => TransferDirection::OutputFromUnit,
            garmin_device::TransferDirection::InputToUnit => TransferDirection::InputToUnit,
            garmin_device::TransferDirection::InputOutput => TransferDirection::InputOutput,
        },
    })
}

#[cfg(test)]
mod tests {
    use garmin_service_api::InspectionState;

    use super::{DemoSource, Source};

    #[tokio::test]
    async fn snapshot_tracks_detach_and_reconnect_of_the_backing_device() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("device");
        let detached = directory.path().join("detached-device");
        let mut source = DemoSource::new(root.clone(), tokio::runtime::Handle::current()).unwrap();
        assert_eq!(source.snapshot().len(), 1);

        std::fs::rename(&root, &detached).unwrap();
        assert!(source.snapshot().is_empty());

        std::fs::rename(&detached, &root).unwrap();
        let snapshots = source.snapshot();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].inspection, InspectionState::Ready);
    }
}
