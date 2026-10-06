//! Bounded host-owned uploads and retained GPX previews.

pub mod delivery;
mod reads;
mod uploads;

#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use garmin_gpx::worker::{Parser, PreparedGpx};
use garmin_model::{
    artifact::{AcquisitionOperationId, ByteCount},
    identity::{Source, UserId},
    route::{CourseGenerationOperationId, RoutePlanRevisionId},
};
use garmin_service_api::routes::{
    GpxUpload, GpxUploadPhase, RouteFailure, RouteFailureKind as Kind, RouteReply, RouteRequest,
    RouteService,
};
use remoc::rtc;
use tokio::task::AbortHandle;
use uuid::Uuid;

use crate::{Application, UserContext, deployment::Deployment};

type Result<T> = std::result::Result<T, RouteFailure>;
const MAX_UPLOADS: usize = 4;
const MAX_GENERATIONS: usize = 16;
const TTL: Duration = Duration::from_mins(15);

enum Stage {
    Uploading(Vec<u8>),
    Parsing { bytes: Arc<[u8]>, task: AbortHandle },
    Review(PreparedGpx),
    Failed { bytes: Arc<[u8]>, message: String },
}

struct Upload {
    actor: UserId,
    epoch: Uuid,
    expires: Instant,
    file_name: String,
    source: Source,
    total: usize,
    confirmations: usize,
    stage: Stage,
}

struct Confirmation {
    operations: Arc<RouteOperations>,
    operation: AcquisitionOperationId,
}

impl Drop for Confirmation {
    fn drop(&mut self) {
        if let Some(entry) = self.operations.pending().uploads.get_mut(&self.operation) {
            entry.confirmations = entry.confirmations.saturating_sub(1);
        }
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        if let Stage::Parsing { task, .. } = &self.stage {
            task.abort();
        }
    }
}

struct Generation {
    actor: UserId,
    epoch: Uuid,
    expires: Instant,
    revision: RoutePlanRevisionId,
}

#[derive(Default)]
struct Pending {
    uploads: HashMap<AcquisitionOperationId, Upload>,
    generations: HashMap<CourseGenerationOperationId, Generation>,
}

/// One registry per native host, shared by profile-bound sessions.
pub struct RouteOperations {
    deployment: Arc<Deployment>,
    parser: Parser,
    pending: Mutex<Pending>,
    deliveries: Arc<tokio::sync::Semaphore>,
}

impl RouteOperations {
    /// Uses the parser helper installed next to this native host.
    /// # Errors
    /// Fails when the platform cannot provide the parser's resource limits.
    pub fn beside_host(deployment: Arc<Deployment>) -> Result<Arc<Self>> {
        Ok(Self::new(
            deployment,
            Parser::beside_host().map_err(internal)?,
        ))
    }

    /// Starts lightweight expiry/restore cleanup; call inside the host runtime.
    #[must_use]
    pub fn new(deployment: Arc<Deployment>, parser: Parser) -> Arc<Self> {
        let mut epoch = deployment.watch_epoch();
        let operations = Arc::new(Self {
            deployment,
            parser,
            pending: Mutex::default(),
            deliveries: Arc::new(tokio::sync::Semaphore::new(8)),
        });
        let weak = Arc::downgrade(&operations);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(1)) => {},
                    changed = epoch.changed() => { if changed.is_err() { break; } },
                }
                let Some(operations) = weak.upgrade() else {
                    break;
                };
                operations.prune();
            }
        });
        operations
    }

    /// Opens a profile-bound session in the current deployment epoch.
    /// # Errors
    /// [`RouteFailure`] for stale storage or an absent profile.
    pub async fn connect(self: &Arc<Self>, actor: UserContext) -> Result<RouteSession> {
        let epoch = self.deployment.epoch();
        self.connect_at(actor, epoch).await
    }

    /// Binds queued adapter work to the epoch captured when the user requested it.
    /// # Errors
    /// Rejects stale epochs and absent profiles, including a restore racing session creation.
    pub async fn connect_at(
        self: &Arc<Self>,
        actor: UserContext,
        epoch: Uuid,
    ) -> Result<RouteSession> {
        let app = self.deployment.application(epoch).await.map_err(stale)?;
        if app
            .storage
            .user(actor.user_id())
            .await
            .map_err(internal)?
            .is_none()
        {
            return Err(failure(Kind::Unauthorized, "profile does not exist"));
        }
        Ok(RouteSession {
            operations: Arc::clone(self),
            actor,
            epoch,
        })
    }

    fn pending(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn prune(&self) {
        let epoch = self.deployment.epoch();
        let now = Instant::now();
        let mut pending = self.pending();
        pending
            .uploads
            .retain(|_, entry| entry.epoch == epoch && entry.expires > now);
        pending
            .generations
            .retain(|_, entry| entry.epoch == epoch && entry.expires > now);
    }
}

