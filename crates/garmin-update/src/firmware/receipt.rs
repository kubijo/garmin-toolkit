use crate::{MountedInstallError, MountedUpdateRecoveryReport, mounted_install::recover_update};
use garmin_device::{DeviceManifest, SafeRelativePath, storage::DeviceWrite};
use garmin_model::map::MapVersion;
use garmin_progress::ProgressReporter;
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::{CatalogError, FirmwareOffer, UpdateMetadata, installed_at};

/// Host-side evidence for checking the selected components after reconnection.
#[derive(Serialize, Deserialize)]
pub struct FirmwareReceipt {
    device_digest: String,
    #[serde(default)]
    plan_digest: Option<String>,
    model_part: Option<String>,
    components: Vec<ExpectedComponent>,
}

#[derive(Serialize, Deserialize)]
struct ExpectedComponent {
    part_number: String,
    #[serde(default)]
    name: Option<String>,
    directory: String,
    file_name: String,
    installed: Option<MapVersion>,
    offered: String,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confirmation {
    Confirmed,
    Unchanged,
    OtherVersion(MapVersion),
    Unreported,
}

pub struct ComponentObservation<'a> {
    pub part_number: &'a str,
    pub name: &'a str,
    pub offered: &'a str,
    pub status: Confirmation,
}

/// Receipt components and whether the device now reports their offered versions.
pub(crate) struct InstalledFirmware<'a>(Vec<(&'a ExpectedComponent, bool)>);

impl<'a> InstalledFirmware<'a> {
    fn from_receipt(receipt: &'a FirmwareReceipt, manifest: &DeviceManifest) -> Self {
        Self(
            receipt
                .components
                .iter()
                .map(|component| {
                    let confirmed =
                        matches!(component.observation(manifest), Ok(Confirmation::Confirmed));
                    (component, confirmed)
                })
                .collect(),
        )
    }

    /// Some(false) marks a selected payload that still needs its staged file.
    pub(crate) fn payload(&self, path: &SafeRelativePath) -> Option<bool> {
        let (directory, name) = path.as_utf8_path().as_str().rsplit_once('/')?;
        self.0.iter().find_map(|(c, installed)| {
            (directory.eq_ignore_ascii_case(&c.directory)
                && name.eq_ignore_ascii_case(&c.file_name))
            .then_some(*installed)
        })
    }

    pub(crate) fn companion(&self, metadata: &UpdateMetadata) -> bool {
        let mut matches = self.0.iter().filter(|(c, _)| {
            c.part_number == metadata.part_number
                && c.offered == metadata.destination_version
                && c.installed
                    .is_some_and(|v| v.to_string() == metadata.source_version)
        });
        matches.next().is_some_and(|(_, installed)| *installed) && matches.next().is_none()
    }
}

impl ExpectedComponent {
    fn observation(&self, fresh: &DeviceManifest) -> Result<Confirmation, CatalogError> {
        let found = installed_at(
            fresh.capabilities(),
            &self.part_number,
            &self.directory,
            &self.file_name,
        )?;
        Ok(match found.map(|file| file.version) {
            Some(version) if version.to_string() == self.offered => Confirmation::Confirmed,
            Some(version) if Some(version) == self.installed => Confirmation::Unchanged,
            Some(version) => Confirmation::OtherVersion(version),
            None => Confirmation::Unreported,
        })
    }
}

impl FirmwareReceipt {
    /// Check a selection against its registered device and plan. Older receipts have no plan binding.
    /// # Errors
    /// The receipt belongs to a different device or firmware selection.
    pub fn is_bound_to(
        &self,
        manifest: &DeviceManifest,
        plan: &str,
    ) -> Result<bool, MountedInstallError> {
        if manifest.identity_digest() != self.device_digest
            || manifest.summary.part_number != self.model_part
        {
            return Err(MountedInstallError::RecoveryDeviceMismatch);
        }
        match &self.plan_digest {
            None => Ok(false),
            Some(value) if value == plan => Ok(true),
            Some(_) => Err(MountedInstallError::FirmwareSelectionMismatch),
        }
    }

