use std::{
    collections::HashSet,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::Utf8PathBuf;
use eframe::egui;
use futures_lite::future::block_on;
use garmin_device::attachments::Candidate as _;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, ByteCount, SourceIdentity},
    identity::{DisplayName, ProfilePreferences, Source, SourceId, User},
    observation::ObservationId,
    value::Timestamp,
};
use garmin_progress::{CancellationToken, ProgressReporter};
use garmin_services::deployment::Deployment;
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
    commands: Sender<(Uuid, Command)>,
    events: Receiver<(Uuid, Event)>,
    epoch: Uuid,
    abort: CancellationToken,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(
        deployment: Arc<Deployment>,
        context: egui::Context,
        mutations: Arc<garmin_services::maps::MutationLocks>,
    ) -> std::io::Result<Self> {
        let epoch = deployment.epoch();
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        let abort = CancellationToken::default();
        let worker_abort = abort.clone();
        let jobs = deployment.start_jobs()?;
        let join = std::thread::Builder::new()
            .name("garmin-toolkit-application".to_owned())
            .spawn(move || {
                let _jobs = jobs;
                run(
                    &deployment,
                    &command_rx,
                    &event_tx,
                    &context,
                    &worker_abort,
                    &mutations,
                );
            })?;
        let worker = Self {
            commands,
            events,
            epoch,
            abort,
            join: Some(join),
        };
        worker.reload();
        Ok(worker)
    }

    fn send(&self, command: Command) {
        let _ignored = self.commands.send((self.epoch, command));
    }

    pub fn set_epoch(&mut self, epoch: Uuid) {
        self.epoch = epoch;
    }

    pub fn reload(&self) {
        self.send(Command::Reload);
    }

    pub fn create_profile(&self, display_name: DisplayName) {
        self.send(Command::CreateProfile(display_name));
    }

    pub fn update_preferences(&self, user: UserContext, preferences: ProfilePreferences) {
        self.send(Command::UpdateProfile {
            user,
            change: ProfileChange::Preferences(preferences),
        });
    }

    pub fn connect_sync(
        &self,
        user: UserContext,
        request: garmin_model::garmin_connect::sync::Request,
    ) {
        self.send(Command::ConnectSync { user, request });
    }

    pub fn read_connect_cache(
        &self,
        user: UserContext,
        start: u32,
        filters: garmin_model::garmin_connect::ActivityFilters,
        revision: garmin_model::garmin_connect::sync::Revision,
    ) {
        self.send(Command::ReadConnectCache {
            request: ConnectCacheRead {
                user: user.user_id(),
                start,
                filters,
                revision,
            },
        });
    }

    pub fn read_trends(&self, user: UserContext, revision: garmin_ui::trends::ReadRequest) {
        self.send(Command::ReadTrends { user, revision });
    }

    pub fn set_garmin_connect_key(
        &self,
        user: UserContext,
        key: Option<garmin_model::garmin_connect::PersonalApiKey>,
    ) {
        self.send(Command::GarminConnect {
            user,
            action: ConnectAction::Save(key),
        });
    }

    pub fn connect_activity(
        &self,
        user: UserContext,
        request: garmin_model::garmin_connect::ActivityRequest,
    ) -> CancellationToken {
        let cancellation = CancellationToken::default();
        self.send(Command::ConnectActivity(ConnectActivityCommand {
            user,
            request,
            cancellation: cancellation.clone(),
        }));
        cancellation
    }

    pub fn check_garmin_connect(&self, user: UserContext) {
        self.send(Command::GarminConnect {
            user,
            action: ConnectAction::Test,
        });
    }

    pub fn update_accent(&self, user: UserContext, accent: Option<garmin_color::Color>) {
        self.send(Command::UpdateProfile {
            user,
            change: ProfileChange::Accent(accent),
        });
    }

    pub fn import(&self, user: UserContext, paths: Vec<PathBuf>) {
        self.send(Command::Import { user, paths });
    }

    pub fn preview_activity_package(&self, user: UserContext, path: PathBuf) {
        self.send(Command::ActivityPackage(PackageCommand::Preview(
            user, path,
        )));
    }

    pub fn confirm_activity_package(
        &self,
        plan: garmin_services::activity_export::PreparedActivityPackage,
    ) {
        self.send(Command::ActivityPackage(PackageCommand::Confirm(plan)));
    }

    pub fn load_activity(&self, user: UserContext, observation_id: ObservationId) {
        self.send(Command::LoadActivity {
            user,
            observation_id,
        });
    }

    pub fn export_activity(
        &self,
        user: UserContext,
        observation_id: ObservationId,
        destination: PathBuf,
    ) {
        self.send(Command::ExportActivity {
            user,
            observation_id,
            destination,
        });
    }

    pub fn import_avatar(
        &self,
        user: UserContext,
        path: PathBuf,
        bytes: Vec<u8>,
        crop: garmin_importer::AvatarCrop,
    ) {
        self.send(Command::ImportAvatar {
            user,
            path,
            bytes,
            crop,
        });
    }

    pub fn browse_device(&self, candidate: crate::device_backend::Candidate) {
        self.send(Command::BrowseDevice(candidate));
    }

    pub fn operate_device(
        &self,
        candidate: crate::device_backend::Candidate,
        operation: DeviceBrowserOperation,
    ) {
        self.send(Command::OperateDevice(candidate, operation));
    }

    pub fn drain(&self) -> impl Iterator<Item = Event> + '_ {
        self.events
            .try_iter()
            .filter_map(|(epoch, event)| (epoch == self.epoch).then_some(event))
    }

    pub fn abort(&self) {
        self.abort.cancel();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.abort();
        self.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ignored = join.join();
        }
    }
}

