//! End-to-end GPX route import contracts.

use std::error::Error;

use futures_lite::future::block_on;
use garmin_gpx::CandidateSource;
use garmin_importer::{RouteImportRequest, RouteImporter};
use garmin_model::{
    artifact::{AcquisitionOperationId, SourceIdentity},
    identity::{Profile, Role, Source, SourceId, User, UserId},
    route::RouteSport,
    value::Timestamp,
};
use garmin_storage::Storage;
use tempfile::tempdir;

const GPX: &[u8] = include_bytes!("../../garmin-gpx/tests/fixtures/candidates.gpx");

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
fn selected_candidates_stay_separate_and_share_the_original() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let storage = Storage::open(root.path().join("routes.sqlite3")).await?;
        let user = User::from_parts(
            UserId::new_v4(),
            Role::Owner,
            Profile::from_display_name("Rider".parse()?),
        );
        let source = Source::from_parts(
            SourceId::new_v4(),
            user.id(),
            "Selected files".parse()?,
            None,
        );
        storage.save_user(&user).await?;
        storage.save_source(&source).await?;
        let operation = AcquisitionOperationId::from_u128(7);
        let importer = RouteImporter::new(&storage);

        let first = importer
            .import(request(
                &user,
                &source,
                operation,
                CandidateSource::TrackSegment {
                    track: 0,
                    segment: 0,
                },
                "First segment",
            )?)
            .await?;
        let retry = importer
            .import(request(
                &user,
                &source,
                operation,
                CandidateSource::TrackSegment {
                    track: 0,
                    segment: 0,
                },
                "First segment",
            )?)
            .await?;
        let second = importer
            .import(request(
                &user,
                &source,
                operation,
                CandidateSource::TrackSegment {
                    track: 0,
                    segment: 1,
                },
                "Second segment",
            )?)
            .await?;

        assert_eq!(first, retry);
        assert_eq!(first.artifact_id(), second.artifact_id());
        assert_eq!(first.acquisition_id(), second.acquisition_id());
        assert_ne!(first.plan_id(), second.plan_id());
        assert_eq!(
            storage.artifact_bytes(first.artifact_id()).await?,
            Some(GPX.to_vec())
        );
        assert_eq!(storage.route_plans(user.id()).await?.len(), 2);
        assert!(
            storage
                .route_plan(user.id(), first.plan_id())
                .await?
                .ok_or("first route was not stored")?
                .revision()
                .shape()
                .is_geometry()
        );
        storage.close().await;
        Ok(())
    })
}

fn request<'a>(
    user: &User,
    source: &'a Source,
    operation: AcquisitionOperationId,
    candidate: CandidateSource,
    name: &str,
) -> TestResult<RouteImportRequest<'a>> {
    Ok(RouteImportRequest::from_parts(
        user.id(),
        source,
        SourceIdentity::from_string("routes.gpx".to_owned())?,
        operation,
        Timestamp::from_unix_seconds(1_788_198_400)?,
        GPX,
        candidate,
        name.parse()?,
        RouteSport::Cycling,
    ))
}
