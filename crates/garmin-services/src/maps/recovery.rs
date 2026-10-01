//! Reconcile durable host receipts with the device's portable transaction.

use anyhow::{Context as _, Result, bail};
use garmin_device::storage::DeviceWrite;
use garmin_progress::ProgressReporter;
use garmin_update::{DeviceTransactionKind, DeviceTransactionStore, PortableTransaction};

use super::pending_recovery::{PendingRecovery, PendingRecoveryKind, PendingRecoveryStore};

pub struct PendingState {
    pub kind: PendingRecoveryKind,
    pub plan: String,
    pub receipt: Option<PendingRecovery>,
    pub transaction: Option<PortableTransaction>,
    pub prepared: bool,
    pub simulated: bool,
    device: String,
}

impl PendingState {
    /// Inspect both sources of evidence without choosing or executing recovery.
    ///
    /// # Errors
    /// Device state or host evidence is unreadable or unsafe.
    pub async fn inspect(
        device: &dyn DeviceWrite,
        identity: &str,
        receipts: &PendingRecoveryStore,
    ) -> Result<Option<Self>> {
        let transaction = DeviceTransactionStore::open(device)
            .await?
            .active(identity)
            .await?;
        let active = transaction
            .as_ref()
            .map(|value| (kind(value.kind()), value.plan_digest()));
        let receipt = receipts.for_selected_device(identity, active)?;
        let prepared = receipt
            .as_ref()
            .map(PendingRecovery::is_prepared)
            .transpose()?
            .unwrap_or(false);
        let (kind, plan) = match (&transaction, &receipt) {
            (Some(value), _) => (kind(value.kind()), value.plan_digest().to_owned()),
            (None, Some(value)) => (value.kind, value.plan_digest.clone()),
            (None, None) => return Ok(None),
        };
        let simulated = receipt
            .as_ref()
            .map(PendingRecovery::is_simulated)
            .transpose()?
            .unwrap_or(false);
        let mut pending = Self {
            kind,
            plan,
            receipt,
            transaction,
            prepared,
            simulated,
            device: identity.to_owned(),
        };
        if prepared && let Some(shadow) = pending.shadow().await? {
            pending.transaction = DeviceTransactionStore::open(shadow.device())
                .await?
                .active(identity)
                .await?;
        }
        Ok(Some(pending))
    }

    async fn shadow(&self) -> Result<Option<garmin_simulator::shadow::Shadow>> {
        if !self.simulated {
            return Ok(None);
        }
        let receipt = self
            .receipt
            .as_ref()
            .context("simulation recovery requires its original host receipt")?;
        let shadow = garmin_simulator::shadow::Shadow::reopen(&receipt.transaction).await?;
        if !shadow.matches_source(&self.device, &self.plan) {
            bail!("simulation snapshot belongs to another device or plan");
        }
        Ok(Some(shadow))
    }

    /// Restore/finalize the original transaction using its retained host evidence.
    ///
    /// # Errors
    /// Original evidence is missing, invalid, or recovery could not complete.
    pub async fn recover(
        &self,
        identity: &str,
        device: &dyn DeviceWrite,
        progress: &ProgressReporter,
        receipts: &PendingRecoveryStore,
    ) -> Result<()> {
        let receipt = self
            .receipt
            .as_ref()
            .context("this host has no retained payload for the device's active transaction")?;
        if !self.prepared {
            bail!("interrupted preparation must be discarded, not recovered");
        }
        let shadow = self.shadow().await?;
        let device = shadow
            .as_ref()
            .map_or(device, garmin_simulator::shadow::Shadow::device);
        match self.kind {
            PendingRecoveryKind::Update => {
                garmin_update::recover_mounted_mtp_update(
                    &receipt.transaction,
                    identity,
                    device,
                    progress,
                )
                .await?;
            }
            PendingRecoveryKind::Removal => {
                garmin_update::recover_removal(&receipt.transaction, identity, progress, device)
                    .await?;
            }
        }
        receipts.clear_completed(receipt);
        Ok(())
    }

    /// Clear device state only after the engine
    /// proves the transaction's outcome.
    ///
    /// # Errors
    /// No portable transaction, proof failed,
    /// or evidence could not be cleared.
    pub async fn prove_and_clear(
        &self,
        device: &dyn DeviceWrite,
        receipts: &PendingRecoveryStore,
    ) -> Result<()> {
        let shadow = self.shadow().await?;
        let device = shadow
            .as_ref()
            .map_or(device, garmin_simulator::shadow::Shadow::device);
        let transaction = self
            .transaction
            .as_ref()
            .context("host-only recovery receipts cannot clear device state")?;
        DeviceTransactionStore::open(device)
            .await?
            .prove_and_clear(transaction)
            .await?;
        if let Some(receipt) = &self.receipt {
            receipts.clear(receipt)?;
        }
        Ok(())
    }

    /// Discard a host preparation only when neither
    /// source records a prepared transaction.
    ///
    /// # Errors
    /// Prepared transaction or unsafe host evidence.
    pub fn discard(&self, receipts: &PendingRecoveryStore) -> Result<()> {
        if self.transaction.is_some() {
            bail!("device transaction must be recovered or proved");
        }
        receipts.discard_unprepared(
            self.receipt
                .as_ref()
                .context("no interrupted preparation is retained")?,
        )
    }
}

const fn kind(value: DeviceTransactionKind) -> PendingRecoveryKind {
    match value {
        DeviceTransactionKind::Update => PendingRecoveryKind::Update,
        DeviceTransactionKind::Removal => PendingRecoveryKind::Removal,
    }
}
