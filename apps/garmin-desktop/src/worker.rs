use std::{
    collections::HashSet,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::Utf8PathBuf;
use eframe::egui;
use futures_lite::future::block_on;
use garmin_device::attachments::Candidate as _;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, SourceIdentity},
    identity::{DisplayName, ProfilePreferences, Source, SourceId, User},
    observation::ObservationId,
    value::Timestamp,
};
use garmin_progress::{CancellationToken, ProgressReporter};
use garmin_services::{
    Application, AvatarImportRequest, FitImportRequest, FitImportResult, ImportDisposition,
    UserContext,
};
use uuid::Uuid;
use walkdir::WalkDir;

const SOURCE_LABEL: &str = "Desktop files";
const SOURCE_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/desktop-files/source/v1";
const OPERATION_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/desktop-files/acquisition/v1";
const AVATAR_SOURCE_LABEL: &str = "Desktop profile pictures";
const AVATAR_SOURCE_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/desktop-profile-pictures/source/v1";
const AVATAR_OPERATION_ID_DOMAIN_V1: &[u8] =
    b"garmin-toolkit/desktop-profile-pictures/acquisition/v1";
const DEVICE_SOURCE_LABEL: &str = "Connected Garmin device";
const DEVICE_SOURCE_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/device-files/source/v1";
const DEVICE_OPERATION_ID_DOMAIN_V1: &[u8] = b"garmin-toolkit/device-files/acquisition/v1";

pub struct Worker {
    commands: Sender<Command>,
    events: Receiver<Event>,
    abort: CancellationToken,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(application: Application, context: egui::Context) -> std::io::Result<Self> {
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        let abort = CancellationToken::default();
        let worker_abort = abort.clone();
        let join = std::thread::Builder::new()
            .name("garmin-toolkit-application".to_owned())
            .spawn(move || {
                run(application, &command_rx, &event_tx, &context, &worker_abort);
            })?;
        let worker = Self {
            commands,
            events,
            abort,
            join: Some(join),
        };
        worker.reload();
        Ok(worker)
    }

    pub fn reload(&self) {
        let _ignored = self.commands.send(Command::Reload);
    }

    pub fn create_profile(&self, display_name: DisplayName) {
        let _ignored = self.commands.send(Command::CreateProfile(display_name));
    }

    pub fn update_preferences(&self, user: UserContext, preferences: ProfilePreferences) {
        let _ignored = self
            .commands
            .send(Command::UpdatePreferences { user, preferences });
    }

    pub fn import(&self, user: UserContext, paths: Vec<PathBuf>) {
        let _ignored = self.commands.send(Command::Import { user, paths });
    }

    pub fn load_activity(&self, user: UserContext, observation_id: ObservationId) {
        let _ignored = self.commands.send(Command::LoadActivity {
            user,
            observation_id,
        });
    }

    pub fn import_avatar(
        &self,
        user: UserContext,
        path: PathBuf,
        bytes: Vec<u8>,
        crop: garmin_importer::AvatarCrop,
    ) {
        let _ignored = self.commands.send(Command::ImportAvatar {
            user,
            path,
            bytes,
            crop,
        });
    }

    pub fn browse_device(&self, candidate: crate::device_backend::Candidate) {
        let _ignored = self.commands.send(Command::BrowseDevice(candidate));
    }

    pub fn operate_device(
        &self,
        candidate: crate::device_backend::Candidate,
        operation: DeviceBrowserOperation,
    ) {
        let _ignored = self
            .commands
            .send(Command::OperateDevice(candidate, operation));
    }

    pub fn drain(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }

    pub fn abort(&self) {
        self.abort.cancel();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.abort();
        let _ignored = self.commands.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ignored = join.join();
        }
    }
}

enum Command {
    Reload,
    CreateProfile(DisplayName),
    UpdatePreferences {
        user: UserContext,
        preferences: ProfilePreferences,
    },
    Import {
        user: UserContext,
        paths: Vec<PathBuf>,
    },
    LoadActivity {
        user: UserContext,
        observation_id: ObservationId,
    },
    ImportAvatar {
        user: UserContext,
        path: PathBuf,
        bytes: Vec<u8>,
        crop: garmin_importer::AvatarCrop,
    },
    BrowseDevice(crate::device_backend::Candidate),
    OperateDevice(crate::device_backend::Candidate, DeviceBrowserOperation),
    Shutdown,
}

