use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use garmin_capture::SessionCapture;
use garmin_device::{PathSafetyError, SafeRelativePath};
use garmin_model::map::MapAuthorization;
use garmin_progress::{OperationStage, ProgressReporter};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

use crate::{DownloadProgress, UpdatePlan};

const JOURNAL_FILE_NAME: &str = ".garmin-cli-transaction-v1.json";
const JOURNAL_NEXT_FILE_NAME: &str = ".garmin-cli-transaction-v1.next";
const JOURNAL_VERSION: u8 = 1;
const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
const MAX_JOURNAL_OPERATIONS: usize = 2048;

#[derive(Debug, Clone, Serialize)]
pub struct ApplyReport {
    pub files_written: usize,
    pub bytes_written: u64,
    pub files_removed: usize,
    pub unlocks_written: usize,
    pub backup_policy: crate::BackupPolicy,
    pub recovery: RecoveryOutcome,
    #[serde(skip)]
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOutcome {
    NoTransaction,
    RolledBack,
    Finalized,
}

impl ApplyReport {
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "an approximate transfer rate is intentionally represented as f64"
    )]
    pub fn bytes_per_second(&self) -> f64 {
        if self.elapsed.is_zero() {
            0.0
        } else {
            self.bytes_written as f64 / self.elapsed.as_secs_f64()
        }
    }
}

/// Timing imposed by a device transport implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceTiming {
    bytes_per_second: Option<NonZeroU64>,
    operation_latency: Duration,
}

impl DeviceTiming {
    #[must_use]
    pub const fn limited(bytes_per_second: NonZeroU64, operation_latency: Duration) -> Self {
        Self {
            bytes_per_second: Some(bytes_per_second),
            operation_latency,
        }
    }

    async fn after_write(self, bytes: u64) {
        if let Some(bytes_per_second) = self.bytes_per_second {
            #[expect(
                clippy::cast_precision_loss,
                reason = "simulated transport timing is intentionally approximate"
            )]
            let seconds = bytes as f64 / bytes_per_second.get() as f64;
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        }
    }

    async fn after_operation(self) {
        if !self.operation_latency.is_zero() {
            tokio::time::sleep(self.operation_latency).await;
        }
    }
}

struct PendingFile {
    relative: SafeRelativePath,
    destination: PathBuf,
    partial: PathBuf,
    backup: Option<PathBuf>,
    content: PendingContent,
}

enum PendingContent {
    Staged {
        path: PathBuf,
        size: u64,
        md5: String,
    },
    Bytes(Vec<u8>),
}

struct CopyProgress<'a> {
    reporter: &'a ProgressReporter,
    completed_before: u64,
    total: u64,
    relative: &'a SafeRelativePath,
    timing: DeviceTiming,
}

struct PendingRemoval {
    relative: SafeRelativePath,
    destination: PathBuf,
    parked: PathBuf,
}

