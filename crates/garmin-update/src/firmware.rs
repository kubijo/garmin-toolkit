//! Offline interpretation of Garmin device software offers.

mod receipt;
mod staging;
pub(crate) use receipt::InstalledFirmware;
pub use receipt::{ComponentObservation, Confirmation, FirmwareReceipt};
pub use staging::{FirmwarePlan, FirmwareTarget, PreflightError, staging_targets};

use std::collections::HashMap;
use std::path::Path;

use crate::{
    DownloadSpec,
    plan::{is_http_loopback_url, normalized_md5},
};
use garmin_device::{ParsedManifest, SafeRelativePath, manifest::InstalledUpdateFile};
use garmin_model::firmware::{Component, ReleaseNote as ComponentReleaseNote, StagingIssue};
use garmin_model::{artifact::ByteCount, value::Timestamp};
use jiff::civil::DateTime;
use md5::{Digest as _, Md5};
use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt as _;
use url::Url;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareCatalog {
    pub updates: Vec<FirmwareOffer>,
    pub access_level: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareOffer {
    pub part_number: String,
    pub semantic_version: Version,
    pub version: String,
    pub display_version: Option<String>,
    pub product_name: String,
    pub data_type: String,
    pub description: Option<String>,
    pub locale: Option<String>,
    pub severity: String,
    pub is_primary_firmware: bool,
    pub contains_critical_fix_version: bool,
    pub release_state: String,
    pub release_date: ReleaseDate,
    pub release_notes: Vec<ReleaseNote>,
    pub installation: Installation,
    pub deliverable: Option<Deliverable>,
    pub delivery_restrictions: Vec<String>,
    pub eula_url: Option<String>,
    pub secure_eula_url: Option<String>,
}

/// Garmin also publishes release dates without a UTC offset.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum ReleaseDate {
    Instant(Timestamp),
    Local(DateTime),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseNote {
    pub version: String,
    pub semantic_version: Version,
    pub change_log: Vec<String>,
    pub is_critical_fix_version: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent Garmin installation flags"
)]
pub struct Installation {
    pub path: String,
    pub file_name: String,
    pub order: u32,
    pub instructions: Vec<serde_json::Value>,
    pub is_product_installed: bool,
    pub is_reinstall: bool,
    pub is_restart_required: bool,
    pub is_delta: bool,
}

