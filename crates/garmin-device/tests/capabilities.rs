//! Device-manifest validation.

mod support;

use std::error::Error;

use garmin_device::{
    DataType, DeviceManifest, ManifestError, ManifestFormat, ManifestQuirk, TransferDirection,
    TransportKind, parse_manifest,
};
use garmin_model::map::{InstalledMapFile, MapVersion};

const GARMIN_DEVICE_V2_NAMESPACE: &str = "http://www.garmin.com/xmlschemas/GarminDevice/v2";

#[test]
fn exposes_installed_map_versions() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest_with_updates(
        GARMIN_DEVICE_V2_NAMESPACE,
        &[],
        &[("006-D9486-03", 24, 10, "D9486030A.img")],
    )?;

    let manifest = parse(&xml)?;
    let parsed = manifest.capabilities();

    assert_eq!(
        parsed.installed_map_files(),
        [InstalledMapFile {
            part_number: "006-D9486-03".to_owned(),
            version: MapVersion::new(24, 10),
        }]
    );
    Ok(())
}

#[test]
fn accepts_data_types_interleaved_with_update_files() -> Result<(), Box<dyn Error>> {
    let xml = format!(
        r#"<Device xmlns="{GARMIN_DEVICE_V2_NAMESPACE}">
            <Model><SoftwareVersion>1705</SoftwareVersion><Description>Mock Venu-o-Matic</Description></Model>
            <Id>123456</Id>
            <MassStorageMode>
                <DataType><Name>FIT_TYPE_4</Name><File><Specification><Identifier>FIT</Identifier></Specification><Location><Path>GARMIN/ACTIVITY</Path><FileExtension>FIT</FileExtension></Location><TransferDirection>OutputFromUnit</TransferDirection></File></DataType>
                <UpdateFile><PartNumber>006-D0000-00</PartNumber><Version><Major>1</Major><Minor>2</Minor></Version></UpdateFile>
                <DataType><Name>FIT_TYPE_5</Name><File><Specification><Identifier>FIT</Identifier></Specification><Location><Path>GARMIN/WORKOUTS</Path><FileExtension>FIT</FileExtension></Location><TransferDirection>InputOutput</TransferDirection></File></DataType>
            </MassStorageMode>
        </Device>"#
    );

    let manifest = parse(&xml)?;
    let parsed = manifest.capabilities();

    assert_eq!(parsed.capabilities().len(), 2);
    assert_eq!(parsed.capabilities()[0].data_type(), DataType::Activity);
    assert_eq!(parsed.capabilities()[1].data_type(), DataType::Workout);
    assert_eq!(parsed.installed_map_files().len(), 1);
    assert_eq!(manifest.format(), ManifestFormat::GarminDeviceV2);
    assert_eq!(
        manifest.quirks(),
        [ManifestQuirk::InterleavedMassStorageEntries]
    );
    Ok(())
}

#[test]
fn exposes_only_supported_capabilities() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest(
        GARMIN_DEVICE_V2_NAMESPACE,
        &[
            ("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit"),
            ("IQWatchApps", "../../private", "InputOutput"),
        ],
    )?;

    let manifest = parse(&xml)?;
    let parsed = manifest.capabilities();

    assert_eq!(parsed.id().as_u32(), 123_456);
    assert_eq!(parsed.model().description(), "Mock Storage-o-Matic 9000");
    assert_eq!(parsed.model().software_version().as_hundredths(), 2244);
    assert_eq!(parsed.model().software_version().to_string(), "22.44");
    assert_eq!(parsed.capabilities().len(), 1);
    assert_eq!(parsed.capabilities()[0].data_type(), DataType::Activity);
    assert_eq!(
        parsed.capabilities()[0].direction(),
        TransferDirection::OutputFromUnit
    );
    Ok(())
}

#[test]
fn rejects_parent_traversal_for_supported_data() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest(
        GARMIN_DEVICE_V2_NAMESPACE,
        &[("FIT_TYPE_4", "GARMIN/../PRIVATE", "OutputFromUnit")],
    )?;

    assert!(matches!(
        parse(&xml),
        Err(ManifestError::InvalidLocation { .. })
    ));
    Ok(())
}

#[test]
fn rejects_absolute_paths_for_supported_data() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest(
        GARMIN_DEVICE_V2_NAMESPACE,
        &[("FIT_TYPE_4", "/GARMIN/ACTIVITY", "OutputFromUnit")],
    )?;

    assert!(matches!(
        parse(&xml),
        Err(ManifestError::InvalidLocation { .. })
    ));
    Ok(())
}

#[test]
fn rejects_ambiguous_duplicate_capabilities() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest(
        GARMIN_DEVICE_V2_NAMESPACE,
        &[
            ("FIT_TYPE_4", "GARMIN/ACTIVITY", "OutputFromUnit"),
            ("FIT_TYPE_4", "GARMIN/BACKUP", "OutputFromUnit"),
        ],
    )?;

    assert!(matches!(
        parse(&xml),
        Err(ManifestError::AmbiguousCapability { .. })
    ));
    Ok(())
}

#[test]
fn rejects_an_unsupported_format() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest("http://www.garmin.com/xmlschemas/GarminDevice/v3", &[])?;

    assert!(matches!(
        parse(&xml),
        Err(ManifestError::UnsupportedFormat { .. })
    ));
    Ok(())
}

#[test]
fn exposes_declared_transfer_directions() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest(
        GARMIN_DEVICE_V2_NAMESPACE,
        &[
            ("FIT_TYPE_5", "GARMIN/WORKOUTS", "InputToUnit"),
            ("FIT_TYPE_6", "GARMIN/COURSES", "InputOutput"),
        ],
    )?;

    let manifest = parse(&xml)?;
    let parsed = manifest.capabilities();

    assert_eq!(parsed.capabilities().len(), 2);
    assert_eq!(
        parsed.capabilities()[0].direction(),
        TransferDirection::InputToUnit
    );
    assert_eq!(
        parsed.capabilities()[1].direction(),
        TransferDirection::InputOutput
    );
    Ok(())
}

fn parse(xml: &str) -> Result<DeviceManifest, ManifestError> {
    parse_manifest(
        xml,
        TransportKind::MassStorage,
        "synthetic-test-device".to_owned(),
    )
}

#[test]
fn rejects_an_unknown_transfer_direction() -> Result<(), Box<dyn Error>> {
    let xml = support::manifest(
        GARMIN_DEVICE_V2_NAMESPACE,
        &[("FIT_TYPE_4", "GARMIN/ACTIVITY", "Sideways")],
    )?;

    assert!(matches!(
        parse(&xml),
        Err(ManifestError::UnsupportedDirection { .. })
    ));
    Ok(())
}
