//! Browser map worker transport and local fallback.

use std::{cell::RefCell, rc::Rc};

use garmin_ui::activity;
use wasm_bindgen::{JsCast as _, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};

const MAX_MAP_TILE_BYTES: usize = 2 * 1024 * 1024;
const MAP_WORKER_INITIALIZATION_TIMEOUT_MILLISECONDS: i32 = 10_000;
const MAP_WORKER_TASK_TIMEOUT_MILLISECONDS: i32 = 20_000;

/// Worker entry point for MVT parsing, styling, and geometry tessellation.
#[wasm_bindgen]
pub fn prepare_map_tile(
    zoom: u8,
    dark_mode: bool,
    bytes: Vec<u8>,
) -> Result<js_sys::Uint8Array, JsValue> {
    let prepared = activity::map_runtime::prepare_tile_for_browser_worker(zoom, dark_mode, &bytes)
        .map_err(|error| js_error(&error))?;
    Ok(js_sys::Uint8Array::from(prepared.as_slice()))
}

/// Worker entry point for map-label shaping, collision placement, and tessellation.
#[wasm_bindgen]
pub fn prepare_map_labels(bytes: Vec<u8>) -> Result<js_sys::Uint8Array, JsValue> {
    let prepared = activity::map_runtime::prepare_labels_for_browser_worker(&bytes)
        .map_err(|error| js_error(&error))?;
    Ok(js_sys::Uint8Array::from(prepared.as_slice()))
}

/// Worker entry point for activity-route projection and segmentation.
#[wasm_bindgen]
pub fn prepare_map_route(bytes: Vec<u8>) -> Result<js_sys::Uint8Array, JsValue> {
    let prepared = activity::map_runtime::prepare_route_for_browser_worker(&bytes)
        .map_err(|error| js_error(&error))?;
    Ok(js_sys::Uint8Array::from(prepared.as_slice()))
}

fn fetch_map_tile(task: activity::map_runtime::TileTask) {
    let request = task.coordinates();
    spawn_local(async move {
        let result = fetch_map_tile_bytes(request).await;
        task.complete_encoded(result);
    });
}

async fn fetch_map_tile_bytes(
    request: activity::map_runtime::TileCoordinates,
) -> Result<Vec<u8>, String> {
    let path = format!("map/tiles/{}/{}/{}.pbf", request.zoom, request.x, request.y);
    let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
    let response = JsFuture::from(window.fetch_with_str(&path))
        .await
        .map_err(map_fetch_error)?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "map tile response had an unexpected type".to_owned())?;
    if !response.ok() {
        return Err(format!(
            "map tile request returned HTTP {}",
            response.status()
        ));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(map_fetch_error)?)
        .await
        .map_err(map_fetch_error)?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    if bytes.len() > MAX_MAP_TILE_BYTES {
        return Err("map tile exceeded the 2 MiB browser limit".to_owned());
    }
    Ok(bytes)
}

fn map_fetch_error(value: JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| "browser map request failed".to_owned())
}

enum BrowserMapTask {
    Tile(activity::map_runtime::TileTask),
    Labels {
        task: activity::map_runtime::BrowserLabelTask,
        request: Vec<u8>,
    },
    Route {
        task: activity::map_runtime::BrowserRouteTask,
        request: Vec<u8>,
    },
}

impl BrowserMapTask {
    const fn kind(&self) -> activity::map_runtime::BrowserWorkerTaskKind {
        match self {
            Self::Tile(_) => activity::map_runtime::BrowserWorkerTaskKind::Tile,
            Self::Labels { .. } => activity::map_runtime::BrowserWorkerTaskKind::Labels,
            Self::Route { .. } => activity::map_runtime::BrowserWorkerTaskKind::Route,
        }
    }

