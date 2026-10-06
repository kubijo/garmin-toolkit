use futures_lite::future::block_on;
use garmin_model::{
    artifact::{Acquisition, AcquisitionId, AcquisitionOperationId},
    identity::{Profile, Role, Source, SourceId, User},
    route::{
        Coordinate, Latitude, Longitude, RevisionProvenance, RevisionSource, RoutePlan, RoutePoint,
        RouteShape, RouteSport,
    },
};
use tempfile::{TempDir, tempdir};

use super::*;
use crate::RouteImport;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn deletion_is_owned_idempotent_and_reclaims_only_unreferenced_bytes() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let first = pending(&fixture)
            .await?
            .commit(b"shared course bytes")
            .await?;
        let second = pending_with_encoder(
            &fixture,
            ComponentVersion::from_parts("test-course-encoder", "2.0.0".parse()?)?,
        )
        .await?
        .commit(b"shared course bytes")
        .await?;
        assert!(
            fixture
                .storage
                .delete_course_generation(UserId::new_v4(), fixture.revision, first.id())
                .await
                .is_err()
        );
        assert!(
            fixture
                .storage
                .delete_course_generation(fixture.owner, RoutePlanRevisionId::new_v4(), first.id())
                .await
                .is_err()
        );
        fixture
            .storage
            .delete_course_generation(fixture.owner, fixture.revision, first.id())
            .await?;
        fixture
            .storage
            .delete_course_generation(fixture.owner, fixture.revision, first.id())
            .await?;
        assert_eq!(
            fixture
                .storage
                .course_artifact(fixture.owner, second.artifact_id())
                .await?,
            Some(b"shared course bytes".to_vec())
        );
        assert_eq!(
            fixture
                .storage
                .course_artifact(fixture.owner, first.artifact_id())
                .await?,
            None
        );
        assert_eq!(
            fixture.storage.artifact_bytes(first.artifact_id()).await?,
            None
        );
        fixture
            .storage
            .delete_course_generation(fixture.owner, fixture.revision, second.id())
            .await?;
        let counts = sqlx::query_file!("queries/test-course-storage-counts.sql")
            .fetch_one(&fixture.storage.pool)
            .await?;
        assert_eq!(
            counts.blobs, 1,
            "the original GPX remains, the unreferenced Course blob is gone"
        );
        assert_eq!(counts.artifacts, 1);
        assert_eq!(counts.generations, 0);
        assert!(
            fixture
                .storage
                .route_revision(fixture.owner, fixture.revision)
                .await?
                .is_some()
        );
        assert_deleted_operation(&fixture, first.operation_id()).await?;
        let third = pending(&fixture).await?.commit(b"new bytes").await?;
        assert_eq!(third.version().get(), 3);
        assert_eq!(third.serial().as_u32(), 3);
        fixture.storage.close().await;
        Ok(())
    })
}

#[test]
fn current_encoder_artifact_precedes_newer_history_across_pages() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let current = generate(&fixture).await?;
        let newer = pending_with_encoder(
            &fixture,
            ComponentVersion::from_parts("test-course-encoder", "2.0.0".parse()?)?,
        )
        .await?
        .commit(b"newer encoder")
        .await?;
        let newest = pending_with_encoder(
            &fixture,
            ComponentVersion::from_parts("test-course-encoder", "3.0.0".parse()?)?,
        )
        .await?
        .commit(b"newest encoder")
        .await?;
        let encoder = encoder()?;
        for (offset, expected) in [current, newest, newer].into_iter().enumerate() {
            let page = fixture
                .storage
                .generated_courses_page(
                    fixture.owner,
                    fixture.revision,
                    &encoder,
                    u32::try_from(offset)?,
                    1,
                )
                .await?;
            assert_eq!(page, vec![expected]);
        }
        assert!(
            fixture
                .storage
                .generated_courses_page(UserId::new_v4(), fixture.revision, &encoder, 0, 1)
                .await?
                .is_empty()
        );
        fixture.storage.close().await;
        Ok(())
    })
}

