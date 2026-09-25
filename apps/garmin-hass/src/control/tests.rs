use super::*;
use garmin_service_api::control::{BrowserControl, BrowserControlServerShared};
use remoc::{codec, rtc, rtc::ServerShared as _};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[tokio::test]
async fn diagnostic_reads_work_without_a_browser_and_include_all_connections() -> anyhow::Result<()>
{
    let broker = Broker::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = broker.routes();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    let client = reqwest::Client::new();
    let endpoint = format!("http://{address}/api/events-get?format=json");
    let before: Value = client
        .get(&endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(before["state"].as_array().unwrap().len(), 1);
    assert_eq!(before["state"][0]["source"], "hass");
    let (first, _, _, first_task) = attach(&broker, "first");
    let (second, _, _, second_task) = attach(&broker, "second");
    let both: Value = client
        .get(&endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(both["state"].as_array().unwrap().len(), 3);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot: Value = client.get(&endpoint).send().await?.json().await?;
            if snapshot["state"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["fields"]["diagnostics"] == "unavailable")
                .count()
                == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await??;
    drop(first);
    let command = control_request(State(broker.clone()), headers(), Json(request(1))).await;
    assert_eq!(command.status(), StatusCode::OK);
    let remaining: Value = client
        .get(&endpoint)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(remaining["state"].as_array().unwrap().len(), 2);
    assert!(
        remaining["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["removed"] == true)
    );
    drop(second);
    first_task.abort();
    second_task.abort();
    server.abort();
    Ok(())
}

#[tokio::test]
async fn maximum_screenshot_crosses_the_real_transport_and_leaves_rpc_usable() -> anyhow::Result<()>
{
    use remoc::ConnectExt as _;
    let browser = Arc::new(Browser {
        windows: Mutex::new(Vec::new()),
        capture_bytes: control::MAX_CAPTURE_BYTES,
        label: "after screenshot",
        calls: AtomicUsize::new(0),
        block: AtomicBool::new(false),
    });
    let (server, local_client) = BrowserControlServerShared::<_, codec::Default>::new(browser);
    let serving = tokio::spawn(server.serve());
    let (left, right) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(left);
    let providing = tokio::spawn(
        remoc::Connect::io(control::transport_config(), read, write).provide(local_client),
    );
    let (read, write) = tokio::io::split(right);
    let client: BrowserControlClient = remoc::Connect::io(control::transport_config(), read, write)
        .consume()
        .await?;
    let capture = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.capture(u64::MAX, None),
    )
    .await??
    .map_err(anyhow::Error::msg)?;
    capture.validate().map_err(anyhow::Error::msg)?;
    assert_eq!(capture.png.len(), control::MAX_CAPTURE_BYTES);
    assert!(capture.png.len() > remoc::Cfg::default().max_data_size);
    let reply = client
        .execute(ControlDispatch {
            request_id: 2,
            command_json: r#"{"operation":"status"}"#.into(),
            expires_at_ms: u64::MAX,
        })
        .await?
        .map_err(anyhow::Error::msg)?;
    assert_eq!(
        serde_json::from_str::<Value>(&reply)?["value"],
        "after screenshot"
    );
    serving.abort();
    providing.abort();
    Ok(())
}

#[tokio::test]
async fn http_routes_allow_local_commands_without_credentials_and_reject_forwarding()
-> anyhow::Result<()> {
    let broker = Broker::new();
    let (_connection, _, browser, task) = attach(&broker, "local");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let app = broker.routes();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
    });
    let client = reqwest::Client::new();
    let url = format!("http://{address}/api/control");
    let reply = client
        .post(&url)
        .json(&json!({"operation": "status"}))
        .send()
        .await?;
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(reply.json::<Value>().await?["value"], "local");
    for operation in ["targets", "screenshot"] {
        let reply = client
            .post(&url)
            .json(&json!({"operation": operation, "window": "files:12"}))
            .send()
            .await?;
        assert_eq!(reply.status(), StatusCode::OK);
    }
    assert_eq!(
        *browser.windows.lock().expect("recorded window selectors"),
        [None, Some("files:12".into()), Some("files:12".into())]
    );
    for header in [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-real-ip",
    ] {
        let reply = client
            .post(&url)
            .header(header, "127.0.0.1")
            .json(&json!({"operation": "status"}))
            .send()
            .await?;
        assert_eq!(reply.status(), StatusCode::NOT_FOUND);
    }
    assert_eq!(
        client
            .post(&url)
            .header(header::ORIGIN, "http://localhost")
            .json(&json!({"operation": "status"}))
            .send()
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client
            .post(&url)
            .header(header::HOST, "example.com")
            .json(&json!({"operation": "status"}))
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 3);
    server.abort();
    task.abort();
    Ok(())
}

struct Browser {
    windows: Mutex<Vec<Option<String>>>,
    capture_bytes: usize,
    label: &'static str,
    calls: AtomicUsize,
    block: AtomicBool,
}

