use std::{path::Path, sync::Arc, time::Duration};

use garmin_fixtures::device as fixture;
use garmin_fixtures::device::Device;
use garmin_gpx::worker::Parser;
use garmin_model::{
    artifact::AcquisitionOperationId,
    identity::{Source, SourceId, UserId},
    route::{CourseGenerationId, CourseGenerationOperationId, RouteCandidateSource, RouteSport},
    value::Timestamp,
};
use garmin_service_api::{
    course_transfer::{CourseTransferPhase, CourseTransferPreparation},
    routes::{RouteReply, RouteRequest},
};
use garmin_update::{PROFILE_MARKER_PATH, ProfileMarker, create_profile_marker};

use super::*;
use crate::{
    RouteImportRequest, maps::device::DirectoryConnector, routes::operations::RouteOperations,
};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

async fn generated_course(
    deployment: &Arc<Deployment>,
) -> TestResult<(
    UserContext,
    UserContext,
    CourseGenerationOperationId,
    CourseGenerationId,
)> {
    let epoch = deployment.epoch();
    let app = deployment.application(epoch).await?;
    let owner = UserContext::new(app.create_profile("Walker".parse()?).await?.id());
    let partner = UserContext::new(app.create_profile("Partner".parse()?).await?.id());
    let source = Source::from_parts(SourceId::new_v4(), owner.user_id(), "GPX".parse()?, None);
    let route = app.import_route(RouteImportRequest::from_parts(
        owner, &source, "walk.gpx".parse()?, AcquisitionOperationId::new_v4(),
        Timestamp::from_unix_seconds(1_788_198_400)?,
        br#"<gpx version="1.1" creator="test"><trk><trkseg><trkpt lat="50" lon="14"/><trkpt lat="50.1" lon="14.1"/></trkseg></trk></gpx>"#,
        RouteCandidateSource::TrackSegment { track: 0, segment: 0 },
        "Walk".parse()?, RouteSport::Walking,
    )).await?;
    drop(app);
    let routes = RouteOperations::new(
        Arc::clone(deployment),
        Parser::new("/missing/test-gpx-worker")?,
    );
    let session = routes.connect(owner).await.map_err(|error| error.message)?;
    let RouteReply::GenerationReady(operation) = session
        .request(RouteRequest::PrepareGeneration {
            revision: route.revision_id(),
        })
        .await
        .map_err(|error| error.message)?
    else {
        return Err("generation intent missing".into());
    };
    let RouteReply::Generated(generated) = session
        .request(RouteRequest::Generate {
            operation,
            revision: route.revision_id(),
        })
        .await
        .map_err(|error| error.message)?
    else {
        return Err("generated Course missing".into());
    };

    Ok((owner, partner, operation, generated.id))
}

async fn watch_with_partner_marker(
    root: &Path,
    partner: UserContext,
) -> TestResult<(Device, Arc<dyn Connector>, Vec<u8>)> {
    let watch = Device::open(root.join("watch"))?;
    let transport = watch.transport();
    let marker = ProfileMarker::new(partner.user_id(), "Partner")?;
    create_profile_marker(&transport, fixture::STORAGE_ID, &marker).await?;
    let before = fs::read(watch.root().join(PROFILE_MARKER_PATH))?;
    let connector: Arc<dyn Connector> = Arc::new(DirectoryConnector {
        root: watch.root().to_owned(),
        storage_id: fixture::STORAGE_ID.to_owned(),
        storage_label: fixture::STORAGE_LABEL.to_owned(),
    });
    Ok((watch, connector, before))
}

