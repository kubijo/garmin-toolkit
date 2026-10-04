use garmin_device::storage::DeviceWrite;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, SourceIdentity},
    device::{InspectionSection, ProfileMarkerInspection},
    identity::{DisplayName, Source as ProfileSource, SourceId, UserId},
    observation::ObservationId,
    value::Timestamp,
};
use garmin_progress::{CancellationToken, ProgressReporter};
use garmin_service_api::{
    ActivityDetailSnapshot, ActivitySnapshot, ApplicationService, AvatarUpload,
    DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload, DeviceCatalogEntry,
    DeviceCatalogEntryKind, DeviceCatalogSnapshot, DeviceCatalogStorage, DeviceFitImportOutcome,
    DeviceFitImportPlan, DeviceFitImportStatus, DeviceFitPreview, DeviceFitPreviewActivity,
    DeviceSnapshot, DownloadTicket, ProfileAvatarSnapshot, ProfileSnapshot,
};
use garmin_services::{
    Application, AvatarImportRequest, FitImportRequest, FitImportResult, ImportDisposition,
    UserContext, deployment::Deployment, devices::fit_import::Plan as FitPlan,
    snapshots::SnapshotOperations,
};
use remoc::prelude::ServerShared as _;
use remoc::{rch, rtc};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

#[cfg(any(feature = "demo", test))]
pub(super) mod demo;
#[cfg(not(feature = "demo"))]
pub(super) mod mounted;

const AVATAR_SOURCE_LABEL: &str = "Browser profile pictures";
const AVATAR_SOURCE_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/browser-profile-pictures/source/v1";
const AVATAR_OPERATION_ID_DOMAIN_V1: &[u8] =
    b"garmin-toolkit/browser-profile-pictures/acquisition/v1";
const DEVICE_SOURCE_LABEL: &str = "Connected Garmin device";
const DEVICE_SOURCE_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/device-files/source/v1";
const MAX_FIT_PLANS: usize = 64;

struct PairingTarget {
    primary_storage: String,
    existing: Option<(String, ProfileMarkerInspection)>,
}

async fn inspect_pairing_target(
    device: &dyn DeviceWrite,
    expected_digest: &str,
) -> Result<PairingTarget, String> {
    let primary_storage = device
        .primary_storage_id()
        .await
        .map_err(|error| error.to_string())?;
    let state = device.state().await.map_err(|error| error.to_string())?;
    if !state
        .storages
        .iter()
        .any(|volume| volume.id == primary_storage)
    {
        return Err("the primary storage is not in the inspected device state".to_owned());
    }
    let mut existing = None;
    for volume in state.storages {
        let inspection =
            garmin_update::inspect_device_state(device, &volume.id, Some(expected_digest)).await;
        if !matches!(
            inspection.namespace,
            InspectionSection::Missing | InspectionSection::Available(_)
        ) || !matches!(
            inspection.identity,
            InspectionSection::Missing | InspectionSection::Available(_)
        ) || !matches!(inspection.transaction, InspectionSection::Missing)
        {
            return Err("device state needs review before pairing".to_owned());
        }
        match inspection.marker {
            InspectionSection::Missing => {}
            InspectionSection::Available(marker) if existing.is_none() => {
                existing = Some((volume.id, marker));
            }
            InspectionSection::Available(_) => {
                return Err("multiple pairing markers need review".to_owned());
            }
            InspectionSection::Unavailable(_) => {
                return Err("the pairing marker needs review".to_owned());
            }
        }
    }
    Ok(PairingTarget {
        primary_storage,
        existing,
    })
}

struct FitPlans {
    epoch: Uuid,
    issued: HashMap<Uuid, Uuid>,
    order: VecDeque<Uuid>,
    active: HashSet<(UserId, Uuid)>,
}

impl FitPlans {
    fn new(epoch: Uuid) -> Self {
        Self {
            epoch,
            issued: HashMap::new(),
            order: VecDeque::new(),
            active: HashSet::new(),
        }
    }

    fn reset_for_epoch(&mut self, epoch: Uuid) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.issued.clear();
            self.order.clear();
            self.active.clear();
        }
    }

    fn issue(&mut self, id: Uuid, job: Uuid) {
        if self.order.len() == MAX_FIT_PLANS
            && let Some(expired) = self.order.pop_front()
        {
            self.issued.remove(&expired);
        }
        self.order.push_back(id);
        self.issued.insert(id, job);
    }

    fn begin(&mut self, user_id: UserId, approval: Uuid) -> Result<Uuid, &'static str> {
        let job = *self
            .issued
            .get(&approval)
            .ok_or("the FIT file changed since review; open it again")?;
        if !self.active.insert((user_id, job)) {
            return Err("this FIT import is already running");
        }
        Ok(job)
    }
}

struct FitImportLease {
    plans: Arc<Mutex<FitPlans>>,
    epoch: Uuid,
    user_id: UserId,
    job: Uuid,
}

#[cfg(test)]
#[derive(Default)]
struct FitImportPause {
    next: std::sync::atomic::AtomicBool,
    started: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

impl Drop for FitImportLease {
    fn drop(&mut self) {
        let mut plans = self.plans.lock().unwrap_or_else(PoisonError::into_inner);
        if plans.epoch == self.epoch {
            plans.active.remove(&(self.user_id, self.job));
        }
    }
}

pub(super) trait Source: Send {
    fn device_connector(
        &mut self,
        _device_key: &str,
    ) -> Result<Arc<dyn garmin_services::maps::device::Connector>, String> {
        Err("device access is unavailable for this source".to_owned())
    }
    fn refresh_device(&mut self, device_key: &str) -> Result<(), String>;
    fn snapshot(&mut self) -> Vec<DeviceSnapshot>;
    fn catalog(
        &mut self,
        device_key: &str,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String>;
    fn browser(
        &mut self,
        request: DeviceBrowserRequest,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String>;
    fn download(
        &mut self,
        device_key: &str,
        target: DeviceBrowserTarget,
        progress: &ProgressReporter,
    ) -> Result<garmin_device::PreparedDeviceBrowserDownload, String>;
    fn upload(
        &mut self,
        request: DeviceBrowserUpload,
        contents: tempfile::TempPath,
        progress: &ProgressReporter,
    ) -> Result<DeviceCatalogSnapshot, String>;
}

pub(super) type SourceOpenError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub(super) trait SourceProvider {
    fn open(
        self,
        data_root: &Path,
        runtime: tokio::runtime::Handle,
    ) -> Result<Box<dyn Source>, SourceOpenError>;
}

#[derive(Clone)]
pub(super) struct Host {
    source: Arc<Mutex<Box<dyn Source>>>,
    snapshots: Arc<rch::watch::Sender<Vec<DeviceSnapshot>>>,
    downloads: crate::downloads::Downloads,
    refresh_gate: Arc<tokio::sync::Mutex<()>>,
    deployment: Arc<Deployment>,
    operations: Arc<SnapshotOperations>,
    maps: Arc<garmin_services::maps::Operations>,
    fit_plans: Arc<Mutex<FitPlans>>,
    #[cfg(test)]
    fit_import_pause: Option<Arc<FitImportPause>>,
    snapshot_session:
        Arc<tokio::sync::Mutex<Option<Arc<garmin_services::snapshots::SnapshotSession>>>>,
    epoch: Uuid,
    control: Option<crate::control::Connection>,
}

impl Host {
    #[cfg(test)]
    pub(super) fn new(source: Box<dyn Source>, deployment: Arc<Deployment>) -> Arc<Self> {
        Self::with_simulation_write_rate(source, deployment, None)
    }

    pub(super) fn with_simulation_write_rate(
        mut source: Box<dyn Source>,
        deployment: Arc<Deployment>,
        bytes_per_second: Option<std::num::NonZeroU64>,
    ) -> Arc<Self> {
        let (snapshots, _receiver) = rch::watch::channel(source.snapshot());
        let operations = SnapshotOperations::new(
            Arc::clone(&deployment),
            garmin_storage::snapshot::Limits::default(),
        )
        .expect("built-in snapshot limits are valid");
        let epoch = deployment.epoch();
        Arc::new(Self {
            maps: Arc::new(
                garmin_services::maps::Operations::new(Arc::clone(&deployment))
                    .with_simulation_write_rate(bytes_per_second),
            ),
            fit_plans: Arc::new(Mutex::new(FitPlans::new(epoch))),
            #[cfg(test)]
            fit_import_pause: None,
            source: Arc::new(Mutex::new(source)),
            snapshots: Arc::new(snapshots),
            downloads: crate::downloads::Downloads::default(),
            refresh_gate: Arc::new(tokio::sync::Mutex::new(())),
            epoch,
            deployment,
            operations,
            snapshot_session: Arc::default(),
            control: None,
        })
    }

    pub(super) fn start(self: &Arc<Self>) {
        let host = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(host) = host.upgrade() else {
                    break;
                };
                host.refresh().await;
            }
        });
    }