    fn post(&self, worker: &web_sys::Worker, id: u32) -> Result<(), JsValue> {
        let message = js_sys::Array::new();
        message.push(&JsValue::from_f64(f64::from(
            activity::map_runtime::BROWSER_WORKER_PROTOCOL_VERSION,
        )));
        message.push(&JsValue::from_str("task"));
        message.push(&JsValue::from_f64(f64::from(id)));
        message.push(&JsValue::from_str(self.kind().wire_name()));
        match self {
            Self::Tile(task) => {
                let request = task.coordinates();
                message.push(&JsValue::from_f64(f64::from(request.zoom)));
                message.push(&JsValue::from_f64(f64::from(request.x)));
                message.push(&JsValue::from_f64(f64::from(request.y)));
                message.push(&JsValue::from_bool(request.dark_mode()));
                worker.post_message(&message)
            }
            Self::Labels { request, .. } | Self::Route { request, .. } => {
                let request = js_sys::Uint8Array::from(request.as_slice());
                let buffer = request.buffer();
                message.push(&buffer);
                worker.post_message_with_transfer(&message, &js_sys::Array::of1(&buffer))
            }
        }
    }

    fn complete(self, result: Result<Vec<u8>, String>) {
        match self {
            Self::Tile(task) => task.complete_prepared(result),
            Self::Labels { task, .. } => task.complete(result),
            Self::Route { task, .. } => task.complete(result),
        }
    }

    fn fallback(self) {
        match self {
            Self::Tile(task) => fetch_map_tile(task),
            Self::Labels { task, request } => task.complete(
                activity::map_runtime::prepare_labels_for_browser_worker(&request),
            ),
            Self::Route { task, request } => task.complete(
                activity::map_runtime::prepare_route_for_browser_worker(&request),
            ),
        }
    }
}

struct BrowserMapWorkerShared {
    worker: web_sys::Worker,
    state: RefCell<activity::map_runtime::BrowserWorkerState<BrowserMapTask>>,
}

impl BrowserMapWorkerShared {
    fn submit(self: &Rc<Self>, task: BrowserMapTask) {
        let kind = task.kind();
        let replace_pending = matches!(
            kind,
            activity::map_runtime::BrowserWorkerTaskKind::Labels
                | activity::map_runtime::BrowserWorkerTaskKind::Route
        );
        let registration = self
            .state
            .borrow_mut()
            .register(kind, task, replace_pending);
        match registration {
            activity::map_runtime::BrowserWorkerRegistration::Fallback(task) => task.fallback(),
            activity::map_runtime::BrowserWorkerRegistration::Queued(_id) => {}
            activity::map_runtime::BrowserWorkerRegistration::Dispatch(id) => {
                self.dispatch(id);
            }
        }
    }

    fn dispatch(self: &Rc<Self>, id: u32) {
        let result = self
            .state
            .borrow()
            .task(id)
            .map_or(Ok(()), |task| task.post(&self.worker, id));
        if let Err(error) = result {
            self.transport_failure(format!(
                "could not dispatch browser map worker task: {}",
                js_reason(&error)
            ));
            return;
        }
        let shared = Rc::downgrade(self);
        spawn_local(async move {
            wait_milliseconds(MAP_WORKER_TASK_TIMEOUT_MILLISECONDS).await;
            if let Some(shared) = shared.upgrade() {
                shared.task_timeout(id);
            }
        });
    }

    fn handle_message(self: &Rc<Self>, value: JsValue) {
        let Some(message) = worker_message(value) else {
            self.transport_failure("browser map worker sent a malformed message".to_owned());
            return;
        };
        match message {
            BrowserWorkerMessage::Ready { version } => {
                let transition = self.state.borrow_mut().ready(version);
                self.apply_transition(
                    "browser map worker protocol version did not match".to_owned(),
                    transition,
                );
            }
            BrowserWorkerMessage::Fatal { version, reason } => {
                let transition = self.state.borrow_mut().fatal(version);
                self.apply_transition(reason, transition);
            }
            BrowserWorkerMessage::Result {
                version,
                id,
                kind,
                result,
            } => {
                let completion = self.state.borrow_mut().complete(version, id, kind);
                match completion {
                    activity::map_runtime::BrowserWorkerCompletion::Publish(task) => {
                        task.complete(result);
                    }
                    activity::map_runtime::BrowserWorkerCompletion::Stale => {}
                    activity::map_runtime::BrowserWorkerCompletion::Fallback(tasks) => {
                        self.enter_fallback(
                            "browser map worker response violated the protocol".to_owned(),
                            tasks,
                        );
                    }
                }
            }
        }
    }

