use std::{sync::mpsc, time::Duration};

use garmin_device::{attachments, attachments::Candidate as _};
use garmin_progress::ProgressReporter;
use garmin_service_api::{
    DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload, DeviceCapability,
    DeviceCatalogSnapshot, DeviceDataType, DeviceSnapshot, InspectionState, TransferDirection,
};

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
        Ok(Box::new(MountedSource::new()))
    }
}

pub(crate) struct MountedSource {
    requests: Option<mpsc::Sender<MountedRequest>>,
}

impl MountedSource {
    pub(crate) fn new() -> Self {
        let (requests, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("garmin-toolkit-hass-devices".to_owned())
            .spawn(move || mounted_worker(&receiver))
        {
            Ok(_worker) => Self {
                requests: Some(requests),
            },
            Err(error) => {
                tracing::error!(%error, "could not start device discovery");
                Self { requests: None }
            }
        }
    }
}

impl Source for MountedSource {
    fn snapshot(&mut self) -> Vec<DeviceSnapshot> {
        let Some(requests) = &self.requests else {
            return Vec::new();
        };
        let (reply, response) = mpsc::channel();
        if requests.send(MountedRequest::Snapshot(reply)).is_err() {
            return Vec::new();
        }
        response
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default()
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
    Snapshot(mpsc::Sender<Vec<DeviceSnapshot>>),
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

fn mounted_worker(requests: &mpsc::Receiver<MountedRequest>) {
    let mut manager = attachments::Manager::new(garmin_device::MountedMtpMonitor::new());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("device browser runtime must start");
    loop {
        log_device_events(manager.poll());
        match requests.recv_timeout(Duration::from_millis(250)) {
            Ok(MountedRequest::Snapshot(reply)) => {
                log_device_events(manager.poll());
                let value = manager.presentations().into_iter().map(snapshot).collect();
                let _ignored = reply.send(value);
            }
            Ok(MountedRequest::Catalog(key, progress, reply)) => {
                log_device_events(manager.poll());
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| candidate.browse_with_progress(&progress))
                    .map(|catalog| device_catalog(key, catalog));
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Browser(request, progress, reply)) => {
                log_device_events(manager.poll());
                let key = browser_device_key(&request).to_owned();
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| browser(&runtime, &candidate, request, &progress));
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Download(key, target, progress, reply)) => {
                log_device_events(manager.poll());
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| download(&runtime, &candidate, target, &progress));
                let _ignored = reply.send(result);
            }
            Ok(MountedRequest::Upload(request, contents, progress, reply)) => {
                log_device_events(manager.poll());
                let key = request.device_key.clone();
                let result = manager
                    .candidate(&key)
                    .ok_or_else(|| "the selected device is no longer connected".to_owned())
                    .and_then(|candidate| {
                        upload(&runtime, &candidate, &request, &contents, &progress)
                    });
                let _ignored = reply.send(result);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn log_device_events(events: Vec<attachments::Event>) {
    for event in events {
        match event {
            attachments::Event::Attached { key, name } => {
                tracing::info!(device_key = %key, device_name = %name, "device attached");
            }
            attachments::Event::Detached { key } => {
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

fn browser(
    runtime: &tokio::runtime::Runtime,
    candidate: &garmin_device::MountedMtpCandidate,
    request: DeviceBrowserRequest,
    progress: &ProgressReporter,
) -> Result<DeviceCatalogSnapshot, String> {
    let device_key = browser_device_key(&request).to_owned();
    let catalog = candidate.browse_with_progress(progress)?;
    let device = garmin_device::MountedMtpDevice::new(candidate.mount_id.clone());
    runtime.block_on(execute_browser_operation(
        &device, &catalog, request, progress,
    ))?;
    candidate
        .browse_with_progress(progress)
        .map(|catalog| device_catalog(device_key, catalog))
}

fn download(
    runtime: &tokio::runtime::Runtime,
    candidate: &garmin_device::MountedMtpCandidate,
    target: DeviceBrowserTarget,
    progress: &ProgressReporter,
) -> Result<garmin_device::PreparedDeviceBrowserDownload, String> {
    let catalog = candidate.browse_with_progress(progress)?;
    let device = garmin_device::MountedMtpDevice::new(candidate.mount_id.clone());
    runtime.block_on(execute_browser_download(
        &device, &catalog, target, progress,
    ))
}

fn upload(
    runtime: &tokio::runtime::Runtime,
    candidate: &garmin_device::MountedMtpCandidate,
    request: &DeviceBrowserUpload,
    contents: &std::path::Path,
    progress: &ProgressReporter,
) -> Result<DeviceCatalogSnapshot, String> {
    let device_key = request.device_key.clone();
    let catalog = candidate.browse_with_progress(progress)?;
    let device = garmin_device::MountedMtpDevice::new(candidate.mount_id.clone());
    runtime.block_on(execute_browser_upload(
        &device, &catalog, request, contents, progress,
    ))?;
    candidate
        .browse_with_progress(progress)
        .map(|catalog| device_catalog(device_key, catalog))
}

fn snapshot(presentation: attachments::Presentation) -> DeviceSnapshot {
    DeviceSnapshot {
        key: presentation.key,
        name: presentation.name,
        identifier: presentation
            .identifier
            .map(garmin_device::DeviceId::into_u32),
        software_version: presentation
            .software_version
            .map(garmin_device::SoftwareVersion::into_hundredths),
        inspection: match presentation.state {
            attachments::InspectionState::Running => InspectionState::Running,
            attachments::InspectionState::Ready => InspectionState::Ready,
            attachments::InspectionState::Failed => InspectionState::Failed,
        },
        inspection_error: presentation.inspection_error,
        capabilities: presentation
            .capabilities
            .into_iter()
            .filter_map(|capability| {
                let data_type = match capability.data_type() {
                    garmin_device::DataType::Activity => DeviceDataType::Activity,
                    garmin_device::DataType::Workout => DeviceDataType::Workout,
                    garmin_device::DataType::Course => DeviceDataType::Course,
                    _ => return None,
                };
                let direction = match capability.direction() {
                    garmin_device::TransferDirection::OutputFromUnit => {
                        TransferDirection::OutputFromUnit
                    }
                    garmin_device::TransferDirection::InputToUnit => TransferDirection::InputToUnit,
                    garmin_device::TransferDirection::InputOutput => TransferDirection::InputOutput,
                };
                Some(DeviceCapability {
                    data_type,
                    direction,
                })
            })
            .collect(),
        storages: presentation
            .storage
            .map(|state| state.storages)
            .unwrap_or_default(),
    }
}
