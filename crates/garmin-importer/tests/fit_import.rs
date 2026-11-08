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
fn preserves_multisession_rejections_without_partial_activities() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("multisession.sqlite3")).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let importer = FitImporter::new(&storage);
        for header in [
            fixture::integrity::Header::Legacy,
            fixture::integrity::Header::Checksummed,
        ] {
            let bytes = fixture::integrity::multisession(header);
            rejected_original(&importer, &storage, &user, &source, &bytes, "multisession").await?;
            let mut chain = fixture::integrity::activity(header);
            chain.extend(bytes);
            rejected_original(
                &importer,
                &storage,
                &user,
                &source,
                &chain,
                "multisession tail",
            )
            .await?;
            assert_eq!(storage.activities(user.id()).await?, Vec::new());
        }
        storage.close().await;
        Ok(())
    })
}

#[test]
fn preserves_every_independent_integrity_failure_without_partial_projections() -> TestResult {
    block_on(async {
        use fixture::integrity;
        use fixture::integrity::Header;

        let root = tempdir()?;
        let storage = Storage::open(root.path().join("integrity.sqlite3")).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let importer = FitImporter::new(&storage);
        let valid = integrity::activity(Header::Legacy);
        let control = importer
            .import(request(
                &user,
                &source,
                AcquisitionOperationId::new_v4(),
                &valid,
            )?)
            .await?;
        assert!(matches!(control, FitImportOutcome::Imported(_)));
        assert_eq!(
            storage
                .artifact_bytes(control.receipt().artifact_id())
                .await?,
            Some(valid.clone())
        );
        let before = storage.activities(user.id()).await?;
        assert_eq!(before.len(), 1);

        for header in Header::ALL {
            for (name, bytes) in integrity::corruptions(header) {
                rejected_original(
                    &importer,
                    &storage,
                    &user,
                    &source,
                    &bytes,
                    &format!("{header:?}: {name}"),
                )
                .await?;
            }
        }
        // This valid protocol form is currently rejected by rustyfit; preserve it too.
        rejected_original(
            &importer,
            &storage,
            &user,
            &source,
            &integrity::activity(Header::ZeroCrc),
            "valid zero-header CRC",
        )
        .await?;
        let mut chained = valid;
        let mut tail = integrity::activity(Header::Checksummed);
        *tail.last_mut().ok_or("missing synthetic file CRC")? ^= 1;
        chained.extend(tail);
        rejected_original(
            &importer,
            &storage,
            &user,
            &source,
            &chained,
            "valid first sequence, corrupt second sequence",
        )
        .await?;
        assert_eq!(storage.activities(user.id()).await?, before);
        storage.close().await;
        Ok(())
    })
}

async fn rejected_original(
    importer: &FitImporter<'_>,
    storage: &Storage,
    user: &User,
    source: &Source,
    bytes: &[u8],
    case: &str,
) -> TestResult {
    let operation = AcquisitionOperationId::new_v4();
    let first = importer
        .import(request(user, source, operation, bytes)?)
        .await?;
    let second = importer
        .import(request(user, source, operation, bytes)?)
        .await?;
    assert_eq!(first, second, "{case}");
    let FitImportOutcome::Rejected { receipt, failure } = first else {
        return Err(format!("{case} unexpectedly normalized").into());
    };
    assert!(!failure.as_str().is_empty(), "{case}");
    assert!(receipt.observation_ids().is_empty(), "{case}");
    assert_eq!(
        storage.artifact_bytes(receipt.artifact_id()).await?,
        Some(bytes.to_vec()),
        "{case}"
    );
    Ok(())
}

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
        assert_ne!(failure.as_str(), "");
        assert_eq!(receipt.observation_ids(), []);
        assert_eq!(
            storage.artifact_bytes(receipt.artifact_id()).await?,
            Some(bytes.to_vec())
        );
        assert_eq!(
            storage.activities(user.id()).await?,
            [] as [garmin_storage::StoredActivitySummary; 0]
        );

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
        assert_eq!(
            storage.activities(user.id()).await?,
            [] as [garmin_storage::StoredActivitySummary; 0]
        );

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
    let device = Device::from_parts(DeviceId::new_v4(), "Mock Watch-o-Matic 9000".parse()?);
    let source = Source::from_parts(
        SourceId::new_v4(),
        user.id(),
        "Mock watch source".parse()?,
        Some(device.id()),
    );
    Ok((user, device, source))
}