impl Installation {
    /// Returns the device-relative destination when the service supplies a safe directory and leaf.
    #[must_use]
    pub fn destination(&self) -> Option<SafeRelativePath> {
        if self.file_name.contains('/') {
            return None;
        }
        SafeRelativePath::parse(format!("{}/{}", self.path, self.file_name)).ok()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deliverable {
    pub download_url: Option<Url>,
    pub secure_download_url: Option<Url>,
    pub size_in_bytes: ByteCount,
    pub md5: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    pub part_number: String,
    pub source_version: String,
    pub destination_version: String,
    pub contains_critical_fix_version: bool,
}

impl UpdateMetadata {
    pub(crate) const MAX_BYTES: ByteCount = ByteCount::from_mebibytes(1);

    /// Decode the exact bytes named by the metadata offer from the verified cache.
    /// # Errors
    /// The file is oversized, has changed in the cache, or contains invalid JSON.
    pub async fn read_verified(path: &Path, spec: &DownloadSpec) -> Result<Self, MetadataError> {
        if spec.size > Self::MAX_BYTES {
            return Err(MetadataError::Oversized);
        }
        let mut bytes = Vec::new();
        tokio::fs::File::open(path)
            .await?
            .take(spec.size.as_bytes() + 1)
            .read_to_end(&mut bytes)
            .await?;
        if ByteCount::from_bytes(bytes.len() as u64) != spec.size
            || hex::encode(Md5::digest(&bytes)) != spec.md5
        {
            return Err(MetadataError::Integrity);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("firmware metadata exceeds the size limit")]
    Oversized,
    #[error("firmware metadata failed integrity verification")]
    Integrity,
    #[error("invalid firmware metadata: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("reading firmware metadata: {0}")]
    Io(#[from] std::io::Error),
}

/// One selectable update, with its matched inventory entry and companion delivery.
pub struct FirmwareComponent<'a> {
    pub offer: &'a FirmwareOffer,
    pub installed: Option<&'a InstalledUpdateFile>,
    pub metadata: Option<&'a FirmwareOffer>,
}

impl FirmwareComponent<'_> {
    /// Build a portable component and account for the payload and its metadata.
    /// # Errors
    /// The combined byte count overflows.
    pub fn snapshot(&self, simulator: Option<&Url>) -> Result<Component, CatalogError> {
        let offer = self.offer;
        let mut bytes = Some(ByteCount::default());
        for entry in std::iter::once(offer).chain(self.metadata) {
            bytes = match (bytes, entry.download_spec(simulator)) {
                (Some(total), Some(file)) => Some(
                    total
                        .checked_add(file.size)
                        .ok_or(CatalogError::SizeOverflow)?,
                ),
                _ => None,
            };
        }
        Ok(Component {
            part_number: offer.part_number.clone(),
            semantic_version: offer.semantic_version.to_string(),
            name: offer.product_name.clone(),
            description: offer.description.clone(),
            installed: self.installed.map(|file| file.version.to_string()),
            available: offer
                .display_version
                .clone()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| offer.semantic_version.to_string()),
            system_bundle: self.is_system_bundle(),
            release_notes: offer
                .release_notes
                .iter()
                .map(|r| ComponentReleaseNote {
                    version: r.version.clone(),
                    changes: r.change_log.clone(),
                })
                .collect(),
            needs_restart: offer.installation.is_restart_required
                || self
                    .metadata
                    .is_some_and(|m| m.installation.is_restart_required),
            has_metadata: self.metadata.is_some(),
            download_bytes: bytes,
            staging_issue: std::iter::once(offer)
                .chain(self.metadata)
                .find_map(FirmwareOffer::staging_issue),
        })
    }

    #[must_use]
    pub fn is_system_bundle(&self) -> bool {
        let location = &self.offer.installation;
        self.offer.is_primary_firmware
            || (self.installed.is_some()
                && self.offer.data_type == "Firmware"
                && location.path.eq_ignore_ascii_case("Garmin")
                && location.file_name.eq_ignore_ascii_case("GUPDATE.GSP"))
    }

    #[must_use]
    pub fn offers_system_upgrade(&self) -> bool {
        let offered = &self.offer.semantic_version;
        self.is_system_bundle()
            && !self.offer.installation.is_reinstall
            && self.installed.is_none_or(|file| {
                (offered.major, offered.minor) > (file.version.major, file.version.minor)
            })
    }

    /// Compare a decoded companion payload to the device's installed version and the offered transition.
    #[must_use]
    pub fn matches_transition(&self, record: &UpdateMetadata) -> bool {
        self.metadata.is_some()
            && record.part_number == self.offer.part_number
            && record.destination_version == self.offer.version
            && self
                .installed
                .is_some_and(|installed| installed.version.to_string() == record.source_version)
    }
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("firmware size overflow")]
    SizeOverflow,
    #[error("duplicate firmware offer {0} at version {1}")]
    DuplicateOffer(String, Version),
    #[error("no unique component for metadata offer {0} at version {1}")]
    UnmatchedMetadata(String, Version),
    #[error("multiple inventory entries for the destination of {0}")]
    AmbiguousInventory(String),
}

impl FirmwareOffer {
    /// Requirements that a file transfer over a wired connection cannot satisfy.
    #[must_use]
    pub fn staging_issue(&self) -> Option<StagingIssue> {
        if !self.installation.instructions.is_empty() {
            return Some(StagingIssue::InstallationInstructions);
        }
        self.delivery_restrictions
            .iter()
            .find(|channel| {
                !["BTC", "BLE", "LTE"]
                    .iter()
                    .any(|wireless| channel.eq_ignore_ascii_case(wireless))
            })
            .map(|restriction| StagingIssue::DeliveryRestriction(restriction.clone()))
    }

