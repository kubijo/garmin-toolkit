use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use garmin_capture::SessionCapture;
use garmin_device::storage::{DeviceIoError, DeviceWrite};
use garmin_device::{
    DeviceInventory, DevicePathState, DevicePathStatus, MountedMtpBackupProgress,
    MountedMtpUploadProgress, PathSafetyError, SafeRelativePath, TransportKind,
};
use garmin_model::map::MapAuthorization;
use garmin_progress::{CancellationToken, OperationStage, ProgressReporter};
use md5::{Digest as _, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::device_state::{
    DeviceContentIdentity, DeviceOperationTarget, DeviceOriginalState, DeviceTransactionEventPhase,
};
use crate::{
    ApplyReport, BackupPolicy, DeviceTransactionKind, DeviceTransactionStore, DownloadProgress,
    PortableTransaction, RecoveryOutcome, UpdatePlan,
};

const TRANSACTION_VERSION: u8 = 1;
const DEVICE_STATE_HEADROOM_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct MountedMtpPreflight {
    pub files_to_write: usize,
    pub bytes_to_write: u64,
    pub files_to_remove: usize,
    pub storage_requirements: Vec<crate::space::StorageSpaceRequirement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MountedUpdateRecoveryOutcome {
    Committed,
    AlreadyRolledBack,
    Restored,
    Resumed,
}

#[derive(Debug, Clone, Serialize)]
pub struct MountedUpdateRecoveryReport {
    pub plan_digest: String,
    pub outcome: MountedUpdateRecoveryOutcome,
    pub files_checked: usize,
}

#[derive(Debug, Clone)]
struct Payload {
    target: BoundObject,
    source: PathBuf,
    sha256: String,
}

struct PayloadSource {
    path: SafeRelativePath,
    source: PathBuf,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BoundObject {
    storage_id: String,
    storage_label: String,
    path: SafeRelativePath,
    size: u64,
}

/// Path-and-size identity for an unbacked object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
struct SizeBoundObject(BoundObject);

impl std::ops::Deref for SizeBoundObject {
    type Target = BoundObject;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OriginalObject {
    target: BoundObject,
    backup_file: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct JournalWrite {
    target: BoundObject,
    sha256: String,
    original: Option<OriginalObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unbacked_original: Option<SizeBoundObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload_file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalState {
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MountedTransactionJournal {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_target: Option<String>,
    device_digest: String,
    plan_digest: String,
    #[serde(default)]
    backup_policy: BackupPolicy,
    state: JournalState,
    writes: Vec<JournalWrite>,
    removals: Vec<OriginalObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unbacked_removals: Vec<SizeBoundObject>,
}

struct PreparedMountedUpdate {
    payloads: Vec<Payload>,
    removals: Vec<BoundObject>,
    existing_writes: BTreeMap<String, BoundObject>,
    requirements: Vec<crate::space::StorageSpaceRequirement>,
}

enum RecoveryObjectState {
    Missing,
    Original,
    Remove { size: u64 },
}

enum ResumeWriteState {
    Installed,
    Missing,
    SizeMatchedOriginal,
    Partial { size: u64 },
}

enum ReadyResumeWriteState {
    Installed,
    Missing,
    SizeMatchedOriginal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeWriteOutcome {
    Reused,
    Uploaded,
}

struct ResumeWriteInspection {
    state: ResumeWriteState,
}

struct PreparedResumeWrite {
    state: ReadyResumeWriteState,
    checkpoint: OperationCheckpoint,
}

impl ResumeWriteInspection {
    const fn observed(state: ResumeWriteState) -> Self {
        Self { state }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeRemovalState {
    Missing,
    SizeMatched,
}

struct PreparedResumeRemoval {
    state: ResumeRemovalState,
    checkpoint: OperationCheckpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointKind {
    Started,
    Applied,
    Audited,
}

impl CheckpointKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Applied => "applied",
            Self::Audited => "audited",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OperationIndex(usize);

impl OperationIndex {
    const fn new(index: usize) -> Self {
        Self(index)
    }

    const fn get(self) -> usize {
        self.0
    }

    const fn ordinal(self) -> usize {
        self.0 + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationCheckpoint {
    NotStarted,
    Started,
    Applied,
    Audited,
}

impl OperationCheckpoint {
    const fn from_presence(started: bool, applied: bool, audited: bool) -> Option<Self> {
        match (started, applied, audited) {
            (false, false, false) => Some(Self::NotStarted),
            (true, false, false) => Some(Self::Started),
            (true, true, false) => Some(Self::Applied),
            (true, true, true) => Some(Self::Audited),
            _ => None,
        }
    }

    const fn inspection_priority(self) -> usize {
        match self {
            Self::Started => 0,
            Self::Applied => 1,
            Self::Audited => 2,
            Self::NotStarted => 3,
        }
    }

    const fn is_applied(self) -> bool {
        matches!(self, Self::Applied | Self::Audited)
    }
}

struct UnprotectedResume {
    payloads: Vec<PathBuf>,
    removals: Vec<PreparedResumeRemoval>,
}

/// Validate the exact mounted-MTP transaction without changing the device.
///
/// Matches [`apply_mounted_mtp_with_progress`] verification, binding, and inventory.
/// # Errors
/// Invalid payload or authorization, ambiguous storage, or changed objects.
pub async fn preflight_mounted_mtp_update<D>(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    device: &D,
    capture: &SessionCapture,
    progress: &ProgressReporter,
) -> Result<MountedMtpPreflight, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let prepared = prepare_update(plan, staged, activation, device, capture, progress).await?;
    let bytes_to_write = prepared.payloads.iter().try_fold(0_u64, |total, payload| {
        total
            .checked_add(payload.target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)
    })?;
    Ok(MountedMtpPreflight {
        files_to_write: prepared.payloads.len(),
        bytes_to_write,
        files_to_remove: prepared.removals.len(),
        storage_requirements: prepared.requirements,
    })
}

/// Apply one capture-backed transaction through an existing desktop MTP mount.
/// Backs up replaced objects and checks uploaded metadata; failures roll back.
/// # Errors
/// Preflight, backup, mutation, capture, or rollback failure.
pub async fn apply_mounted_mtp_with_progress<D>(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    device: &D,
    capture: &SessionCapture,
    progress: ProgressReporter,
) -> Result<ApplyReport, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let started = Instant::now();
    let prepared = prepare_update(plan, staged, activation, device, capture, &progress).await?;
    let originals = match plan.backup_policy {
        BackupPolicy::Verified => backup_originals(&prepared, device, capture, &progress).await?,
        BackupPolicy::Skip => {
            progress.completed(
                OperationStage::Backup,
                "Skipped by user; automatic rollback is unavailable",
                0,
                None,
            );
            Vec::new()
        }
    };
    let state = observe_device_state(device, &progress).await?;
    retain_device_state(&progress, &state);
    crate::space::check_device_space(&state, &prepared.requirements)?;
    let mut journal = build_journal(
        plan,
        &prepared,
        originals,
        device.execution_target(),
        capture.root(),
    )?;
    validate_journal(&journal, &plan.device_digest, JournalState::Prepared)?;
    capture
        .write_json_atomic(
            Path::new("mounted-update/transaction/000000-prepared.json"),
            &journal,
        )
        .await?;

    let portable = build_portable_transaction(plan, &prepared, &journal)?;
    let device_state = DeviceTransactionStore::open(device).await?;
    device_state.prepare(&portable).await?;

    let mutation = commit_update(
        &prepared,
        &journal,
        &portable,
        &device_state,
        device,
        capture,
        &progress,
    )
    .await;
    if let Err(operation) = mutation {
        if plan.backup_policy == BackupPolicy::Skip {
            return Err(unprotected_failure(&journal, capture.root(), operation).await);
        }
        let rollback_progress = progress.clone().for_required_cleanup();
        return Err(rollback_failure(
            operation,
            &journal,
            &portable,
            &device_state,
            device,
            capture.root(),
            &rollback_progress,
        )
        .await);
    }

    journal.state = JournalState::Committed;
    if let Err(error) = capture
        .write_json_atomic(
            Path::new("mounted-update/transaction/committed.json"),
            &journal,
        )
        .await
    {
        journal.state = JournalState::Prepared;
        let operation = MountedInstallError::Capture(error);
        if plan.backup_policy == BackupPolicy::Skip {
            return Err(unprotected_failure(&journal, capture.root(), operation).await);
        }
        let rollback_progress = progress.clone().for_required_cleanup();
        return Err(rollback_failure(
            operation,
            &journal,
            &portable,
            &device_state,
            device,
            capture.root(),
            &rollback_progress,
        )
        .await);
    }
    device_state.finish(&portable).await?;
    publish_device_state(device, &progress).await;
    let cleanup = match plan.backup_policy {
        BackupPolicy::Verified => "Verified recovery backups retained in the capture",
        BackupPolicy::Skip => "Update evidence retained; no recovery backup was created",
    };
    progress.completed(OperationStage::Cleanup, cleanup, 1, Some(1));
    Ok(completed_apply_report(plan, &prepared, activation, started))
}

fn completed_apply_report(
    plan: &UpdatePlan,
    prepared: &PreparedMountedUpdate,
    activation: &MapAuthorization,
    started: Instant,
) -> ApplyReport {
    ApplyReport {
        files_written: prepared.payloads.len(),
        bytes_written: plan.total_bytes,
        files_removed: prepared.removals.len(),
        unlocks_written: activation.unlocks.len(),
        backup_policy: plan.backup_policy,
        recovery: RecoveryOutcome::NoTransaction,
        elapsed: started.elapsed(),
    }
}

async fn rollback_failure<D>(
    operation: MountedInstallError,
    journal: &MountedTransactionJournal,
    portable: &PortableTransaction,
    store: &DeviceTransactionStore<'_, D>,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
) -> MountedInstallError
where
    D: DeviceWrite + ?Sized,
{
    let rollback = rollback_update(journal, device, capture_root, progress).await;
    let rollback = match rollback {
        Ok(()) => store.prove_and_clear(portable).await.map_err(Into::into),
        Err(error) => Err(error),
    };
    match rollback {
        Ok(()) => operation,
        Err(rollback) => MountedInstallError::Rollback {
            operation: Box::new(operation),
            rollback: Box::new(rollback),
        },
    }
}

fn build_portable_transaction(
    plan: &UpdatePlan,
    prepared: &PreparedMountedUpdate,
    journal: &MountedTransactionJournal,
) -> Result<PortableTransaction, MountedInstallError> {
    let mut transaction = PortableTransaction::new(
        DeviceTransactionKind::Update,
        plan.device_digest.clone(),
        plan.digest.clone(),
        plan.backup_policy,
    );
    for (download, payload) in plan.downloads.iter().zip(&prepared.payloads) {
        transaction.add_download(download, payload.sha256.clone());
    }
    transaction.set_identifiers(plan.identifiers.clone());
    for write in &journal.writes {
        let target = DeviceOperationTarget::new(
            write.target.storage_id.clone(),
            write.target.storage_label.clone(),
            write.target.path.clone(),
        );
        let expected = DeviceContentIdentity::new(write.target.size, write.sha256.clone());
        let original = match (&write.original, &write.unbacked_original) {
            (Some(original), None) => DeviceOriginalState::Verified(DeviceContentIdentity::new(
                original.target.size,
                original.sha256.clone(),
            )),
            (None, Some(original)) => DeviceOriginalState::Unverified {
                size: original.size,
            },
            (None, None) => DeviceOriginalState::Missing,
            (Some(_), Some(_)) => return Err(MountedInstallError::InvalidJournal),
        };
        transaction.add_write(target, expected, original)?;
    }
    for original in &journal.removals {
        transaction.add_verified_removal(
            DeviceOperationTarget::new(
                original.target.storage_id.clone(),
                original.target.storage_label.clone(),
                original.target.path.clone(),
            ),
            DeviceContentIdentity::new(original.target.size, original.sha256.clone()),
        )?;
    }
    for original in &journal.unbacked_removals {
        transaction.add_unverified_removal(
            DeviceOperationTarget::new(
                original.storage_id.clone(),
                original.storage_label.clone(),
                original.path.clone(),
            ),
            original.size,
        )?;
    }
    Ok(transaction)
}

fn build_journal(
    plan: &UpdatePlan,
    prepared: &PreparedMountedUpdate,
    originals: Vec<OriginalObject>,
    execution_target: Option<&str>,
    capture_root: &Path,
) -> Result<MountedTransactionJournal, MountedInstallError> {
    let originals_by_path = originals
        .iter()
        .cloned()
        .map(|original| (object_key(&original.target), original))
        .collect::<BTreeMap<_, _>>();
    let writes = prepared
        .payloads
        .iter()
        .map(|payload| {
            let unbacked_original = if plan.backup_policy == BackupPolicy::Skip {
                prepared
                    .existing_writes
                    .get(&object_key(&payload.target))
                    .cloned()
                    .map(SizeBoundObject)
            } else {
                None
            };
            Ok(JournalWrite {
                target: payload.target.clone(),
                sha256: payload.sha256.clone(),
                original: originals_by_path.get(&object_key(&payload.target)).cloned(),
                unbacked_original,
                payload_file: Some(
                    payload
                        .source
                        .strip_prefix(capture_root)
                        .map_err(|_| MountedInstallError::UnsafeStagedFile(payload.source.clone()))?
                        .to_string_lossy()
                        .replace('\\', "/"),
                ),
            })
        })
        .collect::<Result<Vec<_>, MountedInstallError>>()?;
    let write_keys = writes
        .iter()
        .map(|write| object_key(&write.target))
        .collect::<BTreeSet<_>>();
    let removals = if plan.backup_policy == BackupPolicy::Verified {
        originals
            .into_iter()
            .filter(|original| !write_keys.contains(&object_key(&original.target)))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let unbacked_removals = if plan.backup_policy == BackupPolicy::Skip {
        prepared
            .removals
            .iter()
            .cloned()
            .map(SizeBoundObject)
            .collect()
    } else {
        Vec::new()
    };
    Ok(MountedTransactionJournal {
        version: TRANSACTION_VERSION,
        execution_target: execution_target.map(str::to_owned),
        device_digest: plan.device_digest.clone(),
        plan_digest: plan.digest.clone(),
        backup_policy: plan.backup_policy,
        state: JournalState::Prepared,
        writes,
        removals,
        unbacked_removals,
    })
}

/// Recover an interrupted mounted-MTP update from its capture directory.
///
/// Backup-free recovery resumes; backed-up recovery rolls back.
/// # Errors
/// Invalid journal, different device, changed objects, or failed rollback.
pub async fn recover_mounted_mtp_update<D>(
    capture_root: &Path,
    device_digest: &str,
    device: &D,
    progress: &ProgressReporter,
) -> Result<MountedUpdateRecoveryReport, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let prepared_path = recovery_file(
        capture_root,
        "mounted-update/transaction/000000-prepared.json",
    )
    .await?;
    let prepared = read_journal(&prepared_path).await?;
    validate_journal(&prepared, device_digest, JournalState::Prepared)?;
    if prepared.execution_target.as_deref() != device.execution_target() {
        return Err(MountedInstallError::RecoveryDeviceMismatch);
    }
    let device_state = DeviceTransactionStore::open(device).await?;
    let portable = device_state.active(device_digest).await?;
    if portable
        .as_ref()
        .is_some_and(|transaction| transaction.plan_digest() != prepared.plan_digest)
    {
        return Err(MountedInstallError::RecoveryPlanMismatch);
    }
    publish_device_state(device, progress).await;
    let committed_path = capture_root.join("mounted-update/transaction/committed.json");
    if tokio::fs::try_exists(&committed_path).await? {
        let committed = read_journal(&committed_path).await?;
        validate_matching_journal(&prepared, &committed, JournalState::Committed)?;
        verify_committed(&committed, device, progress).await?;
        resolve_portable_transaction(&device_state, portable.as_ref()).await?;
        publish_device_state(device, progress).await;
        return Ok(MountedUpdateRecoveryReport {
            plan_digest: committed.plan_digest,
            outcome: MountedUpdateRecoveryOutcome::Committed,
            files_checked: committed.writes.len()
                + committed.removals.len()
                + committed.unbacked_removals.len(),
        });
    }

    if prepared.backup_policy == BackupPolicy::Skip {
        match resume_unprotected_update(&prepared, device, capture_root, progress).await {
            Ok(()) => {}
            Err(operation) => {
                return Err(unprotected_failure(&prepared, capture_root, operation).await);
            }
        }
        resolve_portable_transaction(&device_state, portable.as_ref()).await?;
        publish_device_state(device, progress).await;
        return Ok(MountedUpdateRecoveryReport {
            plan_digest: prepared.plan_digest,
            outcome: MountedUpdateRecoveryOutcome::Resumed,
            files_checked: prepared.writes.len() + prepared.unbacked_removals.len(),
        });
    }

    let rolled_back_path = capture_root.join("mounted-update/transaction/rolled-back.json");
    if tokio::fs::try_exists(&rolled_back_path).await? {
        let rolled_back = read_journal(&rolled_back_path).await?;
        validate_matching_journal(&prepared, &rolled_back, JournalState::RolledBack)?;
        verify_rolled_back(&rolled_back, device, progress).await?;
        resolve_portable_transaction(&device_state, portable.as_ref()).await?;
        publish_device_state(device, progress).await;
        return Ok(MountedUpdateRecoveryReport {
            plan_digest: rolled_back.plan_digest,
            outcome: MountedUpdateRecoveryOutcome::AlreadyRolledBack,
            files_checked: rolled_back.writes.len() + rolled_back.removals.len(),
        });
    }

    rollback_update(&prepared, device, capture_root, progress).await?;
    resolve_portable_transaction(&device_state, portable.as_ref()).await?;
    publish_device_state(device, progress).await;
    Ok(MountedUpdateRecoveryReport {
        plan_digest: prepared.plan_digest,
        outcome: MountedUpdateRecoveryOutcome::Restored,
        files_checked: prepared.writes.len() + prepared.removals.len(),
    })
}

async fn resolve_portable_transaction<D>(
    store: &DeviceTransactionStore<'_, D>,
    transaction: Option<&PortableTransaction>,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    if let Some(transaction) = transaction {
        store.finish(transaction).await?;
    }
    Ok(())
}

async fn read_journal(path: &Path) -> Result<MountedTransactionJournal, MountedInstallError> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(MountedInstallError::UnsafeJournal(path.to_owned()));
    }
    Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
}

fn validate_journal(
    journal: &MountedTransactionJournal,
    device_digest: &str,
    state: JournalState,
) -> Result<(), MountedInstallError> {
    if journal.version != TRANSACTION_VERSION {
        return Err(MountedInstallError::JournalVersion(journal.version));
    }
    if journal.device_digest != device_digest {
        return Err(MountedInstallError::RecoveryDeviceMismatch);
    }
    if journal.state != state || journal.writes.is_empty() {
        return Err(MountedInstallError::InvalidJournal);
    }
    if journal.backup_policy == BackupPolicy::Verified
        && (!journal.unbacked_removals.is_empty()
            || journal
                .writes
                .iter()
                .any(|write| write.unbacked_original.is_some()))
    {
        return Err(MountedInstallError::InvalidJournal);
    }
    if journal.backup_policy == BackupPolicy::Skip
        && (!journal.removals.is_empty()
            || journal.writes.iter().any(|write| write.original.is_some()))
    {
        return Err(MountedInstallError::InvalidJournal);
    }
    let mut paths = BTreeSet::new();
    for write in &journal.writes {
        if !paths.insert(object_key(&write.target)) {
            return Err(MountedInstallError::InvalidJournal);
        }
        if write.payload_file.is_none() {
            return Err(MountedInstallError::InvalidJournal);
        }
        if let Some(path) = &write.payload_file {
            SafeRelativePath::parse(path)?;
            if !path.starts_with("mounted-update/payloads/") {
                return Err(MountedInstallError::InvalidJournal);
            }
        }
        if let Some(original) = &write.original {
            validate_original(original)?;
            if object_key(&original.target) != object_key(&write.target) {
                return Err(MountedInstallError::InvalidJournal);
            }
        }
        if let Some(original) = &write.unbacked_original
            && object_key(original) != object_key(&write.target)
        {
            return Err(MountedInstallError::InvalidJournal);
        }
    }
    for removal in &journal.removals {
        validate_original(removal)?;
        if !paths.insert(object_key(&removal.target)) {
            return Err(MountedInstallError::InvalidJournal);
        }
    }
    for removal in &journal.unbacked_removals {
        if !paths.insert(object_key(removal)) {
            return Err(MountedInstallError::InvalidJournal);
        }
    }
    Ok(())
}

fn validate_original(original: &OriginalObject) -> Result<(), MountedInstallError> {
    SafeRelativePath::parse(&original.backup_file)?;
    if !original.backup_file.starts_with("mounted-update/backups/") {
        return Err(MountedInstallError::InvalidJournal);
    }
    Ok(())
}

fn validate_matching_journal(
    prepared: &MountedTransactionJournal,
    candidate: &MountedTransactionJournal,
    state: JournalState,
) -> Result<(), MountedInstallError> {
    validate_journal(candidate, &prepared.device_digest, state)?;
    let mut expected = prepared.clone();
    expected.state = state;
    if *candidate != expected {
        return Err(MountedInstallError::InvalidJournal);
    }
    Ok(())
}

async fn verify_committed<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    for write in &journal.writes {
        check_device_object_metadata(device, &write.target, progress).await?;
    }
    for removal in &journal.removals {
        let (state, size) = device
            .inspect_with_progress(&removal.target.storage_id, &removal.target.path, progress)
            .await?
            .into_parts();
        if state != DevicePathState::Missing {
            return Err(MountedInstallError::UnexpectedDeviceState {
                path: removal.target.path.clone(),
                state,
                size,
            });
        }
    }
    for removal in &journal.unbacked_removals {
        let (state, size) = device
            .inspect_with_progress(&removal.storage_id, &removal.path, progress)
            .await?
            .into_parts();
        if state != DevicePathState::Missing {
            return Err(MountedInstallError::UnexpectedDeviceState {
                path: removal.path.clone(),
                state,
                size,
            });
        }
    }
    Ok(())
}

async fn verify_rolled_back<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    for write in &journal.writes {
        if let Some(original) = &write.original {
            check_device_object_metadata(device, &original.target, progress).await?;
        } else {
            let (state, size) = device
                .inspect_with_progress(&write.target.storage_id, &write.target.path, progress)
                .await?
                .into_parts();
            if state != DevicePathState::Missing {
                return Err(MountedInstallError::UnexpectedDeviceState {
                    path: write.target.path.clone(),
                    state,
                    size,
                });
            }
        }
    }
    for removal in &journal.removals {
        check_device_object_metadata(device, &removal.target, progress).await?;
    }
    Ok(())
}

async fn prepare_update<D>(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    device: &D,
    capture: &SessionCapture,
    progress: &ProgressReporter,
) -> Result<PreparedMountedUpdate, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    validate_authorization(activation)?;
    if plan.downloads.len() != staged.len() {
        return Err(MountedInstallError::IncompleteStage {
            expected: plan.downloads.len(),
            actual: staged.len(),
        });
    }
    let sources = prepare_payload_sources(plan, staged, activation, capture, progress).await?;

    let write_keys = sources
        .iter()
        .map(|source| path_key(&source.path))
        .collect::<BTreeSet<_>>();
    let mut paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    paths.extend(
        plan.files_to_remove
            .iter()
            .filter(|path| !write_keys.contains(&path_key(path)))
            .cloned(),
    );
    let inventory = device.inventory(&paths).await?;
    let primary_storage = device.primary_storage_id().await?;
    let mut payloads = sources
        .into_iter()
        .map(|source| {
            Ok(Payload {
                target: bind_write_target(&inventory, &primary_storage, source.path, source.size)?,
                source: source.source,
                sha256: source.sha256,
            })
        })
        .collect::<Result<Vec<_>, MountedInstallError>>()?;
    let removals = plan
        .files_to_remove
        .iter()
        .filter(|path| !write_keys.contains(&path_key(path)))
        .filter_map(|path| bind_removal_target(&inventory, path).transpose())
        .collect::<Result<Vec<_>, MountedInstallError>>()?;
    let existing_writes = bind_existing_write_targets(&payloads, &inventory)?;
    progress.completed(
        OperationStage::Stage,
        "Device transaction validated",
        plan.total_bytes,
        Some(plan.total_bytes),
    );
    let (requirements, backup_bytes) = space_requirements(&payloads, &removals, &inventory)?;
    let snapshot = observe_device_state(device, progress).await?;
    crate::space::check_device_space(&snapshot, &requirements)?;
    let retained_backup_bytes = match plan.backup_policy {
        BackupPolicy::Verified => backup_bytes,
        BackupPolicy::Skip => 0,
    };
    retain_payloads(&mut payloads, retained_backup_bytes, capture).await?;
    Ok(PreparedMountedUpdate {
        payloads,
        removals,
        existing_writes,
        requirements,
    })
}

async fn prepare_payload_sources(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    capture: &SessionCapture,
    progress: &ProgressReporter,
) -> Result<Vec<PayloadSource>, MountedInstallError> {
    let mut sources = Vec::with_capacity(plan.downloads.len() + activation.unlocks.len());
    let mut destinations = BTreeSet::new();
    progress.started(
        OperationStage::Stage,
        "Validating the device transaction",
        Some(plan.total_bytes),
    );
    let mut verified = 0_u64;
    for (spec, staged_file) in plan.downloads.iter().zip(staged) {
        register_destination(&mut destinations, &spec.destination)?;
        let sha256 = verify_local_payload(&staged_file.path, spec.size, Some(&spec.md5)).await?;
        verified = verified
            .checked_add(spec.size)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        progress.advanced_with_path(
            OperationStage::Stage,
            "Validated update payload",
            spec.destination.to_string(),
            verified,
            Some(plan.total_bytes),
        );
        sources.push(PayloadSource {
            path: spec.destination.clone(),
            source: staged_file.path.clone(),
            size: spec.size,
            sha256,
        });
    }
    for (index, unlock) in activation.unlocks.iter().enumerate() {
        let path = SafeRelativePath::parse(&unlock.file_name)?;
        register_destination(&mut destinations, &path)?;
        let bytes = BASE64.decode(&unlock.gma)?;
        let relative = PathBuf::from(format!("mounted-update/payloads/{:06}.bin", index + 1));
        retain_authorization(capture, &relative, &bytes).await?;
        let source = capture.root().join(relative);
        let size =
            u64::try_from(bytes.len()).map_err(|_| MountedInstallError::ByteCountOverflow)?;
        sources.push(PayloadSource {
            path,
            source,
            size,
            sha256: hex::encode(Sha256::digest(&bytes)),
        });
    }
    Ok(sources)
}

fn bind_existing_write_targets(
    payloads: &[Payload],
    inventory: &DeviceInventory,
) -> Result<BTreeMap<String, BoundObject>, MountedInstallError> {
    payloads
        .iter()
        .filter_map(|payload| {
            inventory
                .paths
                .iter()
                .find(|item| {
                    item.storage_id == payload.target.storage_id
                        && path_key(&item.path) == path_key(&payload.target.path)
                        && item.state == DevicePathState::RegularFile
                })
                .map(|item| {
                    let size = item
                        .size
                        .ok_or_else(|| MountedInstallError::MissingDeviceSize(item.path.clone()))?;
                    Ok((
                        object_key(&payload.target),
                        BoundObject {
                            storage_id: item.storage_id.clone(),
                            storage_label: item.storage_label.clone(),
                            path: item.path.clone(),
                            size,
                        },
                    ))
                })
        })
        .collect()
}

fn space_requirements(
    payloads: &[Payload],
    removals: &[BoundObject],
    inventory: &DeviceInventory,
) -> Result<(Vec<crate::space::StorageSpaceRequirement>, u64), MountedInstallError> {
    let mut changes = Vec::new();
    let mut backup_bytes = 0_u64;
    for payload in payloads {
        let before = inventory
            .paths
            .iter()
            .find(|item| {
                item.storage_id == payload.target.storage_id
                    && path_key(&item.path) == path_key(&payload.target.path)
            })
            .and_then(|item| item.size)
            .unwrap_or(0);
        backup_bytes = backup_bytes
            .checked_add(before)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        changes.push(crate::space::StorageChange {
            storage_id: &payload.target.storage_id,
            before_bytes: before,
            after_bytes: payload.target.size,
        });
    }
    for removal in removals {
        backup_bytes = backup_bytes
            .checked_add(removal.size)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        changes.push(crate::space::StorageChange {
            storage_id: &removal.storage_id,
            before_bytes: removal.size,
            after_bytes: 0,
        });
    }
    let mut requirements = crate::space::storage_requirements(changes)?;
    for requirement in &mut requirements {
        requirement.required_free_bytes = requirement
            .required_free_bytes
            .checked_add(DEVICE_STATE_HEADROOM_BYTES)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
    }
    Ok((requirements, backup_bytes))
}

async fn retain_authorization(
    capture: &SessionCapture,
    relative: &Path,
    bytes: &[u8],
) -> Result<(), MountedInstallError> {
    match tokio::fs::symlink_metadata(capture.root().join(relative)).await {
        Ok(_) => {
            let retained = recovery_file(capture.root(), &relative.to_string_lossy()).await?;
            if tokio::fs::read(retained).await? != bytes {
                return Err(MountedInstallError::InvalidJournal);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            capture.write_bytes(relative, bytes).await?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn retain_payloads(
    payloads: &mut [Payload],
    backup_bytes: u64,
    capture: &SessionCapture,
) -> Result<(), MountedInstallError> {
    let retention_bytes = payloads
        .iter()
        .filter(|payload| {
            !payload
                .source
                .starts_with(capture.root().join("mounted-update/payloads"))
        })
        .try_fold(backup_bytes, |sum, payload| {
            sum.checked_add(payload.target.size)
                .ok_or(MountedInstallError::ByteCountOverflow)
        })?;
    crate::space::check_host_space(capture.root(), retention_bytes).await?;
    for (index, payload) in payloads.iter_mut().enumerate() {
        if !payload
            .source
            .starts_with(capture.root().join("mounted-update/payloads"))
        {
            let relative =
                PathBuf::from(format!("mounted-update/payloads/recovery-{index:06}.bin"));
            match tokio::fs::symlink_metadata(capture.root().join(&relative)).await {
                Ok(_) => {
                    recovery_file(capture.root(), &relative.to_string_lossy()).await?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    capture.retain_file(&relative, &payload.source).await?;
                }
                Err(error) => return Err(error.into()),
            }
            payload.source = capture.root().join(relative);
            if verify_local_payload(&payload.source, payload.target.size, None).await?
                != payload.sha256
            {
                return Err(MountedInstallError::StagedChecksum {
                    expected: payload.sha256.clone(),
                    actual: "retained payload changed".to_owned(),
                });
            }
        }
    }
    Ok(())
}

fn validate_authorization(activation: &MapAuthorization) -> Result<(), MountedInstallError> {
    if !activation.embedded_unlocks.is_empty() {
        return Err(MountedInstallError::EmbeddedUnlocksUnsupported);
    }
    if activation.signed_sd_card_bytes.is_some() {
        return Err(MountedInstallError::SignedStorageDataUnsupported);
    }
    Ok(())
}

fn register_destination(
    destinations: &mut BTreeSet<String>,
    path: &SafeRelativePath,
) -> Result<(), MountedInstallError> {
    if destinations.insert(path_key(path)) {
        Ok(())
    } else {
        Err(MountedInstallError::DuplicateDestination(path.clone()))
    }
}

fn bind_write_target(
    inventory: &DeviceInventory,
    primary_storage: &str,
    path: SafeRelativePath,
    size: u64,
) -> Result<BoundObject, MountedInstallError> {
    let matches = inventory_for_path(inventory, &path)?;
    let present = regular_files(&matches);
    match present.as_slice() {
        [item] => Ok(BoundObject {
            storage_id: item.storage_id.clone(),
            storage_label: item.storage_label.clone(),
            path,
            size,
        }),
        [] => {
            let storage = matches
                .iter()
                .find(|item| item.storage_id == primary_storage)
                .ok_or_else(|| MountedInstallError::PrimaryStorageMissing(path.clone()))?;
            Ok(BoundObject {
                storage_id: storage.storage_id.clone(),
                storage_label: storage.storage_label.clone(),
                path,
                size,
            })
        }
        _ => Err(MountedInstallError::AmbiguousStorage(path)),
    }
}

fn bind_removal_target(
    inventory: &DeviceInventory,
    path: &SafeRelativePath,
) -> Result<Option<BoundObject>, MountedInstallError> {
    let matches = inventory_for_path(inventory, path)?;
    let present = regular_files(&matches);
    match present.as_slice() {
        [] => Ok(None),
        [item] => Ok(Some(BoundObject {
            storage_id: item.storage_id.clone(),
            storage_label: item.storage_label.clone(),
            path: path.clone(),
            size: item
                .size
                .ok_or_else(|| MountedInstallError::MissingDeviceSize(path.clone()))?,
        })),
        _ => Err(MountedInstallError::AmbiguousStorage(path.clone())),
    }
}

fn inventory_for_path<'a>(
    inventory: &'a DeviceInventory,
    path: &SafeRelativePath,
) -> Result<Vec<&'a garmin_device::DevicePathInspection>, MountedInstallError> {
    let matches = inventory
        .paths
        .iter()
        .filter(|item| path_key(&item.path) == path_key(path))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(MountedInstallError::IncompleteInventory(path.clone()));
    }
    if let Some(item) = matches.iter().find(|item| {
        matches!(
            item.state,
            DevicePathState::Directory | DevicePathState::Other | DevicePathState::Ambiguous
        )
    }) {
        return Err(MountedInstallError::UnsafeDeviceObject {
            path: path.clone(),
            storage: item.storage_label.clone(),
            state: item.state,
        });
    }
    Ok(matches)
}

fn regular_files<'a>(
    inspected: &[&'a garmin_device::DevicePathInspection],
) -> Vec<&'a garmin_device::DevicePathInspection> {
    inspected
        .iter()
        .copied()
        .filter(|item| item.state == DevicePathState::RegularFile)
        .collect()
}

async fn backup_originals<D>(
    prepared: &PreparedMountedUpdate,
    device: &D,
    capture: &SessionCapture,
    progress: &ProgressReporter,
) -> Result<Vec<OriginalObject>, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let mut targets = Vec::new();
    for payload in &prepared.payloads {
        let (state, size) = device
            .inspect_with_progress(&payload.target.storage_id, &payload.target.path, progress)
            .await?
            .into_parts();
        match (state, size) {
            (DevicePathState::Missing, None) => {}
            (DevicePathState::RegularFile, Some(size)) => {
                let mut target = payload.target.clone();
                target.size = size;
                targets.push(target);
            }
            _ => {
                return Err(MountedInstallError::UnexpectedDeviceState {
                    path: payload.target.path.clone(),
                    state,
                    size,
                });
            }
        }
    }
    targets.extend(prepared.removals.iter().cloned());
    let total = targets.iter().try_fold(0_u64, |sum, target| {
        sum.checked_add(target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)
    })?;
    progress.started(
        OperationStage::Backup,
        "Backing up files affected by the update",
        Some(total),
    );
    let mut completed = 0_u64;
    let mut originals = Vec::with_capacity(targets.len());
    for (index, target) in targets.into_iter().enumerate() {
        let backup_file = format!("mounted-update/backups/{:06}.bin", index + 1);
        let backup_path = capture.artifact_path(Path::new(&backup_file))?;
        let destination = garmin_device::BackupDestination::new(
            backup_path,
            capture
                .create_file(Path::new(&backup_file))
                .await?
                .into_std()
                .await,
        )?;
        let sha256 = device
            .backup(
                &target.storage_id,
                &target.path,
                target.size,
                destination,
                MountedMtpBackupProgress {
                    reporter: progress.clone(),
                    completed_before: completed,
                    total,
                },
            )
            .await?;
        completed = completed
            .checked_add(target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        originals.push(OriginalObject {
            target,
            backup_file,
            sha256,
        });
    }
    progress.completed(
        OperationStage::Backup,
        "All affected device files are backed up",
        completed,
        Some(total),
    );
    Ok(originals)
}

async fn commit_update<D>(
    prepared: &PreparedMountedUpdate,
    journal: &MountedTransactionJournal,
    portable: &PortableTransaction,
    device_state: &DeviceTransactionStore<'_, D>,
    device: &D,
    capture: &SessionCapture,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let total_bytes = prepared.payloads.iter().try_fold(0_u64, |sum, payload| {
        sum.checked_add(payload.target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)
    })?;
    progress.started(
        OperationStage::Commit,
        "Writing verified update files",
        Some(total_bytes),
    );
    let mut completed_bytes = 0_u64;
    let mut operation = 0_usize;
    for (payload, write) in prepared.payloads.iter().zip(&journal.writes) {
        let item_progress = progress.for_item(format!("update-write-{:06}", operation + 1));
        require_running(progress)?;
        ensure_current_write_capacity(device, write, progress).await?;
        let operation_index = OperationIndex::new(operation);
        write_transaction_event(capture, operation_index, CheckpointKind::Started, write).await?;
        device_state
            .event(
                portable,
                u32::try_from(operation).map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Started,
            )
            .await?;
        remove_replaced_target(write, device, progress).await?;
        upload_with_device_state_monitor(
            device,
            &payload.target.storage_id,
            &payload.target.path,
            &payload.source,
            payload.target.size,
            &payload.sha256,
            MountedMtpUploadProgress {
                reporter: item_progress,
                completed_before: 0,
                total: payload.target.size,
            },
            progress,
        )
        .await?;
        completed_bytes = completed_bytes
            .checked_add(payload.target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        progress.advanced_with_path(
            OperationStage::Commit,
            "Wrote and size-checked update file",
            write.target.path.to_string(),
            completed_bytes,
            Some(total_bytes),
        );
        operation += 1;
        write_transaction_event(capture, operation_index, CheckpointKind::Applied, write).await?;
        device_state
            .event(
                portable,
                u32::try_from(operation_index.get())
                    .map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Applied,
            )
            .await?;
        device_state
            .event(
                portable,
                u32::try_from(operation_index.get())
                    .map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Verified,
            )
            .await?;
    }
    commit_removals(
        journal,
        portable,
        device_state,
        device,
        capture,
        progress,
        operation,
    )
    .await?;
    progress.completed(
        OperationStage::Commit,
        "Update files written and size checked",
        completed_bytes,
        Some(total_bytes),
    );
    progress.started(
        OperationStage::Cleanup,
        "Retaining transaction recovery files",
        Some(1),
    );
    Ok(())
}

async fn remove_replaced_target<D>(
    write: &JournalWrite,
    device: &D,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let removal = if let Some(original) = &write.original {
        device
            .delete_size_checked_with_progress(
                &original.target.storage_id,
                &original.target.path,
                original.target.size,
                progress,
            )
            .await
    } else if let Some(original) = &write.unbacked_original {
        device
            .delete_size_checked_with_progress(
                &original.storage_id,
                &original.path,
                original.size,
                progress,
            )
            .await
    } else {
        return Ok(());
    };
    refresh_after_mutation(device, progress, removal).await
}

async fn commit_removals<D>(
    journal: &MountedTransactionJournal,
    portable: &PortableTransaction,
    device_state: &DeviceTransactionStore<'_, D>,
    device: &D,
    capture: &SessionCapture,
    progress: &ProgressReporter,
    mut operation: usize,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    for original in &journal.removals {
        require_running(progress)?;
        let operation_index = OperationIndex::new(operation);
        write_transaction_event(capture, operation_index, CheckpointKind::Started, original)
            .await?;
        device_state
            .event(
                portable,
                u32::try_from(operation).map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Started,
            )
            .await?;
        let removal = device
            .delete_size_checked_with_progress(
                &original.target.storage_id,
                &original.target.path,
                original.target.size,
                progress,
            )
            .await;
        refresh_after_mutation(device, progress, removal).await?;
        operation += 1;
        write_transaction_event(capture, operation_index, CheckpointKind::Applied, original)
            .await?;
        device_state
            .event(
                portable,
                u32::try_from(operation_index.get())
                    .map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Applied,
            )
            .await?;
        device_state
            .event(
                portable,
                u32::try_from(operation_index.get())
                    .map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Verified,
            )
            .await?;
    }
    for target in &journal.unbacked_removals {
        require_running(progress)?;
        let operation_index = OperationIndex::new(operation);
        write_transaction_event(capture, operation_index, CheckpointKind::Started, target).await?;
        device_state
            .event(
                portable,
                u32::try_from(operation).map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Started,
            )
            .await?;
        let removal = device
            .delete_size_checked_with_progress(
                &target.storage_id,
                &target.path,
                target.size,
                progress,
            )
            .await;
        refresh_after_mutation(device, progress, removal).await?;
        operation += 1;
        write_transaction_event(capture, operation_index, CheckpointKind::Applied, target).await?;
        device_state
            .event(
                portable,
                u32::try_from(operation_index.get())
                    .map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Applied,
            )
            .await?;
        device_state
            .event(
                portable,
                u32::try_from(operation_index.get())
                    .map_err(|_| MountedInstallError::ByteCountOverflow)?,
                DeviceTransactionEventPhase::Verified,
            )
            .await?;
    }
    Ok(())
}

async fn ensure_current_write_capacity<D>(
    device: &D,
    write: &JournalWrite,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let before_bytes = write
        .original
        .as_ref()
        .map(|original| original.target.size)
        .or_else(|| {
            write
                .unbacked_original
                .as_ref()
                .map(|original| original.size)
        })
        .unwrap_or(0);
    let mut requirements = crate::space::storage_requirements([crate::space::StorageChange {
        storage_id: &write.target.storage_id,
        before_bytes,
        after_bytes: write.target.size,
    }])?;
    requirements[0].required_free_bytes = requirements[0]
        .required_free_bytes
        .checked_add(DEVICE_STATE_HEADROOM_BYTES)
        .ok_or(MountedInstallError::ByteCountOverflow)?;
    let snapshot = observe_device_state(device, progress).await?;
    crate::space::check_device_space(&snapshot, &requirements)?;
    Ok(())
}

async fn resume_unprotected_update<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let checkpoints = load_resume_checkpoints(journal, capture_root).await?;
    let resume =
        prepare_unprotected_resume(journal, device, capture_root, progress, &checkpoints).await?;
    let total_writes =
        u64::try_from(journal.writes.len()).map_err(|_| MountedInstallError::ByteCountOverflow)?;
    progress.started_operations(
        OperationStage::Commit,
        "Reconciling interrupted update files",
        Some(total_writes),
    );
    let completed_writes = execute_unprotected_resume(
        journal,
        device,
        capture_root,
        progress,
        resume,
        &checkpoints,
    )
    .await?;

    progress.completed_operations(
        OperationStage::Commit,
        "Interrupted update files reconciled by path and size",
        completed_writes,
        Some(total_writes),
    );
    progress.started(
        OperationStage::Cleanup,
        "Committing the reconciled transaction",
        Some(1),
    );
    let mut committed = journal.clone();
    committed.state = JournalState::Committed;
    write_recovery_marker(
        &capture_root.join("mounted-update/transaction/committed.json"),
        &committed,
    )
    .await?;
    progress.completed(
        OperationStage::Cleanup,
        "Reconciled update evidence retained",
        1,
        Some(1),
    );
    Ok(())
}

async fn prepare_unprotected_resume<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
    checkpoints: &[OperationCheckpoint],
) -> Result<UnprotectedResume, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let payloads = verify_resume_payloads(journal, capture_root, progress).await?;
    let removals = inspect_resume_removals(journal, device, progress, checkpoints).await?;
    Ok(UnprotectedResume { payloads, removals })
}

async fn verify_resume_payloads(
    journal: &MountedTransactionJournal,
    capture_root: &Path,
    progress: &ProgressReporter,
) -> Result<Vec<PathBuf>, MountedInstallError> {
    let total_bytes = journal.writes.iter().try_fold(0_u64, |sum, write| {
        sum.checked_add(write.target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)
    })?;
    progress.started(
        OperationStage::Verify,
        "Verifying retained recovery payloads",
        Some(total_bytes),
    );
    let mut payloads = Vec::with_capacity(journal.writes.len());
    let mut payload_bytes = 0_u64;
    for write in &journal.writes {
        require_running(progress)?;
        let payload = recovery_file(
            capture_root,
            write
                .payload_file
                .as_deref()
                .ok_or(MountedInstallError::InvalidJournal)?,
        )
        .await?;
        let completed_before = payload_bytes;
        if verify_local_payload_with_progress(&payload, write.target.size, None, |current| {
            require_running(progress)?;
            progress.advanced_with_path(
                OperationStage::Verify,
                "Verifying retained recovery payload",
                write.target.path.to_string(),
                completed_before.saturating_add(current),
                Some(total_bytes),
            );
            Ok(())
        })
        .await?
            != write.sha256
        {
            return Err(MountedInstallError::RecoveryEvidence(
                write.target.path.clone(),
            ));
        }
        payload_bytes = payload_bytes
            .checked_add(write.target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        payloads.push(payload);
    }
    progress.completed(
        OperationStage::Verify,
        "Retained recovery payloads verified",
        payload_bytes,
        Some(total_bytes),
    );
    Ok(payloads)
}

async fn inspect_resume_removals<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    progress: &ProgressReporter,
    checkpoints: &[OperationCheckpoint],
) -> Result<Vec<PreparedResumeRemoval>, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let mut removals = Vec::with_capacity(journal.unbacked_removals.len());
    let first_removal = journal.writes.len() + journal.removals.len();
    for (index, target) in journal.unbacked_removals.iter().enumerate() {
        require_running(progress)?;
        let item = progress.for_item(format!("recovery-inspect-removal:{}", target.path));
        item.started_operations_with_path(
            OperationStage::DeviceVerify,
            "Inspecting superseded device file",
            target.path.to_string(),
            Some(1),
        );
        let state = match inspect_resume_removal(device, target, progress).await {
            Ok(state) => state,
            Err(error) => {
                item.failed_with_path(
                    OperationStage::DeviceVerify,
                    "Superseded device file inspection failed",
                    target.path.to_string(),
                );
                return Err(error);
            }
        };
        let label = match state {
            ResumeRemovalState::Missing => "Superseded device file is already absent",
            ResumeRemovalState::SizeMatched => {
                "Superseded device path and size match the unprotected journal"
            }
        };
        item.completed_operations_with_path(
            OperationStage::DeviceVerify,
            label,
            target.path.to_string(),
            1,
            Some(1),
        );
        let checkpoint = checkpoints
            .get(first_removal + index)
            .copied()
            .ok_or(MountedInstallError::InvalidJournal)?;
        if checkpoint == OperationCheckpoint::Audited
            || (checkpoint == OperationCheckpoint::Applied && state != ResumeRemovalState::Missing)
        {
            return Err(MountedInstallError::RecoveryEvidence(target.path.clone()));
        }
        removals.push(PreparedResumeRemoval { state, checkpoint });
    }
    Ok(removals)
}

async fn execute_unprotected_resume<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
    mut resume: UnprotectedResume,
    checkpoints: &[OperationCheckpoint],
) -> Result<u64, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let prepared_writes =
        inspect_resume_writes(journal, device, capture_root, progress, checkpoints).await?;
    let completed_bytes = apply_prepared_resume_writes(
        journal,
        device,
        capture_root,
        progress,
        &mut resume,
        prepared_writes,
    )
    .await?;
    execute_resume_removals(journal, device, capture_root, progress, resume.removals).await?;
    Ok(completed_bytes)
}

async fn inspect_resume_writes<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
    checkpoints: &[OperationCheckpoint],
) -> Result<Vec<PreparedResumeWrite>, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let total =
        u64::try_from(journal.writes.len()).map_err(|_| MountedInstallError::ByteCountOverflow)?;
    let mut prepared_writes = (0..journal.writes.len())
        .map(|_| None)
        .collect::<Vec<Option<PreparedResumeWrite>>>();
    progress.started_operations(
        OperationStage::DeviceVerify,
        "Checking current device file paths and sizes",
        Some(total),
    );
    let mut inspected = 0_u64;
    for index in resume_inspection_order(journal, checkpoints)? {
        let operation_index = OperationIndex::new(index);
        let operation = operation_index.ordinal();
        let write = &journal.writes[index];
        let item_progress = progress.for_item(format!("recovery-write-{operation:06}"));
        require_running(progress)?;
        let ResumeWriteInspection { state } = inspect_resume_write(
            device,
            write,
            capture_root,
            operation,
            checkpoints[index],
            &item_progress,
        )
        .await?;
        let state =
            prepare_resume_write_state(checkpoints[index], write, device, &item_progress, state)
                .await?;
        let checkpoint = retain_inspected_resume_checkpoint(
            checkpoints[index],
            &state,
            write,
            capture_root,
            operation_index,
        )
        .await?;
        prepared_writes[index] = Some(PreparedResumeWrite { state, checkpoint });
        inspected = inspected
            .checked_add(1)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        progress.advanced_operations_with_path(
            OperationStage::DeviceVerify,
            "Inspected current device file",
            write.target.path.to_string(),
            inspected,
            Some(total),
        );
    }
    progress.completed_operations(
        OperationStage::DeviceVerify,
        "Current device file paths and sizes checked",
        inspected,
        Some(inspected),
    );
    prepared_writes
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(MountedInstallError::InvalidJournal)
}

