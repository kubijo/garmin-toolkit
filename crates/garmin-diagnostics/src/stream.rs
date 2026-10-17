use super::{
    Diagnostics,
    query::{Format, Query},
    response::Reply,
    view,
};
use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{
        IntoResponse as _, Response, Sse,
        sse::{Event, KeepAlive},
    },
};
use futures_util::{StreamExt as _, stream};
use std::{io, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, watch};

struct Reader {
    adapter: Diagnostics,
    logs: bool,
    query: Query,
    first: Option<Reply>,
    snapshot: Option<Vec<super::events::Event>>,
    error: Option<String>,
    stopped: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit,
}

impl Reader {
    async fn next(&mut self) -> Option<Result<Reply, io::Error>> {
        loop {
            if *self.stopped.borrow() {
                return None;
            }
            let first = self.first.is_some();
            let mut batch = if let Some(batch) = self.first.take() {
                batch
            } else {
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(200)) => {},
                    _ = self.stopped.changed() => return None,
                }
                match self.adapter.batch(self.logs, &self.query) {
                    Ok(batch) => batch,
                    Err(error) => return Some(Err(io::Error::other(error))),
                }
            };
            batch.omit_unchanged_snapshot(&mut self.snapshot);
            let changed = first || batch.has_updates() || self.error.as_deref() != batch.error();
            self.query.after = Some(batch.cursor());
            self.error = batch.error().map(str::to_owned);
            if changed {
                return Some(Ok(batch));
            }
        }
    }
}

pub(crate) fn response(adapter: Diagnostics, logs: bool, query: Query, first: Reply) -> Response {
    let Ok(permit) = adapter.streams.clone().try_acquire_owned() else {
        return (StatusCode::TOO_MANY_REQUESTS, "too many diagnostic streams").into_response();
    };
    let format = query.format;
    let reader = Reader {
        stopped: adapter.stopped.subscribe(),
        adapter,
        logs,
        query,
        first: Some(first),
        snapshot: None,
        error: None,
        _permit: permit,
    };
    let batches = stream::unfold(reader, |mut reader| async move {
        reader.next().await.map(|batch| (batch, reader))
    });
    if matches!(format, Format::Text | Format::Ansi) {
        let chunks = batches
            .map(move |batch| batch.map(|batch| view::readable(&batch, format == Format::Ansi)));
        return (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            Body::from_stream(chunks),
        )
            .into_response();
    }
    let events = batches.map(move |batch| {
        let batch = batch?;
        let event = Event::default().id(batch.cursor().to_string());
        if format == Format::Html {
            let update = view::Update::render(&batch)?;
            event
                .event("view")
                .json_data(update)
                .map_err(io::Error::other)
        } else {
            event
                .event("batch")
                .json_data(batch)
                .map_err(io::Error::other)
        }
    });
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}
