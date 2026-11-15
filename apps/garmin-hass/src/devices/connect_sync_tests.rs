use super::{
    Host, SourceProvider as _,
    connect_refresh_tests::{Phase, synthetic_garmin},
};
use garmin_model::{
    garmin_connect::{
        ActivityFilters, PersonalApiKey,
        sync::{Interval, Outcome, Unit},
    },
    identity::User,
};
use garmin_service_api::ApplicationService as _;
use garmin_services::{UserContext, deployment::Deployment};
use std::{sync::Arc, time::Duration};

async fn setup(root: &std::path::Path) -> (Arc<Host>, Arc<Deployment>, User) {
    let deployment = Deployment::open(root).await.expect("deployment");
    let source = super::demo::Provider
        .open(root, tokio::runtime::Handle::current())
        .expect("demo source");
    let host = Host::new(source, deployment.clone());
    let user = host
        .create_profile("Test Rider".into())
        .await
        .expect("local call")
        .expect("profile");
    let storage = garmin_storage::Storage::open(root.join("storage.sqlite3"))
        .await
        .expect("storage");
    storage
        .set_garmin_connect_key(
            user.id(),
            Some(&PersonalApiKey::from_string("synthetic-key".into()).expect("key")),
        )
        .await
        .expect("key saved");
    storage.close().await;
    (host, deployment, user)
}

async fn enable_sync(
    deployment: &Deployment,
    user: UserContext,
    address: std::net::SocketAddr,
    interval: Option<Interval>,
) {
    let application = deployment
        .application(deployment.epoch())
        .await
        .expect("application");
    application
        .set_connect_test_origin(address)
        .expect("loopback");
    application
        .configure_connect_sync(user, interval)
        .await
        .expect("configure");
}

async fn assert_disabled_cache(application: &garmin_services::Application, user: UserContext) {
    assert!(
        application
            .connect_sync_status(user)
            .await
            .expect("status")
            .progress
            .is_none()
    );
    assert_eq!(
        application
            .activities(user)
            .await
            .expect("local activities")
            .len(),
        0
    );
    assert!(matches!(
        application
            .read_connect_cache(user, 0, ActivityFilters::default())
            .await
            .expect("cache"),
        Some(garmin_model::garmin_connect::ActivityResponse::Page {
            cache: garmin_model::garmin_connect::CacheStatus { downloaded: 1, .. },
            ..
        })
    ));
}

#[tokio::test]
async fn background_sync_runs_without_a_browser_keeps_reads_available_and_disable_drains_it() {
    let root = tempfile::tempdir().expect("directory");
    let (host, deployment, user) = setup(root.path()).await;
    let user = UserContext::new(user.id());
    let started = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let (address, server) =
        synthetic_garmin(Phase::Downloading, started.clone(), finish.clone()).await;
    enable_sync(&deployment, user, address, Interval::new(3, Unit::Hour)).await;
    let runner = deployment.start_jobs().expect("scheduler");
    assert!(
        deployment.start_jobs().is_err(),
        "only one scheduler may own a deployment"
    );
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("scheduled download started");
    {
        let application = deployment
            .application(deployment.epoch())
            .await
            .expect("application");
        let snapshot = application.connect_sync_status(user).await.expect("status");
        assert!(matches!(snapshot.state.outcome, Outcome::Running));
        assert!(snapshot.progress.is_some());
        let page = tokio::time::timeout(
            Duration::from_secs(1),
            application.read_connect_cache(user, 0, ActivityFilters::default()),
        )
        .await
        .expect("read is independent of refresh lock")
        .expect("cache query");
        assert!(page.is_some());
        let listed = tokio::time::timeout(
            Duration::from_secs(1),
            application.garmin_connect_activity(
                user,
                garmin_model::garmin_connect::ActivityRequest::List {
                    start: 0,
                    filters: ActivityFilters::default(),
                },
            ),
        )
        .await
        .expect("paging does not wait for a refresh")
        .expect("cached page");
        assert!(matches!(
            listed,
            garmin_service_api::ConnectActivityResult::State(
                garmin_model::garmin_connect::ActivityResponse::Page { total: 20, .. }
            )
        ));
        assert_eq!(
            application
                .activities(user)
                .await
                .expect("local activities")
                .len(),
            0,
            "downloads must never import"
        );
        let disable = application.configure_connect_sync(user, None);
        tokio::pin!(disable);
        assert!(
            futures_util::poll!(&mut disable).is_pending(),
            "disable waits for the in-flight file"
        );
        finish.notify_one();
        let disabled = tokio::time::timeout(Duration::from_secs(5), &mut disable)
            .await
            .expect("disable drained job")
            .expect("disabled");
        assert_eq!(disabled.state.interval, None);
        assert_eq!(disabled.state.next_attempt, None);
        assert_disabled_cache(&application, user).await;
    }
    server.await.expect("server");
    deployment.close().await;
    drop(runner);
    let storage = garmin_storage::Storage::open(root.path().join("storage.sqlite3"))
        .await
        .expect("reopen");
    assert_eq!(
        storage
            .connect_sync_state(user.user_id())
            .await
            .expect("persisted schedule")
            .interval,
        None
    );
    let snapshot = host.profiles().await;
    assert!(snapshot.is_err(), "closed deployment no longer serves jobs");
    storage.close().await;
}

