#[cfg(not(feature = "demo"))]
use garmin_device::attachments;
use garmin_model::{identity::DisplayName, observation::ObservationId};
use garmin_service_api::{
    ActivityDetailSnapshot, ActivitySnapshot, ApplicationService, DeviceCapability, DeviceDataType,
    DeviceSnapshot, InspectionState, ProfileAvatarSnapshot, ProfileSnapshot, TransferDirection,
};
use garmin_services::{Application, UserContext};
use remoc::{rch, rtc};
use std::future::Future;
#[cfg(not(feature = "demo"))]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub(super) trait Source: Send {
    fn snapshot(&mut self) -> Vec<DeviceSnapshot>;
}

#[cfg(not(feature = "demo"))]
pub(super) struct MountedSource {
    requests: Option<mpsc::Sender<MountedRequest>>,
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
            },
            Err(error) => {
                tracing::error!(%error, "could not start device discovery");
                Self { requests: None }
            }
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
}

#[cfg(not(feature = "demo"))]
enum MountedRequest {
    Snapshot(mpsc::Sender<Vec<DeviceSnapshot>>),
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
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(any(feature = "demo", test))]
pub(super) struct DemoSource;

#[cfg(any(feature = "demo", test))]
impl DemoSource {
    pub(super) const fn new() -> Self {
        Self
    }
}

#[cfg(any(feature = "demo", test))]
impl Source for DemoSource {
    fn snapshot(&mut self) -> Vec<DeviceSnapshot> {
        vec![DeviceSnapshot {
            key: "demo:fenix-8".to_owned(),
            name: "fēnix 8 – 47mm, Solar".to_owned(),
            identifier: Some(42_530_200),
            software_version: Some(1_870),
            inspection: InspectionState::Ready,
            capabilities: vec![
                DeviceCapability {
                    data_type: DeviceDataType::Activity,
                    direction: TransferDirection::OutputFromUnit,
                },
                DeviceCapability {
                    data_type: DeviceDataType::Workout,
                    direction: TransferDirection::InputOutput,
                },
                DeviceCapability {
                    data_type: DeviceDataType::Course,
                    direction: TransferDirection::InputOutput,
                },
            ],
            storages: vec![garmin_device::DeviceStorageState {
                id: "internal".to_owned(),
                label: "Internal storage".to_owned(),
                capacity: garmin_device::StorageCapacity::new(32_000_000_000, 8_600_000_000),
                writable: Some(true),
            }],
        }]
    }
}

pub(super) struct Host {
    source: Arc<Mutex<Box<dyn Source>>>,
    snapshots: Arc<rch::watch::Sender<Vec<DeviceSnapshot>>>,
    application: Application,
}

