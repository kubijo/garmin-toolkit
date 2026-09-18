//! Browser map worker transport and local fallback.

use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use garmin_ui::activity;
use wasm_bindgen::{JsCast as _, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};

mod admission;
mod codec;
use admission::Step as BrowserTileAdmissionStep;

const MAP_WORKER_INITIALIZATION_TIMEOUT_MILLISECONDS: i32 = 10_000;
const MAP_WORKER_TASK_TIMEOUT_MILLISECONDS: i32 = 20_000;

/// Authoritative worker protocol version used to reject mixed browser assets.
#[wasm_bindgen]
pub fn browser_worker_protocol_version() -> u8 {
    activity::map_runtime::BROWSER_WORKER_PROTOCOL_VERSION
}

/// Worker entry point for MVT parsing, styling, and geometry tessellation.
#[wasm_bindgen]
pub fn prepare_map_tile(
    zoom: u8,
    dark_mode: bool,
    bytes: &js_sys::Uint8Array,
) -> Result<js_sys::Array, JsValue> {
    activity::map_runtime::BrowserTileLimits::check_encoded_bytes(bytes.length() as usize)
        .map_err(|error| js_error(&error))?;
    let bytes = bytes.to_vec();
    let prepared = activity::map_runtime::prepare_tile_for_browser_worker(zoom, dark_mode, &bytes)
        .map_err(|error| js_error(&error))?;
    let parts = js_sys::Array::new();
    for part in prepared.parts() {
        parts.push(&js_sys::Uint8Array::from(part));
    }
    Ok(parts)
}

/// Worker entry point for map-label shaping, collision placement, and tessellation.
#[wasm_bindgen]
pub fn prepare_map_labels(bytes: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
    let prepared = activity::map_runtime::prepare_labels_for_browser_worker(bytes)
        .map_err(|error| js_error(&error))?;
    check_data_result(
        activity::map_runtime::BrowserWorkerTaskKind::Labels,
        prepared.len(),
    )?;
    Ok(js_sys::Uint8Array::from(prepared.as_slice()))
}

/// Worker entry point for activity-route projection and segmentation.
#[wasm_bindgen]
pub fn prepare_map_route(bytes: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
    let prepared = activity::map_runtime::prepare_route_for_browser_worker(bytes)
        .map_err(|error| js_error(&error))?;
    check_data_result(
        activity::map_runtime::BrowserWorkerTaskKind::Route,
        prepared.len(),
    )?;
    Ok(js_sys::Uint8Array::from(prepared.as_slice()))
}

