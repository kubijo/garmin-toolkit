//! Opt-in HTTP control broker. Each reverse RPC client belongs to one Remoc connection.
use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header, uri::Authority},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use garmin_service_api::control;
use garmin_service_api::control::{
    BrowserControl as _, BrowserControlClient, ControlCommand, ControlDispatch, ControlSession,
};
use remoc::rtc::Client as _;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

const MAX_SESSIONS: usize = 16;
const TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct Broker(Arc<Inner>);

struct Inner {
    started: Instant,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    last_request: Mutex<u64>,
}

struct Session {
    client: BrowserControlClient,
    admission: Arc<Semaphore>,
}

#[derive(Clone)]
pub(super) struct Connection(Arc<ConnectionState>);

struct ConnectionState {
    broker: Broker,
    id: String,
    registered: Mutex<bool>,
}

impl Drop for ConnectionState {
    fn drop(&mut self) {
        self.broker
            .0
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

impl Broker {
    pub(super) fn new() -> Self {
        Self(Arc::new(Inner {
            started: Instant::now(),
            sessions: Mutex::new(HashMap::new()),
            last_request: Mutex::new(0),
        }))
    }

    pub(super) fn connection(&self) -> Connection {
        Connection(Arc::new(ConnectionState {
            broker: self.clone(),
            id: Uuid::new_v4().to_string(),
            registered: Mutex::new(false),
        }))
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub(super) fn routes(&self) -> Router {
        Router::new()
            .route("/api/control", post(control_request))
            .route("/api/capabilities", get(capabilities))
            .layer(DefaultBodyLimit::max(control::MAX_COMMAND_BYTES))
            .layer(middleware::from_fn(local_only))
            .with_state(self.clone())
    }

    fn active_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        let mut sessions = self
            .0
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        sessions.retain(|_, session| !session.client.is_closed());
        sessions
    }
}

impl Connection {
    pub(super) fn register(
        &self,
        browser: String,
        client: BrowserControlClient,
    ) -> Result<ControlSession, String> {
        if Uuid::parse_str(&browser).is_err() {
            return Err("invalid browser ID".into());
        }
        let mut registered = self
            .0
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *registered {
            return Err("this connection already registered a browser".into());
        }
        let mut sessions = self.0.broker.active_sessions();
        if sessions.len() >= MAX_SESSIONS {
            return Err("control session limit reached".into());
        }
        let info = ControlSession {
            id: self.0.id.clone(),
            browser,
            server_time_ms: self.0.broker.now_ms(),
        };
        sessions.insert(
            info.id.clone(),
            Arc::new(Session {
                client,
                admission: Arc::new(Semaphore::new(1)),
            }),
        );
        *registered = true;
        Ok(info)
    }
}

type HttpReply = (StatusCode, Json<Value>);

pub(super) fn local_connection(peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    peer.is_some_and(|peer| peer.ip().to_canonical().is_loopback())
        && headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok()?.parse::<Authority>().ok())
            .is_some_and(|authority| {
                let host = authority
                    .host()
                    .trim_start_matches('[')
                    .trim_end_matches(']');
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.to_canonical().is_loopback())
            })
        && !headers.keys().any(|name| {
            let name = name.as_str();
            name == "forwarded" || name.starts_with("x-forwarded-") || name == "x-real-ip"
        })
}

async fn local_only(request: axum::extract::Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    if !local_connection(peer, request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

fn error(status: StatusCode, message: &str) -> HttpReply {
    (status, Json(json!({"error": message})))
}

async fn capabilities(headers: HeaderMap) -> HttpReply {
    if headers.contains_key(header::ORIGIN) {
        return error(StatusCode::FORBIDDEN, "browser origins are not accepted");
    }
    let mut value = control::capabilities();
    value["requires_session"] = json!(false);
    value["child_windows"] = json!(false);
    (StatusCode::OK, Json(value))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    #[serde(default)]
    request_id: Option<u64>,
    operation: String,
    #[serde(default)]
    argument: Value,
}

fn admit(
    broker: &Broker,
    headers: &HeaderMap,
    request: &Envelope,
) -> Result<(Arc<Session>, OwnedSemaphorePermit, u64), HttpReply> {
    if headers.contains_key(header::ORIGIN) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "browser origins are not accepted",
        ));
    }
    if request.request_id == Some(0) || !control::OPERATIONS.contains(&request.operation.as_str()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid request ID or unsupported operation",
        ));
    }
    let sessions = broker.active_sessions();
    if sessions.len() > 1 {
        return Err(error(
            StatusCode::CONFLICT,
            "multiple app tabs are connected; close the extra app tabs",
        ));
    }
    let Some(session) = sessions.values().next().cloned() else {
        return Err(error(StatusCode::NOT_FOUND, "no app browser is connected"));
    };
    drop(sessions);
    let Ok(permit) = session.admission.clone().try_acquire_owned() else {
        return Err(error(
            StatusCode::TOO_MANY_REQUESTS,
            "a command is awaiting this browser; retry with a new request ID",
        ));
    };
    let id = {
        let mut last = broker
            .0
            .last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let id = request
            .request_id
            .or_else(|| last.checked_add(1))
            .ok_or_else(|| error(StatusCode::CONFLICT, "request ID exhausted"))?;
        if id <= *last {
            return Err(error(
                StatusCode::CONFLICT,
                "request ID must increase; commands are never replayed",
            ));
        }
        *last = id;
        id
    };
    Ok((session, permit, id))
}

