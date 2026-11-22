use std::{
    path::{Path, PathBuf},
    sync::PoisonError,
};

use anyhow::{Context as _, Result, ensure};
use garmin_capture::SessionCapture;
use garmin_progress::{
    ProgressAmount, ProgressEvent, ProgressReporter, ProgressState, model::ProgressModel,
};
use garmin_service_api::firmware::{
    Action, Inspection, InstalledStatus, Observation, Phase, ReceiptStatus, RetainedTransfer,
    State, TransferProgress, WRITE_STAGES,
};
use garmin_update::{
    AssistedRecoveryPlan, DeviceTransactionStore, MountedUpdateRecoveryOutcome,
    MountedUpdateRecoveryReport, approve_assisted_recovery, firmware::Confirmation,
    review_assisted_recovery,
};
use uuid::Uuid;

use super::{Control, Session, TransferReport, TransferStatus, review_status};
use crate::maps::{
    device::Connection,
    pending_recovery::{PendingRecovery, PendingRecoveryStore},
};

impl Control {
    fn begin_write(&mut self) -> Phase {
        let model = ProgressModel::new(WRITE_STAGES, "Waiting");
        self.state.transcript = model.transcript();
        self.progress = Some(model);
        self.last_progress_publish = None;
        Phase::Staging(TransferProgress {
            label: String::new(),
            completed: None,
            total: None,
        })
    }

    fn open_inspection(&mut self) {
        if let Some(inspection) = &mut self.state.inspection {
            inspection.from_list = false;
        } else {
            self.state.inspection = Some(Inspection {
                background: self.state.phase.clone(),
                from_list: false,
            });
        }
    }

    fn close_inspection(&mut self) -> Result<()> {
        let inspection = self
            .state
            .inspection
            .take()
            .context("no transfer receipt is open")?;
        self.state.phase = inspection.background;
        self.inspected_receipt = None;
        self.recovery_identity = None;
        self.recovery_review = None;
        Ok(())
    }
}

impl Session {
    /// Execute a revision-bound choice on the device-owning host.
    /// # Errors
    /// The revision, approval, or action is unavailable in the current state.
    pub fn submit(&self, revision: Uuid, action: Action) -> Result<State> {
        match action {
            Action::Check => return self.start_check(revision),
            Action::Review(parts) => return self.review_selection(revision, parts),
            _ => {}
        }
        let root = self
            .0
            .deployment
            .as_ref()
            .context("firmware host has no deployment")?
            .0
            .root()
            .join("firmware");
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ensure!(
            control.state.revision == revision,
            "firmware catalog changed"
        );
        ensure!(
            !matches!(
                control.state.phase,
                Phase::Checking | Phase::Staging(_) | Phase::Recovering | Phase::Disconnected
            ),
            "firmware operation already running"
        );
        let phase = match &action {
            Action::EditSelection
                if matches!(control.state.phase, Phase::Available(_))
                    && control.state.inspection.is_none()
                    && control.state.review.is_some() =>
            {
                control.state.review = None;
                return Ok(self.publish(&mut control));
            }
            Action::CloseInspection => {
                control.close_inspection()?;
                return Ok(self.publish(&mut control));
            }
            Action::Stage(token) => {
                ensure!(
                    matches!(control.state.phase, Phase::Available(_))
                        && control
                            .state
                            .review
                            .as_ref()
                            .is_some_and(|r| r.approval == *token),
                    "firmware approval expired"
                );
                control.begin_write()
            }
            Action::Inspect => {
                control.open_inspection();
                Phase::Checking
            }
            Action::InspectReceipt(id) if matches!(&control.state.phase, Phase::Transfers(entries) if entries.iter().any(|entry| entry.receipt == *id)) =>
            {
                control
                    .state
                    .inspection
                    .as_mut()
                    .context("no transfer receipt is open")?
                    .from_list = true;
                Phase::Checking
            }
            Action::Recover if control.state.phase == Phase::RecoveryRequired => Phase::Recovering,
            Action::ApproveRecovery(token) if matches!(&control.state.phase, Phase::ReviewRecovery { approval, .. } if approval == token) => {
                Phase::Recovering
            }
            Action::Acknowledge
                if matches!(
                    control.state.phase,
                    Phase::Reconnected(_) | Phase::VersionsUnverified
                ) =>
            {
                Phase::Checking
            }
            _ => anyhow::bail!("unavailable firmware action"),
        };
        let identity = control.recovery_identity.clone();
        if matches!(action, Action::Recover | Action::ApproveRecovery(_)) {
            if let Some(inspection) = &mut control.state.inspection {
                inspection.background = Phase::Consent;
            }
            control.state.review = None;
            control.checked = None;
        }
        control.state.phase = phase.clone();
        let state = self.publish(&mut control);
        let revision = state.revision;
        let session = self.clone();
        tokio::spawn(session.run_action(root, revision, action, phase, identity));
        Ok(state)
    }

