//! Source-neutral map catalog and authorization data.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Available map components and device cleanup targets.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapCatalog {
    pub maps: Vec<MapComponent>,
    pub files_to_remove: Vec<MapFile>,
    pub directories_to_remove: Vec<String>,
    pub bundled_maps: Vec<MapComponent>,
}

impl MapCatalog {
    pub fn attach_installed_versions(&mut self, installed: &[InstalledMapFile]) {
        for component in self.maps.iter_mut().chain(&mut self.bundled_maps) {
            component.installed_version = component.files_to_remove.iter().find_map(|target| {
                installed
                    .iter()
                    .find(|file| {
                        !target.part_number.is_empty()
                            && file.part_number.eq_ignore_ascii_case(&target.part_number)
                    })
                    .map(|file| file.version)
            });
        }
    }
}

/// One map component and its installation choices.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapComponent {
    pub release: Option<String>,
    pub installed_version: Option<MapVersion>,
    pub display_name: String,
    pub map_type: Option<String>,
    pub files_to_remove: Vec<MapFile>,
    pub install_options: Vec<MapInstallOption>,
    pub is_reinstall: bool,
    pub can_uninstall: bool,
    pub installation_state: InstallationState,
    pub required_for_geographic_area_grouping: bool,
    pub download_hosts: Vec<String>,
    pub geographic_region: Option<String>,
}

impl MapComponent {
    #[must_use]
    pub const fn operation(&self) -> MapOperation {
        if self.is_reinstall {
            return MapOperation::Reinstall;
        }
        match self.installation_state {
            InstallationState::NotInstalled => MapOperation::Install,
            InstallationState::Installed => MapOperation::Update,
            InstallationState::Partial => MapOperation::Repair,
            InstallationState::NewerInstalled => MapOperation::Downgrade,
        }
    }

    #[must_use]
    pub fn installed_release(&self) -> Option<String> {
        let installed = self.installed_version?;
        let normalized = self
            .release
            .as_deref()
            .and_then(MapVersion::from_release)
            .map_or(installed, |available| {
                installed.normalized_against(available)
            });
        Some(normalized.to_string())
    }

    #[must_use]
    pub fn version_status(&self) -> MapVersionStatus {
        let Some(installed) = self.installed_version else {
            return MapVersionStatus::Unknown;
        };
        let Some(available) = self.release.as_deref().and_then(MapVersion::from_release) else {
            return MapVersionStatus::Unknown;
        };
        if installed.normalized_against(available) < available {
            MapVersionStatus::Outdated
        } else {
            MapVersionStatus::UpToDate
        }
    }
}

/// Numeric version reported for one installed map file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct MapVersion {
    pub major: u64,
    pub minor: u64,
}

impl MapVersion {
    #[must_use]
    pub const fn new(major: u64, minor: u64) -> Self {
        Self { major, minor }
    }

    fn from_release(release: &str) -> Option<Self> {
        let (major, minor) = release.split_once('.').unwrap_or((release, "0"));
        Some(Self::new(major.parse().ok()?, minor.parse().ok()?))
    }

    fn normalized_against(self, available: Self) -> Self {
        if self.major < 100 && available.major >= 2000 {
            Self::new(available.major / 100 * 100 + self.major, self.minor)
        } else {
            self
        }
    }
}

impl fmt::Display for MapVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{:02}", self.major, self.minor)
    }
}

/// Installed map metadata extracted from a device manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct InstalledMapFile {
    pub part_number: String,
    pub version: MapVersion,
}

/// One device file associated with a map component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MapFile {
    pub file_name: String,
    pub part_number: String,
    pub size_in_bytes: u64,
    pub is_shared: bool,
}

impl MapFile {
    #[must_use]
    pub fn path(path: impl Into<String>) -> Self {
        Self {
            file_name: path.into(),
            part_number: String::new(),
            size_in_bytes: 0,
            is_shared: false,
        }
    }
}

impl From<String> for MapFile {
    fn from(path: String) -> Self {
        Self::path(path)
    }
}

impl From<&str> for MapFile {
    fn from(path: &str) -> Self {
        Self::path(path)
    }
}

/// Whether a map component is present on the device.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationState {
    #[default]
    NotInstalled,
    Installed,
    Partial,
    NewerInstalled,
}

impl InstallationState {
    #[must_use]
    pub const fn is_present(self) -> bool {
        !matches!(self, Self::NotInstalled)
    }
}

/// Device mutation represented by one catalog component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MapOperation {
    Install,
    Update,
    Reinstall,
    Repair,
    Downgrade,
}

