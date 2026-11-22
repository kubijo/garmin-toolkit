use garmin_device::storage::{DeviceRead as _, DirectoryDevice};
use garmin_device::{DeviceManifest, TransportKind, parse_manifest};
use garmin_fixtures::firmware::{
    AMBIGUOUS_DESTINATION, EDGE_31_29, EDGE_32_20, FENIX_22_44, FENIX_24_20, FENIX_GSP_22_44,
    FONT_METADATA, GSP_METADATA, STORAGE_ROUTING, VIDEO_METADATA,
};
use garmin_model::{artifact::ByteCount, map::MapVersion};
use garmin_update::firmware::{CatalogError, FirmwareCatalog, ReleaseDate, UpdateMetadata};
use garmin_update::{
    firmware::{PreflightError, staging_targets},
    space::SpaceError,
};
use md5::{Digest as _, Md5};
use url::Url;

fn inventory(xml: &str) -> DeviceManifest {
    parse_manifest(xml, TransportKind::MassStorage, "fixture".into()).unwrap()
}

fn catalog(body: &str) -> FirmwareCatalog {
    serde_json::from_str(body).unwrap()
}

#[test]
fn edge_offers_include_the_bundle_and_three_independent_components() {
    for capture in [EDGE_32_20, EDGE_31_29] {
        let manifest = inventory(capture.inventory);
        let response = catalog(capture.catalog);
        let components = response.components(manifest.capabilities()).unwrap();
        let names: Vec<_> = components
            .iter()
            .map(|c| c.offer.product_name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Time Zone Map",
                "Edge 1050 Software Bundle",
                "Audio Prompt Bundle Schema 3",
                "Etyp"
            ]
        );
        assert_eq!(
            components.iter().filter(|c| c.is_system_bundle()).count(),
            1
        );
        let bundle = &components[1];
        assert!(bundle.is_system_bundle());
        assert!(!bundle.offer.is_primary_firmware);
        assert_eq!(bundle.offer.display_version.as_deref(), Some("33.22"));
        assert_eq!(
            bundle.offer.deliverable.as_ref().unwrap().size_in_bytes,
            ByteCount::from_bytes(97_688_531)
        );
        assert!(components.iter().all(|c| c.metadata.is_none()));
        assert!(matches!(bundle.offer.release_date, ReleaseDate::Instant(_)));
        assert!(matches!(
            components[3].offer.release_date,
            ReleaseDate::Local(_)
        ));
        assert_eq!(
            components[3].offer.semantic_version.to_string(),
            "2.70.9999"
        );
    }
}

#[test]
fn the_firmware_badge_requires_a_system_upgrade() {
    let manifest = inventory(FENIX_GSP_22_44.inventory);
    let mut response = catalog(FENIX_GSP_22_44.catalog);
    response
        .updates
        .retain(|offer| offer.data_type != "UpdateMetadata");
    for (version, reinstall, expected) in [
        ((24, 20), false, true),
        ((24, 20), true, false),
        ((22, 44), false, false),
        ((21, 99), false, false),
    ] {
        let offer = &mut response.updates[0];
        offer.semantic_version.major = version.0;
        offer.semantic_version.minor = version.1;
        offer.installation.is_reinstall = reinstall;
        let components = response.components(manifest.capabilities()).unwrap();
        assert_eq!(components[0].offers_system_upgrade(), expected);
        assert!(
            components[1..]
                .iter()
                .all(|item| !item.offers_system_upgrade())
        );
    }
}

#[test]
fn wired_offers_require_supported_instructions_for_both_deliverables() {
    let manifest = inventory(FENIX_GSP_22_44.inventory);
    let mut response = catalog(FENIX_GSP_22_44.catalog);
    let metadata = response
        .updates
        .iter()
        .position(|o| o.part_number == "006-V4532-00-META")
        .unwrap();
    let snapshot = |catalog: &FirmwareCatalog| {
        catalog.components(manifest.capabilities()).unwrap()[0]
            .snapshot(None)
            .unwrap()
    };
    assert!(snapshot(&response).download_bytes.is_some());
    response.updates[metadata]
        .installation
        .instructions
        .push(serde_json::json!({"action": "unknown"}));
    assert!(snapshot(&response).download_bytes.is_none());
    response.updates[metadata].installation.instructions.clear();
    response.updates[metadata]
        .delivery_restrictions
        .push("USB".into());
    assert!(snapshot(&response).download_bytes.is_none());
    assert!(
        snapshot(&response)
            .staging_issue
            .unwrap()
            .to_string()
            .contains("USB")
    );
}

#[test]
fn watch_groups_each_payload_with_the_metadata_for_its_transition() {
    for (capture, expect_system) in [
        (FENIX_24_20, false),
        (FENIX_22_44, false),
        (FENIX_GSP_22_44, true),
    ] {
        let manifest = inventory(capture.inventory);
        let response = catalog(capture.catalog);
        let components = response.components(manifest.capabilities()).unwrap();
        assert_eq!(components.len(), if expect_system { 3 } else { 2 });
        assert_eq!(
            components.iter().filter(|c| c.is_system_bundle()).count(),
            usize::from(expect_system)
        );
        for component in components {
            let metadata = component.metadata.unwrap();
            let payload = match component.offer.part_number.as_str() {
                "006-V4532-00" => GSP_METADATA,
                "006-DB775-00" => VIDEO_METADATA,
                "006-D9971-49" => FONT_METADATA,
                other => panic!("unexpected component {other}"),
            };
            let mut transition: UpdateMetadata = serde_json::from_str(payload).unwrap();
            let compact = serde_json::to_vec(&transition).unwrap();
            let delivery = metadata.deliverable.as_ref().unwrap();
            assert_eq!(
                ByteCount::from_bytes(compact.len() as u64),
                delivery.size_in_bytes
            );
            assert_eq!(hex::encode(Md5::digest(&compact)), delivery.md5);
            assert!(component.matches_transition(&transition));
            assert_eq!(metadata.semantic_version, component.offer.semantic_version);
            transition.source_version = "99.99".into();
            assert!(!component.matches_transition(&transition));
        }
    }
}