    async fn run_action(
        self,
        root: PathBuf,
        revision: Uuid,
        action: Action,
        phase: Phase,
        identity: Option<String>,
    ) {
        let writing = matches!(action, Action::Stage(_));
        let resolving = matches!(
            action,
            Action::Acknowledge | Action::Recover | Action::ApproveRecovery(_)
        );
        let result = self
            .work(root, revision, action, phase, identity.as_deref())
            .await;
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if control.state.phase == Phase::Disconnected {
            return;
        }
        if writing && let Some(model) = control.progress.take() {
            control.state.transcript = model.transcript();
        }
        match result {
            Ok((phase, identity)) => {
                control.state.has_retained_transfer = !matches!(phase, Phase::NoTransfer);
                if phase == Phase::NoTransfer
                    && let Some(inspection) = &mut control.state.inspection
                    && matches!(
                        inspection.background,
                        Phase::PreviousTransferPending | Phase::WriteFailed(_)
                    )
                {
                    inspection.background = Phase::Consent;
                }
                control.state.phase = if phase == Phase::NoTransfer && resolving {
                    control
                        .state
                        .inspection
                        .take()
                        .map_or(Phase::Consent, |i| i.background)
                } else {
                    phase
                };
                control.recovery_identity = Some(identity);
            }
            Err(error) if writing => control.state.phase = Phase::WriteFailed(error.to_string()),
            Err(error) => control.state.phase = Phase::Failed(error.to_string()),
        }
        self.publish(&mut control);
    }

    async fn work(
        &self,
        root: PathBuf,
        revision: Uuid,
        action: Action,
        phase: Phase,
        identity: Option<&str>,
    ) -> Result<(Phase, String)> {
        let registry = self.receipt_store(&root)?;
        if let Action::Stage(approval) = action {
            return self
                .stage_reviewed(revision, approval, phase, &root, &registry)
                .await;
        }
        let _lease = self.lease().await?;
        let _guard = self
            .0
            .mutations
            .acquire(&self.0.device)
            .map_err(anyhow::Error::msg)?;
        let connection = self.0.connector.connect().await?;
        let progress = self.recovery_progress()?;
        let device = connection.manifest.identity_digest();
        if action != Action::Inspect {
            ensure!(identity == Some(device.as_str()), "selected device changed");
        }
        let receipt = match action {
            Action::Inspect => None,
            Action::InspectReceipt(id) => Some(id),
            _ => Some(
                self.0
                    .control
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .inspected_receipt
                    .context("inspect the firmware receipt first")?,
            ),
        };
        if let Action::ApproveRecovery(token) = action {
            let review = self
                .0
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recovery_review
                .take()
                .filter(|r| r.approval() == token)
                .context("firmware recovery approval expired")?;
            approve_empty_uploads(&connection, &registry, receipt, &review, &progress).await?;
        } else if action == Action::Recover
            && let Err(error) = recover_registered(&connection, &registry, receipt, &progress).await
        {
            let Some(review) =
                review_empty_uploads(&connection, &registry, receipt, &progress).await?
            else {
                return Err(error);
            };
            let phase = Phase::ReviewRecovery {
                approval: review.approval(),
                files: review
                    .files()
                    .iter()
                    .map(|f| format!("{} · {}", f.storage_label, f.path))
                    .collect(),
            };
            self.0
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recovery_review = Some(review);
            return Ok((phase, device));
        }
        if action == Action::Acknowledge {
            review_status(&connection, &registry, receipt, |_, _| {
                ensure!(!progress.is_cancelled(), "device was disconnected");
                Ok(true)
            })
            .await?;
        }
        let selected = if matches!(action, Action::InspectReceipt(_)) {
            receipt
        } else {
            None
        };
        let reports = review_status(&connection, &registry, selected, |_, _| Ok(false)).await?;
        let (phase, reviewed) = inspection_phase(reports);
        self.0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .inspected_receipt = reviewed;
        Ok((phase, device))
    }

