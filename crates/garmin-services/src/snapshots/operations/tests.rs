use std::{error::Error, sync::Arc, time::Duration};

use garmin_service_api::snapshots::{
    SnapshotReply as Reply, SnapshotRequest as Request, SnapshotState as State,
};
use tempfile::TempDir;

use super::{
    Deployment, FailureKind, Limits, MAX_SNAPSHOT_CHUNK, SnapshotOperations, SnapshotSession,
    SnapshotStatus, UserContext, Uuid,
};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

#[tokio::test]
async fn removed_owner_can_reconnect_only_to_its_restore_outcome() -> Result {
    let (_source_root, _source, _source_operations, source_session, _) = setup().await?;
    let bytes = backup(&source_session).await?;
    let (_target_root, target, operations, session, actor) = setup().await?;
    let operation = upload(&session, &bytes).await?;
    let preview = wait(&session, operation, State::AwaitingApproval)
        .await?
        .preview
        .ok_or("preview")?;
    session
        .execute(Request::Approve {
            operation,
            approval: preview.approval,
        })
        .await
        .map_err(debug_error)?;
    wait(&session, operation, State::Completed).await?;
    assert!(operations.connect(actor).await.is_err());
    let reconnected = operations
        .reconnect(actor, operation)
        .await
        .map_err(debug_error)?;
    assert_eq!(
        status(
            reconnected
                .execute(Request::Resume { operation })
                .await
                .map_err(debug_error)?
        )?
        .state,
        State::Completed
    );
    assert!(
        reconnected
            .execute(Request::BeginBackup {
                operation: Uuid::new_v4()
            })
            .await
            .is_err()
    );
    assert!(
        reconnected
            .execute(Request::Read {
                operation,
                offset: 0,
                max_bytes: 1
            })
            .await
            .is_err()
    );
    reconnected
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    target.close().await;
    Ok(())
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "Acceptance follows one server file through save, reconnect, restore, and restart"
)]
async fn server_file_save_restore_reconnect_and_restart() -> Result {
    use garmin_service_api::files::{Operation, Selection};
    let (root, deployment, operations, session, actor) = setup().await?;
    let files = tempfile::tempdir()?;
    let destination = files.path().join("backup.tar.zst");
    let path = destination.to_str().ok_or("fixture path")?.to_owned();
    let save = Uuid::new_v4();
    session
        .file(
            save,
            Selection {
                operation: Operation::Save,
                path: path.clone(),
                replace: false,
            },
        )
        .await
        .map_err(debug_error)?;
    wait(&session, save, State::Completed).await?;
    let original = std::fs::read(&destination)?;
    assert!(!original.is_empty());
    assert!(
        session
            .file(
                Uuid::new_v4(),
                Selection {
                    operation: Operation::Save,
                    path: path.clone(),
                    replace: false
                }
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&destination)?, original);
    assert!(
        session
            .file(
                Uuid::new_v4(),
                Selection {
                    operation: Operation::Save,
                    path: root
                        .path()
                        .join("forbidden.tar.zst")
                        .to_str()
                        .ok_or("fixture path")?
                        .into(),
                    replace: true
                }
            )
            .await
            .is_err()
    );
    let epoch = deployment.epoch();
    deployment
        .application(epoch)
        .await?
        .create_profile("After backup".parse()?)
        .await?;
    let restore = Uuid::new_v4();
    session
        .file(
            restore,
            Selection {
                operation: Operation::Open,
                path: path.clone(),
                replace: false,
            },
        )
        .await
        .map_err(debug_error)?;
    let preview = wait(&session, restore, State::AwaitingApproval)
        .await?
        .preview
        .ok_or("preview")?;
    assert_eq!(
        deployment.application(epoch).await?.profiles().await?.len(),
        2
    );
    let reconnected = operations.connect(actor).await.map_err(debug_error)?;
    let resumed = status(
        reconnected
            .execute(Request::Resume { operation: restore })
            .await
            .map_err(debug_error)?,
    )?;
    let approval = resumed.preview.ok_or("resumed preview")?.approval;
    assert_ne!(approval, preview.approval);
    assert!(
        reconnected
            .execute(Request::Approve {
                operation: restore,
                approval: preview.approval
            })
            .await
            .is_err()
    );
    reconnected
        .execute(Request::Approve {
            operation: restore,
            approval,
        })
        .await
        .map_err(debug_error)?;
    wait(&reconnected, restore, State::Completed).await?;
    assert!(deployment.application(epoch).await.is_err());
    assert_eq!(
        deployment
            .application(deployment.epoch())
            .await?
            .profiles()
            .await?
            .len(),
        1
    );
    deployment.close().await;
    drop((session, reconnected, operations, deployment));
    let reopened = Deployment::open(root.path()).await?;
    assert_eq!(
        reopened
            .application(reopened.epoch())
            .await?
            .profiles()
            .await?
            .len(),
        1
    );
    reopened.close().await;
    Ok(())
}

