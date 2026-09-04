use garmin_capture::SessionCapture;
use garmin_device::{
    DeviceInventory, DevicePathState, MountedMtpBackupProgress, PathSafetyError, SafeRelativePath,
    TransportKind,
    storage::{DeviceIoError, DeviceWrite},
};
use garmin_model::map::{InstallationState, MapCatalog, MapComponent, MapFile};
use garmin_progress::{OperationStage, ProgressReporter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;
use tokio::io::AsyncReadExt as _;

const REMOVAL_TRANSACTION_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentDisposition {
    Keep,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalComponent {
    pub index: usize,
    pub name: String,
    pub installation_state: InstallationState,
    pub disposition: ComponentDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalFile {
    pub path: SafeRelativePath,
    pub declared_size: Option<u64>,
    pub removed_with: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preserved_for: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalPlan {
    pub device_digest: String,
    pub components: Vec<RemovalComponent>,
    pub files_to_remove: Vec<RemovalFile>,
    pub files_preserved: Vec<RemovalFile>,
    pub declared_bytes_to_remove: u64,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalExecutionFile {
    pub storage_id: String,
    pub storage_label: String,
    pub path: SafeRelativePath,
    pub size: u64,
    pub removed_with: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalExecutionPlan {
    pub device_digest: String,
    pub source_plan_digest: String,
    pub transport: TransportKind,
    pub files_to_remove: Vec<RemovalExecutionFile>,
    pub files_already_absent: Vec<SafeRelativePath>,
    pub bytes_to_remove: u64,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemovalBackup {
    pub target: RemovalExecutionFile,
    pub backup_file: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalTransactionState {
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemovalTransactionJournal {
    pub version: u8,
    pub device_digest: String,
    pub plan_digest: String,
    pub state: RemovalTransactionState,
    pub deleted_files: usize,
    pub backups: Vec<RemovalBackup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemovalApplyReport {
    pub plan_digest: String,
    pub files_removed: usize,
    pub bytes_removed: u64,
    pub backup_files: usize,
    pub backup_bytes: u64,
    pub backup_directory: PathBuf,
    pub elapsed_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalRecoveryOutcome {
    Committed,
    AlreadyRolledBack,
    Restored,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemovalRecoveryReport {
    pub plan_digest: String,
    pub outcome: RemovalRecoveryOutcome,
    pub files_verified: usize,
    pub files_restored: usize,
}

#[derive(Debug)]
struct Ownership {
    path: SafeRelativePath,
    sizes: BTreeSet<u64>,
    remove_owners: BTreeSet<String>,
    keep_owners: BTreeSet<String>,
}

impl RemovalPlan {
    /// Construct an auditable remove-only plan from the complete OMT catalogue.
    ///
    /// Keeps unchanged installed components to protect their paths. Excludes
    /// response-wide cleanup entries because Garmin omits their owner.
    /// # Errors
    /// Unknown, absent, protected, malformed, empty, or inconsistent selection.
    pub fn from_response_selection(
        response: &MapCatalog,
        device_digest: String,
        removed_components: impl IntoIterator<Item = usize>,
    ) -> Result<Self, RemovalPlanError> {
        let removed = removed_components.into_iter().collect::<BTreeSet<_>>();
        if removed.is_empty() {
            return Err(RemovalPlanError::EmptySelection);
        }
        let maps = response
            .maps
            .iter()
            .chain(&response.bundled_maps)
            .collect::<Vec<_>>();
        if let Some(index) = removed.iter().find(|index| **index >= maps.len()) {
            return Err(RemovalPlanError::UnknownComponent(*index));
        }
        for index in &removed {
            validate_removal_candidate(*index, maps[*index])?;
        }

        let components = maps
            .iter()
            .enumerate()
            .filter(|(_, map)| map.installation_state.is_present())
            .map(|(index, map)| RemovalComponent {
                index,
                name: map.display_name.clone(),
                installation_state: map.installation_state,
                disposition: if removed.contains(&index) {
                    ComponentDisposition::Remove
                } else {
                    ComponentDisposition::Keep
                },
            })
            .collect::<Vec<_>>();

        let mut ownership = BTreeMap::<String, Ownership>::new();
        for (index, map) in maps.iter().enumerate() {
            if !map.installation_state.is_present() {
                continue;
            }
            for file in &map.files_to_remove {
                add_owner(&mut ownership, map, file, removed.contains(&index))?;
            }
        }

        let mut files_to_remove = Vec::new();
        let mut files_preserved = Vec::new();
        for owned in ownership.into_values() {
            if owned.remove_owners.is_empty() {
                continue;
            }
            let declared_size = declared_size(&owned)?;
            let file = RemovalFile {
                path: owned.path,
                declared_size,
                removed_with: owned.remove_owners.into_iter().collect(),
                preserved_for: owned.keep_owners.into_iter().collect(),
            };
            if file.preserved_for.is_empty() {
                files_to_remove.push(file);
            } else {
                files_preserved.push(file);
            }
        }
        if files_to_remove.is_empty() {
            return Err(RemovalPlanError::NoExclusiveFiles);
        }
        let declared_bytes_to_remove = files_to_remove.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.declared_size.unwrap_or(0))
                .ok_or(RemovalPlanError::TotalSizeOverflow)
        })?;
        let mut plan = Self {
            device_digest,
            components,
            files_to_remove,
            files_preserved,
            declared_bytes_to_remove,
            digest: String::new(),
        };
        let canonical = serde_json::to_vec(&plan)?;
        hex::encode(Sha256::digest(canonical))[..32].clone_into(&mut plan.digest);
        Ok(plan)
    }

    #[must_use]
    pub fn paths_to_inventory(&self) -> Vec<SafeRelativePath> {
        self.files_to_remove
            .iter()
            .chain(&self.files_preserved)
            .map(|file| file.path.clone())
            .collect()
    }

    /// Bind this service-derived plan to a read-only physical-device inventory.
    /// # Errors
    /// [`RemovalPlanError`] for an incomplete inventory or unsafe path.
    /// Ambiguity and disagreement with a nonzero Garmin size also fail.
    pub fn bind_inventory(
        &self,
        inventory: &DeviceInventory,
    ) -> Result<RemovalExecutionPlan, RemovalPlanError> {
        let mut files_to_remove = Vec::new();
        let mut files_already_absent = Vec::new();
        for planned in &self.files_to_remove {
            let matches = inventory
                .paths
                .iter()
                .filter(|item| path_key(&item.path) == path_key(&planned.path))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                return Err(RemovalPlanError::IncompleteInventory(planned.path.clone()));
            }
            if let Some(item) = matches.iter().find(|item| {
                matches!(
                    item.state,
                    DevicePathState::Directory
                        | DevicePathState::Other
                        | DevicePathState::Ambiguous
                )
            }) {
                return Err(RemovalPlanError::UnsafeDeviceObject {
                    path: planned.path.clone(),
                    storage: item.storage_label.clone(),
                    state: item.state,
                });
            }
            let present = matches
                .into_iter()
                .filter(|item| item.state == DevicePathState::RegularFile)
                .collect::<Vec<_>>();
            match present.as_slice() {
                [] => files_already_absent.push(planned.path.clone()),
                [item] => {
                    let size = item
                        .size
                        .ok_or_else(|| RemovalPlanError::MissingDeviceSize(planned.path.clone()))?;
                    if let Some(declared) = planned.declared_size
                        && declared != size
                    {
                        return Err(RemovalPlanError::DeviceSizeMismatch {
                            path: planned.path.clone(),
                            declared,
                            actual: size,
                        });
                    }
                    files_to_remove.push(RemovalExecutionFile {
                        storage_id: item.storage_id.clone(),
                        storage_label: item.storage_label.clone(),
                        path: planned.path.clone(),
                        size,
                        removed_with: planned.removed_with.clone(),
                    });
                }
                _ => {
                    return Err(RemovalPlanError::AmbiguousStorage(planned.path.clone()));
                }
            }
        }
        let bytes_to_remove = files_to_remove.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.size)
                .ok_or(RemovalPlanError::TotalSizeOverflow)
        })?;
        let mut plan = RemovalExecutionPlan {
            device_digest: self.device_digest.clone(),
            source_plan_digest: self.digest.clone(),
            transport: inventory.transport,
            files_to_remove,
            files_already_absent,
            bytes_to_remove,
            digest: String::new(),
        };
        let canonical = serde_json::to_vec(&plan)?;
        hex::encode(Sha256::digest(canonical))[..32].clone_into(&mut plan.digest);
        Ok(plan)
    }
}

/// Back up, remove, and verify the exact files in a bound removal plan.
///
/// The device adapter independently verifies every backup.
/// Only then does it write the durable prepared journal.
/// Pre-commit failure restores each missing target
/// from that backup set.
/// # Errors
/// [`RemovalExecutionError`] for an empty plan or changed device state.
/// Capture, cancellation, mutation, and rollback failures are also returned.
pub async fn execute_removal<D: DeviceWrite + ?Sized>(
    plan: &RemovalExecutionPlan,
    capture: &SessionCapture,
    progress: &ProgressReporter,
    device: &D,
) -> Result<RemovalApplyReport, RemovalExecutionError> {
    if plan.files_to_remove.is_empty() {
        return Err(RemovalExecutionError::EmptyPlan);
    }
    let state = refresh_device_state(device, progress).await?;
    crate::space::check_device_writable(
        &state,
        plan.files_to_remove
            .iter()
            .map(|target| target.storage_id.as_str()),
    )?;
    crate::space::check_host_space(capture.root(), plan.bytes_to_remove).await?;
    let started = Instant::now();
    let backup_directory = capture.root().join("removal-backup");
    let (backups, backed_up) = backup_all(plan, capture, progress, device).await?;

    let mut journal = RemovalTransactionJournal {
        version: REMOVAL_TRANSACTION_VERSION,
        device_digest: plan.device_digest.clone(),
        plan_digest: plan.digest.clone(),
        state: RemovalTransactionState::Prepared,
        deleted_files: 0,
        backups,
    };
    capture
        .write_json_atomic(
            Path::new("removal-transaction/000000-prepared.json"),
            &journal,
        )
        .await?;

    let mutation = remove_all(plan, capture, progress, device, &mut journal).await;
    if let Err(operation) = mutation {
        let rollback = rollback_removal(capture, progress, device, &mut journal).await;
        refresh_device_state_after_mutation(device, progress).await;
        return match rollback {
            Ok(()) => Err(operation),
            Err(rollback) => Err(RemovalExecutionError::Rollback {
                operation: operation.to_string(),
                rollback: rollback.to_string(),
            }),
        };
    }

    journal.state = RemovalTransactionState::Committed;
    if let Err(error) = capture
        .write_json_atomic(Path::new("removal-transaction/committed.json"), &journal)
        .await
    {
        let operation = RemovalExecutionError::Capture(error);
        let rollback = rollback_removal(capture, progress, device, &mut journal).await;
        refresh_device_state_after_mutation(device, progress).await;
        return match rollback {
            Ok(()) => Err(operation),
            Err(rollback) => Err(RemovalExecutionError::Rollback {
                operation: operation.to_string(),
                rollback: rollback.to_string(),
            }),
        };
    }
    progress.completed_operations(
        OperationStage::Commit,
        "Selected component files removed and verified",
        u64::try_from(journal.deleted_files)
            .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
        Some(
            u64::try_from(plan.files_to_remove.len())
                .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
        ),
    );
    progress.started(
        OperationStage::Cleanup,
        "Retaining verified recovery backups in the capture",
        Some(1),
    );
    progress.completed(
        OperationStage::Cleanup,
        "Recovery backups retained in the capture",
        1,
        Some(1),
    );
    let report = RemovalApplyReport {
        plan_digest: plan.digest.clone(),
        files_removed: plan.files_to_remove.len(),
        bytes_removed: plan.bytes_to_remove,
        backup_files: journal.backups.len(),
        backup_bytes: backed_up,
        backup_directory,
        elapsed_seconds: started.elapsed().as_secs_f64(),
    };
    refresh_device_state_after_mutation(device, progress).await;
    Ok(report)
}

/// Recover an interrupted removal transaction from its capture directory.
///
/// A durable committed marker preserves a completed removal.
/// Without that marker, every absent target is restored
/// from the prepared backup set.
/// # Errors
/// [`RemovalExecutionError`] for a missing or malformed journal.
/// Invalid backups and failed device restoration or verification are also returned.
pub async fn recover_removal<D: DeviceWrite + ?Sized>(
    capture_root: &Path,
    device_digest: &str,
    progress: &ProgressReporter,
    device: &D,
) -> Result<RemovalRecoveryReport, RemovalExecutionError> {
    let prepared_path = capture_root.join("removal-transaction/000000-prepared.json");
    let prepared = read_removal_journal(&prepared_path).await?;
    validate_removal_journal(&prepared)?;
    if prepared.state != RemovalTransactionState::Prepared || prepared.deleted_files != 0 {
        return Err(RemovalExecutionError::InvalidJournal);
    }
    if prepared.device_digest != device_digest {
        return Err(RemovalExecutionError::RecoveryDeviceMismatch);
    }
    validate_recovery_backups(capture_root, &prepared).await?;
    refresh_device_state(device, progress).await?;
    let committed_path = capture_root.join("removal-transaction/committed.json");
    if tokio::fs::try_exists(&committed_path).await? {
        let committed = read_removal_journal(&committed_path).await?;
        validate_matching_journal(&prepared, &committed, RemovalTransactionState::Committed)?;
        if committed.deleted_files != committed.backups.len() {
            return Err(RemovalExecutionError::InvalidJournal);
        }
        for backup in &prepared.backups {
            require_missing(device, &backup.target).await?;
        }
        return Ok(RemovalRecoveryReport {
            plan_digest: prepared.plan_digest,
            outcome: RemovalRecoveryOutcome::Committed,
            files_verified: prepared.backups.len(),
            files_restored: 0,
        });
    }
    let rolled_back_path = capture_root.join("removal-transaction/rolled-back.json");
    let already_rolled_back = tokio::fs::try_exists(&rolled_back_path).await?;
    if already_rolled_back {
        let rolled_back = read_removal_journal(&rolled_back_path).await?;
        validate_matching_journal(&prepared, &rolled_back, RemovalTransactionState::RolledBack)?;
        for backup in &prepared.backups {
            require_expected_file(device, &backup.target).await?;
            device
                .verify(
                    &backup.target.storage_id,
                    &backup.target.path,
                    backup.target.size,
                    &backup.sha256,
                )
                .await?;
        }
        return Ok(RemovalRecoveryReport {
            plan_digest: prepared.plan_digest,
            outcome: RemovalRecoveryOutcome::AlreadyRolledBack,
            files_verified: prepared.backups.len(),
            files_restored: 0,
        });
    }
    let restored = recover_prepared_targets(capture_root, progress, device, &prepared).await?;
    let report = RemovalRecoveryReport {
        plan_digest: prepared.plan_digest,
        outcome: RemovalRecoveryOutcome::Restored,
        files_verified: prepared.backups.len(),
        files_restored: restored,
    };
    refresh_device_state_after_mutation(device, progress).await;
    Ok(report)
}

async fn recover_prepared_targets<D: DeviceWrite + ?Sized>(
    capture_root: &Path,
    progress: &ProgressReporter,
    device: &D,
    prepared: &RemovalTransactionJournal,
) -> Result<usize, RemovalExecutionError> {
    validate_recovery_targets(device, &prepared.backups).await?;
    check_restoration_space(device, progress, &prepared.backups).await?;
    let mut restored = 0_usize;
    progress.started(
        OperationStage::Cleanup,
        "Recovering interrupted removal",
        Some(
            u64::try_from(prepared.backups.len())
                .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
        ),
    );
    for (index, backup) in prepared.backups.iter().enumerate() {
        match device
            .inspect(&backup.target.storage_id, &backup.target.path)
            .await
            .map_err(RemovalExecutionError::Device)?
        {
            (DevicePathState::Missing, None) => {
                device
                    .restore(
                        &backup.target.storage_id,
                        &backup.target.path,
                        backup.target.size,
                        &capture_root.join(&backup.backup_file),
                        &backup.sha256,
                    )
                    .await
                    .map_err(RemovalExecutionError::Device)?;
                require_expected_file(device, &backup.target).await?;
                device
                    .verify(
                        &backup.target.storage_id,
                        &backup.target.path,
                        backup.target.size,
                        &backup.sha256,
                    )
                    .await
                    .map_err(RemovalExecutionError::Device)?;
                restored += 1;
            }
            (DevicePathState::RegularFile, Some(size)) if size == backup.target.size => {
                device
                    .verify(
                        &backup.target.storage_id,
                        &backup.target.path,
                        backup.target.size,
                        &backup.sha256,
                    )
                    .await
                    .map_err(RemovalExecutionError::Device)?;
            }
            (state, size) => {
                return Err(RemovalExecutionError::UnexpectedDeviceState {
                    path: backup.target.path.clone(),
                    state,
                    size,
                });
            }
        }
        progress.advanced_with_path(
            OperationStage::Cleanup,
            "Verified recovered device file",
            backup.target.path.to_string(),
            u64::try_from(index + 1).map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
            Some(
                u64::try_from(prepared.backups.len())
                    .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
            ),
        );
    }
    progress.completed(
        OperationStage::Cleanup,
        "Interrupted removal recovered",
        u64::try_from(prepared.backups.len())
            .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
        Some(
            u64::try_from(prepared.backups.len())
                .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
        ),
    );
    Ok(restored)
}

async fn read_removal_journal(
    path: &Path,
) -> Result<RemovalTransactionJournal, RemovalExecutionError> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_file() || metadata.len() > 1024 * 1024 {
        return Err(RemovalExecutionError::UnsafeJournal(path.to_owned()));
    }
    Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
}

async fn refresh_device_state<D: DeviceWrite + ?Sized>(
    device: &D,
    progress: &ProgressReporter,
) -> Result<garmin_device::DeviceStateSnapshot, RemovalExecutionError> {
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

async fn refresh_device_state_after_mutation<D: DeviceWrite + ?Sized>(
    device: &D,
    progress: &ProgressReporter,
) {
    progress.device_state(device.state().await.map_err(|error| error.to_string()));
}

async fn check_restoration_space<D: DeviceWrite + ?Sized>(
    device: &D,
    progress: &ProgressReporter,
    backups: &[RemovalBackup],
) -> Result<(), RemovalExecutionError> {
    let mut missing = Vec::new();
    for backup in backups {
        match device
            .inspect(&backup.target.storage_id, &backup.target.path)
            .await?
        {
            (DevicePathState::Missing, None) => missing.push(crate::space::StorageChange {
                storage_id: &backup.target.storage_id,
                before_bytes: 0,
                after_bytes: backup.target.size,
            }),
            (DevicePathState::RegularFile, Some(size)) if size == backup.target.size => {}
            (state, size) => {
                return Err(RemovalExecutionError::UnexpectedDeviceState {
                    path: backup.target.path.clone(),
                    state,
                    size,
                });
            }
        }
    }
    let requirements = crate::space::storage_requirements(missing)?;
    if requirements.is_empty() {
        return Ok(());
    }
    let state = refresh_device_state(device, progress).await?;
    crate::space::check_device_space(&state, &requirements)?;
    Ok(())
}

async fn validate_recovery_targets<D: DeviceWrite + ?Sized>(
    device: &D,
    backups: &[RemovalBackup],
) -> Result<(), RemovalExecutionError> {
    for backup in backups {
        match device
            .inspect(&backup.target.storage_id, &backup.target.path)
            .await?
        {
            (DevicePathState::Missing, None) => {}
            (DevicePathState::RegularFile, Some(size)) if size == backup.target.size => {
                device
                    .verify(
                        &backup.target.storage_id,
                        &backup.target.path,
                        backup.target.size,
                        &backup.sha256,
                    )
                    .await?;
            }
            (state, size) => {
                return Err(RemovalExecutionError::UnexpectedDeviceState {
                    path: backup.target.path.clone(),
                    state,
                    size,
                });
            }
        }
    }
    Ok(())
}

async fn validate_recovery_backups(
    capture_root: &Path,
    journal: &RemovalTransactionJournal,
) -> Result<(), RemovalExecutionError> {
    for backup in &journal.backups {
        let path = capture_root.join(&backup.backup_file);
        let metadata = tokio::fs::symlink_metadata(&path).await?;
        if !metadata.file_type().is_file() || metadata.len() != backup.target.size {
            return Err(RemovalExecutionError::UnsafeJournal(path));
        }
        let mut source = tokio::fs::File::open(&path).await?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = source.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        if hex::encode(digest.finalize()) != backup.sha256 {
            return Err(RemovalExecutionError::InvalidBackup(path));
        }
    }
    Ok(())
}

fn validate_removal_journal(
    journal: &RemovalTransactionJournal,
) -> Result<(), RemovalExecutionError> {
    if journal.version != REMOVAL_TRANSACTION_VERSION {
        return Err(RemovalExecutionError::JournalVersion(journal.version));
    }
    if journal.backups.is_empty() || journal.deleted_files > journal.backups.len() {
        return Err(RemovalExecutionError::InvalidJournal);
    }
    if journal.plan_digest.len() != 32
        || !journal
            .plan_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RemovalExecutionError::InvalidJournal);
    }
    for (index, backup) in journal.backups.iter().enumerate() {
        if backup.backup_file != format!("removal-backup/{:06}.bin", index + 1)
            || backup.sha256.len() != 64
            || !backup.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(RemovalExecutionError::InvalidJournal);
        }
    }
    Ok(())
}

fn validate_matching_journal(
    prepared: &RemovalTransactionJournal,
    other: &RemovalTransactionJournal,
    state: RemovalTransactionState,
) -> Result<(), RemovalExecutionError> {
    validate_removal_journal(other)?;
    if other.state != state
        || other.device_digest != prepared.device_digest
        || other.plan_digest != prepared.plan_digest
        || other.backups != prepared.backups
    {
        return Err(RemovalExecutionError::InvalidJournal);
    }
    Ok(())
}

async fn backup_all<D: DeviceWrite + ?Sized>(
    plan: &RemovalExecutionPlan,
    capture: &SessionCapture,
    progress: &ProgressReporter,
    device: &D,
) -> Result<(Vec<RemovalBackup>, u64), RemovalExecutionError> {
    let mut backups = Vec::with_capacity(plan.files_to_remove.len());
    progress.started(
        OperationStage::Backup,
        "Backing up selected device files",
        Some(plan.bytes_to_remove),
    );
    let mut backed_up = 0_u64;
    for (index, target) in plan.files_to_remove.iter().enumerate() {
        if progress.is_cancelled() {
            return Err(RemovalExecutionError::Cancelled);
        }
        require_expected_file(device, target).await?;
        let backup_file = format!("removal-backup/{:06}.bin", index + 1);
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
                    completed_before: backed_up,
                    total: plan.bytes_to_remove,
                },
            )
            .await
            .map_err(RemovalExecutionError::Device)?;
        let destination = capture.artifact_path(Path::new(&backup_file))?;
        let metadata = tokio::fs::metadata(&destination).await?;
        if !metadata.is_file() || metadata.len() != target.size {
            return Err(RemovalExecutionError::BackupSize {
                path: target.path.clone(),
                expected: target.size,
                actual: metadata.len(),
            });
        }
        backed_up = backed_up
            .checked_add(target.size)
            .ok_or(RemovalExecutionError::ByteCountOverflow)?;
        backups.push(RemovalBackup {
            target: target.clone(),
            backup_file,
            sha256,
        });
        progress.advanced_with_path(
            OperationStage::Backup,
            "Backed up and verified device file",
            target.path.to_string(),
            backed_up,
            Some(plan.bytes_to_remove),
        );
    }
    progress.completed(
        OperationStage::Backup,
        "All selected device files are backed up",
        backed_up,
        Some(plan.bytes_to_remove),
    );
    Ok((backups, backed_up))
}

