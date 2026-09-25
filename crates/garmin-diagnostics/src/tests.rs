use super::*;
use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use futures_util::StreamExt as _;
use garmin_model::diagnostics::Observation;
use std::{collections::BTreeMap, net::SocketAddr};
use tower::ServiceExt as _;

fn request(path: &str) -> axum::http::request::Builder {
    Request::builder()
        .uri(path)
        .header("host", "localhost:8099")
        .extension(ConnectInfo(
            "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
        ))
}

fn observation(message: &str) -> Observation {
    Observation {
        kind: "renderer".into(),
        window: "root".into(),
        fields: BTreeMap::from([("message".into(), message.into())]),
        removed: false,
    }
}

#[test]
fn closing_a_window_removes_all_its_state_but_preserves_other_owners() {
    let events = Events::default();
    let query = query::Query::parse("", false, false, "").unwrap();
    for source in ["desktop", "other"] {
        for kind in ["window", "automation", "renderer"] {
            let mut item = observation("ready");
            item.kind = kind.into();
            events.publish(source, item);
        }
    }
    let before = events.read(&query).cursor;
    let mut closed = observation("closed");
    closed.kind = "window".into();
    closed.removed = true;
    events.publish("desktop", closed.clone());
    let query = query::Query {
        after: Some(before),
        ..query
    };
    let after = events.read(&query);
    let state = after.state.unwrap();
    assert_eq!(state.len(), 3);
    assert!(state.iter().all(|event| event.source == "other"));
    assert_eq!(after.records.len(), 3);
    assert!(after.records.iter().all(|event| event.observation.removed));
    events.publish("desktop", closed);
    assert_eq!(events.read(&query).cursor, after.cursor);
}

#[test]
fn unavailable_telemetry_clears_stale_state_without_disconnect_claims() {
    let events = Events::default();
    events.publish(
        "browser",
        Observation {
            kind: "connection".into(),
            window: String::new(),
            fields: BTreeMap::from([
                ("status".into(), "connected".into()),
                ("title".into(), "Application".into()),
            ]),
            removed: false,
        },
    );
    events.publish("browser", observation("ready"));
    events.unavailable("browser", "Diagnostic subscription closed");
    let query = query::Query::parse("", false, false, "").unwrap();
    let batch = events.read(&query);
    let state = batch.state.unwrap();
    assert_eq!(state.len(), 1);
    assert_eq!(state[0].observation.kind, "connection");
    assert_eq!(state[0].observation.fields["status"], "connected");
    assert_eq!(state[0].observation.fields["diagnostics"], "unavailable");
    assert_eq!(state[0].observation.fields["title"], "Application");
    assert!(batch.records.iter().any(|event| event.observation.removed));
    assert!(
        !batch
            .records
            .iter()
            .any(|event| event.observation.kind == "connection" && event.observation.removed)
    );
    events.update(
        "browser",
        garmin_model::diagnostics::Update {
            revision: 1,
            ..Default::default()
        },
    );
    let recovered = events.read(&query).state.unwrap();
    assert_eq!(recovered[0].observation.fields["status"], "connected");
    assert_eq!(recovered[0].observation.fields["diagnostics"], "ready");
    assert!(!recovered[0].observation.fields.contains_key("error"));
}

#[test]
fn source_gaps_survive_all_filters_and_older_history_eviction() {
    let adapter = Diagnostics::new(None, "browser");
    for index in 0..2048 {
        adapter
            .events
            .publish("browser", observation(&index.to_string()));
    }
    let mut query = query::Query::parse(
        "kind=automation&window=child&source=browser&until_ms=0",
        false,
        false,
        "",
    )
    .unwrap();
    query.after = Some(adapter.events.read(&query).cursor);
    let mut current = observation("passed");
    current.kind = "automation".into();
    current.window = "child".into();
    adapter.events.update(
        "browser",
        garmin_model::diagnostics::Update {
            revision: 1,
            state: vec![current.clone()],
            gap: true,
            ..Default::default()
        },
    );
    let batch = adapter.events.read(&query);
    assert!(batch.gap);
    assert!(batch.records.is_empty());
    assert_eq!(batch.state.unwrap()[0].observation, current);
    query.after = Some(batch.cursor);
    assert!(!adapter.events.read(&query).gap);
    let all = adapter
        .events
        .read(&query::Query::parse("kind=connection", false, false, "").unwrap());
    let connection = &all.state.as_ref().unwrap()[0].observation;
    assert_eq!(connection.fields["status"], "ready");
    assert_eq!(connection.fields["diagnostics"], "ready");
    assert!(all.records.iter().all(|event| {
        event
            .observation
            .fields
            .get("status")
            .is_none_or(|value| value != "gap")
    }));
}

