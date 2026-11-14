use std::{io::Write as _, sync::Arc, time::Duration};

use garmin_model::garmin_connect::{ActivityFilters, ActivityRequest, PersonalApiKey};
use garmin_service_api::{
    ApplicationService as _, ApplicationServiceClient, ApplicationServiceServerShared,
};
use remoc::{ConnectExt as _, prelude::ServerShared as _};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    sync::Notify,
};

use super::{Host, SourceProvider as _};

#[derive(Clone, Copy)]
enum Phase {
    Indexing,
    Downloading,
}

#[derive(Clone, Copy)]
enum Disconnect {
    Caller,
    Transport,
}

async fn synthetic_garmin(
    phase: Phase,
    started: Arc<Notify>,
    finish: Arc<Notify>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("address");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("activity.fit", zip::write::SimpleFileOptions::default())
        .expect("archive entry");
    writer
        .write_all(
            &garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Cycling)
                .expect("FIT fixture"),
        )
        .expect("FIT bytes");
    let archive = writer.finish().expect("archive").into_inner();
    let summaries = (1..=20)
        .map(|id| serde_json::json!({"activityId":id,"activityName":format!("Ride {id}")}))
        .collect::<Vec<_>>();
    let mut responses = vec![
        b"token".to_vec(),
        br#"{"displayName":"synthetic-account"}"#.to_vec(),
        serde_json::to_vec(&summaries).expect("summaries"),
    ];
    match phase {
        Phase::Indexing => responses.push(b"[]".to_vec()),
        Phase::Downloading => {
            responses.push(b"[]".to_vec());
            responses.push(archive);
        }
    }
    let server = tokio::spawn(async move {
        let last = responses.len() - 1;
        for (index, body) in responses.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.expect("Garmin request");
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.expect("request headers");
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
            }
            if index == last {
                started.notify_one();
                finish.notified().await;
            }
            let length = body.len();
            let headers = indoc::formatdoc! {"
                HTTP/1.1 200 OK
                Content-Length: {length}
                Connection: close

                "}
            .replace('\n', "\r\n");
            socket
                .write_all(headers.as_bytes())
                .await
                .expect("response headers");
            socket.write_all(&body).await.expect("response body");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "cancelled refresh started another request"
        );
    });
    (address, server)
}

async fn rpc_client(
    host: Arc<Host>,
) -> (
    ApplicationServiceClient,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
) {
    let (service, local_client) =
        ApplicationServiceServerShared::<_, remoc::codec::Default>::new(host);
    let serving = tokio::spawn(async move {
        let _ = service.serve().await;
    });
    let (left, right) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(left);
    let providing = tokio::spawn(async move {
        let (mut connection, mut sender, _) =
            remoc::Connect::io::<_, _, ApplicationServiceClient, (), remoc::codec::Default>(
                garmin_service_api::control::transport_config(),
                read,
                write,
            )
            .await
            .expect("server transport");
        tokio::select! {
            result = &mut connection => { result.expect("server transport closed before client delivery"); return; }
            result = sender.send(local_client) => { result.expect("deliver RPC client"); }
        }
        let _ = connection.await;
    });
    let (read, write) = tokio::io::split(right);
    let client = remoc::Connect::io(garmin_service_api::control::transport_config(), read, write)
        .consume()
        .await
        .expect("RPC client");
    (client, providing, serving)
}

async fn assert_cache(root: &std::path::Path, phase: Phase) {
    let database = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(root.join("storage.sqlite3"))
            .read_only(true),
    )
    .await
    .expect("database observer");
    let summaries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM connect_cache_summaries")
        .fetch_one(&database)
        .await
        .expect("summary count");
    let originals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM connect_cache_originals WHERE archive IS NOT NULL",
    )
    .fetch_one(&database)
    .await
    .expect("original count");
    match phase {
        Phase::Indexing => assert_eq!((summaries, originals), (0, 0)),
        Phase::Downloading => assert_eq!((summaries, originals), (20, 1)),
    }
    database.close().await;
}

async fn disconnect_refresh(phase: Phase, disconnect: Disconnect) {
    let root = tempfile::tempdir().expect("test directory");
    let deployment = garmin_services::deployment::Deployment::open(root.path())
        .await
        .expect("deployment");
    let source = super::demo::Provider
        .open(root.path(), tokio::runtime::Handle::current())
        .expect("demo source");
    let host = Host::new(source, deployment.clone());
    let user = host
        .create_profile("Test Rider".into())
        .await
        .expect("local call")
        .expect("profile");
    let storage = garmin_storage::Storage::open(root.path().join("storage.sqlite3"))
        .await
        .expect("observer storage");
    storage
        .set_garmin_connect_key(
            user.id(),
            Some(&PersonalApiKey::from_string("synthetic-key".into()).expect("key")),
        )
        .await
        .expect("save key");
    let started = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let (address, server) = synthetic_garmin(phase, started.clone(), finish.clone()).await;
    host.application()
        .await
        .expect("application")
        .set_connect_test_origin(address)
        .expect("loopback endpoint");
    let (client, providing, serving) = rpc_client(host.clone()).await;
    let (cancel_sender, cancellation) = remoc::rch::watch::channel(false);
    let caller = tokio::spawn(async move {
        client
            .garmin_connect_activity(
                user.id(),
                ActivityRequest::Load {
                    start: 0,
                    filters: ActivityFilters::default(),
                },
                None,
                Some(cancellation),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("refresh reached checkpoint");
    let disconnected = host
        .connect_refresh_cancel
        .lock()
        .expect("observer")
        .clone()
        .expect("refresh token");
    match disconnect {
        Disconnect::Caller => {
            caller.abort();
            assert!(caller.await.expect_err("caller aborted").is_cancelled());
        }
        Disconnect::Transport => {
            providing.abort();
            let result = tokio::time::timeout(Duration::from_secs(5), caller)
                .await
                .expect("transport failure reaches caller")
                .expect("caller task");
            assert!(result.is_err(), "interrupted RPC must fail");
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while !disconnected.is_cancelled() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("RPC cancellation reached job");
    assert!(
        !*cancel_sender.borrow(),
        "watch remains false to isolate RPC cancellation"
    );
    finish.notify_one();
    tokio::time::timeout(Duration::from_secs(5), deployment.close())
        .await
        .expect("job released deployment lease");
    server.await.expect("synthetic Garmin server");
    assert_cache(root.path(), phase).await;
    drop(cancel_sender);
    storage.close().await;
    providing.abort();
    serving.abort();
}

#[tokio::test]
async fn rpc_disconnect_discards_staged_summary_generation() {
    disconnect_refresh(Phase::Indexing, Disconnect::Caller).await;
}

#[tokio::test]
async fn rpc_disconnect_finishes_persisting_in_flight_original() {
    disconnect_refresh(Phase::Downloading, Disconnect::Caller).await;
}

#[tokio::test]
async fn transport_disconnect_discards_staged_summary_generation() {
    disconnect_refresh(Phase::Indexing, Disconnect::Transport).await;
}

#[tokio::test]
async fn transport_disconnect_finishes_persisting_in_flight_original() {
    disconnect_refresh(Phase::Downloading, Disconnect::Transport).await;
}