async fn remove_all<D: DeviceWrite + ?Sized>(
    plan: &RemovalExecutionPlan,
    capture: &SessionCapture,
    progress: &ProgressReporter,
    device: &D,
    journal: &mut RemovalTransactionJournal,
) -> Result<(), RemovalExecutionError> {
    let total = u64::try_from(plan.files_to_remove.len())
        .map_err(|_| RemovalExecutionError::ByteCountOverflow)?;
    progress.started_operations(
        OperationStage::Commit,
        "Removing selected component files",
        Some(total),
    );
    for (index, target) in plan.files_to_remove.iter().enumerate() {
        if progress.is_cancelled() {
            return Err(RemovalExecutionError::Cancelled);
        }
        require_expected_file(device, target).await?;
        device
            .delete(
                &target.storage_id,
                &target.path,
                target.size,
                &journal.backups[index].sha256,
            )
            .await
            .map_err(RemovalExecutionError::Device)?;
        require_missing(device, target).await?;
        journal.deleted_files += 1;
        capture
            .write_json_atomic(
                Path::new(&format!(
                    "removal-transaction/{:06}-deleted.json",
                    journal.deleted_files
                )),
                journal,
            )
            .await?;
        progress.advanced_operations_with_path(
            OperationStage::Commit,
            "Removed and verified device file",
            target.path.to_string(),
            u64::try_from(journal.deleted_files)
                .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
            Some(total),
        );
    }
    Ok(())
}

