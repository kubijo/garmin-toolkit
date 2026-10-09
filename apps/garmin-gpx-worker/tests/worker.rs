#![cfg(target_os = "linux")]

use std::{
    io::Write as _,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use garmin_gpx::worker::{CPU_SECONDS, Error, MEMORY_BYTES, Parser};
use garmin_model::{
    artifact::AcquisitionOperationId,
    identity::{Source, SourceId},
    route::{CourseGenerationOperationId, RouteSport},
    value::Timestamp,
};
use garmin_service_api::routes::{GpxUploadPhase, RouteReply, RouteRequest, RouteSelection};
use garmin_services::{Application, RouteImportRequest, UserContext};
use garmin_services::{
    deployment::Deployment,
    routes::operations::{RouteOperations, RouteSession},
};
use garmin_storage::Storage;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const GPX: &[u8] = include_bytes!("../../../crates/garmin-gpx/tests/fixtures/candidates.gpx");
const WORKER: &str = env!("CARGO_BIN_EXE_garmin-gpx-worker");
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn execute(
    session: &RouteSession,
    request: RouteRequest,
) -> Result<RouteReply, Box<dyn std::error::Error>> {
    session
        .request(request)
        .await
        .map_err(|error| error.message.into())
}

async fn prepare_selection(
    session: &RouteSession,
) -> Result<RouteRequest, Box<dyn std::error::Error>> {
    let RouteReply::Upload(upload) = execute(
        session,
        RouteRequest::StartUpload {
            file_name: "walk.gpx".into(),
            size: garmin_model::artifact::ByteCount::from_u64(GPX.len() as u64),
        },
    )
    .await?
    else {
        return Err("missing upload".into());
    };
    let operation = upload.operation;
    execute(
        session,
        RouteRequest::Append {
            operation,
            offset: 0,
            bytes: GPX.to_vec(),
        },
    )
    .await?;
    execute(session, RouteRequest::Inspect { operation }).await?;
    let digest = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let RouteReply::Upload(upload) =
                execute(session, RouteRequest::UploadStatus { operation }).await?
            else {
                return Err("missing status".into());
            };
            match upload.phase {
                GpxUploadPhase::Review { digest, .. } => {
                    return Ok::<_, Box<dyn std::error::Error>>(digest);
                }
                GpxUploadPhase::InvalidFile(error) | GpxUploadPhase::Failed(error) => {
                    return Err(error.into());
                }
                GpxUploadPhase::Uploading | GpxUploadPhase::Parsing => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    })
    .await??;
    let RouteReply::Candidates { items, .. } = execute(
        session,
        RouteRequest::Candidates {
            operation,
            offset: 0,
        },
    )
    .await?
    else {
        return Err("missing candidates".into());
    };
    let candidate = items
        .iter()
        .find(|candidate| candidate.geometry)
        .ok_or("missing geometry")?;
    let RouteReply::Points { points, end, .. } = execute(
        session,
        RouteRequest::PreviewPoints {
            operation,
            candidate: candidate.source,
            offset: 0,
            count: 2048,
        },
    )
    .await?
    else {
        return Err("missing points".into());
    };
    assert!(end);
    assert_eq!(points.len(), candidate.point_count as usize);
    let confirm = RouteRequest::Confirm {
        operation,
        selection: RouteSelection {
            digest,
            candidate: candidate.source,
            name: "Walk".parse().unwrap(),
            sport: RouteSport::Walking,
        },
    };
    Ok(confirm)
}

async fn download_bytes(
    session: &RouteSession,
    artifact: garmin_model::artifact::ArtifactId,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut delivery = session
        .download(artifact)
        .await
        .map_err(|error| error.message)?
        .begin()
        .await
        .map_err(|error| error.message)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = delivery.next_chunk() {
        bytes.extend_from_slice(chunk);
    }
    Ok(bytes)
}