pub enum Event {
    Reloaded(Result<Vec<ProfileData>, String>),
    ProfileCreated(Result<(garmin_model::identity::UserId, Vec<ProfileData>), String>),
    ProfileUpdated(Result<Vec<ProfileData>, String>),
    AvatarUpdated(Result<Vec<ProfileData>, String>),
    DeviceCatalog {
        key: String,
        result: Result<garmin_device::DeviceCatalog, String>,
    },
    DeviceBrowser {
        key: String,
        kind: DeviceBrowserOperationKind,
        result: Result<DeviceBrowserOutcome, String>,
    },
    ImportStarted {
        total: usize,
    },
    ImportItem(ImportItem),
    ImportFinished(Result<Vec<ProfileData>, String>),
    ActivityLoaded {
        observation_id: ObservationId,
        result: Result<Option<Box<garmin_services::ActivityDetails>>, String>,
    },
}

pub struct ProfileData {
    pub user: User,
    pub activities: Vec<garmin_services::ActivityPreview>,
    pub avatar: Option<garmin_services::ProfileAvatar>,
}

pub struct ImportItem {
    pub path: PathBuf,
    pub outcome: ImportOutcome,
}

pub enum ImportOutcome {
    Imported(usize),
    Duplicate,
    Rejected(String),
    Failed(String),
}

pub enum DeviceBrowserOperation {
    OpenFit(garmin_device::DeviceBrowserTarget),
    ImportFit {
        target: garmin_device::DeviceBrowserTarget,
        user: UserContext,
    },
    Download {
        target: garmin_device::DeviceBrowserTarget,
        destination: PathBuf,
    },
    Upload {
        storage_id: String,
        directory: Utf8PathBuf,
        source: PathBuf,
    },
    CreateDirectory {
        storage_id: String,
        parent: Utf8PathBuf,
        name: String,
    },
    Remove(garmin_device::DeviceBrowserTarget),
}

