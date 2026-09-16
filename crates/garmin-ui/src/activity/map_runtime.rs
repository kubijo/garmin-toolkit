//! Platform backend and per-surface completion boundary for activity maps.

use std::sync::Arc;

use walkers::Tiles as _;

#[path = "map_runtime/browser_worker.rs"]
mod browser_worker;
#[path = "map_runtime/platform.rs"]
mod platform;

use platform::{ResponseQueue, Shared};

use super::map::{MapTileDecoder, MapTileResponse, PreparedTile, TileStore};

pub use super::map::MapTileDecoder as TileDecoder;
pub use super::map::gpu_map::{BrowserLabelTask, BrowserRouteTask};
pub use super::map::gpu_map::{
    prepare_labels_for_browser_worker, prepare_route_for_browser_worker,
};
pub use super::map::prepare_tile_for_browser_worker;
pub use browser_worker::{
    BROWSER_WORKER_PROTOCOL_VERSION, BrowserWorkerCompletion, BrowserWorkerRegistration,
    BrowserWorkerState, BrowserWorkerTaskKind, BrowserWorkerTransition,
};
pub use platform::Backend;

type SharedBackend = Shared<dyn Backend>;

/// One rendered activity-map frame, expressed as profiler-friendly measurements.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct MapPerformanceSample {
    /// Time since the previous rendered map frame, when the map was continuously active.
    pub frame_milliseconds: Option<f32>,
    /// CPU time spent in the map's immediate-mode UI pass.
    pub ui_milliseconds: f32,
    /// CPU time spent acquiring and publishing the render scene.
    pub scene_milliseconds: f32,
    /// CPU time spent querying the route spatial index.
    pub route_query_microseconds: f32,
    /// CPU time spent preparing the current label layout.
    pub label_milliseconds: f32,
    /// Whether label preparation is waiting for worker completion.
    pub label_backlog: usize,
    /// Total stale asynchronous results discarded by this surface.
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

/// Host-owned destination for activity-map performance measurements.
pub trait MapMetricsSink: Send + Sync {
    /// Record one map frame without delaying rendering.
    fn record(&self, sample: MapPerformanceSample);

    /// Record one renderer callback without delaying submission.
    fn record_render(&self, _sample: MapRenderPerformanceSample) {}
}

struct DiscardMapMetrics;

impl MapMetricsSink for DiscardMapMetrics {
    fn record(&self, _sample: MapPerformanceSample) {}
}

#[derive(Clone)]
pub(super) struct MapMetrics(Arc<dyn MapMetricsSink>);

impl MapMetrics {
    fn discard() -> Self {
        Self(Arc::new(DiscardMapMetrics))
    }

    fn new(metrics: impl MapMetricsSink + 'static) -> Self {
        Self(Arc::new(metrics))
    }

    fn record(&self, sample: MapPerformanceSample) {
        self.0.record(sample);
    }

