use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub fn load(directory: &Path) -> Value {
    let input = directory.join("openapi.json");
    let pin = directory.join("input.json");
    println!("cargo:rerun-if-changed={}", input.display());
    println!("cargo:rerun-if-changed={}", pin.display());
    let bytes = fs::read(input).expect("pinned Garmin descriptor");
    let pin: Value = serde_json::from_slice(&fs::read(pin).expect("input pin")).expect("valid pin");
    let digest = hex::encode(Sha256::digest(&bytes));
    assert_eq!(
        digest,
        pin["sha256"].as_str().expect("descriptor digest"),
        "descriptor checksum mismatch"
    );
    let spec: Value = serde_json::from_slice(&bytes).expect("valid descriptor JSON");
    assert_eq!(spec["openapi"], "3.0.1");
    assert_eq!(spec["info"]["title"], "Garmin Connect Personal API");
    spec
}

pub fn binary_download(mut spec: Value) -> Value {
    let responses =
        spec["paths"]["/download-service/files/activity/{activityId}"]["get"]["responses"]
            .as_object_mut()
            .expect("download responses");
    let response = responses
        .remove("default")
        .expect("untyped download response");
    assert!(
        !responses.contains_key("200"),
        "review changed download statuses"
    );
    responses.insert("200".into(), response);
    let media = &mut responses.get_mut("200").expect("download success response")["content"]["application/x-zip-compressed"];
    assert_eq!(*media, json!({}), "review changed binary response schema");
    *media = json!({"schema": {"type": "string", "format": "binary"}});
    spec
}
