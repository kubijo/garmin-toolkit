//! Platform backend and per-surface completion boundary for activity maps.

use std::sync::Arc;

#[path = "map_runtime/browser_worker.rs"]
mod browser_worker;
#[path = "map_runtime/platform.rs"]
mod platform;

use platform::{ResponseQueue, SceneSlot, Shared, SurfaceRuntime};

use super::map::{
    MapTileDecoder, MapTileResponse, PreparedTile, TileStore,
    camera::{MapCamera, MapViewDemand},
};

pub use super::map::MapTileDecoder as TileDecoder;
pub use super::map::gpu_map::{
    BrowserLabelTask, BrowserRouteTask, BrowserTileLimits, BrowserTilePacket,
    BrowserTilePacketBuilder, BrowserTileSection, BrowserTileTransfer,
};
pub use super::map::gpu_map::{
    prepare_labels_for_browser_worker, prepare_route_for_browser_worker,
};
pub use super::map::prepare_tile_for_browser_worker;
pub use browser_worker::{
    BROWSER_WORKER_PROTOCOL_VERSION, BrowserWorkerCompletion, BrowserWorkerReceipt,
    BrowserWorkerRegistration, BrowserWorkerState, BrowserWorkerTaskKind, BrowserWorkerTransition,
};
pub use platform::Backend;

type SharedBackend = Shared<dyn Backend>;

/// One rendered activity-map frame, expressed as profiler-friendly measurements.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct MapPerformanceSample {
    /// Time since the previous rendered map frame.
    pub frame_milliseconds: Option<f32>,
    /// Time since the previous map frame when that frame requested continued camera motion.
    pub interaction_frame_milliseconds: Option<f32>,
    /// Whether dragging, zooming, or inertial camera movement affected this frame.
    pub camera_active: bool,
    /// CPU time spent in the map's immediate-mode UI pass.
    pub ui_milliseconds: f32,
    /// CPU time spent assembling the render-ready GPU frame.
    pub scene_milliseconds: f32,
    /// CPU time spent querying the route spatial index.
    pub route_query_microseconds: f32,
    /// CPU time spent preparing the current label layout.
    pub label_milliseconds: f32,
    /// Whether label preparation is waiting for worker completion.
    pub label_backlog: usize,
    /// Total superseded or stale asynchronous work discarded by this surface.
    pub stale_work: u64,
    /// Tiles intersecting the current viewport.
    pub visible_tiles: usize,
    /// Prepared tiles available to the scene.
    pub ready_tiles: usize,
    /// Tile requests awaiting completion.
    pub pending_tiles: usize,
    /// Bytes of prepared GPU data still awaiting upload.
    pub queued_upload_bytes: usize,
    /// Bytes uploaded during this frame.
    pub uploaded_bytes: usize,
}

/// CPU duration of one WGPU callback phase.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct MapRenderPerformanceSample {
    /// WGPU callback phase being measured.
    pub phase: MapRenderPhase,
    /// CPU time spent in that callback.
    pub milliseconds: f32,
}

/// Named WGPU callback phases emitted by the activity map.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MapRenderPhase {
    /// Resource preparation and queue writes before the render pass.
    Prepare,
    /// Render-pass command submission for tiles and routes.
    Draw,
}

/// Correlated browser upload event. Times are milliseconds since this upload was queued.
/// Queue visibility ends at publication; first draw is command encoding, not presentation.
#[derive(Clone, Debug, serde::Serialize)]
pub struct MapUploadEvent {
    pub version: u8,
    pub upload_id: usize,
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
    pub elapsed_ms: f64,
    pub event: MapUploadPhase,
    /// CPU time in allocation/write steps since the previous progress event.
    pub work_ms: f64,
    pub bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MapUploadPhase {
    Queued,
    Hidden,
    Visible,
    FirstWork,
    Progress,
    Published,
    FirstDraw,
    Released,
}

/// Host-owned destination for activity-map performance measurements.
pub trait MapMetricsSink: Send + Sync {
    /// Record one map frame without delaying rendering.
    fn record(&self, sample: MapPerformanceSample);

