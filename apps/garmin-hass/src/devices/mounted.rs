use std::{
    marker::PhantomData,
    sync::{Weak, mpsc},
    time::Duration,
};

use garmin_device::{attachments, attachments::Candidate as _};
use garmin_progress::ProgressReporter;
use garmin_service_api::{
    DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload, DeviceCatalogSnapshot,
    DeviceSnapshot,
};
use garmin_services::maps::Operations;

use super::{
    Source, SourceOpenError, SourceProvider, browser_device_key, device_catalog,
    execute_browser_download, execute_browser_operation, execute_browser_upload,
};

pub(crate) struct Provider;

impl SourceProvider for Provider {
    fn open(
        self,
        _data_root: &std::path::Path,
        _runtime: tokio::runtime::Handle,
    ) -> Result<Box<dyn Source>, SourceOpenError> {
        Ok(Box::new(DeviceSource::<MountedAdapter>::new()))
    }
}

pub(crate) trait Adapter: Send + 'static {
    type Backend: attachments::Backend<Candidate = Self::Candidate>;
    type Candidate: attachments::Candidate;
    type Device: garmin_device::storage::DeviceWrite;

    fn backend() -> Self::Backend;
    fn inspect(
        runtime: &tokio::runtime::Handle,
        candidate: &Self::Candidate,
    ) -> Result<attachments::Metadata, String>;
    fn location(candidate: &Self::Candidate) -> String;
    fn connector(
        location: String,
    ) -> Result<std::sync::Arc<dyn garmin_services::maps::device::Connector>, String>;
    fn device(
        runtime: &tokio::runtime::Runtime,
        candidate: &Self::Candidate,
    ) -> Result<Self::Device, String>;
}

struct MountedAdapter;

impl Adapter for MountedAdapter {
    type Backend = garmin_device::MountedMtpMonitor;
    type Candidate = garmin_device::MountedMtpCandidate;
    type Device = garmin_device::MountedMtpDevice;

    fn backend() -> Self::Backend {
        garmin_device::MountedMtpMonitor::new()
    }

    fn inspect(
        runtime: &tokio::runtime::Handle,
        candidate: &Self::Candidate,
    ) -> Result<attachments::Metadata, String> {
        Ok(
            runtime.block_on(garmin_services::devices::inspect_attachment(
                &garmin_device::MountedMtpDevice::new(&candidate.mount_id),
                candidate.name(),
                candidate.inspect(),
            )),
        )
    }

    fn location(candidate: &Self::Candidate) -> String {
        candidate.mount_id.clone()
    }

    fn connector(
        location: String,
    ) -> Result<std::sync::Arc<dyn garmin_services::maps::device::Connector>, String> {
        Ok(std::sync::Arc::new(
            garmin_services::maps::device::MountedConnector(location),
        ))
    }

    fn device(
        _runtime: &tokio::runtime::Runtime,
        candidate: &Self::Candidate,
    ) -> Result<Self::Device, String> {
        Ok(garmin_device::MountedMtpDevice::new(&candidate.mount_id))
    }
}

pub(crate) struct DeviceSource<A: Adapter> {
    requests: Option<mpsc::Sender<MountedRequest>>,
    last_snapshot: Vec<DeviceSnapshot>,
    adapter: PhantomData<A>,
}

