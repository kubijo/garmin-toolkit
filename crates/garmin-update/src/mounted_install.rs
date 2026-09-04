use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use garmin_capture::SessionCapture;
use garmin_device::storage::{DeviceIoError, DeviceWrite};
use garmin_device::{
    DeviceInventory, DevicePathState, MountedMtpBackupProgress, MountedMtpUploadProgress,
    PathSafetyError, SafeRelativePath, TransportKind,
};
use garmin_model::map::MapAuthorization;
use garmin_progress::{OperationStage, ProgressReporter};
use md5::{Digest as _, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::{ApplyReport, BackupPolicy, DownloadProgress, RecoveryOutcome, UpdatePlan};

const TRANSACTION_VERSION: u8 = 3;

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
}

#[derive(Debug, Clone, Serialize)]
pub struct MountedUpdateRecoveryReport {
    pub plan_digest: String,
    pub outcome: MountedUpdateRecoveryOutcome,
    pub files_verified: usize,
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
    unbacked_original: Option<BoundObject>,
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
    unbacked_removals: Vec<BoundObject>,
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
    Remove { size: u64, sha256: String },
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
///
/// Backs up and verifies every replaced object before mutation. Uploaded
/// objects are read back and hashed; ordinary failures restore the capture.
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
    let state = require_device_state(device, &progress).await?;
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