    /// Record one renderer callback without delaying submission.
    fn record_render(&self, _sample: MapRenderPerformanceSample) {}

    /// Wall time from enqueueing a tile for upload to GPU resource publication, not screen presentation.
    fn record_upload(&self, _milliseconds: f64) {}

    /// Record a bounded upload lifecycle event, without retaining it in the renderer.
    fn record_upload_event(&self, _event: MapUploadEvent) {}
}

struct DiscardMapMetrics;

impl MapMetricsSink for DiscardMapMetrics {
    fn record(&self, _sample: MapPerformanceSample) {}
}

#[derive(Clone)]
pub(super) struct MapMetrics(Arc<dyn MapMetricsSink>);

impl Default for MapMetrics {
    fn default() -> Self {
        Self::discard()
    }
}

impl MapMetrics {
    fn discard() -> Self {
        Self(Arc::new(DiscardMapMetrics))
    }

    pub(super) fn new(metrics: impl MapMetricsSink + 'static) -> Self {
        Self(Arc::new(metrics))
    }

    fn record(&self, sample: MapPerformanceSample) {
        self.0.record(sample);
    }

    pub(super) fn record_render(&self, sample: MapRenderPerformanceSample) {
        self.0.record_render(sample);
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) fn record_upload(&self, milliseconds: f64) {
        self.0.record_upload(milliseconds);
    }

    #[cfg(any(target_arch = "wasm32", test))]
    pub(super) fn record_upload_event(&self, event: MapUploadEvent) {
        self.0.record_upload_event(event);
    }
}

/// Coordinates and presentation state for one runtime-owned tile request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TileCoordinates {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
    dark_mode: bool,
    generation: u64,
}

impl TileCoordinates {
    pub(super) const fn new(zoom: u8, x: u32, y: u32, dark_mode: bool, generation: u64) -> Self {
        Self {
            zoom,
            x,
            y,
            dark_mode,
            generation,
        }
    }

    /// Whether the requested tile uses the dark vector-map style.
    #[must_use]
    pub const fn dark_mode(self) -> bool {
        self.dark_mode
    }

    pub(super) const fn generation(self) -> u64 {
        self.generation
    }
}

/// Cloneable runtime façade shared by activity and FIT-preview map surfaces.
#[derive(Clone)]
pub struct MapRuntimeHandle {
    backend: SharedBackend,
    metrics: MapMetrics,
    renderer: Renderer,
}

/// Rendering strategy paired with a map runtime.
#[derive(Clone)]
pub struct Renderer(Arc<dyn RendererFactory>);

trait RendererFactory: Send + Sync {
    fn painter(&self, metrics: MapMetrics) -> Box<dyn ScenePainter>;
    fn prepare_tile(
        &self,
        id: walkers::TileId,
        tile: walkers::Tile,
    ) -> Result<PreparedTile, String>;
    fn prepare_browser_tile(
        &self,
        packet: BrowserTilePacket,
    ) -> Result<Option<Arc<super::map::PreparedGpuTile>>, String>;
}

impl Renderer {
    /// Use the shared WGPU map renderer.
    #[must_use]
    pub fn wgpu(renderer: super::map::WgpuMapHandle) -> Self {
        Self(Arc::new(WgpuRenderer(renderer)))
    }

    /// Use the explicit software scene adapter in tests and gallery surfaces.
    #[doc(hidden)]
    #[must_use]
    pub fn software() -> Self {
        Self(Arc::new(SoftwareRenderer))
    }

    pub(super) fn prepare_tile(
        &self,
        id: walkers::TileId,
        tile: walkers::Tile,
    ) -> Result<PreparedTile, String> {
        self.0.prepare_tile(id, tile)
    }

    pub(super) fn prepare_browser_tile(
        &self,
        packet: BrowserTilePacket,
    ) -> Result<Option<Arc<super::map::PreparedGpuTile>>, String> {
        self.0.prepare_browser_tile(packet)
    }
}

