use garmin_model::{identity::UserId, route::CourseGenerationOperationId};
use tempfile::{TempDir, tempdir};

use super::*;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::test]
async fn downloads_are_owned_and_restore_drains_active_but_revokes_pending_delivery() -> TestResult
{
    let fixture = fixture().await?;
    let receipt = import_walk(&fixture).await?;
    assert!(fixture.other.download(receipt.artifact_id()).await.is_err());
    let pending = fixture
        .session
        .download(receipt.artifact_id())
        .await
        .map_err(|error| error.message)?;
    let prepared = prepared_restore(&fixture).await?;
    let active = fixture
        .session
        .download(receipt.artifact_id())
        .await
        .map_err(|error| error.message)?
        .begin()
        .await
        .map_err(|error| error.message)?;
    let deployment = Arc::clone(&fixture.operations.deployment);
    let epoch = fixture.session.epoch;
    let actor = fixture.session.actor;
    let mut restoring =
        tokio::spawn(async move { deployment.restore(epoch, actor, prepared).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut restoring)
            .await
            .is_err()
    );
    drop(active);
    tokio::time::timeout(Duration::from_secs(5), restoring).await???;
    assert!(matches!(
        pending.begin().await,
        Err(RouteFailure {
            kind: Kind::Stale,
            ..
        })
    ));
    fixture.operations.deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn restore_waits_for_active_course_encoding_then_selects_only_the_backup() -> TestResult {
    let fixture = fixture().await?;
    let imported = import_walk(&fixture).await?;
    let revision = imported.revision_id();
    let RouteReply::GenerationReady(operation) = request(
        &fixture.session,
        RouteRequest::PrepareGeneration { revision },
    )
    .await?
    else {
        return Err("expected generation intent".into());
    };
    let prepared = prepared_restore(&fixture).await?;
    let gate = Arc::new(crate::CourseGenerationGate::default());
    let deployment = Arc::clone(&fixture.operations.deployment);
    let app = deployment.application(fixture.session.epoch).await?;
    *app.course_generation_gate
        .lock()
        .expect("course generation gate") = Some(Arc::clone(&gate));
    drop(app);

    let session = fixture.session.clone();
    let generating = tokio::spawn(async move {
        session
            .request(RouteRequest::Generate {
                operation,
                revision,
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), gate.started.notified()).await?;
    let epoch = fixture.session.epoch;
    let actor = fixture.session.actor;
    let mut restoring =
        tokio::spawn(async move { deployment.restore(epoch, actor, prepared).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut restoring)
            .await
            .is_err()
    );
    gate.resume.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), generating)
            .await??
            .map_err(|error| error.message)?,
        RouteReply::Generated(_)
    ));
    tokio::time::timeout(Duration::from_secs(5), restoring).await???;
    assert!(
        fixture
            .session
            .request(RouteRequest::Versions {
                revision,
                offset: 0
            })
            .await
            .is_err()
    );
    let app = fixture
        .operations
        .deployment
        .application(fixture.operations.deployment.epoch())
        .await?;
    assert!(
        app.storage
            .course_generation(actor.user_id(), operation)
            .await?
            .is_none()
    );
    drop(app);
    fixture.operations.deployment.close().await;
    Ok(())
}

struct Fixture {
    root: TempDir,
    operations: Arc<RouteOperations>,
    session: RouteSession,
    other: RouteSession,
}

async fn prepared_restore(
    fixture: &Fixture,
) -> TestResult<garmin_storage::snapshot::PreparedRestore> {
    let archive = fixture.root.path().join("backup.tar.zst");
    fixture
        .operations
        .deployment
        .backup(
            fixture.session.epoch,
            fixture.session.actor,
            &archive,
            garmin_storage::snapshot::Limits::default(),
            &garmin_progress::CancellationToken::default(),
        )
        .await?;
    Ok(garmin_storage::snapshot::PreparedRestore::read(
        std::fs::File::open(archive)?,
        fixture.root.path(),
        garmin_storage::snapshot::Limits::default(),
    )
    .await?)
}

async fn fixture() -> TestResult<Fixture> {
    let root = tempdir()?;
    let deployment = Deployment::open(root.path().join("live")).await?;
    let app = deployment.application(deployment.epoch()).await?;
    let owner = UserContext::new(app.create_profile("Walker".parse()?).await?.id());
    let other = UserContext::new(app.create_profile("Other".parse()?).await?.id());
    drop(app);
    let operations = RouteOperations::new(deployment, Parser::new("/missing/test-gpx-worker")?);
    let session = operations
        .connect(owner)
        .await
        .map_err(|error| error.message)?;
    let other = operations
        .connect(other)
        .await
        .map_err(|error| error.message)?;
    Ok(Fixture {
        root,
        operations,
        session,
        other,
    })
}

