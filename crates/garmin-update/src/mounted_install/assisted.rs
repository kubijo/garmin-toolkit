//! Explicitly approved preservation of empty, journal-bound interrupted uploads.

use std::path::{Path, PathBuf};

use garmin_device::{
    DevicePathState, MountedMtpBackupProgress, SafeRelativePath, storage::DeviceWrite,
};
use garmin_progress::{OperationStage, ProgressReporter};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use super::{
    JournalState, MountedInstallError, MountedTransactionJournal, MountedUpdateRecoveryReport,
    OperationCheckpoint, inspect_recovery_original, inspect_recovery_write,
    load_rollback_checkpoints, read_journal, recover_mounted_mtp_update, recovery_file,
    require_running, validate_journal, verify_local_payload,
};
use crate::host::{HostFilesystem as _, SystemFilesystem};
use crate::{BackupPolicy, DeviceTransactionKind, DeviceTransactionStore, PortableTransaction};

/// An empty object that ordinary recovery cannot match to a payload prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EmptyUpload {
    pub storage_id: String,
    pub storage_label: String,
    pub path: SafeRelativePath,
}

/// Host-owned review evidence. Clients cannot manufacture or edit this plan.
#[derive(Clone, Debug)]
pub struct AssistedRecoveryPlan {
    approval: uuid::Uuid,
    journal: MountedTransactionJournal,
    transaction: PortableTransaction,
    root: PathBuf,
    files: Vec<EmptyUpload>,
}

/// Completed rollback and the host directory retaining the approved files.
#[derive(Debug, Clone)]
pub struct AssistedRecoveryReport {
    pub recovery: MountedUpdateRecoveryReport,
    pub quarantine: PathBuf,
}

impl AssistedRecoveryPlan {
    #[must_use]
    pub const fn approval(&self) -> uuid::Uuid {
        self.approval
    }

    #[must_use]
    pub fn files(&self) -> &[EmptyUpload] {
        &self.files
    }
}

/// Review empty uploads without modifying the device.
/// Only started, unapplied writes with verified host backups are eligible.
/// # Errors
/// Invalid identity, journals, payloads, backups, or other ambiguous device objects.
pub async fn review_assisted_recovery<D: DeviceWrite + ?Sized>(
    root: &Path,
    identity: &str,
    device: &D,
    progress: &ProgressReporter,
) -> Result<Option<AssistedRecoveryPlan>, MountedInstallError> {
    let (journal, transaction) = review_journal(root, identity, device).await?;
    let canonical_root = tokio::fs::canonicalize(root).await?;
    let checkpoints = load_rollback_checkpoints(&journal, root).await?;
    verify_backups(&journal, root).await?;
    let mut files = Vec::new();
    for (index, write) in journal.writes.iter().enumerate() {
        require_running(progress)?;
        let observed = device
            .inspect_with_progress(&write.target.storage_id, &write.target.path, progress)
            .await?;
        let empty = observed.into_parts() == (DevicePathState::RegularFile, Some(0));
        if empty && write.target.size > 0 && checkpoints[index] == OperationCheckpoint::Started {
            let payload = recovery_file(
                root,
                write
                    .payload_file
                    .as_deref()
                    .ok_or(MountedInstallError::InvalidJournal)?,
            )
            .await?;
            if verify_local_payload(&payload, write.target.size, None).await? != write.sha256 {
                return Err(MountedInstallError::RecoveryEvidence(
                    write.target.path.clone(),
                ));
            }
            files.push(EmptyUpload {
                storage_id: write.target.storage_id.clone(),
                storage_label: write.target.storage_label.clone(),
                path: write.target.path.clone(),
            });
        } else {
            inspect_recovery_write(device, write, root, index + 1, checkpoints[index], progress)
                .await?;
        }
    }
    for (index, original) in journal.removals.iter().enumerate() {
        inspect_recovery_original(
            device,
            original,
            root,
            checkpoints[journal.writes.len() + index],
            progress,
        )
        .await?;
    }
    Ok((!files.is_empty()).then(|| AssistedRecoveryPlan {
        approval: uuid::Uuid::new_v4(),
        journal,
        transaction,
        root: canonical_root,
        files,
    }))
}