struct WgpuRenderer(super::map::WgpuMapHandle);

impl RendererFactory for WgpuRenderer {
    fn painter(&self, metrics: MapMetrics) -> Box<dyn ScenePainter> {
        Box::new(super::map::gpu_map::GpuMap::new(&self.0, metrics))
    }

    fn prepare_tile(
        &self,
        id: walkers::TileId,
        tile: walkers::Tile,
    ) -> Result<PreparedTile, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = id;
            let gpu = super::map::gpu_map::prepare_local_browser_tile(tile)?.into_prepared();
            Ok(PreparedTile {
                tile: walkers::Tile::Vector {
                    shapes: Vec::new(),
                    texts: Vec::new(),
                },
                gpu,
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tile, gpu) = super::map::gpu_map::prepare_tile(&self.0, id, tile);
            Ok(PreparedTile { tile, gpu })
        }
    }

    fn prepare_browser_tile(
        &self,
        packet: BrowserTilePacket,
    ) -> Result<Option<Arc<super::map::PreparedGpuTile>>, String> {
        Ok(packet.into_prepared())
    }
}

struct SoftwareRenderer;

impl RendererFactory for SoftwareRenderer {
    fn painter(&self, _metrics: MapMetrics) -> Box<dyn ScenePainter> {
        Box::new(SoftwarePainter)
    }

    fn prepare_tile(
        &self,
        _id: walkers::TileId,
        tile: walkers::Tile,
    ) -> Result<PreparedTile, String> {
        Ok(PreparedTile { tile, gpu: None })
    }

    fn prepare_browser_tile(
        &self,
        _packet: BrowserTilePacket,
    ) -> Result<Option<Arc<super::map::PreparedGpuTile>>, String> {
        Err("browser map preparation requires the WGPU renderer".to_owned())
    }
}

trait ScenePainter {
    fn is_gpu(&self) -> bool;
    fn paint_callback(&self, rect: egui::Rect) -> Option<egui::Shape>;
    fn paint_labels(&mut self, frame: LabelFrame<'_>);
    fn update(&mut self, frame: SceneFrame<'_, '_>) -> super::map::gpu_map::ScenePerf;
}

#[derive(Clone, Copy)]
pub(super) struct LabelFrame<'a> {
    pub ui: &'a egui::Ui,
    pub scene: &'a MapScene,
    pub camera: &'a MapCamera,
    pub viewport: egui::Rect,
    pub backend: &'a dyn Backend,
}

#[derive(Clone, Copy)]
pub(super) struct SceneFrame<'frame, 'recording> {
    pub scene: &'frame MapScene,
    pub camera: &'frame MapCamera,
    pub viewport: egui::Rect,
    pub context: &'frame egui::Context,
    pub route: &'frame super::map::gpu_map::RouteScene<'recording>,
    pub backend: &'frame dyn Backend,
}

impl ScenePainter for super::map::gpu_map::GpuMap {
    fn is_gpu(&self) -> bool {
        true
    }

    fn paint_callback(&self, rect: egui::Rect) -> Option<egui::Shape> {
        Some(self.paint_callback(rect))
    }

    fn paint_labels(&mut self, frame: LabelFrame<'_>) {
        self.paint_labels(frame);
    }

    fn update(&mut self, frame: SceneFrame<'_, '_>) -> super::map::gpu_map::ScenePerf {
        self.update(frame)
    }
}

struct SoftwarePainter;

impl ScenePainter for SoftwarePainter {
    fn is_gpu(&self) -> bool {
        false
    }

