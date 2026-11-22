use std::process::Command;

use garmin_fixtures::firmware::{EDGE_32_20, FENIX_24_20, FENIX_GSP_22_44, STORAGE_ROUTING};
use garmin_simulator::{MOCK_DEVICE_MARKER, MOCK_DEVICE_MARKER_CONTENT, MockServer};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stage_and_confirm_a_watch_across_cli_processes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("device");
    std::fs::create_dir(&root).unwrap();
    let manifest = root.join("GarminDevice.xml");
    std::fs::write(&manifest, FENIX_GSP_22_44.inventory).unwrap();
    let server = MockServer::start().await.unwrap();
    let run_on = |path: &std::path::Path, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_garmin-cli"))
            .env("GARMIN_TOOLKIT_STATE_DIR", directory.path().join("state"))
            .env("GARMIN_TOOLKIT_CACHE_DIR", directory.path().join("cache"))
            .args([
                "--json",
                "--mock-server",
                server.base_url().as_str(),
                "firmware",
            ])
            .args(args)
            .arg("--path")
            .arg(path)
            .output()
            .unwrap()
    };
    let run = |args: &[&str]| run_on(&root, args);
    let result = |args: &[&str]| -> serde_json::Value {
        let output = run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let stage = [
        "stage",
        "--part",
        "006-V4532-00",
        "--confirm-device-write",
        "WRITE-DEVICE",
    ];
    assert!(!run(&stage).status.success());
    std::fs::write(root.join(MOCK_DEVICE_MARKER), MOCK_DEVICE_MARKER_CONTENT).unwrap();
    assert!(!run(&stage[..3]).status.success());
    assert_eq!(result(&stage)["state"], "ready_to_eject");
    assert_eq!(
        result(&["status"])[0]["components"][0]["status"],
        "unchanged"
    );
    std::fs::write(&manifest, FENIX_24_20.inventory).unwrap();
    assert_eq!(
        result(&["status", "--acknowledge"])[0]["state"],
        "confirmed"
    );
    assert_eq!(result(&["status"]), serde_json::json!([]));
    std::fs::write(&manifest, STORAGE_ROUTING).unwrap();
    let mut stage = stage.to_vec();
    assert!(!run(&stage).status.success());
    stage.extend(["--storage-label", "Card"]);
    assert!(!run(&stage).status.success());
    *stage.last_mut().unwrap() = "Internal";
    assert_eq!(result(&stage)["state"], "ready_to_eject");
    assert!(root.join("Garmin/GUPDATE.GSP").is_file());
    assert!(root.join("Garmin/UpdateMetadata/006v453200.json").is_file());
    result(&["status", "--acknowledge"]);
    let internal = directory.path().join("Internal");
    std::fs::rename(&root, &internal).unwrap();
    assert!(
        run_on(&internal, &stage[..stage.len() - 2])
            .status
            .success()
    );
    server.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_reads_the_selected_device_and_prints_the_local_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let path = root.join("GarminDevice.xml");
    std::fs::write(&path, EDGE_32_20.inventory).unwrap();
    let server = MockServer::start().await.unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_garmin-cli"))
        .env("GARMIN_TOOLKIT_STATE_DIR", root.join("state"))
        .args([
            "--json",
            "--mock-server",
            server.base_url().as_str(),
            "firmware",
            "check",
            "--path",
        ])
        .arg(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let components: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(components.len(), 4);
    assert_eq!(components[1]["system_bundle"], true);
    assert_eq!(components[1]["installed"], "32.20");
    assert_eq!(components[1]["available"], "33.22");
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("example.invalid")
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), EDGE_32_20.inventory);
    server.shutdown().await.unwrap();
}