struct PreparedTransaction {
    pending: Vec<PendingFile>,
    removals: Vec<PendingRemoval>,
    journal: TransactionJournal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionJournal {
    version: u8,
    plan_digest: String,
    state: TransactionState,
    installs: Vec<JournalInstall>,
    removals: Vec<SafeRelativePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionState {
    Prepared,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalInstall {
    destination: SafeRelativePath,
    had_original: bool,
}

/// Commit a fully staged update to a mounted mass-storage device.
///
/// Stages and syncs new files before moving existing files. A device-local
/// journal survives process termination, disconnection, and power loss.
/// # Errors
/// Unsafe or incomplete staging, insufficient space, unsupported authorization, or I/O.
pub async fn apply_mass_storage(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    root: &Path,
) -> Result<ApplyReport, ApplyError> {
    apply_mass_storage_with_progress(
        plan,
        staged,
        activation,
        root,
        ProgressReporter::default(),
        DeviceTiming::default(),
        None,
    )
    .await
}

/// Validate a mass-storage update transaction without changing the device.
///
/// Checks payloads, authorization, paths, sidecars, and space like the real installer.
/// # Errors
/// The pre-commit failures from [`apply_mass_storage`].
pub async fn preflight_mass_storage_update(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    root: &Path,
) -> Result<(), ApplyError> {
    require_real_directory(root).await?;
    validate_plan_digest(&plan.digest)?;
    if plan.downloads.len() != staged.len() {
        return Err(ApplyError::IncompleteStage {
            expected: plan.downloads.len(),
            actual: staged.len(),
        });
    }
    if path_exists(&root.join(JOURNAL_FILE_NAME)).await?
        || path_exists(&root.join(JOURNAL_NEXT_FILE_NAME)).await?
    {
        return Err(ApplyError::JournalConflict(root.to_owned()));
    }
    ensure_available_space(root, required_update_space(plan, activation)?)?;

    let mut destinations = BTreeSet::new();
    for (spec, staged_file) in plan.downloads.iter().zip(staged) {
        require_non_reserved(&spec.destination)?;
        let destination = inspect_destination(root, &spec.destination).await?;
        register_destination(&mut destinations, &destination)?;
        require_sidecars_absent_if_reachable(&destination, &plan.digest).await?;
        verify_staged_payload(&staged_file.path, spec.size, &spec.md5).await?;
    }
    for unlock in &activation.unlocks {
        let relative = SafeRelativePath::parse(&unlock.file_name)?;
        require_non_reserved(&relative)?;
        let destination = inspect_destination(root, &relative).await?;
        register_destination(&mut destinations, &destination)?;
        require_sidecars_absent_if_reachable(&destination, &plan.digest).await?;
        BASE64.decode(&unlock.gma)?;
    }
    for relative in &plan.files_to_remove {
        require_non_reserved(relative)?;
        let destination = inspect_destination(root, relative).await?;
        if !destinations.contains(&destination) {
            require_sidecars_absent_if_reachable(&destination, &plan.digest).await?;
        }
    }
    Ok(())
}

async fn inspect_destination(
    root: &Path,
    relative: &SafeRelativePath,
) -> Result<PathBuf, ApplyError> {
    let destination = relative.under(root);
    let parent = destination
        .parent()
        .ok_or_else(|| ApplyError::NoParent(destination.clone()))?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| ApplyError::UnsafeDevicePath(parent.to_owned()))?;
    let mut current = root.to_owned();
    let mut reachable = true;
    for component in relative_parent.components() {
        current.push(component);
        if !reachable {
            continue;
        }
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(ApplyError::UnsafeDevicePath(current)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => reachable = false,
            Err(error) => return Err(error.into()),
        }
    }
    if reachable {
        match tokio::fs::symlink_metadata(&destination).await {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err(ApplyError::UnsafeDevicePath(destination)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(destination)
}

async fn require_sidecars_absent_if_reachable(
    destination: &Path,
    digest: &str,
) -> Result<(), ApplyError> {
    let Some(parent) = destination.parent() else {
        return Err(ApplyError::NoParent(destination.to_owned()));
    };
    match tokio::fs::symlink_metadata(parent).await {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err(ApplyError::UnsafeDevicePath(parent.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    require_absent(&sidecar_path(destination, "partial", digest)?).await?;
    require_absent(&sidecar_path(destination, "backup", digest)?).await?;
    require_absent(&sidecar_path(destination, "removed", digest)?).await
}

async fn verify_staged_payload(
    path: &Path,
    expected_size: u64,
    expected_md5: &str,
) -> Result<(), ApplyError> {
    let mut file = tokio::fs::File::open(path).await?;
    let metadata = file.metadata().await?;
    if !metadata.is_file() {
        return Err(ApplyError::UnsafeStagedFile(path.to_owned()));
    }
    if metadata.len() != expected_size {
        return Err(ApplyError::StagedSize {
            path: path.to_owned(),
            expected: expected_size,
            actual: metadata.len(),
        });
    }
    let mut hash = Md5::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(u64::try_from(count).map_err(|_| ApplyError::StagedSizeOverflow)?)
            .ok_or(ApplyError::StagedSizeOverflow)?;
        hash.update(&buffer[..count]);
    }
    if copied != expected_size {
        return Err(ApplyError::StagedSize {
            path: path.to_owned(),
            expected: expected_size,
            actual: copied,
        });
    }
    let actual = hex::encode(hash.finalize());
    if !actual.eq_ignore_ascii_case(expected_md5) {
        return Err(ApplyError::StagedChecksum {
            expected: expected_md5.to_owned(),
            actual,
        });
    }
    Ok(())
}

/// Commit a staged update while reporting its transaction phases.
/// # Errors
/// The same errors as [`apply_mass_storage`].
pub async fn apply_mass_storage_with_progress(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    root: &Path,
    progress: ProgressReporter,
    timing: DeviceTiming,
    capture: Option<&SessionCapture>,
) -> Result<ApplyReport, ApplyError> {
    require_real_directory(root).await?;
    if plan.backup_policy == crate::BackupPolicy::Skip {
        return Err(ApplyError::BackupPolicyUnsupported);
    }
    let recovery = recover_mass_storage(root).await?;
    let required_space = required_update_space(plan, activation)?;
    ensure_available_space(root, required_space)?;

    let started = Instant::now();
    let transaction = prepare_transaction(plan, staged, activation, root).await?;
    let files_removed = transaction.removals.len();
    let commit_operations = u64::try_from(transaction.pending.len() + transaction.removals.len())
        .map_err(|_| ApplyError::TooManyJournalOperations)?
        .max(1);
    if let Some(capture) = capture {
        capture
            .write_json(
                Path::new("transaction/prepared-journal.json"),
                &transaction.journal,
            )
            .await?;
    }
    write_new_journal(root, &transaction.journal).await?;

    if progress.is_cancelled() {
        return Err(abort_transaction(root, ApplyError::Cancelled).await);
    }

    if let Err(error) = stage_pending_files(&transaction.pending, &progress, timing).await {
        progress.failed(OperationStage::Stage, "Device staging failed");
        return Err(abort_transaction(root, error).await);
    }
    if let Err(error) = commit_transaction(&transaction, &progress, timing, commit_operations).await
    {
        progress.failed(OperationStage::Commit, "Device commit failed");
        return Err(abort_transaction(root, error).await);
    }
    if let Err(error) = sync_transaction_directories(root, &transaction).await {
        return Err(abort_transaction(root, error).await);
    }

    let mut committed = transaction.journal.clone();
    committed.state = TransactionState::Committed;
    if let Some(capture) = capture {
        capture
            .write_json(Path::new("transaction/committed-journal.json"), &committed)
            .await?;
    }
    if let Err(error) = replace_journal(root, &committed).await {
        progress.failed(
            OperationStage::Commit,
            "Could not record the committed transaction",
        );
        return Err(abort_transaction(root, error).await);
    }
    progress.completed_operations(
        OperationStage::Commit,
        "Update committed atomically",
        commit_operations,
        Some(commit_operations),
    );
    progress.started(
        OperationStage::Cleanup,
        "Removing transaction recovery files",
        Some(1),
    );
    timing.after_operation().await;
    let finalized = recover_mass_storage(root).await?;
    if finalized != RecoveryOutcome::Finalized {
        return Err(ApplyError::JournalState(
            "committed transaction was not finalized".to_owned(),
        ));
    }
    progress.completed(
        OperationStage::Cleanup,
        "Transaction journal and recovery files removed",
        1,
        Some(1),
    );

    Ok(ApplyReport {
        files_written: plan.downloads.len(),
        bytes_written: plan.total_bytes,
        files_removed,
        unlocks_written: activation.unlocks.len(),
        backup_policy: plan.backup_policy,
        recovery,
        elapsed: started.elapsed(),
    })
}

fn required_update_space(
    plan: &UpdatePlan,
    activation: &MapAuthorization,
) -> Result<u64, ApplyError> {
    if !activation.embedded_unlocks.is_empty() {
        return Err(ApplyError::EmbeddedUnlocksUnsupported);
    }
    if activation.signed_sd_card_bytes.is_some() {
        return Err(ApplyError::SignedStorageDataUnsupported);
    }

    activation
        .unlocks
        .iter()
        .try_fold(plan.total_bytes, |required, unlock| {
            let encoded =
                u64::try_from(unlock.gma.len()).map_err(|_| ApplyError::RequiredSpaceOverflow)?;
            required
                .checked_add(encoded)
                .ok_or(ApplyError::RequiredSpaceOverflow)
        })
}

fn ensure_available_space(root: &Path, required: u64) -> Result<(), ApplyError> {
    let free = fs2::available_space(root)?;
    if free < required {
        return Err(ApplyError::InsufficientSpace {
            required,
            available: free,
        });
    }
    Ok(())
}

async fn prepare_transaction(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    activation: &MapAuthorization,
    root: &Path,
) -> Result<PreparedTransaction, ApplyError> {
    validate_plan_digest(&plan.digest)?;
    if plan.downloads.len() != staged.len() {
        return Err(ApplyError::IncompleteStage {
            expected: plan.downloads.len(),
            actual: staged.len(),
        });
    }

    let mut pending = Vec::with_capacity(plan.downloads.len() + activation.unlocks.len());
    let mut destinations = BTreeSet::new();
    append_downloads(plan, staged, root, &mut destinations, &mut pending).await?;
    append_unlocks(plan, activation, root, &mut destinations, &mut pending).await?;
    let removals = prepare_removals(plan, root, &pending).await?;

    let operation_count = pending
        .len()
        .checked_add(removals.len())
        .ok_or(ApplyError::TooManyJournalOperations)?;
    if operation_count > MAX_JOURNAL_OPERATIONS {
        return Err(ApplyError::TooManyJournalOperations);
    }
    let journal = TransactionJournal {
        version: JOURNAL_VERSION,
        plan_digest: plan.digest.clone(),
        state: TransactionState::Prepared,
        installs: pending
            .iter()
            .map(|item| JournalInstall {
                destination: item.relative.clone(),
                had_original: item.backup.is_some(),
            })
            .collect(),
        removals: removals.iter().map(|item| item.relative.clone()).collect(),
    };
    validate_journal(&journal)?;
    Ok(PreparedTransaction {
        pending,
        removals,
        journal,
    })
}

async fn append_downloads(
    plan: &UpdatePlan,
    staged: &[DownloadProgress],
    root: &Path,
    destinations: &mut BTreeSet<PathBuf>,
    pending: &mut Vec<PendingFile>,
) -> Result<(), ApplyError> {
    for (spec, staged_file) in plan.downloads.iter().zip(staged) {
        if staged_file.bytes != spec.size {
            return Err(ApplyError::StagedSize {
                path: staged_file.path.clone(),
                expected: spec.size,
                actual: staged_file.bytes,
            });
        }
        require_non_reserved(&spec.destination)?;
        let destination = prepare_destination(root, &spec.destination).await?;
        register_destination(destinations, &destination)?;
        let partial = sidecar_path(&destination, "partial", &plan.digest)?;
        require_absent(&partial).await?;
        let backup = path_exists(&destination)
            .await?
            .then(|| sidecar_path(&destination, "backup", &plan.digest))
            .transpose()?;
        if let Some(path) = &backup {
            require_absent(path).await?;
        }
        pending.push(PendingFile {
            relative: spec.destination.clone(),
            destination,
            partial,
            backup,
            content: PendingContent::Staged {
                path: staged_file.path.clone(),
                size: spec.size,
                md5: spec.md5.clone(),
            },
        });
    }
    Ok(())
}

async fn append_unlocks(
    plan: &UpdatePlan,
    activation: &MapAuthorization,
    root: &Path,
    destinations: &mut BTreeSet<PathBuf>,
    pending: &mut Vec<PendingFile>,
) -> Result<(), ApplyError> {
    for unlock in &activation.unlocks {
        let relative = SafeRelativePath::parse(&unlock.file_name)?;
        require_non_reserved(&relative)?;
        let destination = prepare_destination(root, &relative).await?;
        register_destination(destinations, &destination)?;
        let partial = sidecar_path(&destination, "partial", &plan.digest)?;
        require_absent(&partial).await?;
        let bytes = BASE64.decode(&unlock.gma)?;
        let backup = path_exists(&destination)
            .await?
            .then(|| sidecar_path(&destination, "backup", &plan.digest))
            .transpose()?;
        if let Some(path) = &backup {
            require_absent(path).await?;
        }
        pending.push(PendingFile {
            relative,
            destination,
            partial,
            backup,
            content: PendingContent::Bytes(bytes),
        });
    }
    Ok(())
}

async fn prepare_removals(
    plan: &UpdatePlan,
    root: &Path,
    pending: &[PendingFile],
) -> Result<Vec<PendingRemoval>, ApplyError> {
    let targets = pending
        .iter()
        .map(|item| item.destination.clone())
        .collect::<BTreeSet<_>>();
    let mut removals = Vec::new();
    for relative in &plan.files_to_remove {
        require_non_reserved(relative)?;
        let path = prepare_destination(root, relative).await?;
        if targets.contains(&path) || !path_exists(&path).await? {
            continue;
        }
        let parked = sidecar_path(&path, "removed", &plan.digest)?;
        require_absent(&parked).await?;
        removals.push(PendingRemoval {
            relative: relative.clone(),
            destination: path,
            parked,
        });
    }
    Ok(removals)
}

async fn stage_pending_files(
    pending: &[PendingFile],
    progress: &ProgressReporter,
    timing: DeviceTiming,
) -> Result<(), ApplyError> {
    let total = pending.iter().try_fold(0_u64, |sum, item| {
        let size = match &item.content {
            PendingContent::Staged { size, .. } => *size,
            PendingContent::Bytes(bytes) => {
                u64::try_from(bytes.len()).map_err(|_| ApplyError::StagedSizeOverflow)?
            }
        };
        sum.checked_add(size).ok_or(ApplyError::StagedSizeOverflow)
    })?;
    progress.started(
        OperationStage::Stage,
        "Copying verified files to transaction sidecars",
        Some(total),
    );
    let mut completed = 0_u64;
    for item in pending {
        if progress.is_cancelled() {
            return Err(ApplyError::Cancelled);
        }
        match &item.content {
            PendingContent::Staged { path, size, md5 } => {
                copy_verified(
                    path,
                    &item.partial,
                    *size,
                    md5,
                    CopyProgress {
                        reporter: progress,
                        completed_before: completed,
                        total,
                        relative: &item.relative,
                        timing,
                    },
                )
                .await?;
                completed = completed
                    .checked_add(*size)
                    .ok_or(ApplyError::StagedSizeOverflow)?;
            }
            PendingContent::Bytes(bytes) => {
                write_fsynced(&item.partial, bytes).await?;
                let size =
                    u64::try_from(bytes.len()).map_err(|_| ApplyError::StagedSizeOverflow)?;
                timing.after_write(size).await;
                completed = completed
                    .checked_add(size)
                    .ok_or(ApplyError::StagedSizeOverflow)?;
                progress.advanced_with_path(
                    OperationStage::Stage,
                    "Staged transaction file",
                    item.relative.to_string(),
                    completed,
                    Some(total),
                );
            }
        }
    }
    sync_paths(pending.iter().map(|item| item.partial.as_path())).await?;
    progress.completed(
        OperationStage::Stage,
        "All files staged and synced on the device",
        total,
        Some(total),
    );
    Ok(())
}

async fn commit_transaction(
    transaction: &PreparedTransaction,
    progress: &ProgressReporter,
    timing: DeviceTiming,
    total: u64,
) -> Result<(), ApplyError> {
    progress.started_operations(
        OperationStage::Commit,
        "Atomically switching staged files into place",
        Some(total),
    );
    let mut completed = 0_u64;
    for item in &transaction.pending {
        if progress.is_cancelled() {
            return Err(ApplyError::Cancelled);
        }
        if let Some(backup) = &item.backup {
            tokio::fs::rename(&item.destination, backup).await?;
            timing.after_operation().await;
        }
    }
    sync_paths(
        transaction
            .pending
            .iter()
            .filter_map(|item| item.backup.as_deref()),
    )
    .await?;

    for item in &transaction.pending {
        if progress.is_cancelled() {
            return Err(ApplyError::Cancelled);
        }
        tokio::fs::rename(&item.partial, &item.destination).await?;
        timing.after_operation().await;
        completed += 1;
        progress.advanced_operations_with_path(
            OperationStage::Commit,
            "Installed map file",
            item.relative.to_string(),
            completed,
            Some(total),
        );
    }

    for item in &transaction.removals {
        if progress.is_cancelled() {
            return Err(ApplyError::Cancelled);
        }
        tokio::fs::rename(&item.destination, &item.parked).await?;
        timing.after_operation().await;
        completed += 1;
        progress.advanced_operations_with_path(
            OperationStage::Commit,
            "Retired replaced file",
            item.relative.to_string(),
            completed,
            Some(total),
        );
    }
    Ok(())
}

fn register_destination(
    destinations: &mut BTreeSet<PathBuf>,
    destination: &Path,
) -> Result<(), ApplyError> {
    if !destinations.insert(destination.to_owned()) {
        return Err(ApplyError::DuplicateDestination(destination.to_owned()));
    }
    Ok(())
}

async fn prepare_destination(
    root: &Path,
    relative: &SafeRelativePath,
) -> Result<PathBuf, ApplyError> {
    let destination = relative.under(root);
    let parent = destination
        .parent()
        .ok_or_else(|| ApplyError::NoParent(destination.clone()))?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| ApplyError::UnsafeDevicePath(parent.to_owned()))?;
    let mut current = root.to_owned();
    for component in relative_parent.components() {
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(ApplyError::UnsafeDevicePath(current)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&current).await?;
                let parent = current
                    .parent()
                    .ok_or_else(|| ApplyError::NoParent(current.clone()))?;
                sync_directory(parent).await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    match tokio::fs::symlink_metadata(&destination).await {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err(ApplyError::UnsafeDevicePath(destination)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(destination)
}

async fn require_real_directory(path: &Path) -> Result<(), ApplyError> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_dir() {
        return Err(ApplyError::UnsafeDevicePath(path.to_owned()));
    }
    Ok(())
}

async fn require_absent(path: &Path) -> Result<(), ApplyError> {
    match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(ApplyError::SidecarExists(path.to_owned())),
    }
}

async fn path_exists(path: &Path) -> Result<bool, ApplyError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(ApplyError::UnsafeDevicePath(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn sidecar_path(destination: &Path, role: &str, digest: &str) -> Result<PathBuf, ApplyError> {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ApplyError::InvalidFileName(destination.to_owned()))?;
    Ok(destination.with_file_name(format!(".{name}.garmin-cli-{role}-{digest}")))
}

async fn copy_verified(
    source: &Path,
    destination: &Path,
    expected_size: u64,
    expected_md5: &str,
    progress: CopyProgress<'_>,
) -> Result<(), ApplyError> {
    let source_path = source.to_owned();
    let source_file = tokio::fs::File::open(source).await?;
    let metadata = source_file.metadata().await?;
    if !metadata.is_file() {
        return Err(ApplyError::UnsafeStagedFile(source_path));
    }
    if metadata.len() != expected_size {
        return Err(ApplyError::StagedSize {
            path: source_path,
            expected: expected_size,
            actual: metadata.len(),
        });
    }
    let mut source = BufReader::with_capacity(4 * 1024 * 1024, source_file);
    let mut destination = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await?;
    let mut hash = Md5::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(u64::try_from(count).map_err(|_| ApplyError::StagedSizeOverflow)?)
            .ok_or(ApplyError::StagedSizeOverflow)?;
        if copied > expected_size {
            return Err(ApplyError::StagedSize {
                path: source_path,
                expected: expected_size,
                actual: copied,
            });
        }
        destination.write_all(&buffer[..count]).await?;
        progress
            .timing
            .after_write(u64::try_from(count).map_err(|_| ApplyError::StagedSizeOverflow)?)
            .await;
        hash.update(&buffer[..count]);
        progress.reporter.advanced_with_path(
            OperationStage::Stage,
            "Staging map file",
            progress.relative.to_string(),
            progress
                .completed_before
                .checked_add(copied)
                .ok_or(ApplyError::StagedSizeOverflow)?,
            Some(progress.total),
        );
    }
    if copied != expected_size {
        return Err(ApplyError::StagedSize {
            path: source_path,
            expected: expected_size,
            actual: copied,
        });
    }
    let actual_md5 = hex::encode(hash.finalize());
    if !actual_md5.eq_ignore_ascii_case(expected_md5) {
        return Err(ApplyError::StagedChecksum {
            expected: expected_md5.to_owned(),
            actual: actual_md5,
        });
    }
    destination.flush().await?;
    destination.sync_all().await?;
    Ok(())
}

async fn write_fsynced(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    file.sync_all().await
}

/// Recover the transaction journal at a mounted mass-storage root.
///
/// Rolls back a prepared transaction or cleans a committed transaction's
/// sidecars.
/// # Errors
/// Malformed journal, conflicting device state, unsafe path, or durable-I/O failure.
pub async fn recover_mass_storage(root: &Path) -> Result<RecoveryOutcome, ApplyError> {
    require_real_directory(root).await?;
    let Some(journal) = read_journal(root).await? else {
        remove_abandoned_next_journal(root).await?;
        return Ok(RecoveryOutcome::NoTransaction);
    };
    remove_abandoned_next_journal(root).await?;

    let outcome = match journal.state {
        TransactionState::Prepared => {
            rollback_journal(root, &journal).await?;
            RecoveryOutcome::RolledBack
        }
        TransactionState::Committed => {
            finalize_journal(root, &journal).await?;
            RecoveryOutcome::Finalized
        }
    };
    sync_journal_directories(root, &journal).await?;
    remove_regular_file_if_exists(&root.join(JOURNAL_FILE_NAME)).await?;
    sync_directory(root).await?;
    Ok(outcome)
}

async fn rollback_journal(root: &Path, journal: &TransactionJournal) -> Result<(), ApplyError> {
    for relative in journal.removals.iter().rev() {
        let destination = recovery_destination(root, relative).await?;
        let parked = sidecar_path(&destination, "removed", &journal.plan_digest)?;
        if path_exists(&parked).await? {
            if path_exists(&destination).await? {
                return Err(ApplyError::JournalConflict(destination));
            }
            tokio::fs::rename(parked, destination).await?;
        }
    }

    for install in journal.installs.iter().rev() {
        let destination = recovery_destination(root, &install.destination).await?;
        let partial = sidecar_path(&destination, "partial", &journal.plan_digest)?;
        let backup = sidecar_path(&destination, "backup", &journal.plan_digest)?;
        let destination_exists = path_exists(&destination).await?;
        let partial_exists = path_exists(&partial).await?;
        let backup_exists = path_exists(&backup).await?;

        if install.had_original {
            if backup_exists {
                if destination_exists {
                    tokio::fs::remove_file(&destination).await?;
                }
                tokio::fs::rename(&backup, &destination).await?;
            } else if !destination_exists {
                return Err(ApplyError::JournalConflict(destination));
            }
        } else {
            if destination_exists && partial_exists {
                return Err(ApplyError::JournalConflict(destination));
            }
            if destination_exists {
                tokio::fs::remove_file(&destination).await?;
            }
            if backup_exists {
                return Err(ApplyError::JournalConflict(backup));
            }
        }
        if partial_exists {
            tokio::fs::remove_file(partial).await?;
        }
    }
    Ok(())
}

async fn finalize_journal(root: &Path, journal: &TransactionJournal) -> Result<(), ApplyError> {
    for install in &journal.installs {
        let destination = recovery_destination(root, &install.destination).await?;
        if !path_exists(&destination).await? {
            return Err(ApplyError::JournalConflict(destination));
        }
        let partial = sidecar_path(&destination, "partial", &journal.plan_digest)?;
        remove_regular_file_if_exists(&partial).await?;
        if install.had_original {
            let backup = sidecar_path(&destination, "backup", &journal.plan_digest)?;
            remove_regular_file_if_exists(&backup).await?;
        }
    }
    for relative in &journal.removals {
        let destination = recovery_destination(root, relative).await?;
        if path_exists(&destination).await? {
            return Err(ApplyError::JournalConflict(destination));
        }
        let parked = sidecar_path(&destination, "removed", &journal.plan_digest)?;
        remove_regular_file_if_exists(&parked).await?;
    }
    Ok(())
}

async fn abort_transaction(root: &Path, operation: ApplyError) -> ApplyError {
    match recover_mass_storage(root).await {
        Ok(RecoveryOutcome::RolledBack) => operation,
        Ok(outcome) => ApplyError::RecoveryAfterFailure {
            operation: operation.to_string(),
            recovery: format!("unexpected recovery outcome: {outcome:?}"),
        },
        Err(recovery) => ApplyError::RecoveryAfterFailure {
            operation: operation.to_string(),
            recovery: recovery.to_string(),
        },
    }
}

async fn write_new_journal(root: &Path, journal: &TransactionJournal) -> Result<(), ApplyError> {
    let current = root.join(JOURNAL_FILE_NAME);
    let next = root.join(JOURNAL_NEXT_FILE_NAME);
    require_absent(&current).await?;
    require_absent(&next).await?;
    write_journal_file(&next, journal).await?;
    tokio::fs::rename(next, current).await?;
    sync_directory(root).await
}

async fn replace_journal(root: &Path, journal: &TransactionJournal) -> Result<(), ApplyError> {
    let current = root.join(JOURNAL_FILE_NAME);
    let next = root.join(JOURNAL_NEXT_FILE_NAME);
    require_regular_file(&current).await?;
    require_absent(&next).await?;
    write_journal_file(&next, journal).await?;
    tokio::fs::rename(next, current).await?;
    sync_directory(root).await
}

async fn write_journal_file(path: &Path, journal: &TransactionJournal) -> Result<(), ApplyError> {
    validate_journal(journal)?;
    let mut bytes = serde_json::to_vec(journal)?;
    bytes.push(b'\n');
    if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_JOURNAL_BYTES) {
        return Err(ApplyError::JournalTooLarge);
    }
    write_fsynced(path, &bytes).await?;
    Ok(())
}

async fn read_journal(root: &Path) -> Result<Option<TransactionJournal>, ApplyError> {
    let path = root.join(JOURNAL_FILE_NAME);
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_JOURNAL_BYTES {
        return Err(ApplyError::UnsafeJournal(path));
    }
    let file = tokio::fs::File::open(&path).await?;
    let mut bytes = Vec::new();
    file.take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .await?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_JOURNAL_BYTES) {
        return Err(ApplyError::JournalTooLarge);
    }
    let journal = serde_json::from_slice(&bytes)?;
    validate_journal(&journal)?;
    Ok(Some(journal))
}

fn validate_journal(journal: &TransactionJournal) -> Result<(), ApplyError> {
    if journal.version != JOURNAL_VERSION {
        return Err(ApplyError::JournalVersion(journal.version));
    }
    validate_plan_digest(&journal.plan_digest)?;
    let operation_count = journal
        .installs
        .len()
        .checked_add(journal.removals.len())
        .ok_or(ApplyError::TooManyJournalOperations)?;
    if operation_count > MAX_JOURNAL_OPERATIONS {
        return Err(ApplyError::TooManyJournalOperations);
    }

    let mut destinations = BTreeSet::new();
    for install in &journal.installs {
        require_non_reserved(&install.destination)?;
        if !destinations.insert(install.destination.as_path().to_owned()) {
            return Err(ApplyError::JournalState(format!(
                "duplicate install destination {}",
                install.destination
            )));
        }
    }
    for removal in &journal.removals {
        require_non_reserved(removal)?;
        if !destinations.insert(removal.as_path().to_owned()) {
            return Err(ApplyError::JournalState(format!(
                "duplicate or conflicting destination {removal}"
            )));
        }
    }
    Ok(())
}

fn validate_plan_digest(digest: &str) -> Result<(), ApplyError> {
    if digest.len() != 32
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ApplyError::InvalidPlanDigest(digest.to_owned()));
    }
    Ok(())
}

fn require_non_reserved(path: &SafeRelativePath) -> Result<(), ApplyError> {
    if matches!(
        path.as_path().to_str(),
        Some(JOURNAL_FILE_NAME | JOURNAL_NEXT_FILE_NAME)
    ) {
        return Err(ApplyError::ReservedDevicePath(path.as_path().to_owned()));
    }
    Ok(())
}

async fn recovery_destination(
    root: &Path,
    relative: &SafeRelativePath,
) -> Result<PathBuf, ApplyError> {
    let destination = relative.under(root);
    let parent = destination
        .parent()
        .ok_or_else(|| ApplyError::NoParent(destination.clone()))?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| ApplyError::UnsafeDevicePath(parent.to_owned()))?;
    let mut current = root.to_owned();
    for component in relative_parent.components() {
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(ApplyError::UnsafeDevicePath(current)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ApplyError::JournalConflict(current));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(destination)
}

async fn remove_abandoned_next_journal(root: &Path) -> Result<(), ApplyError> {
    let path = root.join(JOURNAL_NEXT_FILE_NAME);
    if remove_regular_file_if_exists(&path).await? {
        sync_directory(root).await?;
    }
    Ok(())
}

async fn remove_regular_file_if_exists(path: &Path) -> Result<bool, ApplyError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_file() => {
            tokio::fs::remove_file(path).await?;
            Ok(true)
        }
        Ok(_) => Err(ApplyError::UnsafeDevicePath(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

async fn require_regular_file(path: &Path) -> Result<(), ApplyError> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_file() {
        return Err(ApplyError::UnsafeJournal(path.to_owned()));
    }
    Ok(())
}

async fn sync_transaction_directories(
    root: &Path,
    transaction: &PreparedTransaction,
) -> Result<(), ApplyError> {
    let paths = transaction
        .pending
        .iter()
        .flat_map(|item| {
            [&item.destination, &item.partial]
                .into_iter()
                .chain(item.backup.iter())
        })
        .chain(
            transaction
                .removals
                .iter()
                .flat_map(|item| [&item.destination, &item.parked]),
        );
    sync_paths(paths.map(PathBuf::as_path)).await?;
    sync_directory(root).await
}

async fn sync_journal_directories(
    root: &Path,
    journal: &TransactionJournal,
) -> Result<(), ApplyError> {
    let paths = journal
        .installs
        .iter()
        .map(|item| item.destination.under(root))
        .chain(journal.removals.iter().map(|item| item.under(root)))
        .collect::<Vec<_>>();
    sync_paths(paths.iter().map(PathBuf::as_path)).await?;
    sync_directory(root).await
}

async fn sync_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Result<(), ApplyError> {
    let directories = paths
        .into_iter()
        .filter_map(Path::parent)
        .map(Path::to_owned)
        .collect::<BTreeSet<_>>();
    for directory in directories {
        sync_directory(&directory).await?;
    }
    Ok(())
}

async fn sync_directory(path: &Path) -> Result<(), ApplyError> {
    require_real_directory(path).await?;
    tokio::fs::File::open(path).await?.sync_all().await?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("device update cancelled")]
    Cancelled,
    #[error("skipping recovery files is unsupported by the mass-storage transaction engine")]
    BackupPolicyUnsupported,
    #[error("staged file count mismatch: expected {expected}, received {actual}")]
    IncompleteStage { expected: usize, actual: usize },
    #[error("device path has no parent: {0}")]
    NoParent(PathBuf),
    #[error("unsafe symbolic link, directory, or special file in device path: {0}")]
    UnsafeDevicePath(PathBuf),
    #[error("update sidecar already exists: {0}")]
    SidecarExists(PathBuf),
    #[error("update plan digest is not a lowercase 128-bit hexadecimal value: {0}")]
    InvalidPlanDigest(String),
    #[error("update targets reserved transaction-journal path: {0}")]
    ReservedDevicePath(PathBuf),
    #[error("transaction journal is not a safe regular file: {0}")]
    UnsafeJournal(PathBuf),
    #[error("transaction journal version {0} is unsupported")]
    JournalVersion(u8),
    #[error("transaction journal exceeds its size limit")]
    JournalTooLarge,
    #[error("transaction contains too many journal operations")]
    TooManyJournalOperations,
    #[error("transaction journal is invalid: {0}")]
    JournalState(String),
    #[error("device state conflicts with the transaction journal at {0}")]
    JournalConflict(PathBuf),
    #[error("operation failed ({operation}); automatic recovery also failed ({recovery})")]
    RecoveryAfterFailure { operation: String, recovery: String },
    #[error("device destination has no Unicode file name: {0}")]
    InvalidFileName(PathBuf),
    #[error("multiple update files target the same destination: {0}")]
    DuplicateDestination(PathBuf),
    #[error("staged update file is not a regular file: {0}")]
    UnsafeStagedFile(PathBuf),
    #[error("staged file size mismatch for {path}: expected {expected}, received {actual}")]
    StagedSize {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error("staged file size exceeds the supported range")]
    StagedSizeOverflow,
    #[error("staged file checksum mismatch: expected {expected}, received {actual}")]
    StagedChecksum { expected: String, actual: String },
    #[error("device has {available} bytes free but staging requires {required}")]
    InsufficientSpace { required: u64, available: u64 },
    #[error("required device staging space exceeds the supported range")]
    RequiredSpaceOverflow,
    #[error("embedded unlock patching is not supported")]
    EmbeddedUnlocksUnsupported,
    #[error("signed storage data is not supported")]
    SignedStorageDataUnsupported,
    #[error("device I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid unlock path: {0}")]
    UnlockPath(#[from] PathSafetyError),
    #[error("invalid unlock payload: {0}")]
    UnlockPayload(#[from] base64::DecodeError),
    #[error("transaction journal JSON is invalid: {0}")]
    JournalJson(#[from] serde_json::Error),
    #[error("session capture failed: {0}")]
    Capture(#[from] garmin_capture::CaptureError),
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use garmin_device::SafeRelativePath;
    use std::os::unix::fs::symlink;

    const PLAN_DIGEST: &str = "0123456789abcdef0123456789abcdef";

    #[tokio::test]
    async fn refuses_a_symlinked_device_directory() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        let outside = fixture.path().join("outside");
        let staged_path = fixture.path().join("staged.img");
        tokio::fs::create_dir(&device).await.unwrap();
        tokio::fs::create_dir(&outside).await.unwrap();
        tokio::fs::write(&staged_path, b"map").await.unwrap();
        symlink(&outside, device.join("Garmin")).unwrap();
        let plan = UpdatePlan {
            schema_version: crate::UPDATE_PLAN_SCHEMA_VERSION,
            device_digest: "device".to_owned(),
            downloads: vec![crate::DownloadSpec {
                map_name: "Map".to_owned(),
                source: "https://download.garmin.com/map.img".parse().unwrap(),
                alternate_sources: Vec::new(),
                requires_garmin_token: false,
                destination: SafeRelativePath::parse("Garmin/map.img").unwrap(),
                cache_name: "00".repeat(16),
                size: 3,
                md5: "00".repeat(16),
            }],
            files_to_remove: Vec::new(),
            identifiers: Vec::new(),
            total_bytes: 3,
            backup_policy: crate::BackupPolicy::Verified,
            digest: PLAN_DIGEST.to_owned(),
        };
        let staged = [DownloadProgress {
            path: staged_path,
            bytes: 3,
            elapsed: Duration::ZERO,
            resumed_at: 0,
            from_cache: false,
        }];

        let result =
            apply_mass_storage(&plan, &staged, &MapAuthorization::default(), &device).await;

        assert!(matches!(result, Err(ApplyError::UnsafeDevicePath(_))));
        assert!(!outside.join("map.img").exists());
    }

    #[tokio::test]
    async fn rechecks_staged_content_before_device_mutation() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        let staged_path = fixture.path().join("staged.img");
        tokio::fs::create_dir(&device).await.unwrap();
        tokio::fs::create_dir(device.join("Garmin")).await.unwrap();
        tokio::fs::write(&staged_path, b"bad").await.unwrap();
        let plan = UpdatePlan {
            schema_version: crate::UPDATE_PLAN_SCHEMA_VERSION,
            device_digest: "device".to_owned(),
            downloads: vec![crate::DownloadSpec {
                map_name: "Map".to_owned(),
                source: "https://download.garmin.com/map.img".parse().unwrap(),
                alternate_sources: Vec::new(),
                requires_garmin_token: false,
                destination: SafeRelativePath::parse("Garmin/map.img").unwrap(),
                cache_name: "00".repeat(16),
                size: 3,
                md5: hex::encode(Md5::digest(b"map")),
            }],
            files_to_remove: Vec::new(),
            identifiers: Vec::new(),
            total_bytes: 3,
            backup_policy: crate::BackupPolicy::Verified,
            digest: PLAN_DIGEST.to_owned(),
        };
        let staged = [DownloadProgress {
            path: staged_path,
            bytes: 3,
            elapsed: Duration::ZERO,
            resumed_at: 0,
            from_cache: false,
        }];

        let result =
            apply_mass_storage(&plan, &staged, &MapAuthorization::default(), &device).await;

        assert!(matches!(result, Err(ApplyError::StagedChecksum { .. })));
        assert!(!device.join("Garmin/map.img").exists());
        assert!(!device.join(JOURNAL_FILE_NAME).exists());
    }

    #[tokio::test]
    async fn prepared_journal_restores_replacements_additions_and_removals() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        let garmin = device.join("Garmin");
        tokio::fs::create_dir_all(&garmin).await.unwrap();
        let replaced = garmin.join("replaced.img");
        let added = garmin.join("added.img");
        let removed = garmin.join("removed.img");
        tokio::fs::write(&replaced, b"old map").await.unwrap();
        tokio::fs::write(&removed, b"old auxiliary map")
            .await
            .unwrap();
        let journal = journal(TransactionState::Prepared);
        write_new_journal(&device, &journal).await.unwrap();

        tokio::fs::rename(
            &replaced,
            sidecar_path(&replaced, "backup", PLAN_DIGEST).unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(&replaced, b"new map").await.unwrap();
        tokio::fs::write(&added, b"new auxiliary map")
            .await
            .unwrap();
        tokio::fs::rename(
            &removed,
            sidecar_path(&removed, "removed", PLAN_DIGEST).unwrap(),
        )
        .await
        .unwrap();

        let outcome = recover_mass_storage(&device).await.unwrap();

        assert_eq!(outcome, RecoveryOutcome::RolledBack);
        assert_eq!(tokio::fs::read(replaced).await.unwrap(), b"old map");
        assert!(!added.exists());
        assert_eq!(
            tokio::fs::read(removed).await.unwrap(),
            b"old auxiliary map"
        );
        assert!(!device.join(JOURNAL_FILE_NAME).exists());
    }

    #[tokio::test]
    async fn committed_journal_preserves_installs_and_finishes_cleanup() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        let garmin = device.join("Garmin");
        tokio::fs::create_dir_all(&garmin).await.unwrap();
        let replaced = garmin.join("replaced.img");
        let added = garmin.join("added.img");
        let removed = garmin.join("removed.img");
        tokio::fs::write(&replaced, b"new map").await.unwrap();
        tokio::fs::write(
            sidecar_path(&replaced, "backup", PLAN_DIGEST).unwrap(),
            b"old map",
        )
        .await
        .unwrap();
        tokio::fs::write(&added, b"new auxiliary map")
            .await
            .unwrap();
        tokio::fs::write(
            sidecar_path(&removed, "removed", PLAN_DIGEST).unwrap(),
            b"old auxiliary map",
        )
        .await
        .unwrap();
        write_new_journal(&device, &journal(TransactionState::Committed))
            .await
            .unwrap();

        let outcome = recover_mass_storage(&device).await.unwrap();

        assert_eq!(outcome, RecoveryOutcome::Finalized);
        assert_eq!(tokio::fs::read(&replaced).await.unwrap(), b"new map");
        assert_eq!(tokio::fs::read(added).await.unwrap(), b"new auxiliary map");
        assert!(!removed.exists());
        assert!(
            !sidecar_path(&replaced, "backup", PLAN_DIGEST)
                .unwrap()
                .exists()
        );
        assert!(
            !sidecar_path(&removed, "removed", PLAN_DIGEST)
                .unwrap()
                .exists()
        );
        assert!(!device.join(JOURNAL_FILE_NAME).exists());
    }

    #[tokio::test]
    async fn journal_deserialization_cannot_escape_the_device() {
        let fixture = tempfile::tempdir().unwrap();
        let device = fixture.path().join("device");
        tokio::fs::create_dir(&device).await.unwrap();
        let encoded = format!(
            r#"{{"version":1,"plan_digest":"{PLAN_DIGEST}","state":"prepared","installs":[{{"destination":"../outside","had_original":false}}],"removals":[]}}"#
        );
        tokio::fs::write(device.join(JOURNAL_FILE_NAME), encoded)
            .await
            .unwrap();

        let result = recover_mass_storage(&device).await;

        assert!(matches!(result, Err(ApplyError::JournalJson(_))));
        assert!(!fixture.path().join("outside").exists());
    }

    fn journal(state: TransactionState) -> TransactionJournal {
        TransactionJournal {
            version: JOURNAL_VERSION,
            plan_digest: PLAN_DIGEST.to_owned(),
            state,
            installs: vec![
                JournalInstall {
                    destination: SafeRelativePath::parse("Garmin/replaced.img").unwrap(),
                    had_original: true,
                },
                JournalInstall {
                    destination: SafeRelativePath::parse("Garmin/added.img").unwrap(),
                    had_original: false,
                },
            ],
            removals: vec![SafeRelativePath::parse("Garmin/removed.img").unwrap()],
        }
    }
}
