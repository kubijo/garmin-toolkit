//! Transactional ingestion contracts.

use std::error::Error;

use futures_lite::future::block_on;
use garmin_model::{
    artifact::{
        Acquisition, AcquisitionId, AcquisitionOperationId, Artifact, ArtifactId, MediaType,
        NormalizationFailure, NormalizationOutcome, NormalizationRun, NormalizationRunId,
        SchemaVersion,
    },
    identity::{AvatarArtifactId, Device, DeviceId, Profile, Role, Source, SourceId, User, UserId},
    observation::{
        FingerprintSchema, Observation, ObservationId, ObservationKind, SemanticFingerprint,
    },
    value::{ComponentVersion, Timestamp, Transformation},
};
use garmin_storage::{Ingestion, IngestionError, Storage};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct Fixture {
    bytes: Vec<u8>,
    artifact: Artifact,
    acquisition: Acquisition,
    normalization: NormalizationRun,
    observations: Vec<Observation>,
}

impl Fixture {
    fn succeeded(
        source: &Source,
        artifact_id: ArtifactId,
        bytes: &[u8],
        canonical_bytes: &[u8],
    ) -> TestResult<Self> {
        let bytes = bytes.to_vec();
        let artifact = Artifact::from_bytes(
            artifact_id,
            "application/vnd.ant.fit".parse::<MediaType>()?,
            &bytes,
        );
        let acquisition = Acquisition::from_source(
            AcquisitionId::new_v4(),
            AcquisitionOperationId::new_v4(),
            artifact.id(),
            source,
            "GARMIN/ACTIVITY/1.FIT".parse()?,
            "2026-08-31T12:00:00Z".parse::<Timestamp>()?,
        );
        let normalization = NormalizationRun::from_parts(
            NormalizationRunId::new_v4(),
            &acquisition,
            ComponentVersion::from_parts("fit-normalizer", "1.0.0".parse()?)?,
            SchemaVersion::from_u32(1)?,
            vec![Transformation::from_component(
                ComponentVersion::from_parts("fit-units", "1.0.0".parse()?)?,
            )],
            NormalizationOutcome::Succeeded,
        );
        let fingerprint = SemanticFingerprint::from_canonical_bytes(
            FingerprintSchema::from_component(ComponentVersion::from_parts(
                "activity-fingerprint",
                "1.0.0".parse()?,
            )?),
            canonical_bytes,
        );
        let observation = Observation::from_run(
            ObservationId::new_v4(),
            &normalization,
            ObservationKind::Measurement,
            Some("2026-08-31T11:00:00Z".parse::<Timestamp>()?),
            fingerprint,
        )?;
        Ok(Self {
            bytes,
            artifact,
            acquisition,
            normalization,
            observations: vec![observation],
        })
    }

    fn ingestion(&self) -> Result<Ingestion<'_>, IngestionError> {
        Ingestion::from_parts(
            &self.artifact,
            &self.bytes,
            &self.acquisition,
            &self.normalization,
            &self.observations,
            &[],
        )
    }
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

#[test]
fn retries_share_bytes_without_merging_observations() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let path = root.path().join("ingestion.sqlite3");
        let storage = Storage::open(&path).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;

        let first = Fixture::succeeded(
            &source,
            ArtifactId::new_v4(),
            b"same complete FIT bytes",
            b"same known activity",
        )?;
        storage.ingest(first.ingestion()?).await?;
        storage.ingest(first.ingestion()?).await?;

        let second = Fixture::succeeded(
            &source,
            ArtifactId::new_v4(),
            b"same complete FIT bytes",
            b"same known activity",
        )?;
        storage.ingest(second.ingestion()?).await?;
        assert_eq!(
            storage.artifact_bytes(first.artifact.id()).await?,
            Some(first.bytes.clone())
        );
        assert_eq!(
            storage.artifact_bytes(second.artifact.id()).await?,
            Some(second.bytes.clone())
        );
        storage.close().await;

        let options = SqliteConnectOptions::new().filename(&path);
        let mut database = SqliteConnection::connect_with(&options).await?;
        let counts = sqlx::query_file!("queries/test-ingestion-counts.sql")
            .fetch_one(&mut database)
            .await?;

        assert_eq!(counts.blob_count, 1);
        assert_eq!(counts.artifact_count, 2);
        assert_eq!(counts.acquisition_count, 2);
        assert_eq!(counts.observation_count, 2);
        assert_eq!(counts.association_count, 1);
        assert_eq!(counts.association_member_count, 2);
        database.close().await?;

        let reopened = Storage::open(&path).await?;
        assert_eq!(
            reopened.artifact_bytes(first.artifact.id()).await?,
            Some(first.bytes)
        );
        reopened.close().await;
        Ok(())
    })
}

