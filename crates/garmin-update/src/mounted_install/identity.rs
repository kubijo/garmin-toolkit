use std::path::Path;

use super::{
    JournalState, MountedInstallError, MountedTransactionJournal, OperationCheckpoint,
    PREPARED_JOURNAL, load_rollback_checkpoints, read_journal, recovery_file, validate_journal,
    validate_matching_journal,
};
use crate::PortableTransaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateTransactionMatch {
    Different,
    Exact,
    Legacy,
}

/// Match a host preparation against the active device transaction. Legacy preparations retain
/// ordered destinations, expected payload hashes and original file hashes.
/// # Errors
/// The preparation is malformed, unreadable, or belongs to another execution target.
pub async fn match_update_transaction(
    capture: &Path,
    active: &PortableTransaction,
    execution_target: Option<&str>,
) -> Result<UpdateTransactionMatch, MountedInstallError> {
    let prepared = read_journal(&recovery_file(capture, PREPARED_JOURNAL).await?).await?;
    validate_journal(&prepared, active.device_digest(), JournalState::Prepared)?;
    if execution_target != prepared.execution_target.as_deref() {
        return Err(MountedInstallError::RecoveryDeviceMismatch);
    }
    if prepared.matches_transaction(active)? {
        return Ok(if prepared.transaction_id.is_some() {
            UpdateTransactionMatch::Exact
        } else {
            UpdateTransactionMatch::Legacy
        });
    }
    if prepared.transaction_id == Some(active.transaction_id()) {
        return Err(MountedInstallError::InvalidJournal);
    }
    Ok(UpdateTransactionMatch::Different)
}

#[derive(PartialEq, Eq)]
struct History {
    journal: MountedTransactionJournal,
    checkpoints: Vec<OperationCheckpoint>,
    markers: Vec<JournalState>,
}

impl History {
    async fn read(capture: &Path) -> Result<Self, MountedInstallError> {
        let journal = read_journal(&recovery_file(capture, PREPARED_JOURNAL).await?).await?;
        let checkpoints = load_rollback_checkpoints(&journal, capture).await?;
        let mut markers = Vec::new();
        for (path, state) in [
            (
                "mounted-update/transaction/committed.json",
                JournalState::Committed,
            ),
            (
                "mounted-update/transaction/rollback-started.json",
                JournalState::Prepared,
            ),
            (
                "mounted-update/transaction/rolled-back.json",
                JournalState::RolledBack,
            ),
        ] {
            match tokio::fs::symlink_metadata(capture.join(path)).await {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                result => {
                    result?;
                }
            }
            let candidate = read_journal(&recovery_file(capture, path).await?).await?;
            validate_matching_journal(&journal, &candidate, state)?;
            markers.push(state);
        }
        Ok(Self {
            journal,
            checkpoints,
            markers,
        })
    }
}

/// Check whether multiple matching captures describe identical preparation and recovery actions.
/// # Errors
/// Their durable checkpoints or journals cannot be validated.
pub async fn same_recovery_history(
    first: &Path,
    second: &Path,
) -> Result<bool, MountedInstallError> {
    Ok(History::read(first).await? == History::read(second).await?)
}