enum ConnectAction {
    Save(Option<garmin_model::garmin_connect::PersonalApiKey>),
    Test,
}

struct ConnectActivityCommand {
    user: UserContext,
    request: garmin_model::garmin_connect::ActivityRequest,
    cancellation: CancellationToken,
}

#[derive(Clone, Copy)]
enum ProfileChange {
    Preferences(ProfilePreferences),
    Accent(Option<garmin_color::Color>),
}

enum Command {
    ReadTrends {
        user: UserContext,
        revision: garmin_ui::trends::ReadRequest,
    },
    ReadConnectCache {
        request: ConnectCacheRead,
    },
    ConnectSync {
        user: UserContext,
        request: garmin_model::garmin_connect::sync::Request,
    },
    ConnectActivity(ConnectActivityCommand),
    GarminConnect {
        user: UserContext,
        action: ConnectAction,
    },
    Reload,
    CreateProfile(DisplayName),
    UpdateProfile {
        user: UserContext,
        change: ProfileChange,
    },
    Import {
        user: UserContext,
        paths: Vec<PathBuf>,
    },
    ActivityPackage(PackageCommand),
    LoadActivity {
        user: UserContext,
        observation_id: ObservationId,
    },
    ExportActivity {
        user: UserContext,
        observation_id: ObservationId,
        destination: PathBuf,
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

pub struct ConnectCacheRead {
    pub user: garmin_model::identity::UserId,
    pub start: u32,
    pub filters: garmin_model::garmin_connect::ActivityFilters,
    pub revision: garmin_model::garmin_connect::sync::Revision,
}

pub enum Event {
    Trends {
        user: garmin_model::identity::UserId,
        revision: garmin_ui::trends::ReadRequest,
        result: Result<Option<garmin_model::garmin_connect::trends::History>, String>,
    },
    TrendsImportRevision(
        garmin_model::identity::UserId,
        Option<garmin_model::artifact::ArtifactDigest>,
    ),
    ConnectSyncFailure {
        user: garmin_model::identity::UserId,
        reason: String,
    },
    CachedConnectPage {
        request: ConnectCacheRead,
        page: Option<garmin_model::garmin_connect::ActivityResponse>,
    },
    ConnectSyncStates(
        Vec<(
            garmin_model::identity::UserId,
            garmin_model::garmin_connect::sync::Snapshot,
        )>,
    ),
    ConnectLoadProgress {
        user: garmin_model::identity::UserId,
        progress: garmin_model::garmin_connect::LoadProgress,
    },
    ConnectActivity {
        user: garmin_model::identity::UserId,
        result: Result<
            garmin_service_api::ConnectActivityResult,
            garmin_model::garmin_connect::ActivityFailure,
        >,
    },
    GarminConnectChecked {
        user: garmin_model::identity::UserId,
        result: Result<Option<garmin_model::garmin_connect::AccountFacts>, String>,
    },
    Unavailable(String),
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
    ActivityPackagePreview(
        Result<garmin_services::activity_export::PreparedActivityPackage, String>,
    ),
    ActivityPackageImported(Result<Vec<ProfileData>, String>),
    ActivityLoaded {
        observation_id: ObservationId,
        result: Result<Option<Box<garmin_services::ActivityDetails>>, String>,
    },
    ActivityExported(Result<bool, String>),
}

pub struct ProfileData {
    pub connect_sync: garmin_model::garmin_connect::sync::Snapshot,
    pub garmin_connect_configured: bool,
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
    RemoveMany(Vec<garmin_device::DeviceBrowserTarget>),
    Rename {
        target: garmin_device::DeviceBrowserTarget,
        name: String,
    },
    Transfer {
        targets: Vec<garmin_device::DeviceBrowserTarget>,
        storage_id: String,
        directory: Utf8PathBuf,
        copy: bool,
        overwrite: bool,
    },
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
            Self::RemoveMany(_) => DeviceBrowserOperationKind::RemoveMany,
            Self::Rename { .. } => DeviceBrowserOperationKind::Rename,
            Self::Transfer { .. } => DeviceBrowserOperationKind::Transfer,
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
    RemoveMany,
    Rename,
    Transfer,
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
    deployment: &Deployment,
    commands: &Receiver<(Uuid, Command)>,
    event_tx: &Sender<(Uuid, Event)>,
    context: &egui::Context,
    abort: &CancellationToken,
    mutations: &garmin_services::maps::MutationLocks,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("device browser runtime must start");
    while !abort.is_cancelled() {
        let (epoch, command) = match commands.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(command) => command,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let epoch = deployment.epoch();
                if let Ok(application) = block_on(deployment.application(epoch)) {
                    let snapshots = block_on(async {
                        let mut snapshots = Vec::new();
                        for user in application.profiles().await? {
                            let state = application
                                .connect_sync_status(UserContext::new(user.id()))
                                .await?;
                            snapshots.push((user.id(), state));
                            if let Ok(revision) = application
                                .connect_trends_import_revision(UserContext::new(user.id()))
                                .await
                            {
                                let _ = emit(
                                    &EventSender {
                                        sender: event_tx.clone(),
                                        epoch,
                                    },
                                    context,
                                    Event::TrendsImportRevision(user.id(), revision),
                                );
                            }
                        }
                        Ok::<_, garmin_services::Error>(snapshots)
                    });
                    if let Ok(snapshots) = snapshots {
                        let _ = emit(
                            &EventSender {
                                sender: event_tx.clone(),
                                epoch,
                            },
                            context,
                            Event::ConnectSyncStates(snapshots),
                        );
                    }
                }
                continue;
            }
        };
        if matches!(command, Command::Shutdown) {
            break;
        }
        let events = &EventSender {
            sender: event_tx.clone(),
            epoch,
        };
        let application = match block_on(deployment.application(epoch)) {
            Ok(application) => application,
            Err(garmin_services::deployment::Error::Stale) => continue,
            Err(error) => {
                emit(events, context, Event::Unavailable(error.to_string()));
                continue;
            }
        };
        if !execute_command(
            &runtime,
            &application,
            command,
            events,
            context,
            abort,
            mutations,
        ) {
            break;
        }
    }
    block_on(deployment.close());
}