    async fn stage_reviewed(
        &self,
        revision: Uuid,
        approval: Uuid,
        phase: Phase,
        root: &Path,
        registry: &PendingRecoveryStore,
    ) -> Result<(Phase, String)> {
        let approved = self.claim(revision, approval, phase).await?;
        let device = approved.connection.manifest.identity_digest();
        let needs_restart = approved
            .offers
            .iter()
            .any(|o| o.installation.is_restart_required);
        let capture = SessionCapture::create(&root.join("captures").join(approval.to_string()))?;
        let session = self.clone();
        let progress = ProgressReporter::from_handler(move |event| session.progress(event));
        match approved
            .stage(&root.join("cache"), &capture, registry, progress)
            .await
        {
            Ok(_) => Ok((Phase::ReadyToEject { needs_restart }, device)),
            Err(error) if error.is::<super::PreviousTransferPending>() => {
                Ok((Phase::PreviousTransferPending, device))
            }
            Err(error) => {
                // A failed write may have registered a recovery receipt before it stopped.
                let retained = registry
                    .for_device(&device)
                    .map_or(true, |entries| !entries.is_empty());
                self.0
                    .control
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .state
                    .has_retained_transfer = retained;
                Err(error)
            }
        }
    }

    fn progress(&self, event: ProgressEvent) {
        let (completed, total) = match event.amount {
            ProgressAmount::Bytes { completed, total } => (Some(completed), total),
            ProgressAmount::Operations { .. } => (None, None),
        };
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        let throttle = event.state == ProgressState::Advanced
            && control.last_progress_publish.is_some_and(|last| {
                now.duration_since(last) < std::time::Duration::from_millis(100)
            });
        let Some(model) = &mut control.progress else {
            return;
        };
        model.apply(&event);
        if throttle {
            return;
        }
        control.state.transcript = model.transcript();
        control.last_progress_publish = Some(now);
        control.state.phase = Phase::Staging(TransferProgress {
            label: event.label,
            completed,
            total,
        });
        self.publish(&mut control);
    }
}

fn inspection_phase(mut reports: Vec<TransferReport>) -> (Phase, Option<Uuid>) {
    if reports.len() > 1 {
        let entries = reports
            .into_iter()
            .map(|r| RetainedTransfer {
                receipt: r.receipt,
                registered_at: r.registered_at,
                status: match r.status {
                    TransferStatus::RecoveryRequired => ReceiptStatus::RecoveryRequired,
                    TransferStatus::VersionsUnverified => ReceiptStatus::VersionsUnverified,
                    TransferStatus::Unavailable { .. } => ReceiptStatus::Unavailable,
                    TransferStatus::Confirmed { .. } => ReceiptStatus::Confirmed,
                    TransferStatus::AwaitingConfirmation { .. } => {
                        ReceiptStatus::AwaitingConfirmation
                    }
                },
            })
            .collect();
        return (Phase::Transfers(entries), None);
    }
    let Some(report) = reports.pop() else {
        return (Phase::NoTransfer, None);
    };
    let phase = match report.status {
        TransferStatus::Unavailable { reason } => Phase::Failed(reason),
        TransferStatus::RecoveryRequired => Phase::RecoveryRequired,
        TransferStatus::VersionsUnverified => Phase::VersionsUnverified,
        TransferStatus::Confirmed { components }
        | TransferStatus::AwaitingConfirmation { components } => Phase::Reconnected(
            components
                .into_iter()
                .map(|item| Observation {
                    part_number: item.part_number,
                    name: item.name,
                    offered: item.offered,
                    status: match item.status {
                        Confirmation::Confirmed => InstalledStatus::Confirmed,
                        Confirmation::Unchanged => InstalledStatus::Unchanged,
                        Confirmation::Unreported => InstalledStatus::Unreported,
                        Confirmation::OtherVersion(v) => {
                            InstalledStatus::OtherVersion(v.to_string())
                        }
                    },
                })
                .collect(),
        ),
    };
    (phase, Some(report.receipt))
}