    fn paint_callback(&self, _rect: egui::Rect) -> Option<egui::Shape> {
        None
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "software fallback transforms bounded f64 world coordinates into egui f32 points"
    )]
    fn paint_labels(&mut self, frame: LabelFrame<'_>) {
        let LabelFrame {
            ui,
            scene,
            camera,
            viewport,
            ..
        } = frame;
        let world_size = super::map::camera::world_size(camera.zoom());
        let center = camera.center_normalized();
        let painter = ui.painter().with_clip_rect(viewport);
        for (id, prepared) in scene.renderable_tiles() {
            let walkers::Tile::Vector { shapes, texts } = &prepared.tile else {
                continue;
            };
            let placement =
                super::map::camera::TilePlacement::new(*id, center, world_size, viewport);
            let scaling =
                (placement.size / f64::from(super::map::tile_store::SOURCE_TILE_SIZE)) as f32;
            for world in placement.copies(viewport) {
                let transform = egui::emath::TSTransform {
                    scaling,
                    translation: placement.origin(world).to_vec2(),
                };
                for mut shape in shapes.clone() {
                    shape.transform(transform);
                    preserve_stroke_width(&mut shape, scaling);
                    painter.add(shape);
                }
                for text in texts {
                    let position = text.position * scaling + transform.translation;
                    painter.text(
                        position,
                        egui::Align2::CENTER_CENTER,
                        &text.text,
                        egui::FontId::proportional(text.font_size),
                        text.text_color,
                    );
                }
            }
        }
    }

    fn update(&mut self, _frame: SceneFrame<'_, '_>) -> super::map::gpu_map::ScenePerf {
        super::map::gpu_map::ScenePerf::default()
    }
}

impl MapRuntimeHandle {
    /// Own a platform backend behind the shared map-runtime boundary.
    #[must_use]
    pub fn new(backend: impl Backend + 'static, renderer: Renderer) -> Self {
        Self {
            backend: Shared::new(backend),
            metrics: MapMetrics::discard(),
            renderer,
        }
    }

    /// Attach a host-owned performance sink to every map surface made by this runtime.
    #[must_use]
    pub fn with_metrics(mut self, metrics: impl MapMetricsSink + 'static) -> Self {
        self.metrics = MapMetrics::new(metrics);
        self
    }

    pub(super) fn surface(&self) -> MapSurfaceHandle {
        let renderer = self.renderer.clone();
        let backend = Shared::clone(&self.backend);
        let scene = SceneSlot::default();
        let driver = SurfaceDriver::new(Shared::clone(&backend), renderer.clone(), scene.clone());
        MapSurfaceHandle {
            backend,
            metrics: self.metrics.clone(),
            painter: renderer.0.painter(self.metrics.clone()),
            runtime: SurfaceRuntime::new(driver),
            scene,
        }
    }
}

pub(super) struct MapSurfaceHandle {
    backend: SharedBackend,
    metrics: MapMetrics,
    painter: Box<dyn ScenePainter>,
    runtime: SurfaceRuntime,
    scene: SceneSlot,
}

impl MapSurfaceHandle {
    pub(super) fn record_performance(&self, sample: MapPerformanceSample) {
        self.metrics.record(sample);
    }

    /// Replace any unstarted view demand.
    pub(super) fn submit_view(
        &mut self,
        context: &egui::Context,
        dark_mode: bool,
        demand: &MapViewDemand,
    ) {
        self.runtime.submit(SurfaceView {
            context: context.clone(),
            dark_mode,
            demand: Some(*demand),
        });
    }

    /// Let the runtime process completions without creating tile demand.
    pub(super) fn poll(&mut self, context: &egui::Context, dark_mode: bool) {
        self.runtime.submit(SurfaceView {
            context: context.clone(),
            dark_mode,
            demand: None,
        });
    }

    pub(super) fn gpu_enabled(&self) -> bool {
        self.painter.is_gpu()
    }

    pub(super) fn paint_callback(&self, rect: egui::Rect) -> Option<egui::Shape> {
        self.painter.paint_callback(rect)
    }

    pub(super) fn paint_labels(
        &mut self,
        scene: &MapScene,
        ui: &egui::Ui,
        camera: &MapCamera,
        viewport: egui::Rect,
    ) {
        self.painter.paint_labels(LabelFrame {
            ui,
            scene,
            camera,
            viewport,
            backend: self.backend.as_ref(),
        });
    }

