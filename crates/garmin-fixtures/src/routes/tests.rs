use std::fs;

use futures_lite::future::block_on;
use garmin_model::route::{
    RevisionProvenance, RevisionSource, RoutePlanRevision, RoutePlanRevisionId,
};
use garmin_storage::snapshot::{Limits, PreparedRestore};
use tempfile::tempdir;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn alex_routes_preserve_recorded_geometry_and_survive_restore() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let mut storage = Storage::open(root.path().join("routes.sqlite3")).await?;
        let corpus = super::super::seed(&storage).await?;
        let owner = corpus.users()[0].id();
        assert_eq!(corpus.routes().len(), CASES.len());
        assert_eq!(storage.route_plans(owner).await?.len(), CASES.len());
        for other in &corpus.users()[1..] {
            assert!(storage.route_plans(other.id()).await?.is_empty());
        }
        for (receipt, (case, name, sport)) in corpus.routes().iter().zip(CASES) {
            let route = storage.route_plan(owner, receipt.plan_id()).await?.unwrap();
            assert_eq!(route.revision().name().as_str(), name);
            assert_eq!(route.revision().sport(), sport);
            assert!(route.revision().shape().is_geometry());
            let activity = corpus
                .activities()
                .iter()
                .find(|item| item.case() == case)
                .unwrap();
            let stored = storage
                .activity(activity.owner_id(), activity.receipt().observation_ids()[0])
                .await?
                .unwrap();
            let recorded = stored.normalized().activity().track();
            let planned = route.revision().shape().points();
            assert_eq!(planned.len(), recorded.len());
            for (planned, recorded) in planned.iter().zip(recorded) {
                assert_eq!(Some(planned.coordinate()), recorded.coordinate());
                assert_eq!(planned.elevation(), recorded.elevation());
            }
            let bytes = storage
                .route_source_artifact(owner, receipt.artifact_id())
                .await?
                .unwrap();
            let document = gpx_format::read(bytes.as_slice())?;
            assert_eq!(
                document.creator.as_deref(),
                Some("Garmin Toolkit demo recording export")
            );
            assert_eq!(document.tracks[0].segments[0].points.len(), planned.len());
        }
        let archive = root.path().join("routes.tar.zst");
        storage.snapshot(&archive, Limits::default()).await?;
        let restored_path = root.path().join("restored.sqlite3");
        PreparedRestore::read(fs::File::open(archive)?, root.path(), Limits::default())
            .await?
            .publish(&restored_path)?;
        let restored = Storage::open(restored_path).await?;
        assert_eq!(
            storage.route_plans(owner).await?,
            restored.route_plans(owner).await?
        );
        for receipt in corpus.routes() {
            assert_eq!(
                storage.route_plan(owner, receipt.plan_id()).await?,
                restored.route_plan(owner, receipt.plan_id()).await?
            );
            assert_eq!(
                storage
                    .route_source_artifact(owner, receipt.artifact_id())
                    .await?,
                restored
                    .route_source_artifact(owner, receipt.artifact_id())
                    .await?
            );
        }
        restored.close().await;
        storage.close().await;
        Ok(())
    })
}

#[test]
fn reseeding_after_reopen_preserves_edited_route_heads() -> TestResult {
    block_on(async {
        let root = tempdir()?;
        let path = root.path().join("routes.sqlite3");
        let storage = Storage::open(&path).await?;
        let corpus = super::super::seed(&storage).await?;
        let owner = corpus.users()[0].id();
        let receipt = corpus.routes()[0];
        let original = storage.route_plan(owner, receipt.plan_id()).await?.unwrap();
        let revision = original.revision();
        let edited = RoutePlanRevision::from_parts(
            RoutePlanRevisionId::new_v4(),
            receipt.plan_id(),
            Some(revision.id()),
            revision.created_at(),
            "My renamed ride".parse()?,
            revision.sport(),
            revision.shape().clone(),
            Vec::new(),
            RevisionProvenance::from_parts(RevisionSource::Revision(revision.id()), Vec::new()),
        )?;
        let saved = storage
            .save_route_revision(owner, original.plan(), &edited)
            .await?;
        storage.close().await;
        let reopened = Storage::open(&path).await?;
        assert_eq!(super::super::seed(&reopened).await?, corpus);
        assert_eq!(
            reopened.route_plan(owner, receipt.plan_id()).await?,
            Some(saved)
        );
        assert_eq!(reopened.route_plans(owner).await?.len(), CASES.len());
        reopened.close().await;
        Ok(())
    })
}
