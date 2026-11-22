use std::collections::HashSet;

use crate::{
    ApplyReport, DownloadProgress, DownloadSpec, UpdatePlan, UpdatePlanError,
    mounted_install::{InstallFiles, apply_files},
};
use garmin_capture::SessionCapture;
use garmin_device::{
    DevicePathState, ParsedManifest, SafeRelativePath,
    storage::{DeviceIoError, DeviceRead, DeviceWrite},
};
use garmin_model::artifact::ByteCount;
use garmin_progress::ProgressReporter;
use thiserror::Error;

use super::FirmwareOffer;
use crate::{MountedInstallError, mounted_install::write_location, space, space::StorageChange};

/// A device destination bound to the storage inspected during preflight.
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareTarget {
    pub path: SafeRelativePath,
    pub storage_id: String,
    pub existing_bytes: Option<ByteCount>,
    pub offered_bytes: ByteCount,
}

/// A firmware transfer has an ordered set of downloads and a device-bound recovery identity.
pub struct FirmwarePlan(UpdatePlan);

impl FirmwarePlan {
    /// Bind the ordered cache requests to this device.
    /// # Errors
    /// An empty selection, an inconsistent content address or a size overflow.
    pub fn new(
        device_digest: String,
        downloads: Vec<DownloadSpec>,
    ) -> Result<Self, PreflightError> {
        if downloads.is_empty() {
            return Err(PreflightError::IncompleteSelection);
        }
        let plan = UpdatePlan::from_files(device_digest, downloads, Vec::new(), Vec::new())?;
        plan.validate()?;
        Ok(Self(plan))
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.0.digest
    }

    #[must_use]
    pub const fn update_plan(&self) -> &UpdatePlan {
        &self.0
    }
    /// Stage verified files with the same backup, upload verification and journal as map updates.
    /// # Errors
    /// The reviewed destinations changed or the transaction could not be completed.
    pub async fn stage(
        &self,
        verified: &[DownloadProgress],
        targets: &[FirmwareTarget],
        device: &dyn DeviceWrite,
        capture: &SessionCapture,
        progress: ProgressReporter,
    ) -> Result<ApplyReport, PreflightError> {
        let plan = &self.0;
        if plan.downloads.len() != targets.len() {
            return Err(PreflightError::IncompleteSelection);
        }
        Ok(apply_files(
            plan,
            verified,
            InstallFiles::Firmware(targets),
            device,
            capture,
            progress,
        )
        .await?)
    }
}

/// Resolve every selected file, including companion metadata, in installation order.
/// # Errors
/// Rejects unsafe or conflicting destinations, missing storage, and insufficient free space.
pub async fn staging_targets(
    offers: &[FirmwareOffer],
    manifest: &ParsedManifest,
    device: &dyn DeviceRead,
) -> Result<Vec<FirmwareTarget>, PreflightError> {
    let mut seen = HashSet::new();
    let paths = offers
        .iter()
        .map(|offer| {
            if let Some(reason) = offer.staging_issue() {
                return Err(PreflightError::Unsupported(reason));
            }
            let path = offer
                .installation
                .destination()
                .ok_or_else(|| PreflightError::InvalidOffer(offer.part_number.clone()))?;
            if !seen.insert(path.as_utf8_path().as_str().to_ascii_lowercase()) {
                return Err(PreflightError::DuplicateDestination(path));
            }
            Ok(path)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let inventory = device.inventory(&paths).await?;
    let primary = device.primary_storage_id().await?;
    let mut targets = Vec::with_capacity(paths.len());
    for (offer, path) in offers.iter().zip(paths) {
        let location = write_location(
            &inventory,
            &primary,
            &path,
            manifest.update_storages(&offer.part_number),
        )?;
        let existing_bytes = match (location.state, location.size) {
            (DevicePathState::RegularFile, Some(size)) => Some(size),
            (DevicePathState::Missing, None) => None,
            _ => return Err(PreflightError::InvalidInventory(path)),
        };
        let delivery = offer
            .deliverable
            .as_ref()
            .ok_or_else(|| PreflightError::InvalidOffer(offer.part_number.clone()))?;
        targets.push(FirmwareTarget {
            path,
            storage_id: location.storage_id.clone(),
            existing_bytes,
            offered_bytes: delivery.size_in_bytes,
        });
    }
    let requirements = space::storage_requirements(targets.iter().map(|target| StorageChange {
        storage_id: &target.storage_id,
        before_bytes: target.existing_bytes.unwrap_or_default(),
        after_bytes: target.offered_bytes,
    }))?;
    space::check_device_space(&device.state().await?, &requirements)?;
    Ok(targets)
}

#[derive(Debug, Error)]
pub enum PreflightError {
    #[error("{0}")]
    Unsupported(garmin_model::firmware::StagingIssue),
    #[error("incomplete firmware selection")]
    IncompleteSelection,
    #[error(transparent)]
    Plan(#[from] UpdatePlanError),
    #[error("invalid firmware deliverable for {0}")]
    InvalidOffer(String),
    #[error("multiple firmware files target {0}")]
    DuplicateDestination(SafeRelativePath),
    #[error("inconsistent device inventory for {0}")]
    InvalidInventory(SafeRelativePath),
    #[error(transparent)]
    Device(#[from] DeviceIoError),
    #[error(transparent)]
    Target(#[from] MountedInstallError),
    #[error(transparent)]
    Space(#[from] space::SpaceError),
}
