//! Catalog access and deterministic planning, independent of the client presentation.

use anyhow::Result;
use garmin_capture::SessionCapture;
use garmin_device::DeviceManifest;
use garmin_map_service::{ClientIdentity, OmtClient};
use garmin_model::map::MapCatalog;
use garmin_update::UpdatePlan;
use std::path::Path;
use url::Url;

/// The composition root chooses production or a loopback fixture service.
#[derive(Clone, Debug)]
pub enum CatalogSource {
    Garmin,
    Loopback(Url),
}

impl CatalogSource {
    /// Describe available choices using the same validated
    /// plans and artifact cache as execution.
    ///
    /// # Errors
    /// Invalid catalog, selection, or inaccessible cache.
    pub async fn components(
        &self,
        catalog: &MapCatalog,
        device: &str,
        cache: &Path,
    ) -> Result<Vec<garmin_service_api::maps::Component>> {
        use garmin_service_api::maps::{Choice, Component};
        let mut components = Vec::new();
        for (index, map) in catalog.maps.iter().chain(&catalog.bundled_maps).enumerate() {
            let can_install = !map.install_options.is_empty();
            let (download_bytes, cached_files, total_files) = if can_install {
                let plan = self.plan(catalog, device.to_owned(), vec![index])?;
                let cache = garmin_update::inspect_artifact_cache(&plan.downloads, cache).await?;
                (
                    plan.total_bytes,
                    u32::try_from(cache.cached_files)?,
                    u32::try_from(cache.total_files)?,
                )
            } else {
                (0, 0, 0)
            };
            components.push(Component {
                index: u32::try_from(index)?,
                name: map.display_name.clone(),
                installed: map.installed_release(),
                available: map.release.clone(),
                operation: map.operation(),
                can_install,
                can_remove: map.installation_state.is_present()
                    && map.can_uninstall
                    && !map.files_to_remove.is_empty(),
                choice: Choice::Keep,
                download_bytes,
                cached_files,
                total_files,
            });
        }
        Ok(components)
    }

    /// Constructs the selected service client.
    ///
    /// # Errors
    /// Invalid service configuration.
    pub fn client(&self) -> Result<OmtClient> {
        let identity = ClientIdentity::default();
        Ok(match self {
            Self::Garmin => OmtClient::anonymous(&identity)?,
            Self::Loopback(base) => OmtClient::local_mock(&identity, base.clone())?,
        })
    }

    /// Queries maps using the retained device manifest and installed versions.
    ///
    /// # Errors
    /// Capture or service failures.
    pub async fn query(
        &self,
        manifest: &DeviceManifest,
        capture: Option<SessionCapture>,
    ) -> Result<(OmtClient, MapCatalog)> {
        if let Some(capture) = &capture {
            capture
                .write_bytes(
                    Path::new("device/GarminDevice.xml"),
                    manifest.raw_xml().as_bytes(),
                )
                .await?;
        }
        let client = self.client()?.with_capture(capture);
        let catalog = client
            .check_maps(
                manifest.raw_xml(),
                manifest.capabilities().installed_map_files(),
            )
            .await?;
        Ok((client, catalog))
    }

    /// Builds the same approved-origin plan for every presentation.
    ///
    /// # Errors
    /// Invalid selections, paths, or deliverables.
    pub fn plan(
        &self,
        catalog: &MapCatalog,
        device: String,
        selected: Vec<usize>,
    ) -> Result<UpdatePlan> {
        Ok(match self {
            Self::Garmin => UpdatePlan::from_response_selection(catalog, device, selected)?,
            Self::Loopback(base) => {
                UpdatePlan::from_mock_response_selection(catalog, device, selected, base)?
            }
        })
    }
}