    /// Build the verified-cache request for a full deliverable. A simulator can only serve its own origin.
    #[must_use]
    pub fn download_spec(&self, simulator: Option<&Url>) -> Option<DownloadSpec> {
        let delivery = self.deliverable.as_ref()?;
        let source = delivery.secure_download_url.as_ref()?;
        let allowed_origin = simulator.map_or_else(
            || source.scheme() == "https",
            |base| is_http_loopback_url(base) && base.origin() == source.origin(),
        );
        if self.staging_issue().is_some()
            || !allowed_origin
            || !source.username().is_empty()
            || source.password().is_some()
            || self.installation.is_delta
            || delivery.size_in_bytes.is_zero()
        {
            return None;
        }
        let md5 = normalized_md5(&delivery.md5)?;
        Some(DownloadSpec {
            map_name: self.product_name.clone(),
            source: source.clone(),
            alternate_sources: Vec::new(),
            requires_garmin_token: false,
            destination: self.installation.destination()?,
            cache_name: md5.clone(),
            size: delivery.size_in_bytes,
            md5,
        })
    }

    /// Match the part and its destination to one inventory entry on this device.
    /// # Errors
    /// The device declares multiple entries for the same part and destination.
    pub fn installed_file<'a>(
        &self,
        device: &'a ParsedManifest,
    ) -> Result<Option<&'a InstalledUpdateFile>, CatalogError> {
        installed_at(
            device,
            &self.part_number,
            &self.installation.path,
            &self.installation.file_name,
        )
    }
}

fn installed_at<'a>(
    device: &'a ParsedManifest,
    part_number: &str,
    directory: &str,
    file_name: &str,
) -> Result<Option<&'a InstalledUpdateFile>, CatalogError> {
    let mut matches = device.installed_update_files().iter().filter(|file| {
        file.part_number == part_number
            && file
                .path
                .as_deref()
                .is_some_and(|path| path.eq_ignore_ascii_case(directory))
            && file
                .file_name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(file_name))
    });
    let installed = matches.next();
    if matches.next().is_some() {
        return Err(CatalogError::AmbiguousInventory(part_number.to_owned()));
    }
    Ok(installed)
}

impl FirmwareCatalog {
    /// Decode a device software response.
    /// # Errors
    /// The body is malformed or has an incompatible schema.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Resolve offers by part and semantic version, preserving the service's component order.
    /// # Errors
    /// Returns a catalog error if a component or companion cannot be identified unambiguously.
    pub fn components<'a>(
        &'a self,
        device: &'a ParsedManifest,
    ) -> Result<Vec<FirmwareComponent<'a>>, CatalogError> {
        let mut components = Vec::new();
        let mut indexes = HashMap::new();
        let (metadata, payloads): (Vec<_>, Vec<_>) = self
            .updates
            .iter()
            .partition(|offer| offer.data_type == "UpdateMetadata");
        for offer in payloads {
            let key = (offer.part_number.as_str(), &offer.semantic_version);
            if indexes.insert(key, components.len()).is_some() {
                return Err(CatalogError::DuplicateOffer(
                    offer.part_number.clone(),
                    offer.semantic_version.clone(),
                ));
            }
            components.push(FirmwareComponent {
                offer,
                installed: offer.installed_file(device)?,
                metadata: None,
            });
        }
        for offer in metadata {
            let index = offer
                .part_number
                .strip_suffix("-META")
                .and_then(|part| indexes.get(&(part, &offer.semantic_version)));
            let Some(component) = index.map(|&i| &mut components[i]) else {
                return Err(CatalogError::UnmatchedMetadata(
                    offer.part_number.clone(),
                    offer.semantic_version.clone(),
                ));
            };
            if component.metadata.replace(offer).is_some() {
                return Err(CatalogError::DuplicateOffer(
                    offer.part_number.clone(),
                    offer.semantic_version.clone(),
                ));
            }
        }
        Ok(components)
    }
}
