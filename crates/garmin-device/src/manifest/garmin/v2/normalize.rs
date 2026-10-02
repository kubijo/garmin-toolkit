use std::collections::HashMap;

use garmin_model::map::{InstalledMapFile, MapVersion};

use crate::manifest::{
    DataType, DeviceId, DeviceModel, FileCapability, ManifestError, ManifestFormat, ManifestQuirk,
    OperationHandle, ParsedManifest, SoftwareVersion, TransferDirection,
};

use super::wire::{Device, File, MassStorageEntry};

pub(super) fn normalize(raw: Device, xml: &str) -> Result<ParsedManifest, ManifestError> {
    let Device {
        namespace: _,
        model,
        id,
        mass_storage_mode,
    } = raw;
    let mut data_types = Vec::new();
    let mut update_files = Vec::new();
    let mut saw_update_file = false;
    let mut interleaved = false;
    for entry in mass_storage_mode.entries {
        match entry {
            MassStorageEntry::DataType(data_type) => {
                interleaved |= saw_update_file;
                data_types.push(data_type);
            }
            MassStorageEntry::UpdateFile(update_file) => {
                saw_update_file = true;
                update_files.push(update_file);
            }
            MassStorageEntry::Unsupported => {}
        }
    }
    let quirks = interleaved
        .then_some(ManifestQuirk::InterleavedMassStorageEntries)
        .into_iter()
        .collect();
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
                if capabilities[index].handle() != &handle {
                    return Err(ManifestError::AmbiguousCapability {
                        data_type,
                        direction,
                    });
                }
                continue;
            }

            indexes.insert(key, capabilities.len());
            capabilities.push(FileCapability::new(data_type, direction, handle));
        }
    }

    Ok(ParsedManifest::new(
        xml.to_owned(),
        ManifestFormat::GarminDeviceV2,
        quirks,
        DeviceId::from_u32(id),
        DeviceModel::new(
            model.part_number,
            model.description,
            SoftwareVersion::from_hundredths(model.software_version),
        ),
        capabilities,
        installed_map_files,
    ))
}

fn supported_data_type(name: &str) -> Option<DataType> {
    match name {
        "FIT_TYPE_4" => Some(DataType::Activity),
        "FIT_TYPE_5" => Some(DataType::Workout),
        "FIT_TYPE_6" => Some(DataType::Course),
        _ => None,
    }
}

fn parse_direction(data_type: &str, direction: &str) -> Result<TransferDirection, ManifestError> {
    match direction {
        "OutputFromUnit" => Ok(TransferDirection::OutputFromUnit),
        "InputToUnit" => Ok(TransferDirection::InputToUnit),
        "InputOutput" => Ok(TransferDirection::InputOutput),
        _ => Err(ManifestError::UnsupportedDirection {
            data_type: data_type.to_owned(),
            direction: direction.to_owned(),
        }),
    }
}

fn validate_location(data_type: &str, file: File) -> Result<OperationHandle, ManifestError> {
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

    Ok(OperationHandle::new(
        file.location.path.into(),
        file.location.basename,
        file.location.extension,
    ))
}

fn invalid_location(data_type: &str, reason: &'static str) -> ManifestError {
    ManifestError::InvalidLocation {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::garmin::v2::wire;

    const NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";
    const RAW_DOCUMENT: &str = "synthetic normalization input";

    #[test]
    fn canonical_and_interleaved_entries_normalize_identically() {
        let canonical = normalize(
            device(vec![activity(), workout(), update_file()]),
            RAW_DOCUMENT,
        )
        .unwrap();
        let interleaved = normalize(
            device(vec![activity(), update_file(), workout()]),
            RAW_DOCUMENT,
        )
        .unwrap();

        assert_eq!(canonical.id(), interleaved.id());
        assert_eq!(canonical.model(), interleaved.model());
        assert_eq!(canonical.capabilities(), interleaved.capabilities());
        assert_eq!(
            canonical.installed_map_files(),
            interleaved.installed_map_files()
        );
        assert!(canonical.quirks().is_empty());
        assert_eq!(
            interleaved.quirks(),
            [ManifestQuirk::InterleavedMassStorageEntries]
        );
    }

    #[test]
    fn rejects_an_unsafe_location_during_normalization() {
        assert!(matches!(
            normalize(
                device(vec![data_type(
                    "FIT_TYPE_4",
                    "GARMIN/../PRIVATE",
                    "OutputFromUnit",
                )]),
                RAW_DOCUMENT,
            ),
            Err(ManifestError::InvalidLocation { .. })
        ));
    }

    #[test]
    fn rejects_ambiguous_capabilities_during_normalization() {
        assert!(matches!(
            normalize(
                device(vec![
                    activity(),
                    data_type("FIT_TYPE_4", "GARMIN/BACKUP", "OutputFromUnit"),
                ]),
                RAW_DOCUMENT,
            ),
            Err(ManifestError::AmbiguousCapability { .. })
        ));
    }

    fn device(entries: Vec<wire::MassStorageEntry>) -> wire::Device {
        wire::Device {
            namespace: NAMESPACE.to_owned(),
            model: wire::Model {
                part_number: None,
                software_version: 1705,
                description: "Mock Watch-o-Matic 9000".to_owned(),
            },
            id: 123_456,
            mass_storage_mode: wire::MassStorageMode { entries },
        }
    }

    fn activity() -> wire::MassStorageEntry {
        data_type("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit")
    }

    fn workout() -> wire::MassStorageEntry {
        data_type("FIT_TYPE_5", "GARMIN/WORKOUTS", "InputOutput")
    }

    fn data_type(name: &str, path: &str, direction: &str) -> wire::MassStorageEntry {
        wire::MassStorageEntry::DataType(wire::DataType {
            name: name.to_owned(),
            files: vec![wire::File {
                specification: wire::Specification {
                    identifier: "FIT".to_owned(),
                },
                location: wire::Location {
                    path: path.to_owned(),
                    basename: None,
                    extension: "FIT".to_owned(),
                },
                transfer_direction: direction.to_owned(),
            }],
        })
    }

    fn update_file() -> wire::MassStorageEntry {
        wire::MassStorageEntry::UpdateFile(wire::UpdateFile {
            part_number: "006-D0000-00".to_owned(),
            version: wire::MapVersion { major: 1, minor: 2 },
        })
    }
}