#[test]
fn catalog_requires_a_unique_part_and_version_for_each_companion() {
    let manifest = inventory(FENIX_GSP_22_44.inventory);
    let device = manifest.capabilities();
    let mut response = catalog(FENIX_GSP_22_44.catalog);
    response.updates.reverse();
    assert_eq!(response.components(device).unwrap().len(), 3);
    response.updates.pop();
    assert!(matches!(
        response.components(device),
        Err(CatalogError::UnmatchedMetadata(..))
    ));
    let mut response = catalog(FENIX_GSP_22_44.catalog);
    response.updates[3].semantic_version.minor += 1;
    assert!(matches!(
        response.components(device),
        Err(CatalogError::UnmatchedMetadata(..))
    ));
    let mut response = catalog(FENIX_GSP_22_44.catalog);
    response
        .updates
        .push(catalog(FENIX_GSP_22_44.catalog).updates.remove(0));
    assert!(matches!(
        response.components(device),
        Err(CatalogError::DuplicateOffer(..))
    ));
    let empty: FirmwareCatalog =
        serde_json::from_value(serde_json::json!({ "updates": [] })).unwrap();
    assert!(empty.components(device).unwrap().is_empty());
}

#[test]
fn inventory_disambiguates_by_destination_and_preserves_declared_storage() {
    let response = catalog(FENIX_GSP_22_44.catalog);
    let manifest = inventory(STORAGE_ROUTING);
    let components = response.components(manifest.capabilities()).unwrap();
    let matched = components[0].installed.unwrap();
    assert_eq!(matched.version, MapVersion::new(22, 44));
    assert_eq!(
        manifest
            .capabilities()
            .update_storages(&matched.part_number),
        ["Internal"]
    );
    assert_eq!(
        manifest.capabilities().update_storages("006-V4532-00-META"),
        ["Internal"]
    );
    let duplicate = inventory(AMBIGUOUS_DESTINATION);
    assert!(matches!(
        response.components(duplicate.capabilities()),
        Err(CatalogError::AmbiguousInventory(..))
    ));
}

#[tokio::test]
async fn preflight_rejects_an_oversized_selection_and_colliding_destinations() {
    let root = tempfile::tempdir().unwrap();
    let device = DirectoryDevice::with_storage(root.path().to_owned(), "internal", "Internal");
    let manifest = inventory(FENIX_GSP_22_44.inventory);
    let mut response = catalog(FENIX_GSP_22_44.catalog);
    let (_, free) = device.state().await.unwrap().storages[0]
        .capacity
        .bytes()
        .unwrap();
    response.updates[0]
        .deliverable
        .as_mut()
        .unwrap()
        .size_in_bytes = free.checked_add(ByteCount::from_gibibytes(1)).unwrap();
    assert!(matches!(
        staging_targets(&response.updates, manifest.capabilities(), &device).await,
        Err(PreflightError::Space(SpaceError::Insufficient { .. }))
    ));
    response.updates[1].installation.path = "garmin".into();
    response.updates[1].installation.file_name = "gupdate.gsp".into();
    assert!(matches!(
        staging_targets(&response.updates, manifest.capabilities(), &device).await,
        Err(PreflightError::DuplicateDestination(_))
    ));
    assert_eq!(root.path().read_dir().unwrap().count(), 0);
}

#[test]
fn deliverables_require_a_valid_digest_and_the_selected_origin() {
    let base = Url::parse("http://127.0.0.1:8123/").unwrap();
    let mut response = catalog(FENIX_GSP_22_44.catalog);
    let offer = &mut response.updates[0];
    assert!(offer.download_spec(None).is_some());
    assert!(offer.download_spec(Some(&base)).is_none());
    offer.deliverable.as_mut().unwrap().secure_download_url = Some(base.join("bundle").unwrap());
    assert!(offer.download_spec(Some(&base)).is_some());
    assert!(offer.download_spec(None).is_none());
    offer.deliverable.as_mut().unwrap().md5 = "truncated".into();
    assert!(offer.download_spec(Some(&base)).is_none());
}

#[tokio::test]
async fn metadata_checksum_covers_the_bytes_used_for_transition_validation() {
    let response = catalog(FENIX_GSP_22_44.catalog);
    let manifest = inventory(FENIX_GSP_22_44.inventory);
    let components = response.components(manifest.capabilities()).unwrap();
    let spec = components[0].metadata.unwrap().download_spec(None).unwrap();
    let mut record: UpdateMetadata = serde_json::from_str(GSP_METADATA).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(&spec.cache_name);
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(
        components[0]
            .matches_transition(&UpdateMetadata::read_verified(&path, &spec).await.unwrap())
    );
    "22.45".clone_into(&mut record.source_version);
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(matches!(
        UpdateMetadata::read_verified(&path, &spec).await,
        Err(garmin_update::firmware::MetadataError::Integrity)
    ));
}