async fn rollback_removal<D: DeviceWrite + ?Sized>(
    capture: &SessionCapture,
    progress: &ProgressReporter,
    device: &D,
    journal: &mut RemovalTransactionJournal,
) -> Result<(), RemovalExecutionError> {
    validate_recovery_backups(capture.root(), journal).await?;
    check_restoration_space(device, progress, &journal.backups).await?;
    progress.started(
        OperationStage::Cleanup,
        "Restoring removal transaction",
        Some(
            u64::try_from(journal.backups.len())
                .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
        ),
    );
    let mut restored = 0_u64;
    for backup in &journal.backups {
        match device
            .inspect(&backup.target.storage_id, &backup.target.path)
            .await
            .map_err(RemovalExecutionError::Device)?
        {
            (DevicePathState::Missing, None) => {
                device
                    .restore(
                        &backup.target.storage_id,
                        &backup.target.path,
                        backup.target.size,
                        &capture.root().join(&backup.backup_file),
                        &backup.sha256,
                    )
                    .await
                    .map_err(RemovalExecutionError::Device)?;
                require_expected_file(device, &backup.target).await?;
                device
                    .verify(
                        &backup.target.storage_id,
                        &backup.target.path,
                        backup.target.size,
                        &backup.sha256,
                    )
                    .await
                    .map_err(RemovalExecutionError::Device)?;
            }
            (DevicePathState::RegularFile, Some(size)) if size == backup.target.size => {
                device
                    .verify(
                        &backup.target.storage_id,
                        &backup.target.path,
                        backup.target.size,
                        &backup.sha256,
                    )
                    .await
                    .map_err(RemovalExecutionError::Device)?;
            }
            (state, size) => {
                return Err(RemovalExecutionError::UnexpectedDeviceState {
                    path: backup.target.path.clone(),
                    state,
                    size,
                });
            }
        }
        restored += 1;
        progress.advanced_with_path(
            OperationStage::Cleanup,
            "Verified restored device file",
            backup.target.path.to_string(),
            restored,
            Some(
                u64::try_from(journal.backups.len())
                    .map_err(|_| RemovalExecutionError::ByteCountOverflow)?,
            ),
        );
    }
    journal.state = RemovalTransactionState::RolledBack;
    capture
        .write_json_atomic(Path::new("removal-transaction/rolled-back.json"), journal)
        .await?;
    progress.completed(
        OperationStage::Cleanup,
        "Removal transaction rolled back",
        restored,
        Some(restored),
    );
    Ok(())
}