async fn apply_prepared_resume_writes<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
    resume: &mut UnprotectedResume,
    prepared_writes: Vec<PreparedResumeWrite>,
) -> Result<u64, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let total =
        u64::try_from(journal.writes.len()).map_err(|_| MountedInstallError::ByteCountOverflow)?;
    let mut completed = 0_u64;
    for (index, ((write, payload), prepared_write)) in journal
        .writes
        .iter()
        .zip(&resume.payloads)
        .zip(prepared_writes)
        .enumerate()
    {
        let operation_index = OperationIndex::new(index);
        let operation = operation_index.ordinal();
        let item_progress = progress.for_item(format!("recovery-write-{operation:06}"));
        let PreparedResumeWrite { state, checkpoint } = prepared_write;
        let reused = matches!(state, ReadyResumeWriteState::Installed);
        require_running(progress)?;
        if checkpoint == OperationCheckpoint::NotStarted {
            write_recovery_event(
                capture_root,
                operation_index,
                CheckpointKind::Started,
                write,
            )
            .await?;
        }
        ensure_resume_write_capacity(
            journal,
            write,
            &state,
            device,
            capture_root,
            progress,
            &mut resume.removals,
        )
        .await?;
        let outcome = apply_resume_write(device, write, payload, state, &item_progress).await?;
        completed = completed
            .checked_add(1)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        progress.advanced_operations_with_path(
            OperationStage::Commit,
            "Reconciled update file",
            write.target.path.to_string(),
            completed,
            Some(total),
        );
        if !checkpoint.is_applied() {
            write_recovery_event(
                capture_root,
                operation_index,
                CheckpointKind::Applied,
                write,
            )
            .await?;
        }
        if checkpoint != OperationCheckpoint::Audited
            && (reused || outcome == ResumeWriteOutcome::Uploaded)
        {
            write_recovery_event(
                capture_root,
                operation_index,
                CheckpointKind::Audited,
                write,
            )
            .await?;
        }
    }
    Ok(completed)
}