    let mutation = commit_update(&prepared, &journal, device, capture, &progress).await;
    if let Err(operation) = mutation {
        if plan.backup_policy == BackupPolicy::Skip {
            return Err(MountedInstallError::UnprotectedMutation {
                operation: operation.to_string(),
            });
        }
        let rollback = rollback_update(&journal, device, capture.root(), &progress).await;
        return match rollback {
            Ok(()) => Err(operation),
            Err(rollback) => Err(MountedInstallError::Rollback {
                operation: operation.to_string(),
                rollback: rollback.to_string(),
            }),
        };
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
            return Err(MountedInstallError::UnprotectedMutation {
                operation: operation.to_string(),
            });
        }
        let rollback = rollback_update(&journal, device, capture.root(), &progress).await;
        return match rollback {
            Ok(()) => Err(operation),
            Err(rollback) => Err(MountedInstallError::Rollback {
                operation: operation.to_string(),
                rollback: rollback.to_string(),
            }),
        };
    }
    let cleanup = match plan.backup_policy {
        BackupPolicy::Verified => "Verified recovery backups retained in the capture",
        BackupPolicy::Skip => "Update evidence retained; no recovery backup was created",
    };
    progress.completed(OperationStage::Cleanup, cleanup, 1, Some(1));
    Ok(ApplyReport {
        files_written: prepared.payloads.len(),
        bytes_written: plan.total_bytes,
        files_removed: prepared.removals.len(),
        unlocks_written: activation.unlocks.len(),
        backup_policy: plan.backup_policy,
        recovery: RecoveryOutcome::NoTransaction,
        elapsed: started.elapsed(),
    })
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
        prepared.removals.clone()
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
/// A commit marker preserves and verifies installed state. Otherwise, new
/// objects are removed and captured originals restored.
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
    publish_device_state(device, progress).await;
    let committed_path = capture_root.join("mounted-update/transaction/committed.json");
    if tokio::fs::try_exists(&committed_path).await? {
        let committed = read_journal(&committed_path).await?;
        validate_matching_journal(&prepared, &committed, JournalState::Committed)?;
        verify_committed(&committed, device).await?;
        publish_device_state(device, progress).await;
        return Ok(MountedUpdateRecoveryReport {
            plan_digest: committed.plan_digest,
            outcome: MountedUpdateRecoveryOutcome::Committed,
            files_verified: committed.writes.len()
                + committed.removals.len()
                + committed.unbacked_removals.len(),
        });
    }

    if prepared.backup_policy == BackupPolicy::Skip {
        return Err(MountedInstallError::RecoveryUnavailable);
    }

    let rolled_back_path = capture_root.join("mounted-update/transaction/rolled-back.json");
    if tokio::fs::try_exists(&rolled_back_path).await? {
        let rolled_back = read_journal(&rolled_back_path).await?;
        validate_matching_journal(&prepared, &rolled_back, JournalState::RolledBack)?;
        verify_rolled_back(&rolled_back, device).await?;
        publish_device_state(device, progress).await;
        return Ok(MountedUpdateRecoveryReport {
            plan_digest: rolled_back.plan_digest,
            outcome: MountedUpdateRecoveryOutcome::AlreadyRolledBack,
            files_verified: rolled_back.writes.len() + rolled_back.removals.len(),
        });
    }

    rollback_update(&prepared, device, capture_root, progress).await?;
    publish_device_state(device, progress).await;
    Ok(MountedUpdateRecoveryReport {
        plan_digest: prepared.plan_digest,
        outcome: MountedUpdateRecoveryOutcome::Restored,
        files_verified: prepared.writes.len() + prepared.removals.len(),
    })
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
    if !(1..=TRANSACTION_VERSION).contains(&journal.version) {
        return Err(MountedInstallError::JournalVersion(journal.version));
    }
    if journal.device_digest != device_digest {
        return Err(MountedInstallError::RecoveryDeviceMismatch);
    }
    if journal.state != state || journal.writes.is_empty() {
        return Err(MountedInstallError::InvalidJournal);
    }
    if journal.version < TRANSACTION_VERSION
        && (journal.backup_policy != BackupPolicy::Verified
            || !journal.unbacked_removals.is_empty()
            || journal
                .writes
                .iter()
                .any(|write| write.unbacked_original.is_some()))
    {
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
        if journal.version >= 2 && write.payload_file.is_none() {
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
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    for write in &journal.writes {
        device
            .verify(
                &write.target.storage_id,
                &write.target.path,
                write.target.size,
                &write.sha256,
            )
            .await?;
    }
    for removal in &journal.removals {
        let (state, size) = device
            .inspect(&removal.target.storage_id, &removal.target.path)
            .await?;
        if state != DevicePathState::Missing {
            return Err(MountedInstallError::UnexpectedDeviceState {
                path: removal.target.path.clone(),
                state,
                size,
            });
        }
    }
    for removal in &journal.unbacked_removals {
        let (state, size) = device.inspect(&removal.storage_id, &removal.path).await?;
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
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    for write in &journal.writes {
        if let Some(original) = &write.original {
            device
                .verify(
                    &original.target.storage_id,
                    &original.target.path,
                    original.target.size,
                    &original.sha256,
                )
                .await?;
        } else {
            let (state, size) = device
                .inspect(&write.target.storage_id, &write.target.path)
                .await?;
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
        device
            .verify(
                &removal.target.storage_id,
                &removal.target.path,
                removal.target.size,
                &removal.sha256,
            )
            .await?;
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
    let snapshot = require_device_state(device, progress).await?;
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
    let requirements = crate::space::storage_requirements(changes)?;
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
            .inspect(&payload.target.storage_id, &payload.target.path)
            .await?;
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
        require_running(progress)?;
        write_transaction_event(capture, operation + 1, "started", write).await?;
        if let Some(original) = &write.original {
            device
                .delete(
                    &original.target.storage_id,
                    &original.target.path,
                    original.target.size,
                    &original.sha256,
                )
                .await?;
        } else if let Some(original) = &write.unbacked_original {
            device
                .delete_unverified(&original.storage_id, &original.path, original.size)
                .await?;
        }
        device
            .upload(
                &payload.target.storage_id,
                &payload.target.path,
                &payload.source,
                payload.target.size,
                &payload.sha256,
                MountedMtpUploadProgress {
                    reporter: progress.clone(),
                    completed_before: completed_bytes,
                    total: total_bytes,
                },
            )
            .await?;
        completed_bytes = completed_bytes
            .checked_add(payload.target.size)
            .ok_or(MountedInstallError::ByteCountOverflow)?;
        operation += 1;
        write_transaction_event(capture, operation, "applied", journal).await?;
    }
    for original in &journal.removals {
        require_running(progress)?;
        device
            .delete(
                &original.target.storage_id,
                &original.target.path,
                original.target.size,
                &original.sha256,
            )
            .await?;
        operation += 1;
        write_transaction_event(capture, operation, "applied", journal).await?;
    }
    for target in &journal.unbacked_removals {
        require_running(progress)?;
        device
            .delete_unverified(&target.storage_id, &target.path, target.size)
            .await?;
        operation += 1;
        write_transaction_event(capture, operation, "applied", journal).await?;
    }
    progress.completed(
        OperationStage::Commit,
        "Update files written and verified",
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

fn require_running(progress: &ProgressReporter) -> Result<(), MountedInstallError> {
    if progress.is_cancelled() {
        return Err(MountedInstallError::Cancelled);
    }
    Ok(())
}

async fn write_transaction_event<T>(
    capture: &SessionCapture,
    operation: usize,
    state: &str,
    value: &T,
) -> Result<(), MountedInstallError>
where
    T: Serialize,
{
    capture
        .write_json_atomic(
            Path::new(&format!(
                "mounted-update/transaction/{operation:06}-{state}.json"
            )),
            value,
        )
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
    ensure_rollback_marker(capture_root, journal).await?;
    let mut writes = Vec::new();
    for (index, write) in journal.writes.iter().enumerate() {
        writes.push(inspect_recovery_write(device, write, capture_root, index + 1).await?);
    }
    let mut removals = Vec::with_capacity(journal.removals.len());
    for original in &journal.removals {
        removals.push(inspect_recovery_original(device, original, capture_root).await?);
    }
    check_recovery_space(journal, &writes, &removals, device, progress).await?;
    for (write, state) in journal.writes.iter().zip(writes).rev() {
        if let RecoveryObjectState::Remove { size, sha256 } = state {
            device
                .delete(&write.target.storage_id, &write.target.path, size, &sha256)
                .await?;
        }
        if let Some(original) = &write.original {
            restore_original(device, capture_root, original).await?;
        }
    }
    for (original, state) in journal.removals.iter().zip(removals).rev() {
        if let RecoveryObjectState::Remove { size, sha256 } = state {
            device
                .delete(
                    &original.target.storage_id,
                    &original.target.path,
                    size,
                    &sha256,
                )
                .await?;
        }
        restore_original(device, capture_root, original).await?;
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
) -> Result<RecoveryObjectState, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let (state, size) = device
        .inspect(&write.target.storage_id, &write.target.path)
        .await?;
    match (state, size) {
        (DevicePathState::Missing, None) => Ok(RecoveryObjectState::Missing),
        (DevicePathState::RegularFile, Some(size)) => {
            if let Some(original) = &write.original
                && original.target.size == size
                && device
                    .verify(
                        &original.target.storage_id,
                        &original.target.path,
                        size,
                        &original.sha256,
                    )
                    .await
                    .is_ok()
            {
                return Ok(RecoveryObjectState::Original);
            }
            if size == write.target.size {
                device
                    .verify(
                        &write.target.storage_id,
                        &write.target.path,
                        size,
                        &write.sha256,
                    )
                    .await?;
                return Ok(RecoveryObjectState::Remove {
                    size,
                    sha256: write.sha256.clone(),
                });
            }
            let digest = verify_partial_write(device, write, capture_root, operation, size).await?;
            Ok(RecoveryObjectState::Remove {
                size,
                sha256: digest,
            })
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
) -> Result<RecoveryObjectState, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let (state, size) = device
        .inspect(&original.target.storage_id, &original.target.path)
        .await?;
    match (state, size) {
        (DevicePathState::Missing, None) => Ok(RecoveryObjectState::Missing),
        (DevicePathState::RegularFile, Some(size)) if size == original.target.size => {
            device
                .verify(
                    &original.target.storage_id,
                    &original.target.path,
                    size,
                    &original.sha256,
                )
                .await?;
            Ok(RecoveryObjectState::Original)
        }
        (DevicePathState::RegularFile, Some(size)) => {
            let backup = recovery_file(capture_root, &original.backup_file).await?;
            let digest = verify_partial_object(
                device,
                &original.target,
                size,
                [(
                    backup.as_path(),
                    original.target.size,
                    original.sha256.as_str(),
                )],
                capture_root,
            )
            .await?;
            Ok(RecoveryObjectState::Remove {
                size,
                sha256: digest,
            })
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
            RecoveryObjectState::Remove { size, .. } => {
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
            RecoveryObjectState::Remove { size, .. } => {
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
    let state = require_device_state(device, progress).await?;
    crate::space::check_device_space(&state, &requirements)?;
    Ok(())
}

async fn require_device_state<D>(
    device: &D,
    progress: &ProgressReporter,
) -> Result<garmin_device::DeviceStateSnapshot, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    match device.state().await {
        Ok(state) => {
            progress.device_state(Ok(state.clone()));
            Ok(state)
        }
        Err(error) => {
            progress.device_state(Err(error.to_string()));
            Err(error.into())
        }
    }
}

async fn publish_device_state<D>(device: &D, progress: &ProgressReporter)
where
    D: DeviceWrite + ?Sized,
{
    progress.device_state(device.state().await.map_err(|error| error.to_string()));
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
) -> Result<String, MountedInstallError> {
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
    verify_partial_object(device, &write.target, size, sources, root).await
}

async fn verify_partial_object<'a, D>(
    device: &D,
    target: &BoundObject,
    size: u64,
    sources: impl IntoIterator<Item = (&'a Path, u64, &'a str)>,
    root: &Path,
) -> Result<String, MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let blocked = || MountedInstallError::RecoveryEvidence(target.path.clone());
    let mut expected = Vec::new();
    for (source, source_size, source_sha256) in sources {
        if size == 0 || size >= source_size {
            continue;
        }
        if verify_local_payload(source, source_size, None).await? != source_sha256 {
            return Err(blocked());
        }
        let mut source = tokio::fs::File::open(source).await?.take(size);
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1024 * 1024];
        loop {
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
                reporter: ProgressReporter::default(),
                completed_before: 0,
                total: size,
            },
        )
        .await?;
    drop(temporary_guard);
    if !expected.contains(&actual) {
        return Err(blocked());
    }
    Ok(actual)
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
) -> Result<(), MountedInstallError>
where
    D: DeviceWrite + ?Sized,
{
    let (state, size) = device
        .inspect(&original.target.storage_id, &original.target.path)
        .await?;
    match (state, size) {
        (DevicePathState::Missing, None) => {
            device
                .restore(
                    &original.target.storage_id,
                    &original.target.path,
                    original.target.size,
                    &recovery_file(capture_root, &original.backup_file).await?,
                    &original.sha256,
                )
                .await?;
            Ok(())
        }
        (DevicePathState::RegularFile, Some(size)) if size == original.target.size => {
            device
                .verify(
                    &original.target.storage_id,
                    &original.target.path,
                    original.target.size,
                    &original.sha256,
                )
                .await?;
            Ok(())
        }
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

#[derive(Debug, Error)]
pub enum MountedInstallError {
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
    Rollback { operation: String, rollback: String },
    #[error(
        "update failed after device mutation with recovery backup disabled: {operation}; automatic rollback is unavailable and reinstall may be required"
    )]
    UnprotectedMutation { operation: String },
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
    #[error("mounted MTP device operation failed: {0}")]
    Device(#[from] DeviceIoError),
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

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_device::DevicePathInspection;

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