#[tokio::test]
async fn filtered_streams_ignore_unmatched_records_and_still_deliver_matches() {
    use garmin_model::logging::{Level, Record};
    for logs in [false, true] {
        for format in ["text", "ansi", "sse"] {
            let directory = tempfile::tempdir().unwrap();
            let store = garmin_logging::Store::open(directory.path(), "test").unwrap();
            let adapter = Diagnostics::new(Some(store.clone()), "test");
            let path = if logs {
                "logs-stream?minimum=error"
            } else {
                "events-stream?kind=automation"
            };
            let response = adapter
                .routes()
                .oneshot(
                    request(&format!("/api/{path}&format={format}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let mut stream = response.into_body().into_data_stream();
            stream.next().await.unwrap().unwrap();
            for (sequence, level, message) in
                [(1, Level::Info, "unmatched"), (2, Level::Error, "matched")]
            {
                if logs {
                    store
                        .append(vec![Record {
                            sequence: 0,
                            timestamp_ms: sequence,
                            level,
                            component: "test".into(),
                            source: "test".into(),
                            session: "test".into(),
                            source_sequence: sequence,
                            message: message.into(),
                            fields: BTreeMap::new(),
                        }])
                        .unwrap();
                } else {
                    let mut item = observation(message);
                    if sequence == 2 {
                        item.kind = "automation".into();
                    }
                    adapter.events.publish("test", item);
                }
                let next =
                    tokio::time::timeout(std::time::Duration::from_millis(450), stream.next())
                        .await;
                if sequence == 1 {
                    assert!(next.is_err(), "unmatched records emitted a {format} batch");
                } else {
                    let data = String::from_utf8(next.unwrap().unwrap().unwrap().to_vec()).unwrap();
                    assert!(data.contains("matched"));
                    assert!(!data.contains("unmatched"));
                }
            }
            adapter.stop();
        }
    }
}

#[tokio::test]
async fn idle_text_and_sse_streams_wait_for_records_or_new_errors() {
    for format in ["text", "ansi", "sse"] {
        let adapter = Diagnostics::new(None, "test");
        let response = adapter
            .routes()
            .oneshot(
                request(&format!("/api/events-stream?format={format}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut stream = response.into_body().into_data_stream();
        stream.next().await.unwrap().unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(450), stream.next())
                .await
                .is_err()
        );
        adapter.events.update(
            "browser",
            garmin_model::diagnostics::Update {
                revision: 1,
                incomplete: true,
                ..Default::default()
            },
        );
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8(error.to_vec())
                .unwrap()
                .contains("incomplete")
        );
        adapter.events.publish("test", observation("new record"));
        let record = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8(record.to_vec())
                .unwrap()
                .contains("new record")
        );
        adapter.stop();
        assert!(stream.next().await.is_none());
    }
}

#[test]
fn snapshot_stays_current_while_history_pages_and_time_filters_apply() {
    let events = Events::default();
    events.publish("a", observation("first"));
    let mut query = query::Query::parse("limit=1", false, false, "").unwrap();
    let first = events.read(&query);
    events.publish("b", observation("unmatched"));
    events.publish("a", observation("second"));
    events.publish("a", observation("third"));
    query.after = Some(first.cursor);
    query.logs.source = "a".into();
    let page = events.read(&query);
    assert!(page.more);
    assert_eq!(page.records[0].observation.fields["message"], "second");
    assert_eq!(
        page.state.unwrap()[0].observation.fields["message"],
        "third"
    );
    assert!(page.snapshot_revision.unwrap().sequence > page.cursor.sequence);
    query.after = Some(page.cursor);
    let last = events.read(&query);
    assert!(!last.more);
    assert_eq!(last.records[0].observation.fields["message"], "third");
    query.logs.until_ms = Some(0);
    let filtered = events.read(&query);
    assert!(filtered.records.is_empty());
    assert_eq!(filtered.state.unwrap().len(), 1);
}

#[test]
fn terminal_events_survive_progress_and_eviction_reports_a_gap() {
    let events = Events::default();
    let mut item = observation("renderer ready");
    events.publish("test", item.clone());
    let mut query = query::Query::parse("", false, false, "").unwrap();
    query.after = Some(events.read(&query).cursor);
    item.kind = "automation".into();
    item.fields = BTreeMap::from([("state".into(), "passed".into())]);
    events.publish("test", item.clone());
    item.fields.insert("state".into(), "running".into());
    for step in 0..3000 {
        item.fields.insert("completed".into(), step.to_string());
        events.publish("test", item.clone());
    }
    let batch = events.read(&query);
    assert!(!batch.gap);
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.records[0].observation.fields["state"], "passed");
    for step in 0..2100 {
        events.publish("test", observation(&step.to_string()));
    }
    let batch = events.read(&query);
    assert!(batch.gap);
    assert!(batch.state.is_some());
    query.after = Some(response::Cursor {
        epoch: uuid::Uuid::nil(),
        sequence: 0,
    });
    assert!(events.read(&query).gap);
}

#[test]
fn logs_take_the_latest_matching_records_and_advance_past_nonmatches() {
    use garmin_model::logging::{Filter, Level, Record};
    let directory = tempfile::tempdir().unwrap();
    let store = garmin_logging::Store::open(directory.path(), "test").unwrap();
    let records = (1..=100)
        .map(|index| Record {
            sequence: 0,
            timestamp_ms: index,
            level: Level::Info,
            component: "test".into(),
            source: "test".into(),
            session: "test".into(),
            source_sequence: index,
            message: "x".repeat(10_000),
            fields: BTreeMap::new(),
        })
        .collect();
    store.append(records).unwrap();
    let (batch, more) = store.recent(&Filter::default(), None, 100);
    assert!(!more);
    assert_eq!(batch.records.last().unwrap().timestamp_ms, 100);
    assert!(batch.records.len() < 100);
    let filter = Filter {
        text: "never matches".into(),
        ..Filter::default()
    };
    let (empty, _) = store.recent(&filter, None, 10);
    assert!(empty.records.is_empty());
    assert_eq!(empty.cursor, batch.cursor);
}

#[tokio::test]
async fn dropping_a_stream_releases_its_independent_admission_slot() {
    let adapter = Diagnostics::new(None, "test");
    let router = adapter.routes();
    let mut streams = Vec::new();
    for _ in 0..32 {
        let response = router
            .clone()
            .oneshot(request("/api/events-stream").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        streams.push(response);
    }
    let rejected = router
        .clone()
        .oneshot(request("/api/events-stream").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(streams.pop());
    let admitted = router
        .oneshot(request("/api/events-stream").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
}

#[tokio::test]
async fn html_is_rendered_and_escaped_without_client_record_formatting() {
    let adapter = Diagnostics::new(None, "test");
    adapter
        .events
        .publish("test", observation("<script>alert('x')</script>"));
    let response = adapter
        .routes()
        .oneshot(
            request("/api/events-get")
                .header("accept", "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("&#60;script&#62;"));
    assert!(!html.contains("<script>alert"));
    assert!(!html.contains("src=\"?format=script\""));
    assert!(html.contains("Current state"));
}

#[tokio::test]
async fn sse_builder_emits_typed_cursor_and_resumes_after_it() {
    let adapter = Diagnostics::new(None, "test");
    adapter.events.publish("test", observation("one\ntwo"));
    let response = adapter
        .routes()
        .oneshot(
            request("/api/events-stream?format=sse")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.into_body().into_data_stream();
    let data = String::from_utf8(stream.next().await.unwrap().unwrap().to_vec()).unwrap();
    assert!(data.contains("event: batch\n"));
    let cursor = data
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap();
    let json = data
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    let batch: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(batch["cursor"], cursor);
    assert_eq!(batch["records"].as_array().unwrap().len(), 2);
    adapter.events.publish("test", observation("three"));
    let response = adapter
        .routes()
        .oneshot(
            request("/api/events-stream?format=sse&after=00000000000000000000000000000000:0")
                .header("last-event-id", cursor)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut resumed = response.into_body().into_data_stream();
    let data = String::from_utf8(resumed.next().await.unwrap().unwrap().to_vec()).unwrap();
    let batch: serde_json::Value = serde_json::from_str(
        data.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(batch["records"].as_array().unwrap().len(), 1);
    assert_eq!(batch["gap"], false);
    adapter.stop();
    assert!(stream.next().await.is_none());
    assert!(resumed.next().await.is_none());
}

#[tokio::test]
async fn browser_stream_uses_rendered_fragments_and_preserves_machine_sse() {
    let adapter = Diagnostics::new(None, "test");
    adapter.events.publish("test", observation("<b>text</b>"));
    let response = adapter
        .routes()
        .oneshot(
            request("/api/events-stream?format=html")
                .header("accept", "text/event-stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let data = String::from_utf8(stream.next().await.unwrap().unwrap().to_vec()).unwrap();
    assert!(data.contains("event: view\n"));
    let update: serde_json::Value = serde_json::from_str(
        data.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert!(update["records"].as_str().unwrap().contains("&#60;b&#62;"));
    assert!(update["state"].as_str().unwrap().contains("&#60;b&#62;"));
}

#[tokio::test]
async fn rejects_cross_origin_forwarded_and_malformed_queries() {
    let router = Diagnostics::new(None, "test").routes();
    for (path, name, value, expected) in [
        (
            "/api/events-get",
            "origin",
            "http://elsewhere",
            StatusCode::FORBIDDEN,
        ),
        (
            "/api/events-get",
            "x-forwarded-host",
            "localhost",
            StatusCode::NOT_FOUND,
        ),
        (
            "/api/events-get?limit=0",
            "accept",
            "*/*",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/events-get?kind=unknown",
            "accept",
            "*/*",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/events-get?after=nope",
            "accept",
            "*/*",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/events-get?source=a&source=b",
            "accept",
            "*/*",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/events-get",
            "origin",
            "http://localhost:8099",
            StatusCode::OK,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                request(path)
                    .header(name, value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path} {name}");
    }
}

#[tokio::test]
async fn ansi_get_and_stream_preserve_plain_text_and_escape_record_controls() {
    use garmin_model::logging::{Level, Record};
    let directory = tempfile::tempdir().unwrap();
    let store = garmin_logging::Store::open(directory.path(), "test").unwrap();
    let hostile = "injected\x1b[2J\r\n\x07text";
    let levels = [
        Level::Trace,
        Level::Debug,
        Level::Info,
        Level::Warn,
        Level::Error,
    ];
    store
        .append(
            levels
                .into_iter()
                .map(|level| Record {
                    sequence: 0,
                    timestamp_ms: 1234,
                    level,
                    component: hostile.into(),
                    source: hostile.into(),
                    session: "test".into(),
                    source_sequence: 0,
                    message: hostile.into(),
                    fields: BTreeMap::from([(hostile.into(), hostile.into())]),
                })
                .collect(),
        )
        .unwrap();
    let adapter = Diagnostics::new(Some(store), "test");
    let mut item = observation(hostile);
    item.window = hostile.into();
    adapter.events.publish(hostile, item);
    for resource in ["logs", "events"] {
        for mode in ["get", "stream"] {
            let mut bodies = Vec::new();
            for format in ["text", "ansi"] {
                let filter = if resource == "logs" {
                    "&minimum=trace"
                } else {
                    ""
                };
                let response = adapter
                    .routes()
                    .oneshot(
                        request(&format!("/api/{resource}-{mode}?format={format}{filter}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    response.headers()["content-type"],
                    "text/plain; charset=utf-8"
                );
                let mut stream = response.into_body().into_data_stream();
                let body = stream.next().await.unwrap().unwrap();
                bodies.push(String::from_utf8(body.to_vec()).unwrap());
            }
            assert!(!bodies[0].contains('\x1b'));
            assert!(bodies[1].contains('\x1b'));
            assert_eq!(without_styles(&bodies[1]), bodies[0]);
            for body in &bodies {
                assert!(!body.contains("\x1b[2J"));
                assert!(!body.contains(['\r', '\x07']));
                assert!(body.contains("injected\\u{1b}[2J\\r\\n\\u{7}text"));
            }
        }
    }
    adapter.stop();
}

fn without_styles(text: &str) -> String {
    let mut segments = text.split('\x1b');
    let mut plain = segments.next().unwrap().to_owned();
    for segment in segments {
        let (style, remainder) = segment.strip_prefix('[').unwrap().split_once('m').unwrap();
        assert!(
            style
                .chars()
                .all(|character| character.is_ascii_digit() || character == ';')
        );
        plain.push_str(remainder);
    }
    plain
}
