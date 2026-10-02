use std::path::{Path, PathBuf};

use garmin_device::attachments::{self, Candidate as _};
use garmin_fixtures::device::{self as fixture, Device};
use garmin_progress::ProgressReporter;
use garmin_service_api::{
    DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload, DeviceCatalogSnapshot,
    DeviceSnapshot,
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
    inspections: attachments::Manager<Device>,
}

impl DemoSource {
    pub(crate) fn new(
        root: PathBuf,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, fixture::DeviceError> {
        let device = Device::open(root)?;
        let handle = runtime.clone();
        let inspections = attachments::Manager::with_inspector(device.clone(), move |candidate| {
            Ok(
                handle.block_on(garmin_services::devices::inspect_attachment(
                    &candidate.transport(),
                    candidate.name(),
                    candidate.inspect(),
                )),
            )
        });
        Ok(Self {
            device,
            runtime,
            inspections,
        })
    }

    fn native_catalog(&self) -> Result<garmin_device::DeviceCatalog, String> {
        self.device.catalog()
    }
}

impl Source for DemoSource {
    fn map_connector(
        &mut self,
        key: &str,
    ) -> Result<std::sync::Arc<dyn garmin_services::maps::device::Connector>, String> {
        if key != fixture::KEY
            || self.device.presence().map_err(|error| error.to_string())?
                != fixture::Presence::Present
        {
            return Err("the selected device is no longer connected".to_owned());
        }
        Ok(std::sync::Arc::new(
            garmin_services::maps::device::DirectoryConnector {
                root: self.device.root().to_owned(),
                storage_id: fixture::STORAGE_ID.to_owned(),
                storage_label: fixture::STORAGE_LABEL.to_owned(),
            },
        ))
    }

    fn refresh_device(&mut self, key: &str) -> Result<(), String> {
        self.inspections.refresh(key)
    }

    fn snapshot(&mut self) -> Vec<DeviceSnapshot> {
        let _events = self.inspections.poll();
        self.inspections
            .presentations()
            .into_iter()
            .map(super::device_snapshot)
            .collect()
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
        self.inspections.invalidate(DEVICE_KEY);
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
        self.inspections.invalidate(DEVICE_KEY);
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
        let snapshots = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let snapshots = source.snapshot();
                if snapshots
                    .first()
                    .is_some_and(|snapshot| snapshot.inspection != InspectionState::Running)
                {
                    break snapshots;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].inspection, InspectionState::Ready);
    }
}