#[test]
fn failed_normalization_and_conflicts_are_atomic() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let path = root.path().join("failure.sqlite3");
        let storage = Storage::open(&path).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;

        let original = Fixture::succeeded(
            &source,
            ArtifactId::new_v4(),
            b"original bytes",
            b"known activity",
        )?;
        storage.ingest(original.ingestion()?).await?;

        let conflicting = Fixture::succeeded(
            &source,
            original.artifact.id(),
            b"different bytes",
            b"different activity",
        )?;
        assert!(matches!(
            storage.ingest(conflicting.ingestion()?).await,
            Err(garmin_storage::Error::Ingestion(
                IngestionError::ArtifactConflict
            ))
        ));

        let failed_bytes = b"malformed complete bytes".to_vec();
        let failed_artifact = Artifact::from_bytes(
            ArtifactId::new_v4(),
            "application/vnd.ant.fit".parse()?,
            &failed_bytes,
        );
        let failed_acquisition = Acquisition::from_source(
            AcquisitionId::new_v4(),
            AcquisitionOperationId::new_v4(),
            failed_artifact.id(),
            &source,
            "GARMIN/ACTIVITY/BAD.FIT".parse()?,
            "2026-08-31T13:00:00Z".parse()?,
        );
        let failed_run = NormalizationRun::from_parts(
            NormalizationRunId::new_v4(),
            &failed_acquisition,
            ComponentVersion::from_parts("fit-normalizer", "1.0.0".parse()?)?,
            SchemaVersion::from_u32(1)?,
            Vec::new(),
            NormalizationOutcome::Failed(NormalizationFailure::from_string(
                "invalid checksum".to_owned(),
            )?),
        );
        let failed = Ingestion::from_parts(
            &failed_artifact,
            &failed_bytes,
            &failed_acquisition,
            &failed_run,
            &[],
            &[],
        )?;
        storage.ingest(failed).await?;
        assert_eq!(
            storage.artifact_bytes(failed_artifact.id()).await?,
            Some(failed_bytes)
        );
        storage.close().await;

        let options = SqliteConnectOptions::new().filename(&path);
        let mut database = SqliteConnection::connect_with(&options).await?;
        let counts = sqlx::query_file!("queries/test-failure-counts.sql")
            .fetch_one(&mut database)
            .await?;

        assert_eq!(counts.blob_count, 2);
        assert_eq!(counts.failed_count, 1);
        assert_eq!(counts.observation_count, 1);
        database.close().await?;
        Ok(())
    })
}

#[test]
fn source_ownership_and_ingestion_boundaries_are_immutable() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("ownership.sqlite3")).await?;
        let (mut owner, device, source) = owner_and_source()?;
        let other_user = User::from_parts(
            UserId::new_v4(),
            Role::Member,
            Profile::from_display_name("Other".parse()?),
        );
        storage.save_user(&owner).await?;
        storage.save_user(&other_user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;

        let moved_source = Source::from_parts(
            source.id(),
            other_user.id(),
            "Moved".parse()?,
            Some(device.id()),
        );
        assert!(matches!(
            storage.save_source(&moved_source).await,
            Err(garmin_storage::Error::SourceOwnerConflict)
        ));

        let fixture = Fixture::succeeded(
            &source,
            ArtifactId::new_v4(),
            b"complete bytes",
            b"known activity",
        )?;
        assert_eq!(
            Ingestion::from_parts(
                &fixture.artifact,
                b"truncated",
                &fixture.acquisition,
                &fixture.normalization,
                &fixture.observations,
                &[],
            )
            .err(),
            Some(IngestionError::ArtifactBytesMismatch)
        );
        storage.ingest(fixture.ingestion()?).await?;
        owner.replace_profile(Profile::from_parts(
            "Rider".parse()?,
            None,
            Some(AvatarArtifactId::from_artifact_id(fixture.artifact.id())),
        ));
        assert!(matches!(
            storage.save_user(&owner).await,
            Err(garmin_storage::Error::ProfileAvatarNotOwned)
        ));
        storage.close().await;
        Ok(())
    })
}