impl DeviceBrowserOperation {
    pub(crate) const fn kind(&self) -> DeviceBrowserOperationKind {
        match self {
            Self::OpenFit(_) => DeviceBrowserOperationKind::OpenFit,
            Self::ImportFit { .. } => DeviceBrowserOperationKind::ImportFit,
            Self::Download { .. } => DeviceBrowserOperationKind::Download,
            Self::Upload { .. } => DeviceBrowserOperationKind::Upload,
            Self::CreateDirectory { .. } => DeviceBrowserOperationKind::CreateDirectory,
            Self::Remove(_) => DeviceBrowserOperationKind::Remove,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceBrowserOperationKind {
    OpenFit,
    ImportFit,
    Download,
    Upload,
    CreateDirectory,
    Remove,
}

pub enum DeviceBrowserOutcome {
    FitPreview {
        target: garmin_service_api::DeviceBrowserTarget,
        preview: garmin_service_api::DeviceFitPreview,
    },
    FitImported {
        item: ImportItem,
        profiles: Vec<ProfileData>,
    },
    Downloaded(PathBuf),
    Refreshed(garmin_device::DeviceCatalog),
}

fn run(
    application: Application,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    context: &egui::Context,
    abort: &CancellationToken,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("device browser runtime must start");
    while !abort.is_cancelled() {
        let Ok(command) = commands.recv() else {
            break;
        };
        match command {
            Command::Reload => {
                if !emit(
                    events,
                    context,
                    Event::Reloaded(block_on(load_profiles(&application))),
                ) {
                    break;
                }
            }
            Command::CreateProfile(display_name) => {
                let result = block_on(application.create_profile(display_name))
                    .map_err(|error| error.to_string())
                    .and_then(|user| {
                        block_on(load_profiles(&application)).map(|profiles| (user.id(), profiles))
                    });
                if !emit(events, context, Event::ProfileCreated(result)) {
                    break;
                }
            }
            Command::UpdatePreferences { user, preferences } => {
                let result = block_on(application.update_profile_preferences(user, preferences))
                    .map_err(|error| error.to_string())
                    .and_then(|_| block_on(load_profiles(&application)));
                if !emit(events, context, Event::ProfileUpdated(result)) {
                    break;
                }
            }
            Command::Import { user, paths } => {
                if !import_paths(&application, user, paths, events, context, abort) {
                    break;
                }
            }
            Command::LoadActivity {
                user,
                observation_id,
            } => {
                let result = block_on(application.activity(user, observation_id))
                    .map(|details| details.map(Box::new))
                    .map_err(|error| error.to_string());
                if !emit(
                    events,
                    context,
                    Event::ActivityLoaded {
                        observation_id,
                        result,
                    },
                ) {
                    break;
                }
            }
            Command::ImportAvatar {
                user,
                path,
                bytes,
                crop,
            } => {
                let result = block_on(import_avatar(&application, user, &path, &bytes, crop))
                    .and_then(|()| block_on(load_profiles(&application)));
                if !emit(events, context, Event::AvatarUpdated(result)) {
                    break;
                }
            }
            Command::BrowseDevice(candidate) => {
                let key = candidate.key().to_owned();
                let progress = ProgressReporter::default().with_cancellation(abort.clone());
                let result = candidate.browse_with_progress(&progress);
                if !emit(events, context, Event::DeviceCatalog { key, result }) {
                    break;
                }
            }
            Command::OperateDevice(candidate, operation) => {
                let key = candidate.key().to_owned();
                let kind = operation.kind();
                let progress = ProgressReporter::default().with_cancellation(abort.clone());
                let result =
                    operate_device(&runtime, &application, &candidate, operation, &progress);
                if !emit(events, context, Event::DeviceBrowser { key, kind, result }) {
                    break;
                }
            }
            Command::Shutdown => break,
        }
    }
    block_on(application.close());
}

fn operate_device(
    runtime: &tokio::runtime::Runtime,
    application: &Application,
    candidate: &crate::device_backend::Candidate,
    operation: DeviceBrowserOperation,
    progress: &ProgressReporter,
) -> Result<DeviceBrowserOutcome, String> {
    let catalog = candidate.browse_with_progress(progress)?;
    let device = crate::device_backend::transport(candidate);
    match operation {
        DeviceBrowserOperation::OpenFit(target) => {
            open_device_fit(runtime, &device, &catalog, target, progress)
        }
        DeviceBrowserOperation::ImportFit { target, user } => import_device_fit(
            runtime,
            DeviceFitImportRequest {
                application,
                device_key: candidate.key(),
                device: &device,
                catalog: &catalog,
                target: &target,
                user,
            },
            progress,
        ),
        DeviceBrowserOperation::Download {
            target,
            destination,
        } => {
            let download = runtime
                .block_on(garmin_device::prepare_browser_download(
                    &device, &catalog, &target, progress,
                ))
                .map_err(|error| error.to_string())?;
            persist_download(&destination, download.path())?;
            Ok(DeviceBrowserOutcome::Downloaded(destination))
        }
        DeviceBrowserOperation::Upload {
            storage_id,
            directory,
            source,
        } => {
            let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
            if !metadata.file_type().is_file() {
                return Err("the selected upload is not a regular file".to_owned());
            }
            if metadata.len() > garmin_device::MAX_BROWSER_TRANSFER_BYTES {
                return Err(garmin_device::DeviceBrowserOperationError::TransferLimit.to_string());
            }
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "the selected upload has an invalid file name".to_owned())?;
            runtime
                .block_on(garmin_device::upload_browser_file_from_path(
                    &device,
                    &catalog,
                    garmin_device::BrowserFileUpload {
                        storage_id: &storage_id,
                        directory: &directory,
                        file_name: name,
                        source: &source,
                        size: metadata.len(),
                    },
                    progress,
                ))
                .map_err(|error| error.to_string())?;
            candidate
                .browse_with_progress(progress)
                .map(DeviceBrowserOutcome::Refreshed)
        }
        DeviceBrowserOperation::CreateDirectory {
            storage_id,
            parent,
            name,
        } => {
            runtime
                .block_on(garmin_device::create_browser_directory_with_progress(
                    &device,
                    &catalog,
                    &storage_id,
                    &parent,
                    &name,
                    progress,
                ))
                .map_err(|error| error.to_string())?;
            candidate
                .browse_with_progress(progress)
                .map(DeviceBrowserOutcome::Refreshed)
        }
        DeviceBrowserOperation::Remove(target) => {
            runtime
                .block_on(garmin_device::remove_browser_target_with_progress(
                    &device, &catalog, &target, progress,
                ))
                .map_err(|error| error.to_string())?;
            candidate
                .browse_with_progress(progress)
                .map(DeviceBrowserOutcome::Refreshed)
        }
    }
}

fn open_device_fit(
    runtime: &tokio::runtime::Runtime,
    device: &crate::device_backend::Device,
    catalog: &garmin_device::DeviceCatalog,
    target: garmin_device::DeviceBrowserTarget,
    progress: &ProgressReporter,
) -> Result<DeviceBrowserOutcome, String> {
    let download = runtime
        .block_on(garmin_device::prepare_browser_download(
            device, catalog, &target, progress,
        ))
        .map_err(|error| error.to_string())?;
    let bytes = fs::read(download.path()).map_err(|error| error.to_string())?;
    let preview = device_fit_preview(download.file_name, &bytes)?;
    Ok(DeviceBrowserOutcome::FitPreview {
        target: service_browser_target(target),
        preview,
    })
}

#[derive(Clone, Copy)]
struct DeviceFitImportRequest<'a> {
    application: &'a Application,
    device_key: &'a str,
    device: &'a crate::device_backend::Device,
    catalog: &'a garmin_device::DeviceCatalog,
    target: &'a garmin_device::DeviceBrowserTarget,
    user: UserContext,
}

fn import_device_fit(
    runtime: &tokio::runtime::Runtime,
    request: DeviceFitImportRequest<'_>,
    progress: &ProgressReporter,
) -> Result<DeviceBrowserOutcome, String> {
    let DeviceFitImportRequest {
        application,
        device_key,
        device,
        catalog,
        target,
        user,
    } = request;
    let download = runtime
        .block_on(garmin_device::prepare_browser_download(
            device, catalog, target, progress,
        ))
        .map_err(|error| error.to_string())?;
    let bytes = fs::read(download.path()).map_err(|error| error.to_string())?;
    let source = source_from_domain(user, DEVICE_SOURCE_ID_DOMAIN_V1, DEVICE_SOURCE_LABEL)?;
    let identity = device_source_identity(device_key, target)?;
    let outcome = import_bytes(
        application,
        user,
        &source,
        identity,
        DEVICE_OPERATION_ID_DOMAIN_V1,
        &bytes,
    )?;
    let item = ImportItem {
        path: PathBuf::from(target.path.as_str()),
        outcome,
    };
    let profiles = block_on(load_profiles(application))?;
    Ok(DeviceBrowserOutcome::FitImported { item, profiles })
}

fn persist_download(path: &Path, source: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the download destination has no parent directory".to_owned())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    let mut source = fs::File::open(source).map_err(|error| error.to_string())?;
    io::copy(&mut source, &mut temporary)
        .map(|_| ())
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| error.error.to_string())?;
    Ok(())
}

