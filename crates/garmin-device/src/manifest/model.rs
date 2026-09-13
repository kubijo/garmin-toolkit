use std::{fmt, path::Path};

use camino::Utf8PathBuf;

use garmin_model::map::InstalledMapFile;

/// The detected device-metadata document format.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ManifestFormat {
    GarminDeviceV2,
}

impl fmt::Display for ManifestFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GarminDeviceV2 => formatter.write_str("GarminDevice v2"),
        }
    }
}

/// A tolerated deviation from a declared metadata format.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ManifestQuirk {
    /// `DataType` entries resume after an `UpdateFile` entry.
    InterleavedMassStorageEntries,
}

/// A supported normalized manifest data type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum DataType {
    Activity,
    Workout,
    Course,
}

/// A normalized manifest transfer direction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransferDirection {
    OutputFromUnit,
    InputToUnit,
    InputOutput,
}

/// Model information reported by device metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceModel {
    part_number: Option<String>,
    description: String,
    software_version: SoftwareVersion,
}

impl DeviceModel {
    pub(crate) fn new(
        part_number: Option<String>,
        description: String,
        software_version: SoftwareVersion,
    ) -> Self {
        Self {
            part_number,
            description,
            software_version,
        }
    }

    #[must_use]
    pub fn part_number(&self) -> Option<&str> {
        self.part_number.as_deref()
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub const fn software_version(&self) -> SoftwareVersion {
        self.software_version
    }
}

/// A globally unique unsigned device identifier reported by its metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DeviceId(u32);

impl DeviceId {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(&self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn into_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A device software version stored in hundredths.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SoftwareVersion(u16);

impl SoftwareVersion {
    #[must_use]
    pub const fn from_hundredths(value: u16) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_hundredths(&self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn into_hundredths(self) -> u16 {
        self.0
    }
}

impl fmt::Display for SoftwareVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{:02}", self.0 / 100, self.0 % 100)
    }
}

/// Format-neutral metadata produced by a registered manifest parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedManifest {
    raw_document: String,
    format: ManifestFormat,
    quirks: Vec<ManifestQuirk>,
    id: DeviceId,
    model: DeviceModel,
    capabilities: Vec<FileCapability>,
    installed_map_files: Vec<InstalledMapFile>,
}

impl ParsedManifest {
    pub(crate) fn new(
        raw_document: String,
        format: ManifestFormat,
        quirks: Vec<ManifestQuirk>,
        id: DeviceId,
        model: DeviceModel,
        capabilities: Vec<FileCapability>,
        installed_map_files: Vec<InstalledMapFile>,
    ) -> Self {
        Self {
            raw_document,
            format,
            quirks,
            id,
            model,
            capabilities,
            installed_map_files,
        }
    }

    #[must_use]
    pub fn raw_document(&self) -> &str {
        &self.raw_document
    }

    #[must_use]
    pub const fn format(&self) -> ManifestFormat {
        self.format
    }

    #[must_use]
    pub fn quirks(&self) -> &[ManifestQuirk] {
        &self.quirks
    }

    #[must_use]
    pub const fn id(&self) -> DeviceId {
        self.id
    }

    #[must_use]
    pub const fn model(&self) -> &DeviceModel {
        &self.model
    }

    #[must_use]
    pub fn capabilities(&self) -> &[FileCapability] {
        &self.capabilities
    }

    #[must_use]
    pub fn installed_map_files(&self) -> &[InstalledMapFile] {
        &self.installed_map_files
    }
}

/// A validated file capability independent of its source document format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileCapability {
    data_type: DataType,
    direction: TransferDirection,
    handle: OperationHandle,
}

impl FileCapability {
    pub(crate) const fn new(
        data_type: DataType,
        direction: TransferDirection,
        handle: OperationHandle,
    ) -> Self {
        Self {
            data_type,
            direction,
            handle,
        }
    }

    #[must_use]
    pub const fn data_type(&self) -> DataType {
        self.data_type
    }

    #[must_use]
    pub const fn direction(&self) -> TransferDirection {
        self.direction
    }

    #[must_use]
    pub const fn handle(&self) -> &OperationHandle {
        &self.handle
    }

    #[must_use]
    pub fn accepts(&self, path: &Path) -> bool {
        self.handle.matches(path)
    }
}

/// An opaque validated device location.
#[derive(Clone, Eq, PartialEq)]
pub struct OperationHandle {
    directory: Utf8PathBuf,
    basename: Option<String>,
    extension: String,
}

impl OperationHandle {
    pub(crate) fn new(directory: Utf8PathBuf, basename: Option<String>, extension: String) -> Self {
        Self {
            directory,
            basename,
            extension,
        }
    }

    pub(crate) fn matches(&self, path: &Path) -> bool {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        let filename = Path::new(filename);
        let basename_matches = self.basename.as_ref().is_none_or(|basename| {
            filename
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case(basename))
        });

        path.parent()
            .is_some_and(|parent| paths_equal(parent, self.directory.as_std_path()))
            && basename_matches
            && filename
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case(&self.extension))
    }
}

impl fmt::Debug for OperationHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationHandle")
            .finish_non_exhaustive()
    }
}

pub(crate) fn paths_equal(left: &Path, right: &Path) -> bool {
    let mut left = left.iter();
    let mut right = right.iter();
    loop {
        match (left.next(), right.next()) {
            (Some(left), Some(right)) => {
                let (Some(left), Some(right)) = (left.to_str(), right.to_str()) else {
                    return false;
                };
                if !left.eq_ignore_ascii_case(right) {
                    return false;
                }
            }
            (None, None) => return true,
            (Some(_), None) | (None, Some(_)) => return false,
        }
    }
}
