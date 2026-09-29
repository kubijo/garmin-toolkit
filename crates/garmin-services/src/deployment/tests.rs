use std::{error::Error as StdError, fs::File, process::Command, sync::Arc};

use garmin_storage::{
    Storage,
    snapshot::{Limits, PreparedRestore},
};
use tempfile::TempDir;

use super::{Checkpoint, Deployment, Error};
use crate::{Application, UserContext};

type Result<T = ()> = std::result::Result<T, Box<dyn StdError>>;

async fn setup() -> Result<(TempDir, Arc<Deployment>, UserContext, PreparedRestore)> {
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path().join("live")).await?;
    let epoch = deployment.epoch();
    let owner = deployment
        .application(epoch)
        .await?
        .create_profile("Original".parse()?)
        .await?;
    let mut replacement =
        Application::new(Storage::open(root.path().join("replacement.sqlite3")).await?);
    let new_owner = replacement.create_profile("Restored".parse()?).await?;
    let archive = root.path().join("backup.tar.zst");
    replacement
        .snapshot(
            UserContext::new(new_owner.id()),
            &archive,
            Limits::default(),
        )
        .await?;
    replacement.close().await;
    let prepared =
        PreparedRestore::read(File::open(archive)?, root.path(), Limits::default()).await?;
    Ok((root, deployment, UserContext::new(owner.id()), prepared))
}

async fn name(deployment: &Deployment) -> Result<String> {
    let epoch = deployment.epoch();
    Ok(deployment.application(epoch).await?.profiles().await?[0]
        .profile()
        .display_name()
        .as_str()
        .to_owned())
}

#[tokio::test]
async fn restore_drains_leases_revokes_old_jobs_and_survives_reopen() -> Result {
    let (root, deployment, actor, prepared) = setup().await?;
    assert!(Deployment::open(root.path().join("live")).await.is_err());
    let epoch = deployment.epoch();
    let lease = deployment.application(epoch).await?;
    let worker = Arc::clone(&deployment);
    let mut restore = tokio::spawn(async move { worker.restore(epoch, actor, prepared).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut restore)
            .await
            .is_err()
    );
    drop(lease);
    restore.await??;
    assert!(matches!(
        deployment.application(epoch).await,
        Err(Error::Stale)
    ));
    assert_eq!(name(&deployment).await?, "Restored");
    deployment.close().await;
    drop(deployment);
    let reopened = Deployment::open(root.path().join("live")).await?;
    assert_eq!(name(&reopened).await?, "Restored");
    assert!(root.path().join("live/storage.sqlite3").exists());
    reopened.close().await;
    Ok(())
}

#[tokio::test]
async fn injected_failures_reopen_the_previous_database() -> Result {
    for phase in [
        Checkpoint::Journaled,
        Checkpoint::Closed,
        Checkpoint::Selected,
        Checkpoint::Opened,
    ] {
        let (_root, deployment, actor, prepared) = setup().await?;
        *deployment.hook.lock().expect("healthy hook") = Some(Box::new(move |checkpoint| {
            if checkpoint == phase {
                Err(std::io::Error::other("injected switch failure").into())
            } else {
                Ok(())
            }
        }));
        let epoch = deployment.epoch();
        assert!(deployment.restore(epoch, actor, prepared).await.is_err());
        assert_eq!(name(&deployment).await?, "Original", "phase {phase:?}");
        assert!(matches!(
            deployment.application(epoch).await,
            Err(Error::Stale)
        ));
        deployment.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn dropping_the_call_does_not_abort_an_approved_switch() -> Result {
    let (_root, deployment, actor, prepared) = setup().await?;
    let epoch = deployment.epoch();
    let lease = deployment.application(epoch).await?;
    let worker = Arc::clone(&deployment);
    let (started, observed) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = started.send(());
        worker.restore(epoch, actor, prepared).await
    });
    observed.await?;
    // The spawned restore owns the transaction independently of its caller.
    tokio::task::yield_now().await;
    task.abort();
    drop(lease);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if deployment.epoch() != epoch {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(name(&deployment).await?, "Restored");
    deployment.close().await;
    Ok(())
}

#[test]
fn process_interruption_recovers_each_durable_boundary() -> Result {
    for phase in [
        "Staged",
        "Journaled",
        "Closed",
        "Selected",
        "Opened",
        "Committed",
    ] {
        let root = tempfile::tempdir()?;
        let status = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "deployment::tests::crash_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("GARMIN_RESTORE_CRASH_ROOT", root.path())
            .env("GARMIN_RESTORE_CRASH_PHASE", phase)
            .status()?;
        assert_eq!(status.code(), Some(73), "child did not reach {phase}");
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {
            let recovered = Deployment::open(root.path().join("live")).await?;
            let expected = if phase == "Committed" {
                "Restored"
            } else {
                "Original"
            };
            assert_eq!(name(&recovered).await?, expected, "phase {phase}");
            recovered.close().await;
            Ok::<_, Box<dyn StdError>>(())
        })?;
    }
    Ok(())
}

