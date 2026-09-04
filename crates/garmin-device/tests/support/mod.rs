//! Typed synthetic manifests.

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename = "Device")]
struct Device {
    #[serde(rename = "@xmlns")]
    namespace: String,
    #[serde(rename = "Model")]
    model: Model,
    #[serde(rename = "Id")]
    id: u32,
    #[serde(rename = "MassStorageMode")]
    mass_storage_mode: MassStorageMode,
}

#[derive(Serialize)]
struct Model {
    #[serde(rename = "SoftwareVersion")]
    software_version: u16,
    #[serde(rename = "Description")]
    description: String,
}

#[derive(Serialize)]
struct MassStorageMode {
    #[serde(rename = "DataType")]
    data_types: Vec<DataType>,
    #[serde(rename = "UpdateFile")]
    update_files: Vec<UpdateFile>,
}

#[derive(Serialize)]
struct UpdateFile {
    #[serde(rename = "PartNumber")]
    part_number: String,
    #[serde(rename = "Version")]
    version: Version,
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "FileName")]
    file_name: String,
}

#[derive(Serialize)]
struct Version {
    #[serde(rename = "Major")]
    major: u64,
    #[serde(rename = "Minor")]
    minor: u64,
}

#[derive(Serialize)]
struct DataType {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "File")]
    file: File,
}

#[derive(Serialize)]
struct File {
    #[serde(rename = "Specification")]
    specification: Specification,
    #[serde(rename = "Location")]
    location: Location,
    #[serde(rename = "TransferDirection")]
    transfer_direction: String,
}

#[derive(Serialize)]
struct Specification {
    #[serde(rename = "Identifier")]
    identifier: String,
}

#[derive(Serialize)]
struct Location {
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "FileExtension")]
    extension: String,
}

pub fn manifest(
    namespace: &str,
    data_types: &[(&str, &str, &str)],
) -> Result<String, quick_xml::SeError> {
    manifest_with_updates(namespace, data_types, &[])
}

pub fn manifest_with_updates(
    namespace: &str,
    data_types: &[(&str, &str, &str)],
    update_files: &[(&str, u64, u64, &str)],
) -> Result<String, quick_xml::SeError> {
    quick_xml::se::to_string(&Device {
        namespace: namespace.to_owned(),
        model: Model {
            software_version: 2244,
            description: "Synthetic Garmin".to_owned(),
        },
        id: 123_456,
        mass_storage_mode: MassStorageMode {
            data_types: data_types
                .iter()
                .map(|(name, path, direction)| DataType {
                    name: (*name).to_owned(),
                    file: File {
                        specification: Specification {
                            identifier: "FIT".to_owned(),
                        },
                        location: Location {
                            path: (*path).to_owned(),
                            extension: "FIT".to_owned(),
                        },
                        transfer_direction: (*direction).to_owned(),
                    },
                })
                .collect(),
            update_files: update_files
                .iter()
                .map(|(part_number, major, minor, file_name)| UpdateFile {
                    part_number: (*part_number).to_owned(),
                    version: Version {
                        major: *major,
                        minor: *minor,
                    },
                    path: "Garmin".to_owned(),
                    file_name: (*file_name).to_owned(),
                })
                .collect(),
        },
    })
}
