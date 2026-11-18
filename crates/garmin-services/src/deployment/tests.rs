use garmin_model::artifact::ByteCount;
use std::{error::Error as StdError, fs::File, path::Path, process::Command, sync::Arc};

use garmin_storage::{
    Storage,
    snapshot::{Limits, PreparedRestore},
};
use tempfile::TempDir;

use super::{Checkpoint, Deployment, Error};
use crate::{Application, UserContext};

type Result<T = ()> = std::result::Result<T, Box<dyn StdError>>;

#[tokio::test]
async fn backup_drains_the_job_runner_and_resumes_it_without_changing_epoch() -> Result {
    use apalis_core::backend::ListWorkers as _;

    let (root, deployment, actor, _) = setup().await?;
    let epoch = deployment.epoch();
    let runner = deployment.start_jobs()?;
    let wait_for_workers = async |count| {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if deployment
                    .application(epoch)
                    .await?
                    .storage
                    .jobs()
                    .list_workers()
                    .await?
                    .len()
                    >= count
                {
                    return Ok::<_, Box<dyn StdError>>(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await?
    };
    wait_for_workers(1).await?;
    let archive = root.path().join("runner-backup.tar.zst");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        deployment.backup(
            epoch,
            actor,
            &archive,
            Limits::default(),
            &garmin_progress::CancellationToken::default(),
        ),
    )
    .await??;
    assert_eq!(deployment.epoch(), epoch);
    let _prepared =
        PreparedRestore::read(File::open(&archive)?, root.path(), Limits::default()).await?;
    wait_for_workers(2).await?;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            deployment.backup(
                epoch,
                actor,
                &archive,
                Limits::default(),
                &garmin_progress::CancellationToken::default(),
            ),
        )
        .await?
        .is_err(),
        "an existing backup cannot be overwritten"
    );
    wait_for_workers(3).await?;
    deployment.close().await;
    drop(runner);
    Ok(())
}

#[tokio::test]
async fn pause_cancels_a_generation_admitted_before_it_started() -> Result {
    let (_root, deployment, _, _) = setup().await?;
    let (captured, ready) = tokio::sync::oneshot::channel();
    let (release, proceed) = std::sync::mpsc::channel();
    let gate = std::sync::Mutex::new(Some((captured, proceed)));
    *deployment.hook.lock().expect("hook") = Some(Box::new(move |checkpoint| {
        if checkpoint == Checkpoint::JobsAdmissionChecked
            && let Some((captured, proceed)) = gate.lock().expect("gate").take()
        {
            captured.send(()).expect("test is waiting");
            proceed
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("test releases admission");
        }
        Ok(())
    }));
    let owner = Arc::clone(&deployment);
    let admission = std::thread::spawn(move || crate::jobs::generation_token(&owner));
    tokio::time::timeout(std::time::Duration::from_secs(5), ready).await??;
    let pause = deployment.pause_jobs();
    release.send(())?;
    let token = admission
        .join()
        .expect("admission thread")
        .expect("generation was admitted before the pause");
    assert!(token.is_cancelled(), "the admitted generation must drain");
    drop(pause);
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn rejected_backups_do_not_interrupt_scheduled_jobs() -> Result {
    let (root, deployment, actor, _) = setup().await?;
    let epoch = deployment.epoch();
    let member = deployment
        .application(epoch)
        .await?
        .create_profile("Member".parse()?)
        .await?;
    let stop = deployment.jobs_stop.lock().expect("stop token").clone();
    let archive = root.path().join("rejected.tar.zst");
    let cancelled = garmin_progress::CancellationToken::default();
    assert!(
        deployment
            .backup(
                epoch,
                UserContext::new(member.id()),
                &archive,
                Limits::default(),
                &cancelled
            )
            .await
            .is_err()
    );
    cancelled.cancel();
    assert!(
        deployment
            .backup(epoch, actor, &archive, Limits::default(), &cancelled)
            .await
            .is_err()
    );
    assert!(!stop.is_cancelled());
    assert!(!archive.exists());
    deployment.close().await;
    Ok(())
}

#[tokio::test]
async fn restore_cancels_old_scheduler_leases_and_rebinds_the_runner() -> Result {
    use garmin_model::garmin_connect::sync::{Interval, Outcome, Unit};
    let (_root, deployment, actor, prepared) =
        setup_with_schedule(Interval::new(1, Unit::Hour)).await?;
    let epoch = deployment.epoch();
    let runner = deployment.start_jobs()?;
    let stop = deployment.jobs_stop.lock().expect("stop token").clone();
    let lease = deployment.application_owned(epoch).await?;
    let old_job = tokio::spawn(async move {
        let _lease = lease;
        while !stop.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        deployment.restore(epoch, actor, prepared),
    )
    .await??;
    old_job.await?;
    let user = deployment
        .application(deployment.epoch())
        .await?
        .profiles()
        .await?[0]
        .id();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let state = deployment
                .application(deployment.epoch())
                .await?
                .connect_sync_status(UserContext::new(user))
                .await?;
            if matches!(
                state.state.outcome,
                Outcome::Failed {
                    kind: garmin_model::garmin_connect::sync::Failure::Credentials,
                    ..
                }
            ) {
                return Ok::<_, Box<dyn StdError>>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(name(&deployment).await?, "Restored");
    deployment.close().await;
    drop(runner);
    Ok(())
}

#[tokio::test]
async fn unauthorized_restore_does_not_interrupt_scheduled_jobs() -> Result {
    let (_root, deployment, _, prepared) = setup().await?;
    let epoch = deployment.epoch();
    let member = deployment
        .application(epoch)
        .await?
        .create_profile("Member".parse()?)
        .await?;
    let stop = deployment.jobs_stop.lock().expect("stop token").clone();
    assert!(
        deployment
            .restore(epoch, UserContext::new(member.id()), prepared)
            .await
            .is_err()
    );
    assert!(!stop.is_cancelled());
    deployment.close().await;
    Ok(())
}

async fn setup() -> Result<(TempDir, Arc<Deployment>, UserContext, PreparedRestore)> {
    setup_with_schedule(None).await
}

async fn setup_with_schedule(
    schedule: Option<garmin_model::garmin_connect::sync::Interval>,
) -> Result<(TempDir, Arc<Deployment>, UserContext, PreparedRestore)> {
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
    if let Some(interval) = schedule {
        let mut state = garmin_model::garmin_connect::sync::State::default();
        state.configure(
            Some(interval),
            garmin_model::value::Timestamp::from_jiff(jiff::Timestamp::now()),
        );
        replacement
            .storage
            .save_connect_sync_state(new_owner.id(), &state)
            .await?;
    }
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

fn copy_tree(from: &Path, to: &Path) -> Result {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
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
async fn stopped_deployment_tree_restores_selected_generation() -> Result {
    let (root, deployment, actor, prepared) = setup().await?;
    let live = root.path().join("live");
    let epoch = deployment.epoch();
    deployment.restore(epoch, actor, prepared).await?;
    assert_eq!(name(&deployment).await?, "Restored");
    deployment.close().await;
    drop(deployment);

    let backup = root.path().join("supervisor-backup-copy");
    copy_tree(&live, &backup)?;
    std::fs::remove_dir_all(&live)?;
    copy_tree(&backup, &live)?;

    let reopened = Deployment::open(&live).await?;
    assert_eq!(name(&reopened).await?, "Restored");
    assert!(live.join("storage.sqlite3").exists());
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
            bytes: ByteCount::from_bytes(10),
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
