use crate::{DeviceManifest, DeviceSummary, TransportKind, capabilities};
use thiserror::Error;

pub(crate) const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("GarminDevice.xml exceeds 4 MB")]
    TooLarge,
    #[error(transparent)]
    Capabilities(#[from] capabilities::Error),
}

/// Parse the exact manifest into public metadata and validated capabilities.
/// # Errors
/// [`ManifestError`] when the XML is oversized, malformed, unsupported, or unsafe.
pub fn parse_manifest(
    raw_xml: &str,
    transport: TransportKind,
    location: String,
) -> Result<DeviceManifest, ManifestError> {
    if raw_xml.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    let capabilities = capabilities::parse(raw_xml)?;
    let model = capabilities.model();

    let summary = DeviceSummary {
        transport,
        model: model.description().to_owned(),
        part_number: model.part_number().map(ToOwned::to_owned),
        software_version: Some(model.software_version().to_string()),
        location,
    };
    Ok(DeviceManifest::new(summary, capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_redacts_identity() {
        let xml = indoc::indoc! {r#"
            <Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2">
              <Model>
                <PartNumber>006-TEST-01</PartNumber>
                <SoftwareVersion>9901</SoftwareVersion>
                <Description>Example Device</Description>
              </Model>
              <Id>1234567890</Id>
              <MassStorageMode />
            </Device>
        "#}
        .to_owned();
        let manifest =
            parse_manifest(&xml, TransportKind::MassStorage, "/media/EDGE".to_owned()).unwrap();
        assert_eq!(manifest.summary.model, "Example Device");
        assert_eq!(manifest.identity_digest().len(), 32);
        assert!(
            !serde_json::to_string(&manifest.summary)
                .unwrap()
                .contains("1234567890")
        );
    }
}