async fn retain_inspected_resume_checkpoint(
    checkpoint: OperationCheckpoint,
    state: &ReadyResumeWriteState,
    write: &JournalWrite,
    capture_root: &Path,
    operation: OperationIndex,
) -> Result<OperationCheckpoint, MountedInstallError> {
    if !matches!(state, ReadyResumeWriteState::Installed) {
        return Ok(checkpoint);
    }
    match checkpoint {
        OperationCheckpoint::Started => {
            write_recovery_event(capture_root, operation, CheckpointKind::Applied, write).await?;
            write_recovery_event(capture_root, operation, CheckpointKind::Audited, write).await?;
            Ok(OperationCheckpoint::Audited)
        }
        OperationCheckpoint::Applied => {
            write_recovery_event(capture_root, operation, CheckpointKind::Audited, write).await?;
            Ok(OperationCheckpoint::Audited)
        }
        OperationCheckpoint::NotStarted | OperationCheckpoint::Audited => Ok(checkpoint),
    }
}

async fn prepare_resume_write_state<D>(
    checkpoint: OperationCheckpoint,
    write: &JournalWrite,
    device: &D,
    progress: &ProgressReporter,
    state: ResumeWriteState,
) -> Result<ReadyResumeWriteState, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    match (checkpoint, state) {
        (_, ResumeWriteState::Installed) => Ok(ReadyResumeWriteState::Installed),
        (OperationCheckpoint::Applied | OperationCheckpoint::Audited, _)
        | (OperationCheckpoint::NotStarted, ResumeWriteState::Partial { .. }) => Err(
            MountedInstallError::RecoveryEvidence(write.target.path.clone()),
        ),
        (_, ResumeWriteState::Missing) => Ok(ReadyResumeWriteState::Missing),
        (_, ResumeWriteState::SizeMatchedOriginal) => {
            Ok(ReadyResumeWriteState::SizeMatchedOriginal)
        }
        (OperationCheckpoint::Started, ResumeWriteState::Partial { size }) => {
            remove_incomplete_resume_write(write, device, progress, size).await?;
            Ok(ReadyResumeWriteState::Missing)
        }
    }
}

