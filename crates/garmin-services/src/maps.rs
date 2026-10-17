//! Map-update execution shared by CLI, desktop, and remote hosts.

use anyhow::{Result, bail};
use garmin_capture::SessionCapture;
use garmin_device::{DeviceManifest, DeviceSummary};
use garmin_map_service::{GARMIN_EXPRESS_USER_AGENT, OmtClient};
use garmin_model::map::MapAuthorization;
use garmin_progress::{OperationStage, ProgressReporter};
use garmin_update::{
    BackupPolicy, DownloadError, DownloadProgress, DownloadUrlAuthorizer, Downloader, UpdatePlan,
    apply_mounted_mtp_with_progress,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

mod catalog;
pub mod device;
mod locks;
mod target;
pub use locks::MutationLocks;
mod operations;
mod outcomes;
mod session;
pub use operations::Operations;
mod workflow;
pub use session::Session;
pub use workflow::Settings;
pub mod recovery;
mod removal;
pub use catalog::CatalogSource;
pub use removal::{RemovalPlanReport, execute_removal, prepare_removal};
pub mod pending_recovery;
pub use target::{PhysicalTarget, SimulatedTarget, TargetSession, UpdateTarget};
type DeviceUpdateAdapter = Box<dyn UpdateTarget>;

pub struct UpdateExecution {
    pub client: OmtClient,
    pub manifest: DeviceManifest,
    pub plan: UpdatePlan,
    pub cache: PathBuf,
    pub concurrency: usize,
    pub download_authorizer: Arc<dyn DownloadUrlAuthorizer>,
    pub progress: ProgressReporter,
    pub capture: SessionCapture,
    pub device: DeviceUpdateAdapter,
    /// Payloads already verified during interactive plan review.
    pub prepared: Option<PreparedUpdatePayloads>,
}

impl UpdateExecution {
    /// Download, verify, and authorize payloads without modifying the target.
    /// # Errors
    /// Download, authorization, cancellation, or capture failure.
    pub async fn prepare_payloads(&self) -> Result<PreparedUpdatePayloads> {
        let downloader = Downloader::new(self.concurrency, GARMIN_EXPRESS_USER_AGENT)?
            .with_url_authorizer(Arc::clone(&self.download_authorizer))
            .with_capture(Some(self.capture.clone()));
        prepare_update_payloads(
            &downloader,
            &self.client,
            &self.manifest,
            &self.plan,
            &self.cache,
            &self.progress,
            &self.capture,
        )
        .await
    }
}

/// Garmin map-service policy for protected deliverable URLs.
#[derive(Debug)]
pub struct GarminDownloadAuthorizer;

impl DownloadUrlAuthorizer for GarminDownloadAuthorizer {
    fn authorize(&self, source: &url::Url) -> Result<url::Url, DownloadError> {
        garmin_map_service::authorize_download_url(source)
            .map_err(|error| DownloadError::ProtectedDownloadAuthorization(error.to_string()))
    }
}

pub struct UpdateOutcome {
    pub apply: garmin_update::ApplyReport,
    pub artifact: Option<PathBuf>,
}

/// Execute a plan while retaining durable host recovery evidence.
///
/// # Errors
/// Receipt creation, preparation, device mutation, or evidence capture failed.
pub async fn execute_registered_update(
    execution: UpdateExecution,
    receipts: Option<&pending_recovery::PendingRecoveryStore>,
) -> Result<UpdateOutcome> {
    if receipts.is_some() {
        execution.capture.write_json(Path::new("workflow-target.json"), &serde_json::json!({
            "version": 1, "device": execution.plan.device_digest, "plan": execution.plan.digest,
            "simulation": !execution.device.modifies_device(),
        })).await?;
    }
    let pending = receipts
        .map(|store| {
            store.register(
                pending_recovery::PendingRecoveryKind::Update,
                execution.capture.root(),
                &execution.plan.device_digest,
                &execution.plan.digest,
            )
        })
        .transpose()?;
    let result = execute_update_plan(execution).await;
    if let Some((store, recovery)) = receipts.zip(pending.as_ref()) {
        let rolled_back = result.as_ref().err().is_some_and(|error| {
            matches!(
                error.downcast_ref::<garmin_update::MountedInstallError>(),
                Some(garmin_update::MountedInstallError::RolledBack(_))
            )
        });
        if result.is_ok() || rolled_back {
            store.clear_completed(recovery);
        } else {
            match recovery.is_prepared() {
                Ok(false) => store.clear_completed(recovery),
                Ok(true) => {}
                Err(error) => tracing::warn!(%error, capture = %recovery.transaction.display(),
                    "pending update state could not be inspected"),
            }
        }
    }
    result
}

pub struct RecoveryExecution {
    pub transaction: PathBuf,
    pub device_digest: String,
    pub device: Box<dyn garmin_device::storage::DeviceWrite>,
    pub progress: ProgressReporter,
}

/// Recover a captured mounted-device transaction.
/// # Errors
/// Invalid evidence, a different device, device I/O, or incomplete recovery.
pub async fn recover_update(
    execution: RecoveryExecution,
) -> Result<garmin_update::MountedUpdateRecoveryReport> {
    Ok(garmin_update::recover_mounted_mtp_update(
        &execution.transaction,
        &execution.device_digest,
        execution.device.as_ref(),
        &execution.progress,
    )
    .await?)
}

/// Verified payloads bound to one immutable device plan.
pub struct PreparedUpdatePayloads {
    downloads: Vec<DownloadProgress>,
    authorization: MapAuthorization,
    plan_digest: String,
}

fn verification_report(
    manifest: DeviceManifest,
    plan: UpdatePlan,
    prepared: &PreparedUpdatePayloads,
    started: Instant,
) -> VerificationReport {
    VerificationReport {
        device: manifest.summary,
        backup_policy: plan.backup_policy,
        plan_id: plan.digest,
        files_verified: prepared.downloads.len(),
        bytes_verified: plan.total_bytes,
        authorization_files: prepared.authorization.unlocks.len(),
        embedded_authorizations: prepared.authorization.embedded_unlocks.len(),
        signed_storage_data: prepared.authorization.signed_sd_card_bytes.is_some(),
        elapsed_seconds: started.elapsed().as_secs_f64(),
        device_modified: false,
    }
}

#[derive(Debug, Serialize)]
struct VerificationReport {
    device: DeviceSummary,
    backup_policy: BackupPolicy,
    plan_id: String,
    files_verified: usize,
    bytes_verified: u64,
    authorization_files: usize,
    embedded_authorizations: usize,
    signed_storage_data: bool,
    elapsed_seconds: f64,
    device_modified: bool,
}

/// Execute an approved plan against the supplied target.
/// # Errors
/// Preparation, capacity, device I/O, recovery, or capture failure.
pub async fn execute_update_plan(execution: UpdateExecution) -> Result<UpdateOutcome> {
    let capture = execution.capture.clone();
    match execute_update_plan_inner(execution).await {
        Ok(outcome) => Ok(outcome),
        Err(operation) => {
            if let Err(evidence) = capture
                .write_bytes(Path::new("error.txt"), operation.to_string().as_bytes())
                .await
            {
                bail!("update failed ({operation}); failure capture also failed ({evidence})");
            }
            Err(operation)
        }
    }
}

async fn execute_update_plan_inner(mut execution: UpdateExecution) -> Result<UpdateOutcome> {
    let evidence = execution.capture.clone();
    execution.progress = execution.progress.observe(move |event| {
        let _ = evidence.append_event(event);
    });
    let started = Instant::now();
    capture_initial_state(
        execution.device.as_ref(),
        &execution.capture,
        &execution.progress,
    )
    .await?;
    let prepared = match execution.prepared.take() {
        Some(prepared) if prepared.plan_digest == execution.plan.digest => prepared,
        Some(_) => bail!("verified payloads belong to a different plan"),
        None => execution.prepare_payloads().await?,
    };
    let UpdateExecution {
        manifest,
        plan,
        progress,
        capture,
        device,
        ..
    } = execution;
    let device_modified = device.modifies_device();
    let target = device
        .prepare(
            &manifest,
            &plan,
            &prepared.authorization,
            &capture,
            &progress,
        )
        .await?;
    let result = apply_mounted_mtp_with_progress(
        &plan,
        &prepared.downloads,
        &prepared.authorization,
        target.device(),
        &capture,
        progress.clone(),
    )
    .await;
    let state_after = target.device().state().await;
    progress.device_state(state_after.as_ref().cloned().map_err(ToString::to_string));
    let artifact = target.finish(&capture, result.is_ok()).await;
    if let Ok(state) = state_after {
        capture
            .write_json(Path::new("device-state-after.json"), &state)
            .await?;
    }
    let apply = match (result, artifact.as_ref()) {
        (Err(operation), Err(evidence)) => {
            bail!("update failed ({operation}); artifact reporting also failed ({evidence})")
        }
        (result, _) => result?,
    };
    let artifact = artifact?;
    if let Some(path) = &artifact {
        progress.completed_with_path(
            OperationStage::Cleanup,
            "Simulation files retained; physical source unchanged",
            path.display().to_string(),
            1,
            Some(1),
        );
    }
    let mut verification = verification_report(manifest, plan, &prepared, started);
    verification.device_modified = device_modified;
    capture
        .write_json(Path::new("verification-report.json"), &verification)
        .await?;
    capture
        .write_json(Path::new("apply-report.json"), &apply)
        .await?;
    capture
        .write_json(
            Path::new("complete.json"),
            &serde_json::json!({
                "status": "complete",
                "device_modified": device_modified,
                "backup_policy": apply.backup_policy,
                "artifact": artifact,
            }),
        )
        .await?;
    Ok(UpdateOutcome { apply, artifact })
}

async fn capture_initial_state(
    device: &dyn UpdateTarget,
    capture: &SessionCapture,
    progress: &ProgressReporter,
) -> Result<()> {
    let state = device.state().await;
    progress.device_state(state.as_ref().cloned().map_err(ToString::to_string));
    capture
        .write_json(Path::new("device-state-before.json"), &state?)
        .await?;
    Ok(())
}

async fn prepare_update_payloads(
    downloader: &Downloader,
    client: &OmtClient,
    manifest: &DeviceManifest,
    plan: &UpdatePlan,
    cache: &Path,
    progress: &ProgressReporter,
    capture: &SessionCapture,
) -> Result<PreparedUpdatePayloads> {
    capture.ensure_healthy()?;
    let downloads = downloader
        .stage_all_with_progress(&plan.downloads, cache, progress.clone())
        .await?;
    capture.ensure_healthy()?;
    if progress.is_cancelled() {
        return Err(garmin_update::DownloadError::Cancelled.into());
    }
    progress.started(
        OperationStage::Authorize,
        "Requesting device-bound map authorization data",
        Some(1),
    );
    let authorization = client
        .activate(manifest.raw_xml(), &plan.identifiers)
        .await
        .inspect_err(|_error| {
            progress.failed(OperationStage::Authorize, "Map authorization failed");
        })?;
    capture
        .write_json(Path::new("authorization.json"), &authorization)
        .await?;
    capture.ensure_healthy()?;
    progress.completed(
        OperationStage::Authorize,
        "Device-bound map authorization data received",
        1,
        Some(1),
    );
    Ok(PreparedUpdatePayloads {
        downloads,
        authorization,
        plan_digest: plan.digest.clone(),
    })
}
