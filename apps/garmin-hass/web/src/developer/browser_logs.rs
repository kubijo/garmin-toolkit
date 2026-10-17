//! Browser callbacks feed the Rust log buffer; workers relay through their parent.
use std::{cell::RefCell, collections::BTreeMap};

use garmin_model::logging::{Level, Record};
use tracing_subscriber::prelude::*;
use wasm_bindgen::prelude::*;

use super::log_buffer::{Batch, Buffer};

thread_local! {
    static BUFFER: RefCell<Buffer> = RefCell::new(Buffer::new(format!(
        "browser-{}-{}", js_sys::Date::now(), js_sys::Math::random()
    )));
}

#[wasm_bindgen(module = "/logging.js")]
extern "C" {
    #[wasm_bindgen(js_name = bindLogs)]
    fn bind_logs(callback: &JsValue);
    #[wasm_bindgen(js_name = postLog)]
    fn post_log(payload: &str);
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Incoming {
    Record {
        record: Record,
    },
    Event {
        level: Level,
        component: String,
        message: String,
        source: String,
    },
}

pub(super) fn install() {
    let callback =
        Closure::<dyn FnMut(String)>::new(|payload: String| {
            match serde_json::from_str::<Incoming>(&payload) {
                Ok(Incoming::Record { record }) => relay(record),
                Ok(Incoming::Event {
                    level,
                    component,
                    message,
                    source,
                }) => {
                    emit(level, &component, &source, &message, BTreeMap::new());
                }
                Err(error) => {
                    web_sys::console::error_1(
                        &format!("Invalid browser log event: {error}").into(),
                    );
                }
            }
        })
        .into_js_value();
    bind_logs(&callback);
    let _ = tracing_subscriber::registry().with(BrowserLogs).try_init();
}

fn emit(
    level: Level,
    component: &str,
    source: &str,
    message: &str,
    fields: BTreeMap<String, String>,
) {
    let record = BUFFER
        .with_borrow_mut(|buffer| buffer.record(now(), level, component, source, message, fields));
    let console = format!("[{component}] {message}").into();
    match level {
        Level::Error => web_sys::console::error_1(&console),
        Level::Warn => web_sys::console::warn_1(&console),
        _ => web_sys::console::debug_1(&console),
    }
    relay(record);
}

fn relay(record: Record) {
    if web_sys::window().is_some() {
        BUFFER.with_borrow_mut(|buffer| buffer.push(record));
    } else {
        post_log(&serde_json::json!({"kind": "record", "record": record}).to_string());
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Date.now is a positive epoch millisecond timestamp"
)]
fn now() -> u64 {
    js_sys::Date::now() as u64
}

pub(super) fn pending() -> Batch {
    BUFFER.with_borrow_mut(|buffer| buffer.pending(now()))
}

pub(super) fn acknowledge(batch: &Batch, rejected: u64) {
    BUFFER.with_borrow_mut(|buffer| {
        buffer.acknowledge(batch);
        buffer.reject(rejected);
    });
}

struct BrowserLogs;
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BrowserLogs {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if *event.metadata().level() > tracing::Level::INFO {
            return;
        }
        let mut fields = Message::default();
        event.record(&mut fields);
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => Level::Error,
            tracing::Level::WARN => Level::Warn,
            _ => Level::Info,
        };
        emit(
            level,
            event.metadata().target(),
            if web_sys::window().is_some() {
                "main"
            } else {
                "worker"
            },
            &fields.0.remove("message").unwrap_or_default(),
            fields.0,
        );
    }
}

#[derive(Default)]
struct Message(BTreeMap<String, String>);
impl tracing::field::Visit for Message {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
}
