//! One-shot HTTP response faults for exercising the browser loader before WASM starts.
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

const MAX_DELAY_MS: u64 = 30_000;

#[derive(Clone, Default)]
pub(super) struct Faults(Arc<Mutex<Option<Rule>>>);

struct Rule {
    path: String,
    effect: Effect,
}

enum Effect {
    Fail,
    Delay(Duration),
    UnknownSize,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Fail { path: String },
    Delay { path: String, milliseconds: u64 },
    UnknownSize { path: String },
    Clear,
}

impl Faults {
    pub(super) fn routes(&self) -> Router {
        Router::new()
            .route("/api/browser-response", post(configure))
            .layer(middleware::from_fn(crate::control::local_only))
            .with_state(self.clone())
    }

    fn take(&self, path: &str) -> Option<Effect> {
        let mut rule = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if rule.as_ref().is_some_and(|rule| rule.path == path) {
            rule.take().map(|rule| rule.effect)
        } else {
            None
        }
    }
}

async fn configure(
    State(faults): State<Faults>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> (StatusCode, Json<Value>) {
    if headers.contains_key(header::ORIGIN) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "browser origins are not accepted"})),
        );
    }
    let rule = match command {
        Command::Clear => None,
        Command::Fail { path } => Some(Rule {
            path,
            effect: Effect::Fail,
        }),
        Command::Delay { path, milliseconds } if (1..=MAX_DELAY_MS).contains(&milliseconds) => {
            Some(Rule {
                path,
                effect: Effect::Delay(Duration::from_millis(milliseconds)),
            })
        }
        Command::UnknownSize { path } => Some(Rule {
            path,
            effect: Effect::UnknownSize,
        }),
        Command::Delay { .. } => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "milliseconds must be between 1 and 30000"})),
            );
        }
    };
    if rule.as_ref().is_some_and(|rule| !asset_path(&rule.path)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "path must name a browser entry point or asset"})),
        );
    }
    *faults.0.lock().unwrap_or_else(PoisonError::into_inner) = rule;
    (StatusCode::OK, Json(json!({"value": "configured"})))
}

fn asset_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    if rest.split('/').any(|part| {
        part.is_empty()
            || matches!(part, "." | "..")
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    }) {
        return false;
    }
    rest.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension,
            "html" | "js" | "wasm" | "css" | "svg" | "png" | "jpg" | "webp" | "woff" | "woff2"
        )
    })
}

pub(super) async fn apply(
    State(faults): State<Option<Faults>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let effect = if request.method() == Method::GET {
        faults
            .as_ref()
            .and_then(|faults| faults.take(request.uri().path()))
    } else {
        None
    };
    match effect {
        Some(Effect::Fail) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Some(Effect::Delay(duration)) => {
            tokio::time::sleep(duration).await;
            next.run(request).await
        }
        Some(Effect::UnknownSize) => {
            let response = next.run(request).await;
            if !response.status().is_success() {
                return response;
            }
            let (mut parts, body) = response.into_parts();
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from_stream(body.into_data_stream()))
        }
        None => next.run(request).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{Faults, apply, asset_path};
    use axum::{Router, middleware, routing::get};
    use reqwest::StatusCode;
    use std::{net::SocketAddr, time::Duration};

    #[test]
    fn faults_accept_only_browser_asset_paths() {
        for path in [
            "/",
            "/index.html",
            "/app-ABCDEFG2.js",
            "/chunks/app_bg.wasm",
        ] {
            assert!(asset_path(path), "{path}");
        }
        for path in [
            "api/control",
            "/api/control",
            "/download/token",
            "/map/tiles/0/0/0.pbf",
            "/../index.html",
            "//index.html",
            "/app.js?x=1",
        ] {
            assert!(!asset_path(path), "{path}");
        }
    }

    #[tokio::test]
    async fn response_faults_are_local_one_shot_and_preserve_normal_reads() -> anyhow::Result<()> {
        let faults = Faults::default();
        let app = Router::new()
            .route("/app.js", get(|| async { "ready" }))
            .route("/other.js", get(|| async { "other" }))
            .merge(faults.routes())
            .layer(middleware::from_fn_with_state(Some(faults), apply));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        });
        let client = reqwest::Client::builder().no_proxy().build()?;
        let control = format!("http://{address}/api/browser-response");
        let asset = format!("http://{address}/app.js");
        let other = format!("http://{address}/other.js");

        let result = async {
            assert_eq!(
                client
                    .post(&control)
                    .header("origin", format!("http://{address}"))
                    .json(&serde_json::json!({"kind": "fail", "path": "/app.js"}))
                    .send()
                    .await?
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                client
                    .post(&control)
                    .json(&serde_json::json!({"kind": "fail", "path": "/app.js"}))
                    .send()
                    .await?
                    .status(),
                StatusCode::OK
            );
            assert_eq!(client.get(&other).send().await?.text().await?, "other");
            assert_eq!(
                client.get(&asset).send().await?.status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert_eq!(client.get(&asset).send().await?.text().await?, "ready");

            assert_eq!(
                client
                    .post(&control)
                    .json(&serde_json::json!({"kind": "unknown_size", "path": "/app.js"}))
                    .send()
                    .await?
                    .status(),
                StatusCode::OK
            );
            let response = client.get(&asset).send().await?;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.content_length(), None);
            assert_eq!(response.text().await?, "ready");
            assert_eq!(client.get(&asset).send().await?.text().await?, "ready");

            assert_eq!(
                client
                    .post(&control)
                    .json(
                        &serde_json::json!({"kind": "delay", "path": "/app.js", "milliseconds": 50})
                    )
                    .send()
                    .await?
                    .status(),
                StatusCode::OK
            );
            let started = tokio::time::Instant::now();
            assert_eq!(client.get(&asset).send().await?.text().await?, "ready");
            assert!(started.elapsed() >= Duration::from_millis(40));

            for command in [
                serde_json::json!({"kind": "fail", "path": "/../index.html"}),
                serde_json::json!({"kind": "delay", "path": "/app.js", "milliseconds": 30001}),
            ] {
                assert_eq!(
                    client.post(&control).json(&command).send().await?.status(),
                    StatusCode::BAD_REQUEST
                );
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        server.abort();
        let _ = server.await;
        result
    }
}
