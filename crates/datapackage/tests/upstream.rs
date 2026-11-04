//! Compatibility with actual pinned Frictionless fixtures; no application dependencies.

use std::{collections::BTreeMap, io::Cursor, path::PathBuf};

use datapackage::{Descriptor, IssueCode, Limits, Package, Writer};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn upstream() -> PathBuf {
    // Cargo and nextest set the package directory as the working directory. Resolving
    // fixtures at runtime also keeps relocated package tests independent of build caches.
    PathBuf::from("tests/upstream")
}

#[derive(Deserialize)]
struct Manifest {
    files: BTreeMap<String, String>,
}

#[test]
fn fixtures_and_python_reference_are_byte_exact() -> TestResult {
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(upstream().join("manifest.json"))?)?;
    for (path, expected) in manifest.files {
        assert_eq!(
            hex::encode(Sha256::digest(std::fs::read(upstream().join(&path))?)),
            expected,
            "{path}"
        );
    }
    Ok(())
}

#[test]
fn upstream_tables_and_extensions_round_trip_through_zip() -> TestResult {
    let mut package = Package::read_directory(
        &upstream().join("data/package/datapackage.json"),
        Limits::default(),
    )?;
    assert!(package.validate().valid());
    package.descriptor.metadata.insert(
        "application".to_owned(),
        serde_json::json!({"extension": true}),
    );
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    for resource in package.resources {
        writer.add(resource.descriptor, &resource.bytes)?;
    }
    let bytes = writer
        .finish(Descriptor {
            resources: Vec::new(),
            ..package.descriptor
        })?
        .into_inner();
    let roundtrip = Package::read_zip(Cursor::new(bytes), Limits::default())?;
    assert!(roundtrip.validate().valid());
    assert_eq!(
        roundtrip.descriptor.metadata["application"],
        serde_json::json!({"extension": true})
    );
    assert_eq!(roundtrip.resources.len(), 2);
    Ok(())
}

#[test]
fn upstream_invalid_rows_produce_the_same_issue_codes() -> TestResult {
    let package = Package::read_directory(
        &upstream().join("data/invalid/datapackage.json"),
        Limits::default(),
    )?;
    let mut codes = package
        .validate()
        .issues
        .into_iter()
        .map(|issue| issue.code)
        .collect::<Vec<_>>();
    codes.sort();
    let mut expected = vec![
        IssueCode::BlankRow,
        IssueCode::PrimaryKey,
        IssueCode::BlankRow,
    ];
    expected.sort();
    assert_eq!(codes, expected);
    Ok(())
}

#[test]
fn unsupported_constraints_are_reported_instead_of_certified() -> TestResult {
    let descriptor = br#"{"resources":[{"name":"data","data":[["id"],[1]],"schema":{"fields":[{"name":"id","type":"integer","constraints":{"minimum":2}}]}}]}"#;
    let package = Package::resolve(descriptor, Limits::default(), |_| unreachable!())?;
    assert!(
        package
            .validate()
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::Unsupported)
    );
    Ok(())
}