    fn initialization_timeout(self: &Rc<Self>) {
        let transition = self.state.borrow_mut().initialization_timeout();
        self.apply_transition(
            "browser map worker initialization timed out".to_owned(),
            transition,
        );
    }

    fn task_timeout(self: &Rc<Self>, id: u32) {
        let transition = self.state.borrow_mut().task_timeout(id);
        self.apply_transition(
            format!("browser map worker task {id} timed out"),
            transition,
        );
    }

    fn transport_failure(self: &Rc<Self>, reason: String) {
        let transition = self.state.borrow_mut().transport_failure();
        self.apply_transition(reason, transition);
    }

    fn apply_transition(
        self: &Rc<Self>,
        reason: String,
        transition: activity::map_runtime::BrowserWorkerTransition<BrowserMapTask>,
    ) {
        match transition {
            activity::map_runtime::BrowserWorkerTransition::None => {}
            activity::map_runtime::BrowserWorkerTransition::Dispatch(ids) => {
                for id in ids {
                    self.dispatch(id);
                }
            }
            activity::map_runtime::BrowserWorkerTransition::Fallback(tasks) => {
                self.enter_fallback(reason, tasks);
            }
        }
    }

    fn enter_fallback(&self, reason: String, tasks: Vec<BrowserMapTask>) {
        tracing::warn!(%reason, "browser map worker failed; using local fallback");
        self.worker.terminate();
        for task in tasks {
            task.fallback();
        }
    }
}

enum BrowserWorkerMessage {
    Ready {
        version: u8,
    },
    Fatal {
        version: u8,
        reason: String,
    },
    Result {
        version: u8,
        id: u32,
        kind: activity::map_runtime::BrowserWorkerTaskKind,
        result: Result<Vec<u8>, String>,
    },
}

fn worker_message(value: JsValue) -> Option<BrowserWorkerMessage> {
    if !js_sys::Array::is_array(&value) {
        return None;
    }
    let message = js_sys::Array::from(&value);
    let version = wire_u8(&message.get(0))?;
    match message.get(1).as_string()?.as_str() {
        "ready" if message.length() == 2 => Some(BrowserWorkerMessage::Ready { version }),
        "fatal" if message.length() == 3 => Some(BrowserWorkerMessage::Fatal {
            version,
            reason: message.get(2).as_string()?,
        }),
        "result" if message.length() == 5 => {
            let buffer = message.get(4).dyn_into::<js_sys::ArrayBuffer>().ok()?;
            worker_result_message(
                version,
                &message,
                Ok(js_sys::Uint8Array::new(&buffer).to_vec()),
            )
        }
        "task-error" if message.length() == 5 => {
            worker_result_message(version, &message, Err(message.get(4).as_string()?))
        }
        _ => None,
    }
}

fn worker_result_message(
    version: u8,
    message: &js_sys::Array,
    result: Result<Vec<u8>, String>,
) -> Option<BrowserWorkerMessage> {
    Some(BrowserWorkerMessage::Result {
        version,
        id: wire_u32(&message.get(2))?,
        kind: activity::map_runtime::BrowserWorkerTaskKind::from_wire_name(
            &message.get(3).as_string()?,
        )?,
        result,
    })
}

fn wire_u8(value: &JsValue) -> Option<u8> {
    wire_u32(value).and_then(|value| u8::try_from(value).ok())
}

fn wire_u32(value: &JsValue) -> Option<u32> {
    let value = value.as_f64()?;
    (value.is_finite() && value.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&value))
        .then(|| value as u32)
}

struct BrowserMapWorker {
    shared: Rc<BrowserMapWorkerShared>,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::ErrorEvent)>,
}

impl BrowserMapWorker {
    fn new() -> Result<Self, JsValue> {
        let module_url = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| {
                document
                    .query_selector("link[rel=\"modulepreload\"]")
                    .ok()
                    .flatten()
            })
            .and_then(|link| link.get_attribute("href"))
            .ok_or_else(|| js_error("the browser application module URL is unavailable"))?;
        let worker = web_sys::Worker::new("map-worker.js")?;
        let shared = Rc::new(BrowserMapWorkerShared {
            worker,
            state: RefCell::new(activity::map_runtime::BrowserWorkerState::default()),
        });