impl BrowserControl for Browser {
    fn diagnostics(
        &self,
    ) -> impl Future<
        Output = Result<
            remoc::rch::mpsc::Receiver<garmin_model::diagnostics::Update>,
            rtc::CallError,
        >,
    > {
        std::future::ready(Ok(remoc::rch::mpsc::with_local_buffer(1).1))
    }
    fn capture(
        &self,
        _expires_at_ms: u64,
        window: Option<String>,
    ) -> impl Future<Output = Result<Result<control::Capture, String>, rtc::CallError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.windows.lock().expect("window selectors").push(window);
        // The broker validates the PNG envelope; renderer tests cover actual decoding.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1_u32.to_be_bytes());
        png.extend_from_slice(&1_u32.to_be_bytes());
        png.resize(self.capture_bytes, u8::MAX);
        std::future::ready(Ok(Ok(control::Capture {
            info: control::CaptureInfo {
                width: if self.block.load(Ordering::SeqCst) {
                    2
                } else {
                    1
                },
                height: 1,
                pixels_per_point: 1.0,
                requested_frame: 10,
                received_frame: 11,
            },
            png,
        })))
    }
    async fn execute(
        &self,
        request: ControlDispatch,
    ) -> Result<Result<String, String>, rtc::CallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let command: control::ControlCommand =
            serde_json::from_str(&request.command_json).expect("forwarded command");
        self.windows
            .lock()
            .expect("window selectors")
            .push(command.window);
        if self.block.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(Ok(json!({"value": self.label}).to_string()))
    }
}

fn attach(
    broker: &Broker,
    label: &'static str,
) -> (
    Connection,
    ControlSession,
    Arc<Browser>,
    tokio::task::JoinHandle<()>,
) {
    let browser = Arc::new(Browser {
        windows: Mutex::new(Vec::new()),
        capture_bytes: 24,
        label,
        calls: AtomicUsize::new(0),
        block: AtomicBool::new(false),
    });
    let (server, client) = BrowserControlServerShared::<_, codec::Default>::new(browser.clone());
    let task = tokio::spawn(async move {
        let _ = server.serve().await;
    });
    let connection = broker.connection();
    let session = connection
        .register(Uuid::new_v4().to_string(), client)
        .expect("register browser");
    (connection, session, browser, task)
}

fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, "localhost:8099".parse().expect("test host"));
    headers
}