fn read_trends_command(
    application: &Application,
    user: UserContext,
    revision: garmin_ui::trends::ReadRequest,
    events: &EventSender,
    context: &egui::Context,
) -> bool {
    let result = block_on(application.connect_trends(user)).map_err(|error| error.to_string());
    emit(
        events,
        context,
        Event::Trends {
            user: user.user_id(),
            revision,
            result,
        },
    )
}

fn read_connect_cache_command(
    application: &Application,
    request: ConnectCacheRead,
    events: &EventSender,
    context: &egui::Context,
) -> bool {
    let page = block_on(application.read_connect_cache(
        UserContext::new(request.user),
        request.start,
        request.filters.clone(),
    ))
    .ok()
    .flatten();
    emit(events, context, Event::CachedConnectPage { request, page })
}

fn connect_sync_command(
    runtime: &tokio::runtime::Runtime,
    application: &Application,
    user: UserContext,
    request: garmin_model::garmin_connect::sync::Request,
    events: &EventSender,
    context: &egui::Context,
) -> bool {
    use garmin_model::garmin_connect::sync::Request;
    match request {
        Request::Configure(interval) => {
            let result = runtime
                .block_on(application.configure_connect_sync(user, interval))
                .map_err(|error| error.to_string())
                .and_then(|_| block_on(load_profiles(application)));
            emit(events, context, Event::ProfileUpdated(result))
        }
        Request::Cancel(_) | Request::Status => {
            let result = if let Request::Cancel(job) = request {
                runtime.block_on(application.cancel_scheduled_connect_sync(user, job))
            } else {
                block_on(application.connect_sync_status(user))
            };
            let event = match result {
                Ok(snapshot) => Event::ConnectSyncStates(vec![(user.user_id(), snapshot)]),
                Err(error) => Event::ConnectSyncFailure {
                    user: user.user_id(),
                    reason: error.to_string(),
                },
            };
            emit(events, context, event)
        }
    }
}