    pub(super) fn with_control(&self, control: Option<crate::control::Connection>) -> Arc<Self> {
        let epoch = self.deployment.epoch();
        self.fit_plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reset_for_epoch(epoch);
        Arc::new(Self {
            control,
            epoch,
            snapshot_session: Arc::default(),
            ..self.clone()
        })
    }

    async fn application(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, Application>, rtc::CallError> {
        self.deployment
            .application(self.epoch)
            .await
            .map_err(|_| rtc::CallError::NotServed)
    }

    async fn refresh(&self) {
        let _guard = self.refresh_gate.lock().await;
        match source_snapshot(Arc::clone(&self.source)).await {
            Ok(next) => publish_snapshot(&self.snapshots, next),
            Err(error) => tracing::warn!(%error, "device snapshot worker failed"),
        }
    }

    pub(super) fn take_download(&self, token: Uuid) -> Option<crate::downloads::Download> {
        self.downloads.take(token)
    }
}

impl ApplicationService for Host {
    async fn maps(
        &self,
        device_key: String,
    ) -> Result<Result<garmin_service_api::maps::MapServiceClient, String>, rtc::CallError> {
        let _lease = self.application().await?;
        let source = Arc::clone(&self.source);
        let key = device_key.clone();
        let connector = tokio::task::spawn_blocking(move || {
            source
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .device_connector(&key)
        })
        .await
        .map_err(|_| rtc::CallError::NotServed)?;
        let connector = match connector {
            Ok(connector) => connector,
            Err(error) => return Ok(Err(error)),
        };
        let session = match self
            .maps
            .open(device_key, connector, cfg!(feature = "demo"))
            .await
        {
            Ok(session) => session,
            Err(error) => return Ok(Err(format!("{error:#}"))),
        };
        let (server, client) = garmin_service_api::maps::MapServiceServerShared::<
            _,
            remoc::codec::Default,
        >::new(Arc::new(session));
        tokio::spawn(server.serve());
        Ok(Ok(client))
    }

    async fn server_directory(
        &self,
        user_id: garmin_model::identity::UserId,
        path: String,
    ) -> Result<Result<garmin_service_api::files::Directory, String>, rtc::CallError> {
        if self.epoch != self.deployment.epoch() {
            return Err(rtc::CallError::NotServed);
        }
        if let Err(error) = self.operations.connect(UserContext::new(user_id)).await {
            return Ok(Err(error.message));
        }
        Ok(
            tokio::task::spawn_blocking(move || crate::files::directory(Path::new(&path)))
                .await
                .map_err(|error| error.to_string())
                .and_then(std::convert::identity),
        )
    }

    async fn snapshot_file(
        &self,
        operation: Uuid,
        selection: garmin_service_api::files::Selection,
    ) -> Result<Result<garmin_service_api::snapshots::SnapshotStatus, String>, rtc::CallError> {
        let session = self.snapshot_session.lock().await.clone();
        Ok(match session {
            Some(session) => session
                .file(operation, selection)
                .await
                .map_err(|error| error.message),
            None => Err("open an owner snapshot session first".into()),
        })
    }

    fn deployment_epoch(&self) -> impl Future<Output = Result<Uuid, rtc::CallError>> {
        std::future::ready(Ok(self.epoch))
    }

    async fn snapshots(
        &self,
        user_id: garmin_model::identity::UserId,
        operation: Option<Uuid>,
    ) -> Result<Result<garmin_service_api::snapshots::SnapshotServiceClient, String>, rtc::CallError>
    {
        if self.epoch != self.deployment.epoch() {
            return Err(rtc::CallError::NotServed);
        }
        let actor = UserContext::new(user_id);
        let session = match match operation {
            Some(operation) => self.operations.reconnect(actor, operation).await,
            None => self.operations.connect(actor).await,
        } {
            Ok(session) => session,
            Err(error) => return Ok(Err(error.message)),
        };
        let session = Arc::new(session);
        *self.snapshot_session.lock().await = Some(Arc::clone(&session));
        let (server, client) = garmin_service_api::snapshots::SnapshotServiceServerShared::<
            _,
            remoc::codec::Default,
        >::new(session);
        tokio::spawn(server.serve());
        Ok(Ok(client))
    }

    async fn snapshot_download(
        &self,
        operation: Uuid,
    ) -> Result<Result<DownloadTicket, String>, rtc::CallError> {
        let session = self.snapshot_session.lock().await.clone();
        Ok(match session {
            Some(session) => crate::snapshots::prepare(session, operation)
                .await
                .and_then(|prepared| self.downloads.insert(prepared)),
            None => Err("open an owner snapshot session first".into()),
        })
    }
    fn register_control(
        &self,
        browser: String,
        client: garmin_service_api::control::BrowserControlClient,
    ) -> impl Future<
        Output = Result<
            Result<Option<garmin_service_api::control::ControlSession>, String>,
            rtc::CallError,
        >,
    > {
        std::future::ready(Ok(self.control.as_ref().map_or(Ok(None), |control| {
            control.register(browser, client).map(Some)
        })))
    }
    fn logs(
        &self,
    ) -> impl Future<Output = Result<garmin_service_api::logging::LogServiceClient, remoc::rtc::CallError>>
    {
        std::future::ready(
            garmin_logging::Store::global()
                .map(|store| store.client())
                .ok_or(remoc::rtc::CallError::NotServed),
        )
    }

    fn deployment_mode(
        &self,
    ) -> impl Future<Output = Result<garmin_service_api::DeploymentMode, rtc::CallError>> {
        std::future::ready(Ok(crate::mode::DEPLOYMENT_MODE))
    }

    fn heartbeat(&self) -> impl Future<Output = Result<(), rtc::CallError>> {
        std::future::ready(if self.epoch == self.deployment.epoch() {
            Ok(())
        } else {
            Err(rtc::CallError::NotServed)
        })
    }

