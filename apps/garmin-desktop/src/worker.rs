use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use futures_lite::future::block_on;
use garmin_model::{
    artifact::{AcquisitionOperationId, ArtifactDigest, SourceIdentity},
    identity::{DisplayName, ProfilePreferences, Source, SourceId, User},
    observation::ObservationId,
    value::Timestamp,
};
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

pub struct Worker {
    commands: Sender<Command>,
    events: Receiver<Event>,
    abort: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(application: Application, context: egui::Context) -> std::io::Result<Self> {
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        let abort = Arc::new(AtomicBool::new(false));
        let worker_abort = Arc::clone(&abort);
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

    pub fn drain(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter()
    }

    pub fn abort(&self) {
        self.abort.store(true, Ordering::Release);
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
    Shutdown,
}

pub enum Event {
    Reloaded(Result<Vec<ProfileData>, String>),
    ProfileCreated(Result<(garmin_model::identity::UserId, Vec<ProfileData>), String>),
    ProfileUpdated(Result<Vec<ProfileData>, String>),
    AvatarUpdated(Result<Vec<ProfileData>, String>),
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

fn run(
    application: Application,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    context: &egui::Context,
    abort: &AtomicBool,
) {
    while !abort.load(Ordering::Acquire) {
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
            Command::Shutdown => break,
        }
    }
    block_on(application.close());
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
    abort: &AtomicBool,
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
                if abort.load(Ordering::Acquire) {
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
        if abort.load(Ordering::Acquire) {
            return false;
        }
        let item = match candidate {
            Candidate::File(path) => import_file(application, user, &source, path),
            Candidate::Failed { path, reason } => ImportItem {
                path,
                outcome: ImportOutcome::Failed(reason),
            },
        };
        if abort.load(Ordering::Acquire) || !emit(events, context, Event::ImportItem(item)) {
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
    let acquired_at = timestamp(SystemTime::now())?;
    let path = path
        .to_str()
        .ok_or_else(|| "FIT path is not valid UTF-8".to_owned())?;
    let source_identity =
        SourceIdentity::from_string(path.to_owned()).map_err(|error| error.to_string())?;
    let operation_id = operation_id(source, &source_identity, &bytes);
    let result = block_on(application.import_fit(FitImportRequest::from_parts(
        user,
        source,
        source_identity,
        operation_id,
        acquired_at,
        &bytes,
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

fn operation_id(
    source: &Source,
    identity: &SourceIdentity,
    bytes: &[u8],
) -> AcquisitionOperationId {
    let mut digest = blake3::Hasher::new();
    digest.update(OPERATION_ID_DOMAIN_V1);
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

fn candidates(paths: Vec<PathBuf>, abort: &AtomicBool) -> Option<Vec<Candidate>> {
    if abort.load(Ordering::Acquire) {
        return None;
    }
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        if abort.load(Ordering::Acquire) {
            return None;
        }
        if path.is_dir() {
            let before = candidates.len();
            for entry in WalkDir::new(&path).follow_links(false) {
                if abort.load(Ordering::Acquire) {
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

        let abort = AtomicBool::new(false);
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
        let abort = AtomicBool::new(true);

        assert!(candidates(Vec::new(), &abort).is_none());
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
