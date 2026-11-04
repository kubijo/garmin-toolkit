use futures_lite::future::block_on;
use garmin_fit::fixture;
use garmin_model::{
    artifact::AcquisitionOperationId,
    identity::{Source, SourceId},
    observation::ObservationId,
    value::Timestamp,
};
use garmin_services::{Application, FitImportRequest, FitImportResult, UserContext};
use garmin_storage::Storage;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct ExportFixture {
    root: tempfile::TempDir,
    application: Application,
    owner: UserContext,
    other: UserContext,
    observation: ObservationId,
    original: Vec<u8>,
}

async fn export_fixture() -> Result<ExportFixture, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let application = Application::new(Storage::open(root.path().join("export.sqlite3")).await?);
    let owner = UserContext::new(
        application
            .create_profile("Export owner".parse()?)
            .await?
            .id(),
    );
    let other = UserContext::new(
        application
            .create_profile("Other profile".parse()?)
            .await?
            .id(),
    );
    let source = Source::from_parts(
        SourceId::new_v4(),
        owner.user_id(),
        "Selected, synthetic files".parse()?,
        None,
    );
    let original = fixture::activity(fixture::Sport::Running)?;
    let imported = application
        .import_fit(FitImportRequest::from_parts(
            owner,
            &source,
            "synthetic-original.fit".parse()?,
            AcquisitionOperationId::new_v4(),
            Timestamp::from_unix_milliseconds(1_788_198_400_123)?,
            &original,
        ))
        .await?;
    let FitImportResult::Imported { observations, .. } = imported else {
        return Err("synthetic activity import failed".into());
    };
    let observation = observations[0];
    Ok(ExportFixture {
        root,
        application,
        owner,
        other,
        observation,
        original,
    })
}

#[test]
fn published_corpus_round_trips_and_rejects_corruption() -> TestResult {
    block_on(async {
        let root = tempfile::tempdir()?;
        let storage = Storage::open(root.path().join("corpus.sqlite3")).await?;
        let corpus = garmin_fixtures::seed(&storage).await?;
        let application = Application::new(storage);
        let mut index = 0;
        for profile in corpus.users() {
            let actor = UserContext::new(profile.id());
            for preview in application.activities(actor).await? {
                let path = root.path().join(format!("activity-{index}.zip"));
                application
                    .export_activity(actor, preview.observation_id(), &path)
                    .await?
                    .ok_or("missing export")?;
                let bytes = std::fs::read(&path)?;
                let parsed = garmin_services::activity_export::read(std::io::Cursor::new(&bytes))?;
                assert_eq!(parsed.selected, preview.observation_id());
                assert!(!parsed.originals.is_empty());
                assert!(
                    garmin_services::activity_export::read(std::io::Cursor::new(
                        &bytes[..bytes.len() / 2]
                    ))
                    .is_err()
                );
                for corruption in [
                    Corruption::MissingName,
                    Corruption::Version,
                    Corruption::Path,
                    Corruption::Hash,
                    Corruption::PrimaryKey,
                    Corruption::ForeignKey,
                    Corruption::DuplicateRow,
                    Corruption::UnlistedFile,
                ] {
                    let damaged = corrupt_package(&bytes, corruption)?;
                    assert!(
                        garmin_services::activity_export::read(std::io::Cursor::new(damaged))
                            .is_err(),
                        "accepted {corruption:?}"
                    );
                }
                if let Some(directory) = std::env::var_os("GARMIN_EXPORT_CONFORMANCE_DIR") {
                    std::fs::create_dir_all(&directory)?;
                    std::fs::copy(
                        path,
                        std::path::Path::new(&directory).join(format!("corpus-{index}.zip")),
                    )?;
                }
                index += 1;
            }
        }
        assert!(index >= 8);
        application.close().await;
        Ok(())
    })
}

#[derive(Clone, Copy, Debug)]
enum Corruption {
    MissingName,
    Version,
    Path,
    Hash,
    PrimaryKey,
    ForeignKey,
    DuplicateRow,
    UnlistedFile,
}

fn corrupt_package(
    bytes: &[u8],
    corruption: Corruption,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use sha2::{Digest as _, Sha256};
    use std::io::{Read as _, Write as _};
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut contents = std::collections::BTreeMap::new();
    for index in 0..source.len() {
        let mut file = source.by_index(index)?;
        let name = file.name()?.into_owned();
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        contents.insert(name, content);
    }
    let mut descriptor: serde_json::Value = serde_json::from_slice(
        contents
            .get("datapackage.json")
            .ok_or("missing descriptor")?,
    )?;
    match corruption {
        Corruption::MissingName => {
            descriptor["resources"][0]
                .as_object_mut()
                .ok_or("invalid resource")?
                .remove("name");
        }
        Corruption::Version => {
            descriptor["$schema"] = "https://datapackage.org/profiles/1.0/datapackage.json".into();
        }
        Corruption::Path => descriptor["resources"][0]["path"] = "../escape.fit".into(),
        Corruption::Hash => descriptor["resources"][0]["hash"] = "sha256:bad".into(),
        Corruption::UnlistedFile => {
            contents.insert("extra.csv".to_owned(), b"private data".to_vec());
        }
        Corruption::PrimaryKey | Corruption::ForeignKey | Corruption::DuplicateRow => {
            let resource = descriptor["resources"]
                .as_array_mut()
                .ok_or("missing resources")?
                .iter_mut()
                .find(|resource| resource["name"] == "samples")
                .ok_or("missing samples")?;
            match corruption {
                Corruption::PrimaryKey => {
                    resource["schema"]["primaryKey"] = serde_json::json!(["position"]);
                }
                Corruption::ForeignKey => resource["schema"]["foreignKeys"] = serde_json::json!([]),
                Corruption::DuplicateRow => {
                    let data = contents
                        .get_mut("data/samples.csv")
                        .ok_or("missing samples")?;
                    let first_row = data
                        .split(|byte| *byte == b'\n')
                        .nth(1)
                        .ok_or("missing row")?
                        .to_vec();
                    data.extend(first_row);
                    data.push(b'\n');
                    resource["bytes"] = data.len().into();
                    resource["hash"] =
                        format!("sha256:{}", hex::encode(Sha256::digest(&data))).into();
                }
                _ => unreachable!(),
            }
        }
    }
    contents.insert(
        "datapackage.json".to_owned(),
        serde_json::to_vec(&descriptor)?,
    );
    let mut output = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in contents {
        output.start_file(name, zip::write::SimpleFileOptions::default())?;
        output.write_all(&content)?;
    }
    Ok(output.finish()?.into_inner())
}