async fn remove_incomplete_resume_write<D>(
    write: &JournalWrite,
    device: &D,
    progress: &ProgressReporter,
    size: u64,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    require_running(progress)?;
    let before = observe_device_state(device, progress).await?;
    progress.started_with_path(
        OperationStage::Commit,
        "Removing proven incomplete update file",
        write.target.path.to_string(),
        Some(size),
    );
    if let Err(error) = device
        .delete_size_checked_with_progress(
            &write.target.storage_id,
            &write.target.path,
            size,
            progress,
        )
        .await
    {
        publish_device_state(device, progress).await;
        progress.failed(
            OperationStage::Commit,
            "Incomplete update file removal failed",
        );
        return Err(error.into());
    }
    let after = observe_device_state(device, progress).await?;
    progress.completed_with_path(
        OperationStage::Commit,
        format!(
            "Removed incomplete update file; reclaimed {}; free {} → {}",
            display_bytes(size),
            storage_free_label(&before, &write.target.storage_id),
            storage_free_label(&after, &write.target.storage_id),
        ),
        write.target.path.to_string(),
        size,
        Some(size),
    );
    Ok(())
}

async fn ensure_resume_write_capacity<D>(
    journal: &MountedTransactionJournal,
    write: &JournalWrite,
    state: &ReadyResumeWriteState,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
    removals: &mut [PreparedResumeRemoval],
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    if matches!(state, ReadyResumeWriteState::Installed) {
        return Ok(());
    }
    let before_bytes = match state {
        ReadyResumeWriteState::Installed | ReadyResumeWriteState::Missing => 0,
        ReadyResumeWriteState::SizeMatchedOriginal => {
            write
                .unbacked_original
                .as_ref()
                .ok_or(MountedInstallError::InvalidJournal)?
                .size
        }
    };
    let requirements = crate::space::storage_requirements([crate::space::StorageChange {
        storage_id: &write.target.storage_id,
        before_bytes,
        after_bytes: write.target.size,
    }])?;
    loop {
        require_running(progress)?;
        let snapshot = observe_device_state(device, progress).await?;
        match crate::space::check_device_space(&snapshot, &requirements) {
            Ok(()) => return Ok(()),
            Err(error @ crate::space::SpaceError::Insufficient { .. }) => {
                let Some(index) = journal
                    .unbacked_removals
                    .iter()
                    .zip(removals.iter())
                    .position(|(target, removal)| {
                        target.storage_id == write.target.storage_id
                            && removal.state == ResumeRemovalState::SizeMatched
                            && !removal.checkpoint.is_applied()
                    })
                else {
                    return Err(error.into());
                };
                let target = &journal.unbacked_removals[index];
                let operation =
                    OperationIndex::new(journal.writes.len() + journal.removals.len() + index);
                write_recovery_event(capture_root, operation, CheckpointKind::Started, target)
                    .await?;
                remove_resume_target(
                    device,
                    target,
                    &snapshot,
                    progress,
                    "Removing superseded file to reclaim device space",
                    "Removed superseded file",
                )
                .await?;
                removals[index] = PreparedResumeRemoval {
                    state: ResumeRemovalState::Missing,
                    checkpoint: OperationCheckpoint::Applied,
                };
                write_recovery_event(capture_root, operation, CheckpointKind::Applied, target)
                    .await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

async fn apply_resume_write<D>(
    device: &D,
    write: &JournalWrite,
    payload: &Path,
    state: ReadyResumeWriteState,
    progress: &ProgressReporter,
) -> Result<ResumeWriteOutcome, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    match state {
        ReadyResumeWriteState::Installed => return Ok(ResumeWriteOutcome::Reused),
        ReadyResumeWriteState::Missing => {}
        ReadyResumeWriteState::SizeMatchedOriginal => {
            let original = write
                .unbacked_original
                .as_ref()
                .ok_or(MountedInstallError::InvalidJournal)?;
            let removal = device
                .delete_size_checked_with_progress(
                    &original.storage_id,
                    &original.path,
                    original.size,
                    progress,
                )
                .await;
            refresh_after_mutation(device, progress, removal).await?;
        }
    }
    upload_recovery_payload(device, write, payload, progress).await?;
    Ok(ResumeWriteOutcome::Uploaded)
}

async fn execute_resume_removals<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
    removals: Vec<PreparedResumeRemoval>,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let first_operation = journal.writes.len() + journal.removals.len();
    for (index, (target, removal)) in journal.unbacked_removals.iter().zip(removals).enumerate() {
        require_running(progress)?;
        let operation_index = OperationIndex::new(first_operation + index);
        match removal.checkpoint {
            OperationCheckpoint::NotStarted => {
                write_recovery_event(
                    capture_root,
                    operation_index,
                    CheckpointKind::Started,
                    target,
                )
                .await?;
            }
            OperationCheckpoint::Started => {}
            OperationCheckpoint::Applied if removal.state == ResumeRemovalState::Missing => {
                continue;
            }
            OperationCheckpoint::Applied | OperationCheckpoint::Audited => {
                return Err(MountedInstallError::RecoveryEvidence(target.path.clone()));
            }
        }
        if removal.state == ResumeRemovalState::SizeMatched {
            let before = observe_device_state(device, progress).await?;
            remove_resume_target(
                device,
                target,
                &before,
                progress,
                "Removing obsolete file",
                "Removed obsolete file",
            )
            .await?;
        }
        write_recovery_event(
            capture_root,
            operation_index,
            CheckpointKind::Applied,
            target,
        )
        .await?;
    }
    Ok(())
}

async fn remove_resume_target<D>(
    device: &D,
    target: &BoundObject,
    before: &garmin_device::DeviceStateSnapshot,
    progress: &ProgressReporter,
    started_label: &str,
    completed_label: &str,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let item = progress.for_item(format!("recovery-remove:{}", target.path));
    item.started_with_path(
        OperationStage::Commit,
        started_label,
        target.path.to_string(),
        Some(target.size),
    );
    if let Err(error) = device
        .delete_size_checked_with_progress(&target.storage_id, &target.path, target.size, progress)
        .await
    {
        publish_device_state(device, progress).await;
        item.failed(OperationStage::Commit, "Device file removal failed");
        return Err(error.into());
    }
    let after = observe_device_state(device, progress).await?;
    item.completed_with_path(
        OperationStage::Commit,
        format!(
            "{completed_label}; reclaimed {}; free {} → {}",
            display_bytes(target.size),
            storage_free_label(before, &target.storage_id),
            storage_free_label(&after, &target.storage_id),
        ),
        target.path.to_string(),
        target.size,
        Some(target.size),
    );
    Ok(())
}

fn storage_free_label(state: &garmin_device::DeviceStateSnapshot, storage_id: &str) -> String {
    state
        .storages
        .iter()
        .find(|storage| storage.id == storage_id)
        .and_then(|storage| storage.capacity.bytes())
        .map_or_else(|| "unavailable".to_owned(), |(_, free)| display_bytes(free))
}

fn display_bytes(bytes: u64) -> String {
    format!(
        "{:.2}",
        byte_unit::Byte::from_u64(bytes).get_appropriate_unit(byte_unit::UnitType::Decimal)
    )
}

async fn check_device_object_metadata<D>(
    device: &D,
    target: &BoundObject,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let item = progress.for_item(format!(
        "device-metadata:{}:{}",
        target.storage_id, target.path
    ));
    item.started_operations_with_path(
        OperationStage::DeviceVerify,
        "Checking current device file path and size",
        target.path.to_string(),
        Some(1),
    );
    let status = device
        .inspect_with_progress(&target.storage_id, &target.path, &item)
        .await?;
    if status != (DevicePathStatus::RegularFile { size: target.size }) {
        let (state, size) = status.into_parts();
        item.failed_with_path(
            OperationStage::DeviceVerify,
            "Current device file metadata check failed",
            target.path.to_string(),
        );
        return Err(MountedInstallError::UnexpectedDeviceState {
            path: target.path.clone(),
            state,
            size,
        });
    }
    item.completed_operations_with_path(
        OperationStage::DeviceVerify,
        "Current device file path and size checked",
        target.path.to_string(),
        1,
        Some(1),
    );
    Ok(())
}

async fn upload_recovery_payload<D>(
    device: &D,
    write: &JournalWrite,
    payload: &Path,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    upload_with_device_state_monitor(
        device,
        &write.target.storage_id,
        &write.target.path,
        payload,
        write.target.size,
        &write.sha256,
        MountedMtpUploadProgress {
            reporter: progress.clone(),
            completed_before: 0,
            total: write.target.size,
        },
        progress,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "one exact device upload contract"
)]
async fn upload_with_device_state_monitor<D>(
    device: &D,
    storage_id: &str,
    path: &SafeRelativePath,
    source: &Path,
    size: u64,
    sha256: &str,
    upload_progress: MountedMtpUploadProgress,
    state_progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let upload = device.upload(storage_id, path, source, size, sha256, upload_progress);
    tokio::pin!(upload);
    let result = {
        let monitor_cancellation = CancellationToken::default();
        let monitor_progress = state_progress
            .clone()
            .with_cancellation(monitor_cancellation.clone());
        let monitor = monitor_device_state(device, &monitor_progress);
        tokio::pin!(monitor);
        let result = tokio::select! {
            result = &mut upload => result,
            () = &mut monitor => unreachable!("device-state monitor only stops when cancelled"),
        };
        monitor_cancellation.cancel();
        result
    };
    match result {
        Ok(()) => {
            observe_device_state(device, state_progress).await?;
            Ok(())
        }
        Err(error) => {
            publish_device_state(device, state_progress).await;
            Err(error.into())
        }
    }
}

async fn monitor_device_state<D>(device: &D, progress: &ProgressReporter)
where
    D: DeviceWrite + ?Sized,
{
    let mut refresh = tokio::time::interval(Duration::from_secs(2));
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    refresh.tick().await;
    loop {
        refresh.tick().await;
        if progress.is_cancelled() {
            return;
        }
        refresh_device_state(device, progress).await;
        if progress.is_cancelled() {
            return;
        }
    }
}

async fn inspect_resume_write<D>(
    device: &D,
    write: &JournalWrite,
    capture_root: &Path,
    operation: usize,
    checkpoint: OperationCheckpoint,
    progress: &ProgressReporter,
) -> Result<ResumeWriteInspection, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    progress.started_operations_with_path(
        OperationStage::DeviceVerify,
        "Inspecting current device path",
        write.target.path.to_string(),
        Some(1),
    );
    let inspection = device
        .inspect_with_progress(&write.target.storage_id, &write.target.path, progress)
        .await;
    let (state, size) = match inspection {
        Ok(inspection) => inspection,
        Err(error) => {
            progress.failed_with_path(
                OperationStage::DeviceVerify,
                "Current device path inspection failed",
                write.target.path.to_string(),
            );
            return Err(error.into());
        }
    }
    .into_parts();
    match (state, size) {
        (DevicePathState::Missing, None) => {
            complete_resume_path_inspection(progress, write, "Current device path is absent");
            Ok(ResumeWriteInspection::observed(ResumeWriteState::Missing))
        }
        (DevicePathState::RegularFile, Some(size)) => {
            inspect_regular_resume_write(
                device,
                write,
                capture_root,
                operation,
                checkpoint,
                progress,
                size,
            )
            .await
        }
        (state, size) => Err(MountedInstallError::UnexpectedDeviceState {
            path: write.target.path.clone(),
            state,
            size,
        }),
    }
}

async fn inspect_regular_resume_write<D>(
    device: &D,
    write: &JournalWrite,
    capture_root: &Path,
    operation: usize,
    checkpoint: OperationCheckpoint,
    progress: &ProgressReporter,
    size: u64,
) -> Result<ResumeWriteInspection, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    if size == write.target.size {
        if checkpoint == OperationCheckpoint::NotStarted {
            if write
                .unbacked_original
                .as_ref()
                .is_some_and(|original| original.size == size)
            {
                complete_resume_path_inspection(
                    progress,
                    write,
                    "Existing path and size match the unprotected original",
                );
                return Ok(ResumeWriteInspection::observed(
                    ResumeWriteState::SizeMatchedOriginal,
                ));
            }
            progress.failed_with_path(
                OperationStage::DeviceVerify,
                "Unexpected unjournaled device file",
                write.target.path.to_string(),
            );
            return Err(MountedInstallError::RecoveryEvidence(
                write.target.path.clone(),
            ));
        }
        if checkpoint == OperationCheckpoint::Started
            && write
                .unbacked_original
                .as_ref()
                .is_some_and(|original| original.size == size)
        {
            complete_resume_path_inspection(
                progress,
                write,
                "Existing path and size match the unprotected original",
            );
            return Ok(ResumeWriteInspection::observed(
                ResumeWriteState::SizeMatchedOriginal,
            ));
        }
        complete_resume_path_inspection(
            progress,
            write,
            "Existing device path and size match the journal",
        );
        return Ok(ResumeWriteInspection::observed(ResumeWriteState::Installed));
    }
    if write
        .unbacked_original
        .as_ref()
        .is_some_and(|original| original.size == size)
    {
        complete_resume_path_inspection(
            progress,
            write,
            "Existing path and size match the unprotected original",
        );
        return Ok(ResumeWriteInspection::observed(
            ResumeWriteState::SizeMatchedOriginal,
        ));
    }
    complete_resume_path_inspection(
        progress,
        write,
        "Current device file requires partial-upload inspection",
    );
    verify_partial_write(device, write, capture_root, operation, size, progress).await?;
    Ok(ResumeWriteInspection::observed(ResumeWriteState::Partial {
        size,
    }))
}

