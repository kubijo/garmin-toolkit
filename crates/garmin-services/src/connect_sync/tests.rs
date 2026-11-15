use super::*;
use garmin_model::garmin_connect::{
    PersonalApiKey,
    sync::{JobId, State, Unit},
};

#[test]
fn backwards_clock_change_defers_delivery_instead_of_consuming_the_schedule() {
    let at = |seconds| Timestamp::from_unix_seconds(seconds).expect("timestamp");
    let mut state = State::default();
    state.configure(Interval::new(1, Unit::Hour), at(100));
    assert!(eligible(&state, Some(at(100)), at(110)).expect("due"));
    assert!(matches!(
        eligible(&state, Some(at(100)), at(90)),
        Err(Error::BackgroundJobDeferred)
    ));
    state.configure(Interval::new(2, Unit::Hour), at(200));
    assert!(!eligible(&state, Some(at(100)), at(300)).expect("replaced"));
}

#[cfg(feature = "integration-hooks")]
#[tokio::test]
async fn dropping_a_registered_refresh_clears_progress_before_it_is_disabled() {
    let root = tempfile::tempdir().expect("directory");
    let app = Arc::new(Application::new(
        garmin_storage::Storage::open(root.path().join("sync.sqlite3"))
            .await
            .expect("storage"),
    ));
    let user = UserContext::new(
        app.create_profile("Rider".parse().expect("name"))
            .await
            .expect("profile")
            .id(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    app.set_connect_test_origin(listener.local_addr().expect("address"))
        .expect("origin");
    app.set_garmin_connect_key(
        user,
        Some(PersonalApiKey::from_string("synthetic-key".into()).expect("key")),
    )
    .await
    .expect("key");
    app.configure_connect_sync(user, Interval::new(1, Unit::Day))
        .await
        .expect("enable");
    let running = Arc::clone(&app);
    let task = tokio::spawn(async move { running.run_connect_sync(user, None, &|| false).await });
    // Withhold the token response after a real request has registered and entered transport.
    let (_socket, _) = tokio::time::timeout(std::time::Duration::from_secs(3), listener.accept())
        .await
        .expect("connect deadline")
        .expect("connection");
    assert!(
        app.connect_sync_status(user)
            .await
            .expect("status")
            .progress
            .is_some()
    );
    task.abort();
    assert!(task.await.expect_err("aborted").is_cancelled());
    let after = app.connect_sync_status(user).await.expect("status");
    assert!(after.job.is_none() && after.progress.is_none());
    let disabled = app
        .configure_connect_sync(user, None)
        .await
        .expect("disable");
    assert!(disabled.progress.is_none());
    assert!(
        app.connect_sync_status(user)
            .await
            .expect("status")
            .progress
            .is_none()
    );
}

#[tokio::test]
async fn manual_priority_cancels_a_job_registered_after_the_wait_begins() {
    let root = tempfile::tempdir().expect("directory");
    let app = Application::new(
        garmin_storage::Storage::open(root.path().join("sync.sqlite3"))
            .await
            .expect("storage"),
    );
    let user = UserContext::new(garmin_model::identity::UserId::new_v4());
    let lock = app.connect_lock(user);
    let guard = lock.lock().await;
    let waiting = app.interrupt_connect_sync(user);
    tokio::pin!(waiting);
    assert!(
        futures_lite::future::poll_once(&mut waiting)
            .await
            .is_none()
    );
    let cancel = CancellationToken::default();
    app.connect_sync_runs.lock().expect("runs").insert(
        user.user_id(),
        Run {
            id: JobId::new_v4(),
            cancel: cancel.clone(),
            progress: LoadProgress::Waiting,
        },
    );
    tokio::select! {
        _guard = &mut waiting => panic!("background lock is still held"),
        () = async {
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while !cancel.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }).await.expect("late registration must still be cancelled");
        } => {},
    }
    drop(guard);
    let _manual_guard = waiting.await;
}

