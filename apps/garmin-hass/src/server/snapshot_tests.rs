use super::*;
use crate::devices::{Host, demo::DemoSource};
use garmin_service_api::{
    ApplicationService as _,
    files::{Operation, Selection},
    snapshots::{
        SnapshotReply as Reply, SnapshotRequest as Request, SnapshotService as _,
        SnapshotServiceClient, SnapshotState as Phase, SnapshotStatus,
    },
};
use garmin_services::deployment::Deployment;
use std::time::Duration;

mod portable;

#[tokio::test]
async fn reloaded_connection_can_discard_an_abandoned_upload() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path().join("data")).await?;
    let owner = deployment
        .application(deployment.epoch())
        .await?
        .create_profile("Owner".parse()?)
        .await?;
    let host = Host::new(
        Box::new(DemoSource::new(
            root.path().join("device"),
            tokio::runtime::Handle::current(),
        )?),
        deployment,
    );
    let original = host.with_control(None);
    let session = original
        .snapshots(owner.id(), None)
        .await?
        .map_err(anyhow::Error::msg)?;
    let operation = uuid::Uuid::new_v4();
    session
        .execute(Request::BeginRestore {
            operation,
            bytes: 4096,
        })
        .await?
        .expect("begin upload");
    session
        .execute(Request::Upload {
            operation,
            offset: 0,
            bytes: vec![1; 128],
        })
        .await?
        .expect("partial upload");

    let reloaded = host.with_control(None);
    let recovery = reloaded
        .snapshots(owner.id(), None)
        .await?
        .map_err(anyhow::Error::msg)?;
    assert!(
        recovery
            .execute(Request::Recover { operation })
            .await?
            .is_err()
    );
    drop(session);
    // Host retains the session for download/file RPCs until its connection also closes.
    assert!(
        recovery
            .execute(Request::Recover { operation })
            .await?
            .is_err()
    );
    drop(original);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Reply::Operations(operations) = recovery
                .execute(Request::List)
                .await?
                .expect("list operations")
            else {
                anyhow::bail!("missing operations");
            };
            assert_eq!(operations.len(), 1);
            assert_eq!(operations[0].status.transferred, 128);
            if !operations[0].active {
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    for request in [
        Request::Recover { operation },
        Request::Cancel { operation },
        Request::Release { operation },
    ] {
        recovery
            .execute(request)
            .await?
            .expect("discard abandoned upload");
    }
    let Reply::Operations(operations) = recovery
        .execute(Request::List)
        .await?
        .expect("list remaining operations")
    else {
        anyhow::bail!("missing operations");
    };
    assert!(operations.is_empty());
    Ok(())
}

async fn wait(
    session: &SnapshotServiceClient,
    operation: uuid::Uuid,
    expected: Phase,
) -> anyhow::Result<SnapshotStatus> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let Reply::Status(status) = session
                .execute(Request::Status { operation })
                .await?
                .map_err(|error| anyhow::anyhow!(error.message))?
            else {
                anyhow::bail!("missing status");
            };
            if status.state == expected {
                return Ok(status);
            }
            anyhow::ensure!(status.state != Phase::Failed, "{:?}", status.error);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "Acceptance follows one download through server-file restore and connection invalidation"
)]
async fn host_download_server_files_and_database_epoch() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path().join("data")).await?;
    let epoch = deployment.epoch();
    let owner = deployment
        .application(epoch)
        .await?
        .create_profile("Owner".parse()?)
        .await?;
    let member = deployment
        .application(epoch)
        .await?
        .create_profile("Member".parse()?)
        .await?;
    let host = Host::new(
        Box::new(DemoSource::new(
            root.path().join("device"),
            tokio::runtime::Handle::current(),
        )?),
        Arc::clone(&deployment),
    );
    let connection = host.with_control(None);
    assert!(connection.snapshots(member.id(), None).await?.is_err());
    assert!(
        connection
            .server_directory(member.id(), root.path().to_string_lossy().into())
            .await?
            .is_err()
    );
    let session = connection
        .snapshots(owner.id(), None)
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
    assert!(connection.snapshot_download(operation).await?.is_err());
    let (app, _control) = router(
        Arc::clone(&host),
        garmin_map_tiles::Service::new(root.path().join("cache"))?,
        crate::BrowserOptions::default(),
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let url = format!("http://{address}/download/{}", ticket.token);
    let response = reqwest::get(&url).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = response.bytes().await?;
    assert_eq!(reqwest::get(&url).await?.status(), StatusCode::NOT_FOUND);
    let path = root.path().join("backup.tar.zst");
    std::fs::write(&path, bytes)?;
    deployment
        .application(epoch)
        .await?
        .create_profile("After backup".parse()?)
        .await?;
    let directory = connection
        .server_directory(owner.id(), root.path().to_string_lossy().into())
        .await?
        .map_err(anyhow::Error::msg)?;
    assert!(
        directory
            .entries
            .iter()
            .any(|entry| entry.path == "backup.tar.zst")
    );
    let restore = uuid::Uuid::new_v4();
    connection
        .snapshot_file(
            restore,
            Selection {
                operation: Operation::Open,
                path: path.to_string_lossy().into(),
                replace: false,
            },
        )
        .await?
        .map_err(anyhow::Error::msg)?;
    let preview = wait(&session, restore, Phase::AwaitingApproval)
        .await?
        .preview
        .expect("verified preview");
    assert_eq!(
        connection
            .profiles()
            .await?
            .map_err(anyhow::Error::msg)?
            .len(),
        3
    );
    session
        .execute(Request::Approve {
            operation: restore,
            approval: preview.approval,
        })
        .await?
        .map_err(|error| anyhow::anyhow!(error.message))?;
    wait(&session, restore, Phase::Completed).await?;
    assert!(connection.profiles().await.is_err());
    assert!(connection.heartbeat().await.is_err());
    assert_eq!(
        host.with_control(None)
            .profiles()
            .await?
            .map_err(anyhow::Error::msg)?
            .len(),
        2
    );
    server.abort();
    let _ = server.await;
    drop((session, connection, host));
    deployment.close().await;
    Ok(())
}