impl<A: Adapter> DeviceSource<A> {
    pub(crate) fn new() -> Self {
        let (requests, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("garmin-toolkit-hass-devices".to_owned())
            .spawn(move || device_worker::<A>(&receiver))
        {
            Ok(_worker) => Self {
                requests: Some(requests),
                last_snapshot: Vec::new(),
                adapter: PhantomData,
            },
            Err(error) => {
                tracing::error!(%error, "could not start device discovery");
                Self {
                    requests: None,
                    last_snapshot: Vec::new(),
                    adapter: PhantomData,
                }
            }
        }
    }
}

impl<A: Adapter> Source for DeviceSource<A> {
    fn observe_firmware_attachments(&mut self, operations: Weak<Operations>) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(MountedRequest::ObserveFirmware(operations));
        }
    }

    fn device_connector(
        &mut self,
        key: &str,
    ) -> Result<std::sync::Arc<dyn garmin_services::maps::device::Connector>, String> {
        let requests = self
            .requests
            .as_ref()
            .ok_or_else(|| "device discovery is unavailable".to_owned())?;
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Maps(key.to_owned(), reply))
            .map_err(|error| error.to_string())?;
        let location = response
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())??;
        A::connector(location)
    }

    fn refresh_device(&mut self, key: &str) -> Result<(), String> {
        let requests = self
            .requests
            .as_ref()
            .ok_or_else(|| "device discovery is unavailable".to_owned())?;
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Refresh(key.to_owned(), reply))
            .map_err(|error| error.to_string())?;
        response
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())?
    }

    fn invalidate_device(&mut self, key: &str) -> Result<(), String> {
        let requests = self
            .requests
            .as_ref()
            .ok_or_else(|| "device discovery is unavailable".to_owned())?;
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Invalidate(key.to_owned(), reply))
            .map_err(|error| error.to_string())?;
        response
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| error.to_string())
    }

    fn snapshot(&mut self) -> Vec<DeviceSnapshot> {
        let Some(requests) = &self.requests else {
            return Vec::new();
        };
        let (reply, response) = mpsc::channel();
        if requests.send(MountedRequest::Snapshot(reply)).is_err() {
            return Vec::new();
        }
        match response.recv_timeout(Duration::from_secs(2)) {
            Ok(snapshot) => self.last_snapshot = snapshot,
            Err(error) => tracing::warn!(%error, "device snapshot is delayed"),
        }
        self.last_snapshot.clone()
    }

    fn catalog(
        &mut self,
        device_key: &str,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String> {
        let Some(requests) = &self.requests else {
            return Err("device discovery is unavailable".to_owned());
        };
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Catalog(
                device_key.to_owned(),
                progress.clone(),
                reply,
            ))
            .map_err(|_| "device discovery stopped".to_owned())?;
        receive_with_cancellation(&response, Duration::from_secs(15), progress)
    }

    fn browser(
        &mut self,
        request: DeviceBrowserRequest,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String> {
        let Some(requests) = &self.requests else {
            return Err("device discovery is unavailable".to_owned());
        };
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Browser(request, progress.clone(), reply))
            .map_err(|_| "device discovery stopped".to_owned())?;
        receive_with_cancellation(&response, Duration::from_secs(600), progress)
    }

    fn download(
        &mut self,
        device_key: &str,
        target: DeviceBrowserTarget,
        progress: &ProgressReporter,
    ) -> Result<garmin_device::PreparedDeviceBrowserDownload, String> {
        let Some(requests) = &self.requests else {
            return Err("device discovery is unavailable".to_owned());
        };
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Download(
                device_key.to_owned(),
                target,
                progress.clone(),
                reply,
            ))
            .map_err(|_| "device discovery stopped".to_owned())?;
        receive_with_cancellation(&response, Duration::from_secs(600), progress)
    }

    fn upload(
        &mut self,
        request: DeviceBrowserUpload,
        contents: tempfile::TempPath,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String> {
        let Some(requests) = &self.requests else {
            return Err("device discovery is unavailable".to_owned());
        };
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Upload(
                request,
                contents,
                progress.clone(),
                reply,
            ))
            .map_err(|_| "device discovery stopped".to_owned())?;
        receive_with_cancellation(&response, Duration::from_secs(600), progress)
    }
}

fn receive_with_cancellation<T>(
    response: &mpsc::Receiver<Result<T, String>>,
    timeout: Duration,
    progress: &ProgressReporter,
) -> Result<T, String> {
    match response.recv_timeout(timeout) {
        Ok(result) => result,
        Err(error) => {
            progress.cancellation_token().cancel();
            Err(error.to_string())
        }
    }
}

