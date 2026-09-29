//! Complete portable-data coverage through the production importers and storage API.

use std::{error::Error, fs::File, io::Cursor};

use futures_lite::future::block_on;
use garmin_importer::{AvatarImportRequest, AvatarImporter, RouteImportRequest, RouteImporter};
use garmin_model::{
    artifact::AcquisitionOperationId,
    identity::{Source, SourceId},
    route::{RoutePlanRevisionId, RouteSport},
};
use garmin_storage::{
    Storage,
    snapshot::{Limits, PreparedRestore},
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use tempfile::tempdir;

const GPX: &[u8] = include_bytes!("../../garmin-gpx/tests/fixtures/candidates.gpx");

#[test]
fn complete_contents_survive_snapshot_restore_and_reopen() -> Result<(), Box<dyn Error>> {
    block_on(async {
        let root = tempdir()?;
        let mut source = Storage::open(root.path().join("source.sqlite3")).await?;
        let corpus = garmin_fixtures::seed(&source).await?;
        let owner = corpus.users().first().ok_or("missing fixture user")?.id();
        let connector =
            Source::from_parts(SourceId::new_v4(), owner, "Snapshot fixture".parse()?, None);
        source.save_source(&connector).await?;
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::from_pixel(32, 32, Rgb([32, 128, 224])))
            .write_to(&mut png, ImageFormat::Png)?;
        let avatar_operation = AcquisitionOperationId::new_v4();
        let avatar = AvatarImporter::new(&source)
            .import(AvatarImportRequest::from_parts(
                owner,
                &connector,
                "avatar.png".parse()?,
                avatar_operation,
                "2026-09-01T12:00:00Z".parse()?,
                png.get_ref(),
            ))
            .await?;
        let (route, revised) = route_fixture(&source, &connector).await?;
        let profiles = source.users().await?;
        let selected_avatar = source.profile_avatar(owner).await?;
        let snapshot = root.path().join("complete.tar.zst");
        source.snapshot(&snapshot, Limits::default()).await?;
        let restored_path = root.path().join("restored.sqlite3");
        PreparedRestore::read(File::open(snapshot)?, root.path(), Limits::default())
            .await?
            .publish(&restored_path)?;
        for _ in 0..2 {
            let restored = Storage::open(&restored_path).await?;
            assert_eq!(restored.users().await?, profiles);
            assert_eq!(restored.profile_avatar(owner).await?, selected_avatar);
            assert_eq!(
                restored
                    .artifact_bytes(avatar.original_artifact_id().as_artifact_id())
                    .await?,
                Some(png.get_ref().clone())
            );
            assert_eq!(
                restored
                    .artifact_bytes(avatar.thumbnail_artifact_id())
                    .await?,
                source
                    .artifact_bytes(avatar.thumbnail_artifact_id())
                    .await?
            );
            assert_eq!(
                restored.acquisition_time(owner, avatar_operation).await?,
                source.acquisition_time(owner, avatar_operation).await?
            );
            assert_eq!(
                restored.artifact_bytes(route.artifact_id()).await?,
                Some(GPX.to_vec())
            );
            assert_eq!(
                restored.route_plan(owner, route.plan_id()).await?,
                Some(revised.clone())
            );
            for activity in corpus.activities() {
                assert_eq!(
                    restored
                        .artifact_bytes(activity.receipt().artifact_id())
                        .await?,
                    Some(activity.case().encode()?)
                );
                for observation in activity.receipt().observation_ids() {
                    assert_eq!(
                        restored.activity(activity.owner_id(), *observation).await?,
                        source.activity(activity.owner_id(), *observation).await?
                    );
                }
            }
            // An exact import retry must still resolve to the existing acquisition after restore.
            let retry = AvatarImporter::new(&restored)
                .import(AvatarImportRequest::from_parts(
                    owner,
                    &connector,
                    "avatar.png".parse()?,
                    avatar_operation,
                    "2026-09-01T12:00:00Z".parse()?,
                    png.get_ref(),
                ))
                .await?;
            assert_eq!(retry, avatar);
            restored.check_integrity().await?;
            restored.close().await;
        }
        source.close().await;
        Ok(())
    })
}

async fn route_fixture(
    source: &Storage,
    connector: &Source,
) -> Result<
    (
        garmin_importer::RouteImportReceipt,
        garmin_storage::StoredRoutePlan,
    ),
    Box<dyn Error>,
> {
    let owner = connector.owner_id();
    let route = RouteImporter::new(source)
        .import(RouteImportRequest::from_parts(
            owner,
            connector,
            "route.gpx".parse()?,
            AcquisitionOperationId::new_v4(),
            "2026-09-01T12:00:00Z".parse()?,
            GPX,
            garmin_gpx::CandidateSource::Route { route: 0 },
            "Test route".parse()?,
            RouteSport::Cycling,
        ))
        .await?;
    let original = source
        .route_plan(owner, route.plan_id())
        .await?
        .ok_or("missing route")?;
    let revision = garmin_route::confirm_straight_lines(
        original.revision(),
        RoutePlanRevisionId::new_v4(),
        "2026-09-01T13:00:00Z".parse()?,
    )?;
    let revised = source
        .save_route_revision(owner, original.plan(), &revision)
        .await?;
    Ok((route, revised))
}
