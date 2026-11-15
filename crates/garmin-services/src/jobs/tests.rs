//! Exercise the pinned engine against the application's migrated database, without Garmin networking.
use super::*;
use garmin_model::{identity::UserId, value::Timestamp};
use garmin_storage::Storage;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn fixture_id() -> TaskId {
    TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV")
}

fn payload() -> Job {
    Job::ConnectSync {
        user: UserId::new_v4(),
        due: Timestamp::from_jiff(jiff::Timestamp::now()),
    }
}

async fn status(storage: &Storage) -> Result<String> {
    use apalis_core::backend::FetchById;
    let task = storage
        .jobs()
        .fetch_by_id(&fixture_id())
        .await?
        .expect("persisted job");
    Ok(task.status().to_string())
}

#[tokio::test]
async fn delayed_job_survives_reopen_is_deduplicated_and_acknowledged() -> Result {
    let root = tempfile::tempdir()?;
    let path = root.path().join("jobs.sqlite3");
    let storage = Storage::open(&path).await?;
    let due = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        + 2;
    let task = TaskBuilder::new(payload())
        .task_id(fixture_id())
        .run_at_timestamp(due)
        .idempotency_key("fixture")
        .build();
    storage.jobs().push_task(task).await?;
    storage
        .jobs()
        .push_task(
            TaskBuilder::new(payload())
                .idempotency_key("fixture")
                .build(),
        )
        .await?;
    storage.close().await;
    let storage = Storage::open(&path).await?;
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&seen);
    let worker = WorkerBuilder::new("reopened")
        .backend(storage.jobs().poll_with_interval(Duration::from_millis(50)))
        .build(move |_: Job, worker: WorkerContext| {
            let seen = Arc::clone(&observed);
            async move {
                assert!(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_secs()
                        >= due,
                    "delayed work must not run early"
                );
                seen.fetch_add(1, Ordering::SeqCst);
                worker.stop().expect("one job");
            }
        });
    tokio::time::timeout(Duration::from_secs(6), worker.run()).await??;
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(status(&storage).await?, "Done");
    storage.close().await;
    Ok(())
}

#[tokio::test]
async fn graceful_shutdown_waits_for_the_handler_and_flushes_its_acknowledgement() -> Result {
    use garmin_model::{
        garmin_connect::sync::State,
        identity::{Profile, Role, User},
    };
    let root = tempfile::tempdir()?;
    let storage = Arc::new(Storage::open(root.path().join("jobs.sqlite3")).await?);
    let user = User::from_parts(
        UserId::new_v4(),
        Role::Owner,
        Profile::from_display_name("Rider".parse()?),
    );
    storage.save_user(&user).await?;
    let user = user.id();
    storage
        .jobs()
        .push_task(TaskBuilder::new(payload()).task_id(fixture_id()).build())
        .await?;
    let started = Arc::new(tokio::sync::Notify::new());
    let stopping = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let handler_started = Arc::clone(&started);
    let handler_release = Arc::clone(&release);
    let writer = Arc::clone(&storage);
    let worker = WorkerBuilder::new("drain")
        .backend(storage.jobs().poll_with_interval(Duration::from_millis(50)))
        .build(move |_: Job| {
            let started = Arc::clone(&handler_started);
            let release = Arc::clone(&handler_release);
            let writer = Arc::clone(&writer);
            async move {
                started.notify_one();
                release.notified().await;
                writer
                    .save_connect_sync_state(user, &State::default())
                    .await
                    .expect("handler's durable state");
            }
        });
    let stop = Arc::clone(&stopping);
    let mut run = tokio::spawn(worker.run_with_ctx(move |worker| {
        let stop = Arc::clone(&stop);
        async move {
            stop.notified().await;
            drain(worker).await
        }
    }));
    tokio::time::timeout(Duration::from_secs(3), started.notified()).await?;
    stopping.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut run)
            .await
            .is_err()
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), run).await???;
    assert_eq!(status(&storage).await?, "Done");
    Arc::into_inner(storage)
        .expect("worker released storage")
        .close()
        .await;
    Ok(())
}