fn update_profile(
    application: &Application,
    user: UserContext,
    change: ProfileChange,
    events: &EventSender,
    context: &egui::Context,
) -> bool {
    let result = match change {
        ProfileChange::Preferences(preferences) => {
            block_on(application.update_profile_preferences(user, preferences))
        }
        ProfileChange::Accent(accent) => block_on(application.update_profile_accent(user, accent)),
    }
    .map_err(|error| error.to_string())
    .and_then(|_| block_on(load_profiles(application)));
    emit(events, context, Event::ProfileUpdated(result))
}

enum PackageCommand {
    Preview(UserContext, PathBuf),
    Confirm(garmin_services::activity_export::PreparedActivityPackage),
}

fn activity_package_event(application: &Application, command: PackageCommand) -> Event {
    match command {
        PackageCommand::Preview(user, path) => Event::ActivityPackagePreview(
            if is_fit(&path) {
                block_on(application.preview_activity_fit(user, &path))
            } else {
                block_on(application.preview_activity_package(user, &path))
            }
            .map_err(|error| error.to_string()),
        ),
        PackageCommand::Confirm(plan) => Event::ActivityPackageImported(
            block_on(application.confirm_activity_package(plan))
                .map_err(|error| error.to_string())
                .and_then(|_| block_on(load_profiles(application))),
        ),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "The worker dispatches commands against one deployment lease"
)]
fn execute_command(
    runtime: &tokio::runtime::Runtime,
    application: &Application,
    command: Command,
    events: &EventSender,
    context: &egui::Context,
    abort: &CancellationToken,
    mutations: &garmin_services::maps::MutationLocks,
) -> bool {
    match command {
        Command::ActivityPackage(command) => {
            return emit(
                events,
                context,
                activity_package_event(application, command),
            );
        }
        Command::ReadTrends { user, revision } => {
            return read_trends_command(application, user, revision, events, context);
        }
        Command::ReadConnectCache { request } => {
            return read_connect_cache_command(application, request, events, context);
        }
        Command::ConnectSync { user, request } => {
            return connect_sync_command(runtime, application, user, request, events, context);
        }
        Command::Reload => {
            if !emit(
                events,
                context,
                Event::Reloaded(block_on(load_profiles(application))),
            ) {
                return false;
            }
        }
        Command::CreateProfile(display_name) => {
            let result = block_on(application.create_profile(display_name))
                .map_err(|error| error.to_string())
                .and_then(|user| {
                    block_on(load_profiles(application)).map(|profiles| (user.id(), profiles))
                });
            if !emit(events, context, Event::ProfileCreated(result)) {
                return false;
            }
        }
        Command::ConnectActivity(activity) => {
            return connect_activity(runtime, application, activity, events, context, abort);
        }
        Command::GarminConnect { user, action } => {
            return check_connect_key(runtime, application, user, action, events, context, abort);
        }
        Command::UpdateProfile { user, change } => {
            return update_profile(application, user, change, events, context);
        }
        Command::Import { user, paths } => {
            if !import_paths(application, user, paths, events, context, abort) {
                return false;
            }
        }
        Command::LoadActivity {
            user,
            observation_id,
        } => {
            return emit(
                events,
                context,
                load_activity(application, user, observation_id),
            );
        }
        Command::ExportActivity {
            user,
            observation_id,
            destination,
        } => {
            let result = block_on(application.export_activity(user, observation_id, &destination))
                .map(|report| report.is_some())
                .map_err(|error| error.to_string());
            return emit(events, context, Event::ActivityExported(result));
        }
        Command::ImportAvatar {
            user,
            path,
            bytes,
            crop,
        } => {
            let result = block_on(import_avatar(application, user, &path, &bytes, crop))
                .and_then(|()| block_on(load_profiles(application)));
            if !emit(events, context, Event::AvatarUpdated(result)) {
                return false;
            }
        }
        Command::BrowseDevice(candidate) => {
            let key = candidate.key().to_owned();
            let progress = ProgressReporter::default().with_cancellation(abort.clone());
            let result = candidate.browse_with_progress(&progress);
            if !emit(events, context, Event::DeviceCatalog { key, result }) {
                return false;
            }
        }
        Command::OperateDevice(candidate, operation) => {
            let key = candidate.key().to_owned();
            let kind = operation.kind();
            let progress = ProgressReporter::default().with_cancellation(abort.clone());
            let result = mutations
                .acquire(&key)
                .map_err(str::to_owned)
                .and_then(|_guard| {
                    operate_device(runtime, application, &candidate, operation, &progress)
                });
            if !emit(events, context, Event::DeviceBrowser { key, kind, result }) {
                return false;
            }
        }
        Command::Shutdown => return false,
    }
    true
}