async fn review_journal<D: DeviceWrite + ?Sized>(
    root: &Path,
    identity: &str,
    device: &D,
) -> Result<(MountedTransactionJournal, PortableTransaction), MountedInstallError> {
    let path = recovery_file(root, "mounted-update/transaction/000000-prepared.json").await?;
    let journal = read_journal(&path).await?;
    validate_journal(&journal, identity, JournalState::Prepared)?;
    if journal.execution_target.as_deref() != device.execution_target() {
        return Err(MountedInstallError::RecoveryDeviceMismatch);
    }
    if journal.backup_policy != BackupPolicy::Verified {
        return Err(MountedInstallError::RecoveryUnavailable);
    }
    for name in ["committed.json", "rolled-back.json"] {
        if tokio::fs::try_exists(root.join("mounted-update/transaction").join(name)).await? {
            return Err(MountedInstallError::InvalidJournal);
        }
    }
    let store = DeviceTransactionStore::open(device).await?;
    let transaction = store
        .active(identity)
        .await?
        .ok_or(MountedInstallError::InvalidJournal)?;
    if transaction.kind() != DeviceTransactionKind::Update
        || transaction.plan_digest() != journal.plan_digest
    {
        return Err(MountedInstallError::RecoveryPlanMismatch);
    }
    Ok((journal, transaction))
}

async fn verify_backups(
    journal: &MountedTransactionJournal,
    root: &Path,
) -> Result<(), MountedInstallError> {
    for original in journal
        .writes
        .iter()
        .filter_map(|write| write.original.as_ref())
        .chain(&journal.removals)
    {
        let backup = recovery_file(root, &original.backup_file).await?;
        if verify_local_payload(&backup, original.target.size, None).await? != original.sha256 {
            return Err(MountedInstallError::RecoveryEvidence(
                original.target.path.clone(),
            ));
        }
    }
    Ok(())
}

/// Preserve the explicitly reviewed empty objects, then run ordinary verified rollback.
/// The entire review is revalidated before quarantine or deletion. Quarantines survive
/// cancellation and crashes; normal recovery remains available after removal.
/// # Errors
/// Stale review, changed identity/evidence/files, cancellation, or failed preservation/rollback.
pub async fn approve_assisted_recovery<D: DeviceWrite + ?Sized>(
    root: &Path,
    identity: &str,
    device: &D,
    reviewed: &AssistedRecoveryPlan,
    progress: &ProgressReporter,
) -> Result<AssistedRecoveryReport, MountedInstallError> {
    let fresh = review_assisted_recovery(root, identity, device, progress)
        .await?
        .ok_or(MountedInstallError::InvalidJournal)?;
    if fresh.journal != reviewed.journal
        || fresh.files != reviewed.files
        || fresh.transaction != reviewed.transaction
        || fresh.root != reviewed.root
    {
        return Err(MountedInstallError::RecoveryPlanMismatch);
    }
    let quarantine = preserve_empty_uploads(root, device, reviewed, progress).await?;
    for file in &reviewed.files {
        require_running(progress)?;
        device
            .delete_size_checked_with_progress(&file.storage_id, &file.path, 0, progress)
            .await?;
    }
    progress.completed(
        OperationStage::Cleanup,
        format!("Empty uploads preserved in {}", quarantine.display()),
        u64::try_from(reviewed.files.len()).map_err(|_| MountedInstallError::ByteCountOverflow)?,
        None,
    );
    let recovery = recover_mounted_mtp_update(root, identity, device, progress).await?;
    Ok(AssistedRecoveryReport {
        recovery,
        quarantine,
    })
}

async fn preserve_empty_uploads<D: DeviceWrite + ?Sized>(
    root: &Path,
    device: &D,
    reviewed: &AssistedRecoveryPlan,
    progress: &ProgressReporter,
) -> Result<PathBuf, MountedInstallError> {
    // A fresh directory prevents collisions or symlink redirection by older evidence.
    // Keep it immediately: even a failed readback remains available for inspection.
    let directory = tempfile::Builder::new()
        .prefix("assisted-recovery-")
        .tempdir_in(root)?
        .keep();
    for (index, file) in reviewed.files.iter().enumerate() {
        require_running(progress)?;
        let path = directory.join(format!("{index:06}.bin"));
        let output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let sha256 = device
            .backup(
                &file.storage_id,
                &file.path,
                0,
                garmin_device::BackupDestination::new(path.clone(), output)?,
                MountedMtpBackupProgress {
                    reporter: progress.clone(),
                    completed_before: 0,
                    total: 0,
                },
            )
            .await?;
        if sha256 != hex::encode(Sha256::digest([])) || tokio::fs::metadata(&path).await?.len() != 0
        {
            return Err(MountedInstallError::RecoveryEvidence(file.path.clone()));
        }
        tokio::fs::File::open(path).await?.sync_all().await?;
    }
    let manifest = serde_json::to_vec_pretty(&serde_json::json!({
        "version": 1, "approval": reviewed.approval, "device": reviewed.journal.device_digest,
        "plan": reviewed.journal.plan_digest, "files": reviewed.files,
    }))?;
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("approval.json"))
        .await?;
    output.write_all(&manifest).await?;
    output.sync_all().await?;
    SystemFilesystem.sync_directory(&directory)?;
    SystemFilesystem.sync_directory(root)?;
    Ok(directory)
}