/// The host supplies actor context; requests cannot retarget another profile or epoch.
#[derive(Clone)]
pub struct RouteSession {
    operations: Arc<RouteOperations>,
    actor: UserContext,
    epoch: Uuid,
}

impl RouteSession {
    /// Executes one bounded client operation under its deployment lease.
    /// # Errors
    /// [`RouteFailure`] for invalid scope, input, state, or persistence failure.
    pub async fn request(&self, request: RouteRequest) -> Result<RouteReply> {
        let app = self
            .operations
            .deployment
            .application(self.epoch)
            .await
            .map_err(stale)?;
        self.operations.prune();
        match request {
            RouteRequest::List { offset } => self.list(&app, offset).await,
            RouteRequest::Detail { plan } => self.detail(&app, plan).await,
            RouteRequest::RevisionPoints {
                revision,
                offset,
                count,
            } => self.revision_points(&app, revision, offset, count).await,
            RouteRequest::Versions { revision, offset } => {
                self.versions(&app, revision, offset).await
            }
            RouteRequest::StartUpload { file_name, size } => self.start(file_name, size),
            RouteRequest::Append {
                operation,
                offset,
                bytes,
            } => self.append(operation, offset, &bytes),
            RouteRequest::Inspect { operation } => self.inspect(operation),
            RouteRequest::UploadStatus { operation } => self.status(operation),
            RouteRequest::Candidates { operation, offset } => self.candidates(operation, offset),
            RouteRequest::PreviewPoints {
                operation,
                candidate,
                offset,
                count,
            } => self.preview_points(operation, candidate, offset, count),
            RouteRequest::Confirm {
                operation,
                selection,
            } => self.confirm(&app, operation, selection).await,
            RouteRequest::Cancel { operation } => self.cancel(operation),
            RouteRequest::PrepareGeneration { revision } => {
                self.prepare_generation(&app, revision).await
            }
            RouteRequest::Generate {
                operation,
                revision,
            } => self.generate(&app, operation, revision).await,
            RouteRequest::DeleteCourse {
                revision,
                generation,
            } => {
                app.storage
                    .delete_course_generation(self.actor.user_id(), revision, generation)
                    .await
                    .map_err(internal)?;
                Ok(RouteReply::CourseDeleted { revision })
            }
        }
    }

    fn entry<'a>(
        &self,
        pending: &'a mut Pending,
        operation: AcquisitionOperationId,
    ) -> Result<&'a mut Upload> {
        pending
            .uploads
            .get_mut(&operation)
            .filter(|entry| entry.actor == self.actor.user_id() && entry.epoch == self.epoch)
            .ok_or_else(|| failure(Kind::NotFound, "upload was not found or has expired"))
    }
}

impl RouteService for RouteSession {
    async fn execute(
        &self,
        request: RouteRequest,
    ) -> std::result::Result<Result<RouteReply>, rtc::CallError> {
        Ok(self.request(request).await)
    }
}

impl Upload {
    fn snapshot(&self, operation: AcquisitionOperationId) -> GpxUpload {
        let (received, phase) = match &self.stage {
            Stage::Uploading(bytes) => (bytes.len(), GpxUploadPhase::Uploading),
            Stage::Parsing { bytes, .. } => (bytes.len(), GpxUploadPhase::Parsing),
            Stage::Review(prepared) => (
                prepared.bytes().len(),
                GpxUploadPhase::Review {
                    digest: garmin_model::artifact::ArtifactDigest::from_bytes(prepared.bytes()),
                    candidates: u32::try_from(prepared.document().candidates().len())
                        .unwrap_or(u32::MAX),
                    rejected: u32::try_from(prepared.document().rejected().len())
                        .unwrap_or(u32::MAX),
                },
            ),
            Stage::Failed { bytes, message } => {
                (bytes.len(), GpxUploadPhase::Failed(message.clone()))
            }
        };
        GpxUpload {
            operation,
            file_name: self.file_name.clone(),
            total: ByteCount::from_u64(self.total as u64),
            received: ByteCount::from_u64(received as u64),
            phase,
        }
    }
}

fn failure(kind: Kind, message: impl std::fmt::Display) -> RouteFailure {
    RouteFailure {
        kind,
        message: message.to_string(),
    }
}
fn internal(error: impl std::fmt::Display) -> RouteFailure {
    failure(Kind::Internal, error)
}
fn stale(error: impl std::fmt::Display) -> RouteFailure {
    failure(Kind::Stale, error)
}
