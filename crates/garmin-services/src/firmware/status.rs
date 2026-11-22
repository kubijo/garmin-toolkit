use std::path::Path;

use crate::maps::device::Connection;
use anyhow::{Context as _, Result, ensure};
use garmin_model::artifact::ByteCount;
use garmin_model::value::Timestamp;
use garmin_update::firmware::{Confirmation, FirmwareReceipt};
use garmin_update::{
    DeviceTransactionStore, MountedInstallError, PortableTransaction, UpdateTransactionMatch,
    match_update_transaction, update_is_committed,
};
use serde::Serialize;
use tokio::io::AsyncReadExt as _;

use crate::maps::pending_recovery::{PendingRecovery, PendingRecoveryStore};
use uuid::Uuid;

#[derive(Serialize)]
pub struct ComponentStatus {
    pub part_number: String,
    pub name: String,
    pub offered: String,
    pub status: Confirmation,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TransferStatus {
    RecoveryRequired,
    VersionsUnverified,
    Unavailable { reason: String },
    Confirmed { components: Vec<ComponentStatus> },
    AwaitingConfirmation { components: Vec<ComponentStatus> },
}

#[derive(Serialize)]
pub struct TransferReport {
    pub receipt: Uuid,
    pub registered_at: Timestamp,
    #[serde(flatten)]
    pub status: TransferStatus,
}

/// Inspect a device's retained firmware transfers and optionally dismiss completed journals.
/// # Errors
/// A receipt is unreadable, belongs to a different device, or still needs recovery.
pub async fn status(
    connection: &Connection,
    registry: &PendingRecoveryStore,
    acknowledge: bool,
) -> Result<Vec<TransferStatus>> {
    Ok(review_status(connection, registry, None, |_, count| {
        ensure!(
            !acknowledge || count == 1,
            "select one firmware receipt to dismiss"
        );
        Ok(acknowledge)
    })
    .await?
    .into_iter()
    .map(|report| report.status)
    .collect())
}

/// Hold the receipt lock through inspection and the user's dismissal decision.
/// # Errors
/// The device, journal or review could not be read, or the transfer still needs recovery.
pub async fn review_status(
    connection: &Connection,
    registry: &PendingRecoveryStore,
    selected: Option<Uuid>,
    mut dismiss: impl FnMut(&TransferReport, usize) -> Result<bool>,
) -> Result<Vec<TransferReport>> {
    let manifest = &connection.manifest;
    let digest = manifest.identity_digest();
    let _slot = registry.lock_device(&digest)?;
    let pending = registry.for_device(&digest)?;
    if pending.is_empty() {
        ensure!(
            selected.is_none(),
            "the reviewed firmware receipt is no longer registered; inspect the transfer again"
        );
        return Ok(Vec::new());
    }
    let active = DeviceTransactionStore::open(connection.device.as_ref())
        .await?
        .active_prepared(&digest)
        .await?;
    let mut reports = Vec::new();
    let mut clear = Vec::new();
    let count = if selected.is_some() { 1 } else { pending.len() };
    for (transfer, matched) in classify_captures(
        pending,
        active.as_ref(),
        connection.device.execution_target(),
    )
    .await
    {
        let id = transfer.id()?;
        if selected.is_some_and(|selection| selection != id) {
            continue;
        }
        let status = inspect_transfer(connection, &transfer, matched)
            .await
            .unwrap_or_else(|error| TransferStatus::Unavailable {
                reason: format!("{error:#}"),
            });
        let report = TransferReport {
            receipt: id,
            registered_at: transfer.registered_at(),
            status,
        };
        if dismiss(&report, count)? {
            match &report.status {
                TransferStatus::RecoveryRequired => {
                    anyhow::bail!("use firmware recover for an interrupted transfer")
                }
                TransferStatus::Unavailable { reason } => anyhow::bail!("{reason}"),
                _ => clear.push(transfer),
            }
        }
        reports.push(report);
    }
    ensure!(
        selected.is_none() || !reports.is_empty(),
        "the reviewed firmware receipt is no longer registered; inspect the transfer again"
    );
    for transfer in &clear {
        registry.clear(transfer)?;
    }
    Ok(reports)
}

pub(super) async fn classify_captures(
    entries: Vec<PendingRecovery>,
    active: Option<&PortableTransaction>,
    target: Option<&str>,
) -> Vec<(
    PendingRecovery,
    Result<UpdateTransactionMatch, MountedInstallError>,
)> {
    let mut classified = Vec::with_capacity(entries.len());
    let mut exact = false;
    for entry in entries {
        let matched = match active {
            Some(active) if active.plan_digest() == entry.plan_digest => {
                match_update_transaction(&entry.transaction, active, target).await
            }
            _ => Ok(UpdateTransactionMatch::Different),
        };
        exact |= matches!(matched, Ok(UpdateTransactionMatch::Exact));
        classified.push((entry, matched));
    }
    if exact {
        for (_, matched) in &mut classified {
            if matches!(matched, Ok(UpdateTransactionMatch::Legacy)) {
                *matched = Ok(UpdateTransactionMatch::Different);
            }
        }
    }
    classified
}

pub(super) fn find_receipt(entries: Vec<PendingRecovery>, id: Uuid) -> Result<PendingRecovery> {
    for entry in entries {
        if entry.id()? == id {
            return Ok(entry);
        }
    }
    anyhow::bail!(
        "the reviewed firmware receipt is no longer registered; inspect the transfer again"
    )
}

async fn inspect_transfer(
    connection: &Connection,
    transfer: &PendingRecovery,
    matched: Result<UpdateTransactionMatch, MountedInstallError>,
) -> Result<TransferStatus> {
    let manifest = &connection.manifest;
    let needs_recovery = !transfer.is_prepared()? || matched? != UpdateTransactionMatch::Different;
    if needs_recovery
        && !transfer
            .transaction
            .join("firmware/selection.json")
            .try_exists()?
    {
        return Ok(TransferStatus::RecoveryRequired);
    }
    let receipt = read_selection(&transfer.transaction).await?;
    let bound = receipt.is_bound_to(manifest, &transfer.plan_digest)?;
    if needs_recovery {
        return Ok(TransferStatus::RecoveryRequired);
    }
    if !update_is_committed(
        &transfer.transaction,
        &manifest.identity_digest(),
        &transfer.plan_digest,
        connection.device.execution_target(),
    )
    .await
    .with_context(|| {
        format!(
            "restore the firmware journals at {}",
            transfer.transaction.display()
        )
    })? {
        return Ok(TransferStatus::RecoveryRequired);
    }
    if !bound {
        return Ok(TransferStatus::VersionsUnverified);
    }
    let components: Vec<_> = receipt
        .confirm(manifest)?
        .context("selected device differs from the firmware receipt")?
        .into_iter()
        .map(|item| ComponentStatus {
            part_number: item.part_number.to_owned(),
            name: item.name.to_owned(),
            offered: item.offered.to_owned(),
            status: item.status,
        })
        .collect();
    ensure!(
        !components.is_empty(),
        "firmware receipt contains no components"
    );
    Ok(
        if components
            .iter()
            .all(|c| c.status == Confirmation::Confirmed)
        {
            TransferStatus::Confirmed { components }
        } else {
            TransferStatus::AwaitingConfirmation { components }
        },
    )
}

pub(super) async fn read_selection(capture: &Path) -> Result<FirmwareReceipt> {
    let limit = ByteCount::from_mebibytes(1);
    let file = tokio::fs::File::open(capture.join("firmware/selection.json")).await?;
    let mut bytes = Vec::new();
    file.take(limit.as_bytes() + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        ByteCount::from_bytes(bytes.len() as u64) <= limit,
        "firmware receipt is too large"
    );
    Ok(serde_json::from_slice(&bytes)?)
}