/// Recover one retained transfer for the connected device under the registry lock.
/// # Errors
/// The selected device has no unique interrupted transfer or recovery failed.
pub async fn recover_registered(
    device: &Connection,
    registry: &PendingRecoveryStore,
    selected: Option<Uuid>,
    progress: &ProgressReporter,
) -> Result<Option<MountedUpdateRecoveryReport>> {
    let identity = device.manifest.identity_digest();
    let _slot = registry.lock_device(&identity)?;
    let (pending, active) = recovery_entry(device, registry, selected).await?;
    let receipt = super::status::read_selection(&pending.transaction)
        .await
        .with_context(|| {
            format!(
                "restore the original firmware capture at {}",
                pending.transaction.display()
            )
        })?;
    let bound = receipt.is_bound_to(&device.manifest, &pending.plan_digest)?;
    if !pending.is_prepared()? {
        ensure!(
            bound,
            "this older receipt has no plan binding; restore its original prepared journal at {}",
            pending.transaction.display()
        );
        ensure!(
            !active
                && !pending
                    .transaction
                    .join("mounted-update/transaction/committed.json")
                    .try_exists()?,
            "restore the original firmware recovery journal at {}",
            pending.transaction.display()
        );
        ensure!(!progress.is_cancelled(), "device was disconnected");
        registry.discard_unprepared(&pending)?;
        return Ok(None);
    }
    let committed = garmin_update::update_is_committed(
        &pending.transaction,
        &identity,
        &pending.plan_digest,
        device.device.execution_target(),
    )
    .await?;
    ensure!(
        active || !committed,
        "transfer already finalized; inspect its installed versions"
    );
    let report = receipt
        .recover(
            &pending.transaction,
            &device.manifest,
            &pending.plan_digest,
            device.device.as_ref(),
            progress,
        )
        .await?;
    if matches!(
        report.outcome,
        MountedUpdateRecoveryOutcome::Restored | MountedUpdateRecoveryOutcome::AlreadyRolledBack
    ) {
        ensure!(!progress.is_cancelled(), "device was disconnected");
        registry.clear(&pending)?;
    }
    Ok(Some(report))
}

/// Inspect whether a failed rollback can preserve a journal-bound empty upload.
/// # Errors
/// The registry is unavailable.
pub async fn review_empty_uploads(
    device: &Connection,
    registry: &PendingRecoveryStore,
    selected: Option<Uuid>,
    progress: &ProgressReporter,
) -> Result<Option<AssistedRecoveryPlan>> {
    let identity = device.manifest.identity_digest();
    let _slot = registry.lock_device(&identity)?;
    let Ok((pending, true)) = recovery_entry(device, registry, selected).await else {
        return Ok(None);
    };
    if !super::status::read_selection(&pending.transaction)
        .await
        .is_ok_and(|receipt| {
            receipt
                .is_bound_to(&device.manifest, &pending.plan_digest)
                .is_ok()
        })
    {
        return Ok(None);
    }
    let result = review_assisted_recovery(
        &pending.transaction,
        &identity,
        device.device.as_ref(),
        progress,
    )
    .await;
    // The original rollback error remains authoritative unless the evidence proves an assisted repair.
    Ok(result.ok().flatten())
}