    async fn refresh_device(
        &self,
        device_key: String,
    ) -> Result<Result<(), String>, rtc::CallError> {
        let source = Arc::clone(&self.source);
        let result = tokio::task::spawn_blocking(move || {
            source
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .refresh_device(&device_key)
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(std::convert::identity);
        self.refresh().await;
        Ok(result)
    }

    async fn pair_device(
        &self,
        user_id: UserId,
        device_key: String,
        expected_digest: String,
        expected_marker: Option<ProfileMarkerInspection>,
    ) -> Result<Result<ProfileMarkerInspection, String>, rtc::CallError> {
        let application = self.application().await?;
        let refresh_key = device_key.clone();
        let result = async {
            let user = application
                .profiles()
                .await
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|user| user.id() == user_id)
                .ok_or_else(|| "the selected profile no longer exists".to_owned())?;
            let profile_name = user.profile().display_name().to_string();
            let _guard = self
                .maps
                .mutations()
                .acquire(&device_key)
                .map_err(str::to_owned)?;
            let source = Arc::clone(&self.source);
            let connector = tokio::task::spawn_blocking(move || {
                source
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .device_connector(&device_key)
            })
            .await
            .map_err(|error| error.to_string())??;
            let connection = connector
                .connect()
                .await
                .map_err(|error| error.to_string())?;
            if connection.manifest.identity_digest() != expected_digest {
                return Err("the connected device changed since inspection".to_owned());
            }
            let target =
                inspect_pairing_target(connection.device.as_ref(), &expected_digest).await?;
            let marker = match (expected_marker.as_ref(), target.existing.as_ref()) {
                (None, None) => {
                    let marker = garmin_update::ProfileMarker::new(user_id, &profile_name)
                        .map_err(|error| error.to_string())?;
                    garmin_update::create_profile_marker(
                        connection.device.as_ref(),
                        &target.primary_storage,
                        &marker,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                    marker
                }
                (Some(expected), Some((volume, current))) if expected == current => {
                    garmin_update::reassign_profile_marker(
                        connection.device.as_ref(),
                        volume,
                        expected.device_id,
                        expected.revision,
                        user_id,
                        &profile_name,
                    )
                    .await
                    .map_err(|error| error.to_string())?
                }
                _ => return Err("the pairing state changed since review".to_owned()),
            };
            Ok(ProfileMarkerInspection {
                device_id: marker.device_id(),
                user_id: marker.user_id(),
                profile_name: marker.profile_name().to_owned(),
                revision: marker.revision(),
            })
        }
        .await;
        let source = Arc::clone(&self.source);
        let _refresh = tokio::task::spawn_blocking(move || {
            source
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .refresh_device(&refresh_key)
        })
        .await;
        self.refresh().await;
        Ok(result)
    }

    fn watch_devices(
        &self,
    ) -> impl Future<Output = Result<rch::watch::Receiver<Vec<DeviceSnapshot>>, rtc::CallError>>
    {
        std::future::ready(Ok(self.snapshots.subscribe()))
    }

    async fn device_catalog(
        &self,
        device_key: String,
    ) -> Result<Result<DeviceCatalogSnapshot, String>, rtc::CallError> {
        Ok(source_catalog(Arc::clone(&self.source), device_key).await)
    }

    async fn device_browser(
        &self,
        request: DeviceBrowserRequest,
    ) -> Result<Result<DeviceCatalogSnapshot, String>, rtc::CallError> {
        let locks = self.maps.mutations();
        let guard = match locks.acquire(browser_device_key(&request)) {
            Ok(guard) => guard,
            Err(error) => return Ok(Err(error.to_owned())),
        };
        Ok(source_browser(Arc::clone(&self.source), request, guard).await)
    }

    async fn upload_device_browser_file(
        &self,
        request: DeviceBrowserUpload,
        contents: rch::io::Receiver,
    ) -> Result<Result<DeviceCatalogSnapshot, String>, rtc::CallError> {
        let locks = self.maps.mutations();
        let guard = match locks.acquire(&request.device_key) {
            Ok(guard) => guard,
            Err(error) => return Ok(Err(error.to_owned())),
        };
        let result = match receive_browser_upload(&request, contents).await {
            Ok(contents) => source_upload(Arc::clone(&self.source), request, contents, guard).await,
            Err(error) => Err(error),
        };
        Ok(result)
    }

    async fn prepare_device_browser_download(
        &self,
        device_key: String,
        target: DeviceBrowserTarget,
    ) -> Result<Result<DownloadTicket, String>, rtc::CallError> {
        let result = async {
            let download = source_download(Arc::clone(&self.source), device_key, target).await?;
            let file = tokio::fs::File::open(download.path())
                .await
                .map_err(|error| error.to_string())?;
            self.downloads.insert(crate::downloads::Prepared::from_file(
                download.file_name.clone(),
                download.size,
                file,
                download,
            ))
        }
        .await;
        Ok(result)
    }

    async fn device_fit_preview(
        &self,
        user_id: garmin_model::identity::UserId,
        device_key: String,
        target: DeviceBrowserTarget,
    ) -> Result<Result<DeviceFitImportPlan, String>, rtc::CallError> {
        let _lease = self.application().await?;
        let result = async {
            let identity = device_fit_identity(&device_key, &target)?;
            let source = device_source(UserContext::new(user_id))?;
            let download =
                read_device_browser_file(Arc::clone(&self.source), device_key, target).await?;
            let plan = FitPlan::for_file(&source, &identity, &download.bytes);
            Ok(DeviceFitImportPlan {
                id: plan.id(),
                job: plan.job(),
                preview: device_fit_preview(download.file_name, &download.bytes)?,
            })
        }
        .await;
        if let Ok(plan) = &result {
            self.fit_plans
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .issue(plan.id, plan.job);
        }
        Ok(result)
    }

    async fn import_device_fit(
        &self,
        user_id: garmin_model::identity::UserId,
        device_key: String,
        target: DeviceBrowserTarget,
        approval: Uuid,
    ) -> Result<Result<DeviceFitImportOutcome, String>, rtc::CallError> {
        let lease = self.application().await?;
        let identity = match device_fit_identity(&device_key, &target) {
            Ok(identity) => identity,
            Err(error) => return Ok(Err(error)),
        };
        let job = match self
            .fit_plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .begin(user_id, approval)
        {
            Ok(job) => job,
            Err(error) => return Ok(Err(error.to_owned())),
        };
        let import_lease = FitImportLease {
            plans: Arc::clone(&self.fit_plans),
            epoch: self.epoch,
            user_id,
            job,
        };
        let bytes =
            match read_device_browser_file(Arc::clone(&self.source), device_key, target).await {
                Ok(download) => download.bytes,
                Err(error) => return Ok(Err(error)),
            };
        let user = UserContext::new(user_id);
        let source = match device_source(user) {
            Ok(source) => source,
            Err(error) => return Ok(Err(error)),
        };
        let plan = FitPlan::for_file(&source, &identity, &bytes);
        if job != plan.job() {
            return Ok(Err(
                "the FIT file changed since review; open it again".to_owned()
            ));
        }
        let deployment = Arc::clone(&self.deployment);
        let epoch = self.epoch;
        #[cfg(test)]
        let pause = self.fit_import_pause.clone();
        drop(lease);
        let result = tokio::spawn(async move {
            let _import_lease = import_lease;
            let application = deployment
                .application(epoch)
                .await
                .map_err(|error| error.to_string())?;
            #[cfg(test)]
            if let Some(pause) = pause
                && pause.next.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                pause.started.notify_one();
                pause.resume.notified().await;
            }
            import_device_fit(
                &application,
                user,
                &source,
                identity,
                plan.operation(),
                &bytes,
            )
            .await
        })
        .await
        .map_err(|error| error.to_string());
        let result = match result {
            Ok(result) => result,
            Err(error) => return Ok(Err(error)),
        };
        Ok(result)
    }

    async fn device_fit_import_status(
        &self,
        user_id: garmin_model::identity::UserId,
        job: Uuid,
    ) -> Result<Result<DeviceFitImportStatus, String>, rtc::CallError> {
        let application = self.application().await?;
        if self
            .fit_plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .contains(&(user_id, job))
        {
            return Ok(Ok(DeviceFitImportStatus::Running));
        }
        Ok(application
            .fit_import_completed(
                UserContext::new(user_id),
                AcquisitionOperationId::from_u128(job.as_u128()),
            )
            .await
            .map(|completed| {
                if completed {
                    DeviceFitImportStatus::Completed
                } else {
                    DeviceFitImportStatus::Stopped
                }
            })
            .map_err(|error| error.to_string()))
    }

    async fn profiles(&self) -> Result<Result<Vec<ProfileSnapshot>, String>, rtc::CallError> {
        Ok(profile_snapshots(&*self.application().await?).await)
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
            .application()
            .await?
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
            .application()
            .await?
            .update_profile_preferences(UserContext::new(user_id), preferences)
            .await
            .map_err(|error| error.to_string()))
    }

