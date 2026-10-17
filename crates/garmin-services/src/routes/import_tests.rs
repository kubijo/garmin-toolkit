use std::fs::File;

use futures_lite::future::{block_on, zip};
use garmin_gpx::CandidateSource;
use garmin_model::{
    identity::{Source, SourceId},
    route::RouteSport,
};
use garmin_storage::{RoutePersistenceError, Storage, snapshot::Limits};
use tempfile::{TempDir, tempdir};

use super::*;
use crate::RouteImportRequest;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const GPX: &[u8] = include_bytes!("../../../garmin-gpx/tests/fixtures/candidates.gpx");

struct Fixture {
    root: TempDir,
    app: Application,
    owner: UserContext,
    source: Source,
    operation: AcquisitionOperationId,
}

async fn fixture() -> TestResult<Fixture> {
    let root = tempdir()?;
    let app = Application::new(Storage::open(root.path().join("imports.sqlite3")).await?);
    let owner = UserContext::new(app.create_profile("Walker".parse()?).await?.id());
    let source = Source::from_parts(
        SourceId::new_v4(),
        owner.user_id(),
        "GPX files".parse()?,
        None,
    );
    Ok(Fixture {
        root,
        app,
        owner,
        source,
        operation: AcquisitionOperationId::new_v4(),
    })
}

fn request(fixture: &Fixture) -> TestResult<RouteImportRequest<'_>> {
    Ok(RouteImportRequest::from_parts(
        fixture.owner,
        &fixture.source,
        "walk.gpx".parse()?,
        fixture.operation,
        Timestamp::from_unix_seconds(1_788_198_400)?,
        GPX,
        CandidateSource::Route { route: 0 },
        "Morning walk".parse()?,
        RouteSport::Walking,
    ))
}

#[test]
fn import_retry_after_restart_preserves_the_receipt_and_edited_head() -> TestResult {
    block_on(async {
        let mut fixture = fixture().await?;
        let receipt = fixture.app.import_route(request(&fixture)?).await?;
        let provenance = fixture
            .app
            .route_import(fixture.owner, fixture.operation)
            .await?
            .ok_or("missing receipt")?;
        assert_eq!(
            provenance.identity().candidate(),
            CandidateSource::Route { route: 0 }
        );
        assert_eq!(provenance.parser().name(), garmin_gpx::ADAPTER_NAME);
        let edited = fixture
            .app
            .confirm_route_straight_lines(
                fixture.owner,
                receipt.plan_id(),
                provenance.acquired_at(),
            )
            .await?;
        fixture.app.close().await;
        fixture.app =
            Application::new(Storage::open(fixture.root.path().join("imports.sqlite3")).await?);
        let mut retry = request(&fixture)?;
        retry.acquired_at = Timestamp::from_unix_seconds(1_788_199_400)?;
        assert_eq!(fixture.app.import_route(retry).await?, receipt);
        assert_eq!(
            fixture
                .app
                .route_plan(fixture.owner, receipt.plan_id())
                .await?,
            Some(edited)
        );
        assert_eq!(
            fixture
                .app
                .route_import(fixture.owner, fixture.operation)
                .await?,
            Some(provenance)
        );
        fixture.app.close().await;
        Ok(())
    })
}

#[test]
fn import_retries_reject_every_changed_selection_argument() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let receipt = fixture.app.import_route(request(&fixture)?).await?;
        let alternative_source = Source::from_parts(
            SourceId::new_v4(),
            fixture.owner.user_id(),
            "Other source".parse()?,
            None,
        );
        for change in 0..6 {
            let mut changed = request(&fixture)?;
            match change {
                0 => {
                    changed.candidate = CandidateSource::TrackSegment {
                        track: 0,
                        segment: 1,
                    }
                }
                1 => changed.name = "Changed name".parse()?,
                2 => changed.sport = RouteSport::Hiking,
                3 => changed.bytes = b"malformed replacement input",
                4 => changed.source_identity = "different.gpx".parse()?,
                5 => changed.source = &alternative_source,
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    fixture.app.import_route(changed).await,
                    Err(Error::RouteImport(
                        garmin_importer::RouteImportError::Persistence(
                            RoutePersistenceError::ImportOperationConflict
                        )
                    ))
                ),
                "changed argument {change} was not rejected as a conflict"
            );
        }
        assert_eq!(fixture.app.route_plans(fixture.owner).await?.len(), 1);
        assert_eq!(fixture.app.import_route(request(&fixture)?).await?, receipt);
        fixture.app.close().await;
        Ok(())
    })
}

#[test]
fn concurrent_confirmations_coalesce_and_conflicting_selections_do_not_commit() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let (left, right) = zip(
            fixture.app.import_route(request(&fixture)?),
            fixture.app.import_route(request(&fixture)?),
        )
        .await;
        assert_eq!(left?, right?);
        let mut first = request(&fixture)?;
        first.operation_id = AcquisitionOperationId::new_v4();
        let mut second = request(&fixture)?;
        second.operation_id = first.operation_id;
        second.sport = RouteSport::Hiking;
        let (left, right) = zip(
            fixture.app.import_route(first),
            fixture.app.import_route(second),
        )
        .await;
        assert_ne!(left.is_ok(), right.is_ok());
        assert_eq!(fixture.app.route_plans(fixture.owner).await?.len(), 2);
        fixture.app.close().await;
        Ok(())
    })
}

#[test]
fn import_receipts_are_profile_scoped_and_survive_snapshot_restore() -> TestResult {
    block_on(async {
        let mut fixture = fixture().await?;
        let original = fixture.app.import_route(request(&fixture)?).await?;
        let other = UserContext::new(fixture.app.create_profile("Other".parse()?).await?.id());
        assert!(
            fixture
                .app
                .route_import(other, fixture.operation)
                .await?
                .is_none()
        );
        let mut foreign = request(&fixture)?;
        foreign.user = other;
        assert!(matches!(
            fixture.app.import_route(foreign).await,
            Err(Error::RouteImport(
                garmin_importer::RouteImportError::ActorCannotUseSource
            ))
        ));
        let archive = fixture.root.path().join("snapshot.tar.zst");
        fixture
            .app
            .snapshot(fixture.owner, &archive, Limits::default())
            .await?;
        let prepared = fixture
            .app
            .prepare_restore(
                fixture.owner,
                File::open(archive)?,
                fixture.root.path(),
                Limits::default(),
            )
            .await?;
        let restored = fixture.root.path().join("restored.sqlite3");
        prepared.publish(&restored)?;
        fixture.app.close().await;
        fixture.app = Application::new(Storage::open(restored).await?);
        assert_eq!(
            fixture.app.import_route(request(&fixture)?).await?,
            original
        );
        fixture.app.close().await;
        Ok(())
    })
}

#[test]
fn rejected_input_does_not_register_a_source() -> TestResult {
    block_on(async {
        let fixture = fixture().await?;
        let mut invalid = request(&fixture)?;
        invalid.bytes = b"invalid GPX";
        assert!(fixture.app.import_route(invalid).await.is_err());
        assert!(fixture.app.route_plans(fixture.owner).await?.is_empty());
        // Reusing the unsaved source ID under another owner proves no orphan source was committed.
        let other = fixture.app.create_profile("Other".parse()?).await?;
        let source = Source::from_parts(
            fixture.source.id(),
            other.id(),
            "Other source".parse()?,
            None,
        );
        fixture.app.storage.save_source(&source).await?;
        fixture.app.close().await;
        Ok(())
    })
}
