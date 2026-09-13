mod normalize;
mod wire;

use crate::manifest::{ManifestError, ManifestFormat, ParsedManifest};

pub(in crate::manifest) const NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";

pub(in crate::manifest) fn parse(xml: &str) -> Result<ParsedManifest, ManifestError> {
    let raw = wire::deserialize(xml)?;
    if raw.namespace != NAMESPACE {
        return Err(ManifestError::UnsupportedFormat {
            root: "Device".to_owned(),
            namespace: raw.namespace,
        });
    }
    let parsed = normalize::normalize(raw, xml)?;
    debug_assert_eq!(parsed.format(), ManifestFormat::GarminDeviceV2);
    Ok(parsed)
}
