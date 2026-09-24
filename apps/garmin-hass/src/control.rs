//! Opt-in HTTP control broker. Each reverse RPC client belongs to one Remoc connection.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
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
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

const MAX_SESSIONS: usize = 16;
const TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
mod tests;
pub(super) const TOKEN_ENV: &str = "GARMIN_TOOLKIT_CONTROL_TOKEN";

#[derive(Clone)]
pub(super) struct Broker(Arc<Inner>);

struct Inner {
    authorization: String,
    started: Instant,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

struct Session {
    info: ControlSession,
    client: BrowserControlClient,
    admission: Semaphore,
    last_request: Mutex<u64>,
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
    pub(super) fn from_environment() -> std::io::Result<Self> {
        let token = std::env::var(TOKEN_ENV)
            .map_err(|_| std::io::Error::other(format!("--control-server requires {TOKEN_ENV}")))?;
        Self::new(&token)
    }

    fn new(token: &str) -> std::io::Result<Self> {
        if token.len() < 32 || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(std::io::Error::other(
                "control token must contain at least 32 non-space ASCII characters",
            ));
        }
        Ok(Self(Arc::new(Inner {
            authorization: format!("Bearer {token}"),
            started: Instant::now(),
            sessions: Mutex::new(HashMap::new()),
        })))
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
            .route("/api/control", post(dispatch))
            .route("/api/control/sessions", get(sessions))
            .route("/api/capabilities", get(capabilities))
            .layer(DefaultBodyLimit::max(control::MAX_COMMAND_BYTES))
            .with_state(self.clone())
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        // This endpoint serves external automation clients; browser commands use Remoc.
        !headers.contains_key(header::ORIGIN)
            && headers.get(header::AUTHORIZATION).is_some_and(|value| {
                let supplied = value.as_bytes();
                let expected = self.0.authorization.as_bytes();
                supplied.len() == expected.len()
                    && supplied
                        .iter()
                        .zip(expected)
                        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
                        == 0
            })
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
            last_request_id: 0,
        };
        sessions.insert(
            info.id.clone(),
            Arc::new(Session {
                info: info.clone(),
                client,
                admission: Semaphore::new(1),
                last_request: Mutex::new(0),
            }),
        );
        *registered = true;
        Ok(info)
    }
}

type HttpReply = (StatusCode, Json<Value>);

fn error(status: StatusCode, message: &str) -> HttpReply {
    (status, Json(json!({"error": message})))
}

async fn capabilities(State(broker): State<Broker>, headers: HeaderMap) -> HttpReply {
    if !broker.authorized(&headers) {
        return error(StatusCode::UNAUTHORIZED, "control authorization required");
    }
    let mut value = control::capabilities();
    value["requires_session"] = json!(true);
    value["screenshots"] = json!(false);
    value["child_windows"] = json!(false);
    (StatusCode::OK, Json(value))
}

async fn sessions(State(broker): State<Broker>, headers: HeaderMap) -> HttpReply {
    if !broker.authorized(&headers) {
        return error(StatusCode::UNAUTHORIZED, "control authorization required");
    }
    let mut sessions = broker
        .active_sessions()
        .values()
        .map(|session| {
            let mut info = session.info.clone();
            info.last_request_id = *session
                .last_request
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            info
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|a, b| a.id.cmp(&b.id));
    (StatusCode::OK, Json(json!({"sessions": sessions})))
}

#[derive(Deserialize)]
struct Envelope {
    session: String,
    request_id: u64,
    #[serde(flatten)]
    command: ControlCommand,
}

async fn dispatch(
    State(broker): State<Broker>,
    headers: HeaderMap,
    Json(request): Json<Envelope>,
) -> HttpReply {
    if !broker.authorized(&headers) {
        return error(StatusCode::UNAUTHORIZED, "control authorization required");
    }
    if request.request_id == 0 || !control::OPERATIONS.contains(&request.command.operation.as_str())
    {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid request ID or unsupported operation",
        );
    }
    let Some(session) = broker.active_sessions().get(&request.session).cloned() else {
        return error(
            StatusCode::NOT_FOUND,
            "browser session is disconnected or unknown",
        );
    };
    let Ok(_permit) = session.admission.try_acquire() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "a command is awaiting this browser; retry with a new request ID",
        );
    };
    {
        let mut last = session
            .last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if request.request_id <= *last {
            return error(
                StatusCode::CONFLICT,
                "request ID must increase; commands are never replayed",
            );
        }
        *last = request.request_id;
    }
    let Ok(command_json) = serde_json::to_string(&request.command) else {
        return error(StatusCode::BAD_REQUEST, "invalid command");
    };
    let pending = session.client.execute(ControlDispatch {
        request_id: request.request_id,
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
    response.1.0["session"] = json!(request.session);
    response.1.0["request_id"] = json!(request.request_id);
    response
}
