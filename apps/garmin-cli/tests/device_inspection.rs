use std::process::Command;

fn inspect(root: &std::path::Path, cache: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_garmin-cli"))
        .env("GARMIN_TOOLKIT_STATE_DIR", cache.join("state"))
        .args(["--json", "--cache-dir"])
        .arg(cache)
        .args(["device", "inspect", "--path"])
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn json_keeps_summary_fields_and_prints_partial_results_before_failing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    let cache = directory.path().join("cache");
    let _fixture = garmin_fixtures::device::Device::recreate(root.clone()).unwrap();
    let manifest_path = root.join("Garmin/GarminDevice.xml");
    let original = std::fs::read(&manifest_path).unwrap();
    let output = inspect(&root, &cache);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["model"], garmin_fixtures::device::NAME);
    for field in [
        "transport",
        "model",
        "part_number",
        "software_version",
        "location",
    ] {
        assert!(
            json.get(field).is_some(),
            "missing existing summary field {field}"
        );
    }
    let report: garmin_model::device::DeviceInspection =
        serde_json::from_value(json["inspection"].clone()).unwrap();
    assert!(!report.has_errors());
    assert_eq!(std::fs::read(&manifest_path).unwrap(), original);
    assert!(!root.join("GARMIN-TOOLKIT/manifest.toml").exists());

    std::fs::write(&manifest_path, b"malformed Garmin manifest").unwrap();
    let output = inspect(&root, &cache);
    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let report: garmin_model::device::DeviceInspection =
        serde_json::from_value(json["inspection"].clone()).unwrap();
    assert!(report.has_errors());
    assert!(matches!(
        report.storage,
        garmin_model::device::InspectionSection::Available(_)
    ));
    assert_eq!(report.toolkit.len(), 1);
    assert_eq!(
        std::fs::read(&manifest_path).unwrap(),
        b"malformed Garmin manifest"
    );
}
