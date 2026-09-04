//! Activity persistence contracts.

use std::error::Error;

use futures_lite::future::block_on;
use garmin_fit::{
    CreatorDiagnostics, ManufacturerId, NormalizedActivity, ProductId, ProductName,
    SequencePosition, SerialNumber, SoftwareVersion,
};
use garmin_model::{
    activity::{
        Activity, ActivityDuration, ActivityMetric, ActivityMetrics, ActivitySport,
        ActivitySummary, ActivityTotals, Cadence, Distance, Energy, HeartRate, Lap, Power, Speed,
        Temperature, TimeRange, TimerEvent, TimerState, TrackMeasurements, TrackPoint,
    },
    artifact::{
        Acquisition, AcquisitionId, AcquisitionOperationId, Artifact, ArtifactId,
        NormalizationOutcome, NormalizationRun, NormalizationRunId, SchemaVersion,
    },
    identity::{Device, DeviceId, Profile, Role, Source, SourceId, User, UserId},
    observation::{Observation, ObservationId, ObservationKind},
    route::{Coordinate, Elevation, Latitude, Longitude},
    value::{ComponentVersion, Timestamp},
};
use garmin_storage::{ActivityProjection, Ingestion, IngestionError, Storage};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tempfile::tempdir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct Fixture {
    bytes: Vec<u8>,
    artifact: Artifact,
    acquisition: Acquisition,
    normalization: NormalizationRun,
    observation: Observation,
    normalized: NormalizedActivity,
}

impl Fixture {
    fn new(source: &Source) -> TestResult<Self> {
        let bytes = b"complete activity FIT bytes".to_vec();
        let artifact = Artifact::from_bytes(
            ArtifactId::new_v4(),
            "application/vnd.ant.fit".parse()?,
            &bytes,
        );
        let acquisition = Acquisition::from_source(
            AcquisitionId::new_v4(),
            AcquisitionOperationId::new_v4(),
            artifact.id(),
            source,
            "GARMIN/ACTIVITY/1.FIT".parse()?,
            "2026-08-31T18:40:00Z".parse()?,
        );
        let normalization = NormalizationRun::from_parts(
            NormalizationRunId::new_v4(),
            &acquisition,
            ComponentVersion::from_parts("fit-normalizer", "1.0.0".parse()?)?,
            SchemaVersion::from_u32(1)?,
            Vec::new(),
            NormalizationOutcome::Succeeded,
        );
        let activity = activity()?;
        let observation = Observation::from_run(
            ObservationId::new_v4(),
            &normalization,
            ObservationKind::Activity,
            Some(activity.summary().time().start()),
            activity.semantic_fingerprint()?,
        )?;
        Ok(Self {
            bytes,
            artifact,
            acquisition,
            normalization,
            observation,
            normalized: NormalizedActivity::from_parts(creator("fēnix 8", 2_244)?, activity),
        })
    }

    fn projection(&self) -> TestResult<ActivityProjection<'_>> {
        Ok(ActivityProjection::from_parts(
            &self.observation,
            SequencePosition::from_u32(1),
            &self.normalized,
        )?)
    }

    fn ingestion<'a>(
        &'a self,
        projections: &'a [ActivityProjection<'a>],
    ) -> Result<Ingestion<'a>, IngestionError> {
        Ingestion::from_parts(
            &self.artifact,
            &self.bytes,
            &self.acquisition,
            &self.normalization,
            std::slice::from_ref(&self.observation),
            projections,
        )
    }
}

#[test]
fn round_trips_typed_activity_and_reopens() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let path = root.path().join("activities.sqlite3");
        let storage = Storage::open(&path).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let fixture = Fixture::new(&source)?;
        let projections = [fixture.projection()?];

        storage.ingest(fixture.ingestion(&projections)?).await?;
        storage.ingest(fixture.ingestion(&projections)?).await?;

        let summaries = storage.activities(user.id()).await?;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].observation_id(), fixture.observation.id());
        assert_eq!(summaries[0].owner_id(), user.id());
        assert_eq!(
            summaries[0].summary(),
            fixture.normalized.activity().summary()
        );
        assert_eq!(summaries[0].creator(), fixture.normalized.creator());

        let stored = storage
            .activity(user.id(), fixture.observation.id())
            .await?
            .ok_or("stored activity was missing")?;
        assert_eq!(stored.normalized(), &fixture.normalized);
        assert_eq!(stored.sequence_position(), SequencePosition::from_u32(1));
        assert!(
            storage
                .activity(UserId::new_v4(), fixture.observation.id())
                .await?
                .is_none()
        );
        storage.close().await;

        let reopened = Storage::open(&path).await?;
        let stored = reopened
            .activity(user.id(), fixture.observation.id())
            .await?
            .ok_or("reopened activity was missing")?;
        assert_eq!(stored.normalized(), &fixture.normalized);
        reopened.close().await;
        Ok(())
    })
}