async fn rate_limited_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("token request");
        let mut request = Vec::new();
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).await.expect("headers");
            assert!(count > 0);
            request.extend_from_slice(&bytes[..count]);
        }
        let response = indoc::indoc! {"
            HTTP/1.1 429 Too Many Requests
            Retry-After: 900
            Content-Length: 0
            Connection: close

            "}
        .replace('\n', "\r\n");
        socket
            .write_all(response.as_bytes())
            .await
            .expect("rate limit response");
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err(),
            "rate limits must not retry immediately"
        );
    });
    (address, server)
}

#[tokio::test]
async fn rate_limit_is_not_success_and_its_retry_after_survives_restart() {
    let root = tempfile::tempdir().expect("directory");
    let (_, deployment, user) = setup(root.path()).await;
    let (address, server) = rate_limited_server().await;
    let user = UserContext::new(user.id());
    {
        let application = deployment
            .application(deployment.epoch())
            .await
            .expect("application");
        application
            .set_connect_test_origin(address)
            .expect("loopback");
        application
            .configure_connect_sync(user, Interval::new(1, Unit::Day))
            .await
            .expect("configure");
    }
    let runner = deployment.start_jobs().expect("scheduler");
    let state = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = deployment
                .application(deployment.epoch())
                .await
                .expect("application")
                .connect_sync_status(user)
                .await
                .expect("state");
            if matches!(snapshot.state.outcome, Outcome::Failed { .. }) {
                break snapshot.state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failure recorded");
    assert!(matches!(
        state.outcome,
        Outcome::Failed {
            kind: garmin_model::garmin_connect::sync::Failure::RateLimit,
            ..
        }
    ));
    assert_eq!(state.last_success, None);
    assert!(
        state.next_attempt.expect("retry time").as_unix_seconds()
            - state.last_attempt.expect("attempt").as_unix_seconds()
            >= 900
    );
    server.await.expect("server");
    deployment.close().await;
    drop(runner);
    let storage = garmin_storage::Storage::open(root.path().join("storage.sqlite3"))
        .await
        .expect("reopen");
    assert_eq!(
        storage
            .connect_sync_state(user.user_id())
            .await
            .expect("persisted state"),
        state
    );
    storage.close().await;
}

#[tokio::test]
async fn shutdown_cancels_background_indexing_and_releases_its_deployment_lease() {
    let root = tempfile::tempdir().expect("directory");
    let (_, deployment, user) = setup(root.path()).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let (address, server) =
        synthetic_garmin(Phase::Indexing, started.clone(), finish.clone()).await;
    {
        let app = deployment
            .application(deployment.epoch())
            .await
            .expect("application");
        app.set_connect_test_origin(address).expect("loopback");
        app.configure_connect_sync(UserContext::new(user.id()), Interval::new(1, Unit::Week))
            .await
            .expect("configure");
    }
    let runner = deployment.start_jobs().expect("scheduler");
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("index page in flight");
    let close = deployment.close();
    tokio::pin!(close);
    assert!(
        futures_util::poll!(&mut close).is_pending(),
        "shutdown drains the owned refresh"
    );
    finish.notify_one();
    tokio::time::timeout(Duration::from_secs(5), &mut close)
        .await
        .expect("shutdown completed");
    server.await.expect("server");
    drop(runner);
    let database = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.path().join("storage.sqlite3"))
            .read_only(true),
    )
    .await
    .expect("database observer");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM connect_cache_summaries")
        .fetch_one(&database)
        .await
        .expect("staging count");
    assert_eq!(count, 0);
    database.close().await;
    let storage = garmin_storage::Storage::open(root.path().join("storage.sqlite3"))
        .await
        .expect("reopen");
    let state = storage
        .connect_sync_state(user.id())
        .await
        .expect("interrupted state");
    assert!(matches!(state.outcome, Outcome::Interrupted));
    assert_eq!(
        state.next_attempt, state.last_attempt,
        "shutdown remains due for one restart catch-up"
    );
    storage.close().await;
}

