//! Host-owned, reviewed transfers of existing Course artifacts.

use std::{
    collections::HashMap,
    fs,
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use anyhow::{Result, bail, ensure};
use garmin_device::{DevicePathStatus, SafeRelativePath, StorageCapacity, storage::DeviceRead};
use garmin_model::{
    artifact::ArtifactDigest, identity::UserId, route::CourseGenerationOperationId,
};
use garmin_progress::{CancellationToken, OperationStage, ProgressUnit};
use garmin_service_api::course_transfer::{
    CourseCleanupReview, CourseTarget, CourseTransferPhase, CourseTransferPreparation,
    CourseTransferProgress, CourseTransferReview, CourseTransferStatus,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    UserContext,
    deployment::Deployment,
    maps::{MutationLocks, device::Connector},
};

#[cfg(test)]
mod tests;

const APPROVAL_LIFETIME: Duration = Duration::from_mins(15);
const MAX_PENDING_APPROVALS: usize = 64;
const MAX_RECORD_BYTES: u64 = 64 * 1024;

struct Prepared {
    actor: UserId,
    epoch: Uuid,
    expires: Instant,
    review: CourseTransferReview,
    connector: Arc<dyn Connector>,
}

struct PreparedCleanup {
    actor: UserId,
    epoch: Uuid,
    expires: Instant,
    review: CourseCleanupReview,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u8,
    actor: UserId,
    epoch: Uuid,
    review: CourseTransferReview,
    path: SafeRelativePath,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    version: u8,
    phase: CourseTransferPhase,
}

struct ActiveTransfer {
    cancellation: CancellationToken,
    progress: Mutex<CourseTransferProgress>,
}

impl ActiveTransfer {
    fn progress(&self) -> CourseTransferProgress {
        self.progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// One host's Course transfer approvals and durable, device-specific receipts.
pub struct CourseTransfers {
    deployment: Arc<Deployment>,
    mutations: Arc<MutationLocks>,
    root: PathBuf,
    prepared: Mutex<HashMap<Uuid, Prepared>>,
    cleanup: Mutex<HashMap<Uuid, PreparedCleanup>>,
    active: Mutex<HashMap<Uuid, Arc<ActiveTransfer>>>,
    outcome_write: Mutex<()>,
}

impl CourseTransfers {
    #[must_use]
    pub fn new(deployment: Arc<Deployment>, mutations: Arc<MutationLocks>) -> Arc<Self> {
        Arc::new(Self {
            root: deployment.root().join("course-transfers"),
            deployment,
            mutations,
            prepared: Mutex::default(),
            cleanup: Mutex::default(),
            active: Mutex::default(),
            outcome_write: Mutex::default(),
        })
    }

    /// List manifest-declared FIT Course input locations for one explicitly selected device.
    /// # Errors
    /// A missing owned Course, unavailable device, or inconsistent capability evidence.
    pub async fn targets(
        &self,
        actor: UserContext,
        generation: CourseGenerationOperationId,
        device_key: &str,
        connector: &dyn Connector,
    ) -> Result<Vec<CourseTarget>> {
        let lease = self.deployment.application(self.deployment.epoch()).await?;
        let record = lease
            .storage
            .course_generation(actor.user_id(), generation)
            .await?
            .context("saved Course version is unavailable")?;
        let connection = connector.connect().await?;
        let state = connection.device.state().await?;
        Ok(targets_for(
            &connection.manifest,
            &state,
            device_key,
            record.byte_count().as_u64(),
        ))
    }

    /// Prepare a single-use review. No device mutation occurs here.
    /// # Errors
    /// An invalid Course, device, storage, or prior conflicting transfer.
    pub async fn prepare(
        &self,
        actor: UserContext,
        generation: CourseGenerationOperationId,
        device_key: String,
        storage_id: &str,
        connector: Arc<dyn Connector>,
    ) -> Result<CourseTransferPreparation> {
        let epoch = self.deployment.epoch();
        let lease = self.deployment.application(epoch).await?;
        let record = lease
            .storage
            .course_generation(actor.user_id(), generation)
            .await?
            .context("saved Course version is unavailable")?;
        let bytes = lease
            .course_artifact(actor, record.artifact_id())
            .await?
            .context("saved Course bytes are unavailable")?;
        ensure!(
            ArtifactDigest::from_bytes(&bytes) == record.digest(),
            "saved Course bytes changed"
        );
        ensure!(
            bytes.len() as u64 == record.byte_count().as_u64(),
            "saved Course size changed"
        );
        let connection = connector.connect().await?;
        let target = targets_for(
            &connection.manifest,
            &connection.device.state().await?,
            &device_key,
            record.byte_count().as_u64(),
        )
        .into_iter()
        .find(|target| target.storage_id == storage_id)
        .context("selected storage has no writable FIT Course input")?;
        if let Some(existing) = self
            .verified_copy(
                actor.user_id(),
                record.id(),
                &target,
                connection.device.as_ref(),
            )
            .await?
        {
            return Ok(CourseTransferPreparation::AlreadyOnDevice(existing));
        }
        let transfer = Uuid::new_v4();
        let file_name = format!("gt-{transfer}.fit");
        let review = CourseTransferReview {
            approval: Uuid::new_v4(),
            transfer,
            generation: record.id(),
            generation_operation: record.operation_id(),
            artifact: record.artifact_id(),
            version: record.version().get(),
            digest: record.digest(),
            byte_count: record.byte_count(),
            target,
            file_name,
        };
        let mut approvals = self.prepared.lock().unwrap_or_else(PoisonError::into_inner);
        approvals.retain(|_, prepared| prepared.expires > Instant::now());
        ensure!(
            approvals.len() < MAX_PENDING_APPROVALS,
            "too many pending Course transfer reviews"
        );
        approvals.insert(
            review.approval,
            Prepared {
                actor: actor.user_id(),
                epoch,
                expires: Instant::now() + APPROVAL_LIFETIME,
                review: review.clone(),
                connector,
            },
        );
        Ok(CourseTransferPreparation::Review(review))
    }

    /// Start an approved write independently of the caller's connection.
    /// # Errors
    /// Stale approval, changed evidence, occupied device, or failed intent persistence.
    pub async fn approve(
        self: &Arc<Self>,
        actor: UserContext,
        approval: Uuid,
    ) -> Result<CourseTransferStatus> {
        let prepared = {
            let mut approvals = self.prepared.lock().unwrap_or_else(PoisonError::into_inner);
            ensure!(
                approvals
                    .get(&approval)
                    .is_some_and(|item| item.actor == actor.user_id()),
                "Course transfer approval is unavailable"
            );
            approvals
                .remove(&approval)
                .context("Course transfer approval is unavailable")?
        };
        ensure!(
            Instant::now() < prepared.expires,
            "Course transfer approval expired"
        );
        ensure!(
            prepared.epoch == self.deployment.epoch(),
            "database changed since Course transfer review"
        );
        let review = prepared.review;
        let mutation = self
            .mutations
            .acquire(&review.target.device_key)
            .map_err(|error| anyhow::anyhow!(error))?;
        let lease = self.deployment.application(prepared.epoch).await?;
        let bytes = reviewed_bytes(&lease, actor, &review).await?;
        let connection = prepared.connector.connect().await?;
        let target = targets_for(
            &connection.manifest,
            &connection.device.state().await?,
            &review.target.device_key,
            review.byte_count.as_u64(),
        )
        .into_iter()
        .find(|target| {
            target.storage_id == review.target.storage_id
                && target.device_digest == review.target.device_digest
                && target.directory == review.target.directory
        })
        .context("Course destination changed since review")?;
        if let Some(existing) = self
            .verified_copy(
                actor.user_id(),
                review.generation,
                &target,
                connection.device.as_ref(),
            )
            .await?
        {
            return Ok(existing);
        }
        let path = destination(&connection.manifest, &target, &review.file_name)?;
        ensure!(
            connection.device.inspect(&target.storage_id, &path).await?
                == DevicePathStatus::Missing,
            "Course destination is occupied"
        );
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let intent = Intent {
            version: 1,
            actor: actor.user_id(),
            epoch: prepared.epoch,
            review: review.clone(),
            path,
            sha256,
        };
        let directory = self.root.join(review.transfer.to_string());
        persist_intent(&directory, &intent, &bytes)?;
        drop(lease);
        Ok(self.start_upload(intent, target, connection, directory, mutation))
    }

    fn start_upload(
        self: &Arc<Self>,
        intent: Intent,
        target: CourseTarget,
        connection: crate::maps::device::Connection,
        directory: PathBuf,
        mutation: tokio::sync::OwnedMutexGuard<()>,
    ) -> CourseTransferStatus {
        let active = Arc::new(ActiveTransfer {
            cancellation: CancellationToken::default(),
            progress: Mutex::new(CourseTransferProgress {
                bytes_sent: 0,
                total_bytes: intent.review.byte_count.as_u64(),
                finishing: false,
            }),
        });
        let mut status = status(&intent, CourseTransferPhase::Running);
        status.progress = Some(active.progress());
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(intent.review.transfer, Arc::clone(&active));
        let owner = Arc::clone(self);
        tokio::spawn(async move {
            let result = upload_and_verify(&connection, &target, &intent, &directory, active).await;
            let phase = match result {
                Ok(()) => CourseTransferPhase::Verified,
                Err(error) => CourseTransferPhase::NeedsReview(error.to_string()),
            };
            if let Err(error) = owner.record_phase(&directory, phase) {
                tracing::error!(%error, transfer = %intent.review.transfer, "Course transfer outcome could not be persisted");
            }
            owner
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&intent.review.transfer);
            drop(mutation);
        });
        status
    }

    /// Read or reconcile one transfer without starting another write.
    /// # Errors
    /// Foreign receipt, unsafe journal, or unavailable device evidence.
    pub async fn status(
        &self,
        actor: UserContext,
        transfer: Uuid,
        connector: &dyn Connector,
    ) -> Result<CourseTransferStatus> {
        let directory = self.root.join(transfer.to_string());
        let intent = read_intent(&directory)?;
        ensure!(
            intent.actor == actor.user_id(),
            "transfer belongs to another profile"
        );
        if let Some(active) = self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&transfer)
            .cloned()
        {
            let mut status = status(&intent, CourseTransferPhase::Running);
            status.progress = Some(active.progress());
            return Ok(status);
        }
        let accepted = read_outcome(&directory)?
            .is_some_and(|outcome| matches!(outcome.phase, CourseTransferPhase::Accepted));
        let connection = connector.connect().await?;
        ensure!(
            connection.manifest.identity_digest() == intent.review.target.device_digest,
            "a different device is connected"
        );
        ensure!(
            destination(
                &connection.manifest,
                &intent.review.target,
                &intent.review.file_name
            )? == intent.path,
            "recorded Course destination is no longer declared"
        );
        let phase = match connection
            .device
            .inspect(&intent.review.target.storage_id, &intent.path)
            .await?
        {
            DevicePathStatus::Missing => CourseTransferPhase::Missing,
            DevicePathStatus::RegularFile { size } if size == intent.review.byte_count.as_u64() => {
                match connection
                    .device
                    .verify(
                        &intent.review.target.storage_id,
                        &intent.path,
                        size,
                        &intent.sha256,
                    )
                    .await
                {
                    Ok(()) if accepted => CourseTransferPhase::Accepted,
                    Ok(()) => CourseTransferPhase::Verified,
                    Err(error) => CourseTransferPhase::NeedsReview(error.to_string()),
                }
            }
            _ => CourseTransferPhase::NeedsReview("recorded Course file differs on device".into()),
        };
        let phase = self.record_phase(&directory, phase)?;
        Ok(status(&intent, phase))
    }

    /// Find this profile's retained transfers for one Course and selected device.
    /// # Errors
    /// A malformed retained intent is never silently skipped.
    pub async fn list(
        &self,
        actor: UserContext,
        generation: CourseGenerationOperationId,
        device_key: &str,
    ) -> Result<Vec<CourseTransferStatus>> {
        let lease = self.deployment.application(self.deployment.epoch()).await?;
        let generation = lease
            .storage
            .course_generation(actor.user_id(), generation)
            .await?
            .context("saved Course version is unavailable")?
            .id();
        let mut statuses = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(statuses),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let intent = match read_intent(&entry.path()) {
                Ok(intent) => intent,
                Err(error) if uncommitted_directory(&entry.path())? => {
                    tracing::warn!(%error, path = %entry.path().display(),
                        "ignoring interrupted Course intent without an upload payload");
                    continue;
                }
                Err(error) => return Err(error),
            };
            if intent.actor == actor.user_id()
                && intent.review.generation == generation
                && intent.review.target.device_key == device_key
            {
                let phase = read_outcome(&entry.path())?
                    .map_or(CourseTransferPhase::Running, |outcome| outcome.phase);
                statuses.push(status(&intent, phase));
            }
        }
        statuses.sort_by_key(|item| item.transfer);
        Ok(statuses)
    }

    /// Request cancellation of this transfer; reconciliation decides its final state.
    /// # Errors
    /// Unknown or foreign transfer.
    pub fn cancel(&self, actor: UserContext, transfer: Uuid) -> Result<()> {
        let intent = read_intent(&self.root.join(transfer.to_string()))?;
        ensure!(
            intent.actor == actor.user_id(),
            "transfer belongs to another profile"
        );
        if let Some(active) = self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&transfer)
        {
            active.cancellation.cancel();
        }
        Ok(())
    }

    /// Record an explicit acknowledgement of a byte-verified transfer.
    /// This does not establish firmware import while the device is mounted in MTP mode.
    /// # Errors
    /// Device readback does not match the transfer intent.
    pub async fn accept(
        &self,
        actor: UserContext,
        transfer: Uuid,
        connector: &dyn Connector,
    ) -> Result<CourseTransferStatus> {
        let current = self.status(actor, transfer, connector).await?;
        ensure!(
            matches!(
                current.phase,
                CourseTransferPhase::Verified | CourseTransferPhase::Accepted
            ),
            "Course transfer is not verified"
        );
        let directory = self.root.join(transfer.to_string());
        self.record_phase(&directory, CourseTransferPhase::Accepted)?;
        Ok(status(
            &read_intent(&directory)?,
            CourseTransferPhase::Accepted,
        ))
    }

    /// Review deletion of only a retained transfer's exact partial file.
    /// # Errors
    /// Missing proof, intact Course, active transfer, or changed device.
    pub async fn prepare_cleanup(
        &self,
        actor: UserContext,
        transfer: Uuid,
        connector: &dyn Connector,
    ) -> Result<CourseCleanupReview> {
        ensure!(
            !self
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&transfer),
            "Course transfer is still running"
        );
        let directory = self.root.join(transfer.to_string());
        let intent = read_intent(&directory)?;
        ensure!(
            intent.actor == actor.user_id(),
            "transfer belongs to another profile"
        );
        let connection = connector.connect().await?;
        ensure!(
            connection.manifest.identity_digest() == intent.review.target.device_digest,
            "a different device is connected"
        );
        ensure!(
            destination(
                &connection.manifest,
                &intent.review.target,
                &intent.review.file_name
            )? == intent.path,
            "recorded Course destination is no longer declared"
        );
        let size = match connection
            .device
            .inspect(&intent.review.target.storage_id, &intent.path)
            .await?
        {
            DevicePathStatus::RegularFile { size } if size < intent.review.byte_count.as_u64() => {
                size
            }
            _ => bail!("no partial Course upload is available for cleanup"),
        };
        let bytes = connection
            .device
            .read_bounded_file(&intent.review.target.storage_id, &intent.path, size)
            .await?
            .context("partial Course upload disappeared")?;
        let payload = fs::read(directory.join("payload.fit"))?;
        ensure!(
            payload.len() as u64 == intent.review.byte_count.as_u64()
                && hex::encode(Sha256::digest(&payload)) == intent.sha256
                && bytes.len() <= payload.len()
                && bytes == payload[..bytes.len()],
            "partial Course bytes do not match the retained upload"
        );
        let review = CourseCleanupReview {
            approval: Uuid::new_v4(),
            transfer,
            target: intent.review.target,
            file_name: intent.review.file_name,
            byte_count: size,
        };
        let mut approvals = self.cleanup.lock().unwrap_or_else(PoisonError::into_inner);
        approvals.retain(|_, prepared| prepared.expires > Instant::now());
        ensure!(
            approvals.len() < MAX_PENDING_APPROVALS,
            "too many pending Course cleanup reviews"
        );
        approvals.insert(
            review.approval,
            PreparedCleanup {
                actor: actor.user_id(),
                epoch: self.deployment.epoch(),
                expires: Instant::now() + APPROVAL_LIFETIME,
                review: review.clone(),
            },
        );
        Ok(review)
    }

    /// Recheck exact partial bytes and remove them after separate confirmation.
    /// # Errors
    /// Consumed approval, changed device or file, or transport failure.
    pub async fn approve_cleanup(
        &self,
        actor: UserContext,
        approval: Uuid,
        connector: &dyn Connector,
    ) -> Result<CourseTransferStatus> {
        let prepared = {
            let mut approvals = self.cleanup.lock().unwrap_or_else(PoisonError::into_inner);
            ensure!(
                approvals
                    .get(&approval)
                    .is_some_and(|item| item.actor == actor.user_id()),
                "Course cleanup approval is unavailable"
            );
            approvals
                .remove(&approval)
                .context("Course cleanup approval is unavailable")?
        };
        ensure!(
            Instant::now() < prepared.expires,
            "Course cleanup approval expired"
        );
        ensure!(
            prepared.epoch == self.deployment.epoch(),
            "database changed since Course cleanup review"
        );
        let review = prepared.review;
        let _mutation = self
            .mutations
            .acquire(&review.target.device_key)
            .map_err(|error| anyhow::anyhow!(error))?;
        let directory = self.root.join(review.transfer.to_string());
        let intent = read_intent(&directory)?;
        ensure!(
            intent.actor == actor.user_id()
                && intent.epoch == prepared.epoch
                && intent.review.target.device_digest == review.target.device_digest,
            "Course cleanup target changed"
        );
        let connection = connector.connect().await?;
        ensure!(
            connection.manifest.identity_digest() == review.target.device_digest,
            "a different device is connected"
        );
        ensure!(
            destination(&connection.manifest, &review.target, &review.file_name)? == intent.path,
            "recorded Course destination is no longer declared"
        );
        let bytes = connection
            .device
            .read_bounded_file(&review.target.storage_id, &intent.path, review.byte_count)
            .await?
            .context("partial Course upload disappeared")?;
        let payload = fs::read(directory.join("payload.fit"))?;
        ensure!(
            payload.len() as u64 == intent.review.byte_count.as_u64()
                && hex::encode(Sha256::digest(&payload)) == intent.sha256
                && bytes.len() as u64 == review.byte_count
                && bytes.len() <= payload.len()
                && bytes == payload[..bytes.len()],
            "partial Course upload changed since review"
        );
        connection
            .device
            .delete_size_checked(&review.target.storage_id, &intent.path, review.byte_count)
            .await?;
        self.record_phase(&directory, CourseTransferPhase::Missing)?;
        Ok(status(&intent, CourseTransferPhase::Missing))
    }

    fn record_phase(
        &self,
        directory: &Path,
        phase: CourseTransferPhase,
    ) -> Result<CourseTransferPhase> {
        let _guard = self
            .outcome_write
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let phase = if matches!(phase, CourseTransferPhase::Verified)
            && read_outcome(directory)?
                .is_some_and(|outcome| matches!(outcome.phase, CourseTransferPhase::Accepted))
        {
            CourseTransferPhase::Accepted
        } else {
            phase
        };
        write_outcome(directory, &phase)?;
        Ok(phase)
    }

    async fn verified_copy(
        &self,
        actor: UserId,
        generation: garmin_model::route::CourseGenerationId,
        target: &CourseTarget,
        device: &dyn DeviceRead,
    ) -> Result<Option<CourseTransferStatus>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let intent = match read_intent(&entry.path()) {
                Ok(intent) => intent,
                Err(error) if uncommitted_directory(&entry.path())? => {
                    tracing::warn!(%error, path = %entry.path().display(),
                        "ignoring interrupted Course intent without an upload payload");
                    continue;
                }
                Err(error) => return Err(error),
            };
            if intent.actor != actor
                || intent.review.generation != generation
                || intent.review.target.device_digest != target.device_digest
                || intent.review.target.storage_id != target.storage_id
                || intent.review.target.directory != target.directory
            {
                continue;
            }
            match device.inspect(&target.storage_id, &intent.path).await? {
                DevicePathStatus::Missing => {}
                DevicePathStatus::RegularFile { size }
                    if size == intent.review.byte_count.as_u64()
                        && device
                            .verify(&target.storage_id, &intent.path, size, &intent.sha256)
                            .await
                            .is_ok() =>
                {
                    let accepted = read_outcome(&entry.path())?.is_some_and(|outcome| {
                        matches!(outcome.phase, CourseTransferPhase::Accepted)
                    });
                    let phase = if accepted {
                        CourseTransferPhase::Accepted
                    } else {
                        CourseTransferPhase::Verified
                    };
                    return Ok(Some(status(&intent, phase)));
                }
                _ => bail!("earlier Course transfer needs review before another send"),
            }
        }
        Ok(None)
    }
}