#[tokio::test]
async fn start_response_can_be_lost_without_duplicating_the_operation() -> Result {
    let (_root, _deployment, operations, session, actor) = setup().await?;
    let operation = Uuid::new_v4();
    session
        .execute(Request::BeginRestore {
            operation,
            bytes: 1,
        })
        .await
        .map_err(debug_error)?;
    assert!(
        session
            .execute(Request::BeginRestore {
                operation,
                bytes: 1
            })
            .await
            .is_err()
    );
    let reconnected = operations.connect(actor).await.map_err(debug_error)?;
    let resumed = status(
        reconnected
            .execute(Request::Resume { operation })
            .await
            .map_err(debug_error)?,
    )?;
    assert_eq!(resumed.operation, operation);
    assert_eq!(resumed.state, State::Uploading);
    reconnected
        .execute(Request::Cancel { operation })
        .await
        .map_err(debug_error)?;
    reconnected
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    Ok(())
}

async fn setup() -> Result<(
    TempDir,
    Arc<Deployment>,
    Arc<SnapshotOperations>,
    SnapshotSession,
    UserContext,
)> {
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path()).await?;
    let epoch = deployment.epoch();
    let owner = deployment
        .application(epoch)
        .await?
        .create_profile("Backup owner".parse()?)
        .await?;
    let actor = UserContext::new(owner.id());
    let operations =
        SnapshotOperations::new(Arc::clone(&deployment), Limits::default()).map_err(debug_error)?;
    let session = operations.connect(actor).await.map_err(debug_error)?;
    Ok((root, deployment, operations, session, actor))
}

fn debug_error(error: impl std::fmt::Debug) -> Box<dyn Error> {
    std::io::Error::other(format!("{error:?}")).into()
}
fn status(reply: Reply) -> Result<SnapshotStatus> {
    match reply {
        Reply::Status(status) => Ok(status),
        _ => Err("expected operation status".into()),
    }
}

async fn wait(
    session: &SnapshotSession,
    operation: Uuid,
    expected: State,
) -> Result<SnapshotStatus> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let status = status(
                session
                    .execute(Request::Status { operation })
                    .await
                    .map_err(debug_error)?,
            )?;
            if status.state == expected {
                return Ok(status);
            }
            if status.state == State::Failed {
                return Err(debug_error(status));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?
}

async fn backup(session: &SnapshotSession) -> Result<Vec<u8>> {
    let operation = status(
        session
            .execute(Request::BeginBackup {
                operation: uuid::Uuid::new_v4(),
            })
            .await
            .map_err(debug_error)?,
    )?
    .operation;
    wait(session, operation, State::DownloadReady).await?;
    let mut bytes = Vec::new();
    loop {
        let reply = session
            .execute(Request::Read {
                operation,
                offset: bytes.len() as u64,
                max_bytes: MAX_SNAPSHOT_CHUNK,
            })
            .await
            .map_err(debug_error)?;
        let Reply::Chunk {
            bytes: chunk, end, ..
        } = reply
        else {
            return Err("expected download chunk".into());
        };
        bytes.extend(chunk);
        if end {
            break;
        }
    }
    session
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    Ok(bytes)
}