#[tokio::test]
async fn completed_background_sync_downloads_without_importing_and_does_not_repeat_on_restart() {
    let root = tempfile::tempdir().expect("directory");
    let (_, deployment, user) = setup(root.path()).await;
    let user = UserContext::new(user.id());
    let started = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let (address, server) =
        synthetic_garmin(Phase::Complete, started.clone(), finish.clone()).await;
    {
        let app = deployment
            .application(deployment.epoch())
            .await
            .expect("application");
        app.set_connect_test_origin(address).expect("loopback");
        app.configure_connect_sync(user, Interval::new(7, Unit::Hour))
            .await
            .expect("configure");
    }
    let runner = deployment.start_jobs().expect("scheduler");
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("download began without a browser");
    finish.notify_one();
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = deployment
                .application(deployment.epoch())
                .await
                .expect("application")
                .connect_sync_status(user)
                .await
                .expect("state");
            if matches!(snapshot.state.outcome, Outcome::Complete { .. }) {
                break snapshot.state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background refresh completed");
    assert!(matches!(
        completed.outcome,
        Outcome::Complete { unavailable: 0 }
    ));
    assert_eq!(
        completed.next_attempt.expect("next").as_unix_seconds()
            - completed.last_success.expect("success").as_unix_seconds(),
        7 * 3600
    );
    {
        let app = deployment
            .application(deployment.epoch())
            .await
            .expect("application");
        assert_eq!(
            app.activities(user).await.expect("local activities").len(),
            0
        );
        assert!(matches!(
            app.read_connect_cache(user, 0, ActivityFilters::default())
                .await
                .expect("cache"),
            Some(garmin_model::garmin_connect::ActivityResponse::Page {
                cache: garmin_model::garmin_connect::CacheStatus {
                    total: 20,
                    downloaded: 20,
                    ..
                },
                ..
            })
        ));
    }
    server.await.expect("server");
    deployment.close().await;
    drop(runner);
    drop(deployment);
    let deployment = Deployment::open(root.path()).await.expect("restart");
    deployment
        .application(deployment.epoch())
        .await
        .expect("application")
        .set_connect_test_origin(address)
        .expect("loopback remains selected even if a regression retries");
    let runner = deployment.start_jobs().expect("restarted scheduler");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        deployment
            .application(deployment.epoch())
            .await
            .expect("application")
            .connect_sync_status(user)
            .await
            .expect("state")
            .state,
        completed
    );
    deployment.close().await;
    drop(runner);
}

#[tokio::test]
async fn manual_refresh_warning_preserves_the_typed_rate_limit_and_retry_deadline() {
    use garmin_model::{
        garmin_connect::{AccountFacts, ActivityRequest, sync::Failure},
        identity::SourceId,
    };
    let root = tempfile::tempdir().expect("directory");
    let (_, deployment, user) = setup(root.path()).await;
    let storage = garmin_storage::Storage::open(root.path().join("storage.sqlite3"))
        .await
        .expect("storage");
    let key = storage
        .garmin_connect_key(user.id())
        .await
        .expect("key lookup")
        .expect("configured");
    let fingerprint = blake3::hash(key.expose().as_bytes()).to_hex().to_string();
    storage
        .publish_connect_cache(
            user.id(),
            &fingerprint,
            &garmin_storage::CachedAccount {
                source: SourceId::from_u128(77),
                generation: "previous".into(),
                facts: AccountFacts {
                    display_name: Some("synthetic-account".into()),
                    locale: None,
                    measurement_system: None,
                    time_zone: None,
                },
            },
        )
        .await
        .expect("published snapshot");
    let (address, server) = rate_limited_server().await;
    let user = UserContext::new(user.id());
    enable_sync(&deployment, user, address, Interval::new(1, Unit::Day)).await;
    {
        let app = deployment
            .application(deployment.epoch())
            .await
            .expect("application");
        let response = app
            .garmin_connect_activity(
                user,
                ActivityRequest::Load {
                    start: 0,
                    filters: ActivityFilters::default(),
                },
            )
            .await
            .expect("cached page remains readable");
        assert!(matches!(
            response,
            garmin_service_api::ConnectActivityResult::State(
                garmin_model::garmin_connect::ActivityResponse::Page {
                    warning: Some(_),
                    ..
                }
            )
        ));
        let state = app.connect_sync_status(user).await.expect("state").state;
        assert!(matches!(
            state.outcome,
            Outcome::Failed {
                kind: Failure::RateLimit,
                ..
            }
        ));
        assert_eq!(state.last_success, None);
        assert!(
            state.next_attempt.expect("next").as_unix_seconds()
                - state.last_attempt.expect("attempt").as_unix_seconds()
                >= 900
        );
    }
    server.await.expect("server");
    deployment.close().await;
    storage.close().await;
}
