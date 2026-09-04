//! End-to-end FIT import service contracts.

use std::error::Error;

use futures_lite::future::block_on;
use garmin_fit::fixture;
use garmin_importer::{FitImportOutcome, FitImportRequest, FitImporter, ImportError};
use garmin_model::{
    artifact::{AcquisitionOperationId, SourceIdentity},
    identity::{Device, DeviceId, Profile, Role, Source, SourceId, User, UserId},
    value::Timestamp,
};
use garmin_storage::Storage;
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
fn imports_chained_activities_and_retries_with_stable_ids() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("import.sqlite3")).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let bytes = fixture::activity_settings_activity()?;
        let operation_id = AcquisitionOperationId::new_v4();
        let importer = FitImporter::new(&storage);

        let first = importer
            .import(request(&user, &source, operation_id, &bytes)?)
            .await?;
        let second = importer
            .import(request(&user, &source, operation_id, &bytes)?)
            .await?;

        assert_eq!(first, second);
        let FitImportOutcome::Imported(receipt) = first else {
            return Err("valid chained FIT was rejected".into());
        };
        assert_eq!(receipt.observation_ids().len(), 2);
        assert_eq!(
            storage.artifact_bytes(receipt.artifact_id()).await?,
            Some(bytes)
        );
        let mut summaries = storage.activities(user.id()).await?;
        summaries.sort_unstable_by_key(garmin_storage::StoredActivitySummary::sequence_position);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].sequence_position().as_u32(), 0);
        assert_eq!(summaries[1].sequence_position().as_u32(), 2);
        assert_eq!(summaries[0].observation_id(), receipt.observation_ids()[0]);
        assert_eq!(summaries[1].observation_id(), receipt.observation_ids()[1]);

        storage.close().await;
        Ok(())
    })
}

#[test]
fn rejects_changed_retry_data_without_partial_writes() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("conflict.sqlite3")).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let bytes = fixture::activity(fixture::Sport::Running)?;
        let changed = fixture::activity(fixture::Sport::Cycling)?;
        let operation_id = AcquisitionOperationId::new_v4();
        let importer = FitImporter::new(&storage);
        let original = importer
            .import(request(&user, &source, operation_id, &bytes)?)
            .await?;

        assert!(matches!(
            importer
                .import(request(&user, &source, operation_id, &changed)?)
                .await,
            Err(ImportError::Storage(_))
        ));
        assert_eq!(
            storage
                .artifact_bytes(original.receipt().artifact_id())
                .await?,
            Some(bytes)
        );
        assert_eq!(storage.activities(user.id()).await?.len(), 1);

        storage.close().await;
        Ok(())
    })
}

#[test]
fn preserves_parser_rejection_without_projections() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("rejected.sqlite3")).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let bytes = b"truncated FIT";
        let operation_id = AcquisitionOperationId::new_v4();
        let importer = FitImporter::new(&storage);

        let first = importer
            .import(request(&user, &source, operation_id, bytes)?)
            .await?;
        let second = importer
            .import(request(&user, &source, operation_id, bytes)?)
            .await?;

        assert_eq!(first, second);
        let FitImportOutcome::Rejected { receipt, failure } = first else {
            return Err("truncated FIT unexpectedly normalized".into());
        };
        assert!(!failure.as_str().is_empty());
        assert!(receipt.observation_ids().is_empty());
        assert_eq!(
            storage.artifact_bytes(receipt.artifact_id()).await?,
            Some(bytes.to_vec())
        );
        assert!(storage.activities(user.id()).await?.is_empty());

        storage.close().await;
        Ok(())
    })
}

#[test]
fn rejects_a_source_owned_by_another_actor() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("actor.sqlite3")).await?;
        let (user, _, source) = owner_and_source()?;
        let bytes = fixture::activity(fixture::Sport::Running)?;
        let importer = FitImporter::new(&storage);

        assert!(matches!(
            importer
                .import(FitImportRequest::from_parts(
                    UserId::new_v4(),
                    &source,
                    "GARMIN/ACTIVITY/1.FIT".parse()?,
                    AcquisitionOperationId::new_v4(),
                    "2026-08-31T18:40:00Z".parse()?,
                    &bytes,
                ))
                .await,
            Err(ImportError::ActorCannotUseSource)
        ));
        assert!(storage.activities(user.id()).await?.is_empty());

        storage.close().await;
        Ok(())
    })
}

fn request<'a>(
    user: &User,
    source: &'a Source,
    operation_id: AcquisitionOperationId,
    bytes: &'a [u8],
) -> TestResult<FitImportRequest<'a>> {
    Ok(FitImportRequest::from_parts(
        user.id(),
        source,
        SourceIdentity::from_string("GARMIN/ACTIVITY/1.FIT".to_owned())?,
        operation_id,
        "2026-08-31T18:40:00Z".parse::<Timestamp>()?,
        bytes,
    ))
}

fn owner_and_source() -> TestResult<(User, Device, Source)> {
    let user = User::from_parts(
        UserId::new_v4(),
        Role::Owner,
        Profile::from_display_name("Rider".parse()?),
    );
    let device = Device::from_parts(DeviceId::new_v4(), "Watch".parse()?);
    let source = Source::from_parts(
        SourceId::new_v4(),
        user.id(),
        "USB watch".parse()?,
        Some(device.id()),
    );
    Ok((user, device, source))
}