impl Host {
    pub(super) fn new(mut source: Box<dyn Source>, application: Application) -> Arc<Self> {
        let (snapshots, _receiver) = rch::watch::channel(source.snapshot());
        Arc::new(Self {
            source: Arc::new(Mutex::new(source)),
            snapshots: Arc::new(snapshots),
            application,
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

impl ApplicationService for Host {
    fn deployment_mode(
        &self,
    ) -> impl Future<Output = Result<garmin_service_api::DeploymentMode, rtc::CallError>> {
        std::future::ready(Ok(crate::mode::DEPLOYMENT_MODE))
    }

    fn heartbeat(&self) -> impl Future<Output = Result<(), rtc::CallError>> {
        std::future::ready(Ok(()))
    }

    fn watch_devices(
        &self,
    ) -> impl Future<Output = Result<rch::watch::Receiver<Vec<DeviceSnapshot>>, rtc::CallError>>
    {
        std::future::ready(Ok(self.snapshots.subscribe()))
    }

    async fn profiles(&self) -> Result<Result<Vec<ProfileSnapshot>, String>, rtc::CallError> {
        Ok(profile_snapshots(&self.application).await)
    }

    async fn create_profile(
        &self,
        display_name: String,
    ) -> Result<Result<garmin_model::identity::User, String>, rtc::CallError> {
        let display_name = match DisplayName::from_string(display_name) {
            Ok(display_name) => display_name,
            Err(error) => return Ok(Err(error.to_string())),
        };
        Ok(self
            .application
            .create_profile(display_name)
            .await
            .map_err(|error| error.to_string()))
    }

    async fn update_preferences(
        &self,
        user_id: garmin_model::identity::UserId,
        preferences: garmin_model::identity::ProfilePreferences,
    ) -> Result<Result<garmin_model::identity::User, String>, rtc::CallError> {
        Ok(self
            .application
            .update_profile_preferences(UserContext::new(user_id), preferences)
            .await
            .map_err(|error| error.to_string()))
    }

    async fn activity(
        &self,
        user_id: garmin_model::identity::UserId,
        observation_id: ObservationId,
    ) -> Result<Result<Option<ActivityDetailSnapshot>, String>, rtc::CallError> {
        Ok(self
            .application
            .activity(UserContext::new(user_id), observation_id)
            .await
            .map(|details| details.as_ref().map(activity_detail_snapshot))
            .map_err(|error| error.to_string()))
    }
}

async fn profile_snapshots(application: &Application) -> Result<Vec<ProfileSnapshot>, String> {
    let users = application
        .profiles()
        .await
        .map_err(|error| error.to_string())?;
    let mut profiles = Vec::with_capacity(users.len());
    for user in users {
        let context = UserContext::new(user.id());
        let activities = application
            .activities(context)
            .await
            .map_err(|error| error.to_string())?;
        let avatar = application
            .profile_avatar(context)
            .await
            .map_err(|error| error.to_string())?;
        profiles.push(ProfileSnapshot {
            avatar: avatar.map(|avatar| ProfileAvatarSnapshot {
                key: avatar.artifact_id().to_string(),
                thumbnail: avatar.into_thumbnail(),
            }),
            activities: activities.iter().map(activity_snapshot).collect::<Vec<_>>(),
            user,
        });
    }
    Ok(profiles)
}

fn activity_snapshot(activity: &garmin_services::ActivityPreview) -> ActivitySnapshot {
    let summary = activity.summary();
    ActivitySnapshot {
        id: activity.observation_id(),
        source: activity
            .creator()
            .product_name()
            .map_or_else(|| "FIT".to_owned(), ToString::to_string),
        summary,
    }
}

fn activity_detail_snapshot(details: &garmin_services::ActivityDetails) -> ActivityDetailSnapshot {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    for sample in details.normalized().activity().track() {
        if let Some(coordinate) = sample.coordinate() {
            current.push(coordinate);
        } else if current.len() >= 2 {
            segments.push(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if current.len() >= 2 {
        segments.push(current);
    }
    ActivityDetailSnapshot {
        id: details.observation_id(),
        segments,
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
            attachments::InspectionState::Running => InspectionState::Running,
            attachments::InspectionState::Ready => InspectionState::Ready,
            attachments::InspectionState::Failed => InspectionState::Failed,
        },
        capabilities: presentation
            .capabilities
            .into_iter()
            .filter_map(|capability| {
                let data_type = match capability.data_type() {
                    garmin_device::capabilities::DataType::Activity => DeviceDataType::Activity,
                    garmin_device::capabilities::DataType::Workout => DeviceDataType::Workout,
                    garmin_device::capabilities::DataType::Course => DeviceDataType::Course,
                    _ => return None,
                };
                let direction = match capability.direction() {
                    garmin_device::capabilities::TransferDirection::OutputFromUnit => {
                        TransferDirection::OutputFromUnit
                    }
                    garmin_device::capabilities::TransferDirection::InputToUnit => {
                        TransferDirection::InputToUnit
                    }
                    garmin_device::capabilities::TransferDirection::InputOutput => {
                        TransferDirection::InputOutput
                    }
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

#[cfg(test)]
mod tests {
    use super::{DemoSource, Host};
    use garmin_service_api::{ApplicationService as _, InspectionState};
    use garmin_services::Application;

    #[tokio::test]
    async fn automatic_inspection_publishes_capacity_through_the_service_contract() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let host = Host::new(Box::new(DemoSource::new()), Application::new(storage));
        let snapshots = host.watch_devices().await.unwrap();
        let ready = snapshots.borrow().unwrap();

        assert_eq!(ready[0].inspection, InspectionState::Ready);
        assert_eq!(
            ready[0].storages[0].capacity.bytes(),
            Some((32_000_000_000, 8_600_000_000))
        );
    }

    #[tokio::test]
    async fn profile_workflows_use_the_same_application_service_contract() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let host = Host::new(Box::new(DemoSource::new()), Application::new(storage));

        let created = host
            .create_profile("Alex Rider".to_owned())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(created.profile().display_name().as_str(), "Alex Rider");
    }
}