async fn load_profiles(application: &Application) -> Result<Vec<ProfileData>, String> {
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
        profiles.push(ProfileData {
            user,
            activities,
            avatar,
        });
    }
    Ok(profiles)
}

async fn import_avatar(
    application: &Application,
    user: UserContext,
    path: &Path,
    bytes: &[u8],
    crop: garmin_importer::AvatarCrop,
) -> Result<(), String> {
    let source = avatar_source(user)?;
    let path = path
        .to_str()
        .ok_or_else(|| "profile-picture path is not valid UTF-8".to_owned())?;
    let identity =
        SourceIdentity::from_string(path.to_owned()).map_err(|error| error.to_string())?;
    let operation = avatar_operation_id(&source, &identity, bytes, crop);
    application
        .import_avatar(AvatarImportRequest::from_parts(
            user,
            &source,
            identity,
            operation,
            timestamp(SystemTime::now())?,
            bytes,
            crop,
        ))
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn import_paths(
    application: &Application,
    user: UserContext,
    paths: Vec<PathBuf>,
    events: &Sender<Event>,
    context: &egui::Context,
    abort: &CancellationToken,
) -> bool {
    let Some(candidates) = candidates(paths, abort) else {
        return false;
    };
    if !emit(
        events,
        context,
        Event::ImportStarted {
            total: candidates.len(),
        },
    ) {
        return false;
    }
    let source = match source(user) {
        Ok(source) => source,
        Err(error) => {
            for candidate in candidates {
                if abort.is_cancelled() {
                    return false;
                }
                let path = candidate.path().to_owned();
                if !emit(
                    events,
                    context,
                    Event::ImportItem(ImportItem {
                        path,
                        outcome: ImportOutcome::Failed(error.clone()),
                    }),
                ) {
                    return false;
                }
            }
            return emit(
                events,
                context,
                Event::ImportFinished(block_on(load_profiles(application))),
            );
        }
    };

    for candidate in candidates {
        if abort.is_cancelled() {
            return false;
        }
        let item = match candidate {
            Candidate::File(path) => import_file(application, user, &source, path),
            Candidate::Failed { path, reason } => ImportItem {
                path,
                outcome: ImportOutcome::Failed(reason),
            },
        };
        if abort.is_cancelled() || !emit(events, context, Event::ImportItem(item)) {
            return false;
        }
    }
    emit(
        events,
        context,
        Event::ImportFinished(block_on(load_profiles(application))),
    )
}

fn import_file(
    application: &Application,
    user: UserContext,
    source: &Source,
    path: PathBuf,
) -> ImportItem {
    let outcome =
        read_import(application, user, source, &path).unwrap_or_else(ImportOutcome::Failed);
    ImportItem { path, outcome }
}

fn read_import(
    application: &Application,
    user: UserContext,
    source: &Source,
    path: &Path,
) -> Result<ImportOutcome, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let path = path
        .to_str()
        .ok_or_else(|| "FIT path is not valid UTF-8".to_owned())?;
    let source_identity =
        SourceIdentity::from_string(path.to_owned()).map_err(|error| error.to_string())?;
    import_bytes(
        application,
        user,
        source,
        source_identity,
        OPERATION_ID_DOMAIN_V1,
        &bytes,
    )
}