    /// Finalize a committed transfer even after the device has consumed its staged files.
    /// # Errors
    /// The receipt, device identity, transaction or observed device state fails validation.
    pub async fn recover(
        &self,
        capture: &Path,
        manifest: &DeviceManifest,
        registered_plan: &str,
        device: &dyn DeviceWrite,
        progress: &ProgressReporter,
    ) -> Result<MountedUpdateRecoveryReport, MountedInstallError> {
        let installed = self
            .is_bound_to(manifest, registered_plan)?
            .then(|| InstalledFirmware::from_receipt(self, manifest));
        recover_update(
            capture,
            &self.device_digest,
            device,
            progress,
            Some((registered_plan, installed.as_ref())),
        )
        .await
    }

    /// Record the payload versions. Companion files describe the transition of their parent component.
    /// # Errors
    /// An installed part has an ambiguous destination.
    pub fn new(
        device: &DeviceManifest,
        plan_digest: &str,
        offers: &[FirmwareOffer],
    ) -> Result<Self, CatalogError> {
        let components = offers
            .iter()
            .filter(|o| o.data_type != "UpdateMetadata")
            .map(|offer| {
                Ok(ExpectedComponent {
                    part_number: offer.part_number.clone(),
                    name: Some(offer.product_name.clone()),
                    directory: offer.installation.path.clone(),
                    file_name: offer.installation.file_name.clone(),
                    installed: offer
                        .installed_file(device.capabilities())?
                        .map(|f| f.version),
                    offered: offer.version.clone(),
                })
            })
            .collect::<Result<_, CatalogError>>()?;
        Ok(Self {
            device_digest: device.identity_digest(),
            plan_digest: Some(plan_digest.to_owned()),
            model_part: device.summary.part_number.clone(),
            components,
        })
    }

    /// Return one observation per selected component when the same device reconnects.
    /// Confirmation records the version reported by the device, including a matching reinstall.
    /// # Errors
    /// The new inventory has ambiguous entries for a selected part and destination.
    pub fn confirm(
        &self,
        fresh: &DeviceManifest,
    ) -> Result<Option<Vec<ComponentObservation<'_>>>, CatalogError> {
        if fresh.identity_digest() != self.device_digest
            || fresh.summary.part_number != self.model_part
        {
            return Ok(None);
        }
        self.components
            .iter()
            .map(|expected| {
                Ok(ComponentObservation {
                    part_number: &expected.part_number,
                    name: expected.name.as_deref().unwrap_or(&expected.file_name),
                    offered: &expected.offered,
                    status: expected.observation(fresh)?,
                })
            })
            .collect::<Result<_, _>>()
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::FirmwareCatalog;
    use garmin_device::{TransportKind, parse_manifest};
    use garmin_fixtures::firmware::{FENIX_24_20, FENIX_GSP_22_44};

    #[test]
    fn matching_installed_version_confirms_a_reinstall() {
        let manifest = parse_manifest(
            FENIX_24_20.inventory,
            TransportKind::MassStorage,
            "fixture".into(),
        )
        .unwrap();
        let catalog = FirmwareCatalog::from_json(FENIX_GSP_22_44.catalog.as_bytes()).unwrap();
        let receipt = FirmwareReceipt::new(&manifest, "reinstall-plan", &catalog.updates).unwrap();
        let evidence = InstalledFirmware::from_receipt(&receipt, &manifest);
        let path = SafeRelativePath::parse("Garmin/GUPDATE.GSP").unwrap();
        assert_eq!(evidence.payload(&path), Some(true));
        assert!(evidence.companion(&UpdateMetadata {
            part_number: "006-V4532-00".into(),
            source_version: "24.20".into(),
            destination_version: "24.20".into(),
            contains_critical_fix_version: false,
        }));
    }
}