fn complete_resume_path_inspection(progress: &ProgressReporter, write: &JournalWrite, label: &str) {
    progress.completed_operations_with_path(
        OperationStage::DeviceVerify,
        label,
        write.target.path.to_string(),
        1,
        Some(1),
    );
}

async fn inspect_resume_removal<D>(
    device: &D,
    target: &BoundObject,
    progress: &ProgressReporter,
) -> Result<ResumeRemovalState, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let (state, size) = device
        .inspect_with_progress(&target.storage_id, &target.path, progress)
        .await?
        .into_parts();
    match (state, size) {
        (DevicePathState::Missing, None) => Ok(ResumeRemovalState::Missing),
        (DevicePathState::RegularFile, Some(size)) if size == target.size => {
            Ok(ResumeRemovalState::SizeMatched)
        }
        (state, size) => Err(MountedInstallError::UnexpectedDeviceState {
            path: target.path.clone(),
            state,
            size,
        }),
    }
}

async fn load_resume_checkpoints(
    journal: &MountedTransactionJournal,
    capture_root: &Path,
) -> Result<Vec<OperationCheckpoint>, MountedInstallError> {
    load_operation_checkpoints(journal, capture_root, false).await
}

async fn load_rollback_checkpoints(
    journal: &MountedTransactionJournal,
    capture_root: &Path,
) -> Result<Vec<OperationCheckpoint>, MountedInstallError> {
    load_operation_checkpoints(journal, capture_root, true).await
}