#[test]
fn reuse_preserves_receipts_and_encoder_changes_create_versions() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let first = generate(&fixture).await?;
        let operation = CourseGenerationOperationId::new_v4();
        let reused = fixture
            .storage
            .prepare_course_generation(
                fixture.owner,
                operation,
                fixture.revision,
                encoder()?,
                time(),
            )
            .await?;
        let CourseGenerationStart::Existing(reused) = reused else {
            return Err("expected reuse".into());
        };
        assert_eq!(*reused, first);
        assert_eq!(
            fixture
                .storage
                .course_generation(fixture.owner, operation)
                .await?,
            Some(first.clone())
        );
        let next = pending_with_encoder(
            &fixture,
            ComponentVersion::from_parts("test-course-encoder", "2.0.0".parse()?)?,
        )
        .await?
        .commit(b"changed encoder")
        .await?;
        assert_eq!(next.version().get(), 2);
        assert_eq!(next.serial().as_u32(), 2);
        let retry = fixture
            .storage
            .prepare_course_generation(
                fixture.owner,
                operation,
                fixture.revision,
                next.encoder().clone(),
                time(),
            )
            .await?;
        let CourseGenerationStart::Existing(retry) = retry else {
            return Err("expected original receipt".into());
        };
        assert_eq!(*retry, first);
        fixture
            .storage
            .delete_course_generation(fixture.owner, fixture.revision, first.id())
            .await?;
        assert_deleted_operation(&fixture, operation).await?;
        fixture.storage.close().await;
        Ok(())
    })
}

#[test]
fn deletion_tombstone_survives_snapshot_restore() -> TestResult {
    use crate::snapshot::{Limits, PreparedRestore};
    block_on(async {
        let mut fixture = fixture().await?;
        let generated = pending(&fixture).await?.commit(b"course bytes").await?;
        let alias = CourseGenerationOperationId::new_v4();
        assert!(matches!(
            fixture
                .storage
                .prepare_course_generation(
                    fixture.owner,
                    alias,
                    fixture.revision,
                    encoder()?,
                    time()
                )
                .await?,
            CourseGenerationStart::Existing(_)
        ));
        fixture
            .storage
            .delete_course_generation(fixture.owner, fixture.revision, generated.id())
            .await?;
        let archive = fixture.root.path().join("deleted.tar.zst");
        fixture
            .storage
            .snapshot(&archive, Limits::default())
            .await?;
        let restored = fixture.root.path().join("restored.sqlite3");
        PreparedRestore::read(
            std::fs::File::open(archive)?,
            fixture.root.path(),
            Limits::default(),
        )
        .await?
        .publish(&restored)?;
        let storage = Storage::open(restored).await?;
        storage
            .delete_course_generation(fixture.owner, fixture.revision, generated.id())
            .await?;
        assert!(
            storage
                .generated_courses(fixture.owner, fixture.revision)
                .await?
                .is_empty()
        );
        assert!(matches!(
            storage
                .course_generation(fixture.owner, generated.operation_id())
                .await,
            Err(crate::Error::Course(CoursePersistenceError::Deleted))
        ));
        assert!(matches!(
            storage.course_generation(fixture.owner, alias).await,
            Err(crate::Error::Course(CoursePersistenceError::Deleted))
        ));
        storage.close().await;
        fixture.storage.close().await;
        Ok(())
    })
}

struct Fixture {
    root: TempDir,
    storage: Storage,
    owner: UserId,
    revision: RoutePlanRevisionId,
}