async fn request(session: &RouteSession, request: RouteRequest) -> TestResult<RouteReply> {
    session
        .request(request)
        .await
        .map_err(|error| error.message.into())
}

async fn start(session: &RouteSession, total: u64) -> TestResult<AcquisitionOperationId> {
    let RouteReply::Upload(upload) = request(
        session,
        RouteRequest::StartUpload {
            file_name: "walk.gpx".into(),
            size: ByteCount::from_u64(total),
        },
    )
    .await?
    else {
        return Err("expected upload".into());
    };
    Ok(upload.operation)
}

#[tokio::test]
async fn uploads_are_bounded_ordered_idempotent_and_profile_scoped() -> TestResult {
    let fixture = fixture().await?;
    assert!(
        fixture
            .operations
            .connect(UserContext::new(UserId::new_v4()))
            .await
            .is_err()
    );
    for (name, size) in [
        ("../walk.gpx", 1),
        ("walk.gpx", 0),
        ("walk.gpx", garmin_gpx::MAX_BYTES as u64 + 1),
    ] {
        assert!(
            fixture
                .session
                .request(RouteRequest::StartUpload {
                    file_name: name.into(),
                    size: ByteCount::from_u64(size)
                })
                .await
                .is_err()
        );
    }
    let operation = start(&fixture.session, 4).await?;
    let chunk = || RouteRequest::Append {
        operation,
        offset: 0,
        bytes: vec![1, 2],
    };
    request(&fixture.session, chunk()).await?;
    request(&fixture.session, chunk()).await?;
    for bad in [
        RouteRequest::Append {
            operation,
            offset: 0,
            bytes: vec![3, 4],
        },
        RouteRequest::Append {
            operation,
            offset: 3,
            bytes: vec![3],
        },
        RouteRequest::Append {
            operation,
            offset: 2,
            bytes: vec![3, 4, 5],
        },
        RouteRequest::Inspect { operation },
    ] {
        assert!(fixture.session.request(bad).await.is_err());
    }
    assert!(
        fixture
            .other
            .request(RouteRequest::UploadStatus { operation })
            .await
            .is_err()
    );
    assert!(
        fixture
            .other
            .request(RouteRequest::Cancel { operation })
            .await
            .is_err()
    );
    request(
        &fixture.session,
        RouteRequest::Append {
            operation,
            offset: 2,
            bytes: vec![3, 4],
        },
    )
    .await?;
    let RouteReply::Upload(upload) =
        request(&fixture.session, RouteRequest::UploadStatus { operation }).await?
    else {
        return Err("expected upload".into());
    };
    assert_eq!(upload.received.as_u64(), 4);
    request(&fixture.session, RouteRequest::Cancel { operation }).await?;
    assert!(fixture.operations.pending().uploads.is_empty());
    fixture.operations.deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn cancellation_expiry_and_restore_release_pending_uploads() -> TestResult {
    let fixture = fixture().await?;
    let operation = start(&fixture.session, 1).await?;
    request(
        &fixture.session,
        RouteRequest::Append {
            operation,
            offset: 0,
            bytes: vec![0],
        },
    )
    .await?;
    request(&fixture.session, RouteRequest::Inspect { operation }).await?;
    request(&fixture.session, RouteRequest::Cancel { operation }).await?;
    assert!(fixture.operations.pending().uploads.is_empty());
    for _ in 0..MAX_UPLOADS {
        start(&fixture.session, 1).await?;
    }
    assert!(start(&fixture.other, 1).await.is_err());
    for upload in fixture.operations.pending().uploads.values_mut() {
        upload.expires = Instant::now();
    }
    fixture.operations.prune();
    assert!(fixture.operations.pending().uploads.is_empty());
    let operation = start(&fixture.session, 1).await?;
    let root = tempfile::tempdir()?;
    let mut replacement = Application::new(
        garmin_storage::Storage::open(root.path().join("replacement.sqlite3")).await?,
    );
    let owner = replacement.create_profile("Restored".parse()?).await?;
    let archive = root.path().join("snapshot.tar.zst");
    replacement
        .snapshot(
            UserContext::new(owner.id()),
            &archive,
            garmin_storage::snapshot::Limits::default(),
        )
        .await?;
    replacement.close().await;
    let prepared = garmin_storage::snapshot::PreparedRestore::read(
        std::fs::File::open(archive)?,
        root.path(),
        garmin_storage::snapshot::Limits::default(),
    )
    .await?;
    fixture
        .operations
        .deployment
        .restore(fixture.session.epoch, fixture.session.actor, prepared)
        .await?;
    assert!(matches!(
        fixture
            .session
            .request(RouteRequest::UploadStatus { operation })
            .await,
        Err(RouteFailure {
            kind: Kind::Stale,
            ..
        })
    ));
    fixture.operations.prune();
    assert!(fixture.operations.pending().uploads.is_empty());
    fixture.operations.deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn active_confirmation_cannot_be_cancelled_and_guard_recovers_on_drop() -> TestResult {
    let fixture = fixture().await?;
    let operation = start(&fixture.session, 1).await?;
    fixture
        .operations
        .pending()
        .uploads
        .get_mut(&operation)
        .ok_or("missing upload")?
        .confirmations = 1;
    let guard = Confirmation {
        operations: Arc::clone(&fixture.operations),
        operation,
    };
    assert!(matches!(
        fixture
            .session
            .request(RouteRequest::Cancel { operation })
            .await,
        Err(RouteFailure {
            kind: Kind::InvalidState,
            ..
        })
    ));
    drop(guard);
    request(&fixture.session, RouteRequest::Cancel { operation }).await?;
    assert!(
        fixture
            .session
            .request(RouteRequest::Generate {
                operation: CourseGenerationOperationId::new_v4(),
                revision: RoutePlanRevisionId::new_v4()
            })
            .await
            .is_err()
    );
    fixture.operations.deployment.close().await;
    Ok(())
}

async fn import_walk(fixture: &Fixture) -> TestResult<garmin_model::route::RouteImportReceipt> {
    use garmin_model::{
        identity::{Source, SourceId},
        route::{RouteCandidateSource, RouteSport},
        value::Timestamp,
    };
    let app = fixture
        .operations
        .deployment
        .application(fixture.session.epoch)
        .await?;
    let source = Source::from_parts(
        SourceId::new_v4(),
        fixture.session.actor.user_id(),
        "GPX".parse()?,
        None,
    );
    let bytes = br#"<gpx version="1.1" creator="test"><trk><trkseg><trkpt lat="50" lon="14"/><trkpt lat="50.1" lon="14.1"/></trkseg></trk></gpx>"#;
    let receipt = app
        .import_route(crate::RouteImportRequest::from_parts(
            fixture.session.actor,
            &source,
            "walk.gpx".parse()?,
            AcquisitionOperationId::new_v4(),
            Timestamp::from_unix_seconds(1_788_198_400)?,
            bytes,
            RouteCandidateSource::TrackSegment {
                track: 0,
                segment: 0,
            },
            "Walk".parse()?,
            RouteSport::Walking,
        ))
        .await?;
    Ok(receipt)
}

#[tokio::test]
async fn course_deletion_is_profile_scoped_and_revokes_new_downloads() -> TestResult {
    let fixture = fixture().await?;
    let imported = import_walk(&fixture).await?;
    let revision = imported.revision_id();
    let RouteReply::GenerationReady(operation) = fixture
        .session
        .request(RouteRequest::PrepareGeneration { revision })
        .await
        .map_err(|e| e.message)?
    else {
        panic!("generation intent");
    };
    let RouteReply::Generated(course) = fixture
        .session
        .request(RouteRequest::Generate {
            operation,
            revision,
        })
        .await
        .map_err(|e| e.message)?
    else {
        panic!("generated course");
    };
    let request = RouteRequest::DeleteCourse {
        revision,
        generation: course.id,
    };
    assert!(fixture.other.request(request.clone()).await.is_err());
    assert!(fixture.session.download(course.artifact).await.is_ok());
    assert!(matches!(
        fixture
            .session
            .request(request.clone())
            .await
            .map_err(|e| e.message)?,
        RouteReply::CourseDeleted { .. }
    ));
    fixture
        .session
        .request(request)
        .await
        .map_err(|e| e.message)?;
    assert!(fixture.session.download(course.artifact).await.is_err());
    assert!(
        fixture
            .session
            .download(imported.artifact_id())
            .await
            .is_ok()
    );
    assert!(
        fixture
            .session
            .request(RouteRequest::Generate {
                operation,
                revision
            })
            .await
            .is_err()
    );
    let RouteReply::Versions { items, .. } = fixture
        .session
        .request(RouteRequest::Versions {
            revision,
            offset: 0,
        })
        .await
        .map_err(|e| e.message)?
    else {
        panic!("versions");
    };
    assert!(items.is_empty());
    fixture.operations.deployment.close().await;
    Ok(())
}
