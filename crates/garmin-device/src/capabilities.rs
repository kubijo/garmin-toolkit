//! Validated `GarminDevice.xml` capabilities.

use std::{collections::HashMap, fmt, path::Path};

use garmin_model::map::{InstalledMapFile, MapVersion};
use serde::Deserialize;
use thiserror::Error;

const GARMIN_DEVICE_V2_NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";
const GARMIN_DEVICE_MANIFEST: &str = "GARMIN/GarminDevice.xml";

#[must_use]
pub fn is_device_manifest(path: &Path) -> bool {
    paths_equal(path, Path::new(GARMIN_DEVICE_MANIFEST))
}

/// A supported manifest data type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum DataType {
    Activity,
    Workout,
    Course,
}

/// A manifest-declared transfer direction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransferDirection {
    OutputFromUnit,
    InputToUnit,
    InputOutput,
}

/// Model information reported by the manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceModel {
    part_number: Option<String>,
    description: String,
    software_version: SoftwareVersion,
}

impl DeviceModel {
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

/// Garmin's globally unique unsigned device identifier.
///
/// Defined by the official
/// [`GarminDevice` v2 schema](https://www8.garmin.com/xmlschemas/GarminDevicev2.xsd).
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

/// A Garmin software version stored in hundredths.
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

/// A validated device manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    raw_xml: String,
    id: DeviceId,
    model: DeviceModel,
    capabilities: Vec<FileCapability>,
    installed_map_files: Vec<InstalledMapFile>,
}

impl Manifest {
    #[must_use]
    pub fn raw_xml(&self) -> &str {
        &self.raw_xml
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

/// A validated file capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileCapability {
    data_type: DataType,
    direction: TransferDirection,
    handle: OperationHandle,
}

impl FileCapability {
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
    directory: String,
    basename: Option<String>,
    extension: String,
}

impl OperationHandle {
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
            .is_some_and(|parent| paths_equal(parent, Path::new(&self.directory)))
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

/// A manifest rejection.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid Garmin device XML: {0}")]
    Xml(String),
    #[error("unsupported Garmin device namespace: {0}")]
    UnsupportedSchema(String),
    #[error("invalid location for {data_type}: {reason}")]
    InvalidLocation {
        /// Data-type name.
        data_type: String,
        /// Diagnostic reason.
        reason: &'static str,
    },
    #[error("unsupported transfer direction for {data_type}: {direction}")]
    UnsupportedDirection {
        /// Data-type name.
        data_type: String,
        /// Declared direction.
        direction: String,
    },
    #[error("ambiguous duplicate declaration for {data_type:?} {direction:?}")]
    AmbiguousCapability {
        /// Manifest data type.
        data_type: DataType,
        /// Transfer direction.
        direction: TransferDirection,
    },
}

/// Parses and narrows `GarminDevice.xml`.
/// # Errors
/// Malformed, unsupported, unsafe, or ambiguous input.
pub fn parse(xml: &str) -> Result<Manifest, Error> {
    let raw: RawDevice =
        quick_xml::de::from_str(xml).map_err(|error| Error::Xml(error.to_string()))?;

    if raw.namespace != GARMIN_DEVICE_V2_NAMESPACE {
        return Err(Error::UnsupportedSchema(raw.namespace));
    }

    let RawDevice {
        namespace: _,
        model,
        id,
        mass_storage_mode,
    } = raw;
    let RawMassStorageMode {
        data_types,
        update_files,
    } = mass_storage_mode;
    let installed_map_files = update_files
        .into_iter()
        .filter_map(|file| {
            let part_number = file.part_number.trim();
            (!part_number.is_empty()).then(|| InstalledMapFile {
                part_number: part_number.to_owned(),
                version: MapVersion::new(file.version.major, file.version.minor),
            })
        })
        .collect();
    let mut capabilities: Vec<FileCapability> = Vec::new();
    let mut indexes: HashMap<(DataType, TransferDirection), usize> = HashMap::new();

    for raw_data_type in data_types {
        let Some(data_type) = supported_data_type(&raw_data_type.name) else {
            continue;
        };

        for file in raw_data_type.files {
            let direction = parse_direction(&raw_data_type.name, &file.transfer_direction)?;
            let handle = validate_location(&raw_data_type.name, file)?;
            let key = (data_type, direction);

            if let Some(index) = indexes.get(&key).copied() {
                if capabilities[index].handle != handle {
                    return Err(Error::AmbiguousCapability {
                        data_type,
                        direction,
                    });
                }
                continue;
            }

            indexes.insert(key, capabilities.len());
            capabilities.push(FileCapability {
                data_type,
                direction,
                handle,
            });
        }
    }

    Ok(Manifest {
        raw_xml: xml.to_owned(),
        id: DeviceId::from_u32(id),
        model: DeviceModel {
            part_number: model.part_number,
            description: model.description,
            software_version: SoftwareVersion::from_hundredths(model.software_version),
        },
        capabilities,
        installed_map_files,
    })
}

fn supported_data_type(name: &str) -> Option<DataType> {
    match name {
        "FIT_TYPE_4" => Some(DataType::Activity),
        "FIT_TYPE_5" => Some(DataType::Workout),
        "FIT_TYPE_6" => Some(DataType::Course),
        _ => None,
    }
}

fn parse_direction(data_type: &str, direction: &str) -> Result<TransferDirection, Error> {
    match direction {
        "OutputFromUnit" => Ok(TransferDirection::OutputFromUnit),
        "InputToUnit" => Ok(TransferDirection::InputToUnit),
        "InputOutput" => Ok(TransferDirection::InputOutput),
        _ => Err(Error::UnsupportedDirection {
            data_type: data_type.to_owned(),
            direction: direction.to_owned(),
        }),
    }
}

fn validate_location(data_type: &str, file: RawFile) -> Result<OperationHandle, Error> {
    if file.specification.identifier != "FIT" {
        return Err(invalid_location(data_type, "expected FIT specification"));
    }
    if !file.location.extension.eq_ignore_ascii_case("fit") {
        return Err(invalid_location(data_type, "expected FIT extension"));
    }
    if !is_safe_relative_path(&file.location.path) {
        return Err(invalid_location(data_type, "unsafe relative path"));
    }
    if file
        .location
        .basename
        .as_deref()
        .is_some_and(|basename| !is_safe_filename_component(basename))
    {
        return Err(invalid_location(data_type, "unsafe basename"));
    }

    Ok(OperationHandle {
        directory: file.location.path,
        basename: file.location.basename,
        extension: file.location.extension,
    })
}

fn invalid_location(data_type: &str, reason: &'static str) -> Error {
    Error::InvalidLocation {
        data_type: data_type.to_owned(),
        reason,
    }
}

fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && path.split('/').all(is_safe_filename_component)
}