#[tokio::test]
async fn replacement_worker_recovers_unacknowledged_work() -> Result {
    let root = tempfile::tempdir()?;
    let storage = Storage::open(root.path().join("jobs.sqlite3")).await?;
    let config = apalis_sqlite::Config::default()
        .queue("garmin-core-v1")
        .heartbeat_interval(Duration::from_secs(1))
        .missed_heartbeats(1);
    let mut backend = storage.jobs().with_config(config.clone());
    backend
        .push_task(TaskBuilder::new(payload()).task_id(fixture_id()).build())
        .await?;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&started);
    let first = WorkerBuilder::new("abandoned")
        .backend(backend.poll_with_interval(Duration::from_millis(50)))
        .build(move |_: Job| {
            let signal = Arc::clone(&signal);
            async move {
                signal.notify_one();
                std::future::pending::<()>().await;
            }
        });
    let running = tokio::spawn(first.run());
    tokio::time::timeout(Duration::from_secs(3), started.notified()).await?;
    running.abort();
    assert!(running.await.expect_err("aborted").is_cancelled());
    let worker = WorkerBuilder::new("replacement")
        .backend(
            storage
                .jobs()
                .with_config(config)
                .poll_with_interval(Duration::from_millis(50)),
        )
        .build(|_: Job, worker: WorkerContext| async move {
            worker.stop().expect("recovered");
        });
    tokio::time::timeout(Duration::from_secs(6), worker.run()).await??;
    assert_eq!(status(&storage).await?, "Done");
    storage.close().await;
    Ok(())
}

#[tokio::test]
async fn stale_schedule_delivery_cannot_run_or_pause_the_replacement() -> Result {
    use garmin_model::garmin_connect::sync::{Failure, Interval, Outcome, State, Unit};
    let root = tempfile::tempdir()?;
    let app = Application::new(Storage::open(root.path().join("jobs.sqlite3")).await?);
    let user = app.create_profile("Rider".parse()?).await?.id();
    let old = Timestamp::from_unix_seconds(100).expect("timestamp");
    let due = Timestamp::from_unix_seconds(200).expect("timestamp");
    let mut state = State::default();
    state.configure(Interval::new(1, Unit::Hour), due);
    app.storage.save_connect_sync_state(user, &state).await?;
    assert!(matches!(
        app.dispatch_job(Job::ConnectSync { user, due }, &|| true)
            .await,
        Err(crate::Error::BackgroundJobDeferred)
    ));
    assert_eq!(
        app.storage.connect_sync_state(user).await?,
        state,
        "closed admission cannot alter application state"
    );
    app.dispatch_job(Job::ConnectSync { user, due: old }, &|| false)
        .await?;
    assert_eq!(app.storage.connect_sync_state(user).await?, state);
    app.dispatch_job(Job::ConnectSync { user, due }, &|| false)
        .await?;
    assert!(matches!(
        app.storage.connect_sync_state(user).await?.outcome,
        Outcome::Failed {
            kind: Failure::Credentials,
            ..
        }
    ));
    app.storage.close().await;
    Ok(())
}

#[test]
fn core_job_lifecycle_does_not_require_a_host_tokio_runtime() -> Result {
    futures_lite::future::block_on(async {
        let root = tempfile::tempdir()?;
        let deployment = Deployment::open(root.path()).await?;
        let jobs = deployment.start_jobs()?;
        let closed = futures_lite::future::race(
            async {
                deployment.close().await;
                true
            },
            async {
                futures_timer::Delay::new(Duration::from_secs(5)).await;
                false
            },
        )
        .await;
        assert!(
            closed,
            "core runtime must drain independently of its host's executor"
        );
        drop(jobs);
        Ok(())
    })
}

#[tokio::test]
async fn rapid_worker_restart_on_the_same_deployment_runs_due_work_without_a_lease_delay() -> Result
{
    use garmin_model::garmin_connect::sync::{Failure, Interval, Outcome, State, Unit};
    let root = tempfile::tempdir()?;
    let deployment = Deployment::open(root.path()).await?;
    let user = deployment
        .application(deployment.epoch())
        .await?
        .create_profile("Rider".parse()?)
        .await?
        .id();
    let first = deployment.start_jobs()?;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let app = deployment
                .application(deployment.epoch())
                .await
                .expect("lease");
            if !app
                .storage
                .jobs()
                .list_workers()
                .await
                .expect("workers")
                .is_empty()
            {
                break;
            }
            drop(app);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    drop(first);
    let mut state = State::default();
    state.configure(
        Interval::new(1, Unit::Hour),
        Timestamp::from_jiff(jiff::Timestamp::now()),
    );
    deployment
        .application(deployment.epoch())
        .await?
        .storage
        .save_connect_sync_state(user, &state)
        .await?;
    let second = deployment.start_jobs()?;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let app = deployment
                .application(deployment.epoch())
                .await
                .expect("lease");
            if matches!(
                app.storage
                    .connect_sync_state(user)
                    .await
                    .expect("state")
                    .outcome,
                Outcome::Failed {
                    kind: Failure::Credentials,
                    ..
                }
            ) {
                break;
            }
            drop(app);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert_eq!(
        deployment
            .application(deployment.epoch())
            .await?
            .storage
            .jobs()
            .list_workers()
            .await?
            .len(),
        2
    );
    deployment.close().await;
    drop(second);
    Ok(())
}