        let message_shared = Rc::downgrade(&shared);
        let on_message = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            if let Some(shared) = message_shared.upgrade() {
                shared.handle_message(event.data());
            }
        }) as Box<dyn FnMut(_)>);
        shared
            .worker
            .set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        let error_shared = Rc::downgrade(&shared);
        let on_error = Closure::wrap(Box::new(move |event: web_sys::ErrorEvent| {
            if let Some(shared) = error_shared.upgrade() {
                shared.transport_failure(event.message());
            }
        }) as Box<dyn FnMut(_)>);
        shared
            .worker
            .set_onerror(Some(on_error.as_ref().unchecked_ref()));

        let initialization = js_sys::Array::new();
        initialization.push(&JsValue::from_f64(f64::from(
            activity::map_runtime::BROWSER_WORKER_PROTOCOL_VERSION,
        )));
        initialization.push(&JsValue::from_str("init"));
        initialization.push(&JsValue::from_str(&module_url));
        shared.worker.post_message(&initialization)?;

        let timeout_shared = Rc::downgrade(&shared);
        spawn_local(async move {
            wait_milliseconds(MAP_WORKER_INITIALIZATION_TIMEOUT_MILLISECONDS).await;
            if let Some(shared) = timeout_shared.upgrade() {
                shared.initialization_timeout();
            }
        });

        Ok(Self {
            shared,
            _on_message: on_message,
            _on_error: on_error,
        })
    }

    fn request(&self, task: activity::map_runtime::TileTask) {
        self.shared.submit(BrowserMapTask::Tile(task));
    }

    fn request_labels(&self, task: activity::map_runtime::BrowserLabelTask) {
        match task.request() {
            Ok(request) => self.shared.submit(BrowserMapTask::Labels { task, request }),
            Err(error) => task.complete(Err(error)),
        }
    }

    fn request_route(&self, task: activity::map_runtime::BrowserRouteTask) {
        match task.request() {
            Ok(request) => self.shared.submit(BrowserMapTask::Route { task, request }),
            Err(error) => task.complete(Err(error)),
        }
    }
}

impl Drop for BrowserMapWorker {
    fn drop(&mut self) {
        self.shared.worker.terminate();
    }
}

pub(super) enum BrowserMapBackend {
    Worker(BrowserMapWorker),
    Fallback,
}

impl BrowserMapBackend {
    pub(super) fn new() -> Self {
        match BrowserMapWorker::new() {
            Ok(worker) => Self::Worker(worker),
            Err(error) => {
                tracing::warn!(reason = %js_reason(&error), "browser map worker could not start; using local fallback");
                Self::Fallback
            }
        }
    }
}

impl activity::map_runtime::Backend for BrowserMapBackend {
    fn submit(&self, task: activity::map_runtime::TileTask) {
        match self {
            Self::Worker(worker) => worker.request(task),
            Self::Fallback => BrowserMapTask::Tile(task).fallback(),
        }
    }

    fn submit_labels(&self, task: activity::map_runtime::BrowserLabelTask) {
        match self {
            Self::Worker(worker) => worker.request_labels(task),
            Self::Fallback => match task.request() {
                Ok(request) => BrowserMapTask::Labels { task, request }.fallback(),
                Err(error) => task.complete(Err(error)),
            },
        }
    }

    fn submit_route(&self, task: activity::map_runtime::BrowserRouteTask) {
        match self {
            Self::Worker(worker) => worker.request_route(task),
            Self::Fallback => match task.request() {
                Ok(request) => BrowserMapTask::Route { task, request }.fallback(),
                Err(error) => task.complete(Err(error)),
            },
        }
    }
}

async fn wait_milliseconds(milliseconds: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let scheduled = web_sys::window().is_some_and(|window| {
            window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds)
                .is_ok()
        });
        if !scheduled {
            let _ignored = resolve.call0(&JsValue::UNDEFINED);
        }
    });
    let _ignored = JsFuture::from(promise).await;
}

fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

fn js_reason(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            value
                .dyn_ref::<js_sys::Error>()
                .map(|error| error.message().into())
        })
        .unwrap_or_else(|| "unknown browser error".to_owned())
}