async fn fixture() -> TestResult<Fixture> {
    let root = tempdir()?;
    let storage = Storage::open(root.path().join("courses.sqlite3")).await?;
    let user = User::from_parts(
        UserId::new_v4(),
        Role::Owner,
        Profile::from_display_name("Walker".parse()?),
    );
    let source = Source::from_parts(SourceId::new_v4(), user.id(), "GPX".parse()?, None);
    storage.save_user(&user).await?;
    storage.save_source(&source).await?;
    let bytes = include_bytes!("../../../garmin-gpx/tests/fixtures/candidates.gpx");
    let artifact =
        Artifact::from_bytes(ArtifactId::new_v4(), "application/gpx+xml".parse()?, bytes);
    let acquisition = Acquisition::from_source(
        AcquisitionId::new_v4(),
        AcquisitionOperationId::new_v4(),
        artifact.id(),
        &source,
        "walk.gpx".parse()?,
        time(),
    );
    let points = [(50.0755, 14.4378), (50.0810, 14.4510)]
        .into_iter()
        .map(|(lat, lon)| {
            Ok(RoutePoint::from_parts(
                Coordinate::from_parts(Latitude::from_degrees(lat)?, Longitude::from_degrees(lon)?),
                None,
            ))
        })
        .collect::<Result<Vec<_>, garmin_model::route::Error>>()?;
    let revision = RoutePlanRevision::from_parts(
        RoutePlanRevisionId::new_v4(),
        RoutePlanId::new_v4(),
        None,
        time(),
        "Walk".parse()?,
        RouteSport::Walking,
        RouteShape::from_geometry(points)?,
        Vec::new(),
        RevisionProvenance::from_parts(RevisionSource::Artifact(artifact.id()), Vec::new()),
    )?;
    let plan = RoutePlan::from_parts(revision.plan_id(), user.id(), revision.id());
    storage
        .save_route_import(RouteImport::from_parts(
            &artifact,
            bytes,
            &acquisition,
            &plan,
            &revision,
        )?)
        .await?;
    Ok(Fixture {
        root,
        storage,
        owner: user.id(),
        revision: revision.id(),
    })
}

fn time() -> Timestamp {
    Timestamp::from_unix_seconds(1_788_198_400).expect("valid fixture timestamp")
}

fn encoder() -> TestResult<ComponentVersion> {
    Ok(ComponentVersion::from_parts(
        "test-course-encoder",
        "1.0.0".parse()?,
    )?)
}

async fn pending(fixture: &Fixture) -> TestResult<Box<PendingCourseGeneration>> {
    pending_with_encoder(fixture, encoder()?).await
}

async fn pending_with_encoder(
    fixture: &Fixture,
    encoder: ComponentVersion,
) -> TestResult<Box<PendingCourseGeneration>> {
    match fixture
        .storage
        .prepare_course_generation(
            fixture.owner,
            CourseGenerationOperationId::new_v4(),
            fixture.revision,
            encoder,
            time(),
        )
        .await?
    {
        CourseGenerationStart::Pending(pending) => Ok(pending),
        CourseGenerationStart::Existing(_) => Err("unexpected prior generation".into()),
    }
}

async fn counts(storage: &Storage) -> TestResult<(i64, i64, i64, i64)> {
    let counts = sqlx::query_file!("queries/test-course-storage-counts.sql")
        .fetch_one(&storage.pool)
        .await?;
    Ok((
        counts.blobs,
        counts.artifacts,
        counts.generations,
        counts.serial,
    ))
}

#[test]
fn receipt_write_failure_rolls_back_bytes_and_allocations() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let before = counts(&fixture.storage).await?;
        let mut pending = pending(&fixture).await?;
        sqlx::raw_sql(include_str!(
            "../../queries/test-abort-course-generation.sql"
        ))
        .execute(&mut *pending.transaction)
        .await?;
        let bytes = garmin_fit::course::encode(pending.revision(), pending.serial())?;
        assert!(pending.commit(&bytes).await.is_err());
        assert_eq!(counts(&fixture.storage).await?, before);
        let retry = generate(&fixture).await?;
        assert_eq!(retry.version().get(), 1);
        assert_eq!(retry.serial().as_u32(), 1);
        fixture.storage.close().await;
        Ok(())
    })
}

async fn generate(fixture: &Fixture) -> TestResult<GeneratedCourse> {
    let pending = pending(fixture).await?;
    let bytes = garmin_fit::course::encode(pending.revision(), pending.serial())?;
    Ok(pending.commit(&bytes).await?)
}