enum MountedRequest {
    ObserveFirmware(Weak<Operations>),
    Maps(String, mpsc::Sender<Result<String, String>>),
    Snapshot(mpsc::Sender<Vec<DeviceSnapshot>>),
    Refresh(String, mpsc::Sender<Result<(), String>>),
    Invalidate(String, mpsc::Sender<()>),
    Catalog(
        String,
        ProgressReporter,
        mpsc::Sender<Result<DeviceCatalogSnapshot, String>>,
    ),
    Browser(
        DeviceBrowserRequest,
        ProgressReporter,
        mpsc::Sender<Result<DeviceCatalogSnapshot, String>>,
    ),
    Download(
        String,
        DeviceBrowserTarget,
        ProgressReporter,
        mpsc::Sender<Result<garmin_device::PreparedDeviceBrowserDownload, String>>,
    ),
    Upload(
        DeviceBrowserUpload,
        tempfile::TempPath,
        ProgressReporter,
        mpsc::Sender<Result<DeviceCatalogSnapshot, String>>,
    ),
}

fn device_worker<A: Adapter>(requests: &mpsc::Receiver<MountedRequest>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("device browser runtime must start");
    let handle = runtime.handle().clone();
    let mut manager = attachments::Manager::with_inspector(A::backend(), move |candidate| {
        A::inspect(&handle, candidate)
    });
    let mut firmware = Weak::new();
    loop {
        handle_device_events(manager.poll(), &firmware);
        match requests.recv_timeout(Duration::from_millis(250)) {
            Ok(MountedRequest::ObserveFirmware(operations)) => firmware = operations,
            Ok(MountedRequest::Snapshot(reply)) => {
                handle_device_events(manager.poll(), &firmware);
                let value = manager
                    .presentations()
                    .into_iter()
                    .map(super::device_snapshot)
                    .collect();
                let _ignored = reply.send(value);
            }
            Ok(MountedRequest::Refresh(key, reply)) => {
                let result = manager.refresh(&key);
                handle_device_events(manager.poll(), &firmware);
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Invalidate(key, reply)) => {
                manager.invalidate(&key);
                handle_device_events(manager.poll(), &firmware);
                let _ignored = reply.send(());
            }
            Ok(MountedRequest::Maps(key, reply)) => {
                handle_device_events(manager.poll(), &firmware);
                let result = manager
                    .candidate(&key)
                    .map(|candidate| A::location(&candidate))
                    .ok_or_else(|| "the selected device is no longer connected".to_owned());
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Catalog(key, progress, reply)) => {
                handle_device_events(manager.poll(), &firmware);
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| candidate.browse_with_progress(&progress))
                    .map(|catalog| device_catalog(key, catalog));
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Browser(request, progress, reply)) => {
                handle_device_events(manager.poll(), &firmware);
                let key = browser_device_key(&request).to_owned();
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| browser::<A>(&runtime, &candidate, request, &progress));
                manager.invalidate(&key);
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Download(key, target, progress, reply)) => {
                handle_device_events(manager.poll(), &firmware);
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| download::<A>(&runtime, &candidate, target, &progress));
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Upload(request, contents, progress, reply)) => {
                handle_device_events(manager.poll(), &firmware);
                let key = request.device_key.clone();
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| {
                        upload::<A>(&runtime, &candidate, &request, &contents, &progress)
                    });
                manager.invalidate(&key);
                let _ignored = reply.send(result);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn handle_device_events(events: Vec<attachments::Event>, firmware: &Weak<Operations>) {
    for event in events {
        match event {
            attachments::Event::Attached { key, name } => {
                tracing::info!(device_key = %key, device_name = %name, "device attached");
            }
            attachments::Event::Detached { key } => {
                if let Some(firmware) = firmware.upgrade() {
                    firmware.forget_firmware(&key);
                }
                tracing::info!(device_key = %key, "device detached");
            }
            attachments::Event::Inspected { key, name } => {
                tracing::info!(device_key = %key, device_name = %name, "device inspected");
            }
            attachments::Event::InspectionFailed { key, name, reason } => {
                tracing::error!(
                    device_key = %key,
                    device_name = %name,
                    error = %reason,
                    "device inspection failed"
                );
            }
        }
    }
}