    pub(super) fn record_render(&self, sample: MapRenderPerformanceSample) {
        self.0.record_render(sample);
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
    fn prepare_tile(&self, id: walkers::TileId, tile: walkers::Tile) -> PreparedTile;
    fn prepare_browser_tile(
        &self,
        bytes: &[u8],
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

    pub(super) fn prepare_tile(&self, id: walkers::TileId, tile: walkers::Tile) -> PreparedTile {
        self.0.prepare_tile(id, tile)
    }

    pub(super) fn prepare_browser_tile(
        &self,
        bytes: &[u8],
    ) -> Result<Option<Arc<super::map::PreparedGpuTile>>, String> {
        self.0.prepare_browser_tile(bytes)
    }
}

struct WgpuRenderer(super::map::WgpuMapHandle);

impl RendererFactory for WgpuRenderer {
    fn painter(&self, metrics: MapMetrics) -> Box<dyn ScenePainter> {
        Box::new(super::map::gpu_map::GpuMap::new(&self.0, metrics))
    }

    fn prepare_tile(&self, id: walkers::TileId, tile: walkers::Tile) -> PreparedTile {
        let (tile, gpu) = super::map::gpu_map::prepare_tile(&self.0, id, tile);
        PreparedTile { tile, gpu }
    }

    fn prepare_browser_tile(
        &self,
        bytes: &[u8],
    ) -> Result<Option<Arc<super::map::PreparedGpuTile>>, String> {
        super::map::gpu_map::decode_browser_tile(bytes)
    }
}

struct SoftwareRenderer;

impl RendererFactory for SoftwareRenderer {
    fn painter(&self, _metrics: MapMetrics) -> Box<dyn ScenePainter> {
        Box::new(SoftwarePainter)
    }

    fn prepare_tile(&self, _id: walkers::TileId, tile: walkers::Tile) -> PreparedTile {
        PreparedTile { tile, gpu: None }
    }

    fn prepare_browser_tile(
        &self,
        _bytes: &[u8],
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
    pub memory: &'a walkers::MapMemory,
    pub followed_position: walkers::Position,
    pub viewport: egui::Rect,
    pub backend: &'a dyn Backend,
}

#[derive(Clone, Copy)]
pub(super) struct SceneFrame<'frame, 'recording> {
    pub scene: MapScene<'frame>,
    pub memory: &'frame walkers::MapMemory,
    pub followed_position: walkers::Position,
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

    fn paint_labels(&mut self, _frame: LabelFrame<'_>) {}

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
        MapSurfaceHandle {
            backend: Shared::clone(&self.backend),
            metrics: self.metrics.clone(),
            painter: renderer.0.painter(self.metrics.clone()),
            renderer,
            responses: ResponseQueue::default(),
            tiles: TileStore::default(),
        }
    }
}

pub(super) struct MapSurfaceHandle {
    backend: SharedBackend,
    metrics: MapMetrics,
    painter: Box<dyn ScenePainter>,
    renderer: Renderer,
    responses: ResponseQueue,
    tiles: TileStore,
}

impl MapSurfaceHandle {
    pub(super) fn record_performance(&self, sample: MapPerformanceSample) {
        self.metrics.record(sample);
    }

    /// Begin one scoped demand frame that always dispatches on drop.
    pub(super) fn frame<'a>(
        &'a mut self,
        context: &egui::Context,
        dark_mode: bool,
    ) -> MapSurfaceFrame<'a> {
        self.begin_frame(context, dark_mode);
        MapSurfaceFrame {
            surface: self,
            context: context.clone(),
        }
    }

    pub(super) fn gpu_enabled(&self) -> bool {
        self.painter.is_gpu()
    }

    pub(super) fn paint_callback(&self, rect: egui::Rect) -> Option<egui::Shape> {
        self.painter.paint_callback(rect)
    }

    pub(super) fn paint_labels(
        &mut self,
        ui: &egui::Ui,
        memory: &walkers::MapMemory,
        followed_position: walkers::Position,
        viewport: egui::Rect,
    ) {
        self.painter.paint_labels(LabelFrame {
            ui,
            memory,
            followed_position,
            viewport,
            backend: self.backend.as_ref(),
        });
    }

    pub(super) fn update_scene(
        &mut self,
        memory: &walkers::MapMemory,
        followed_position: walkers::Position,
        viewport: egui::Rect,
        context: &egui::Context,
        route: &super::map::gpu_map::RouteScene<'_>,
    ) -> super::map::gpu_map::ScenePerf {
        self.painter.update(SceneFrame {
            scene: MapScene { tiles: &self.tiles },
            memory,
            followed_position,
            viewport,
            context,
            route,
            backend: self.backend.as_ref(),
        })
    }

    /// Apply completed work and begin collecting the current frame's tile demand.
    fn begin_frame(&mut self, context: &egui::Context, dark_mode: bool) {
        for response in self.responses.drain() {
            self.tiles.resolve(context, response);
        }
        self.tiles.set_theme(dark_mode);
        self.tiles.begin_frame();
    }

    /// Dispatch the bounded request batch produced by the current frame's demand.
    fn finish_frame(&mut self, context: &egui::Context) {
        self.tiles.schedule_requests();
        for request in self.tiles.take_requests() {
            self.backend.submit(TileTask {
                request,
                renderer: self.renderer.clone(),
                responses: self.responses.clone(),
                context: context.clone(),
            });
        }
    }

    /// Read-only scene state published by this surface.
    pub(super) const fn scene(&self) -> MapScene<'_> {
        MapScene { tiles: &self.tiles }
    }
}

