use super::{
    Events,
    query::{Format, Query},
    response::{Batch, Reply},
    view,
};
use axum::{
    Router,
    extract::{OriginalUri, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{IntoResponse as _, Response},
    routing::get,
};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Semaphore, watch};

const ROUTES: [&str; 4] = [
    "/api/logs-get",
    "/api/logs-stream",
    "/api/events-get",
    "/api/events-stream",
];

#[must_use]
pub fn is_route(path: &str) -> bool {
    ROUTES.iter().any(|route| {
        path == *route
            || path
                .strip_prefix(route)
                .is_some_and(|tail| tail.starts_with('/'))
    })
}

#[derive(Clone)]
pub struct Diagnostics {
    logs: Option<garmin_logging::Store>,
    pub events: Events,
    pub(crate) stopped: watch::Sender<bool>,
    pub(crate) streams: Arc<Semaphore>,
}

impl Diagnostics {
    #[must_use]
    pub fn new(logs: Option<garmin_logging::Store>, source: &str) -> Self {
        let events = Events::default();
        events.publish(
            source,
            garmin_model::diagnostics::Observation {
                kind: "connection".into(),
                window: String::new(),
                removed: false,
                fields: BTreeMap::from([("status".into(), "ready".into())]),
            },
        );
        Self {
            logs,
            events,
            stopped: watch::channel(false).0,
            streams: Arc::new(Semaphore::new(32)),
        }
    }

    pub fn stop(&self) {
        self.stopped.send_replace(true);
    }

    pub fn routes(&self) -> Router {
        let mut router = Router::new();
        for route in ROUTES {
            router = router.route(route, get(read));
        }
        router
            .layer(middleware::from_fn(super::access::local_only))
            .with_state(self.clone())
    }

    pub(crate) fn batch(&self, logs: bool, query: &Query) -> Result<Reply, &'static str> {
        if !logs {
            return Ok(Reply::Events(self.events.read(query)));
        }
        let store = self.logs.as_ref().ok_or("log store unavailable")?;
        let (batch, more) = store.recent(&query.logs, query.after.map(Into::into), query.limit);
        Ok(Reply::Logs(Batch {
            state: None,
            snapshot_revision: None,
            records: batch.records,
            cursor: batch.cursor.into(),
            gap: batch.gap,
            more,
            error: batch.error,
        }))
    }
}

async fn read(
    State(adapter): State<Diagnostics>,
    OriginalUri(uri): OriginalUri,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    if *adapter.stopped.borrow() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let logs = uri.path().starts_with("/api/logs-");
    let live = uri.path().ends_with("-stream");
    let raw = raw.unwrap_or_default();
    if raw == "format=script" {
        return (
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            include_str!("../view.js"),
        )
            .into_response();
    }
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let mut query = match Query::parse(&raw, logs, live, accept) {
        Ok(query) => query,
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };
    if live && let Some(cursor) = headers.get("last-event-id") {
        match cursor.to_str().ok().and_then(|value| value.parse().ok()) {
            Some(cursor) => query.after = Some(cursor),
            None => return (StatusCode::BAD_REQUEST, "invalid Last-Event-ID").into_response(),
        }
    }
    let batch = match adapter.batch(logs, &query) {
        Ok(batch) => batch,
        Err(error) => return (StatusCode::SERVICE_UNAVAILABLE, error).into_response(),
    };
    if query.format == Format::Html && (!live || !accept.contains("text/event-stream")) {
        return view::page(&batch, logs, live);
    }
    if !live {
        return match query.format {
            Format::Json => axum::Json(batch).into_response(),
            _ => (
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                view::readable(&batch, query.format == Format::Ansi),
            )
                .into_response(),
        };
    }
    super::stream::response(adapter, logs, query, batch)
}