#[cfg(test)]
#[tokio::test]
async fn reconnect_between_snapshots_revokes_the_review_and_checks_the_new_device() {
    use garmin_fixtures::firmware::FENIX_GSP_22_44;
    use garmin_service_api::firmware::Phase;
    use garmin_services::maps::device::{Connector, DirectoryConnector};
    use std::sync::Arc;

    let devices = tempfile::tempdir().unwrap();
    let manifest = devices.path().join("GarminDevice.xml");
    std::fs::write(&manifest, FENIX_GSP_22_44.inventory).unwrap();
    let connector: Arc<dyn Connector> = Arc::new(DirectoryConnector {
        root: devices.path().into(),
        storage_id: "internal".into(),
        storage_label: "Internal".into(),
    });
    let operations = Arc::new(Operations::default());
    let key = "port";
    let old = operations
        .open_firmware(key.into(), connector.clone(), true)
        .await
        .unwrap();
    let (revision, _) = old.check_components().await.unwrap();
    let review = old.review_selection(revision, vec![0]).unwrap();
    let previous = old.available_update().unwrap().attachment;

    handle_device_events(
        vec![
            attachments::Event::Detached { key: key.into() },
            attachments::Event::Attached {
                key: key.into(),
                name: "Reattached device".into(),
            },
        ],
        &Arc::downgrade(&operations),
    );
    assert!(operations.firmware_session(key).is_none());
    assert_eq!(old.snapshot().phase, Phase::Disconnected);
    assert!(
        old.claim_for_staging(review.revision, review.review.unwrap().approval)
            .await
            .is_err()
    );

    let new = operations
        .open_firmware(key.into(), connector, true)
        .await
        .unwrap();
    new.check_on_attachment();
    tokio::time::timeout(Duration::from_secs(5), async {
        while new.available_update().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_ne!(new.available_update().unwrap().attachment, previous);
}

fn browser<A: Adapter>(
    runtime: &tokio::runtime::Runtime,
    candidate: &A::Candidate,
    request: DeviceBrowserRequest,
    progress: &ProgressReporter,
) -> Result<DeviceCatalogSnapshot, String> {
    let device_key = browser_device_key(&request).to_owned();
    let catalog = candidate.browse_with_progress(progress)?;
    let device = A::device(runtime, candidate)?;
    runtime.block_on(execute_browser_operation(
        &device, &catalog, request, progress,
    ))?;
    candidate
        .browse_with_progress(progress)
        .map(|catalog| device_catalog(device_key, catalog))
}

fn download<A: Adapter>(
    runtime: &tokio::runtime::Runtime,
    candidate: &A::Candidate,
    target: DeviceBrowserTarget,
    progress: &ProgressReporter,
) -> Result<garmin_device::PreparedDeviceBrowserDownload, String> {
    let catalog = candidate.browse_with_progress(progress)?;
    let device = A::device(runtime, candidate)?;
    runtime.block_on(execute_browser_download(
        &device, &catalog, target, progress,
    ))
}

fn upload<A: Adapter>(
    runtime: &tokio::runtime::Runtime,
    candidate: &A::Candidate,
    request: &DeviceBrowserUpload,
    contents: &std::path::Path,
    progress: &ProgressReporter,
) -> Result<DeviceCatalogSnapshot, String> {
    let device_key = request.device_key.clone();
    let catalog = candidate.browse_with_progress(progress)?;
    let device = A::device(runtime, candidate)?;
    runtime.block_on(execute_browser_upload(
        &device, &catalog, request, contents, progress,
    ))?;
    candidate
        .browse_with_progress(progress)
        .map(|catalog| device_catalog(device_key, catalog))
}