#[test]
fn conflicting_creator_is_atomic() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("conflict.sqlite3")).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let fixture = Fixture::new(&source)?;
        let projections = [fixture.projection()?];
        storage.ingest(fixture.ingestion(&projections)?).await?;

        let conflicting = NormalizedActivity::from_parts(
            creator("tampered creator", 2_245)?,
            fixture.normalized.activity().clone(),
        );
        let conflicting_projection = ActivityProjection::from_parts(
            &fixture.observation,
            SequencePosition::from_u32(1),
            &conflicting,
        )?;
        let conflicting_projections = [conflicting_projection];
        assert!(matches!(
            storage
                .ingest(fixture.ingestion(&conflicting_projections)?)
                .await,
            Err(garmin_storage::Error::Ingestion(
                IngestionError::ActivityProjectionConflict
            ))
        ));

        let stored = storage
            .activity(user.id(), fixture.observation.id())
            .await?
            .ok_or("original activity was missing")?;
        assert_eq!(stored.normalized(), &fixture.normalized);
        storage.close().await;
        Ok(())
    })
}

#[test]
fn activity_observations_require_exactly_one_projection() -> TestResult {
    let (_, _, source) = owner_and_source()?;
    let fixture = Fixture::new(&source)?;
    assert_eq!(
        fixture.ingestion(&[]).err(),
        Some(IngestionError::MissingActivityProjection)
    );

    let projection = fixture.projection()?;
    assert_eq!(
        fixture
            .ingestion(&[projection, fixture.projection()?])
            .err(),
        Some(IngestionError::DuplicateActivityProjection)
    );
    Ok(())
}

#[test]
fn persists_activity_positions_across_a_chained_artifact() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("chained.sqlite3")).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let fixture = Fixture::new(&source)?;
        let second = Observation::from_run(
            ObservationId::new_v4(),
            &fixture.normalization,
            ObservationKind::Activity,
            Some(fixture.normalized.activity().summary().time().start()),
            fixture.normalized.activity().semantic_fingerprint()?,
        )?;
        let observations = [fixture.observation.clone(), second.clone()];
        let projections = [
            ActivityProjection::from_parts(
                &observations[0],
                SequencePosition::from_u32(0),
                &fixture.normalized,
            )?,
            ActivityProjection::from_parts(
                &observations[1],
                SequencePosition::from_u32(2),
                &fixture.normalized,
            )?,
        ];
        let ingestion = Ingestion::from_parts(
            &fixture.artifact,
            &fixture.bytes,
            &fixture.acquisition,
            &fixture.normalization,
            &observations,
            &projections,
        )?;

        storage.ingest(ingestion).await?;

        let mut positions = storage
            .activities(user.id())
            .await?
            .into_iter()
            .map(|activity| activity.sequence_position().into_u32())
            .collect::<Vec<_>>();
        positions.sort_unstable();
        assert_eq!(positions, [0, 2]);
        assert_eq!(
            storage
                .activity(user.id(), second.id())
                .await?
                .ok_or("second chained activity was missing")?
                .sequence_position(),
            SequencePosition::from_u32(2)
        );
        storage.close().await;
        Ok(())
    })
}

#[test]
fn invalid_rows_do_not_escape_as_domain_values() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let path = root.path().join("invalid.sqlite3");
        let storage = Storage::open(&path).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let fixture = Fixture::new(&source)?;
        let projections = [fixture.projection()?];
        storage.ingest(fixture.ingestion(&projections)?).await?;
        storage.close().await;

        let options = SqliteConnectOptions::new().filename(&path);
        let mut database = SqliteConnection::connect_with(&options).await?;
        let observation_id = fixture.observation.id().to_string();
        sqlx::query_file!("tests/queries/delete-activity-creator.sql", observation_id)
            .execute(&mut database)
            .await?;
        database.close().await?;

        let reopened = Storage::open(&path).await?;
        assert!(matches!(
            reopened.activities(user.id()).await,
            Err(garmin_storage::Error::InvalidData {
                field: "FIT manufacturer ID",
                ..
            })
        ));
        reopened.close().await;
        Ok(())
    })
}