async fn load_operation_checkpoints(
    journal: &MountedTransactionJournal,
    capture_root: &Path,
    allow_rollback_markers: bool,
) -> Result<Vec<OperationCheckpoint>, MountedInstallError> {
    if !allow_rollback_markers {
        for marker in ["rollback-started.json", "rolled-back.json"] {
            if tokio::fs::try_exists(
                capture_root.join(format!("mounted-update/transaction/{marker}")),
            )
            .await?
            {
                return Err(MountedInstallError::InvalidJournal);
            }
        }
    }
    let total = transaction_operation_count(journal);
    let mut checkpoints = Vec::with_capacity(total);
    for index in 0..total {
        let operation = OperationIndex::new(index);
        let started_value = transaction_started_value(journal, operation)?;
        let started = validate_optional_event(
            capture_root,
            operation,
            CheckpointKind::Started,
            &started_value,
        )
        .await?;
        let applied = validate_optional_event(
            capture_root,
            operation,
            CheckpointKind::Applied,
            &started_value,
        )
        .await?;
        let audited = if index < journal.writes.len() {
            validate_optional_event(
                capture_root,
                operation,
                CheckpointKind::Audited,
                &serde_json::to_value(&journal.writes[index])?,
            )
            .await?
        } else {
            if validate_optional_event(
                capture_root,
                operation,
                CheckpointKind::Audited,
                &serde_json::Value::Null,
            )
            .await?
            {
                return Err(MountedInstallError::InvalidJournal);
            }
            false
        };
        checkpoints.push(
            OperationCheckpoint::from_presence(started, applied, audited)
                .ok_or(MountedInstallError::InvalidJournal)?,
        );
    }
    let unexpected = OperationIndex::new(total);
    for kind in [
        CheckpointKind::Started,
        CheckpointKind::Applied,
        CheckpointKind::Audited,
    ] {
        if validate_optional_event(capture_root, unexpected, kind, &serde_json::Value::Null).await?
        {
            return Err(MountedInstallError::InvalidJournal);
        }
    }
    Ok(checkpoints)
}

fn resume_inspection_order(
    journal: &MountedTransactionJournal,
    checkpoints: &[OperationCheckpoint],
) -> Result<Vec<usize>, MountedInstallError> {
    if checkpoints.len() != transaction_operation_count(journal) {
        return Err(MountedInstallError::InvalidJournal);
    }
    let mut writes = Vec::with_capacity(journal.writes.len());
    for (index, checkpoint) in checkpoints
        .iter()
        .copied()
        .enumerate()
        .take(journal.writes.len())
    {
        writes.push((checkpoint.inspection_priority(), index));
    }
    writes.sort_unstable();
    Ok(writes.into_iter().map(|(_, index)| index).collect())
}

fn transaction_started_value(
    journal: &MountedTransactionJournal,
    operation: OperationIndex,
) -> Result<serde_json::Value, MountedInstallError> {
    let index = operation.get();
    if let Some(write) = journal.writes.get(index) {
        return Ok(serde_json::to_value(write)?);
    }
    let index = index - journal.writes.len();
    if let Some(removal) = journal.removals.get(index) {
        return Ok(serde_json::to_value(removal)?);
    }
    let index = index - journal.removals.len();
    journal
        .unbacked_removals
        .get(index)
        .map(serde_json::to_value)
        .transpose()?
        .ok_or(MountedInstallError::InvalidJournal)
}

async fn validate_optional_event(
    capture_root: &Path,
    operation: OperationIndex,
    kind: CheckpointKind,
    expected: &serde_json::Value,
) -> Result<bool, MountedInstallError> {
    let relative = checkpoint_path(operation, kind);
    let path = capture_root.join(&relative);
    if !tokio::fs::try_exists(&path).await? {
        return Ok(false);
    }
    let path = recovery_file(capture_root, &relative).await?;
    if tokio::fs::metadata(&path).await?.len() > 4 * 1024 * 1024 {
        return Err(MountedInstallError::UnsafeJournal(path));
    }
    let observed = serde_json::from_slice::<serde_json::Value>(&tokio::fs::read(path).await?)?;
    if observed != *expected {
        return Err(MountedInstallError::InvalidJournal);
    }
    Ok(true)
}

async fn write_recovery_event<T>(
    capture_root: &Path,
    operation: OperationIndex,
    kind: CheckpointKind,
    value: &T,
) -> Result<(), MountedInstallError>
where
    T: Serialize,
{
    let relative = checkpoint_path(operation, kind);
    let expected = serde_json::to_value(value)?;
    if validate_optional_event(capture_root, operation, kind, &expected).await? {
        return Ok(());
    }
    let target = capture_root.join(&relative);
    let parent = target
        .parent()
        .ok_or_else(|| MountedInstallError::UnsafeJournal(target.clone()))?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let (file, temporary) = temporary.into_parts();
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let mut file = tokio::fs::File::from_std(file);
    file.write_all(&bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    match temporary.persist_noclobber(&target) {
        Ok(()) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if validate_optional_event(capture_root, operation, kind, &expected).await? {
                Ok(())
            } else {
                Err(MountedInstallError::InvalidJournal)
            }
        }
        Err(error) => Err(error.error.into()),
    }
}

fn checkpoint_path(operation: OperationIndex, kind: CheckpointKind) -> String {
    format!(
        "mounted-update/transaction/{:06}-{}.json",
        operation.ordinal(),
        kind.as_str(),
    )
}

fn transaction_operation_count(journal: &MountedTransactionJournal) -> usize {
    journal.writes.len() + journal.removals.len() + journal.unbacked_removals.len()
}

fn transaction_operation_target(
    journal: &MountedTransactionJournal,
    operation: OperationIndex,
) -> Option<String> {
    let operation = operation.get();
    if operation < journal.writes.len() {
        return Some(journal.writes[operation].target.path.to_string());
    }
    let operation = operation - journal.writes.len();
    if operation < journal.removals.len() {
        return Some(journal.removals[operation].target.path.to_string());
    }
    let operation = operation - journal.removals.len();
    journal
        .unbacked_removals
        .get(operation)
        .map(|target| target.path.to_string())
}

async fn unprotected_failure(
    journal: &MountedTransactionJournal,
    capture_root: &Path,
    operation: MountedInstallError,
) -> MountedInstallError {
    let total_operations = transaction_operation_count(journal);
    let evidence = match load_resume_checkpoints(journal, capture_root).await {
        Ok(checkpoints) => {
            let applied_operations = checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.is_applied())
                .count();
            let started_index = checkpoints
                .iter()
                .position(|checkpoint| *checkpoint == OperationCheckpoint::Started);
            if applied_operations == 0 && started_index.is_none() {
                return operation;
            }
            let failed_target = started_index
                .and_then(|index| transaction_operation_target(journal, OperationIndex(index)));
            if applied_operations > 0 {
                UnprotectedMutationEvidence::Applied {
                    applied_operations,
                    total_operations,
                    failed_target,
                }
            } else {
                UnprotectedMutationEvidence::IntentRecorded {
                    total_operations,
                    failed_target,
                }
            }
        }
        Err(error) => UnprotectedMutationEvidence::Unavailable {
            total_operations,
            reason: error.to_string(),
        },
    };
    MountedInstallError::UnprotectedMutation {
        operation: Box::new(operation),
        evidence,
    }
}

fn require_running(progress: &ProgressReporter) -> Result<(), MountedInstallError> {
    if progress.is_cancelled() {
        return Err(MountedInstallError::Cancelled);
    }
    Ok(())
}

async fn write_transaction_event<T>(
    capture: &SessionCapture,
    operation: OperationIndex,
    kind: CheckpointKind,
    value: &T,
) -> Result<(), MountedInstallError>
where
    T: Serialize,
{
    capture
        .write_json_atomic(Path::new(&checkpoint_path(operation, kind)), value)
        .await?;
    Ok(())
}