fn import_bytes(
    application: &Application,
    user: UserContext,
    source: &Source,
    source_identity: SourceIdentity,
    operation_domain: &[u8],
    bytes: &[u8],
) -> Result<ImportOutcome, String> {
    let acquired_at = timestamp(SystemTime::now())?;
    let operation_id = operation_id_from_domain(operation_domain, source, &source_identity, bytes);
    let result = block_on(application.import_fit(FitImportRequest::from_parts(
        user,
        source,
        source_identity,
        operation_id,
        acquired_at,
        bytes,
    )))
    .map_err(|error| error.to_string())?;
    if result.disposition() == ImportDisposition::Existing {
        return Ok(ImportOutcome::Duplicate);
    }
    Ok(match result {
        FitImportResult::Imported { observations, .. } => {
            ImportOutcome::Imported(observations.len())
        }
        FitImportResult::Rejected { failure, .. } => ImportOutcome::Rejected(failure.to_string()),
    })
}

fn source(user: UserContext) -> Result<Source, String> {
    source_from_domain(user, SOURCE_ID_DOMAIN_V1, SOURCE_LABEL)
}

fn avatar_source(user: UserContext) -> Result<Source, String> {
    source_from_domain(user, AVATAR_SOURCE_ID_DOMAIN_V1, AVATAR_SOURCE_LABEL)
}

fn source_from_domain(
    user: UserContext,
    domain: &[u8],
    label: &'static str,
) -> Result<Source, String> {
    let mut digest = blake3::Hasher::new();
    digest.update(domain);
    digest.update(user.user_id().to_string().as_bytes());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, digest.finalize().as_bytes());
    let label = label
        .parse()
        .map_err(|error: garmin_model::identity::Error| error.to_string())?;
    Ok(Source::from_parts(
        SourceId::from_u128(id.as_u128()),
        user.user_id(),
        label,
        None,
    ))
}

fn avatar_operation_id(
    source: &Source,
    identity: &SourceIdentity,
    bytes: &[u8],
    crop: garmin_importer::AvatarCrop,
) -> AcquisitionOperationId {
    let mut digest = blake3::Hasher::new();
    digest.update(AVATAR_OPERATION_ID_DOMAIN_V1);
    digest.update(source.id().to_string().as_bytes());
    digest.update(&(identity.as_str().len() as u64).to_le_bytes());
    digest.update(identity.as_str().as_bytes());
    digest.update(ArtifactDigest::from_bytes(bytes).as_blake3().as_bytes());
    digest.update(&crop.left().to_le_bytes());
    digest.update(&crop.top().to_le_bytes());
    digest.update(&crop.edge().to_le_bytes());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, digest.finalize().as_bytes());
    AcquisitionOperationId::from_u128(id.as_u128())
}