fn is_safe_filename_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains(['/', '\\'])
}

pub(crate) fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = left
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>();
    let right = right
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>();

    left.len() == right.len()
        && left
            .iter()
            .zip(&right)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

#[derive(Debug, Deserialize)]
#[serde(rename = "Device")]
struct RawDevice {
    #[serde(rename = "@xmlns")]
    namespace: String,
    #[serde(rename = "Model")]
    model: RawModel,
    #[serde(rename = "Id")]
    id: u32,
    #[serde(rename = "MassStorageMode")]
    mass_storage_mode: RawMassStorageMode,
}

#[derive(Debug, Deserialize)]
struct RawModel {
    #[serde(default, rename = "PartNumber")]
    part_number: Option<String>,
    #[serde(rename = "SoftwareVersion")]
    software_version: u16,
    #[serde(rename = "Description")]
    description: String,
}

#[derive(Debug, Deserialize)]
struct RawMassStorageMode {
    #[serde(default, rename = "DataType")]
    data_types: Vec<RawDataType>,
    #[serde(default, rename = "UpdateFile")]
    update_files: Vec<RawUpdateFile>,
}

#[derive(Debug, Deserialize)]
struct RawUpdateFile {
    #[serde(rename = "PartNumber")]
    part_number: String,
    #[serde(rename = "Version")]
    version: RawMapVersion,
}

#[derive(Debug, Deserialize)]
struct RawMapVersion {
    #[serde(rename = "Major")]
    major: u64,
    #[serde(rename = "Minor")]
    minor: u64,
}

#[derive(Debug, Deserialize)]
struct RawDataType {
    #[serde(rename = "Name")]
    name: String,
    #[serde(default, rename = "File")]
    files: Vec<RawFile>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(rename = "Specification")]
    specification: RawSpecification,
    #[serde(rename = "Location")]
    location: RawLocation,
    #[serde(rename = "TransferDirection")]
    transfer_direction: String,
}

#[derive(Debug, Deserialize)]
struct RawSpecification {
    #[serde(rename = "Identifier")]
    identifier: String,
}

#[derive(Debug, Deserialize)]
struct RawLocation {
    #[serde(rename = "Path")]
    path: String,
    #[serde(default, rename = "BaseName")]
    basename: Option<String>,
    #[serde(rename = "FileExtension")]
    extension: String,
}