fn load_activity(
    application: &Application,
    user: UserContext,
    observation_id: ObservationId,
) -> Event {
    let result = block_on(application.activity(user, observation_id))
        .map(|details| details.map(Box::new))
        .map_err(|error| error.to_string());
    Event::ActivityLoaded {
        observation_id,
        result,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "device operation dispatch retains one catalog and transport snapshot"
)]
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
            let size = ByteCount::from_bytes(metadata.len());
            if size > garmin_device::MAX_BROWSER_TRANSFER_BYTES {
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
                        size,
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
        DeviceBrowserOperation::RemoveMany(targets) => {
            runtime
                .block_on(garmin_device::remove_browser_targets_with_progress(
                    &device, &catalog, &targets, progress,
                ))
                .map_err(|error| error.to_string())?;
            candidate
                .browse_with_progress(progress)
                .map(DeviceBrowserOutcome::Refreshed)
        }
        DeviceBrowserOperation::Rename { target, name } => {
            runtime
                .block_on(garmin_device::rename_browser_target_with_progress(
                    &device, &catalog, &target, &name, progress,
                ))
                .map_err(|error| error.to_string())?;
            candidate
                .browse_with_progress(progress)
                .map(DeviceBrowserOutcome::Refreshed)
        }
        DeviceBrowserOperation::Transfer {
            targets,
            storage_id,
            directory,
            copy,
            overwrite,
        } => {
            let result = if overwrite {
                runtime.block_on(garmin_device::replace_browser_file_with_progress(
                    &device,
                    &catalog,
                    &targets,
                    &storage_id,
                    &directory,
                    copy,
                    progress,
                ))
            } else {
                runtime.block_on(garmin_device::transfer_browser_targets_with_progress(
                    &device,
                    &catalog,
                    &targets,
                    &storage_id,
                    &directory,
                    copy,
                    progress,
                ))
            };
            result.map_err(|error| error.to_string())?;
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
            connect_sync: application
                .connect_sync_status(context)
                .await
                .map_err(|error| error.to_string())?,
            garmin_connect_configured: application
                .garmin_connect_configured(context)
                .await
                .map_err(|error| error.to_string())?,
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
    events: &EventSender,
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
    if is_activity_package(path) {
        let count = block_on(application.import_activity_package(user, path))
            .map_err(|error| error.to_string())?;
        return Ok(if count == 0 {
            ImportOutcome::Duplicate
        } else {
            ImportOutcome::Imported(count)
        });
    }
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
    garmin_services::devices::fit_preview::inspect(file_name, bytes)
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
        } else if is_fit(&path) || is_activity_package(&path) {
            push_file(&mut candidates, &mut seen, path);
        } else {
            candidates.push(Candidate::Failed {
                path,
                reason: "not a FIT file, activity package, or directory".to_owned(),
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

pub(crate) fn is_fit(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("fit"))
}

pub(crate) fn is_activity_package(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("zip")
                || extension.eq_ignore_ascii_case("garmin-activity")
        })
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

struct EventSender {
    sender: Sender<(Uuid, Event)>,
    epoch: Uuid,
}

fn connect_activity(
    runtime: &tokio::runtime::Runtime,
    application: &Application,
    activity: ConnectActivityCommand,
    events: &EventSender,
    context: &egui::Context,
    abort: &CancellationToken,
) -> bool {
    let ConnectActivityCommand {
        user,
        request,
        cancellation,
    } = activity;
    let refreshes_cache = matches!(
        &request,
        garmin_model::garmin_connect::ActivityRequest::Load { .. }
            | garmin_model::garmin_connect::ActivityRequest::List { .. }
            | garmin_model::garmin_connect::ActivityRequest::RefreshAll { .. }
    );
    let result = runtime.block_on(async {
        tokio::select! {
            biased;
            () = cancelled(abort) => None,
            result = application.garmin_connect_activity_cancellable(user, request, |progress| {
                let _ = emit(events, context, Event::ConnectLoadProgress { user: user.user_id(), progress });
            }, || cancellation.is_cancelled()) => Some(result),
        }
    });
    let Some(result) = result else {
        return false;
    };
    let result = result.map_err(garmin_model::garmin_connect::ActivityFailure::from);
    let imported = matches!(
        &result,
        Ok(garmin_service_api::ConnectActivityResult::State(
            garmin_model::garmin_connect::ActivityResponse::Imported { .. }
        ))
    );
    emit(
        events,
        context,
        Event::ConnectActivity {
            user: user.user_id(),
            result,
        },
    ) && (!imported
        || emit(
            events,
            context,
            Event::Reloaded(block_on(load_profiles(application))),
        ))
        && (!refreshes_cache
            || connect_sync_command(
                runtime,
                application,
                user,
                garmin_model::garmin_connect::sync::Request::Status,
                events,
                context,
            ))
}

async fn cancelled(abort: &CancellationToken) {
    while !abort.is_cancelled() {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

fn check_connect_key(
    runtime: &tokio::runtime::Runtime,
    application: &Application,
    user: UserContext,
    action: ConnectAction,
    events: &EventSender,
    context: &egui::Context,
    abort: &CancellationToken,
) -> bool {
    let result = runtime.block_on(async {
        tokio::select! {
            biased;
            () = cancelled(abort) => None,
            result = async {
                match action {
                    ConnectAction::Save(key) => application.verify_and_save_garmin_connect_key(user, key).await,
                    ConnectAction::Test => application.check_garmin_connect(user).await.map(Some),
                }
            } => Some(result),
        }
    });
    let Some(result) = result else {
        return false;
    };
    emit(
        events,
        context,
        Event::GarminConnectChecked {
            user: user.user_id(),
            result: result.map_err(|error| error.to_string()),
        },
    )
}

fn emit(events: &EventSender, context: &egui::Context, event: Event) -> bool {
    if events.sender.send((events.epoch, event)).is_err() {
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
    fn manual_refresh_publishes_its_final_sync_status_without_waiting_for_polling()
    -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let application = Application::new(runtime.block_on(garmin_storage::Storage::open(
            root.path().join("state.sqlite3"),
        ))?);
        let profile = runtime.block_on(application.create_profile("Owner".parse()?))?;
        let user = UserContext::new(profile.id());
        let seed = runtime.block_on(garmin_storage::Storage::open(
            root.path().join("state.sqlite3"),
        ))?;
        runtime.block_on(seed.set_garmin_connect_key(
            user.user_id(),
            Some(&garmin_model::garmin_connect::PersonalApiKey::from_string(
                "fixture-key".into(),
            )?),
        ))?;
        runtime.block_on(seed.close());
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let (sender, received) = channel();
        let events = EventSender {
            sender,
            epoch: Uuid::new_v4(),
        };
        assert!(connect_activity(
            &runtime,
            &application,
            ConnectActivityCommand {
                user,
                request: garmin_model::garmin_connect::ActivityRequest::Load {
                    start: 0,
                    filters: garmin_model::garmin_connect::ActivityFilters::default()
                },
                cancellation,
            },
            &events,
            &egui::Context::default(),
            &CancellationToken::default(),
        ));
        let mut completed = false;
        let mut updated = false;
        for (_, event) in received.try_iter() {
            match event {
                Event::ConnectActivity {
                    result: Ok(garmin_service_api::ConnectActivityResult::RefreshCancelled(_)),
                    ..
                } => completed = true,
                Event::ConnectSyncStates(states) => {
                    assert!(
                        completed,
                        "completion clears the busy flag before status arrives"
                    );
                    assert_eq!(states.len(), 1);
                    assert_eq!(states[0].0, user.user_id());
                    assert!(matches!(
                        states[0].1.state.outcome,
                        garmin_model::garmin_connect::sync::Outcome::Cancelled
                    ));
                    updated = true;
                }
                _ => {}
            }
        }
        assert!(
            updated,
            "completion must publish the persisted outcome immediately"
        );
        runtime.block_on(application.close());
        Ok(())
    }

    #[test]
    fn cancelled_connect_command_stops_without_a_result() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let application = Application::new(block_on(garmin_storage::Storage::open(
            root.path().join("storage.sqlite3"),
        ))?);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let abort = CancellationToken::default();
        abort.cancel();
        let (sender, received) = channel();
        let events = EventSender {
            sender,
            epoch: Uuid::new_v4(),
        };
        // A nonexistent profile also prevents service contact if the cancellation guard regresses.
        let user = UserContext::new(garmin_model::identity::UserId::new_v4());
        assert!(!check_connect_key(
            &runtime,
            &application,
            user,
            ConnectAction::Test,
            &events,
            &egui::Context::default(),
            &abort
        ));
        assert!(received.try_recv().is_err());
        block_on(application.close());
        Ok(())
    }

    #[test]
    fn epoch_change_discards_queued_results_and_tags_new_commands() {
        let (commands, received) = channel();
        let (sent, events) = channel();
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        let mut worker = Worker {
            commands,
            events,
            epoch: old,
            abort: CancellationToken::default(),
            join: None,
        };
        sent.send((old, Event::Unavailable("old result".into())))
            .unwrap();
        worker.set_epoch(new);
        sent.send((new, Event::Unavailable("new result".into())))
            .unwrap();
        assert_eq!(worker.drain().count(), 1);
        worker.reload();
        assert!(matches!(received.recv().unwrap(), (epoch, Command::Reload) if epoch == new));
    }

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
        assert_eq!(preview.activities().len(), 1);
        assert_ne!(preview.activities()[0].source, "");
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