async fn settled_transfer(
    transfers: &CourseTransfers,
    actor: UserContext,
    transfer: Uuid,
    connector: &dyn Connector,
) -> anyhow::Result<CourseTransferStatus> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = transfers.status(actor, transfer, connector).await?;
            if !matches!(current.phase, CourseTransferPhase::Running) {
                break Ok::<_, anyhow::Error>(current);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

async fn assert_recovered_acceptance(
    deployment: &Arc<Deployment>,
    actor: UserContext,
    transfer: Uuid,
    connector: &dyn Connector,
) -> TestResult {
    let recovered =
        CourseTransfers::new(Arc::clone(deployment), Arc::new(MutationLocks::default()));
    assert!(matches!(
        recovered.status(actor, transfer, connector).await?.phase,
        CourseTransferPhase::Verified
    ));
    assert!(matches!(
        recovered.accept(actor, transfer, connector).await?.phase,
        CourseTransferPhase::Accepted
    ));
    assert!(matches!(
        recovered.status(actor, transfer, connector).await?.phase,
        CourseTransferPhase::Accepted
    ));
    Ok(())
}

async fn assert_confirmed_transfer_stays_confirmed(
    transfers: &CourseTransfers,
    actor: UserContext,
    generation: CourseGenerationOperationId,
    transfer: Uuid,
    connector: Arc<dyn Connector>,
) -> TestResult {
    assert!(matches!(
        transfers
            .prepare(
                actor,
                generation,
                fixture::KEY.into(),
                fixture::STORAGE_ID,
                Arc::clone(&connector)
            )
            .await?,
        CourseTransferPreparation::AlreadyOnDevice(CourseTransferStatus {
            phase: CourseTransferPhase::Accepted,
            ..
        })
    ));
    assert!(matches!(
        transfers.record_phase(
            &transfers.root.join(transfer.to_string()),
            CourseTransferPhase::Verified
        )?,
        CourseTransferPhase::Accepted
    ));
    assert!(matches!(
        transfers
            .status(actor, transfer, connector.as_ref())
            .await?
            .phase,
        CourseTransferPhase::Accepted
    ));
    Ok(())
}

#[tokio::test]
async fn sends_owned_course_without_pairing_and_recovers_exact_copy() -> TestResult {
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path().join("host")).await?;
    let (owner, partner, operation, generated_id) = generated_course(&deployment).await?;
    let (watch, connector, before) = watch_with_partner_marker(root.path(), partner).await?;
    let transfers =
        CourseTransfers::new(Arc::clone(&deployment), Arc::new(MutationLocks::default()));
    let targets = transfers
        .targets(owner, operation, fixture::KEY, connector.as_ref())
        .await?;
    assert!(
        targets
            .iter()
            .any(|target| target.storage_id == fixture::STORAGE_ID)
    );
    assert!(
        transfers
            .prepare(
                partner,
                operation,
                fixture::KEY.into(),
                fixture::STORAGE_ID,
                Arc::clone(&connector)
            )
            .await
            .is_err()
    );
    let CourseTransferPreparation::Review(review) = transfers
        .prepare(
            owner,
            operation,
            fixture::KEY.into(),
            fixture::STORAGE_ID,
            Arc::clone(&connector),
        )
        .await?
    else {
        return Err("expected transfer review".into());
    };
    let CourseTransferPreparation::Review(second_review) = transfers
        .prepare(
            owner,
            operation,
            fixture::KEY.into(),
            fixture::STORAGE_ID,
            Arc::clone(&connector),
        )
        .await?
    else {
        return Err("expected a second transfer review before the first upload".into());
    };
    assert!(transfers.approve(partner, review.approval).await.is_err());
    let started = transfers.approve(owner, review.approval).await?;
    assert!(matches!(started.phase, CourseTransferPhase::Running));
    assert_eq!(
        started
            .progress
            .as_ref()
            .map(|progress| progress.total_bytes),
        Some(review.byte_count.as_u64())
    );
    assert!(transfers.approve(owner, review.approval).await.is_err());
    let verified = settled_transfer(&transfers, owner, review.transfer, connector.as_ref()).await?;
    assert!(matches!(verified.phase, CourseTransferPhase::Verified));
    assert!(verified.progress.is_none());
    let duplicate = transfers.approve(owner, second_review.approval).await?;
    assert_eq!(duplicate.transfer, review.transfer);
    assert!(matches!(duplicate.phase, CourseTransferPhase::Verified));
    assert_eq!(
        fs::read_dir(watch.root().join("Garmin/Courses"))?.count(),
        1
    );
    assert_eq!(fs::read(watch.root().join(PROFILE_MARKER_PATH))?, before);
    assert_eq!(
        fs::read(watch.root().join("Garmin/Courses").join(&review.file_name))?.len() as u64,
        review.byte_count.as_u64()
    );
    assert!(matches!(
        transfers
            .prepare(
                owner,
                operation,
                fixture::KEY.into(),
                fixture::STORAGE_ID,
                Arc::clone(&connector)
            )
            .await?,
        CourseTransferPreparation::AlreadyOnDevice(_)
    ));
    assert_recovered_acceptance(&deployment, owner, review.transfer, connector.as_ref()).await?;
    assert_confirmed_transfer_stays_confirmed(
        &transfers,
        owner,
        operation,
        review.transfer,
        Arc::clone(&connector),
    )
    .await?;
    assert_eq!(generated_id, review.generation);
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn cleanup_requires_exact_partial_bytes_and_separate_approval() -> TestResult {
    let root = tempfile::tempdir()?;
    let watch = Device::open(root.path().join("watch"))?;
    let deployment = Deployment::open(root.path().join("host")).await?;
    let owner = UserContext::new(UserId::new_v4());
    let target = CourseTarget {
        device_key: fixture::KEY.into(),
        device_name: fixture::NAME.into(),
        device_digest: fixture::manifest().identity_digest(),
        storage_id: fixture::STORAGE_ID.into(),
        storage_label: fixture::STORAGE_LABEL.into(),
        directory: "Garmin/Courses".into(),
        free_bytes: None,
    };
    let transfer = Uuid::new_v4();
    let file_name = format!("gt-{transfer}.fit");
    let review = CourseTransferReview {
        approval: Uuid::new_v4(),
        transfer,
        generation: garmin_model::route::CourseGenerationId::new_v4(),
        generation_operation: CourseGenerationOperationId::new_v4(),
        artifact: garmin_model::artifact::ArtifactId::new_v4(),
        version: 1,
        digest: ArtifactDigest::from_bytes(b"abcdefgh"),
        byte_count: garmin_model::artifact::ByteCount::from_u64(8),
        target,
        file_name: file_name.clone(),
    };
    let intent = Intent {
        version: 1,
        actor: owner.user_id(),
        epoch: deployment.epoch(),
        path: SafeRelativePath::parse(format!("Garmin/Courses/{file_name}"))?,
        sha256: hex::encode(Sha256::digest(b"abcdefgh")),
        review,
    };
    let operations =
        CourseTransfers::new(Arc::clone(&deployment), Arc::new(MutationLocks::default()));
    let directory = operations.root.join(transfer.to_string());
    create_private_directory(&directory)?;
    write_new(
        &directory.join("intent.json"),
        &serde_json::to_vec(&intent)?,
    )?;
    write_new(&directory.join("payload.fit"), b"abcdefgh")?;
    let device_file = watch.root().join("Garmin/Courses").join(file_name);
    fs::write(&device_file, b"abcd")?;
    let connector: Arc<dyn Connector> = Arc::new(DirectoryConnector {
        root: watch.root().to_owned(),
        storage_id: fixture::STORAGE_ID.into(),
        storage_label: fixture::STORAGE_LABEL.into(),
    });
    fs::write(directory.join("payload.fit"), b"abcxefgh")?;
    assert!(
        operations
            .prepare_cleanup(owner, transfer, connector.as_ref())
            .await
            .is_err()
    );
    fs::write(directory.join("payload.fit"), b"abcdefgh")?;
    let cleanup = operations
        .prepare_cleanup(owner, transfer, connector.as_ref())
        .await?;
    assert_eq!(cleanup.byte_count, 4);
    fs::write(&device_file, b"abce")?;
    assert!(
        operations
            .approve_cleanup(owner, cleanup.approval, connector.as_ref())
            .await
            .is_err()
    );
    assert_eq!(fs::read(&device_file)?, b"abce");
    let cleanup = operations
        .prepare_cleanup(owner, transfer, connector.as_ref())
        .await;
    assert!(cleanup.is_err());
    fs::write(&device_file, b"abcd")?;
    let cleanup = operations
        .prepare_cleanup(owner, transfer, connector.as_ref())
        .await?;
    assert!(matches!(
        operations
            .approve_cleanup(owner, cleanup.approval, connector.as_ref())
            .await?
            .phase,
        CourseTransferPhase::Missing
    ));
    assert!(!device_file.exists());
    deployment.close().await;
    Ok(())
}