#[cfg(test)]
fn operation_id(
    source: &Source,
    identity: &SourceIdentity,
    bytes: &[u8],
) -> AcquisitionOperationId {
    operation_id_from_domain(OPERATION_ID_DOMAIN_V1, source, identity, bytes)
}

fn operation_id_from_domain(
    domain: &[u8],
    source: &Source,
    identity: &SourceIdentity,
    bytes: &[u8],
) -> AcquisitionOperationId {
    let mut digest = blake3::Hasher::new();
    digest.update(domain);
    digest.update(source.id().to_string().as_bytes());
    digest.update(&(identity.as_str().len() as u64).to_le_bytes());
    digest.update(identity.as_str().as_bytes());
    digest.update(ArtifactDigest::from_bytes(bytes).as_blake3().as_bytes());
    let id = Uuid::new_v5(&Uuid::NAMESPACE_URL, digest.finalize().as_bytes());
    AcquisitionOperationId::from_u128(id.as_u128())
}

fn device_source_identity(
    device_key: &str,
    target: &garmin_device::DeviceBrowserTarget,
) -> Result<SourceIdentity, String> {
    SourceIdentity::from_string(format!(
        "device/{device_key}/{}/{}",
        target.storage_id, target.path
    ))
    .map_err(|error| error.to_string())
}

fn service_browser_target(
    target: garmin_device::DeviceBrowserTarget,
) -> garmin_service_api::DeviceBrowserTarget {
    garmin_service_api::DeviceBrowserTarget {
        storage_id: target.storage_id,
        path: target.path,
        kind: match target.kind {
            garmin_device::DeviceCatalogEntryKind::Directory => {
                garmin_service_api::DeviceCatalogEntryKind::Directory
            }
            garmin_device::DeviceCatalogEntryKind::File => {
                garmin_service_api::DeviceCatalogEntryKind::File
            }
        },
    }
}

fn device_fit_preview(
    file_name: String,
    bytes: &[u8],
) -> Result<garmin_service_api::DeviceFitPreview, String> {
    let import = garmin_fit::normalize_activities(bytes).map_err(|error| error.to_string())?;
    let activities = import
        .normalized_activities()
        .map(|activity| {
            let source = activity
                .creator()
                .product_name()
                .map_or_else(|| "FIT".to_owned(), ToString::to_string);
            garmin_service_api::DeviceFitPreviewActivity {
                source,
                summary: activity.activity().summary(),
                recording: garmin_service_api::ActivityRecordingSnapshot::from(activity.activity()),
            }
        })
        .collect::<Vec<_>>();
    if activities.is_empty() {
        return Err("the FIT file does not contain an activity".to_owned());
    }
    Ok(garmin_service_api::DeviceFitPreview {
        file_name,
        activities,
    })
}

fn timestamp(time: SystemTime) -> Result<Timestamp, String> {
    let milliseconds = time
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let milliseconds = i64::try_from(milliseconds).map_err(|error| error.to_string())?;
    Timestamp::from_unix_milliseconds(milliseconds).map_err(|error| error.to_string())
}

fn candidates(paths: Vec<PathBuf>, abort: &CancellationToken) -> Option<Vec<Candidate>> {
    if abort.is_cancelled() {
        return None;
    }
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        if abort.is_cancelled() {
            return None;
        }
        if path.is_dir() {
            let before = candidates.len();
            for entry in WalkDir::new(&path).follow_links(false) {
                if abort.is_cancelled() {
                    return None;
                }
                match entry {
                    Ok(entry) if entry.file_type().is_file() && is_fit(entry.path()) => {
                        push_file(&mut candidates, &mut seen, entry.into_path());
                    }
                    Ok(_) => {}
                    Err(error) => candidates.push(Candidate::Failed {
                        path: error.path().unwrap_or(&path).to_owned(),
                        reason: error.to_string(),
                    }),
                }
            }
            if candidates.len() == before {
                candidates.push(Candidate::Failed {
                    path,
                    reason: "directory contains no FIT files".to_owned(),
                });
            }
        } else if is_fit(&path) {
            push_file(&mut candidates, &mut seen, path);
        } else {
            candidates.push(Candidate::Failed {
                path,
                reason: "not a FIT file or directory".to_owned(),
            });
        }
    }
    candidates.sort_by(|left, right| left.path().cmp(right.path()));
    Some(candidates)
}