    async fn update_accent(
        &self,
        user_id: garmin_model::identity::UserId,
        accent: Option<garmin_color::Color>,
    ) -> Result<Result<garmin_model::identity::User, String>, rtc::CallError> {
        Ok(self
            .application()
            .await?
            .update_profile_accent(UserContext::new(user_id), accent)
            .await
            .map_err(|error| error.to_string()))
    }

    async fn import_avatar(
        &self,
        user_id: garmin_model::identity::UserId,
        upload: AvatarUpload,
    ) -> Result<Result<ProfileAvatarSnapshot, String>, rtc::CallError> {
        Ok(import_avatar(
            &*self.application().await?,
            UserContext::new(user_id),
            upload,
        )
        .await)
    }

    async fn activity(
        &self,
        user_id: garmin_model::identity::UserId,
        observation_id: ObservationId,
    ) -> Result<Result<Option<ActivityDetailSnapshot>, String>, rtc::CallError> {
        Ok(self
            .application()
            .await?
            .activity(UserContext::new(user_id), observation_id)
            .await
            .map(|details| details.as_ref().map(activity_detail_snapshot))
            .map_err(|error| error.to_string()))
    }
}

async fn import_avatar(
    application: &Application,
    user: UserContext,
    upload: AvatarUpload,
) -> Result<ProfileAvatarSnapshot, String> {
    let source = avatar_source(user)?;
    let identity =
        SourceIdentity::from_string(upload.file_name).map_err(|error| error.to_string())?;
    let crop = garmin_importer::AvatarCrop::from_pixels(
        upload.crop.left,
        upload.crop.top,
        upload.crop.edge,
    )
    .map_err(|error| error.to_string())?;
    let operation = avatar_operation_id(&source, &identity, &upload.bytes, upload.crop);
    application
        .import_avatar(AvatarImportRequest::from_parts(
            user,
            &source,
            identity,
            operation,
            timestamp(SystemTime::now())?,
            &upload.bytes,
            crop,
        ))
        .await
        .map_err(|error| error.to_string())?;
    let avatar = application
        .profile_avatar(user)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "the imported profile picture is unavailable".to_owned())?;
    Ok(ProfileAvatarSnapshot {
        key: avatar.artifact_id().to_string(),
        thumbnail: avatar.into_thumbnail(),
    })
}

fn avatar_source(user: UserContext) -> Result<ProfileSource, String> {
    let mut digest = blake3::Hasher::new();
    digest.update(AVATAR_SOURCE_ID_DOMAIN_V1);
    digest.update(user.user_id().to_string().as_bytes());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, digest.finalize().as_bytes());
    let label = AVATAR_SOURCE_LABEL
        .parse()
        .map_err(|error: garmin_model::identity::Error| error.to_string())?;
    Ok(ProfileSource::from_parts(
        SourceId::from_u128(id.as_u128()),
        user.user_id(),
        label,
        None,
    ))
}

fn device_source(user: UserContext) -> Result<ProfileSource, String> {
    let mut digest = blake3::Hasher::new();
    digest.update(DEVICE_SOURCE_ID_DOMAIN_V1);
    digest.update(user.user_id().to_string().as_bytes());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, digest.finalize().as_bytes());
    let label = DEVICE_SOURCE_LABEL
        .parse()
        .map_err(|error: garmin_model::identity::Error| error.to_string())?;
    Ok(ProfileSource::from_parts(
        SourceId::from_u128(id.as_u128()),
        user.user_id(),
        label,
        None,
    ))
}

fn avatar_operation_id(
    source: &ProfileSource,
    identity: &SourceIdentity,
    bytes: &[u8],
    crop: garmin_service_api::AvatarCrop,
) -> AcquisitionOperationId {
    let mut digest = blake3::Hasher::new();
    digest.update(AVATAR_OPERATION_ID_DOMAIN_V1);
    digest.update(source.id().to_string().as_bytes());
    digest.update(&(identity.as_str().len() as u64).to_le_bytes());
    digest.update(identity.as_str().as_bytes());
    digest.update(ArtifactDigest::from_bytes(bytes).as_blake3().as_bytes());
    digest.update(&crop.left.to_le_bytes());
    digest.update(&crop.top.to_le_bytes());
    digest.update(&crop.edge.to_le_bytes());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, digest.finalize().as_bytes());
    AcquisitionOperationId::from_u128(id.as_u128())
}