#[test]
fn full_reads_verify_the_observation_fingerprint() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let path = root.path().join("fingerprint.sqlite3");
        let storage = Storage::open(&path).await?;
        let (user, device, source) = owner_and_source()?;
        storage.save_user(&user).await?;
        storage.save_device(&device).await?;
        storage.save_source(&source).await?;
        let fixture = Fixture::new(&source)?;
        let projections = [fixture.projection()?];
        storage.ingest(fixture.ingestion(&projections)?).await?;
        storage.close().await;

        let options = SqliteConnectOptions::new().filename(&path);
        let mut database = SqliteConnection::connect_with(&options).await?;
        let observation_id = fixture.observation.id().to_string();
        sqlx::query_file!(
            "tests/queries/corrupt-observation-fingerprint.sql",
            observation_id,
        )
        .execute(&mut database)
        .await?;
        database.close().await?;

        let reopened = Storage::open(&path).await?;
        assert!(matches!(
            reopened.activity(user.id(), fixture.observation.id()).await,
            Err(garmin_storage::Error::InvalidData {
                field: "activity fingerprint",
                ..
            })
        ));
        reopened.close().await;
        Ok(())
    })
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

fn creator(name: &str, software_version: u16) -> TestResult<CreatorDiagnostics> {
    Ok(CreatorDiagnostics::from_parts(
        ManufacturerId::from_u16(1)?,
        Some(ProductId::from_u16(4_532)?),
        Some(SerialNumber::from_u32(123_456)?),
        Some(ProductName::from_string(name.to_owned())?),
        Some(SoftwareVersion::from_hundredths(software_version)?),
    ))
}

fn activity() -> TestResult<Activity> {
    let start = "2026-08-31T18:36:06Z".parse::<Timestamp>()?;
    let middle = Timestamp::from_unix_milliseconds(start.as_unix_milliseconds() + 1_800_000)?;
    let end = Timestamp::from_unix_milliseconds(start.as_unix_milliseconds() + 3_600_500)?;
    let time = TimeRange::from_parts(start, end)?;
    let totals = ActivityTotals::from_parts(
        ActivityDuration::from_milliseconds(3_600_500),
        ActivityDuration::from_milliseconds(3_500_000),
        Some(Distance::from_millimeters(42_195_000)),
        Some(Energy::from_kilocalories(1_234)),
        Some(Distance::from_millimeters(321_000)),
        Some(Distance::from_millimeters(319_000)),
    )?;
    let metrics = ActivityMetrics::from_parts(
        ActivityMetric::from_parts(
            Some(Speed::from_millimeters_per_second(3_500)),
            Some(Speed::from_millimeters_per_second(5_200)),
        ),
        ActivityMetric::from_parts(
            Some(HeartRate::from_beats_per_minute(151)),
            Some(HeartRate::from_beats_per_minute(181)),
        ),
        ActivityMetric::from_parts(
            Some(Cadence::from_revolutions_per_minute(88.5)?),
            Some(Cadence::from_revolutions_per_minute(101.25)?),
        ),
        ActivityMetric::from_parts(Some(Power::from_watts(260)), Some(Power::from_watts(740))),
    );
    let coordinate = Coordinate::from_parts(
        Latitude::from_degrees(60.1699)?,
        Longitude::from_degrees(24.9384)?,
    );
    Activity::from_parts(
        ActivitySummary::from_parts(ActivitySport::Running, time, totals, metrics),
        vec![Lap::from_parts(time, totals, metrics)],
        vec![
            TrackPoint::from_parts(
                start,
                Some(coordinate),
                Some(Elevation::from_meters(12.75)?),
                Some(Distance::from_millimeters(0)),
                TrackMeasurements::from_parts(
                    Some(Speed::from_millimeters_per_second(3_400)),
                    Some(HeartRate::from_beats_per_minute(140)),
                    Some(Cadence::from_revolutions_per_minute(87.5)?),
                    Some(Power::from_watts(250)),
                    Some(Temperature::from_millicelsius(-1_500)),
                ),
            ),
            TrackPoint::from_parts(middle, None, None, None, TrackMeasurements::default()),
        ],
        vec![
            TimerEvent::from_parts(start, TimerState::Running),
            TimerEvent::from_parts(end, TimerState::Stopped),
        ],
    )
    .map_err(Into::into)
}