fn targets_for(
    manifest: &garmin_device::DeviceManifest,
    state: &garmin_device::DeviceStateSnapshot,
    device_key: &str,
    bytes: u64,
) -> Vec<CourseTarget> {
    let sample = format!("gt-{}.fit", Uuid::nil());
    let mut targets = Vec::new();
    for capability in manifest.capabilities().capabilities() {
        let Some(path) = capability.course_destination(&sample) else {
            continue;
        };
        let Some(directory) = path.as_path().parent() else {
            continue;
        };
        for storage in &state.storages {
            if storage.writable == Some(false) {
                continue;
            }
            let free_bytes = match &storage.capacity {
                StorageCapacity::Available { free_bytes, .. } => Some(*free_bytes),
                StorageCapacity::Unavailable { .. } => None,
            };
            if free_bytes.is_some_and(|free| free < bytes) {
                continue;
            }
            let target = CourseTarget {
                device_key: device_key.to_owned(),
                device_name: manifest.summary.model.clone(),
                device_digest: manifest.identity_digest(),
                storage_id: storage.id.clone(),
                storage_label: storage.label.clone(),
                directory: directory.to_string_lossy().into_owned(),
                free_bytes,
            };
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    targets
}

async fn reviewed_bytes(
    application: &crate::Application,
    actor: UserContext,
    review: &CourseTransferReview,
) -> Result<Vec<u8>> {
    let record = application
        .storage
        .course_generation(actor.user_id(), review.generation_operation)
        .await?
        .context("saved Course version was removed")?;
    ensure!(
        record.id() == review.generation
            && record.artifact_id() == review.artifact
            && record.digest() == review.digest
            && record.byte_count() == review.byte_count
            && record.version().get() == review.version,
        "saved Course version changed since review"
    );
    let bytes = application
        .course_artifact(actor, review.artifact)
        .await?
        .context("saved Course bytes were removed")?;
    ensure!(
        ArtifactDigest::from_bytes(&bytes) == review.digest
            && bytes.len() as u64 == review.byte_count.as_u64(),
        "saved Course bytes changed since review"
    );
    Ok(bytes)
}

fn persist_intent(directory: &Path, intent: &Intent, bytes: &[u8]) -> Result<()> {
    create_private_directory(directory)?;
    write_new(&directory.join("intent.json"), &serde_json::to_vec(intent)?)?;
    write_new(&directory.join("payload.fit"), bytes)
}

async fn upload_and_verify(
    connection: &crate::maps::device::Connection,
    target: &CourseTarget,
    intent: &Intent,
    directory: &Path,
    active: Arc<ActiveTransfer>,
) -> Result<()> {
    let progress = Arc::clone(&active);
    let reporter = garmin_progress::ProgressReporter::default()
        .with_cancellation(active.cancellation.clone())
        .observe(move |event| {
            if event.stage == OperationStage::Commit && event.unit == ProgressUnit::Bytes {
                let mut value = progress
                    .progress
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                value.bytes_sent = value.bytes_sent.max(event.completed.min(value.total_bytes));
            } else if matches!(
                event.stage,
                OperationStage::DeviceFinalize | OperationStage::DeviceVerify
            ) {
                progress
                    .progress
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .finishing = true;
            }
        });
    connection
        .device
        .upload(
            &target.storage_id,
            &intent.path,
            &directory.join("payload.fit"),
            intent.review.byte_count.as_u64(),
            &intent.sha256,
            garmin_device::MountedMtpUploadProgress {
                reporter,
                completed_before: 0,
                total: intent.review.byte_count.as_u64(),
            },
        )
        .await?;
    {
        let mut progress = active
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        progress.bytes_sent = progress.total_bytes;
        progress.finishing = true;
    }
    connection
        .device
        .verify(
            &target.storage_id,
            &intent.path,
            intent.review.byte_count.as_u64(),
            &intent.sha256,
        )
        .await?;
    Ok(())
}

fn destination(
    manifest: &garmin_device::DeviceManifest,
    target: &CourseTarget,
    file_name: &str,
) -> Result<SafeRelativePath> {
    manifest
        .capabilities()
        .capabilities()
        .iter()
        .filter_map(|capability| capability.course_destination(file_name))
        .find(|path| {
            path.as_path()
                .parent()
                .is_some_and(|parent| parent.to_string_lossy() == target.directory)
        })
        .context("Course destination is no longer declared by the device")
}

fn status(intent: &Intent, phase: CourseTransferPhase) -> CourseTransferStatus {
    CourseTransferStatus {
        transfer: intent.review.transfer,
        generation: intent.review.generation,
        target: intent.review.target.clone(),
        file_name: intent.review.file_name.clone(),
        phase,
        progress: None,
    }
}

fn read_intent(directory: &Path) -> Result<Intent> {
    let bytes = fs::read(directory.join("intent.json"))?;
    ensure!(
        bytes.len() as u64 <= MAX_RECORD_BYTES,
        "Course transfer intent is oversized"
    );
    let intent: Intent = serde_json::from_slice(&bytes)?;
    ensure!(intent.version == 1, "unsupported Course transfer intent");
    let transfer_id = intent.review.transfer.to_string();
    ensure!(
        directory.file_name().and_then(|name| name.to_str()) == Some(transfer_id.as_str()),
        "Course transfer directory does not match its intent"
    );
    ensure!(
        intent.review.file_name == format!("gt-{}.fit", intent.review.transfer),
        "Course transfer file name does not match its intent"
    );
    ensure!(
        intent.path
            == SafeRelativePath::parse(format!(
                "{}/{}",
                intent.review.target.directory, intent.review.file_name
            ))?,
        "Course transfer path does not match its intent"
    );
    ensure!(
        intent.sha256.len() == 64 && intent.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Course transfer checksum is invalid"
    );
    Ok(intent)
}

fn uncommitted_directory(directory: &Path) -> Result<bool> {
    let mut entries = fs::read_dir(directory)?;
    Ok(entries.try_fold(true, |safe, entry| {
        let name = entry?.file_name();
        Ok::<_, std::io::Error>(safe && name == "intent.json")
    })? && !directory.join("payload.fit").exists()
        && !directory.join("outcome.json").exists())
}

fn read_outcome(directory: &Path) -> Result<Option<Outcome>> {
    let bytes = match fs::read(directory.join("outcome.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        bytes.len() as u64 <= MAX_RECORD_BYTES,
        "Course transfer outcome is oversized"
    );
    let outcome: Outcome = serde_json::from_slice(&bytes)?;
    ensure!(outcome.version == 1, "unsupported Course transfer outcome");
    Ok(Some(outcome))
}

fn write_outcome(directory: &Path, phase: &CourseTransferPhase) -> Result<()> {
    let bytes = serde_json::to_vec(&Outcome {
        version: 1,
        phase: phase.clone(),
    })?;
    let temporary = directory.join(format!("outcome-{}.tmp", Uuid::new_v4()));
    write_new(&temporary, &bytes)?;
    fs::rename(&temporary, directory.join("outcome.json"))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(
        path.parent()
            .context("Course transfer path has no parent")?,
    )?
    .sync_all()?;
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("Course transfer root is unavailable")?;
    if !parent.exists() {
        let mut root_builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            root_builder.mode(0o700);
        }
        root_builder.create(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
