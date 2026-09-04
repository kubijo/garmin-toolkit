#[cfg(not(feature = "demo"))]
use garmin_device::attachments;
use garmin_service_api::{DeviceService, DeviceSnapshot, InspectionError, InspectionState};
use remoc::{rch, rtc};
use std::future::Future;
#[cfg(not(feature = "demo"))]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub(super) trait Source: Send {
    fn snapshot(&mut self) -> Vec<DeviceSnapshot>;
    fn inspect(&mut self, key: &str) -> Result<(), String>;
}

#[cfg(not(feature = "demo"))]
pub(super) struct MountedSource {
    requests: Option<mpsc::Sender<MountedRequest>>,
    startup_error: Option<String>,
}

#[cfg(not(feature = "demo"))]
impl MountedSource {
    pub(super) fn new() -> Self {
        let (requests, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("garmin-toolkit-hass-devices".to_owned())
            .spawn(move || mounted_worker(&receiver))
        {
            Ok(_worker) => Self {
                requests: Some(requests),
                startup_error: None,
            },
            Err(error) => Self {
                requests: None,
                startup_error: Some(format!("could not start device discovery: {error}")),
            },
        }
    }
}

#[cfg(not(feature = "demo"))]
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

    fn inspect(&mut self, key: &str) -> Result<(), String> {
        let requests = self.requests.as_ref().ok_or_else(|| {
            self.startup_error
                .clone()
                .unwrap_or_else(|| "device discovery stopped".to_owned())
        })?;
        let (reply, response) = mpsc::channel();
        requests
            .send(MountedRequest::Inspect(key.to_owned(), reply))
            .map_err(|_| "device discovery stopped".to_owned())?;
        response
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "device discovery did not respond".to_owned())?
    }
}

#[cfg(not(feature = "demo"))]
enum MountedRequest {
    Snapshot(mpsc::Sender<Vec<DeviceSnapshot>>),
    Inspect(String, mpsc::Sender<Result<(), String>>),
}

#[cfg(not(feature = "demo"))]
fn mounted_worker(requests: &mpsc::Receiver<MountedRequest>) {
    let mut manager = attachments::Manager::new(garmin_device::MountedMtpMonitor::new());
    loop {
        let _events = manager.poll();
        match requests.recv_timeout(Duration::from_millis(250)) {
            Ok(MountedRequest::Snapshot(reply)) => {
                let _events = manager.poll();
                let value = manager.presentations().into_iter().map(snapshot).collect();
                let _ignored = reply.send(value);
            }
            Ok(MountedRequest::Inspect(key, reply)) => {
                let result = manager.inspect(&key);
                let _ignored = reply.send(result);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(any(feature = "demo", test))]
pub(super) struct DemoSource {
    inspected: bool,
}

#[cfg(any(feature = "demo", test))]
impl DemoSource {
    pub(super) const fn new() -> Self {
        Self { inspected: false }
    }
}

#[cfg(any(feature = "demo", test))]
impl Source for DemoSource {
    fn snapshot(&mut self) -> Vec<DeviceSnapshot> {
        vec![DeviceSnapshot {
            key: "demo:fenix-8".to_owned(),
            name: "fēnix 8 – 47mm, Solar".to_owned(),
            identifier: self.inspected.then_some(42_530_200),
            software_version: self.inspected.then_some(1_870),
            inspection: if self.inspected {
                InspectionState::Ready
            } else {
                InspectionState::Available
            },
            storages: self
                .inspected
                .then(|| garmin_device::DeviceStorageState {
                    id: "internal".to_owned(),
                    label: "Internal storage".to_owned(),
                    capacity: garmin_device::StorageCapacity::new(32_000_000_000, 8_600_000_000),
                    writable: Some(true),
                })
                .into_iter()
                .collect(),
        }]
    }

    fn inspect(&mut self, key: &str) -> Result<(), String> {
        if key != "demo:fenix-8" {
            return Err("the attached device is no longer available".to_owned());
        }
        self.inspected = true;
        Ok(())
    }
}

pub(super) struct Host {
    source: Arc<Mutex<Box<dyn Source>>>,
    snapshots: Arc<rch::watch::Sender<Vec<DeviceSnapshot>>>,
}

impl Host {
    pub(super) fn new(mut source: Box<dyn Source>) -> Arc<Self> {
        let (snapshots, _receiver) = rch::watch::channel(source.snapshot());
        Arc::new(Self {
            source: Arc::new(Mutex::new(source)),
            snapshots: Arc::new(snapshots),
        })
    }

    pub(super) fn start(self: &Arc<Self>) {
        let host = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                host.refresh().await;
            }
        });
    }

    async fn refresh(&self) {
        match source_snapshot(Arc::clone(&self.source)).await {
            Ok(next) => publish_snapshot(&self.snapshots, next),
            Err(error) => tracing::warn!(%error, "device snapshot worker failed"),
        }
    }
}

