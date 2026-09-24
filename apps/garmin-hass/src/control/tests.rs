use super::*;
use garmin_service_api::control::{BrowserControl, BrowserControlServerShared};
use remoc::{codec, rtc, rtc::ServerShared as _};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const TOKEN: &str = "test-control-token-with-at-least-32-characters";

struct Browser {
    label: &'static str,
    calls: AtomicUsize,
    block: AtomicBool,
}

impl BrowserControl for Browser {
    async fn execute(
        &self,
        _request: ControlDispatch,
    ) -> Result<Result<String, String>, rtc::CallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {TOKEN}").parse().expect("test header"),
    );
    headers
}

fn request(session: &str, request_id: u64) -> Envelope {
    Envelope {
        session: session.into(),
        request_id,
        command: ControlCommand {
            operation: "status".into(),
            argument: Value::Null,
        },
    }
}

#[test]
fn authentication_requires_the_token_and_rejects_browser_origins() {
    let broker = Broker::new(TOKEN).expect("broker");
    assert!(!broker.authorized(&HeaderMap::new()));
    let mut headers = headers();
    assert!(broker.authorized(&headers));
    headers.insert(header::ORIGIN, "http://localhost".parse().expect("origin"));
    assert!(!broker.authorized(&headers));
    headers.remove(header::ORIGIN);
    headers.insert(
        header::AUTHORIZATION,
        "Bearer wrong".parse().expect("wrong token"),
    );
    assert!(!broker.authorized(&headers));
    assert!(Broker::new("short").is_err());
}

#[tokio::test]
async fn requests_target_one_browser_and_never_replay_or_retarget_after_disconnect() {
    let broker = Broker::new(TOKEN).expect("broker");
    let (connection_a, a, first, task_a) = attach(&broker, "first");
    let (_connection_b, b, second, task_b) = attach(&broker, "second");
    let id = 1;
    let (status, Json(reply)) =
        dispatch(State(broker.clone()), headers(), Json(request(&a.id, id))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["value"], "first");
    assert_eq!(reply["request_id"], id);
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);
    assert_eq!(second.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        dispatch(State(broker.clone()), headers(), Json(request(&a.id, id)))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(first.calls.load(Ordering::SeqCst), 1);
    drop(connection_a);
    assert_eq!(
        dispatch(State(broker.clone()), headers(), Json(request(&a.id, 2)))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, Json(reply)) = dispatch(State(broker), headers(), Json(request(&b.id, id))).await;
    assert_eq!(reply["value"], "second");
    task_a.abort();
    task_b.abort();
}

#[tokio::test]
async fn busy_and_unsupported_requests_do_not_reach_the_browser() {
    let broker = Broker::new(TOKEN).expect("broker");
    let (_connection, info, browser, task) = attach(&broker, "first");
    let session = broker
        .active_sessions()
        .get(&info.id)
        .cloned()
        .expect("registered session");
    let _permit = session.admission.acquire().await.expect("permit");
    assert_eq!(
        dispatch(State(broker.clone()), headers(), Json(request(&info.id, 2)))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let mut unsupported = request(&info.id, 2);
    unsupported.command.operation = "eval".into();
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
    let broker = Broker::new(TOKEN).expect("broker");
    let browser = Arc::new(Browser {
        label: "reconnecting",
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
    let broker = Broker::new(TOKEN).expect("broker");
    let (_connection, info, browser, task) = attach(&broker, "blocked");
    browser.block.store(true, Ordering::SeqCst);
    let id = 1;
    let (status, Json(reply)) = dispatch(
        State(broker.clone()),
        headers(),
        Json(request(&info.id, id)),
    )
    .await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert!(
        reply["error"]
            .as_str()
            .expect("error")
            .contains("outcome may be unknown")
    );
    assert_eq!(
        dispatch(State(broker), headers(), Json(request(&info.id, id)))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(browser.calls.load(Ordering::SeqCst), 1);
    task.abort();
}