fn timestamp(time: SystemTime) -> Result<Timestamp, String> {
    let milliseconds = time
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let milliseconds = i64::try_from(milliseconds).map_err(|error| error.to_string())?;
    Timestamp::from_unix_milliseconds(milliseconds).map_err(|error| error.to_string())
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
    ActivityDetailSnapshot {
        id: details.observation_id(),
        recording: garmin_service_api::ActivityRecordingSnapshot::from(
            details.normalized().activity(),
        ),
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

async fn source_catalog(
    source: Arc<Mutex<Box<dyn Source>>>,
    device_key: String,
) -> Result<DeviceCatalogSnapshot, String> {
    let cancellation = CancellationToken::default();
    let progress = ProgressReporter::default().with_cancellation(cancellation.clone());
    let _cancel_on_drop = CancelOnDrop(cancellation);
    tokio::task::spawn_blocking(move || {
        source
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .catalog(&device_key, &progress)
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn source_browser(
    source: Arc<Mutex<Box<dyn Source>>>,
    request: DeviceBrowserRequest,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<DeviceCatalogSnapshot, String> {
    let cancellation = CancellationToken::default();
    let progress = ProgressReporter::default().with_cancellation(cancellation.clone());
    let _cancel_on_drop = CancelOnDrop(cancellation);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        source
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .browser(request, &progress)
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn source_download(
    source: Arc<Mutex<Box<dyn Source>>>,
    device_key: String,
    target: DeviceBrowserTarget,
) -> Result<garmin_device::PreparedDeviceBrowserDownload, String> {
    let cancellation = CancellationToken::default();
    let progress = ProgressReporter::default().with_cancellation(cancellation.clone());
    let _cancel_on_drop = CancelOnDrop(cancellation);
    tokio::task::spawn_blocking(move || {
        source
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .download(&device_key, target, &progress)
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn source_upload(
    source: Arc<Mutex<Box<dyn Source>>>,
    request: DeviceBrowserUpload,
    contents: tempfile::TempPath,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<DeviceCatalogSnapshot, String> {
    let cancellation = CancellationToken::default();
    let progress = ProgressReporter::default().with_cancellation(cancellation.clone());
    let _cancel_on_drop = CancelOnDrop(cancellation);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        source
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .upload(request, contents, &progress)
    })
    .await
    .map_err(|error| error.to_string())?
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct DeviceBrowserFile {
    file_name: String,
    bytes: Vec<u8>,
}

async fn read_device_browser_file(
    source: Arc<Mutex<Box<dyn Source>>>,
    device_key: String,
    target: DeviceBrowserTarget,
) -> Result<DeviceBrowserFile, String> {
    let download = source_download(source, device_key, target).await?;
    let bytes = tokio::fs::read(download.path())
        .await
        .map_err(|error| error.to_string())?;
    Ok(DeviceBrowserFile {
        file_name: download.file_name,
        bytes,
    })
}

async fn receive_browser_upload(
    request: &DeviceBrowserUpload,
    mut contents: rch::io::Receiver,
) -> Result<tempfile::TempPath, String> {
    if request.size > garmin_device::MAX_BROWSER_TRANSFER_BYTES {
        return Err("the device upload exceeds the 512 MiB browser limit".to_owned());
    }
    if contents.size() != Some(request.size) {
        return Err("the device upload stream size does not match its request".to_owned());
    }
    let temporary = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    let mut destination =
        tokio::fs::File::from_std(temporary.reopen().map_err(|error| error.to_string())?);
    let copied = tokio::io::copy(&mut contents, &mut destination)
        .await
        .map_err(|error| error.to_string())?;
    destination
        .flush()
        .await
        .map_err(|error| error.to_string())?;
    destination
        .sync_all()
        .await
        .map_err(|error| error.to_string())?;
    if copied != request.size {
        return Err(format!(
            "the device upload ended after {copied} bytes; expected {}",
            request.size
        ));
    }
    Ok(temporary.into_temp_path())
}

fn device_fit_preview(file_name: String, bytes: &[u8]) -> Result<DeviceFitPreview, String> {
    let import = garmin_fit::normalize_activities(bytes).map_err(|error| error.to_string())?;
    let activities = import
        .normalized_activities()
        .map(|activity| DeviceFitPreviewActivity {
            source: activity
                .creator()
                .product_name()
                .map_or_else(|| "FIT".to_owned(), ToString::to_string),
            summary: activity.activity().summary(),
            recording: garmin_service_api::ActivityRecordingSnapshot::from(activity.activity()),
        })
        .collect::<Vec<_>>();
    if activities.is_empty() {
        return Err("the FIT file does not contain an activity".to_owned());
    }
    Ok(DeviceFitPreview {
        file_name,
        activities,
    })
}

fn device_fit_identity(
    device_key: &str,
    target: &DeviceBrowserTarget,
) -> Result<SourceIdentity, String> {
    SourceIdentity::from_string(format!(
        "device/{device_key}/{}/{}",
        target.storage_id, target.path
    ))
    .map_err(|error| error.to_string())
}

async fn import_device_fit(
    application: &Application,
    user: UserContext,
    source: &ProfileSource,
    identity: SourceIdentity,
    operation: AcquisitionOperationId,
    bytes: &[u8],
) -> Result<DeviceFitImportOutcome, String> {
    let result = application
        .import_fit(FitImportRequest::from_parts(
            user,
            source,
            identity,
            operation,
            timestamp(SystemTime::now())?,
            bytes,
        ))
        .await
        .map_err(|error| error.to_string())?;
    if result.disposition() == ImportDisposition::Existing {
        return Ok(DeviceFitImportOutcome::Duplicate);
    }
    Ok(match result {
        FitImportResult::Imported { observations, .. } => DeviceFitImportOutcome::Imported {
            activities: u64::try_from(observations.len()).unwrap_or(u64::MAX),
        },
        FitImportResult::Rejected { failure, .. } => DeviceFitImportOutcome::Rejected {
            reason: failure.to_string(),
        },
    })
}

pub(super) fn browser_device_key(request: &DeviceBrowserRequest) -> &str {
    match request {
        DeviceBrowserRequest::CreateDirectory { device_key, .. }
        | DeviceBrowserRequest::Remove { device_key, .. } => device_key,
    }
}

pub(super) async fn execute_browser_operation<D>(
    device: &D,
    catalog: &garmin_device::DeviceCatalog,
    request: DeviceBrowserRequest,
    progress: &ProgressReporter,
) -> Result<(), String>
where
    D: garmin_device::storage::DeviceWrite + ?Sized,
{
    match request {
        DeviceBrowserRequest::CreateDirectory {
            storage_id,
            parent,
            name,
            ..
        } => garmin_device::create_browser_directory_with_progress(
            device,
            catalog,
            &storage_id,
            &parent,
            &name,
            progress,
        )
        .await
        .map_err(|error| error.to_string()),
        DeviceBrowserRequest::Remove { target, .. } => {
            let target = device_browser_target(target);
            garmin_device::remove_browser_target_with_progress(device, catalog, &target, progress)
                .await
                .map_err(|error| error.to_string())
        }
    }
}

pub(super) async fn execute_browser_download<D>(
    device: &D,
    catalog: &garmin_device::DeviceCatalog,
    target: DeviceBrowserTarget,
    progress: &ProgressReporter,
) -> Result<garmin_device::PreparedDeviceBrowserDownload, String>
where
    D: garmin_device::storage::DeviceRead + ?Sized,
{
    garmin_device::prepare_browser_download(
        device,
        catalog,
        &device_browser_target(target),
        progress,
    )
    .await
    .map_err(|error| error.to_string())
}

pub(super) async fn execute_browser_upload<D>(
    device: &D,
    catalog: &garmin_device::DeviceCatalog,
    request: &DeviceBrowserUpload,
    contents: &Path,
    progress: &ProgressReporter,
) -> Result<(), String>
where
    D: garmin_device::storage::DeviceWrite + ?Sized,
{
    garmin_device::upload_browser_file_from_path(
        device,
        catalog,
        garmin_device::BrowserFileUpload {
            storage_id: &request.storage_id,
            directory: &request.directory,
            file_name: &request.file_name,
            source: contents,
            size: request.size,
        },
        progress,
    )
    .await
    .map_err(|error| error.to_string())
}

pub(super) fn device_browser_target(
    target: garmin_service_api::DeviceBrowserTarget,
) -> garmin_device::DeviceBrowserTarget {
    garmin_device::DeviceBrowserTarget {
        storage_id: target.storage_id,
        path: target.path,
        kind: match target.kind {
            DeviceCatalogEntryKind::Directory => garmin_device::DeviceCatalogEntryKind::Directory,
            DeviceCatalogEntryKind::File => garmin_device::DeviceCatalogEntryKind::File,
        },
    }
}

pub(super) fn device_catalog(
    device_key: String,
    catalog: garmin_device::DeviceCatalog,
) -> DeviceCatalogSnapshot {
    DeviceCatalogSnapshot {
        device_key,
        storages: catalog
            .storages
            .into_iter()
            .map(|storage| DeviceCatalogStorage {
                id: storage.id,
                label: storage.label,
                entries: storage
                    .entries
                    .into_iter()
                    .map(|entry| DeviceCatalogEntry {
                        path: entry.path,
                        kind: match entry.kind {
                            garmin_device::DeviceCatalogEntryKind::Directory => {
                                DeviceCatalogEntryKind::Directory
                            }
                            garmin_device::DeviceCatalogEntryKind::File => {
                                DeviceCatalogEntryKind::File
                            }
                        },
                        size: entry.size,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn publish_snapshot(
    snapshots: &rch::watch::Sender<Vec<DeviceSnapshot>>,
    next: Vec<DeviceSnapshot>,
) {
    if snapshots.borrow().as_slice() != next {
        snapshots.send_replace(next);
    }
}

fn device_snapshot(presentation: garmin_device::attachments::Presentation) -> DeviceSnapshot {
    use garmin_device::attachments;
    use garmin_service_api::{
        DeviceCapability, DeviceDataType, InspectionState, TransferDirection,
    };
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
        report: presentation.report,
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

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        DeviceBrowserFile, FitImportLease, FitImportPause, Host, Source, SourceProvider,
        demo::{
            DEVICE_KEY as DEMO_DEVICE_KEY, Provider as DemoProvider, STORAGE_ID as DEMO_STORAGE_ID,
        },
    };
    use garmin_fit::fixture;
    use garmin_progress::ProgressReporter;
    use garmin_service_api::{
        ApplicationService as _, ApplicationServiceClient, ApplicationServiceServerShared,
        AvatarCrop, AvatarUpload, DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload,
        DeviceCatalogEntryKind, DeviceCatalogSnapshot, DeviceFitImportOutcome,
        DeviceFitImportStatus, DeviceSnapshot, InspectionState,
    };
    use image::{DynamicImage, ImageFormat, RgbImage};
    use remoc::ConnectExt as _;
    use remoc::prelude::ServerShared as _;
    use uuid::Uuid;

    struct FitSource {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    fn demo_source(directory: &tempfile::TempDir) -> Box<dyn Source> {
        DemoProvider
            .open(directory.path(), tokio::runtime::Handle::current())
            .unwrap()
    }

    async fn demo_browser(
        host: &Host,
        request: DeviceBrowserRequest,
    ) -> Result<DeviceCatalogSnapshot, String> {
        host.device_browser(request).await.unwrap()
    }

    async fn demo_upload(
        host: &Host,
        directory: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<DeviceCatalogSnapshot, String> {
        use tokio::io::AsyncWriteExt as _;

        let size = u64::try_from(bytes.len()).unwrap();
        let (mut sender, receiver) = remoc::rch::io::sized::<remoc::codec::Default>(size);
        let send = async move {
            sender
                .write_all(bytes)
                .await
                .map_err(|error| error.to_string())?;
            sender.shutdown().await.map_err(|error| error.to_string())
        };
        let receive = host.upload_device_browser_file(
            DeviceBrowserUpload {
                device_key: DEMO_DEVICE_KEY.to_owned(),
                storage_id: DEMO_STORAGE_ID.to_owned(),
                directory: directory.into(),
                file_name: file_name.to_owned(),
                size,
            },
            receiver,
        );
        let (upload_result, stream_result) = futures_lite::future::zip(receive, send).await;
        stream_result?;
        upload_result.unwrap()
    }

    async fn demo_download(
        host: &Host,
        target: DeviceBrowserTarget,
    ) -> Result<DeviceBrowserFile, String> {
        let ticket = host
            .prepare_device_browser_download(DEMO_DEVICE_KEY.to_owned(), target)
            .await
            .unwrap()?;
        let prepared = host
            .take_download(ticket.token)
            .ok_or_else(|| "prepared download is missing".to_owned())?;
        let bytes = axum::body::to_bytes(prepared.response().into_body(), usize::MAX)
            .await
            .map_err(|error| error.to_string())?;
        Ok(DeviceBrowserFile {
            file_name: ticket.file_name,
            bytes: bytes.to_vec(),
        })
    }

    fn target(path: &str, kind: DeviceCatalogEntryKind) -> DeviceBrowserTarget {
        DeviceBrowserTarget {
            storage_id: DEMO_STORAGE_ID.to_owned(),
            path: path.into(),
            kind,
        }
    }

    impl Source for FitSource {
        fn refresh_device(&mut self, _key: &str) -> Result<(), String> {
            Ok(())
        }
        fn snapshot(&mut self) -> Vec<DeviceSnapshot> {
            Vec::new()
        }

        fn catalog(
            &mut self,
            _device_key: &str,
            _progress: &ProgressReporter,
        ) -> Result<DeviceCatalogSnapshot, String> {
            Err("catalog is not used by this test".to_owned())
        }

        fn browser(
            &mut self,
            _request: DeviceBrowserRequest,
            _progress: &ProgressReporter,
        ) -> Result<DeviceCatalogSnapshot, String> {
            Err("browser mutations are not used by this test".to_owned())
        }

        fn download(
            &mut self,
            _device_key: &str,
            _target: DeviceBrowserTarget,
            _progress: &ProgressReporter,
        ) -> Result<garmin_device::PreparedDeviceBrowserDownload, String> {
            let mut temporary =
                tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
            temporary
                .write_all(&self.bytes.lock().unwrap())
                .and_then(|()| temporary.flush())
                .and_then(|()| temporary.as_file().sync_all())
                .map_err(|error| error.to_string())?;
            garmin_device::PreparedDeviceBrowserDownload::from_temporary(
                "activity.fit".to_owned(),
                temporary.into_temp_path(),
            )
            .map_err(|error| error.to_string())
        }

        fn upload(
            &mut self,
            _request: DeviceBrowserUpload,
            _contents: tempfile::TempPath,
            _progress: &ProgressReporter,
        ) -> Result<DeviceCatalogSnapshot, String> {
            Err("uploads are not used by this test".to_owned())
        }
    }

    #[tokio::test]
    async fn automatic_inspection_publishes_capacity_through_the_service_contract() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), storage);
        let snapshots = host.watch_devices().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                host.refresh().await;
                if snapshots.borrow().unwrap()[0].inspection != InspectionState::Running {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let ready = snapshots.borrow().unwrap();
        assert_eq!(ready[0].inspection, InspectionState::Ready);
        assert!(ready[0].storages[0].capacity.bytes().is_some());
        let report = ready[0].report.as_ref().unwrap();
        assert!(!report.has_errors());
        assert!(
            matches!(&report.storage, garmin_model::device::InspectionSection::Available(state) if state.storages == ready[0].storages)
        );
        let previous = ready[0].report.clone();
        drop(ready);
        assert!(
            host.refresh_device("detached".to_owned())
                .await
                .unwrap()
                .is_err()
        );
        host.refresh_device(DEMO_DEVICE_KEY.to_owned())
            .await
            .unwrap()
            .unwrap();
        let refreshing = snapshots.borrow().unwrap();
        assert_eq!(refreshing[0].inspection, InspectionState::Running);
        assert_eq!(refreshing[0].report, previous);
    }

    #[tokio::test]
    async fn profile_workflows_use_the_same_application_service_contract() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), storage);

        let created = host
            .create_profile("Alex Rider".to_owned())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(created.profile().display_name().as_str(), "Alex Rider");
    }

    #[tokio::test]
    async fn pairing_and_reassociation_are_create_only_and_bound_to_reviewed_state() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), storage);
        let snapshots = host.watch_devices().await.unwrap();
        let user = host
            .create_profile("Alex Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let path = directory
            .path()
            .join("device")
            .join(garmin_update::PROFILE_MARKER_PATH);
        let digest = garmin_fixtures::device::manifest().identity_digest();
        assert!(
            host.pair_device(
                user.id(),
                DEMO_DEVICE_KEY.to_owned(),
                "wrong digest".to_owned(),
                None
            )
            .await
            .unwrap()
            .is_err()
        );
        assert!(!path.exists());

        let paired = host
            .pair_device(user.id(), DEMO_DEVICE_KEY.to_owned(), digest.clone(), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(paired.user_id, user.id());
        assert_eq!(paired.profile_name, "Alex Rider");
        assert_eq!(paired.revision, 0);
        let original = std::fs::read(&path).unwrap();
        let other = host
            .create_profile("Sam Runner".to_owned())
            .await
            .unwrap()
            .unwrap();
        assert!(
            host.pair_device(other.id(), DEMO_DEVICE_KEY.to_owned(), digest.clone(), None)
                .await
                .unwrap()
                .is_err()
        );
        let reassigned = host
            .pair_device(
                other.id(),
                DEMO_DEVICE_KEY.to_owned(),
                digest.clone(),
                Some(paired.clone()),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reassigned.device_id, paired.device_id);
        assert_eq!(reassigned.user_id, other.id());
        assert_eq!(reassigned.revision, 1);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                host.refresh().await;
                let current = snapshots.borrow().unwrap();
                if current[0].report.as_ref().is_some_and(|report| {
                    report.toolkit.iter().any(|volume| {
                        matches!(
                            &volume.marker,
                            garmin_model::device::InspectionSection::Available(marker)
                                if marker == &reassigned
                        )
                    })
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            host.pair_device(user.id(), DEMO_DEVICE_KEY.to_owned(), digest, Some(paired))
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn explicit_device_browser_returns_only_device_relative_entries() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), storage);

        let catalog = host
            .device_catalog("demo:watch-o-matic-9000".to_owned())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(catalog.storages.len(), 1);
        assert_eq!(catalog.storages[0].id, "internal");
        assert!(catalog.storages[0].entries.iter().all(|entry| {
            !entry.path.as_str().starts_with('/')
                && !entry.path.as_str().contains("..")
                && matches!(
                    entry.kind,
                    DeviceCatalogEntryKind::Directory | DeviceCatalogEntryKind::File
                )
        }));
        assert!(catalog.storages[0].entries.iter().any(|entry| {
            entry.path == "Garmin/Activity/History/2026/city-ride.fit"
                && entry.kind == DeviceCatalogEntryKind::File
                && entry.size.is_some_and(|size| size > 0)
        }));
    }

    #[tokio::test]
    async fn demo_browser_mutations_refresh_and_download_the_isolated_device() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), storage);

        let created = demo_browser(
            &host,
            DeviceBrowserRequest::CreateDirectory {
                device_key: DEMO_DEVICE_KEY.to_owned(),
                storage_id: DEMO_STORAGE_ID.to_owned(),
                parent: "Garmin".into(),
                name: "Inbox".to_owned(),
            },
        )
        .await
        .unwrap();
        assert!(created.storages[0].entries.iter().any(|entry| {
            entry.path == "Garmin/Inbox" && entry.kind == DeviceCatalogEntryKind::Directory
        }));

        let payload = b"mock upload".to_vec();
        let uploaded = demo_upload(&host, "Garmin/Inbox", "payload.bin", &payload)
            .await
            .unwrap();
        assert!(uploaded.storages[0].entries.iter().any(|entry| {
            entry.path == "Garmin/Inbox/payload.bin"
                && entry.kind == DeviceCatalogEntryKind::File
                && entry.size == Some(11)
        }));

        let downloaded = demo_download(
            &host,
            target("Garmin/Inbox/payload.bin", DeviceCatalogEntryKind::File),
        )
        .await
        .unwrap();
        assert_eq!(downloaded.file_name, "payload.bin");
        assert_eq!(downloaded.bytes, payload);

        let archive = demo_download(
            &host,
            target("Garmin/Inbox", DeviceCatalogEntryKind::Directory),
        )
        .await
        .unwrap();
        assert_eq!(archive.file_name, "Inbox.zip");
        assert!(archive.bytes.starts_with(b"PK"));

        let removed = demo_browser(
            &host,
            DeviceBrowserRequest::Remove {
                device_key: DEMO_DEVICE_KEY.to_owned(),
                target: target("Garmin/Inbox", DeviceCatalogEntryKind::Directory),
            },
        )
        .await
        .unwrap();
        assert!(
            removed.storages[0]
                .entries
                .iter()
                .all(|entry| !entry.path.starts_with("Garmin/Inbox"))
        );
    }

    #[tokio::test]
    async fn demo_browser_preserves_the_toolkit_managed_namespace() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), storage);
        let request = DeviceBrowserRequest::CreateDirectory {
            device_key: DEMO_DEVICE_KEY.to_owned(),
            storage_id: DEMO_STORAGE_ID.to_owned(),
            parent: "GARMIN-TOOLKIT".into(),
            name: "unsafe".to_owned(),
        };

        let error = demo_browser(&host, request).await.unwrap_err();

        assert!(error.contains("browse/download-only"));
    }

    #[cfg(feature = "demo")]
    mod maps;

    #[tokio::test]
    async fn browser_avatar_upload_uses_the_application_importer() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), storage);
        let user = host
            .create_profile("Mock Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::new(64, 48))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();

        let avatar = host
            .import_avatar(
                user.id(),
                AvatarUpload {
                    file_name: "mock-avatar.png".to_owned(),
                    bytes: bytes.into_inner(),
                    crop: AvatarCrop {
                        left: 8,
                        top: 0,
                        edge: 48,
                    },
                },
            )
            .await
            .unwrap()
            .unwrap();

        assert!(!avatar.key.is_empty());
        assert!(!avatar.thumbnail.is_empty());
        let profiles = host.profiles().await.unwrap().unwrap();
        let stored = profiles
            .iter()
            .find(|profile| profile.user.id() == user.id())
            .unwrap();
        assert_eq!(
            stored.avatar.as_ref().map(|avatar| avatar.key.as_str()),
            Some(avatar.key.as_str())
        );
    }

    fn fit_target() -> DeviceBrowserTarget {
        DeviceBrowserTarget {
            storage_id: "internal".to_owned(),
            path: "Garmin/Activity/activity.fit".into(),
            kind: DeviceCatalogEntryKind::File,
        }
    }

    #[tokio::test]
    async fn device_fit_preview_approval_binds_profile_and_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let original = fixture::activity(fixture::Sport::Cycling).unwrap();
        let bytes = Arc::new(Mutex::new(original.clone()));
        let host = Host::new(
            Box::new(FitSource {
                bytes: Arc::clone(&bytes),
            }),
            storage,
        );
        let user = host
            .create_profile("Mock Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let target = fit_target();

        let preview = host
            .device_fit_preview(user.id(), "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preview.preview.file_name, "activity.fit");
        assert_eq!(preview.preview.activities.len(), 1);
        assert_ne!(preview.id, preview.job);
        let other_user = host
            .create_profile("Other Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let other_plan = host
            .device_fit_preview(other_user.id(), "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(other_plan.id, preview.id);
        assert!(
            host.import_device_fit(
                user.id(),
                "device".to_owned(),
                target.clone(),
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .unwrap_err()
            .contains("changed since review")
        );
        assert!(
            host.import_device_fit(
                other_user.id(),
                "device".to_owned(),
                target.clone(),
                preview.id,
            )
            .await
            .unwrap()
            .unwrap_err()
            .contains("changed since review")
        );

        *bytes.lock().unwrap() = fixture::activity(fixture::Sport::Running).unwrap();
        let stale = host
            .import_device_fit(user.id(), "device".to_owned(), target.clone(), preview.id)
            .await
            .unwrap()
            .unwrap_err();
        assert!(stale.contains("changed since review"));
        assert_eq!(
            host.device_fit_import_status(user.id(), preview.job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Stopped
        );
    }

    #[tokio::test]
    async fn device_fit_import_is_idempotent_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_deployment(directory.path()).await.unwrap();
        let bytes = Arc::new(Mutex::new(
            fixture::activity(fixture::Sport::Cycling).unwrap(),
        ));
        let host = Host::new(
            Box::new(FitSource {
                bytes: Arc::clone(&bytes),
            }),
            storage,
        );
        let user = host
            .create_profile("Mock Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let target = fit_target();
        let preview = host
            .device_fit_preview(user.id(), "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();

        let first = host
            .import_device_fit(user.id(), "device".to_owned(), target.clone(), preview.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, DeviceFitImportOutcome::Imported { activities: 1 });
        let second = host
            .import_device_fit(user.id(), "device".to_owned(), target.clone(), preview.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second, DeviceFitImportOutcome::Duplicate);
        assert_eq!(
            host.device_fit_import_status(user.id(), preview.job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Completed
        );
        let other_user = host
            .create_profile("Other Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            host.device_fit_import_status(other_user.id(), preview.job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Stopped
        );
        let profiles = host.profiles().await.unwrap().unwrap();
        let imported = profiles
            .iter()
            .find(|profile| profile.user.id() == user.id())
            .unwrap();
        assert_eq!(imported.activities.len(), 1);

        let user_id = user.id();
        host.deployment.close().await;
        drop(host);
        let reopened = crate::prepare_deployment(directory.path()).await.unwrap();
        let restarted = Host::new(Box::new(FitSource { bytes }), reopened);
        assert_eq!(
            restarted
                .device_fit_import_status(user_id, preview.job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Completed
        );
        assert!(
            restarted
                .import_device_fit(user_id, "device".to_owned(), target.clone(), preview.id)
                .await
                .unwrap()
                .unwrap_err()
                .contains("changed since review")
        );
        let renewed = restarted
            .device_fit_preview(user_id, "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(renewed.id, preview.id);
        assert_eq!(renewed.job, preview.job);
        assert_eq!(
            restarted
                .import_device_fit(user_id, "device".to_owned(), target, renewed.id)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportOutcome::Duplicate
        );
    }

    #[tokio::test]
    async fn fit_import_status_tracks_running_job_and_blocks_concurrent_retry() {
        let directory = tempfile::tempdir().unwrap();
        let deployment = crate::prepare_deployment(directory.path()).await.unwrap();
        let bytes = Arc::new(Mutex::new(
            fixture::activity(fixture::Sport::Cycling).unwrap(),
        ));
        let host = Host::new(Box::new(FitSource { bytes }), deployment);
        let user = host
            .create_profile("Mock Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let target = fit_target();
        let preview = host
            .device_fit_preview(user.id(), "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();

        let job = host
            .fit_plans
            .lock()
            .unwrap()
            .begin(user.id(), preview.id)
            .unwrap();
        let lease = FitImportLease {
            plans: Arc::clone(&host.fit_plans),
            epoch: host.epoch,
            user_id: user.id(),
            job,
        };
        assert_eq!(
            host.device_fit_import_status(user.id(), job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Running
        );
        assert!(
            host.import_device_fit(user.id(), "device".to_owned(), target.clone(), preview.id)
                .await
                .unwrap()
                .unwrap_err()
                .contains("already running")
        );

        drop(lease);
        assert_eq!(
            host.device_fit_import_status(user.id(), job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Stopped
        );
        assert_eq!(
            host.import_device_fit(user.id(), "device".to_owned(), target, preview.id)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportOutcome::Imported { activities: 1 }
        );
        assert_eq!(
            host.device_fit_import_status(user.id(), job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Completed
        );
    }

    #[tokio::test]
    async fn fit_import_completes_after_caller_disconnects() {
        let directory = tempfile::tempdir().unwrap();
        let deployment = crate::prepare_deployment(directory.path()).await.unwrap();
        let bytes = Arc::new(Mutex::new(
            fixture::activity(fixture::Sport::Cycling).unwrap(),
        ));
        let base = Host::new(Box::new(FitSource { bytes }), deployment);
        let pause = Arc::new(FitImportPause::default());
        pause.next.store(true, std::sync::atomic::Ordering::SeqCst);
        let host = Arc::new(Host {
            fit_import_pause: Some(Arc::clone(&pause)),
            ..(*base).clone()
        });
        let user = host
            .create_profile("Mock Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let target = fit_target();
        let preview = host
            .device_fit_preview(user.id(), "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();
        let user_id = user.id();
        let plan_id = preview.id;

        let (service, local_client) =
            ApplicationServiceServerShared::<_, remoc::codec::Default>::new(Arc::clone(&host));
        let serving = tokio::spawn(service.serve());
        let (left, right) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(left);
        let providing = tokio::spawn(
            remoc::Connect::io(garmin_service_api::control::transport_config(), read, write)
                .provide(local_client),
        );
        let (read, write) = tokio::io::split(right);
        let client: ApplicationServiceClient =
            remoc::Connect::io(garmin_service_api::control::transport_config(), read, write)
                .consume()
                .await
                .unwrap();

        let disconnected_caller = tokio::spawn({
            let client = client.clone();
            let target = target.clone();
            async move {
                client
                    .import_device_fit(user_id, "device".to_owned(), target, plan_id)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), pause.started.notified())
            .await
            .expect("the host import job started");
        assert_eq!(
            host.device_fit_import_status(user.id(), preview.job)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportStatus::Running
        );

        disconnected_caller.abort();
        assert!(disconnected_caller.await.unwrap_err().is_cancelled());
        drop(client);
        providing.abort();
        let _ = providing.await;
        pause.resume.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match host
                    .device_fit_import_status(user.id(), preview.job)
                    .await
                    .unwrap()
                    .unwrap()
                {
                    DeviceFitImportStatus::Completed => break,
                    DeviceFitImportStatus::Running => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    DeviceFitImportStatus::Stopped => panic!("the detached import stopped"),
                }
            }
        })
        .await
        .expect("the detached import completed");
        assert_eq!(
            host.import_device_fit(user.id(), "device".to_owned(), target, preview.id)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportOutcome::Duplicate
        );
        let profiles = host.profiles().await.unwrap().unwrap();
        let imported = profiles
            .iter()
            .find(|profile| profile.user.id() == user.id())
            .expect("the importing profile remains available");
        assert_eq!(imported.activities.len(), 1);
        serving.abort();
    }

    #[tokio::test]
    async fn fit_approval_survives_reconnect_after_deployment_epoch_change() {
        let directory = tempfile::tempdir().unwrap();
        let deployment = crate::prepare_deployment(directory.path()).await.unwrap();
        let bytes = Arc::new(Mutex::new(
            fixture::activity(fixture::Sport::Cycling).unwrap(),
        ));
        let host = Host::new(Box::new(FitSource { bytes }), deployment);
        let user = host
            .create_profile("Mock Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        // The server's root Host retains its original epoch after a database restore.
        let stale_root = Host {
            epoch: Uuid::new_v4(),
            ..(*host).clone()
        };
        let first = stale_root.with_control(None);
        let target = fit_target();
        let preview = first
            .device_fit_preview(user.id(), "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();
        let reconnected = stale_root.with_control(None);
        assert_eq!(
            reconnected
                .import_device_fit(user.id(), "device".to_owned(), target, preview.id)
                .await
                .unwrap()
                .unwrap(),
            DeviceFitImportOutcome::Imported { activities: 1 }
        );
    }
}