    pub(super) fn update_scene(
        &mut self,
        scene: &MapScene,
        camera: &MapCamera,
        viewport: egui::Rect,
        context: &egui::Context,
        route: &super::map::gpu_map::RouteScene<'_>,
    ) -> super::map::gpu_map::ScenePerf {
        self.painter.update(SceneFrame {
            scene,
            camera,
            viewport,
            context,
            route,
            backend: self.backend.as_ref(),
        })
    }

    pub(super) fn scene(&self) -> MapScene {
        MapScene(self.scene.load())
    }

    pub(super) fn discarded_views(&self) -> u64 {
        self.runtime.discarded()
    }

    #[cfg(test)]
    fn synchronize(&self) {
        self.runtime.synchronize();
    }
}

fn preserve_stroke_width(shape: &mut egui::Shape, scaling: f32) {
    if scaling == 0.0 {
        return;
    }
    match shape {
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                preserve_stroke_width(shape, scaling);
            }
        }
        egui::Shape::Path(path) => path.stroke.width /= scaling,
        egui::Shape::LineSegment { stroke, .. } => stroke.width /= scaling,
        egui::Shape::Circle(circle) => circle.stroke.width /= scaling,
        egui::Shape::Ellipse(ellipse) => ellipse.stroke.width /= scaling,
        egui::Shape::Rect(rect) => rect.stroke.width /= scaling,
        egui::Shape::QuadraticBezier(curve) => curve.stroke.width /= scaling,
        egui::Shape::CubicBezier(curve) => curve.stroke.width /= scaling,
        egui::Shape::Noop
        | egui::Shape::Text(_)
        | egui::Shape::Mesh(_)
        | egui::Shape::Callback(_) => {}
    }
}

#[derive(Default)]
struct MapSceneData {
    tiles: Box<[SceneTile]>,
    background_failure: Option<String>,
    ready_tiles: usize,
    pending_tiles: usize,
    pending_visible_tiles: usize,
}

impl MapSceneData {
    fn capture(tiles: &TileStore) -> Self {
        let mut renderable = tiles
            .renderable_tiles()
            .map(|(id, tile)| SceneTile {
                id: *id,
                tile: Arc::clone(tile),
            })
            .collect::<Vec<_>>();
        renderable.sort_unstable_by_key(|tile| (tile.id.zoom, tile.id.y, tile.id.x));
        Self {
            tiles: renderable.into_boxed_slice(),
            background_failure: tiles.visible_background_failure(),
            ready_tiles: tiles.ready_len(),
            pending_tiles: tiles.pending_len(),
            pending_visible_tiles: tiles.pending_visible_len(),
        }
    }
}

struct SceneTile {
    id: walkers::TileId,
    tile: Arc<PreparedTile>,
}

struct PublishedScene {
    slot: SceneSlot,
    revision: u64,
}

impl PublishedScene {
    fn new(slot: SceneSlot) -> Self {
        Self { slot, revision: 0 }
    }

    fn publish(&mut self, tiles: &TileStore) -> bool {
        let revision = tiles.scene_revision();
        if revision == self.revision {
            return false;
        }
        self.slot.publish(MapSceneData::capture(tiles));
        self.revision = revision;
        true
    }
}

pub(in crate::activity::map_runtime) struct SurfaceView {
    context: egui::Context,
    dark_mode: bool,
    demand: Option<MapViewDemand>,
}

pub(in crate::activity::map_runtime) struct SurfaceDriver {
    backend: SharedBackend,
    renderer: Renderer,
    responses: ResponseQueue,
    scene: PublishedScene,
    tiles: TileStore,
}

impl SurfaceDriver {
    fn new(backend: SharedBackend, renderer: Renderer, scene: SceneSlot) -> Self {
        Self {
            backend,
            renderer,
            responses: ResponseQueue::default(),
            scene: PublishedScene::new(scene),
            tiles: TileStore::default(),
        }
    }