#[tokio::test]
async fn stale_and_foreign_cancel_requests_cannot_cancel_a_newer_job() {
    let root = tempfile::tempdir().expect("directory");
    let app = Application::new(
        garmin_storage::Storage::open(root.path().join("sync.sqlite3"))
            .await
            .expect("storage"),
    );
    let user = UserContext::new(
        app.create_profile("Rider".parse().expect("name"))
            .await
            .expect("profile")
            .id(),
    );
    let other = UserContext::new(
        app.create_profile("Other rider".parse().expect("name"))
            .await
            .expect("profile")
            .id(),
    );
    app.set_garmin_connect_key(
        user,
        Some(PersonalApiKey::from_string("synthetic-key".into()).expect("key")),
    )
    .await
    .expect("save key");
    let current = JobId::new_v4();
    let mut state = State::default();
    state.configure(Interval::new(1, Unit::Day), now());
    state.last_job = Some(current);
    state.outcome = Outcome::Running;
    app.storage
        .save_connect_sync_state(user.user_id(), &state)
        .await
        .expect("state");
    let cancel = CancellationToken::default();
    app.connect_sync_runs.lock().expect("runs").insert(
        user.user_id(),
        Run {
            id: current,
            cancel: cancel.clone(),
            progress: LoadProgress::Connecting,
        },
    );
    app.cancel_scheduled_connect_sync(user, JobId::new_v4())
        .await
        .expect("stale cancel");
    app.cancel_scheduled_connect_sync(other, current)
        .await
        .expect("foreign cancel");
    assert!(!cancel.is_cancelled());
    assert_eq!(
        app.storage
            .connect_sync_state(user.user_id())
            .await
            .expect("state"),
        state
    );
    let snapshot = app
        .cancel_scheduled_connect_sync(user, current)
        .await
        .expect("matching cancel");
    assert!(cancel.is_cancelled());
    assert!(matches!(snapshot.state.outcome, Outcome::Cancelled));
    assert!(snapshot.state.next_attempt.is_some());
    app.finish_connect_sync(
        user,
        Outcome::Complete { unavailable: 0 },
        None,
        Some(current),
    )
    .await
    .expect("late completion");
    assert_eq!(
        app.connect_sync_status(user).await.expect("snapshot").state,
        snapshot.state,
        "late completion cannot erase the user's cancellation deferral"
    );
    app.connect_sync_runs
        .lock()
        .expect("runs")
        .remove(&user.user_id());
    let configured = app
        .configure_connect_sync(user, Interval::new(2, Unit::Hour))
        .await
        .expect("new interval");
    let after_stale_cancel = app
        .cancel_scheduled_connect_sync(user, current)
        .await
        .expect("old job");
    assert_eq!(configured.state, after_stale_cancel.state);
}

#[tokio::test]
async fn restored_schedule_without_credentials_pauses_and_repair_reenables_it() {
    let root = tempfile::tempdir().expect("directory");
    let app = Application::new(
        garmin_storage::Storage::open(root.path().join("sync.sqlite3"))
            .await
            .expect("storage"),
    );
    let user = UserContext::new(
        app.create_profile("Rider".parse().expect("name"))
            .await
            .expect("profile")
            .id(),
    );
    let mut state = State::default();
    state.configure(Interval::new(2, Unit::Week), now());
    app.storage
        .save_connect_sync_state(user.user_id(), &state)
        .await
        .expect("restored configuration");
    app.run_connect_sync(user, None, &|| false)
        .await
        .expect("missing credentials is a classified outcome");
    let snapshot = app.connect_sync_status(user).await.expect("snapshot");
    assert!(matches!(
        snapshot.state.outcome,
        Outcome::Failed {
            kind: Failure::Credentials,
            ..
        }
    ));
    assert_eq!(snapshot.state.next_attempt, None);
    app.set_garmin_connect_key(
        user,
        Some(PersonalApiKey::from_string("synthetic-key".into()).expect("key")),
    )
    .await
    .expect("repair");
    let snapshot = app.connect_sync_status(user).await.expect("snapshot");
    assert_eq!(snapshot.state.interval, Interval::new(2, Unit::Week));
    assert!(snapshot.state.due(now()));
    assert_eq!(snapshot.state.last_job, None);
}