#[test]
fn activity_package_is_owned_and_never_replaces_an_existing_file() -> TestResult {
    block_on(async {
        let ExportFixture {
            root,
            application,
            owner,
            other,
            observation,
            original,
        } = export_fixture().await?;
        let output = root.path().join("activity.zip");
        assert!(
            application
                .export_activity(other, observation, &output)
                .await?
                .is_none()
        );
        assert!(!output.exists());
        assert!(
            application
                .export_activity(owner, ObservationId::new_v4(), &output)
                .await?
                .is_none()
        );
        assert!(
            application
                .export_activity(owner, observation, &output)
                .await?
                .is_some()
        );
        let first = std::fs::read(&output)?;
        let package = garmin_services::activity_export::read(std::io::Cursor::new(&first))?;
        assert_eq!(package.selected, observation);
        assert_eq!(package.activities.len(), 1);
        assert_eq!(package.originals.values().collect::<Vec<_>>(), [&original]);
        assert_eq!(
            package.provenance[0].acquisition.source_label,
            "Selected, synthetic files"
        );
        let normalized = garmin_fit::normalize_activities(&original)?;
        let expected = normalized.activities().next().ok_or("missing activity")?;
        assert_eq!(package.samples.len(), expected.track().len());
        assert_eq!(package.laps.len(), expected.laps().len());
        for (row, point) in package.samples.iter().zip(expected.track()) {
            assert_eq!(row.timestamp_ms, point.timestamp().as_unix_milliseconds());
            assert_eq!(
                row.distance_mm,
                point.distance().map(|value| value.as_millimeters())
            );
            assert_eq!(
                row.latitude_degrees,
                point
                    .coordinate()
                    .map(|value| value.latitude().as_degrees())
            );
            assert_eq!(
                row.heart_rate_bpm,
                point
                    .measurements()
                    .heart_rate()
                    .map(|value| value.as_beats_per_minute())
            );
        }
        if let Some(directory) = std::env::var_os("GARMIN_EXPORT_CONFORMANCE_DIR") {
            std::fs::create_dir_all(&directory)?;
            std::fs::copy(
                &output,
                std::path::Path::new(&directory).join("synthetic.zip"),
            )?;
        }
        assert!(
            application
                .export_activity(owner, observation, &output)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&output)?, first);
        application.close().await;
        Ok(())
    })
}

#[test]
fn associated_recordings_preserve_originals_after_restart_and_remain_scoped() -> TestResult {
    block_on(async {
        let ExportFixture {
            root,
            application,
            owner,
            other,
            observation,
            original,
        } = export_fixture().await?;
        let chained = fixture::activity_settings_activity()?;
        for (actor, bytes) in [(owner, chained.as_slice()), (other, original.as_slice())] {
            let source = Source::from_parts(
                SourceId::new_v4(),
                actor.user_id(),
                "Another source".parse()?,
                None,
            );
            application
                .import_fit(FitImportRequest::from_parts(
                    actor,
                    &source,
                    "../../source, \"quoted\".fit".parse()?,
                    AcquisitionOperationId::new_v4(),
                    Timestamp::from_unix_milliseconds(1_788_198_400_123)?,
                    bytes,
                ))
                .await?;
        }
        application.close().await;
        let reopened = Application::new(Storage::open(root.path().join("export.sqlite3")).await?);
        let associated = root.path().join("associated.zip");
        let report = reopened
            .export_activity(owner, observation, &associated)
            .await?
            .ok_or("missing associated export")?;
        assert_eq!(report.observations, 2);
        assert_eq!(report.originals, 2);
        let package = garmin_services::activity_export::read(std::fs::File::open(&associated)?)?;
        assert_eq!(package.originals.len(), 2);
        assert!(package.originals.values().any(|bytes| bytes == &original));
        assert!(package.originals.values().any(|bytes| bytes == &chained));
        if let Some(directory) = std::env::var_os("GARMIN_EXPORT_CONFORMANCE_DIR") {
            std::fs::create_dir_all(&directory)?;
            std::fs::copy(
                &associated,
                std::path::Path::new(&directory).join("associated.zip"),
            )?;
        }
        reopened.close().await;
        Ok(())
    })
}
