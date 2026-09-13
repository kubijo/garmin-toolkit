use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, SourceIdentity},
    identity::{DisplayName, Source as ProfileSource, SourceId},
    observation::ObservationId,
    value::Timestamp,
};
use garmin_progress::{CancellationToken, ProgressReporter};
use garmin_service_api::{
    ActivityDetailSnapshot, ActivitySnapshot, ApplicationService, AvatarUpload,
    DeviceBrowserDownloadTicket, DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload,
    DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogSnapshot, DeviceCatalogStorage,
    DeviceFitImportOutcome, DeviceFitPreview, DeviceFitPreviewActivity, DeviceSnapshot,
    ProfileAvatarSnapshot, ProfileSnapshot,
};
use garmin_services::{
    Application, AvatarImportRequest, FitImportRequest, FitImportResult, ImportDisposition,
    UserContext,
};
use remoc::{rch, rtc};
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
const DEVICE_OPERATION_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/device-files/acquisition/v1";
const BROWSER_DOWNLOAD_TTL: Duration = Duration::from_secs(60);
const MAX_PENDING_BROWSER_DOWNLOADS: usize = 8;

pub(super) trait Source: Send {
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

pub(super) struct Host {
    source: Arc<Mutex<Box<dyn Source>>>,
    snapshots: Arc<rch::watch::Sender<Vec<DeviceSnapshot>>>,
    browser_downloads: PendingBrowserDownloads,
    application: Application,
}

impl Host {
    pub(super) fn new(mut source: Box<dyn Source>, application: Application) -> Arc<Self> {
        let (snapshots, _receiver) = rch::watch::channel(source.snapshot());
        Arc::new(Self {
            source: Arc::new(Mutex::new(source)),
            snapshots: Arc::new(snapshots),
            browser_downloads: PendingBrowserDownloads::default(),
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

    pub(super) fn take_browser_download(
        &self,
        token: &str,
    ) -> Option<garmin_device::PreparedDeviceBrowserDownload> {
        self.browser_downloads.take(token)
    }
}

#[derive(Clone, Default)]
struct PendingBrowserDownloads {
    entries: Arc<Mutex<HashMap<Uuid, PendingBrowserDownload>>>,
}

struct PendingBrowserDownload {
    expires_at: Instant,
    download: garmin_device::PreparedDeviceBrowserDownload,
}

impl PendingBrowserDownloads {
    fn insert(
        &self,
        download: garmin_device::PreparedDeviceBrowserDownload,
    ) -> Result<DeviceBrowserDownloadTicket, String> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|_, pending| pending.expires_at > now);
        if entries.len() >= MAX_PENDING_BROWSER_DOWNLOADS {
            return Err("too many browser downloads are waiting to be collected".to_owned());
        }
        let pending_bytes = entries.values().try_fold(0_u64, |total, pending| {
            total.checked_add(pending.download.size)
        });
        if pending_bytes.is_none_or(|total| {
            total.saturating_add(download.size) > garmin_device::MAX_BROWSER_TRANSFER_BYTES
        }) {
            return Err("pending browser downloads exceed the 512 MiB limit".to_owned());
        }

        let token = loop {
            let candidate = Uuid::new_v4();
            if !entries.contains_key(&candidate) {
                break candidate;
            }
        };
        let ticket = DeviceBrowserDownloadTicket {
            token: token.to_string(),
            file_name: download.file_name.clone(),
        };
        entries.insert(
            token,
            PendingBrowserDownload {
                expires_at: now + BROWSER_DOWNLOAD_TTL,
                download,
            },
        );
        drop(entries);

        let pending = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(BROWSER_DOWNLOAD_TTL).await;
            pending.expire(token);
        });
        Ok(ticket)
    }

