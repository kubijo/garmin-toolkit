//! Removal preparation and execution shared by every interface.

use std::path::Path;

use anyhow::{Result, bail};
use garmin_capture::SessionCapture;
use garmin_device::{DeviceInventory, storage::DeviceWrite};
use garmin_progress::ProgressReporter;
use garmin_update::{RemovalApplyReport, RemovalExecutionPlan, RemovalPlan};
use serde::Serialize;

use super::pending_recovery::{PendingRecoveryKind, PendingRecoveryStore};

#[derive(Debug, Clone, Serialize)]
pub struct RemovalPlanReport {
    pub service_plan: RemovalPlan,
    pub execution_plan: RemovalExecutionPlan,
}

/// Bind a catalog removal to the observed files
/// and retain the approval evidence.
///
/// # Errors
/// Inventory binding or evidence capture failed.
pub async fn prepare_removal(
    plan: RemovalPlan,
    inventory: &DeviceInventory,
    capture: Option<&SessionCapture>,
) -> Result<RemovalPlanReport> {
    let execution_plan = plan.bind_inventory(inventory)?;
    if let Some(capture) = capture {
        capture
            .write_json(Path::new("removal-plan.json"), &plan)
            .await?;
        capture
            .write_json(Path::new("device-inventory.json"), inventory)
            .await?;
        capture
            .write_json(Path::new("removal-execution-plan.json"), &execution_plan)
            .await?;
    }
    Ok(RemovalPlanReport {
        service_plan: plan,
        execution_plan,
    })
}

/// Execute an approved removal, retaining a receipt until completion is captured.
///
/// # Errors
/// Empty plan, receipt, device I/O, transaction, or capture failure.
pub async fn execute_removal(
    plan: &RemovalExecutionPlan,
    capture: &SessionCapture,
    progress: &ProgressReporter,
    device: &dyn DeviceWrite,
    receipts: &PendingRecoveryStore,
) -> Result<RemovalApplyReport> {
    if plan.files_to_remove.is_empty() {
        bail!("no selected component files are present on the device; nothing was changed");
    }
    let pending = receipts.register(
        PendingRecoveryKind::Removal,
        capture.root(),
        &plan.device_digest,
        &plan.digest,
    )?;
    let evidence = capture.clone();
    let progress = progress.observe(move |event| {
        let _ = evidence.append_event(event);
    });
    let result = garmin_update::execute_removal(plan, capture, &progress, device).await;
    if matches!(
        &result,
        Err(garmin_update::RemovalExecutionError::RolledBack(_))
    ) {
        receipts.clear_completed(&pending);
    }
    let result = result?;
    capture
        .write_json(Path::new("removal-report.json"), &result)
        .await?;
    receipts.clear_completed(&pending);
    Ok(result)
}