async fn control_request(
    State(broker): State<Broker>,
    headers: HeaderMap,
    Json(request): Json<Envelope>,
) -> Response {
    if request.operation == "screenshot" {
        capture(broker, headers, request).await
    } else {
        dispatch(State(broker), headers, Json(request))
            .await
            .into_response()
    }
}

async fn capture(broker: Broker, headers: HeaderMap, request: Envelope) -> Response {
    let (session, _permit, request_id) = match admit(&broker, &headers, &request) {
        Ok(admission) => admission,
        Err(reply) => return reply.into_response(),
    };
    if !request.argument.is_null() {
        return error(
            StatusCode::BAD_REQUEST,
            "screenshot takes no argument; only the root window is supported",
        )
        .into_response();
    }
    let deadline = broker.now_ms().saturating_add(TIMEOUT.as_secs() * 1_000);
    let mut response = match tokio::time::timeout(TIMEOUT, session.client.capture(deadline)).await {
        Ok(Ok(Ok(capture))) => {
            if let Err(reason) = capture.validate() {
                return error(StatusCode::BAD_GATEWAY, &reason).into_response();
            }
            let metadata = serde_json::to_string(&capture.info).unwrap_or_default();
            (
                [
                    (header::CONTENT_TYPE, "image/png"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                [("x-garmin-capture", metadata)],
                capture.png,
            )
                .into_response()
        }
        Ok(Ok(Err(reason))) => error(StatusCode::BAD_REQUEST, &reason).into_response(),
        Ok(Err(error)) => {
            tracing::warn!(%error, "Screenshot RPC failed");
            (
                StatusCode::GONE,
                Json(json!({"error": "browser screenshot reply failed; not retried"})),
            )
                .into_response()
        }
        Err(_) => error(
            StatusCode::GATEWAY_TIMEOUT,
            "screenshot timed out; not retried",
        )
        .into_response(),
    };
    if let Ok(value) = request_id.to_string().parse() {
        response.headers_mut().insert("x-garmin-request-id", value);
    }
    response
}

async fn dispatch(
    State(broker): State<Broker>,
    headers: HeaderMap,
    Json(request): Json<Envelope>,
) -> HttpReply {
    let (session, _permit, request_id) = match admit(&broker, &headers, &request) {
        Ok(admission) => admission,
        Err(reply) => return reply,
    };
    let Ok(command_json) = serde_json::to_string(&ControlCommand {
        operation: request.operation,
        argument: request.argument,
    }) else {
        return error(StatusCode::BAD_REQUEST, "invalid command");
    };
    let pending = session.client.execute(ControlDispatch {
        request_id,
        command_json,
        expires_at_ms: broker.now_ms().saturating_add(TIMEOUT.as_secs() * 1_000),
    });
    let mut response = match tokio::time::timeout(TIMEOUT, pending).await {
        Ok(Ok(Ok(reply))) if reply.len() <= control::MAX_REPLY_BYTES => {
            match serde_json::from_str::<Value>(&reply) {
                Ok(value)
                    if value.is_object()
                        && (value.get("value").is_some() || value.get("error").is_some()) =>
                {
                    (
                        if value.get("error").is_some() {
                            StatusCode::BAD_REQUEST
                        } else {
                            StatusCode::OK
                        },
                        Json(value),
                    )
                }
                _ => error(StatusCode::BAD_GATEWAY, "invalid browser reply"),
            }
        }
        Ok(Ok(Ok(_))) => error(StatusCode::BAD_GATEWAY, "browser reply exceeds size limit"),
        Ok(Ok(Err(reason))) => error(StatusCode::BAD_REQUEST, &reason),
        Ok(Err(_)) => error(
            StatusCode::GONE,
            "browser disconnected; command outcome may be unknown; not retried",
        ),
        Err(_) => error(
            StatusCode::GATEWAY_TIMEOUT,
            "browser command timed out; outcome may be unknown; not retried",
        ),
    };
    response.1.0["request_id"] = json!(request_id);
    response
}
