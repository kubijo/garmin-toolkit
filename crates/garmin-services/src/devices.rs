//! Shared, read-only device inspection for local and remote clients.

pub mod fit_import;

use garmin_device::{attachments::Metadata, storage::DeviceRead};
use garmin_model::device::{
    DeviceInspection, InspectionFailure, InspectionFailureKind, InspectionSection,
    ManifestInspection,
};

#[must_use]
pub fn manifest_inspection(manifest: &garmin_device::DeviceManifest) -> ManifestInspection {
    ManifestInspection {
        identifier: manifest.capabilities().id().into_u32(),
        model: manifest.summary.model.clone(),
        software_version: manifest
            .capabilities()
            .model()
            .software_version()
            .into_hundredths(),
        device_digest: manifest.identity_digest(),
    }
}

/// Reads storage and toolkit state even when the Garmin manifest could not be read.
pub async fn inspect<D: DeviceRead + ?Sized>(
    device: &D,
    manifest: InspectionSection<ManifestInspection>,
) -> DeviceInspection {
    let digest = match &manifest {
        InspectionSection::Available(manifest) => Some(manifest.device_digest.as_str()),
        _ => None,
    };
    let mut toolkit = Vec::new();
    let storage = match device.state().await {
        Ok(state) => {
            for storage in &state.storages {
                toolkit
                    .push(garmin_update::inspect_device_state(device, &storage.id, digest).await);
            }
            InspectionSection::Available(state)
        }
        Err(error) => InspectionSection::Unavailable(failure(error.to_string())),
    };
    DeviceInspection {
        manifest,
        storage,
        toolkit,
    }
}

/// Adapter hook for attachment managers.
/// A partial inspection is still a usable result.
pub async fn inspect_attachment<D: DeviceRead + ?Sized>(
    device: &D,
    name: &str,
    metadata: Result<Metadata, String>,
) -> Metadata {
    let (mut metadata, manifest) = match metadata {
        Ok(metadata) => {
            let manifest = match (
                metadata.id,
                metadata.software_version,
                &metadata.device_digest,
            ) {
                (Some(id), Some(version), Some(digest)) => {
                    InspectionSection::Available(ManifestInspection {
                        identifier: id.into_u32(),
                        model: metadata.name.clone(),
                        software_version: version.into_hundredths(),
                        device_digest: digest.clone(),
                    })
                }
                _ => InspectionSection::Missing,
            };
            (metadata, manifest)
        }
        Err(message) => (
            Metadata {
                id: None,
                software_version: None,
                name: name.to_owned(),
                capabilities: Vec::new(),
                storage: garmin_device::DeviceStateSnapshot::default(),
                device_digest: None,
                report: None,
            },
            InspectionSection::Unavailable(failure(message)),
        ),
    };
    let report = inspect(device, manifest).await;
    if let InspectionSection::Available(storage) = &report.storage {
        metadata.storage.clone_from(storage);
    }
    metadata.report = Some(report);
    metadata
}

#[must_use]
pub fn failure(message: String) -> InspectionFailure {
    InspectionFailure {
        kind: InspectionFailureKind::Unreadable,
        message,
    }
}

#[cfg(test)]
mod tests;