async fn rollback_update<D>(
    journal: &MountedTransactionJournal,
    device: &D,
    capture_root: &Path,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    if journal.backup_policy == BackupPolicy::Skip {
        return Err(MountedInstallError::RecoveryUnavailable);
    }
    progress.started(
        OperationStage::Cleanup,
        "Rolling back the mounted MTP update",
        Some(
            u64::try_from(journal.writes.len() + journal.removals.len())
                .map_err(|_| MountedInstallError::ByteCountOverflow)?,
        ),
    );
    for original in journal
        .writes
        .iter()
        .filter_map(|write| write.original.as_ref())
        .chain(&journal.removals)
    {
        let backup = recovery_file(capture_root, &original.backup_file).await?;
        if verify_local_payload(&backup, original.target.size, None).await? != original.sha256 {
            return Err(MountedInstallError::RecoveryEvidence(
                original.target.path.clone(),
            ));
        }
    }
    let checkpoints = load_rollback_checkpoints(journal, capture_root).await?;
    ensure_rollback_marker(capture_root, journal).await?;
    let mut writes = Vec::new();
    for (index, write) in journal.writes.iter().enumerate() {
        writes.push(
            inspect_recovery_write(
                device,
                write,
                capture_root,
                index + 1,
                checkpoints[index],
                progress,
            )
            .await?,
        );
    }
    let mut removals = Vec::with_capacity(journal.removals.len());
    for (index, original) in journal.removals.iter().enumerate() {
        removals.push(
            inspect_recovery_original(
                device,
                original,
                capture_root,
                checkpoints[journal.writes.len() + index],
                progress,
            )
            .await?,
        );
    }
    check_recovery_space(journal, &writes, &removals, device, progress).await?;
    for (write, state) in journal.writes.iter().zip(writes).rev() {
        if let RecoveryObjectState::Remove { size } = state {
            device
                .delete_size_checked_with_progress(
                    &write.target.storage_id,
                    &write.target.path,
                    size,
                    progress,
                )
                .await?;
        }
        if let Some(original) = &write.original {
            restore_original(device, capture_root, original, progress).await?;
        }
    }
    for (original, state) in journal.removals.iter().zip(removals).rev() {
        if let RecoveryObjectState::Remove { size } = state {
            device
                .delete_size_checked_with_progress(
                    &original.target.storage_id,
                    &original.target.path,
                    size,
                    progress,
                )
                .await?;
        }
        restore_original(device, capture_root, original, progress).await?;
    }
    let mut rolled_back = journal.clone();
    rolled_back.state = JournalState::RolledBack;
    write_recovery_marker(
        &capture_root.join("mounted-update/transaction/rolled-back.json"),
        &rolled_back,
    )
    .await?;
    progress.completed(
        OperationStage::Cleanup,
        "Mounted MTP update rolled back",
        1,
        Some(1),
    );
    Ok(())
}

async fn inspect_recovery_write<D>(
    device: &D,
    write: &JournalWrite,
    capture_root: &Path,
    operation: usize,
    checkpoint: OperationCheckpoint,
    progress: &ProgressReporter,
) -> Result<RecoveryObjectState, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let (state, size) = device
        .inspect_with_progress(&write.target.storage_id, &write.target.path, progress)
        .await?
        .into_parts();
    match (state, size) {
        (DevicePathState::Missing, None) => Ok(RecoveryObjectState::Missing),
        (DevicePathState::RegularFile, Some(size)) => {
            if checkpoint == OperationCheckpoint::NotStarted {
                return if write
                    .original
                    .as_ref()
                    .is_some_and(|original| original.target.size == size)
                {
                    Ok(RecoveryObjectState::Original)
                } else {
                    Err(MountedInstallError::RecoveryEvidence(
                        write.target.path.clone(),
                    ))
                };
            }
            if size == write.target.size
                || write
                    .original
                    .as_ref()
                    .is_some_and(|original| original.target.size == size)
            {
                return Ok(RecoveryObjectState::Remove { size });
            }
            verify_partial_write(device, write, capture_root, operation, size, progress).await?;
            Ok(RecoveryObjectState::Remove { size })
        }
        (state, size) => Err(MountedInstallError::UnexpectedDeviceState {
            path: write.target.path.clone(),
            state,
            size,
        }),
    }
}

async fn inspect_recovery_original<D>(
    device: &D,
    original: &OriginalObject,
    capture_root: &Path,
    checkpoint: OperationCheckpoint,
    progress: &ProgressReporter,
) -> Result<RecoveryObjectState, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let (state, size) = device
        .inspect_with_progress(&original.target.storage_id, &original.target.path, progress)
        .await?
        .into_parts();
    match (state, size) {
        (DevicePathState::Missing, None) => Ok(RecoveryObjectState::Missing),
        (DevicePathState::RegularFile, Some(size))
            if checkpoint == OperationCheckpoint::NotStarted && size == original.target.size =>
        {
            Ok(RecoveryObjectState::Original)
        }
        (DevicePathState::RegularFile, Some(_))
            if checkpoint == OperationCheckpoint::NotStarted =>
        {
            Err(MountedInstallError::RecoveryEvidence(
                original.target.path.clone(),
            ))
        }
        (DevicePathState::RegularFile, Some(size)) if size == original.target.size => {
            Ok(RecoveryObjectState::Remove { size })
        }
        (DevicePathState::RegularFile, Some(size)) => {
            let backup = recovery_file(capture_root, &original.backup_file).await?;
            verify_partial_object(
                device,
                &original.target,
                size,
                [(
                    backup.as_path(),
                    original.target.size,
                    original.sha256.as_str(),
                )],
                capture_root,
                progress,
            )
            .await?;
            Ok(RecoveryObjectState::Remove { size })
        }
        (state, size) => Err(MountedInstallError::UnexpectedDeviceState {
            path: original.target.path.clone(),
            state,
            size,
        }),
    }
}

async fn check_recovery_space<D>(
    journal: &MountedTransactionJournal,
    writes: &[RecoveryObjectState],
    removals: &[RecoveryObjectState],
    device: &D,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let mut changes = Vec::new();
    for (write, state) in journal.writes.iter().zip(writes).rev() {
        match state {
            RecoveryObjectState::Missing => {
                if let Some(original) = &write.original {
                    changes.push(crate::space::StorageChange {
                        storage_id: &original.target.storage_id,
                        before_bytes: 0,
                        after_bytes: original.target.size,
                    });
                }
            }
            RecoveryObjectState::Remove { size } => {
                changes.push(crate::space::StorageChange {
                    storage_id: &write.target.storage_id,
                    before_bytes: *size,
                    after_bytes: write
                        .original
                        .as_ref()
                        .map_or(0, |original| original.target.size),
                });
            }
            RecoveryObjectState::Original => {}
        }
    }
    for (original, state) in journal.removals.iter().zip(removals).rev() {
        match state {
            RecoveryObjectState::Missing => changes.push(crate::space::StorageChange {
                storage_id: &original.target.storage_id,
                before_bytes: 0,
                after_bytes: original.target.size,
            }),
            RecoveryObjectState::Remove { size } => {
                changes.push(crate::space::StorageChange {
                    storage_id: &original.target.storage_id,
                    before_bytes: *size,
                    after_bytes: original.target.size,
                });
            }
            RecoveryObjectState::Original => {}
        }
    }
    let requirements = crate::space::storage_requirements(changes)?;
    if requirements.is_empty() {
        return Ok(());
    }
    let state = observe_device_state(device, progress).await?;
    crate::space::check_device_space(&state, &requirements)?;
    Ok(())
}

async fn observe_device_state<D>(
    device: &D,
    progress: &ProgressReporter,
) -> Result<garmin_device::DeviceStateSnapshot, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    match device.state_with_progress(progress).await {
        Ok(state) => {
            progress.device_state(Ok(state.clone()));
            Ok(state)
        }
        Err(error) => {
            record_device_state_error(progress, &error.to_string());
            Err(error.into())
        }
    }
}

async fn publish_device_state<D>(device: &D, progress: &ProgressReporter)
where
    D: DeviceWrite + ?Sized,
{
    if let Ok(state) = observe_device_state(device, progress).await {
        retain_device_state(progress, &state);
    }
}

async fn refresh_device_state<D>(device: &D, progress: &ProgressReporter)
where
    D: DeviceWrite + ?Sized,
{
    let _ = observe_device_state(device, progress).await;
}

async fn refresh_after_mutation<D, E>(
    device: &D,
    progress: &ProgressReporter,
    result: Result<(), E>,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
    E: Into<MountedInstallError>,
{
    match result {
        Ok(()) => {
            refresh_device_state(device, progress).await;
            Ok(())
        }
        Err(error) => {
            publish_device_state(device, progress).await;
            Err(error.into())
        }
    }
}

fn retain_device_state(progress: &ProgressReporter, state: &garmin_device::DeviceStateSnapshot) {
    for storage in &state.storages {
        let label = storage.capacity.bytes().map_or_else(
            || format!("Storage snapshot — {}: capacity unavailable", storage.label),
            |(total, free)| {
                format!(
                    "Storage snapshot — {}: {} free of {}",
                    storage.label,
                    display_bytes(free),
                    display_bytes(total),
                )
            },
        );
        progress
            .for_item(format!("storage-snapshot:{}", storage.id))
            .with_event_kind(garmin_progress::ProgressEventKind::StorageSnapshot)
            .completed_operations(OperationStage::Inspect, label, 1, Some(1));
    }
}

fn record_device_state_error(progress: &ProgressReporter, error: &str) {
    if progress.changed_device_state(Err(error.to_owned())) {
        progress.for_item("storage-snapshot-error").failed(
            OperationStage::Inspect,
            format!("Storage refresh failed — {error}"),
        );
    }
}

async fn recovery_file(root: &Path, relative: &str) -> Result<PathBuf, MountedInstallError> {
    let safe = SafeRelativePath::parse(relative)?;
    let mut path = root.to_owned();
    if !tokio::fs::symlink_metadata(&path).await?.is_dir() {
        return Err(MountedInstallError::UnsafeJournal(path));
    }
    for part in safe.as_path().components() {
        path.push(part);
        let meta = tokio::fs::symlink_metadata(&path).await?;
        if meta.file_type().is_symlink() {
            return Err(MountedInstallError::UnsafeJournal(path));
        }
    }
    if !tokio::fs::symlink_metadata(&path).await?.is_file() {
        return Err(MountedInstallError::UnsafeJournal(path));
    }
    Ok(path)
}

async fn verify_partial_write<D: DeviceWrite + ?Sized>(
    device: &D,
    write: &JournalWrite,
    root: &Path,
    operation: usize,
    size: u64,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError> {
    let blocked = || MountedInstallError::RecoveryEvidence(write.target.path.clone());
    let intent_path = recovery_file(
        root,
        &format!("mounted-update/transaction/{operation:06}-started.json"),
    )
    .await?;
    if tokio::fs::metadata(&intent_path).await?.len() > 1024 * 1024 {
        return Err(blocked());
    }
    let intent: JournalWrite = serde_json::from_slice(&tokio::fs::read(intent_path).await?)?;
    if intent != *write {
        return Err(blocked());
    }
    let payload = recovery_file(root, write.payload_file.as_deref().ok_or_else(blocked)?).await?;
    let mut sources = vec![(payload.as_path(), write.target.size, write.sha256.as_str())];
    let backup;
    if let Some(original) = &write.original {
        backup = recovery_file(root, &original.backup_file).await?;
        sources.push((
            backup.as_path(),
            original.target.size,
            original.sha256.as_str(),
        ));
    }
    verify_partial_object(device, &write.target, size, sources, root, progress).await
}

async fn verify_partial_object<'a, D>(
    device: &D,
    target: &BoundObject,
    size: u64,
    sources: impl IntoIterator<Item = (&'a Path, u64, &'a str)>,
    root: &Path,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let blocked = || MountedInstallError::RecoveryEvidence(target.path.clone());
    let mut expected = Vec::new();
    for (source, source_size, source_sha256) in sources {
        if size == 0 || size >= source_size {
            continue;
        }
        if verify_local_payload_with_progress(source, source_size, None, |_| {
            require_running(progress)
        })
        .await?
            != source_sha256
        {
            return Err(blocked());
        }
        let mut source = tokio::fs::File::open(source).await?.take(size);
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        loop {
            require_running(progress)?;
            let count = source.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        expected.push(hex::encode(hash.finalize()));
    }
    if expected.is_empty() {
        return Err(blocked());
    }
    crate::space::check_host_space(root, size).await?;
    let temporary = tempfile::NamedTempFile::new_in(root)?;
    let temporary_path = temporary.path().to_owned();
    let (temporary, temporary_guard) = temporary.into_parts();
    let actual = device
        .backup(
            &target.storage_id,
            &target.path,
            size,
            garmin_device::BackupDestination::new(temporary_path, temporary)?,
            MountedMtpBackupProgress {
                reporter: progress.clone(),
                completed_before: 0,
                total: size,
            },
        )
        .await?;
    drop(temporary_guard);
    if !expected.contains(&actual) {
        return Err(blocked());
    }
    Ok(())
}

async fn ensure_rollback_marker(
    root: &Path,
    journal: &MountedTransactionJournal,
) -> Result<(), MountedInstallError> {
    let target = root.join("mounted-update/transaction/rollback-started.json");
    if tokio::fs::try_exists(&target).await? {
        let observed = read_journal(
            &recovery_file(root, "mounted-update/transaction/rollback-started.json").await?,
        )
        .await?;
        return validate_matching_journal(journal, &observed, JournalState::Prepared);
    }
    write_recovery_marker(&target, journal).await
}

async fn restore_original<D>(
    device: &D,
    capture_root: &Path,
    original: &OriginalObject,
    progress: &ProgressReporter,
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let (state, size) = device
        .inspect_with_progress(&original.target.storage_id, &original.target.path, progress)
        .await?
        .into_parts();
    match (state, size) {
        (DevicePathState::Missing, None) => {
            device
                .restore_with_progress(
                    &original.target.storage_id,
                    &original.target.path,
                    original.target.size,
                    &recovery_file(capture_root, &original.backup_file).await?,
                    &original.sha256,
                    progress,
                )
                .await?;
            Ok(())
        }
        (DevicePathState::RegularFile, Some(size)) if size == original.target.size => Ok(()),
        (state, size) => Err(MountedInstallError::UnexpectedDeviceState {
            path: original.target.path.clone(),
            state,
            size,
        }),
    }
}

async fn write_recovery_marker(
    target: &Path,
    journal: &MountedTransactionJournal,
) -> Result<(), MountedInstallError> {
    if tokio::fs::try_exists(target).await? {
        return if read_journal(target).await? == *journal {
            Ok(())
        } else {
            Err(MountedInstallError::InvalidJournal)
        };
    }
    let parent = target
        .parent()
        .ok_or_else(|| MountedInstallError::UnsafeJournal(target.to_owned()))?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let (file, temporary) = temporary.into_parts();
    let mut bytes = serde_json::to_vec_pretty(journal)?;
    bytes.push(b'\n');
    let mut file = tokio::fs::File::from_std(file);
    file.write_all(&bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    match temporary.persist_noclobber(target) {
        Ok(()) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_journal(target).await? == *journal {
                Ok(())
            } else {
                Err(MountedInstallError::InvalidJournal)
            }
        }
        Err(error) => Err(error.error.into()),
    }
}

async fn verify_local_payload(
    path: &Path,
    expected_size: u64,
    expected_md5: Option<&str>,
) -> Result<String, MountedInstallError> {
    verify_local_payload_with_progress(path, expected_size, expected_md5, |_| Ok(())).await
}

async fn verify_local_payload_with_progress(
    path: &Path,
    expected_size: u64,
    expected_md5: Option<&str>,
    mut advanced: impl FnMut(u64) -> Result<(), MountedInstallError>,
) -> Result<String, MountedInstallError> {
    let mut file = tokio::fs::File::open(path).await?;
    let metadata = file.metadata().await?;
    if !metadata.is_file() {
        return Err(MountedInstallError::UnsafeStagedFile(path.to_owned()));
    }
    if metadata.len() != expected_size {
        return Err(MountedInstallError::StagedSize {
            path: path.to_owned(),
            expected: expected_size,
            actual: metadata.len(),
        });
    }
    let mut md5 = Md5::new();
    let mut sha256 = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(u64::try_from(count).map_err(|_| MountedInstallError::ByteCountOverflow)?)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        md5.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
        advanced(size)?;
    }
    if size != expected_size {
        return Err(MountedInstallError::StagedSize {
            path: path.to_owned(),
            expected: expected_size,
            actual: size,
        });
    }
    if let Some(expected) = expected_md5 {
        let actual = hex::encode(md5.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(MountedInstallError::StagedChecksum {
                expected: expected.to_owned(),
                actual,
            });
        }
    }
    Ok(hex::encode(sha256.finalize()))
}

fn object_key(target: &BoundObject) -> String {
    format!("{}:{}", target.storage_id, path_key(&target.path))
}

fn path_key(path: &SafeRelativePath) -> String {
    path.to_string().replace('\\', "/").to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnprotectedMutationEvidence {
    IntentRecorded {
        total_operations: usize,
        failed_target: Option<String>,
    },
    Applied {
        applied_operations: usize,
        total_operations: usize,
        failed_target: Option<String>,
    },
    Unavailable {
        total_operations: usize,
        reason: String,
    },
}

impl std::fmt::Display for UnprotectedMutationEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntentRecorded { failed_target, .. } => {
                formatter.write_str(
                    "device mutation may have started; no applied operation was recorded",
                )?;
                if let Some(target) = failed_target {
                    write!(formatter, "; failure at {target}")?;
                }
                Ok(())
            }
            Self::Applied {
                applied_operations,
                total_operations,
                failed_target,
            } => {
                write!(
                    formatter,
                    "{applied_operations} of {total_operations} device operations applied"
                )?;
                if let Some(target) = failed_target {
                    write!(formatter, "; failure at {target}")?;
                }
                Ok(())
            }
            Self::Unavailable {
                total_operations,
                reason,
            } => write!(
                formatter,
                "applied count unknown across {total_operations} device operations; checkpoint evidence unavailable: {reason}"
            ),
        }
    }
}

