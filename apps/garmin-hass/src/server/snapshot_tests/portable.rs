//! Native snapshot files and HASS transfers share the same portable data contract.

use super::*;
use garmin_model::{
    artifact::{
        Acquisition, AcquisitionId, AcquisitionOperationId, Artifact, ArtifactId,
        NormalizationOutcome, NormalizationRun, NormalizationRunId, SchemaVersion,
    },
    identity::{Source, SourceId, User},
    value::ComponentVersion,
};
use garmin_service_api::snapshots::MAX_SNAPSHOT_CHUNK;
use garmin_storage::{
    Ingestion, Storage,
    snapshot::{Limits, PreparedRestore},
};
use std::{fs::File, io::Read as _, path::Path};

const ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

async fn native_fixture(path: &Path) -> anyhow::Result<(Vec<User>, Artifact)> {
    let mut storage = Storage::open(path.with_extension("sqlite3")).await?;
    let corpus = garmin_fixtures::seed(&storage).await?;
    let owner = corpus
        .users()
        .first()
        .ok_or_else(|| anyhow::anyhow!("fixture owner"))?;
    let source = Source::from_parts(
        SourceId::new_v4(),
        owner.id(),
        "Large snapshot".parse()?,
        None,
    );
    storage.save_source(&source).await?;
    // Deterministic incompressible bytes exercise thousands of transfer chunks, not just a large zero-filled database.
    let mut bytes = vec![0; ARTIFACT_BYTES];
    blake3::Hasher::new()
        .update(b"portable snapshot acceptance")
        .finalize_xof()
        .fill(&mut bytes);
    let artifact = Artifact::from_bytes(
        ArtifactId::new_v4(),
        "application/octet-stream".parse()?,
        &bytes,
    );
    let acquisition = Acquisition::from_source(
        AcquisitionId::new_v4(),
        AcquisitionOperationId::new_v4(),
        artifact.id(),
        &source,
        "synthetic-original.bin".parse()?,
        "2026-09-29T12:00:00Z".parse()?,
    );
    let normalization = NormalizationRun::from_parts(
        NormalizationRunId::new_v4(),
        &acquisition,
        ComponentVersion::from_parts("snapshot-acceptance", "1.0.0".parse()?)?,
        SchemaVersion::from_u32(1)?,
        vec![],
        NormalizationOutcome::Failed("Synthetic opaque original".parse()?),
    );
    storage
        .ingest(Ingestion::from_parts(
            &artifact,
            &bytes,
            &acquisition,
            &normalization,
            &[],
            &[],
        )?)
        .await?;
    drop(bytes);
    let manifest = storage.snapshot(path, Limits::default()).await?;
    assert!(manifest.database_bytes > ARTIFACT_BYTES as u64);
    assert!(std::fs::metadata(path)?.len() > ARTIFACT_BYTES as u64);
    let users = storage.users().await?;
    storage.close().await;
    Ok((users, artifact))
}

async fn upload_file(session: &SnapshotServiceClient, path: &Path) -> anyhow::Result<uuid::Uuid> {
    let operation = uuid::Uuid::new_v4();
    let mut file = File::open(path)?;
    session
        .execute(Request::BeginRestore {
            operation,
            bytes: file.metadata()?.len(),
        })
        .await?
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let mut buffer = vec![0; MAX_SNAPSHOT_CHUNK as usize];
    let mut offset = 0;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        session
            .execute(Request::Upload {
                operation,
                offset,
                bytes: buffer[..count].to_vec(),
            })
            .await?
            .map_err(|error| anyhow::anyhow!(error.message))?;
        offset += count as u64;
    }
    assert_eq!(offset, file.metadata()?.len());
    session
        .execute(Request::Verify { operation })
        .await?
        .map_err(|error| anyhow::anyhow!(error.message))?;
    Ok(operation)
}

#[tokio::test]
async fn large_native_file_survives_hass_upload_restore_and_http_download() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let native = root.path().join("native.tar.zst");
    let (users, artifact) = native_fixture(&native).await?;
    let deployment = Deployment::open(root.path().join("hass")).await?;
    let epoch = deployment.epoch();
    let owner = deployment
        .application(epoch)
        .await?
        .create_profile("HASS owner".parse()?)
        .await?;
    let host = Host::new(
        Box::new(DemoSource::new(
            root.path().join("device"),
            tokio::runtime::Handle::current(),
        )?),
        Arc::clone(&deployment),
    );
    let connection = host.with_control(None);
    let session = connection
        .snapshots(owner.id(), None)
        .await?
        .map_err(anyhow::Error::msg)?;
    let operation = upload_file(&session, &native).await?;
    let preview = wait(&session, operation, Phase::AwaitingApproval)
        .await?
        .preview
        .expect("verified preview");
    assert_eq!(
        deployment.application(epoch).await?.profiles().await?.len(),
        1
    );
    session
        .execute(Request::Approve {
            operation,
            approval: preview.approval,
        })
        .await?
        .map_err(|error| anyhow::anyhow!(error.message))?;
    wait(&session, operation, Phase::Completed).await?;
    assert!(connection.profiles().await.is_err());
    drop((connection, session));

    let connection = host.with_control(None);
    let session = connection
        .snapshots(users[0].id(), None)
        .await?
        .map_err(anyhow::Error::msg)?;
    let operation = uuid::Uuid::new_v4();
    session
        .execute(Request::BeginBackup { operation })
        .await?
        .map_err(|error| anyhow::anyhow!(error.message))?;
    wait(&session, operation, Phase::DownloadReady).await?;
    let ticket = connection
        .snapshot_download(operation)
        .await?
        .map_err(anyhow::Error::msg)?;
    let (app, _control) = router(
        Arc::clone(&host),
        garmin_map_tiles::Service::new(root.path().join("cache"))?,
        crate::BrowserOptions::default(),
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let result = download_and_reopen(root.path(), address, ticket.token, &users, &artifact).await;
    server.abort();
    let _ = server.await;
    drop((connection, session, host));
    deployment.close().await;
    result
}

async fn download_and_reopen(
    root: &Path,
    address: std::net::SocketAddr,
    token: uuid::Uuid,
    users: &[User],
    artifact: &Artifact,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let path = root.join("hass.tar.zst");
    let mut response = reqwest::get(format!("http://{address}/download/{token}"))
        .await?
        .error_for_status()?;
    let expected = response.content_length().expect("known archive size");
    assert!(expected > ARTIFACT_BYTES as u64);
    let mut file = tokio::fs::File::create(&path).await?;
    let mut received = 0;
    while let Some(bytes) = response.chunk().await? {
        file.write_all(&bytes).await?;
        received += bytes.len() as u64;
    }
    file.flush().await?;
    drop(file);
    assert_eq!(received, expected);
    let restored = root.join("restored.sqlite3");
    PreparedRestore::read(File::open(path)?, root, Limits::default())
        .await?
        .publish(&restored)?;
    for _ in 0..2 {
        let storage = Storage::open(&restored).await?;
        assert_eq!(storage.users().await?, users);
        let bytes = storage
            .artifact_bytes(artifact.id())
            .await?
            .expect("original artifact");
        assert_eq!(bytes.len(), ARTIFACT_BYTES);
        assert_eq!(
            garmin_model::artifact::ArtifactDigest::from_bytes(&bytes),
            artifact.digest()
        );
        storage.check_integrity().await?;
        storage.close().await;
    }
    Ok(())
}