#[tokio::test]
async fn screenshots_are_binary_correlated_and_share_command_admission() {
    let broker = Broker::new();
    let (_connection, _info, browser, task) = attach(&broker, "capture");
    let capture_request = |id| {
        let mut envelope = request(id);
        envelope.operation = "screenshot".into();
        envelope
    };
    let mut origin = headers();
    origin.insert(
        header::ORIGIN,
        "http://localhost:8099".parse().expect("origin"),
    );
    let response = control_request(State(broker.clone()), origin, Json(capture_request(1))).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(browser.calls.load(Ordering::SeqCst), 0);
    let response =
        control_request(State(broker.clone()), headers(), Json(capture_request(1))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert!(!response.headers().contains_key("x-garmin-session"));
    assert_eq!(response.headers()["x-garmin-request-id"], "1");
    let body = axum::body::to_bytes(response.into_body(), control::MAX_CAPTURE_BYTES)
        .await
        .expect("PNG body");
    assert_eq!(&body[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        control_request(State(broker.clone()), headers(), Json(capture_request(1)))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    browser.block.store(true, Ordering::SeqCst);
    assert_eq!(
        control_request(State(broker.clone()), headers(), Json(capture_request(2)))
            .await
            .status(),
        StatusCode::BAD_GATEWAY
    );
    let mut invalid = capture_request(3);
    invalid.argument = json!({"window": "child"});
    assert_eq!(
        control_request(State(broker), headers(), Json(invalid))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 2);
    task.abort();
}

fn request(request_id: u64) -> Envelope {
    Envelope {
        window: None,
        request_id: Some(request_id),
        operation: "status".into(),
        argument: Value::Null,
    }
}

#[tokio::test]
async fn requests_select_the_only_browser_and_reject_ambiguity_and_replay() {
    let broker = Broker::new();
    let (connection_a, _a, first, task_a) = attach(&broker, "first");
    let id = 1;
    let (status, Json(reply)) = dispatch(State(broker.clone()), headers(), Json(request(id))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["value"], "first");
    assert_eq!(reply["request_id"], id);
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        dispatch(State(broker.clone()), headers(), Json(request(id)))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);
    let (connection_b, _b, second, task_b) = attach(&broker, "second");
    assert_eq!(
        dispatch(State(broker.clone()), headers(), Json(request(2)))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(second.calls.load(Ordering::SeqCst), 0);
    drop(connection_a);
    let (_, Json(reply)) = dispatch(State(broker.clone()), headers(), Json(request(2))).await;
    assert_eq!(reply["value"], "second");
    drop(connection_b);
    assert_eq!(
        dispatch(State(broker), headers(), Json(request(3))).await.0,
        StatusCode::NOT_FOUND
    );
    task_a.abort();
    task_b.abort();
}

#[tokio::test]
async fn busy_and_unsupported_requests_do_not_reach_the_browser() {
    let broker = Broker::new();
    let (_connection, info, browser, task) = attach(&broker, "first");
    let session = broker
        .active_sessions()
        .get(&info.id)
        .cloned()
        .expect("registered session");
    let _permit = session.admission.acquire().await.expect("permit");
    assert_eq!(
        dispatch(State(broker.clone()), headers(), Json(request(2)))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let mut unsupported = request(2);
    unsupported.operation = "eval".into();
    assert_eq!(
        dispatch(State(broker), headers(), Json(unsupported))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn reconnect_uses_a_new_session_and_registration_is_single_use() {
    let broker = Broker::new();
    let browser = Arc::new(Browser {
        windows: Mutex::new(Vec::new()),
        label: "reconnecting",
        capture_bytes: 24,
        calls: AtomicUsize::new(0),
        block: AtomicBool::new(false),
    });
    let (server, client) = BrowserControlServerShared::<_, codec::Default>::new(browser);
    let task = tokio::spawn(async move {
        let _ = server.serve().await;
    });
    let page = Uuid::new_v4().to_string();
    let original = broker.connection();
    let first = original
        .register(page.clone(), client.clone())
        .expect("first registration");
    assert!(original.register(page.clone(), client.clone()).is_err());
    let reconnected = broker.connection();
    let second = reconnected
        .register(page, client)
        .expect("reconnected registration");
    assert_ne!(first.id, second.id);
    assert_eq!(first.browser, second.browser);
    drop(original);
    assert!(!broker.active_sessions().contains_key(&first.id));
    assert!(broker.active_sessions().contains_key(&second.id));
    task.abort();
}

#[tokio::test]
async fn timed_out_request_is_not_replayed() {
    let broker = Broker::new();
    let (_connection, _info, browser, task) = attach(&broker, "blocked");
    browser.block.store(true, Ordering::SeqCst);
    let id = 1;
    let (status, Json(reply)) = dispatch(State(broker.clone()), headers(), Json(request(id))).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert!(
        reply["error"]
            .as_str()
            .expect("error")
            .contains("outcome may be unknown")
    );
    assert_eq!(
        dispatch(State(broker), headers(), Json(request(id)))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[test]
fn localhost_means_the_socket_peer_and_never_forwarded_headers() {
    let local_headers = headers();
    for address in ["127.0.0.1:8123", "[::1]:8123", "[::ffff:127.0.0.1]:8123"] {
        assert!(local_connection(
            Some(address.parse().expect("peer")),
            &local_headers
        ));
    }
    assert!(!local_connection(None, &local_headers));
    assert!(!local_connection(
        Some("192.168.1.2:8123".parse().expect("peer")),
        &local_headers
    ));
    for name in [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-real-ip",
    ] {
        let mut headers = headers();
        headers.insert(name, "127.0.0.1".parse().expect("header"));
        assert!(!local_connection(
            Some("127.0.0.1:8123".parse().expect("peer")),
            &headers
        ));
    }
}

#[test]
fn local_hosts_are_required_even_with_a_loopback_peer() {
    let peer = Some("127.0.0.1:8123".parse().expect("peer"));
    assert!(!local_connection(peer, &HeaderMap::new()));
    for (host, expected) in [
        ("localhost:8099", true),
        ("LOCALHOST", true),
        ("127.0.0.1:8099", true),
        ("[::1]:8099", true),
        ("[::ffff:127.0.0.1]:8099", true),
        ("example.com", false),
        ("localhost.example.com", false),
        ("192.168.1.3:8099", false),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().expect("host"));
        assert_eq!(local_connection(peer, &headers), expected, "{host}");
    }
}

#[tokio::test]
async fn commands_need_no_ids_and_explicit_replays_fail_across_reconnects() {
    let broker = Broker::new();
    let (connection, _, _, first) = attach(&broker, "first");
    let envelope: Envelope =
        serde_json::from_value(json!({"operation": "status"})).expect("plain command");
    let (status, Json(reply)) = dispatch(State(broker.clone()), headers(), Json(envelope)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["request_id"], 1);
    assert!(reply.get("session").is_none());
    assert!(
        serde_json::from_value::<Envelope>(json!({"operation": "status", "session": "old"}))
            .is_err()
    );
    drop(connection);
    let (_connection, _, browser, second) = attach(&broker, "second");
    assert_eq!(
        dispatch(State(broker), headers(), Json(request(1))).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 0);
    first.abort();
    second.abort();
}