#[test]
fn abandoning_a_generation_releases_its_allocations() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let before = counts(&fixture.storage).await?;
        drop(pending(&fixture).await?);
        let generated = generate(&fixture).await?;
        assert_eq!(generated.version().get(), 1);
        assert_eq!(generated.serial().as_u32(), 1);
        let after = counts(&fixture.storage).await?;
        assert_eq!(after, (before.0 + 1, before.1 + 1, 1, 1));
        fixture.storage.close().await;
        Ok(())
    })
}

#[test]
fn saved_bytes_are_verified_instead_of_regenerated() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let generated = generate(&fixture).await?;
        let artifact = generated.artifact_id().to_string();
        sqlx::query_file!("queries/test-corrupt-course-blob.sql", artifact)
            .execute(&fixture.storage.pool)
            .await?;
        assert!(matches!(
            fixture
                .storage
                .course_artifact(fixture.owner, generated.artifact_id())
                .await,
            Err(crate::Error::Course(
                CoursePersistenceError::ArtifactIntegrity
            ))
        ));
        fixture.storage.close().await;
        Ok(())
    })
}

#[test]
fn encoder_upgrade_preserves_committed_receipts() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let original = generate(&fixture).await?;
        let upgraded = ComponentVersion::from_parts("replacement-encoder", "2.0.0".parse()?)?;
        let later = Timestamp::from_unix_seconds(time().as_unix_seconds() + 60)?;
        let retry = fixture
            .storage
            .prepare_course_generation(
                fixture.owner,
                original.operation_id(),
                fixture.revision,
                upgraded.clone(),
                later,
            )
            .await?;
        let CourseGenerationStart::Existing(retry) = retry else {
            return Err("committed retry allocated a new generation".into());
        };
        assert_eq!(*retry, original);
        let next = fixture
            .storage
            .prepare_course_generation(
                fixture.owner,
                CourseGenerationOperationId::new_v4(),
                fixture.revision,
                upgraded.clone(),
                later,
            )
            .await?;
        let CourseGenerationStart::Pending(next) = next else {
            return Err("explicit generation reused a prior receipt".into());
        };
        let bytes = garmin_fit::course::encode(next.revision(), next.serial())?;
        let next = next.commit(&bytes).await?;
        assert_eq!(next.encoder(), &upgraded);
        assert_eq!(next.generated_at(), later);
        assert_eq!(next.version().get(), 2);
        assert_ne!(next.artifact_id(), original.artifact_id());
        fixture.storage.close().await;
        Ok(())
    })
}

#[test]
fn exhausted_serials_do_not_overflow_or_leave_artifacts() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        sqlx::query_file!("queries/test-exhaust-course-serial.sql")
            .execute(&fixture.storage.pool)
            .await?;
        let before = counts(&fixture.storage).await?;
        assert!(matches!(
            fixture
                .storage
                .prepare_course_generation(
                    fixture.owner,
                    CourseGenerationOperationId::new_v4(),
                    fixture.revision,
                    encoder()?,
                    time()
                )
                .await,
            Err(crate::Error::Course(
                CoursePersistenceError::SerialExhausted
            ))
        ));
        assert_eq!(counts(&fixture.storage).await?, before);
        fixture.storage.close().await;
        Ok(())
    })
}

async fn assert_deleted_operation(
    fixture: &Fixture,
    operation: CourseGenerationOperationId,
) -> TestResult {
    assert!(matches!(
        fixture
            .storage
            .course_generation(fixture.owner, operation)
            .await,
        Err(crate::Error::Course(CoursePersistenceError::Deleted))
    ));
    assert!(matches!(
        fixture
            .storage
            .prepare_course_generation(
                fixture.owner,
                operation,
                fixture.revision,
                encoder()?,
                time()
            )
            .await,
        Err(crate::Error::Course(CoursePersistenceError::Deleted))
    ));
    Ok(())
}