#[test]
#[ignore = "subprocess helper for process-interruption acceptance"]
fn crash_worker() -> Result {
    let root = std::path::PathBuf::from(
        std::env::var_os("GARMIN_RESTORE_CRASH_ROOT").ok_or("missing crash root")?,
    );
    let phase = std::env::var("GARMIN_RESTORE_CRASH_PHASE")?;
    tokio::runtime::Runtime::new()?.block_on(async {
        let deployment = Deployment::open(root.join("live")).await?;
        let epoch = deployment.epoch();
        let owner = deployment
            .application(epoch)
            .await?
            .create_profile("Original".parse()?)
            .await?;
        let mut replacement =
            Application::new(Storage::open(root.join("replacement.sqlite3")).await?);
        let replacement_owner = replacement.create_profile("Restored".parse()?).await?;
        replacement
            .snapshot(
                UserContext::new(replacement_owner.id()),
                &root.join("snapshot.tar.zst"),
                Limits::default(),
            )
            .await?;
        replacement.close().await;
        let prepared = PreparedRestore::read(
            File::open(root.join("snapshot.tar.zst"))?,
            &root,
            Limits::default(),
        )
        .await?;
        *deployment.hook.lock().expect("healthy hook") = Some(Box::new(move |checkpoint| {
            if format!("{checkpoint:?}") == phase {
                std::process::exit(73);
            }
            Ok(())
        }));
        deployment
            .restore(epoch, UserContext::new(owner.id()), prepared)
            .await?;
        Err("crash phase was never reached".into())
    })
}

#[tokio::test]
async fn startup_cleans_abandoned_stages_but_retains_selected_databases() -> Result {
    let (root, deployment, actor, prepared) = setup().await?;
    deployment
        .restore(deployment.epoch(), actor, prepared)
        .await?;
    deployment.close().await;
    drop(deployment);
    let live = root.path().join("live");
    std::fs::create_dir_all(live.join(".tmp/snapshots/interrupted"))?;
    std::fs::write(
        live.join(".tmp/snapshots/interrupted/upload.tar.zst"),
        b"partial",
    )?;
    let abandoned = super::files::generation(&live)?;
    std::fs::write(abandoned.path(&live), b"unpublished")?;
    let reopened = Deployment::open(&live).await?;
    assert!(!live.join(".tmp/snapshots").exists());
    assert!(!abandoned.path(&live).exists());
    assert_eq!(name(&reopened).await?, "Restored");
    assert!(live.join("storage.sqlite3").exists());
    reopened.close().await;
    Ok(())
}

#[tokio::test]
async fn progress_remains_readable_while_the_application_is_exclusively_locked() -> Result {
    use garmin_service_api::snapshots::{SnapshotReply, SnapshotRequest, SnapshotState};
    let (_root, deployment, actor, _prepared) = setup().await?;
    let operations =
        crate::snapshots::SnapshotOperations::new(Arc::clone(&deployment), Limits::default())
            .expect("valid limits");
    let session = operations.connect(actor).await.expect("owner session");
    let SnapshotReply::Status(started) = session
        .execute(SnapshotRequest::BeginRestore {
            operation: uuid::Uuid::new_v4(),
            bytes: 10,
        })
        .await
        .expect("upload starts")
    else {
        return Err("expected status".into());
    };
    let exclusive = deployment.live.write().await;
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        session.execute(SnapshotRequest::Status {
            operation: started.operation,
        }),
    )
    .await?
    .expect("status remains readable");
    assert!(
        matches!(status, SnapshotReply::Status(status) if status.state == SnapshotState::Uploading)
    );
    drop(exclusive);
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn missing_selection_fails_closed_without_pruning_recoverable_data() -> Result {
    let (root, deployment, actor, prepared) = setup().await?;
    deployment
        .restore(deployment.epoch(), actor, prepared)
        .await?;
    deployment.close().await;
    drop(deployment);
    let live = root.path().join("live");
    let generations = std::fs::read_dir(live.join("storage-generations"))?.count();
    std::fs::remove_file(live.join("storage-selection.json"))?;
    assert!(matches!(
        Deployment::open(&live).await,
        Err(Error::Recovery(_))
    ));
    assert_eq!(
        std::fs::read_dir(live.join("storage-generations"))?.count(),
        generations
    );
    assert!(live.join("storage.sqlite3").exists());
    Ok(())
}

#[tokio::test]
async fn failed_initialization_is_retryable_and_selected_databases_skip_it() -> Result {
    let root = tempfile::tempdir()?;
    let failed = Deployment::open_initialized(root.path(), async |_storage| {
        Err(std::io::Error::other("synthetic initialization failure").into())
    })
    .await;
    assert!(matches!(failed, Err(Error::Initialization(_))));
    assert!(!root.path().join("storage-selection.json").exists());
    let deployment = Deployment::open(root.path()).await?;
    deployment.close().await;
    drop(deployment);
    let reopened = Deployment::open_initialized(root.path(), async |_storage| {
        panic!("a selected database must not be initialized again")
    })
    .await?;
    reopened.close().await;
    Ok(())
}