async fn upload(session: &SnapshotSession, bytes: &[u8]) -> Result<Uuid> {
    let operation = status(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: bytes.len() as u64,
            })
            .await
            .map_err(debug_error)?,
    )?
    .operation;
    let mut offset = 0;
    for chunk in bytes.chunks(MAX_SNAPSHOT_CHUNK as usize) {
        session
            .execute(Request::Upload {
                operation,
                offset,
                bytes: chunk.to_vec(),
            })
            .await
            .map_err(debug_error)?;
        offset += chunk.len() as u64;
    }
    session
        .execute(Request::Verify { operation })
        .await
        .map_err(debug_error)?;
    wait(session, operation, State::AwaitingApproval).await?;
    Ok(operation)
}

#[tokio::test]
async fn backup_restore_requires_approval_and_rejects_replay() -> Result {
    let (_root, deployment, _operations, session, _actor) = setup().await?;
    let epoch = deployment.epoch();
    let bytes = backup(&session).await?;
    deployment
        .application(epoch)
        .await?
        .create_profile("After backup".parse()?)
        .await?;
    let operation = upload(&session, &bytes).await?;
    assert_eq!(
        deployment.application(epoch).await?.profiles().await?.len(),
        2
    );
    let preview = wait(&session, operation, State::AwaitingApproval)
        .await?
        .preview
        .ok_or("missing preview")?;
    let lease = deployment.application(epoch).await?;
    session
        .execute(Request::Approve {
            operation,
            approval: preview.approval,
        })
        .await
        .map_err(debug_error)?;
    assert_eq!(
        session
            .execute(Request::Cancel { operation })
            .await
            .expect_err("switch is non-cancellable")
            .kind,
        FailureKind::InvalidState
    );
    drop(lease);
    let done = wait(&session, operation, State::Completed).await?;
    assert_ne!(done.epoch, epoch);
    assert_eq!(
        deployment
            .application(done.epoch)
            .await?
            .profiles()
            .await?
            .len(),
        1
    );
    assert!(
        session
            .execute(Request::Approve {
                operation,
                approval: preview.approval
            })
            .await
            .is_err()
    );
    assert!(deployment.application(epoch).await.is_err());
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn reconnect_rotates_approval_and_revokes_the_old_session() -> Result {
    let (_root, deployment, operations, session, actor) = setup().await?;
    let bytes = backup(&session).await?;
    let operation = upload(&session, &bytes).await?;
    let old = wait(&session, operation, State::AwaitingApproval)
        .await?
        .preview
        .ok_or("missing preview")?
        .approval;
    let new_session = operations.connect(actor).await.map_err(debug_error)?;
    let resumed = status(
        new_session
            .execute(Request::Resume { operation })
            .await
            .map_err(debug_error)?,
    )?;
    let new = resumed.preview.ok_or("missing preview")?.approval;
    assert_ne!(old, new);
    assert_eq!(
        new_session
            .execute(Request::Approve {
                operation,
                approval: old
            })
            .await
            .expect_err("approval rotated")
            .kind,
        FailureKind::InvalidApproval
    );
    assert_eq!(
        session
            .execute(Request::Approve {
                operation,
                approval: old
            })
            .await
            .expect_err("old session revoked")
            .kind,
        FailureKind::Stale
    );
    new_session
        .execute(Request::Approve {
            operation,
            approval: new,
        })
        .await
        .map_err(debug_error)?;
    wait(&new_session, operation, State::Completed).await?;
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn rejects_members_invalid_chunks_and_incomplete_uploads() -> Result {
    let (_root, deployment, operations, session, _actor) = setup().await?;
    let epoch = deployment.epoch();
    let member = deployment
        .application(epoch)
        .await?
        .create_profile("Member".parse()?)
        .await?;
    assert!(
        operations
            .connect(UserContext::new(member.id()))
            .await
            .is_err()
    );
    assert!(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: 0
            })
            .await
            .is_err()
    );
    assert!(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: u64::MAX
            })
            .await
            .is_err()
    );
    let operation = status(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: 10,
            })
            .await
            .map_err(debug_error)?,
    )?
    .operation;
    for (offset, bytes) in [
        (1, vec![1]),
        (0, vec![]),
        (0, vec![1; MAX_SNAPSHOT_CHUNK as usize + 1]),
        (0, vec![1; 11]),
    ] {
        assert!(
            session
                .execute(Request::Upload {
                    operation,
                    offset,
                    bytes
                })
                .await
                .is_err()
        );
    }
    assert!(
        session
            .execute(Request::Verify { operation })
            .await
            .is_err()
    );
    session
        .execute(Request::Cancel { operation })
        .await
        .map_err(debug_error)?;
    assert!(
        session
            .execute(Request::Upload {
                operation,
                offset: 0,
                bytes: vec![1]
            })
            .await
            .is_err()
    );
    session
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    assert!(
        session
            .execute(Request::Status { operation })
            .await
            .is_err()
    );
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn download_handles_cannot_read_or_release_a_reused_operation_id() -> Result {
    let (_root, deployment, operations, session, _actor) = setup().await?;
    let session = Arc::new(session);
    let operation = Uuid::new_v4();
    session
        .execute(Request::BeginBackup { operation })
        .await
        .map_err(debug_error)?;
    wait(&session, operation, State::DownloadReady).await?;
    let old = session.download(operation).await.map_err(debug_error)?;
    assert!(old.size() > 0);
    old.read(0, MAX_SNAPSHOT_CHUNK).await.map_err(debug_error)?;
    session
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;

    session
        .execute(Request::BeginBackup { operation })
        .await
        .map_err(debug_error)?;
    wait(&session, operation, State::DownloadReady).await?;
    assert_eq!(
        old.read(0, MAX_SNAPSHOT_CHUNK)
            .await
            .expect_err("old download is stale")
            .kind,
        FailureKind::Stale
    );
    assert_eq!(
        old.release().expect_err("old cleanup is stale").kind,
        FailureKind::Stale
    );
    let current = session.download(operation).await.map_err(debug_error)?;
    current
        .read(0, MAX_SNAPSHOT_CHUNK)
        .await
        .map_err(debug_error)?;
    // Cleanup also releases an explicitly cancelled original operation.
    session
        .execute(Request::Cancel { operation })
        .await
        .map_err(debug_error)?;
    current.release().map_err(debug_error)?;
    assert!(operations.entries().is_empty());
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn cancelled_workers_cannot_update_a_reused_operation_id() -> Result {
    for worker in ["backup", "verify", "import"] {
        let (root, deployment, operations, session, actor) = setup().await?;
        let operation = Uuid::new_v4();
        let cancel = garmin_progress::CancellationToken::default();
        session
            .begin(
                operation,
                Some(1),
                super::SnapshotSource::Upload,
                None,
                cancel.clone(),
            )
            .map_err(debug_error)?;
        session.upload(operation, 0, &[0]).map_err(debug_error)?;
        let directory = operations.entries()[&operation]
            .directory
            .as_ref()
            .ok_or("staging directory")?
            .clone();
        let task = match worker {
            "backup" => super::worker::backup(
                Arc::clone(&operations),
                operation,
                deployment.epoch(),
                actor,
                directory,
                cancel,
                None,
            ),
            "verify" => {
                super::worker::verify(Arc::clone(&operations), operation, directory, cancel)
            }
            _ => {
                let input = std::fs::File::open(directory.path().join("upload.tar.zst"))?;
                drop(directory);
                super::worker::import_file(Arc::clone(&operations), operation, input, cancel)
            }
        };
        // No yield before replacement: the old task first runs with this ID assigned to a fresh upload.
        session.cancel(operation).map_err(debug_error)?;
        session.release(operation, None).map_err(debug_error)?;
        session
            .begin(
                operation,
                Some(3),
                super::SnapshotSource::Upload,
                None,
                garmin_progress::CancellationToken::default(),
            )
            .map_err(debug_error)?;
        tokio::time::timeout(Duration::from_secs(15), task).await??;
        let current = status(
            session
                .status(operation, false, false)
                .await
                .map_err(debug_error)?,
        )?;
        assert_eq!(
            current.state,
            State::Uploading,
            "old {worker} changed replacement state"
        );
        assert_eq!(current.transferred, 0);
        assert_eq!(current.total, Some(3));
        session
            .upload(operation, 0, &[1, 2, 3])
            .map_err(debug_error)?;
        session.cancel(operation).map_err(debug_error)?;
        session.release(operation, None).map_err(debug_error)?;
        assert_eq!(
            std::fs::read_dir(root.path().join(".tmp/snapshots"))?.count(),
            0,
            "old {worker} retained its staging directory"
        );
        deployment.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn cancellation_discards_backup_and_upload_files() -> Result {
    let (root, deployment, _operations, session, _actor) = setup().await?;
    let epoch = deployment.epoch();
    let lease = deployment.application(epoch).await?;
    let operation = status(
        session
            .execute(Request::BeginBackup {
                operation: uuid::Uuid::new_v4(),
            })
            .await
            .map_err(debug_error)?,
    )?
    .operation;
    session
        .execute(Request::Cancel { operation })
        .await
        .map_err(debug_error)?;
    session
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    drop(lease);
    tokio::time::timeout(Duration::from_secs(15), async {
        while std::fs::read_dir(root.path().join(".tmp/snapshots"))
            .map_err(debug_error)?
            .count()
            != 0
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await??;
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn restore_invalidates_other_prepared_approvals() -> Result {
    let (_root, deployment, _operations, session, _actor) = setup().await?;
    let bytes = backup(&session).await?;
    let first = upload(&session, &bytes).await?;
    let second = upload(&session, &bytes).await?;
    let approval = wait(&session, first, State::AwaitingApproval)
        .await?
        .preview
        .ok_or("missing preview")?
        .approval;
    session
        .execute(Request::Approve {
            operation: first,
            approval,
        })
        .await
        .map_err(debug_error)?;
    wait(&session, first, State::Completed).await?;
    let stale = status(
        session
            .execute(Request::Status { operation: second })
            .await
            .map_err(debug_error)?,
    )?;
    assert_eq!(stale.state, State::Failed);
    assert!(stale.preview.is_none());
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn remote_service_uses_the_same_operation_engine() -> Result {
    use garmin_service_api::snapshots::{SnapshotService as _, SnapshotServiceServerShared};
    use remoc::rtc::ServerShared as _;
    let (_root, deployment, _operations, session, _actor) = setup().await?;
    let (server, client) =
        SnapshotServiceServerShared::<_, remoc::codec::Default>::new(Arc::new(session));
    let serving = tokio::spawn(server.serve());
    let operation = status(
        client
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: 10,
            })
            .await?
            .map_err(debug_error)?,
    )?
    .operation;
    assert_eq!(
        status(
            client
                .execute(Request::Cancel { operation })
                .await?
                .map_err(debug_error)?
        )?
        .state,
        State::Cancelled
    );
    drop(client);
    serving.await??;
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn verification_cancellation_cannot_publish_a_late_approval() -> Result {
    let (root, deployment, _operations, session, _actor) = setup().await?;
    let operation = status(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: 4,
            })
            .await
            .map_err(debug_error)?,
    )?
    .operation;
    session
        .execute(Request::Upload {
            operation,
            offset: 0,
            bytes: vec![1, 2, 3, 4],
        })
        .await
        .map_err(debug_error)?;
    session
        .execute(Request::Verify { operation })
        .await
        .map_err(debug_error)?;
    session
        .execute(Request::Cancel { operation })
        .await
        .map_err(debug_error)?;
    tokio::time::timeout(Duration::from_secs(15), async {
        while std::fs::read_dir(root.path().join(".tmp/snapshots"))
            .map_err(debug_error)?
            .count()
            != 0
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<_, Box<dyn Error>>(())
    })
    .await??;
    let result = status(
        session
            .execute(Request::Status { operation })
            .await
            .map_err(debug_error)?,
    )?;
    assert_eq!(result.state, State::Cancelled);
    assert!(result.preview.is_none());
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn restart_rejects_old_approval_ids() -> Result {
    let (root, deployment, operations, session, actor) = setup().await?;
    let bytes = backup(&session).await?;
    let operation = upload(&session, &bytes).await?;
    let approval = wait(&session, operation, State::AwaitingApproval)
        .await?
        .preview
        .ok_or("missing preview")?
        .approval;
    drop(session);
    drop(operations);
    deployment.close().await;
    drop(deployment);
    let reopened = Deployment::open(root.path()).await?;
    let operations =
        SnapshotOperations::new(Arc::clone(&reopened), Limits::default()).map_err(debug_error)?;
    let session = operations.connect(actor).await.map_err(debug_error)?;
    assert_eq!(
        session
            .execute(Request::Approve {
                operation,
                approval
            })
            .await
            .expect_err("restart revokes old IDs")
            .kind,
        FailureKind::NotFound
    );
    reopened.close().await;
    Ok(())
}

#[tokio::test]
async fn registry_and_host_limits_are_bounded() -> Result {
    let (_root, deployment, _operations, session, _actor) = setup().await?;
    assert!(
        SnapshotOperations::new(
            Arc::clone(&deployment),
            Limits {
                database_bytes: 0,
                compressed_bytes: 1
            }
        )
        .is_err()
    );
    let mut operations = Vec::new();
    for _ in 0..super::MAX_OPERATIONS {
        operations.push(
            status(
                session
                    .execute(Request::BeginRestore {
                        operation: uuid::Uuid::new_v4(),
                        bytes: 1,
                    })
                    .await
                    .map_err(debug_error)?,
            )?
            .operation,
        );
    }
    assert_eq!(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: 1
            })
            .await
            .expect_err("registry is full")
            .kind,
        FailureKind::Limit
    );
    for operation in operations {
        session
            .execute(Request::Cancel { operation })
            .await
            .map_err(debug_error)?;
        session
            .execute(Request::Release { operation })
            .await
            .map_err(debug_error)?;
    }
    assert!(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: 1
            })
            .await
            .is_ok()
    );
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn reconnect_can_discover_and_release_abandoned_uploads() -> Result {
    let (_root, deployment, operations, session, actor) = setup().await?;
    let operation = status(
        session
            .execute(Request::BeginRestore {
                operation: uuid::Uuid::new_v4(),
                bytes: 10,
            })
            .await
            .map_err(debug_error)?,
    )?
    .operation;
    drop(session);
    let reconnected = operations.connect(actor).await.map_err(debug_error)?;
    let Reply::Operations(entries) = reconnected
        .execute(Request::List)
        .await
        .map_err(debug_error)?
    else {
        return Err("expected operation list".into());
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status.operation, operation);
    assert!(entries[0].status.preview.is_none());
    assert!(!entries[0].active);
    reconnected
        .execute(Request::Recover { operation })
        .await
        .map_err(debug_error)?;
    reconnected
        .execute(Request::Cancel { operation })
        .await
        .map_err(debug_error)?;
    reconnected
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn recovery_reclaims_a_full_registry_without_stealing_live_work() -> Result {
    use garmin_service_api::snapshots::{SnapshotService as _, SnapshotServiceServerShared};
    use remoc::rtc::ServerShared as _;
    let (root, deployment, operations, session, actor) = setup().await?;
    let (server, client) =
        SnapshotServiceServerShared::<_, remoc::codec::Default>::new(Arc::new(session));
    let serving = tokio::spawn(server.serve());
    for _ in 0..super::MAX_OPERATIONS {
        client
            .execute(Request::BeginRestore {
                operation: Uuid::new_v4(),
                bytes: 10,
            })
            .await?
            .map_err(debug_error)?;
    }
    let observer = operations.connect(actor).await.map_err(debug_error)?;
    let Reply::Operations(live) = observer.execute(Request::List).await.map_err(debug_error)?
    else {
        panic!("operation list");
    };
    assert_eq!(live.len(), super::MAX_OPERATIONS);
    assert!(
        live.iter()
            .all(|entry| entry.active && entry.status.preview.is_none())
    );
    let operation = live[0].status.operation;
    assert_eq!(
        observer
            .execute(Request::Recover { operation })
            .await
            .unwrap_err()
            .kind,
        FailureKind::InvalidState
    );
    assert!(
        observer
            .execute(Request::BeginRestore {
                operation: Uuid::new_v4(),
                bytes: 10
            })
            .await
            .is_err()
    );
    // A browser reload drops its remote client; the host keeps the staged operations.
    drop(client);
    serving.await??;
    let Reply::Operations(abandoned) =
        observer.execute(Request::List).await.map_err(debug_error)?
    else {
        panic!("operation list");
    };
    assert!(abandoned.iter().all(|entry| !entry.active));
    for entry in abandoned {
        let operation = entry.status.operation;
        for request in [
            Request::Recover { operation },
            Request::Cancel { operation },
            Request::Release { operation },
        ] {
            observer.execute(request).await.map_err(debug_error)?;
        }
    }
    assert_eq!(
        std::fs::read_dir(root.path().join(".tmp/snapshots"))?.count(),
        0
    );
    observer
        .execute(Request::BeginRestore {
            operation: Uuid::new_v4(),
            bytes: 10,
        })
        .await
        .map_err(debug_error)?;
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn recovering_verified_upload_requires_fresh_approval() -> Result {
    let (_root, deployment, operations, session, actor) = setup().await?;
    let bytes = backup(&session).await?;
    let operation = upload(&session, &bytes).await?;
    let before = wait(&session, operation, State::AwaitingApproval)
        .await?
        .preview
        .ok_or("preview")?;
    drop(session);
    let reloaded = operations.connect(actor).await.map_err(debug_error)?;
    let recovered = status(
        reloaded
            .execute(Request::Recover { operation })
            .await
            .map_err(debug_error)?,
    )?;
    assert_eq!(recovered.state, State::AwaitingApproval);
    let after = recovered.preview.ok_or("recovered preview")?;
    assert_ne!(before.approval, after.approval);
    assert_eq!(before.database_sha256, after.database_sha256);
    assert_eq!(
        reloaded
            .execute(Request::Approve {
                operation,
                approval: before.approval
            })
            .await
            .unwrap_err()
            .kind,
        FailureKind::InvalidApproval
    );
    reloaded
        .execute(Request::Cancel { operation })
        .await
        .map_err(debug_error)?;
    reloaded
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn recovery_waits_for_active_download_handle() -> Result {
    let (_root, deployment, operations, session, actor) = setup().await?;
    let session = Arc::new(session);
    let operation = Uuid::new_v4();
    session
        .execute(Request::BeginBackup { operation })
        .await
        .map_err(debug_error)?;
    wait(&session, operation, State::DownloadReady).await?;
    let download = session.download(operation).await.map_err(debug_error)?;
    drop(session);
    let reloaded = operations.connect(actor).await.map_err(debug_error)?;
    assert!(
        reloaded
            .execute(Request::Recover { operation })
            .await
            .is_err()
    );
    drop(download);
    assert_eq!(
        status(
            reloaded
                .execute(Request::Recover { operation })
                .await
                .map_err(debug_error)?
        )?
        .state,
        State::DownloadReady
    );
    reloaded
        .execute(Request::Release { operation })
        .await
        .map_err(debug_error)?;
    deployment.close().await;
    Ok(())
}
