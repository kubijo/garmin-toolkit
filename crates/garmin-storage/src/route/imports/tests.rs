use futures_lite::future::block_on;
use garmin_model::{
    artifact::{Acquisition, AcquisitionId, Artifact, ArtifactId},
    identity::{Profile, Role, SourceId, User},
    route::{
        Coordinate, Latitude, Longitude, RevisionProvenance, RevisionSource, RoutePlan,
        RoutePlanId, RoutePlanRevision, RoutePlanRevisionId, RoutePoint, RouteShape, RouteSport,
    },
    value::Transformation,
};
use tempfile::tempdir;

use super::*;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn counts(storage: &Storage) -> TestResult<[i64; 8]> {
    let row = sqlx::query_file!("queries/test-route-import-counts.sql")
        .fetch_one(&storage.pool)
        .await?;
    Ok([
        row.sources,
        row.blobs,
        row.artifacts,
        row.acquisitions,
        row.plans,
        row.revisions,
        row.heads,
        row.receipts,
    ])
}

#[test]
fn failed_receipt_write_rolls_back_the_source_and_all_import_records() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("imports.sqlite3")).await?;
        let owner = User::from_parts(
            UserId::new_v4(),
            Role::Owner,
            Profile::from_display_name("Walker".parse()?),
        );
        storage.save_user(&owner).await?;
        let source = Source::from_parts(SourceId::new_v4(), owner.id(), "Files".parse()?, None);
        let bytes = b"immutable original bytes";
        let artifact =
            Artifact::from_bytes(ArtifactId::new_v4(), "application/gpx+xml".parse()?, bytes);
        let operation = AcquisitionOperationId::new_v4();
        let acquired = Timestamp::from_unix_seconds(1_788_198_400)?;
        let acquisition = Acquisition::from_source(
            AcquisitionId::new_v4(),
            operation,
            artifact.id(),
            &source,
            "walk.gpx".parse()?,
            acquired,
        );
        let parser = ComponentVersion::from_parts("test-gpx", "1.0.0".parse()?)?;
        let shape = RouteShape::from_geometry(vec![point(50.0, 14.0)?, point(50.1, 14.1)?])?;
        let revision = RoutePlanRevision::from_parts(
            RoutePlanRevisionId::new_v4(),
            RoutePlanId::new_v4(),
            None,
            acquired,
            "Walk".parse()?,
            RouteSport::Walking,
            shape,
            Vec::new(),
            RevisionProvenance::from_parts(
                RevisionSource::Artifact(artifact.id()),
                vec![Transformation::from_component(parser.clone())],
            ),
        )?;
        let plan = RoutePlan::from_parts(revision.plan_id(), owner.id(), revision.id());
        let identity = RouteImportIdentity::from_parts(
            owner.id(),
            source.id(),
            acquisition.source_identity().clone(),
            artifact.digest(),
            RouteCandidateSource::TrackSegment {
                track: 0,
                segment: 0,
            },
            revision.name().clone(),
            revision.sport(),
        );
        let import = || RouteImport::from_parts(&artifact, bytes, &acquisition, &plan, &revision);
        let before = counts(&storage).await?;
        sqlx::raw_sql(include_str!("../../../queries/test-abort-route-import.sql"))
            .execute(&storage.pool)
            .await?;
        assert!(
            storage
                .save_selected_route_import(&source, &identity, &parser, import()?)
                .await
                .is_err()
        );
        assert_eq!(counts(&storage).await?, before);
        assert!(storage.route_import(owner.id(), operation).await?.is_none());
        sqlx::raw_sql(include_str!(
            "../../../queries/test-drop-abort-route-import.sql"
        ))
        .execute(&storage.pool)
        .await?;
        let receipt = storage
            .save_selected_route_import(&source, &identity, &parser, import()?)
            .await?;
        assert_eq!(receipt.revision_id(), revision.id());
        assert_eq!(counts(&storage).await?, [1; 8]);
        assert_eq!(
            storage
                .route_import(owner.id(), operation)
                .await?
                .ok_or("missing receipt")?
                .identity(),
            &identity
        );
        storage.close().await;
        Ok(())
    })
}

fn point(latitude: f64, longitude: f64) -> TestResult<RoutePoint> {
    Ok(RoutePoint::from_parts(
        Coordinate::from_parts(
            Latitude::from_degrees(latitude)?,
            Longitude::from_degrees(longitude)?,
        ),
        None,
    ))
}