#[derive(Debug, Error)]
pub enum MountedInstallError {
    #[error(transparent)]
    DeviceState(#[from] crate::DeviceStateError),
    #[error(
        "recovery evidence does not match {0}; device files and recovery backups were preserved"
    )]
    RecoveryEvidence(SafeRelativePath),
    #[error(transparent)]
    Space(#[from] crate::space::SpaceError),
    #[error("mounted MTP update cancelled")]
    Cancelled,
    #[error("staged file count mismatch: expected {expected}, received {actual}")]
    IncompleteStage { expected: usize, actual: usize },
    #[error("staged update file is not a regular file: {0}")]
    UnsafeStagedFile(PathBuf),
    #[error("staged file size mismatch for {path}: expected {expected}, received {actual}")]
    StagedSize {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error("staged file checksum mismatch: expected {expected}, received {actual}")]
    StagedChecksum { expected: String, actual: String },
    #[error("multiple update files target {0}")]
    DuplicateDestination(SafeRelativePath),
    #[error("embedded unlock patching is not supported")]
    EmbeddedUnlocksUnsupported,
    #[error("signed storage data is not supported")]
    SignedStorageDataUnsupported,
    #[error("device inventory used unexpected transport {0:?}")]
    WrongTransport(TransportKind),
    #[error("device inventory did not inspect {0}")]
    IncompleteInventory(SafeRelativePath),
    #[error("{path} resolved to {state:?} on {storage}")]
    UnsafeDeviceObject {
        path: SafeRelativePath,
        storage: String,
        state: DevicePathState,
    },
    #[error("device did not report a size for {0}")]
    MissingDeviceSize(SafeRelativePath),
    #[error("{0} exists in more than one device storage")]
    AmbiguousStorage(SafeRelativePath),
    #[error("the canonical device storage did not inventory new object {0}")]
    PrimaryStorageMissing(SafeRelativePath),
    #[error("unexpected device state for {path}: {state:?}, size {size:?}")]
    UnexpectedDeviceState {
        path: SafeRelativePath,
        state: DevicePathState,
        size: Option<u64>,
    },
    #[error("mounted MTP byte count exceeds the supported range")]
    ByteCountOverflow,
    #[error("operation failed ({operation}); rollback also failed ({rollback})")]
    Rollback {
        operation: Box<MountedInstallError>,
        rollback: Box<MountedInstallError>,
    },
    #[error(
        "update failed after backup-free device mutation became possible ({evidence}): {operation}; automatic rollback is unavailable and reconciliation is required"
    )]
    UnprotectedMutation {
        operation: Box<MountedInstallError>,
        evidence: UnprotectedMutationEvidence,
    },
    #[error(
        "this update explicitly skipped recovery backups; automatic rollback is unavailable and reinstall may be required"
    )]
    RecoveryUnavailable,
    #[error("mounted update journal is not a safe regular file: {0}")]
    UnsafeJournal(PathBuf),
    #[error("mounted update journal version {0} is unsupported")]
    JournalVersion(u8),
    #[error("mounted update journal is internally inconsistent")]
    InvalidJournal,
    #[error("the recovery journal belongs to a different device")]
    RecoveryDeviceMismatch,
    #[error("the host recovery journal does not match the device's active transaction")]
    RecoveryPlanMismatch,
    #[error("mounted MTP device operation failed: {0}")]
    Device(DeviceIoError),
    #[error("local update payload I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid device path: {0}")]
    Path(#[from] PathSafetyError),
    #[error("invalid unlock payload: {0}")]
    UnlockPayload(#[from] base64::DecodeError),
    #[error("session capture failed: {0}")]
    Capture(#[from] garmin_capture::CaptureError),
    #[error("mounted update journal is invalid JSON: {0}")]
    JournalJson(#[from] serde_json::Error),
}

impl From<DeviceIoError> for MountedInstallError {
    fn from(error: DeviceIoError) -> Self {
        match error {
            DeviceIoError::Cancelled => Self::Cancelled,
            error => Self::Device(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_device::{DevicePathInspection, DeviceStorageState, StorageCapacity};

    fn inspection(
        storage_id: &str,
        path: &SafeRelativePath,
        state: DevicePathState,
        size: Option<u64>,
    ) -> DevicePathInspection {
        DevicePathInspection {
            storage_id: storage_id.to_owned(),
            storage_label: format!("Storage {storage_id}"),
            path: path.clone(),
            state,
            size,
        }
    }

    #[test]
    fn retained_capacity_history_uses_a_typed_snapshot_event() {
        let (progress, events) = ProgressReporter::channel();
        retain_device_state(
            &progress,
            &garmin_device::DeviceStateSnapshot {
                storages: vec![DeviceStorageState {
                    id: "internal".to_owned(),
                    label: "Internal storage".to_owned(),
                    capacity: StorageCapacity::new(1_000, 250),
                    writable: Some(true),
                }],
            },
        );

        let event = events.try_iter().next().expect("capacity history event");
        assert_eq!(
            event.kind,
            garmin_progress::ProgressEventKind::StorageSnapshot
        );
        assert_eq!(event.state, garmin_progress::ProgressState::Completed);
    }

    #[tokio::test]
    async fn backup_free_failure_before_mutation_keeps_its_original_error_type() {
        let capture = tempfile::tempdir().unwrap();
        let journal = MountedTransactionJournal {
            version: TRANSACTION_VERSION,
            execution_target: None,
            device_digest: "device".to_owned(),
            plan_digest: "plan".to_owned(),
            backup_policy: BackupPolicy::Skip,
            state: JournalState::Prepared,
            writes: vec![JournalWrite {
                target: BoundObject {
                    storage_id: "internal".to_owned(),
                    storage_label: "Internal storage".to_owned(),
                    path: SafeRelativePath::parse("Garmin/map.img").unwrap(),
                    size: 1,
                },
                sha256: "digest".to_owned(),
                original: None,
                unbacked_original: None,
                payload_file: Some("mounted-update/payloads/000001.bin".to_owned()),
            }],
            removals: Vec::new(),
            unbacked_removals: Vec::new(),
        };

        let error =
            unprotected_failure(&journal, capture.path(), MountedInstallError::Cancelled).await;

        assert!(matches!(error, MountedInstallError::Cancelled));
    }

    #[test]
    fn new_objects_bind_to_the_manifest_storage() {
        let path = SafeRelativePath::parse("Garmin/new.img").unwrap();
        let inventory = DeviceInventory {
            transport: TransportKind::MountedMtp,
            paths: vec![
                inspection("internal", &path, DevicePathState::Missing, None),
                inspection("sd", &path, DevicePathState::Missing, None),
            ],
        };

        let target = bind_write_target(&inventory, "internal", path, 42).unwrap();

        assert_eq!(target.storage_id, "internal");
        assert_eq!(target.size, 42);
    }

    #[test]
    fn replacements_remain_on_the_storage_that_owns_the_object() {
        let path = SafeRelativePath::parse("Garmin/existing.img").unwrap();
        let inventory = DeviceInventory {
            transport: TransportKind::MountedMtp,
            paths: vec![
                inspection("internal", &path, DevicePathState::Missing, None),
                inspection("sd", &path, DevicePathState::RegularFile, Some(12)),
            ],
        };

        let target = bind_write_target(&inventory, "internal", path, 24).unwrap();

        assert_eq!(target.storage_id, "sd");
        assert_eq!(target.size, 24);
    }

    #[test]
    fn duplicate_existing_objects_are_not_guessed_between_storages() {
        let path = SafeRelativePath::parse("Garmin/existing.img").unwrap();
        let inventory = DeviceInventory {
            transport: TransportKind::MountedMtp,
            paths: vec![
                inspection("internal", &path, DevicePathState::RegularFile, Some(12)),
                inspection("sd", &path, DevicePathState::RegularFile, Some(12)),
            ],
        };

        let error = bind_write_target(&inventory, "internal", path, 24).unwrap_err();

        assert!(matches!(error, MountedInstallError::AmbiguousStorage(_)));
    }
}
