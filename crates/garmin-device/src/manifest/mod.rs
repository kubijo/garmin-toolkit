//! Detection, parsing, and normalization of device metadata documents.

mod error;
mod garmin;
mod model;

use std::path::Path;

use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};

use crate::{DeviceManifest, DeviceSummary, TransportKind};

pub use error::ManifestError;
pub use model::{
    DataType, DeviceId, DeviceModel, FileCapability, ManifestFormat, ManifestQuirk,
    OperationHandle, ParsedManifest, SoftwareVersion, TransferDirection,
};

pub(crate) const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

/// Parse a metadata document into transport-neutral device metadata.
pub(crate) fn parse_document(raw_document: &str) -> Result<ParsedManifest, ManifestError> {
    if raw_document.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    match detect_format(raw_document)? {
        ManifestFormat::GarminDeviceV2 => garmin::v2::parse(raw_document),
    }
}

/// Parse exact device metadata into its public transport presentation.
/// # Errors
/// [`ManifestError`] when the document is oversized, malformed, unsupported, unsafe, or ambiguous.
pub fn parse_manifest(
    raw_document: &str,
    transport: TransportKind,
    location: String,
) -> Result<DeviceManifest, ManifestError> {
    let parsed = parse_document(raw_document)?;
    let model = parsed.model();
    let summary = DeviceSummary {
        transport,
        model: model.description().to_owned(),
        part_number: model.part_number().map(ToOwned::to_owned),
        software_version: Some(model.software_version().to_string()),
        location,
    };
    Ok(DeviceManifest::new(summary, parsed))
}

pub(crate) fn is_device_manifest(path: &Path) -> bool {
    garmin::is_device_manifest(path)
}

fn detect_format(raw_document: &str) -> Result<ManifestFormat, ManifestError> {
    let mut reader = NsReader::from_str(raw_document);
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| ManifestError::MalformedDocument(error.to_string()))?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let root = element.local_name().as_ref().to_owned();
                let namespace = match namespace {
                    ResolveResult::Bound(namespace) => namespace.as_ref().to_owned(),
                    ResolveResult::Unbound => String::new(),
                    ResolveResult::Unknown(prefix) => {
                        return Err(ManifestError::MalformedDocument(format!(
                            "root element uses unknown namespace prefix {prefix:?}"
                        )));
                    }
                };
                return match (root.as_str(), namespace.as_str()) {
                    ("Device", garmin::v2::NAMESPACE) => Ok(ManifestFormat::GarminDeviceV2),
                    _ => Err(ManifestError::UnsupportedFormat { root, namespace }),
                };
            }
            Event::Eof => {
                return Err(ManifestError::MalformedDocument(
                    "document has no root element".to_owned(),
                ));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_registered_garmin_v2_format() {
        let xml = r#"<Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2" />"#;
        assert_eq!(detect_format(xml).unwrap(), ManifestFormat::GarminDeviceV2);
    }

    #[test]
    fn rejects_an_unregistered_namespace() {
        let xml = r#"<Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v3" />"#;
        assert!(matches!(
            detect_format(xml),
            Err(ManifestError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn rejects_a_different_root_in_the_registered_namespace() {
        let xml = r#"<Other xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2" />"#;
        assert!(matches!(
            detect_format(xml),
            Err(ManifestError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn rejects_a_document_without_a_parseable_root() {
        assert!(matches!(
            detect_format("<"),
            Err(ManifestError::MalformedDocument(_))
        ));
    }

    #[test]
    fn rejects_an_oversized_document_before_detection() {
        let xml = " ".repeat(MAX_MANIFEST_BYTES + 1);
        assert!(matches!(parse_document(&xml), Err(ManifestError::TooLarge)));
    }

    #[test]
    fn parses_and_redacts_identity() {
        let xml = indoc::indoc! {r#"
            <Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2">
              <Model>
                <PartNumber>006-TEST-01</PartNumber>
                <SoftwareVersion>9901</SoftwareVersion>
                <Description>Mock Storage-o-Matic 9000</Description>
              </Model>
              <Id>1234567890</Id>
              <MassStorageMode />
            </Device>
        "#};
        let manifest =
            parse_manifest(xml, TransportKind::MassStorage, "/media/MOCK".to_owned()).unwrap();

        assert_eq!(manifest.summary.model, "Mock Storage-o-Matic 9000");
        assert_eq!(manifest.format(), ManifestFormat::GarminDeviceV2);
        assert_eq!(manifest.raw_xml(), xml);
        assert_eq!(manifest.identity_digest().len(), 32);
        assert!(
            !serde_json::to_string(&manifest.summary)
                .unwrap()
                .contains("1234567890")
        );
    }
}