    fn take(&self, token: &str) -> Option<garmin_device::PreparedDeviceBrowserDownload> {
        let token = Uuid::parse_str(token).ok()?;
        let pending = self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&token)?;
        (pending.expires_at > Instant::now()).then_some(pending.download)
    }

    fn expire(&self, token: Uuid) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries
            .get(&token)
            .is_some_and(|pending| pending.expires_at <= Instant::now())
        {
            entries.remove(&token);
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
        Ok(source_browser(Arc::clone(&self.source), request).await)
    }

    async fn upload_device_browser_file(
        &self,
        request: DeviceBrowserUpload,
        contents: rch::io::Receiver,
    ) -> Result<Result<DeviceCatalogSnapshot, String>, rtc::CallError> {
        let result = match receive_browser_upload(&request, contents).await {
            Ok(contents) => source_upload(Arc::clone(&self.source), request, contents).await,
            Err(error) => Err(error),
        };
        Ok(result)
    }

    async fn prepare_device_browser_download(
        &self,
        device_key: String,
        target: DeviceBrowserTarget,
    ) -> Result<Result<DeviceBrowserDownloadTicket, String>, rtc::CallError> {
        let result = source_download(Arc::clone(&self.source), device_key, target)
            .await
            .and_then(|download| self.browser_downloads.insert(download));
        Ok(result)
    }

    async fn device_fit_preview(
        &self,
        device_key: String,
        target: DeviceBrowserTarget,
    ) -> Result<Result<DeviceFitPreview, String>, rtc::CallError> {
        let result = read_device_browser_file(Arc::clone(&self.source), device_key, target)
            .await
            .and_then(|download| device_fit_preview(download.file_name, &download.bytes));
        Ok(result)
    }

    async fn import_device_fit(
        &self,
        user_id: garmin_model::identity::UserId,
        device_key: String,
        target: DeviceBrowserTarget,
    ) -> Result<Result<DeviceFitImportOutcome, String>, rtc::CallError> {
        Ok(import_device_fit(
            &self.application,
            Arc::clone(&self.source),
            UserContext::new(user_id),
            device_key,
            target,
        )
        .await)
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

    async fn import_avatar(
        &self,
        user_id: garmin_model::identity::UserId,
        upload: AvatarUpload,
    ) -> Result<Result<ProfileAvatarSnapshot, String>, rtc::CallError> {
        Ok(import_avatar(&self.application, UserContext::new(user_id), upload).await)
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

fn device_operation_id(
    source: &ProfileSource,
    identity: &SourceIdentity,
    bytes: &[u8],
) -> AcquisitionOperationId {
    let mut digest = blake3::Hasher::new();
    digest.update(DEVICE_OPERATION_ID_DOMAIN_V1);
    digest.update(source.id().to_string().as_bytes());
    digest.update(&(identity.as_str().len() as u64).to_le_bytes());
    digest.update(identity.as_str().as_bytes());
    digest.update(ArtifactDigest::from_bytes(bytes).as_blake3().as_bytes());
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
) -> Result<DeviceCatalogSnapshot, String> {
    let cancellation = CancellationToken::default();
    let progress = ProgressReporter::default().with_cancellation(cancellation.clone());
    let _cancel_on_drop = CancelOnDrop(cancellation);
    tokio::task::spawn_blocking(move || {
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
) -> Result<DeviceCatalogSnapshot, String> {
    let cancellation = CancellationToken::default();
    let progress = ProgressReporter::default().with_cancellation(cancellation.clone());
    let _cancel_on_drop = CancelOnDrop(cancellation);
    tokio::task::spawn_blocking(move || {
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
            segments: track_segments(activity.activity().track()),
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

fn track_segments(
    track: &[garmin_model::activity::TrackPoint],
) -> Vec<Vec<garmin_model::route::Coordinate>> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    for sample in track {
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
    segments
}

async fn import_device_fit(
    application: &Application,
    browser: Arc<Mutex<Box<dyn Source>>>,
    user: UserContext,
    device_key: String,
    target: DeviceBrowserTarget,
) -> Result<DeviceFitImportOutcome, String> {
    let identity = SourceIdentity::from_string(format!(
        "device/{device_key}/{}/{}",
        target.storage_id, target.path
    ))
    .map_err(|error| error.to_string())?;
    let download = read_device_browser_file(browser, device_key, target).await?;
    let source = device_source(user)?;
    let operation = device_operation_id(&source, &identity, &download.bytes);
    let result = application
        .import_fit(FitImportRequest::from_parts(
            user,
            &source,
            identity,
            operation,
            timestamp(SystemTime::now())?,
            &download.bytes,
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

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use super::{
        DeviceBrowserFile, Host, Source, SourceProvider,
        demo::{
            DEVICE_KEY as DEMO_DEVICE_KEY, Provider as DemoProvider, STORAGE_ID as DEMO_STORAGE_ID,
        },
    };
    use garmin_fit::fixture;
    use garmin_progress::ProgressReporter;
    use garmin_service_api::{
        ApplicationService as _, AvatarCrop, AvatarUpload, DeviceBrowserRequest,
        DeviceBrowserTarget, DeviceBrowserUpload, DeviceCatalogEntryKind, DeviceCatalogSnapshot,
        DeviceFitImportOutcome, DeviceSnapshot, InspectionState,
    };
    use garmin_services::Application;
    use image::{DynamicImage, ImageFormat, RgbImage};

    struct FitSource {
        bytes: Vec<u8>,
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
            .take_browser_download(&ticket.token)
            .ok_or_else(|| "prepared download is missing".to_owned())?;
        let bytes = tokio::fs::read(prepared.path())
            .await
            .map_err(|error| error.to_string())?;
        Ok(DeviceBrowserFile {
            file_name: prepared.file_name,
            bytes,
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
                .write_all(&self.bytes)
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
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), Application::new(storage));
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
        let host = Host::new(demo_source(&directory), Application::new(storage));

        let created = host
            .create_profile("Alex Rider".to_owned())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(created.profile().display_name().as_str(), "Alex Rider");
    }

    #[tokio::test]
    async fn explicit_device_browser_returns_only_device_relative_entries() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), Application::new(storage));

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
            entry.path == "Garmin/Activity/History/2026/made-up-morning-ride.fit"
                && entry.kind == DeviceCatalogEntryKind::File
                && entry.size.is_some_and(|size| size > 0)
        }));
    }

    #[tokio::test]
    async fn demo_browser_mutations_refresh_and_download_the_isolated_device() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), Application::new(storage));

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
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), Application::new(storage));
        let request = DeviceBrowserRequest::CreateDirectory {
            device_key: DEMO_DEVICE_KEY.to_owned(),
            storage_id: DEMO_STORAGE_ID.to_owned(),
            parent: "GARMIN-TOOLKIT".into(),
            name: "unsafe".to_owned(),
        };

        let error = demo_browser(&host, request).await.unwrap_err();

        assert!(error.contains("browse/download-only"));
    }

    #[tokio::test]
    async fn browser_avatar_upload_uses_the_application_importer() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let host = Host::new(demo_source(&directory), Application::new(storage));
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

    #[tokio::test]
    async fn device_fit_preview_and_import_use_the_shared_service_contract() {
        let directory = tempfile::tempdir().unwrap();
        let storage = crate::prepare_storage(directory.path()).await.unwrap();
        let bytes = fixture::activity(fixture::Sport::Cycling).unwrap();
        let host = Host::new(Box::new(FitSource { bytes }), Application::new(storage));
        let user = host
            .create_profile("Mock Rider".to_owned())
            .await
            .unwrap()
            .unwrap();
        let target = DeviceBrowserTarget {
            storage_id: "internal".to_owned(),
            path: "Garmin/Activity/activity.fit".into(),
            kind: DeviceCatalogEntryKind::File,
        };

        let preview = host
            .device_fit_preview("device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(preview.file_name, "activity.fit");
        assert_eq!(preview.activities.len(), 1);

        let first = host
            .import_device_fit(user.id(), "device".to_owned(), target.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, DeviceFitImportOutcome::Imported { activities: 1 });
        let second = host
            .import_device_fit(user.id(), "device".to_owned(), target)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second, DeviceFitImportOutcome::Duplicate);
        let profiles = host.profiles().await.unwrap().unwrap();
        let imported = profiles
            .iter()
            .find(|profile| profile.user.id() == user.id())
            .unwrap();
        assert_eq!(imported.activities.len(), 1);
    }
}