#[tokio::test]
async fn client_upload_review_save_and_versioned_course_survive_reconnect() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path()).await?;
    let app = deployment.application(deployment.epoch()).await?;
    let actor = UserContext::new(app.create_profile("Walker".parse()?).await?.id());
    drop(app);
    let operations = RouteOperations::new(Arc::clone(&deployment), Parser::new(WORKER)?);
    let session = operations
        .connect(actor)
        .await
        .map_err(|error| error.message)?;
    let confirm = prepare_selection(&session).await?;
    let RouteReply::Imported(receipt) = execute(&session, confirm.clone()).await? else {
        return Err("missing receipt".into());
    };
    let session = operations
        .connect(actor)
        .await
        .map_err(|error| error.message)?;
    let RouteReply::Imported(retried) = execute(&session, confirm.clone()).await? else {
        return Err("missing retry".into());
    };
    assert_eq!(receipt, retried);
    let revision = receipt.revision_id();
    let RouteReply::GenerationReady(operation) =
        execute(&session, RouteRequest::PrepareGeneration { revision }).await?
    else {
        return Err("missing generation approval".into());
    };
    let RouteReply::Generated(first) = execute(
        &session,
        RouteRequest::Generate {
            operation,
            revision,
        },
    )
    .await?
    else {
        return Err("missing Course".into());
    };
    let RouteReply::Generated(retried) = execute(
        &session,
        RouteRequest::Generate {
            operation,
            revision,
        },
    )
    .await?
    else {
        return Err("missing Course retry".into());
    };
    assert_eq!(first, retried);
    assert_eq!(download_bytes(&session, receipt.artifact_id()).await?, GPX);
    let delivered = download_bytes(&session, first.artifact).await?;
    assert_eq!(
        garmin_fit::course::decode(&delivered)?.sport(),
        RouteSport::Walking
    );
    let RouteReply::Versions { items, next } = execute(
        &session,
        RouteRequest::Versions {
            revision,
            offset: 0,
        },
    )
    .await?
    else {
        return Err("missing versions".into());
    };
    assert_eq!(items, vec![first.clone()]);
    assert!(next.is_none());
    let app = deployment.application(deployment.epoch()).await?;
    let bytes = app
        .course_artifact(actor, first.artifact)
        .await?
        .ok_or("missing bytes")?;
    assert_eq!(bytes, delivered);
    assert_eq!(
        garmin_fit::course::decode(&bytes)?.sport(),
        RouteSport::Walking
    );
    drop(app);
    drop(session);
    drop(operations);
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn contained_preview_confirms_and_generates_saved_course_bytes() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let root = tempfile::tempdir()?;
    let app = Application::new(Storage::open(root.path().join("routes.sqlite3")).await?);
    let owner = UserContext::new(app.create_profile("Walker".parse()?).await?.id());
    let source = Source::from_parts(
        SourceId::new_v4(),
        owner.user_id(),
        "GPX files".parse()?,
        None,
    );
    let prepared = Parser::new(WORKER)?.parse(Arc::from(GPX)).await?;
    let candidate = prepared
        .document()
        .candidates()
        .iter()
        .find(|candidate| candidate.shape().is_geometry())
        .ok_or("missing geometry")?;
    let operation = AcquisitionOperationId::new_v4();
    let time = Timestamp::from_unix_seconds(1_788_198_400)?;
    let request = |bytes| -> Result<_, Box<dyn std::error::Error>> {
        Ok(RouteImportRequest::from_parts(
            owner,
            &source,
            "walk.gpx".parse()?,
            operation,
            time,
            bytes,
            candidate.source(),
            "Walk".parse()?,
            RouteSport::Walking,
        ))
    };
    assert!(
        app.import_prepared_route(request(b"different input")?, &prepared)
            .await
            .is_err()
    );
    assert!(app.route_plans(owner).await?.is_empty());
    let receipt = app.import_prepared_route(request(GPX)?, &prepared).await?;
    assert_eq!(
        app.import_prepared_route(request(GPX)?, &prepared).await?,
        receipt
    );
    assert_eq!(
        app.route_import(owner, operation)
            .await?
            .ok_or("missing receipt")?
            .parser(),
        prepared.parser()
    );
    let generated = app
        .generate_course(
            owner,
            CourseGenerationOperationId::new_v4(),
            receipt.revision_id(),
            time,
        )
        .await?;
    let bytes = app
        .course_artifact(owner, generated.artifact_id())
        .await?
        .ok_or("missing generated bytes")?;
    let decoded = garmin_fit::course::decode(&bytes)?;
    assert_eq!(decoded.sport(), RouteSport::Walking);
    assert_eq!(decoded.points().len(), candidate.shape().points().len());
    app.close().await;
    Ok(())
}