/// Revalidate the reviewed empty files, preserve them and restore the originals.
/// # Errors
/// The device, files, receipt or recovery evidence changed.
pub async fn approve_empty_uploads(
    device: &Connection,
    registry: &PendingRecoveryStore,
    selected: Option<Uuid>,
    review: &AssistedRecoveryPlan,
    progress: &ProgressReporter,
) -> Result<MountedUpdateRecoveryReport> {
    let identity = device.manifest.identity_digest();
    let _slot = registry.lock_device(&identity)?;
    let (pending, active) = recovery_entry(device, registry, selected).await?;
    ensure!(active, "no active firmware transaction");
    super::status::read_selection(&pending.transaction)
        .await?
        .is_bound_to(&device.manifest, &pending.plan_digest)?;
    let report = approve_assisted_recovery(
        &pending.transaction,
        &identity,
        device.device.as_ref(),
        review,
        progress,
    )
    .await?;
    ensure!(!progress.is_cancelled(), "device was disconnected");
    registry.clear(&pending)?;
    Ok(report.recovery)
}

async fn recovery_entry(
    device: &Connection,
    registry: &PendingRecoveryStore,
    selected: Option<Uuid>,
) -> Result<(PendingRecovery, bool)> {
    let identity = device.manifest.identity_digest();
    let active = DeviceTransactionStore::open(device.device.as_ref())
        .await?
        .active_prepared(&identity)
        .await?;
    let mut entries = registry.for_device(&identity)?;
    if let Some(active) = &active {
        entries.retain(|entry| entry.plan_digest == active.plan_digest());
        entries =
            matching_captures(entries, active, device.device.execution_target(), selected).await?;
    }
    let pending = if let Some(id) = selected {
        super::status::find_receipt(entries, id)?
    } else {
        ensure!(
            entries.len() == 1,
            "select one firmware receipt from firmware status"
        );
        entries.pop().expect("one pending receipt")
    };
    Ok((pending, active.is_some()))
}

async fn matching_captures(
    entries: Vec<PendingRecovery>,
    active: &garmin_update::PortableTransaction,
    target: Option<&str>,
    selected: Option<Uuid>,
) -> Result<Vec<PendingRecovery>> {
    use garmin_update::{UpdateTransactionMatch, same_recovery_history};
    let mut identified = false;
    let mut candidates = Vec::new();
    let mut unreadable = Vec::new();
    for (entry, matched) in super::status::classify_captures(entries, Some(active), target).await {
        match matched {
            Ok(UpdateTransactionMatch::Exact) => {
                identified = true;
                candidates.push(entry);
            }
            Ok(UpdateTransactionMatch::Legacy) => candidates.push(entry),
            Ok(UpdateTransactionMatch::Different) => {}
            Err(error) => unreadable.push(format!("{} ({error})", entry.transaction.display())),
        }
    }
    if !identified {
        ensure!(
            unreadable.is_empty(),
            "restore the original firmware capture at {} and inspect again",
            unreadable.join(", ")
        );
    }
    let Some(first) = candidates.first() else {
        anyhow::bail!("restore the capture for the active firmware transaction and inspect again");
    };
    if candidates.len() > 1 {
        for candidate in &candidates[1..] {
            ensure!(
                same_recovery_history(&first.transaction, &candidate.transaction).await?,
                "captures match the device's prepared files but their recovery histories differ; restore the original transaction captures and inspect again"
            );
        }
        ensure!(
            selected.is_some(),
            "matching firmware captures have identical recovery evidence; select one receipt from firmware status"
        );
    }
    Ok(candidates)
}

/// Bind the confirmation prompt to the registration the user is about to recover.
/// # Errors
/// The device or registry is unavailable or the receipt is ambiguous.
pub async fn recovery_receipt_id(
    device: &Connection,
    registry: &PendingRecoveryStore,
    selected: Option<Uuid>,
) -> Result<Uuid> {
    let _slot = registry.lock_device(&device.manifest.identity_digest())?;
    recovery_entry(device, registry, selected).await?.0.id()
}