/// Relationship between installed and offered map versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MapVersionStatus {
    Outdated,
    UpToDate,
    Unknown,
}

impl MapOperation {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Install => "Install",
            Self::Update => "Update",
            Self::Reinstall => "Reinstall",
            Self::Repair => "Repair",
            Self::Downgrade => "Downgrade",
        }
    }
}

/// One installation choice for a map component.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapInstallOption {
    pub identifier: MapInstallIdentifier,
    pub display_name: String,
    pub preview_url: Option<String>,
    pub is_preferred: bool,
    pub files: Vec<MapContent>,
}

/// Fields that identify a map installation during authorization.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct MapInstallIdentifier {
    pub region_part_number: Option<String>,
    pub map_image_part_number: Option<String>,
    pub activation_request_code: Option<String>,
}

/// One content file in a map installation.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapContent {
    pub part_number: Option<String>,
    pub file_name: String,
    pub data_type: Option<String>,
    pub downloads: Vec<MapDownload>,
    pub locale: Option<String>,
    pub external_file_name: Option<String>,
}

/// One downloadable representation of a map content file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapDownload {
    pub md5: String,
    pub size_in_bytes: u64,
    pub delivery_type: Option<String>,
    pub url: String,
}

/// Device-bound material required to install protected maps.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapAuthorization {
    pub signed_sd_card_bytes: Option<String>,
    pub unlocks: Vec<MapUnlock>,
    pub embedded_unlocks: Vec<EmbeddedMapUnlock>,
}

/// One authorization file and its unlock codes.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapUnlock {
    pub file_name: String,
    pub gma: String,
    pub codes: Vec<MapUnlockCode>,
}

/// An unlock code and the map parts it authorizes.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapUnlockCode {
    pub code: Option<String>,
    pub part_numbers: Option<Vec<String>>,
}

/// Authorization data embedded into an installed map file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EmbeddedMapUnlock {
    pub part_number: String,
    pub segments: Vec<MapFileSegment>,
}

/// One byte segment embedded into a map file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MapFileSegment {
    pub offset: i64,
    pub data: String,
}

#[cfg(test)]
mod tests {
    use super::{
        InstallationState, InstalledMapFile, MapCatalog, MapComponent, MapFile, MapOperation,
        MapVersion, MapVersionStatus,
    };

    #[test]
    fn component_operation_preserves_catalog_semantics() {
        for (is_reinstall, installation_state, expected) in [
            (
                false,
                InstallationState::NotInstalled,
                MapOperation::Install,
            ),
            (false, InstallationState::Installed, MapOperation::Update),
            (false, InstallationState::Partial, MapOperation::Repair),
            (
                false,
                InstallationState::NewerInstalled,
                MapOperation::Downgrade,
            ),
            (true, InstallationState::Installed, MapOperation::Reinstall),
        ] {
            let component = MapComponent {
                is_reinstall,
                installation_state,
                ..MapComponent::default()
            };
            assert_eq!(component.operation(), expected);
        }
    }

    #[test]
    fn catalog_matches_and_compares_installed_versions() {
        let component = |release: &str, part_number: &str| MapComponent {
            release: Some(release.to_owned()),
            files_to_remove: vec![MapFile {
                file_name: "Garmin/map.img".to_owned(),
                part_number: part_number.to_owned(),
                size_in_bytes: 1,
                is_shared: false,
            }],
            ..MapComponent::default()
        };
        let mut catalog = MapCatalog {
            maps: vec![
                component("2026.11", "006-TOPO"),
                component("9.00", "006-BASE"),
            ],
            bundled_maps: vec![component("2026.11", "006-UNKNOWN")],
            ..MapCatalog::default()
        };
        catalog.attach_installed_versions(&[
            InstalledMapFile {
                part_number: "006-topo".to_owned(),
                version: MapVersion::new(24, 10),
            },
            InstalledMapFile {
                part_number: "006-BASE".to_owned(),
                version: MapVersion::new(9, 0),
            },
        ]);

        assert_eq!(
            catalog.maps[0].installed_release().as_deref(),
            Some("2024.10")
        );
        assert_eq!(catalog.maps[0].version_status(), MapVersionStatus::Outdated);
        assert_eq!(catalog.maps[1].installed_release().as_deref(), Some("9.00"));
        assert_eq!(catalog.maps[1].version_status(), MapVersionStatus::UpToDate);
        assert_eq!(catalog.bundled_maps[0].installed_release(), None);
        assert_eq!(
            catalog.bundled_maps[0].version_status(),
            MapVersionStatus::Unknown
        );
    }
}