fn check_data_result(
    kind: activity::map_runtime::BrowserWorkerTaskKind,
    bytes: usize,
) -> Result<(), JsValue> {
    if bytes > kind.result_byte_limit() {
        return Err(js_error("prepared map data exceeded its transfer limit"));
    }
    Ok(())
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
    let bytes = js_sys::Uint8Array::new(&buffer);
    activity::map_runtime::BrowserTileLimits::check_encoded_bytes(bytes.length() as usize)?;
    Ok(bytes.to_vec())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a Result::map_err callback"
)]
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
        let version = activity::map_runtime::BROWSER_WORKER_PROTOCOL_VERSION;
        match self {
            Self::Tile(task) => {
                let request = task.coordinates();
                let message = codec::tile_task(
                    version,
                    id,
                    request.zoom,
                    request.x,
                    request.y,
                    request.dark_mode(),
                )?;
                worker.post_message(&message)
            }
            Self::Labels { request, .. } | Self::Route { request, .. } => {
                let request = js_sys::Uint8Array::from(request.as_slice());
                let buffer = request.buffer();
                let message = codec::data_task(version, id, self.kind().wire_name(), &buffer)?;
                worker.post_message_with_transfer(&message, &js_sys::Array::of1(&buffer))
            }
        }
    }

    fn complete_data(self, result: Result<Vec<u8>, String>) {
        match self {
            Self::Tile(task) => task.complete_prepared(Err(
                "browser map worker returned encoded data for a tile task".to_owned(),
            )),
            Self::Labels { task, .. } => task.complete(result),
            Self::Route { task, .. } => task.complete(result),
        }
    }

    fn complete_error(self, error: String) {
        match self {
            Self::Tile(task) => {
                let coordinates = task.coordinates();
                web_sys::console::warn_1(
                    &format!(
                        "Map tile {}/{}/{} failed: {error}",
                        coordinates.zoom, coordinates.x, coordinates.y
                    )
                    .into(),
                );
                task.complete_prepared(Err(error));
            }
            Self::Labels { task, .. } => task.complete(Err(error)),
            Self::Route { task, .. } => task.complete(Err(error)),
        }
    }

    fn fallback(self) {
        crate::browser_timing::measure_duration("garmin.map.worker-fallback", 0.0);
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

struct BrowserTileBuffers {
    vertices: js_sys::Uint8Array,
    indices: js_sys::Uint8Array,
    text_records: js_sys::Uint8Array,
    strings: js_sys::Uint8Array,
}

impl BrowserTileBuffers {
    fn lengths(&self) -> [usize; 4] {
        [
            self.vertices.length() as usize,
            self.indices.length() as usize,
            self.text_records.length() as usize,
            self.strings.length() as usize,
        ]
    }

    fn source(&self, section: activity::map_runtime::BrowserTileSection) -> &js_sys::Uint8Array {
        match section {
            activity::map_runtime::BrowserTileSection::Vertices => &self.vertices,
            activity::map_runtime::BrowserTileSection::Indices => &self.indices,
            activity::map_runtime::BrowserTileSection::TextRecords => &self.text_records,
            activity::map_runtime::BrowserTileSection::Strings => &self.strings,
        }
    }
}

struct PendingBrowserTile {
    task: activity::map_runtime::TileTask,
    builder: activity::map_runtime::BrowserTilePacketBuilder,
    sources: BrowserTileBuffers,
    offsets: BrowserTileOffsets,
    queued_at: f64,
    first_step: bool,
}

#[derive(Default)]
struct BrowserTileOffsets {
    vertices: usize,
    indices: usize,
    text_records: usize,
    strings: usize,
}

impl BrowserTileOffsets {
    fn get_mut(&mut self, section: activity::map_runtime::BrowserTileSection) -> &mut usize {
        match section {
            activity::map_runtime::BrowserTileSection::Vertices => &mut self.vertices,
            activity::map_runtime::BrowserTileSection::Indices => &mut self.indices,
            activity::map_runtime::BrowserTileSection::TextRecords => &mut self.text_records,
            activity::map_runtime::BrowserTileSection::Strings => &mut self.strings,
        }
    }
}

impl PendingBrowserTile {
    fn new(
        task: activity::map_runtime::TileTask,
        sources: BrowserTileBuffers,
    ) -> Result<Self, (activity::map_runtime::TileTask, String)> {
        let [vertices, indices, text_records, strings] = sources.lengths();
        let builder = match activity::map_runtime::BrowserTilePacketBuilder::new(
            vertices,
            indices,
            text_records,
            strings,
        ) {
            Ok(builder) => builder,
            Err(error) => return Err((task, error)),
        };
        Ok(Self {
            task,
            builder,
            sources,
            offsets: BrowserTileOffsets::default(),
            queued_at: crate::browser_timing::now(),
            first_step: true,
        })
    }

    fn is_complete(&self) -> bool {
        self.builder.next_section().is_none()
    }

    fn advance(
        &mut self,
        maximum_bytes: usize,
        scratch: &mut Vec<u8>,
    ) -> Result<BrowserTileAdmissionStep, String> {
        let started = crate::browser_timing::now();
        if std::mem::take(&mut self.first_step) {
            crate::browser_timing::measure_duration(
                "garmin.map.tile-admission-wait",
                started - self.queued_at,
            );
        }
        let allocation = self.builder.allocate_next();
        if !matches!(allocation, Ok(None)) {
            crate::browser_timing::measure_duration(
                "garmin.map.tile-allocation",
                crate::browser_timing::now() - started,
            );
            allocation?;
            return Ok(BrowserTileAdmissionStep::Allocated);
        }
        let Some((section, length)) = self.builder.next_chunk(maximum_bytes) else {
            return Ok(BrowserTileAdmissionStep::Deferred);
        };
        let offset = self.offsets.get_mut(section);
        let start = *offset;
        let end = start
            .checked_add(length)
            .ok_or_else(|| "browser tile admission offset overflowed".to_owned())?;
        let source = self.sources.source(section);
        if end > source.length() as usize {
            return Err("browser tile admission exceeded a transferred buffer".to_owned());
        }
        let chunk = source.subarray(
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(end).unwrap_or(u32::MAX),
        );
        let text_started = (section == activity::map_runtime::BrowserTileSection::TextRecords)
            .then(crate::browser_timing::now);
        if text_started.is_some() {
            scratch.resize(length, 0);
            chunk.copy_to(scratch);
            self.builder.append(section, scratch)?;
        } else {
            self.builder
                .append_from(section, length, |destination| chunk.copy_to(destination))?;
        }
        *offset = end;
        Ok(BrowserTileAdmissionStep::Copied {
            bytes: length,
            text_milliseconds: text_started
                .map_or(0.0, |started| crate::browser_timing::now() - started),
        })
    }

    fn finish(
        self,
    ) -> (
        activity::map_runtime::TileTask,
        Result<activity::map_runtime::BrowserTilePacket, String>,
    ) {
        let result = self.builder.finish();
        if result.is_ok() {
            crate::browser_timing::measure_duration(
                "garmin.map.tile-admission-latency",
                crate::browser_timing::now() - self.queued_at,
            );
        }
        (self.task, result)
    }
}

type BrowserTileAdmission = admission::Admission<PendingBrowserTile>;

impl admission::Work for PendingBrowserTile {
    fn retained_bytes(&self) -> usize {
        self.builder.retained_bytes()
    }
    fn is_complete(&self) -> bool {
        self.is_complete()
    }
    fn advance(
        &mut self,
        maximum: usize,
        scratch: &mut Vec<u8>,
    ) -> Result<admission::Step, String> {
        self.advance(maximum, scratch)
    }
}

struct BrowserMapWorkerShared {
    worker: web_sys::Worker,
    state: RefCell<activity::map_runtime::BrowserWorkerState<BrowserMapTask>>,
    admission: RefCell<BrowserTileAdmission>,
    data: RefCell<VecDeque<PendingBrowserData>>,
}

struct PendingBrowserData {
    version: u8,
    id: u32,
    kind: activity::map_runtime::BrowserWorkerTaskKind,
    bytes: js_sys::Uint8Array,
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
            self.transport_failure(&format!(
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
            self.transport_failure("browser map worker sent a malformed message");
            return;
        };
        match message {
            BrowserWorkerMessage::Ready { version } => {
                let transition = self.state.borrow_mut().ready(version);
                self.apply_transition(
                    "browser map worker protocol version did not match",
                    transition,
                );
            }
            BrowserWorkerMessage::Fatal { version, reason } => {
                let transition = self.state.borrow_mut().fatal(version);
                self.apply_transition(&reason, transition);
            }
            BrowserWorkerMessage::Result {
                version,
                id,
                kind,
                result,
            } => {
                if let Ok(BrowserWorkerResult::Data(bytes)) = &result
                    && bytes.length() as usize <= kind.result_byte_limit()
                {
                    let receipt = self.state.borrow_mut().receive(version, id, kind);
                    match receipt {
                        activity::map_runtime::BrowserWorkerReceipt::Accepted => {}
                        activity::map_runtime::BrowserWorkerReceipt::Stale => return,
                        activity::map_runtime::BrowserWorkerReceipt::Fallback(tasks) => {
                            self.enter_fallback(
                                "browser map worker response violated the protocol",
                                tasks,
                            );
                            return;
                        }
                    }
                    let mut data = self.data.borrow_mut();
                    data.retain(|pending| pending.kind != kind);
                    data.push_back(PendingBrowserData {
                        version,
                        id,
                        kind,
                        bytes: bytes.clone(),
                    });
                    drop(data);
                    self.schedule_admission();
                    return;
                }
                let completion = self.state.borrow_mut().complete(version, id, kind);
                match completion {
                    activity::map_runtime::BrowserWorkerCompletion::Publish(task) => {
                        self.publish_worker_result(task, result);
                    }
                    activity::map_runtime::BrowserWorkerCompletion::Stale => {}
                    activity::map_runtime::BrowserWorkerCompletion::Fallback(tasks) => {
                        self.enter_fallback(
                            "browser map worker response violated the protocol",
                            tasks,
                        );
                    }
                }
            }
        }
    }

    fn publish_worker_result(
        self: &Rc<Self>,
        task: BrowserMapTask,
        result: Result<BrowserWorkerResult, String>,
    ) {
        match (task, result) {
            (BrowserMapTask::Tile(task), Ok(BrowserWorkerResult::Tile(buffers))) => {
                let pending = match PendingBrowserTile::new(task, buffers) {
                    Ok(pending) => pending,
                    Err((task, error)) => {
                        BrowserMapTask::Tile(task).complete_error(error);
                        return;
                    }
                };
                if let Err(pending) = self.admission.borrow_mut().enqueue(pending) {
                    let task = pending.task;
                    let error =
                        "browser tile admission queue exceeded its retention limit".to_owned();
                    BrowserMapTask::Tile(task).complete_error(error);
                    return;
                }
                self.schedule_admission();
            }
            (
                task @ (BrowserMapTask::Labels { .. } | BrowserMapTask::Route { .. }),
                Ok(BrowserWorkerResult::Data(_)),
            ) => {
                // Valid data responses are admitted on the animation clock above.
                task.complete_error(
                    "browser map worker data exceeded its transfer limit".to_owned(),
                );
            }
            (task, Err(error)) => task.complete_error(error),
            (task, Ok(_)) => {
                task.fallback();
                self.transport_failure("browser map worker returned the wrong payload for a task");
            }
        }
    }

    fn schedule_admission(self: &Rc<Self>) {
        if !self
            .admission
            .borrow_mut()
            .schedule(!self.data.borrow().is_empty())
        {
            return;
        }
        let shared = Rc::downgrade(self);
        spawn_local(async move {
            next_animation_frame().await;
            if let Some(shared) = shared.upgrade() {
                shared.advance_admission();
            }
        });
    }

    fn advance_admission(self: &Rc<Self>) {
        let started = crate::browser_timing::now();
        let pending = self.data.borrow_mut().pop_front();
        if let Some(pending) = pending {
            let completion =
                self.state
                    .borrow_mut()
                    .complete(pending.version, pending.id, pending.kind);
            if let activity::map_runtime::BrowserWorkerCompletion::Publish(task) = completion {
                task.complete_data(Ok(pending.bytes.to_vec()));
                crate::browser_timing::measure_duration(
                    "garmin.map.data-admission",
                    crate::browser_timing::now() - started,
                );
            }
        }
        let tile_started = crate::browser_timing::now();
        let frame = self
            .admission
            .borrow_mut()
            .advance_frame(started, crate::browser_timing::now);
        if frame.text_milliseconds > 0.0 {
            crate::browser_timing::measure_duration(
                "garmin.map.tile-text-reconstruction",
                frame.text_milliseconds,
            );
        }
        let tile_work = frame.did_work;
        for (pending, result) in frame.completed {
            match result {
                Ok(()) => {
                    let (task, result) = pending.finish();
                    match result {
                        Ok(packet) => task.complete_prepared(Ok(packet)),
                        Err(error) => BrowserMapTask::Tile(task).complete_error(error),
                    }
                }
                Err(error) => BrowserMapTask::Tile(pending.task).complete_error(error),
            }
        }
        if tile_work {
            crate::browser_timing::measure_duration(
                "garmin.map.tile-admission",
                crate::browser_timing::now() - tile_started,
            );
        }
        self.schedule_admission();
    }

    fn initialization_timeout(self: &Rc<Self>) {
        let transition = self.state.borrow_mut().initialization_timeout();
        self.apply_transition("browser map worker initialization timed out", transition);
    }

    fn task_timeout(self: &Rc<Self>, id: u32) {
        let transition = self.state.borrow_mut().task_timeout(id);
        self.apply_transition(
            &format!("browser map worker task {id} timed out"),
            transition,
        );
    }

    fn transport_failure(self: &Rc<Self>, reason: &str) {
        let transition = self.state.borrow_mut().transport_failure();
        self.apply_transition(reason, transition);
    }

    fn apply_transition(
        self: &Rc<Self>,
        reason: &str,
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

    fn enter_fallback(&self, reason: &str, tasks: Vec<BrowserMapTask>) {
        tracing::warn!(%reason, "browser map worker failed; using local fallback");
        self.worker.terminate();
        self.data.borrow_mut().clear();
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
        result: Result<BrowserWorkerResult, String>,
    },
}

enum BrowserWorkerResult {
    Tile(BrowserTileBuffers),
    Data(js_sys::Uint8Array),
}

fn worker_message(value: JsValue) -> Option<BrowserWorkerMessage> {
    let message = codec::decode(value).ok()?;
    let version = message.version();
    match message.message_type().as_str() {
        "ready" => Some(BrowserWorkerMessage::Ready { version }),
        "fatal" => Some(BrowserWorkerMessage::Fatal {
            version,
            reason: message.reason(),
        }),
        "result" | "task-error" => {
            let kind =
                activity::map_runtime::BrowserWorkerTaskKind::from_wire_name(&message.kind())?;
            let result = if message.message_type() == "task-error" {
                Err(message.reason())
            } else {
                let buffers = message.buffers();
                let buffer = |index| js_sys::Uint8Array::new(&buffers.get(index));
                Ok(match kind {
                    activity::map_runtime::BrowserWorkerTaskKind::Tile => {
                        BrowserWorkerResult::Tile(BrowserTileBuffers {
                            vertices: buffer(0),
                            indices: buffer(1),
                            text_records: buffer(2),
                            strings: buffer(3),
                        })
                    }
                    _ => BrowserWorkerResult::Data(buffer(0)),
                })
            };
            Some(BrowserWorkerMessage::Result {
                version,
                id: message.id(),
                kind,
                result,
            })
        }
        _ => None,
    }
}

pub(super) struct BrowserMapWorker {
    shared: Rc<BrowserMapWorkerShared>,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::ErrorEvent)>,
}

impl BrowserMapWorker {
    fn new() -> Result<Self, JsValue> {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| js_error("the browser document is unavailable"))?;
        let asset_url = |selector: &str| -> Result<String, JsValue> {
            document
                .query_selector(selector)?
                .and_then(|link| link.dyn_into::<web_sys::HtmlLinkElement>().ok())
                .map(|link| link.href())
                .filter(|href| !href.is_empty())
                .ok_or_else(|| {
                    js_error(&format!("browser application asset is missing: {selector}"))
                })
        };
        let module_url = asset_url("link[rel=\"modulepreload\"]")?;
        let wasm_url = asset_url("link[rel=\"preload\"][as=\"fetch\"][type=\"application/wasm\"]")?;
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        let worker = web_sys::Worker::new_with_options("map-worker.js", &options)?;
        let shared = Rc::new(BrowserMapWorkerShared {
            worker,
            state: RefCell::new(activity::map_runtime::BrowserWorkerState::default()),
            admission: RefCell::new(BrowserTileAdmission::default()),
            data: RefCell::new(VecDeque::new()),
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
                shared.transport_failure(&event.message());
            }
        }) as Box<dyn FnMut(_)>);
        shared
            .worker
            .set_onerror(Some(on_error.as_ref().unchecked_ref()));

        let initialization = codec::init_message(
            activity::map_runtime::BROWSER_WORKER_PROTOCOL_VERSION,
            &module_url,
            &wasm_url,
        )?;
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

async fn next_animation_frame() {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let scheduled = web_sys::window().is_some_and(|window| {
            window
                .request_animation_frame(resolve.unchecked_ref())
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