fn push_file(candidates: &mut Vec<Candidate>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        candidates.push(Candidate::File(path));
    }
}

fn is_fit(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("fit"))
}

enum Candidate {
    File(PathBuf),
    Failed { path: PathBuf, reason: String },
}

impl Candidate {
    fn path(&self) -> &Path {
        match self {
            Self::File(path) | Self::Failed { path, .. } => path,
        }
    }
}

fn emit(events: &Sender<Event>, context: &egui::Context, event: Event) -> bool {
    if events.send(event).is_err() {
        return false;
    }
    context.request_repaint();
    true
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use garmin_fit::fixture;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn path_expansion_is_recursive_filtered_sorted_and_deduplicated() -> Result<(), Box<dyn Error>>
    {
        let root = tempdir()?;
        let nested = root.path().join("nested");
        fs::create_dir(&nested)?;
        fs::write(root.path().join("b.FIT"), b"b")?;
        fs::write(nested.join("a.fit"), b"a")?;
        fs::write(nested.join("ignored.txt"), b"x")?;

        let abort = CancellationToken::default();
        let found = candidates(vec![root.path().to_owned(), nested.join("a.fit")], &abort)
            .expect("the test never requests cancellation");
        let paths = found.iter().map(Candidate::path).collect::<Vec<_>>();

        assert_eq!(paths, [root.path().join("b.FIT"), nested.join("a.fit")]);
        assert!(
            found
                .iter()
                .all(|candidate| matches!(candidate, Candidate::File(_)))
        );
        Ok(())
    }

    #[test]
    fn path_expansion_honors_cancellation() {
        let abort = CancellationToken::default();
        abort.cancel();

        assert!(candidates(Vec::new(), &abort).is_none());
    }

    #[test]
    fn download_is_persisted_at_the_selected_destination() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let destination = root.path().join("selected-download.fit");

        let source = root.path().join("prepared-download");
        fs::write(&source, b"downloaded device bytes")?;
        persist_download(&destination, &source)?;

        assert_eq!(fs::read(destination)?, b"downloaded device bytes");
        Ok(())
    }

    #[test]
    fn operation_identity_changes_with_provenance_or_bytes() -> Result<(), Box<dyn Error>> {
        let user = UserContext::new(garmin_model::identity::UserId::new_v4());
        let source = source(user)?;
        let first = "first.fit".parse()?;
        let second = "second.fit".parse()?;

        assert_eq!(
            operation_id(&source, &first, b"same"),
            operation_id(&source, &first, b"same")
        );
        assert_ne!(
            operation_id(&source, &first, b"same"),
            operation_id(&source, &second, b"same")
        );
        assert_ne!(
            operation_id(&source, &first, b"same"),
            operation_id(&source, &first, b"changed")
        );
        Ok(())
    }

    #[test]
    fn device_fit_preview_normalizes_activity_data() -> Result<(), Box<dyn Error>> {
        let bytes = fixture::activity(fixture::Sport::Cycling)?;

        let preview = device_fit_preview("activity.fit".to_owned(), &bytes)?;

        assert_eq!(preview.file_name, "activity.fit");
        assert_eq!(preview.activities.len(), 1);
        assert!(!preview.activities[0].source.is_empty());
        Ok(())
    }

    #[test]
    fn file_import_is_persisted_and_repeatable() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage =
                garmin_storage::Storage::open(root.path().join("storage.sqlite3")).await?;
            let application = Application::new(storage);
            let user = application.create_profile("Rider".parse()?).await?;
            let context = UserContext::new(user.id());
            let source = source(context)?;
            let path = root.path().join("activity.fit");
            fs::write(&path, fixture::activity(fixture::Sport::Cycling)?)?;

            assert!(matches!(
                read_import(&application, context, &source, &path)?,
                ImportOutcome::Imported(1)
            ));
            assert!(matches!(
                read_import(&application, context, &source, &path)?,
                ImportOutcome::Duplicate
            ));
            assert_eq!(application.activities(context).await?.len(), 1);

            application.close().await;
            Ok(())
        })
    }
}