#[tokio::test]
async fn malformed_upload_is_reported_as_an_invalid_file() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path()).await?;
    let app = deployment.application(deployment.epoch()).await?;
    let actor = UserContext::new(app.create_profile("Walker".parse()?).await?.id());
    drop(app);
    let operations = RouteOperations::new(Arc::clone(&deployment), Parser::new(WORKER)?);
    let session = operations
        .connect(actor)
        .await
        .map_err(|error| error.message)?;
    let bytes = b"<gpx><trk /></gpx>";
    let RouteReply::Upload(upload) = execute(
        &session,
        RouteRequest::StartUpload {
            file_name: "malformed.gpx".into(),
            size: garmin_model::artifact::ByteCount::from_u64(bytes.len() as u64),
        },
    )
    .await?
    else {
        return Err("missing upload".into());
    };
    let operation = upload.operation;
    execute(
        &session,
        RouteRequest::Append {
            operation,
            offset: 0,
            bytes: bytes.to_vec(),
        },
    )
    .await?;
    execute(&session, RouteRequest::Inspect { operation }).await?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let RouteReply::Upload(upload) =
                execute(&session, RouteRequest::UploadStatus { operation }).await?
            else {
                return Err("missing upload status".into());
            };
            match upload.phase {
                GpxUploadPhase::InvalidFile(message) => {
                    assert!(message.contains("GPX could not be parsed"));
                    return Ok::<_, Box<dyn std::error::Error>>(());
                }
                GpxUploadPhase::Uploading | GpxUploadPhase::Parsing => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                other => return Err(format!("unexpected upload phase: {other:?}").into()),
            }
        }
    })
    .await??;
    assert!(matches!(
        execute(&session, RouteRequest::Cancel { operation }).await?,
        RouteReply::Cancelled
    ));
    drop(session);
    drop(operations);
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn worker_preserves_candidates_original_bytes_and_parser_identity() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let prepared = Parser::new(WORKER)?.parse(Arc::from(GPX)).await?;
    assert_eq!(prepared.document(), &garmin_gpx::parse(GPX)?);
    assert_eq!(prepared.bytes(), GPX);
    assert_eq!(prepared.parser().name(), garmin_gpx::ADAPTER_NAME);
    assert_eq!(
        prepared.parser().version().to_string(),
        garmin_gpx::ADAPTER_VERSION
    );
    Ok(())
}

#[tokio::test]
async fn worker_rejects_bad_and_excessive_input_without_poisoning_later_parses() -> TestResult {
    let _test = TEST_LOCK.lock().await;
    let parser = Parser::new(WORKER)?;
    assert!(matches!(
        parser.parse(Arc::from(b"invalid GPX".as_slice())).await,
        Err(Error::Rejected(_))
    ));
    assert!(matches!(
        parser
            .parse(vec![0; garmin_gpx::MAX_BYTES + 1].into())
            .await,
        Err(Error::InputTooLarge)
    ));
    let excessive = format!(
        "<gpx version=\"1.1\" creator=\"test\">{}</gpx>",
        "<rte><rtept lat=\"50\" lon=\"14\"/></rte>".repeat(129)
    );
    assert!(matches!(
        parser.parse(excessive.into_bytes().into()).await,
        Err(Error::Rejected(_))
    ));
    parser.parse(Arc::from(GPX)).await?;
    Ok(())
}

#[test]
fn helper_installs_limits_before_reading_the_source() -> TestResult {
    let mut child = Command::new(WORKER)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    let limits = format!("/proc/{}/limits", child.id());
    let deadline = Instant::now() + Duration::from_secs(2);
    let expected_memory = MEMORY_BYTES.to_string();
    let expected_cpu = CPU_SECONDS.to_string();
    let result = loop {
        let current = std::fs::read_to_string(&limits)?;
        let bounded = [
            ("Max address space", expected_memory.as_str()),
            ("Max cpu time", expected_cpu.as_str()),
            ("Max core file size", "0"),
        ]
        .into_iter()
        .all(|(name, limit)| {
            current
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .is_some_and(|rest| rest.split_whitespace().take(2).eq([limit, limit]))
        });
        if bounded {
            break Ok(());
        }
        if Instant::now() > deadline {
            break Err("worker did not install resource limits");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if result.is_err() {
        child.kill()?;
        child.wait()?;
    }
    result?;
    let mut stdin = child.stdin.take().ok_or("missing stdin")?;
    stdin.write_all(GPX)?;
    drop(stdin);
    assert!(child.wait()?.success());
    Ok(())
}