    pub(in crate::activity::map_runtime) fn update(&mut self, view: &SurfaceView) {
        for response in self.responses.drain() {
            self.tiles.resolve(&view.context, response);
        }
        self.tiles.set_theme(view.dark_mode);
        if let Some(demand) = view.demand {
            self.tiles.apply_demand(&demand);
            self.tiles.schedule_requests();
            for request in self.tiles.take_requests() {
                self.backend.submit(TileTask {
                    request,
                    renderer: self.renderer.clone(),
                    responses: self.responses.clone(),
                    context: view.context.clone(),
                });
            }
        }

        if self.scene.publish(&self.tiles) {
            view.context.request_repaint();
        }
    }
}

/// Immutable tile-scene snapshot acquired once for a complete UI frame.
#[derive(Clone)]
pub(super) struct MapScene(Arc<MapSceneData>);

impl MapScene {
    pub(super) fn renderable_tiles(
        &self,
    ) -> impl Iterator<Item = (&walkers::TileId, &PreparedTile)> {
        self.0
            .tiles
            .iter()
            .map(|tile| (&tile.id, tile.tile.as_ref()))
    }

    pub(super) fn renderable_gpu_tiles(
        &self,
    ) -> impl Iterator<
        Item = (
            &walkers::TileId,
            &std::sync::Arc<super::map::PreparedGpuTile>,
        ),
    > {
        self.0
            .tiles
            .iter()
            .filter_map(|tile| tile.tile.gpu.as_ref().map(|gpu| (&tile.id, gpu)))
    }

    pub(super) fn background_failure(&self) -> Option<&str> {
        self.0.background_failure.as_deref()
    }

    pub(super) fn ready_tiles(&self) -> usize {
        self.0.ready_tiles
    }

    pub(super) fn pending_tiles(&self) -> usize {
        self.0.pending_tiles
    }

    pub(super) fn pending_visible_tiles(&self) -> usize {
        self.0.pending_visible_tiles
    }

    #[cfg(test)]
    fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// One runtime-owned tile operation. Completing it publishes only to its originating surface.
pub struct TileTask {
    request: TileCoordinates,
    renderer: Renderer,
    responses: ResponseQueue,
    context: egui::Context,
}

impl TileTask {
    /// Return the requested XYZ coordinates and map theme.
    #[must_use]
    pub const fn coordinates(&self) -> TileCoordinates {
        self.request
    }

    /// Decode and publish encoded MVT bytes on the caller's worker thread.
    pub fn complete_decoded(self, decoder: &MapTileDecoder, result: Result<Vec<u8>, String>) {
        let response = decoder.decode(self.request, result, &self.renderer);
        self.complete(response);
    }

    /// Publish encoded bytes for lightweight or deterministic backends.
    pub fn complete_encoded(self, result: Result<Vec<u8>, String>) {
        self.complete_decoded(&MapTileDecoder::default(), result);
    }

    /// Publish a tile prepared by the browser worker.
    pub fn complete_prepared(self, result: Result<BrowserTilePacket, String>) {
        let response = MapTileResponse::prepared(self.request, result, &self.renderer);
        self.complete(response);
    }

