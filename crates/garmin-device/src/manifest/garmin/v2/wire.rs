use serde::Deserialize;

use crate::manifest::{ManifestError, ManifestFormat};

#[derive(Debug, Deserialize)]
#[serde(rename = "Device")]
pub(super) struct Device {
    #[serde(rename = "@xmlns")]
    pub(super) namespace: String,
    #[serde(rename = "Model")]
    pub(super) model: Model,
    #[serde(rename = "Id")]
    pub(super) id: u32,
    #[serde(rename = "MassStorageMode")]
    pub(super) mass_storage_mode: MassStorageMode,
}

#[derive(Debug, Deserialize)]
pub(super) struct Model {
    #[serde(default, rename = "PartNumber")]
    pub(super) part_number: Option<String>,
    #[serde(rename = "SoftwareVersion")]
    pub(super) software_version: u16,
    #[serde(rename = "Description")]
    pub(super) description: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct MassStorageMode {
    #[serde(default, rename = "$value")]
    pub(super) entries: Vec<MassStorageEntry>,
}

#[derive(Debug, Deserialize)]
pub(super) enum MassStorageEntry {
    DataType(DataType),
    UpdateFile(UpdateFile),
    #[serde(other)]
    Unsupported,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateFile {
    #[serde(rename = "PartNumber")]
    pub(super) part_number: String,
    #[serde(rename = "Version")]
    pub(super) version: MapVersion,
}

#[derive(Debug, Deserialize)]
pub(super) struct MapVersion {
    #[serde(rename = "Major")]
    pub(super) major: u64,
    #[serde(rename = "Minor")]
    pub(super) minor: u64,
}

#[derive(Debug, Deserialize)]
pub(super) struct DataType {
    #[serde(rename = "Name")]
    pub(super) name: String,
    #[serde(default, rename = "File")]
    pub(super) files: Vec<File>,
}

#[derive(Debug, Deserialize)]
pub(super) struct File {
    #[serde(rename = "Specification")]
    pub(super) specification: Specification,
    #[serde(rename = "Location")]
    pub(super) location: Location,
    #[serde(rename = "TransferDirection")]
    pub(super) transfer_direction: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Specification {
    #[serde(rename = "Identifier")]
    pub(super) identifier: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct Location {
    #[serde(rename = "Path")]
    pub(super) path: String,
    #[serde(default, rename = "BaseName")]
    pub(super) basename: Option<String>,
    #[serde(rename = "FileExtension")]
    pub(super) extension: String,
}

pub(super) fn deserialize(xml: &str) -> Result<Device, ManifestError> {
    quick_xml::de::from_str(xml).map_err(|error| ManifestError::InvalidDocument {
        format: ManifestFormat::GarminDeviceV2,
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_mixed_entry_order_and_tolerates_unknown_entries() {
        let xml = r#"
            <Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2">
              <Model><SoftwareVersion>1705</SoftwareVersion><Description>Mock Watch-o-Matic 9000</Description></Model>
              <Id>123456</Id>
              <MassStorageMode>
                <DataType><Name>FIT_TYPE_4</Name></DataType>
                <Mystery />
                <UpdateFile><PartNumber>006-D0000-00</PartNumber><Version><Major>1</Major><Minor>2</Minor></Version></UpdateFile>
                <DataType><Name>FIT_TYPE_5</Name></DataType>
              </MassStorageMode>
            </Device>
        "#;

        let raw = deserialize(xml).unwrap();
        assert!(matches!(
            raw.mass_storage_mode.entries.as_slice(),
            [
                MassStorageEntry::DataType(_),
                MassStorageEntry::Unsupported,
                MassStorageEntry::UpdateFile(_),
                MassStorageEntry::DataType(_),
            ]
        ));
    }

    #[test]
    fn reports_format_specific_deserialization_errors() {
        let xml = r#"<Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2"><Model /></Device>"#;
        assert!(matches!(
            deserialize(xml),
            Err(ManifestError::InvalidDocument {
                format: ManifestFormat::GarminDeviceV2,
                ..
            })
        ));
    }
}