impl DeviceService for Host {
    fn watch(
        &self,
    ) -> impl Future<Output = Result<rch::watch::Receiver<Vec<DeviceSnapshot>>, rtc::CallError>>
    {
        std::future::ready(Ok(self.snapshots.subscribe()))
    }

    fn inspect(&self, key: String) -> impl Future<Output = Result<(), InspectionError>> {
        let source = Arc::clone(&self.source);
        let snapshots = Arc::clone(&self.snapshots);
        async move {
            let inspection_source = Arc::clone(&source);
            tokio::task::spawn_blocking(move || {
                inspection_source
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .inspect(&key)
            })
            .await
            .map_err(|error| InspectionError::Rejected(error.to_string()))?
            .map_err(InspectionError::Rejected)?;
            let next = source_snapshot(source)
                .await
                .map_err(InspectionError::Rejected)?;
            publish_snapshot(&snapshots, next);
            Ok(())
        }
    }
}

async fn source_snapshot(
    source: Arc<Mutex<Box<dyn Source>>>,
) -> Result<Vec<DeviceSnapshot>, String> {
    tokio::task::spawn_blocking(move || {
        source
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot()
    })
    .await
    .map_err(|error| error.to_string())
}

fn publish_snapshot(
    snapshots: &rch::watch::Sender<Vec<DeviceSnapshot>>,
    next: Vec<DeviceSnapshot>,
) {
    if snapshots.borrow().as_slice() != next {
        snapshots.send_replace(next);
    }
}

#[cfg(not(feature = "demo"))]
fn snapshot(presentation: attachments::Presentation) -> DeviceSnapshot {
    DeviceSnapshot {
        key: presentation.key,
        name: presentation.name,
        identifier: presentation
            .identifier
            .map(garmin_device::capabilities::DeviceId::into_u32),
        software_version: presentation
            .software_version
            .map(garmin_device::capabilities::SoftwareVersion::into_hundredths),
        inspection: match presentation.state {
            attachments::InspectionState::Available => InspectionState::Available,
            attachments::InspectionState::Running => InspectionState::Running,
            attachments::InspectionState::Ready => InspectionState::Ready,
            attachments::InspectionState::Failed => InspectionState::Failed,
        },
        storages: presentation
            .storage
            .map(|state| state.storages)
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{DemoSource, Host};
    use garmin_service_api::{DeviceService as _, InspectionState};

    #[tokio::test]
    async fn inspection_publishes_capacity_through_the_service_contract() {
        let host = Host::new(Box::new(DemoSource::new()));
        let mut snapshots = host.watch().await.unwrap();
        assert_eq!(
            snapshots.borrow().unwrap()[0].inspection,
            InspectionState::Available
        );

        host.inspect("demo:fenix-8".to_owned()).await.unwrap();
        snapshots.changed().await.unwrap();
        let ready = snapshots.borrow_and_update().unwrap();

        assert_eq!(ready[0].inspection, InspectionState::Ready);
        assert_eq!(
            ready[0].storages[0].capacity.bytes(),
            Some((32_000_000_000, 8_600_000_000))
        );
    }
}