    fn complete(self, response: MapTileResponse) {
        self.responses.push(response);
        self.context.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct RecordingBackend {
        submitted: Arc<Mutex<Vec<TileCoordinates>>>,
    }

    impl Backend for RecordingBackend {
        fn submit(&self, task: TileTask) {
            self.submitted.lock().unwrap().push(task.coordinates());
        }
    }

    struct CompletingBackend;

    impl Backend for CompletingBackend {
        fn submit(&self, task: TileTask) {
            task.complete_encoded(Ok(Vec::new()));
        }
    }

    struct FailingBackend;

    impl Backend for FailingBackend {
        fn submit(&self, task: TileTask) {
            task.complete_encoded(Err("fixture failure".to_owned()));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Clone, Default)]
    struct ThreadRecordingBackend {
        submitted_on: Arc<Mutex<Option<std::thread::ThreadId>>>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl Backend for ThreadRecordingBackend {
        fn submit(&self, _task: TileTask) {
            *self.submitted_on.lock().unwrap() = Some(std::thread::current().id());
        }
    }

    fn demand(ids: impl IntoIterator<Item = walkers::TileId>) -> MapViewDemand {
        let visible = ids.into_iter().collect::<Vec<_>>();
        MapViewDemand::from_tiles(&visible)
    }

    #[test]
    fn surface_dispatches_center_tiles_first() {
        let backend = RecordingBackend::default();
        let submitted = Arc::clone(&backend.submitted);
        let runtime = MapRuntimeHandle::new(backend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();
        surface.submit_view(
            &context,
            true,
            &demand((0..8).map(|x| walkers::TileId { zoom: 4, x, y: 6 })),
        );
        surface.synchronize();

        let requests = submitted
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 6);
        assert_eq!([requests[0].x, requests[1].x], [4, 3]);
        assert!(!requests.iter().any(|request| request.x == 0));
        assert!(!requests.iter().any(|request| request.x == 7));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_surface_driver_dispatches_away_from_the_ui_thread() {
        let backend = ThreadRecordingBackend::default();
        let submitted_on = Arc::clone(&backend.submitted_on);
        let runtime = MapRuntimeHandle::new(backend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();
        let ui_thread = std::thread::current().id();

        surface.submit_view(
            &context,
            true,
            &demand([walkers::TileId {
                zoom: 4,
                x: 8,
                y: 6,
            }]),
        );
        surface.synchronize();

        let worker_thread = submitted_on.lock().unwrap().expect("tile was dispatched");
        assert_ne!(worker_thread, ui_thread);
    }

    #[test]
    fn completions_publish_only_at_the_next_surface_boundary() {
        let runtime = MapRuntimeHandle::new(CompletingBackend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();
        let current = demand([walkers::TileId {
            zoom: 4,
            x: 8,
            y: 6,
        }]);
        surface.submit_view(&context, true, &current);
        surface.synchronize();
        assert!(surface.scene().pending_tiles() > 0);

        surface.submit_view(&context, true, &current);
        surface.synchronize();
        assert!(surface.scene().pending_tiles() > 0);
        surface.submit_view(&context, true, &current);
        surface.synchronize();
        assert_eq!(surface.scene().pending_tiles(), 0);
    }

    #[test]
    fn acquired_scene_remains_stable_after_a_later_publication() {
        let runtime = MapRuntimeHandle::new(FailingBackend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();
        let current = demand([walkers::TileId {
            zoom: 4,
            x: 8,
            y: 6,
        }]);

        surface.submit_view(&context, true, &current);
        surface.synchronize();
        let before_failure = surface.scene();
        surface.submit_view(&context, true, &current);
        surface.synchronize();
        let after_failure = surface.scene();

        assert!(before_failure.background_failure().is_none());
        assert_eq!(
            after_failure.background_failure(),
            Some("Tile 4/8/6: fixture failure")
        );
        assert!(before_failure.background_failure().is_none());
    }

    #[test]
    fn unchanged_demand_reuses_the_published_scene() {
        let runtime = MapRuntimeHandle::new(RecordingBackend::default(), Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();
        let current = demand([walkers::TileId {
            zoom: 4,
            x: 8,
            y: 6,
        }]);

        surface.submit_view(&context, true, &current);
        surface.synchronize();
        let first = surface.scene();
        surface.submit_view(&context, true, &current);
        surface.synchronize();
        let second = surface.scene();

        assert!(first.ptr_eq(&second));
    }

    #[test]
    fn polling_without_demand_does_not_dispatch_work() {
        let backend = RecordingBackend::default();
        let submitted = Arc::clone(&backend.submitted);
        let runtime = MapRuntimeHandle::new(backend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();

        surface.poll(&context, true);
        surface.synchronize();

        assert!(submitted.lock().unwrap().is_empty());
    }
}