pub(super) struct MapSurfaceFrame<'a> {
    surface: &'a mut MapSurfaceHandle,
    context: egui::Context,
}

impl std::ops::Deref for MapSurfaceFrame<'_> {
    type Target = MapSurfaceHandle;

    fn deref(&self) -> &Self::Target {
        self.surface
    }
}

impl std::ops::DerefMut for MapSurfaceFrame<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.surface
    }
}

impl walkers::Tiles for MapSurfaceFrame<'_> {
    fn at(&mut self, tile_id: walkers::TileId) -> Option<walkers::TilePiece> {
        self.surface.at(tile_id)
    }

    fn attribution(&self) -> walkers::sources::Attribution {
        self.surface.attribution()
    }

    fn tile_size(&self) -> u32 {
        self.surface.tile_size()
    }
}

impl Drop for MapSurfaceFrame<'_> {
    fn drop(&mut self) {
        self.surface.finish_frame(&self.context);
    }
}

impl walkers::Tiles for MapSurfaceHandle {
    fn at(&mut self, tile_id: walkers::TileId) -> Option<walkers::TilePiece> {
        self.tiles.at(tile_id)
    }

    fn attribution(&self) -> walkers::sources::Attribution {
        self.tiles.attribution()
    }

    fn tile_size(&self) -> u32 {
        self.tiles.tile_size()
    }
}

/// Immutable view of a surface's most recently published tile scene.
#[derive(Clone, Copy)]
pub(super) struct MapScene<'a> {
    tiles: &'a TileStore,
}

impl<'a> MapScene<'a> {
    pub(super) fn gpu_tiles(
        self,
    ) -> impl Iterator<
        Item = (
            &'a walkers::TileId,
            &'a std::sync::Arc<super::map::PreparedGpuTile>,
        ),
    > {
        self.tiles.gpu_tiles()
    }

    pub(super) fn background_unavailable(self) -> bool {
        self.tiles.visible_background_unavailable()
    }

    pub(super) fn ready_tiles(self) -> usize {
        self.tiles.ready_len()
    }

    pub(super) fn pending_tiles(self) -> usize {
        self.tiles.pending_len()
    }

    pub(super) fn attribution(self) -> walkers::sources::Attribution {
        self.tiles.attribution()
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
    pub fn complete_prepared(self, result: Result<Vec<u8>, String>) {
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

    #[test]
    fn surface_coalesces_demand_and_dispatches_center_tiles_first() {
        let backend = RecordingBackend::default();
        let submitted = Arc::clone(&backend.submitted);
        let runtime = MapRuntimeHandle::new(backend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();
        {
            let mut frame = surface.frame(&context, true);
            for x in 0..8 {
                let id = walkers::TileId { zoom: 4, x, y: 6 };
                let _first = frame.at(id);
                let _duplicate = frame.at(id);
            }
        }

        let requests = submitted
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 6);
        assert_eq!([requests[0].x, requests[1].x], [3, 4]);
        assert!(!requests.iter().any(|request| request.x == 0));
        assert!(!requests.iter().any(|request| request.x == 7));
    }

    #[test]
    fn completions_publish_only_at_the_next_surface_boundary() {
        let runtime = MapRuntimeHandle::new(CompletingBackend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();
        {
            let mut frame = surface.frame(&context, true);
            let _tile = frame.at(walkers::TileId {
                zoom: 4,
                x: 8,
                y: 6,
            });
        }
        assert_eq!(surface.scene().pending_tiles(), 1);

        assert_eq!(surface.scene().pending_tiles(), 1);

        {
            let frame = surface.frame(&context, true);
            assert_eq!(frame.scene().pending_tiles(), 0);
        }
    }

    #[test]
    fn empty_surface_frames_finalize_without_dispatching_work() {
        let backend = RecordingBackend::default();
        let submitted = Arc::clone(&backend.submitted);
        let runtime = MapRuntimeHandle::new(backend, Renderer::software());
        let mut surface = runtime.surface();
        let context = egui::Context::default();

        {
            let _frame = surface.frame(&context, true);
        }

        assert!(submitted.lock().unwrap().is_empty());
    }
}