async fn require_expected_file<D: DeviceWrite + ?Sized>(
    device: &D,
    target: &RemovalExecutionFile,
) -> Result<(), RemovalExecutionError> {
    let (state, size) = device
        .inspect(&target.storage_id, &target.path)
        .await
        .map_err(RemovalExecutionError::Device)?;
    if state == DevicePathState::RegularFile && size == Some(target.size) {
        Ok(())
    } else {
        Err(RemovalExecutionError::UnexpectedDeviceState {
            path: target.path.clone(),
            state,
            size,
        })
    }
}

async fn require_missing<D: DeviceWrite + ?Sized>(
    device: &D,
    target: &RemovalExecutionFile,
) -> Result<(), RemovalExecutionError> {
    let (state, size) = device
        .inspect(&target.storage_id, &target.path)
        .await
        .map_err(RemovalExecutionError::Device)?;
    if state == DevicePathState::Missing {
        Ok(())
    } else {
        Err(RemovalExecutionError::UnexpectedDeviceState {
            path: target.path.clone(),
            state,
            size,
        })
    }
}

#[derive(Debug, Error)]
pub enum RemovalExecutionError {
    #[error("the bound removal plan contains no device files")]
    EmptyPlan,
    #[error("removal was cancelled")]
    Cancelled,
    #[error("device removal operation failed: {0}")]
    Device(#[from] DeviceIoError),
    #[error(transparent)]
    Space(#[from] crate::space::SpaceError),
    #[error("capture operation failed: {0}")]
    Capture(#[from] garmin_capture::CaptureError),
    #[error("removal backup I/O failed: {0}")]
    BackupIo(#[from] std::io::Error),
    #[error("removal journal is unsafe: {0}")]
    UnsafeJournal(PathBuf),
    #[error("removal recovery backup failed verification: {0}")]
    InvalidBackup(PathBuf),
    #[error("unsupported removal journal version {0}")]
    JournalVersion(u8),
    #[error("removal journal is internally inconsistent")]
    InvalidJournal,
    #[error("the recovery journal belongs to a different device")]
    RecoveryDeviceMismatch,
    #[error("removal journal is invalid JSON: {0}")]
    JournalJson(#[from] serde_json::Error),
    #[error("backup size mismatch for {path}: expected {expected}, found {actual}")]
    BackupSize {
        path: SafeRelativePath,
        expected: u64,
        actual: u64,
    },
    #[error("device state changed for {path}: {state:?}, size {size:?}")]
    UnexpectedDeviceState {
        path: SafeRelativePath,
        state: DevicePathState,
        size: Option<u64>,
    },
    #[error("removal byte count exceeds the supported range")]
    ByteCountOverflow,
    #[error("removal failed ({operation}); rollback also failed ({rollback})")]
    Rollback { operation: String, rollback: String },
}

fn validate_removal_candidate(index: usize, map: &MapComponent) -> Result<(), RemovalPlanError> {
    if !map.installation_state.is_present() {
        return Err(RemovalPlanError::NotInstalled {
            index,
            name: map.display_name.clone(),
        });
    }
    if !map.can_uninstall {
        return Err(RemovalPlanError::RequiredComponent {
            index,
            name: map.display_name.clone(),
        });
    }
    if map.files_to_remove.is_empty() {
        return Err(RemovalPlanError::NoRemovalMetadata {
            index,
            name: map.display_name.clone(),
        });
    }
    Ok(())
}

fn add_owner(
    ownership: &mut BTreeMap<String, Ownership>,
    map: &MapComponent,
    file: &MapFile,
    remove: bool,
) -> Result<(), RemovalPlanError> {
    let path = SafeRelativePath::parse(&file.file_name)?;
    let key = path_key(&path);
    let owned = ownership.entry(key).or_insert_with(|| Ownership {
        path,
        sizes: BTreeSet::new(),
        remove_owners: BTreeSet::new(),
        keep_owners: BTreeSet::new(),
    });
    if file.size_in_bytes != 0 {
        owned.sizes.insert(file.size_in_bytes);
    }
    if remove {
        owned.remove_owners.insert(map.display_name.clone());
    } else {
        owned.keep_owners.insert(map.display_name.clone());
    }
    Ok(())
}

fn declared_size(owned: &Ownership) -> Result<Option<u64>, RemovalPlanError> {
    match owned.sizes.len() {
        0 => Ok(None),
        1 => Ok(owned.sizes.first().copied()),
        _ => Err(RemovalPlanError::ConflictingFileSize {
            path: owned.path.clone(),
            sizes: owned.sizes.iter().copied().collect(),
        }),
    }
}

fn path_key(path: &SafeRelativePath) -> String {
    path.to_string().replace('\\', "/").to_ascii_lowercase()
}

#[derive(Debug, Error)]
pub enum RemovalPlanError {
    #[error("select at least one installed component to remove")]
    EmptySelection,
    #[error("component index {0} is outside the Garmin response")]
    UnknownComponent(usize),
    #[error("component {index} ({name}) is not installed")]
    NotInstalled { index: usize, name: String },
    #[error("component {index} ({name}) is required by this device and cannot be removed")]
    RequiredComponent { index: usize, name: String },
    #[error("component {index} ({name}) has no removal metadata")]
    NoRemovalMetadata { index: usize, name: String },
    #[error("selected components have no files that are not required by retained components")]
    NoExclusiveFiles,
    #[error("Garmin reported conflicting sizes for {path}: {sizes:?}")]
    ConflictingFileSize {
        path: SafeRelativePath,
        sizes: Vec<u64>,
    },
    #[error(transparent)]
    UnsafePath(#[from] PathSafetyError),
    #[error("unable to serialize the canonical removal plan: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("declared removal size exceeds the supported range")]
    TotalSizeOverflow,
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
    #[error("size mismatch for {path}: Garmin declared {declared}, device reported {actual}")]
    DeviceSizeMismatch {
        path: SafeRelativePath,
        declared: u64,
        actual: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_device::{
        DevicePathInspection, DeviceStateSnapshot, DeviceStorageState, StorageCapacity,
        storage::{DeviceRead, DeviceWrite},
    };
    use std::io::Write as _;
    use std::sync::Mutex;

    struct FakeRemovalDevice {
        state: Mutex<FakeRemovalState>,
    }

    struct FakeRemovalState {
        files: BTreeMap<String, Vec<u8>>,
        fail_delete: Option<String>,
        mutate_after_backup: Option<String>,
        capacity: u64,
        writable: bool,
    }

    impl FakeRemovalDevice {
        fn key(storage: &str, path: &SafeRelativePath) -> String {
            format!("{storage}:{path}")
        }

        fn files_len(&self) -> usize {
            self.state.lock().expect("fixture state").files.len()
        }

        fn set_delete_failure(&self, path: &str) {
            self.state.lock().expect("fixture state").fail_delete = Some(path.to_owned());
        }

        fn set_mutation_after_backup(&self, path: &str) {
            self.state
                .lock()
                .expect("fixture state")
                .mutate_after_backup = Some(path.to_owned());
        }

        fn remove(&self, target: &RemovalExecutionFile) {
            self.state
                .lock()
                .expect("fixture state")
                .files
                .remove(&Self::key(&target.storage_id, &target.path));
        }

        fn replace(&self, target: &RemovalExecutionFile, bytes: Vec<u8>) {
            self.state
                .lock()
                .expect("fixture state")
                .files
                .insert(Self::key(&target.storage_id, &target.path), bytes);
        }

        fn set_storage(&self, capacity: u64, writable: bool) {
            let mut state = self.state.lock().expect("fixture state");
            state.capacity = capacity;
            state.writable = writable;
        }
    }

    #[async_trait::async_trait]
    impl DeviceRead for FakeRemovalDevice {
        fn execution_target(&self) -> Option<&str> {
            None
        }

        async fn state(&self) -> Result<DeviceStateSnapshot, DeviceIoError> {
            let state = self.state.lock().expect("fixture state");
            let used = state
                .files
                .values()
                .try_fold(0_u64, |total, bytes| {
                    total.checked_add(u64::try_from(bytes.len()).expect("fixture size"))
                })
                .expect("fixture total");
            Ok(DeviceStateSnapshot {
                storages: vec![DeviceStorageState {
                    id: "internal".to_owned(),
                    label: "Device storage".to_owned(),
                    capacity: StorageCapacity::new(
                        state.capacity,
                        state.capacity.saturating_sub(used),
                    ),
                    writable: Some(state.writable),
                }],
            })
        }

        async fn inventory(
            &self,
            paths: &[SafeRelativePath],
        ) -> Result<DeviceInventory, DeviceIoError> {
            let state = self.state.lock().expect("fixture state");
            Ok(DeviceInventory {
                transport: TransportKind::MountedMtp,
                paths: paths
                    .iter()
                    .map(|path| {
                        let size = state
                            .files
                            .get(&Self::key("internal", path))
                            .map(|bytes| u64::try_from(bytes.len()).expect("fixture size"));
                        DevicePathInspection {
                            storage_id: "internal".to_owned(),
                            storage_label: "Device storage".to_owned(),
                            path: path.clone(),
                            state: if size.is_some() {
                                DevicePathState::RegularFile
                            } else {
                                DevicePathState::Missing
                            },
                            size,
                        }
                    })
                    .collect(),
            })
        }

        async fn primary_storage_id(&self) -> Result<String, DeviceIoError> {
            Ok("internal".to_owned())
        }

        async fn inspect(
            &self,
            storage: &str,
            path: &SafeRelativePath,
        ) -> Result<(DevicePathState, Option<u64>), DeviceIoError> {
            Ok(self
                .state
                .lock()
                .expect("fixture state")
                .files
                .get(&Self::key(storage, path))
                .map_or((DevicePathState::Missing, None), |bytes| {
                    (
                        DevicePathState::RegularFile,
                        Some(u64::try_from(bytes.len()).expect("fixture size")),
                    )
                }))
        }

        async fn backup(
            &self,
            storage: &str,
            path: &SafeRelativePath,
            _size: u64,
            destination: garmin_device::BackupDestination,
            _progress: MountedMtpBackupProgress,
        ) -> Result<String, DeviceIoError> {
            let mut state = self.state.lock().expect("fixture state");
            let bytes = state
                .files
                .get(&Self::key(storage, path))
                .ok_or_else(|| std::io::Error::other("fixture file is absent"))?
                .clone();
            let mut destination = destination.into_file();
            destination.write_all(&bytes)?;
            destination.flush()?;
            destination.sync_all()?;
            if state.mutate_after_backup.as_deref() == Some(path.to_string().as_str()) {
                state
                    .files
                    .get_mut(&Self::key(storage, path))
                    .expect("fixture file")
                    .fill(0x5A);
            }
            Ok(hex::encode(Sha256::digest(&bytes)))
        }

        async fn verify(
            &self,
            storage: &str,
            path: &SafeRelativePath,
            _size: u64,
            sha256: &str,
        ) -> Result<(), DeviceIoError> {
            let state = self.state.lock().expect("fixture state");
            let bytes = state
                .files
                .get(&Self::key(storage, path))
                .ok_or_else(|| std::io::Error::other("fixture file is absent"))?;
            if hex::encode(Sha256::digest(bytes)) != sha256 {
                return Err(std::io::Error::other("device checksum mismatch").into());
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl DeviceWrite for FakeRemovalDevice {
        async fn delete(
            &self,
            storage: &str,
            path: &SafeRelativePath,
            _size: u64,
            sha256: &str,
        ) -> Result<(), DeviceIoError> {
            let mut state = self.state.lock().expect("fixture state");
            if state.fail_delete.as_deref() == Some(path.to_string().as_str()) {
                return Err(std::io::Error::other("injected deletion failure").into());
            }
            let bytes = state
                .files
                .get(&Self::key(storage, path))
                .ok_or_else(|| std::io::Error::other("fixture file is absent"))?;
            if hex::encode(Sha256::digest(bytes)) != sha256 {
                return Err(std::io::Error::other("device checksum mismatch").into());
            }
            state.files.remove(&Self::key(storage, path));
            Ok(())
        }

        async fn upload(
            &self,
            _storage: &str,
            _path: &SafeRelativePath,
            _source: &Path,
            _size: u64,
            _sha256: &str,
            _progress: garmin_device::MountedMtpUploadProgress,
        ) -> Result<(), DeviceIoError> {
            Err(DeviceIoError::Transport(
                "fixture upload is unsupported".to_owned(),
            ))
        }

        async fn restore(
            &self,
            storage: &str,
            path: &SafeRelativePath,
            _size: u64,
            backup: &Path,
            sha256: &str,
        ) -> Result<(), DeviceIoError> {
            let bytes = tokio::fs::read(backup).await?;
            if hex::encode(Sha256::digest(&bytes)) != sha256 {
                return Err(std::io::Error::other("backup checksum mismatch").into());
            }
            self.state
                .lock()
                .expect("fixture state")
                .files
                .insert(Self::key(storage, path), bytes);
            Ok(())
        }
    }

    fn execution_plan() -> RemovalExecutionPlan {
        let files_to_remove = [("Garmin/one.img", 3_u64), ("Garmin/two.sid", 4_u64)]
            .into_iter()
            .map(|(path, size)| RemovalExecutionFile {
                storage_id: "internal".to_owned(),
                storage_label: "Device storage".to_owned(),
                path: SafeRelativePath::parse(path).expect("safe fixture path"),
                size,
                removed_with: vec!["Optional map".to_owned()],
            })
            .collect();
        RemovalExecutionPlan {
            device_digest: "device".to_owned(),
            source_plan_digest: "source".to_owned(),
            transport: TransportKind::MountedMtp,
            files_to_remove,
            files_already_absent: Vec::new(),
            bytes_to_remove: 7,
            digest: "0123456789abcdef0123456789abcdef".to_owned(),
        }
    }

    fn fake_device(plan: &RemovalExecutionPlan) -> FakeRemovalDevice {
        let files = plan
            .files_to_remove
            .iter()
            .map(|target| {
                (
                    FakeRemovalDevice::key(&target.storage_id, &target.path),
                    vec![0xA5; usize::try_from(target.size).expect("fixture size")],
                )
            })
            .collect();
        FakeRemovalDevice {
            state: Mutex::new(FakeRemovalState {
                files,
                fail_delete: None,
                mutate_after_backup: None,
                capacity: 1_000_000,
                writable: true,
            }),
        }
    }

    async fn prepared_recovery(
        plan: &RemovalExecutionPlan,
        device: &FakeRemovalDevice,
    ) -> (tempfile::TempDir, SessionCapture) {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let capture = SessionCapture::create(&temporary.path().join("capture")).expect("capture");
        let mut backups = Vec::new();
        for (index, target) in plan.files_to_remove.iter().enumerate() {
            let backup_file = format!("removal-backup/{:06}.bin", index + 1);
            let destination_file = capture
                .create_file(Path::new(&backup_file))
                .await
                .expect("backup file")
                .into_std()
                .await;
            let destination = garmin_device::BackupDestination::new(
                capture
                    .artifact_path(Path::new(&backup_file))
                    .expect("backup path"),
                destination_file,
            )
            .expect("backup destination");
            let sha256 = device
                .backup(
                    &target.storage_id,
                    &target.path,
                    target.size,
                    destination,
                    MountedMtpBackupProgress {
                        reporter: ProgressReporter::default(),
                        completed_before: 0,
                        total: plan.bytes_to_remove,
                    },
                )
                .await
                .expect("fixture backup");
            backups.push(RemovalBackup {
                target: target.clone(),
                backup_file,
                sha256,
            });
        }
        capture
            .write_json_atomic(
                Path::new("removal-transaction/000000-prepared.json"),
                &RemovalTransactionJournal {
                    version: REMOVAL_TRANSACTION_VERSION,
                    device_digest: plan.device_digest.clone(),
                    plan_digest: plan.digest.clone(),
                    state: RemovalTransactionState::Prepared,
                    deleted_files: 0,
                    backups,
                },
            )
            .await
            .expect("prepared journal");
        (temporary, capture)
    }

    fn map(
        name: &str,
        state: InstallationState,
        can_uninstall: bool,
        files: &[(&str, u64)],
    ) -> MapComponent {
        MapComponent {
            display_name: name.to_owned(),
            installation_state: state,
            can_uninstall,
            files_to_remove: files
                .iter()
                .map(|(path, size)| MapFile {
                    file_name: (*path).to_owned(),
                    part_number: String::new(),
                    size_in_bytes: *size,
                    is_shared: false,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_paths_owned_by_an_installed_component_that_remains() {
        let response = MapCatalog {
            maps: vec![
                map(
                    "Central",
                    InstallationState::Installed,
                    true,
                    &[("Garmin/central.img", 20), ("Garmin/shared.img", 10)],
                ),
                map(
                    "West",
                    InstallationState::Installed,
                    true,
                    &[("Garmin/west.img", 30), ("garmin/SHARED.img", 10)],
                ),
            ],
            ..Default::default()
        };

        let plan = RemovalPlan::from_response_selection(&response, "device".to_owned(), [0])
            .expect("safe removal plan");

        assert_eq!(plan.files_to_remove.len(), 1);
        assert_eq!(
            plan.files_to_remove[0].path.to_string(),
            "Garmin/central.img"
        );
        assert_eq!(plan.declared_bytes_to_remove, 20);
        assert_eq!(plan.files_preserved.len(), 1);
        assert_eq!(plan.files_preserved[0].preserved_for, ["West"]);
    }

    #[test]
    fn rejects_absent_and_required_components() {
        let response = MapCatalog {
            maps: vec![
                map("Optional", InstallationState::NotInstalled, true, &[]),
                map(
                    "Base maps",
                    InstallationState::Installed,
                    false,
                    &[("Garmin/base.img", 10)],
                ),
            ],
            ..Default::default()
        };

        assert!(matches!(
            RemovalPlan::from_response_selection(&response, "device".to_owned(), [0]),
            Err(RemovalPlanError::NotInstalled { .. })
        ));
        assert!(matches!(
            RemovalPlan::from_response_selection(&response, "device".to_owned(), [1]),
            Err(RemovalPlanError::RequiredComponent { .. })
        ));
    }

    #[test]
    fn binds_only_one_regular_object_per_removal_path() {
        let response = MapCatalog {
            maps: vec![map(
                "Optional",
                InstallationState::Installed,
                true,
                &[("Garmin/map.img", 20)],
            )],
            ..Default::default()
        };
        let plan = RemovalPlan::from_response_selection(&response, "device".to_owned(), [0])
            .expect("removal plan");
        let inventory = DeviceInventory {
            transport: TransportKind::MountedMtp,
            paths: vec![DevicePathInspection {
                storage_id: "internal".to_owned(),
                storage_label: "Device storage".to_owned(),
                path: SafeRelativePath::parse("Garmin/map.img").unwrap(),
                state: DevicePathState::RegularFile,
                size: Some(20),
            }],
        };

        let execution = plan.bind_inventory(&inventory).expect("bound inventory");

        assert_eq!(execution.files_to_remove.len(), 1);
        assert_eq!(execution.bytes_to_remove, 20);
        assert!(execution.files_already_absent.is_empty());
    }

    #[tokio::test]
    async fn execution_backs_up_journals_deletes_and_verifies() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        let temporary = tempfile::tempdir().expect("temporary directory");
        let capture = SessionCapture::create(&temporary.path().join("capture")).expect("capture");

        let report = execute_removal(&plan, &capture, &ProgressReporter::default(), &device)
            .await
            .expect("completed removal");

        assert_eq!(device.files_len(), 0);
        assert_eq!(report.files_removed, 2);
        assert_eq!(report.backup_bytes, 7);
        assert!(
            capture
                .root()
                .join("removal-transaction/committed.json")
                .is_file()
        );
    }

    #[tokio::test]
    async fn execution_rejects_read_only_storage_before_creating_backups() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        device.set_storage(1_000_000, false);
        let temporary = tempfile::tempdir().expect("temporary directory");
        let capture = SessionCapture::create(&temporary.path().join("capture")).expect("capture");

        let result = execute_removal(&plan, &capture, &ProgressReporter::default(), &device).await;

        assert!(matches!(
            result,
            Err(RemovalExecutionError::Space(
                crate::space::SpaceError::ReadOnly(_)
            ))
        ));
        assert_eq!(device.files_len(), 2);
        assert!(!capture.root().join("removal-backup/000001.bin").exists());
    }

    #[tokio::test]
    async fn execution_restores_prior_deletions_when_a_later_delete_fails() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        device.set_delete_failure("Garmin/two.sid");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let capture = SessionCapture::create(&temporary.path().join("capture")).expect("capture");

        let result = execute_removal(&plan, &capture, &ProgressReporter::default(), &device).await;

        assert!(matches!(result, Err(RemovalExecutionError::Device(_))));
        assert_eq!(device.files_len(), 2);
        assert!(
            capture
                .root()
                .join("removal-transaction/rolled-back.json")
                .is_file()
        );
        assert!(
            !capture
                .root()
                .join("removal-transaction/committed.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn execution_refuses_a_same_sized_file_that_changed_after_backup() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        device.set_mutation_after_backup("Garmin/two.sid");
        let temporary = tempfile::tempdir().expect("temporary directory");
        let capture = SessionCapture::create(&temporary.path().join("capture")).expect("capture");

        let result = execute_removal(&plan, &capture, &ProgressReporter::default(), &device).await;

        assert!(matches!(
            result,
            Err(RemovalExecutionError::Rollback { .. })
        ));
        assert_eq!(device.files_len(), 2);
        assert!(
            !capture
                .root()
                .join("removal-transaction/committed.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn recovery_restores_an_absent_target_without_a_committed_marker() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        let (_temporary, capture) = prepared_recovery(&plan, &device).await;
        device.remove(&plan.files_to_remove[0]);

        let report = recover_removal(
            capture.root(),
            &plan.device_digest,
            &ProgressReporter::default(),
            &device,
        )
        .await
        .expect("recovered removal");

        assert_eq!(report.outcome, RemovalRecoveryOutcome::Restored);
        assert_eq!(report.files_restored, 1);
        assert_eq!(device.files_len(), 2);
    }

    #[tokio::test]
    async fn recovery_rejects_insufficient_space_before_restoring_any_file() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        let (_temporary, capture) = prepared_recovery(&plan, &device).await;
        device.remove(&plan.files_to_remove[0]);
        device.set_storage(5, true);

        let result = recover_removal(
            capture.root(),
            &plan.device_digest,
            &ProgressReporter::default(),
            &device,
        )
        .await;

        assert!(matches!(
            result,
            Err(RemovalExecutionError::Space(
                crate::space::SpaceError::Insufficient { .. }
            ))
        ));
        assert_eq!(device.files_len(), 1);
    }

    #[tokio::test]
    async fn recovery_validates_every_backup_before_restoring_any_file() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        let (_temporary, capture) = prepared_recovery(&plan, &device).await;
        device.remove(&plan.files_to_remove[0]);
        std::fs::write(capture.root().join("removal-backup/000002.bin"), [0_u8; 4])
            .expect("corrupt backup");

        let result = recover_removal(
            capture.root(),
            &plan.device_digest,
            &ProgressReporter::default(),
            &device,
        )
        .await;

        assert!(matches!(
            result,
            Err(RemovalExecutionError::InvalidBackup(_))
        ));
        assert_eq!(device.files_len(), 1);
    }

    #[tokio::test]
    async fn recovery_validates_existing_targets_before_restoring_any_file() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        let (_temporary, capture) = prepared_recovery(&plan, &device).await;
        device.remove(&plan.files_to_remove[0]);
        device.replace(&plan.files_to_remove[1], vec![0x5A; 4]);

        let result = recover_removal(
            capture.root(),
            &plan.device_digest,
            &ProgressReporter::default(),
            &device,
        )
        .await;

        assert!(matches!(result, Err(RemovalExecutionError::Device(_))));
        assert_eq!(device.files_len(), 1);
    }

    #[tokio::test]
    async fn rolled_back_marker_verifies_device_drift_without_repairing_it() {
        let plan = execution_plan();
        let device = fake_device(&plan);
        let (_temporary, capture) = prepared_recovery(&plan, &device).await;
        let prepared_path = capture
            .root()
            .join("removal-transaction/000000-prepared.json");
        let mut journal = read_removal_journal(&prepared_path)
            .await
            .expect("prepared journal");
        journal.state = RemovalTransactionState::RolledBack;
        capture
            .write_json_atomic(Path::new("removal-transaction/rolled-back.json"), &journal)
            .await
            .expect("rolled-back marker");
        device.remove(&plan.files_to_remove[0]);

        let result = recover_removal(
            capture.root(),
            &plan.device_digest,
            &ProgressReporter::default(),
            &device,
        )
        .await;

        assert!(matches!(
            result,
            Err(RemovalExecutionError::UnexpectedDeviceState { .. })
        ));
        assert_eq!(device.files_len(), 1);
    }
}
