//! Persistent WGPU rendering for decoded vector-map geometry.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::similar_names,
    reason = "bounded GPU coordinates, indices, and grid cells intentionally use f32/i32"
)]

use std::{num::NonZeroU64, sync::Arc, time::Instant};

use arc_swap::{ArcSwap, ArcSwapOption};
use bytemuck::{Pod, Zeroable};
use egui::{Color32, Rect, Shape, pos2};
use egui_wgpu::{Callback, CallbackResources, CallbackTrait, ScreenDescriptor};
use garmin_service_api::ActivitySampleSnapshot;
use walkers::{Tile, TileId};
use wgpu::util::DeviceExt as _;

use super::{WALKERS_TILE_SIZE, mercator_y, speed_bounds};
use crate::activity::map_style;

#[path = "gpu_map/labels.rs"]
mod labels;
#[path = "gpu_map/platform.rs"]
mod platform;
#[path = "gpu_map/route.rs"]
mod route;

#[cfg(not(target_arch = "wasm32"))]
use labels::build_label_result;
pub use labels::{BrowserLabelTask, prepare_labels_for_browser_worker};
use labels::{LabelCache, LabelResult, LabelTask, LabelView};
#[cfg(not(target_arch = "wasm32"))]
use route::build_route;
pub use route::{BrowserRouteTask, prepare_route_for_browser_worker};
use route::{RouteOutcome, RouteResult, RouteSample, RouteTask};

#[cfg(test)]
use labels::{
    LABEL_PROTOCOL_VERSION, LabelOutcome, LabelRequestWire, LabelResultWire, decode_browser_labels,
    encode_browser_labels,
};
#[cfg(test)]
use route::{
    BrowserRouteSample, BrowserRouteSegment, ROUTE_PROTOCOL_VERSION, RouteRequestWire,
    RouteResultWire, decode_browser_route, encode_browser_route,
};

/// Device-scoped renderer capability installed by an eframe composition root.
#[derive(Clone)]
pub struct WgpuMapHandle {
    context: Arc<UploadContext>,
}

/// Install the map renderer in eframe's shared WGPU callback resources.
#[must_use]
pub fn install(render_state: &egui_wgpu::RenderState) -> WgpuMapHandle {
    let context = Arc::new(UploadContext::new(&render_state.device));
    let resources = Resources::new(
        &render_state.device,
        render_state.target_format,
        &context.camera_layout,
        &context.tile_layout,
        &context.route_layout,
        &context.route_style_layout,
    );
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);
    WgpuMapHandle { context }
}

#[derive(Clone)]
struct CpuTileMesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    texts: Vec<walkers::Text>,
}

const BROWSER_TILE_PROTOCOL_VERSION: u8 = 1;
const MAX_BROWSER_TILE_UPLOAD_BYTES: usize = 8 * 1024 * 1024;

#[derive(serde::Deserialize, serde::Serialize)]
struct BrowserPreparedTile {
    version: u8,
    vertices: Vec<BrowserVertex>,
    indices: Vec<u32>,
    texts: Vec<BrowserText>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct BrowserVertex {
    position: [f32; 2],
    color: [u8; 4],
}

#[derive(serde::Deserialize, serde::Serialize)]
struct BrowserText {
    text: String,
    position: [f32; 2],
    font_size: f32,
    text_color: [u8; 4],
    halo_color: [u8; 4],
    halo_width: f32,
    angle: f32,
    line_placement: bool,
}

/// Tessellate a worker tile directly into the versioned POD format consumed by WGPU.
pub(in crate::activity) fn encode_browser_tile(tile: Tile) -> Result<Vec<u8>, String> {
    let Tile::Vector { shapes, texts } = tile else {
        return Err("the vector map worker received a raster tile".to_owned());
    };
    let mut tessellator = egui::epaint::tessellator::Tessellator::new(
        1.0,
        egui::epaint::tessellator::TessellationOptions::default(),
        [1, 1],
        Vec::new(),
    );
    let mut mesh = egui::Mesh::default();
    for shape in shapes {
        tessellator.tessellate_shape(shape, &mut mesh);
    }
    let prepared = BrowserPreparedTile {
        version: BROWSER_TILE_PROTOCOL_VERSION,
        vertices: mesh
            .vertices
            .into_iter()
            .map(|vertex| BrowserVertex {
                position: [vertex.pos.x, vertex.pos.y],
                color: vertex.color.to_array(),
            })
            .collect(),
        indices: mesh.indices,
        texts: texts
            .into_iter()
            .map(|text| BrowserText {
                text: text.text,
                position: [text.position.x, text.position.y],
                font_size: text.font_size,
                text_color: text.text_color.to_array(),
                halo_color: text.halo_color.to_array(),
                halo_width: text.halo_width,
                angle: text.angle,
                line_placement: text.placement == walkers::Placement::Line,
            })
            .collect(),
    };
    postcard::to_stdvec(&prepared).map_err(|error| error.to_string())
}

pub(in crate::activity) fn decode_browser_tile(
    bytes: &[u8],
) -> Result<Option<Arc<PreparedGpuTile>>, String> {
    if bytes.len() > MAX_BROWSER_TILE_UPLOAD_BYTES {
        return Err("prepared map tile exceeded the 8 MiB upload limit".to_owned());
    }
    let prepared: BrowserPreparedTile =
        postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
    if prepared.version != BROWSER_TILE_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported map tile protocol version {}",
            prepared.version
        ));
    }
    if !prepared.indices.len().is_multiple_of(3)
        || prepared
            .indices
            .iter()
            .any(|index| *index as usize >= prepared.vertices.len())
    {
        return Err("prepared map tile contained invalid triangle indices".to_owned());
    }
    let mesh = CpuTileMesh {
        vertices: prepared
            .vertices
            .into_iter()
            .map(|vertex| Vertex {
                position: vertex.position,
                color: vertex.color,
            })
            .collect(),
        indices: prepared.indices,
        texts: prepared
            .texts
            .into_iter()
            .map(|text| walkers::Text {
                text: text.text,
                position: pos2(text.position[0], text.position[1]),
                font_size: text.font_size,
                text_color: Color32::from_rgba_premultiplied(
                    text.text_color[0],
                    text.text_color[1],
                    text.text_color[2],
                    text.text_color[3],
                ),
                halo_color: Color32::from_rgba_premultiplied(
                    text.halo_color[0],
                    text.halo_color[1],
                    text.halo_color[2],
                    text.halo_color[3],
                ),
                halo_width: text.halo_width,
                angle: text.angle,
                placement: if text.line_placement {
                    walkers::Placement::Line
                } else {
                    walkers::Placement::Point
                },
            })
            .collect(),
    };
    Ok(
        (!mesh.indices.is_empty() || !mesh.texts.is_empty()).then(|| {
            Arc::new(PreparedGpuTile {
                mesh: Arc::new(mesh),
                gpu: ArcSwapOption::empty(),
            })
        }),
    )
}

/// Tessellate tile-local shapes once and retain only text in the walkers fallback tile.
pub(in crate::activity) fn prepare_tile(
    handle: &WgpuMapHandle,
    id: TileId,
    tile: Tile,
) -> (Tile, Option<Arc<PreparedGpuTile>>) {
    match tile {
        Tile::Vector { shapes, texts } => {
            let mut tessellator = egui::epaint::tessellator::Tessellator::new(
                1.0,
                egui::epaint::tessellator::TessellationOptions::default(),
                [1, 1],
                Vec::new(),
            );
            let mut mesh = egui::Mesh::default();
            for shape in shapes {
                tessellator.tessellate_shape(shape, &mut mesh);
            }
            let vertices = mesh
                .vertices
                .into_iter()
                .map(|vertex| Vertex {
                    position: [vertex.pos.x, vertex.pos.y],
                    color: vertex.color.to_array(),
                })
                .collect();
            let prepared = (!mesh.indices.is_empty() || !texts.is_empty()).then(|| {
                let mesh = Arc::new(CpuTileMesh {
                    vertices,
                    indices: mesh.indices,
                    texts,
                });
                let gpu = (!mesh.indices.is_empty())
                    .then(|| Arc::new(GpuTile::new(&handle.context, id, Arc::clone(&mesh))));
                Arc::new(PreparedGpuTile {
                    mesh,
                    gpu: ArcSwapOption::from(gpu),
                })
            });
            (
                Tile::Vector {
                    shapes: Vec::new(),
                    texts: Vec::new(),
                },
                prepared,
            )
        }
        raster @ Tile::Raster(_) => (raster, None),
    }
}

pub(crate) struct PreparedGpuTile {
    mesh: Arc<CpuTileMesh>,
    gpu: ArcSwapOption<GpuTile>,
}

impl PreparedGpuTile {
    fn is_publishable(&self) -> bool {
        self.mesh.indices.is_empty() || self.gpu.load().is_some()
    }
}

#[derive(Clone, Default)]
struct Frame {
    camera: CameraUniform,
    visible: Vec<VisibleTile>,
    route: Option<VisibleRoute>,
}

#[derive(Clone)]
struct VisibleTile {
    id: TileId,
    tile: Arc<PreparedGpuTile>,
}

#[derive(Clone)]
struct VisibleRoute {
    resource: Arc<GpuRoute>,
    outline: RouteStyleUniform,
    color: RouteStyleUniform,
    highlight: Option<HighlightStyles>,
}

#[derive(Clone)]
struct HighlightStyles {
    outline: RouteStyleUniform,
    color: RouteStyleUniform,
}

struct RouteSource {
    resource: Arc<GpuRoute>,
}

#[derive(Default)]
struct RouteCache {
    state: RoutePreparation,
}

#[derive(Default)]
enum RoutePreparation {
    #[default]
    Vacant,
    Pending {
        key: String,
    },
    Ready {
        key: String,
        source: Option<RouteSource>,
    },
}

impl RouteCache {
    fn source(&self) -> Option<&RouteSource> {
        match &self.state {
            RoutePreparation::Ready {
                source: Some(source),
                ..
            } => Some(source),
            RoutePreparation::Vacant
            | RoutePreparation::Pending { .. }
            | RoutePreparation::Ready { source: None, .. } => None,
        }
    }

    fn begin(&mut self, key: &str) -> bool {
        if self.key() == Some(key) {
            return false;
        }
        self.state = RoutePreparation::Pending {
            key: key.to_owned(),
        };
        true
    }

    fn apply(&mut self, result: RouteResult) -> bool {
        let RouteResult { key, outcome } = result;
        if self.key() != Some(key.as_str()) {
            return false;
        }
        self.state = match outcome {
            RouteOutcome::Ready(source) => RoutePreparation::Ready { key, source },
            RouteOutcome::Failed => RoutePreparation::Vacant,
        };
        true
    }

    fn key(&self) -> Option<&str> {
        match &self.state {
            RoutePreparation::Vacant => None,
            RoutePreparation::Pending { key } | RoutePreparation::Ready { key, .. } => Some(key),
        }
    }
}

pub(in crate::activity) struct GpuMap {
    frame: Arc<ArcSwap<Frame>>,
    metrics: crate::activity::map_runtime::MapMetrics,
    runtime: WgpuRuntime,
    labels: LabelCache,
    route: RouteCache,
}

struct WgpuRuntime {
    surface: Arc<SurfaceGpu>,
    executor: platform::Executor,
}

impl WgpuRuntime {
    fn upload_visible(&self, visible: &[VisibleTile]) -> UploadStats {
        platform::upload_visible(&self.executor, visible)
    }

    fn poll_label(&self) -> Option<LabelResult> {
        self.executor.poll_label()
    }

    fn schedule_label(
        &self,
        task: LabelTask,
        backend: &dyn crate::activity::map_runtime::Backend,
    ) -> (Option<LabelResult>, bool) {
        self.executor.schedule_label(task, backend)
    }

    fn poll_route(&self) -> Option<RouteResult> {
        self.executor.poll_route()
    }

    fn schedule_route(&self, task: RouteTask, backend: &dyn crate::activity::map_runtime::Backend) {
        self.executor.schedule_route(task, backend);
    }
}

#[derive(Clone, Copy, Default)]
struct UploadStats {
    queued_bytes: usize,
    uploaded_bytes: usize,
}

#[derive(Clone)]
pub(in crate::activity) struct RouteScene<'a> {
    pub key: &'a str,
    pub samples: &'a [ActivitySampleSnapshot],
    pub sample_offset: usize,
    pub highlighted_range: Option<std::ops::RangeInclusive<usize>>,
    pub color: Color32,
    pub outline: Color32,
    pub opacity: f32,
}

#[derive(Default)]
pub(in crate::activity) struct ScenePerf {
    pub milliseconds: f32,
    pub visible_tiles: usize,
    pub label_milliseconds: f32,
    pub label_backlog: usize,
    pub stale_work: u64,
    pub queued_upload_bytes: usize,
    pub uploaded_bytes: usize,
}

impl GpuMap {
    pub(in crate::activity) fn new(
        handle: &WgpuMapHandle,
        metrics: crate::activity::map_runtime::MapMetrics,
    ) -> Self {
        let context = Arc::clone(&handle.context);
        Self {
            frame: Arc::new(ArcSwap::from_pointee(Frame::default())),
            metrics,
            runtime: WgpuRuntime {
                surface: Arc::new(SurfaceGpu::new(&context)),
                executor: platform::Executor::new(context),
            },
            labels: LabelCache::default(),
            route: RouteCache::default(),
        }
    }

    pub(in crate::activity) fn paint_callback(&self, rect: Rect) -> Shape {
        Shape::Callback(Callback::new_paint_callback(
            rect,
            Paint {
                frame: Arc::clone(&self.frame),
                metrics: self.metrics.clone(),
                surface: Arc::clone(&self.runtime.surface),
            },
        ))
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "egui and WGPU viewport coordinates are f32 after bounded f64 Mercator projection"
    )]
    pub(in crate::activity) fn update(
        &mut self,
        frame: crate::activity::map_runtime::SceneFrame<'_, '_>,
    ) -> ScenePerf {
        let crate::activity::map_runtime::SceneFrame {
            scene,
            memory,
            followed_position,
            viewport,
            context,
            route,
            backend,
        } = frame;
        let started = Instant::now();
        let _span = tracing::trace_span!("activity_map_scene_acquisition").entered();
        self.prepare_route(route, context, backend);
        let center = memory.detached().unwrap_or(followed_position);
        let zoom = memory.zoom();
        let world_size = f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(zoom);
        let center_normalized = [center.x() / 360.0 + 0.5, mercator_y(center.y())];
        let center_x = center_normalized[0] * world_size;
        let center_y = center_normalized[1] * world_size;
        let viewport_size = [viewport.width().max(1.0), viewport.height().max(1.0)];
        let mut visible = Vec::new();

        for (id, tile) in scene.gpu_tiles() {
            let tile_size = f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(zoom - f64::from(id.zoom));
            let left =
                f64::from(viewport.center().x) + f64::from(id.x).mul_add(tile_size, -center_x);
            let top =
                f64::from(viewport.center().y) + f64::from(id.y).mul_add(tile_size, -center_y);
            let tile_rect = Rect::from_min_size(
                egui::pos2(left as f32, top as f32),
                egui::Vec2::splat(tile_size as f32),
            );
            if !viewport.intersects(tile_rect) {
                continue;
            }
            visible.push(VisibleTile {
                id: *id,
                tile: Arc::clone(tile),
            });
        }

        visible.sort_unstable_by_key(|tile| (tile.id.zoom, tile.id.y, tile.id.x));
        let upload = self.runtime.upload_visible(&visible);
        visible.retain(|tile| tile.tile.is_publishable());
        let route = self.route.source().map(|source| {
            let fallback = color(route.color);
            let highlight = route.highlighted_range.as_ref().map(|range| {
                let index_range_padding = [*range.start() as f32, *range.end() as f32, 0.0, 0.0];
                HighlightStyles {
                    outline: RouteStyleUniform::new(
                        map_style::HIGHLIGHT_OUTLINE_WIDTH,
                        1.0,
                        2.0,
                        color(route.outline),
                        index_range_padding,
                    ),
                    color: RouteStyleUniform::new(
                        map_style::HIGHLIGHT_WIDTH,
                        1.0,
                        3.0,
                        fallback,
                        index_range_padding,
                    ),
                }
            });
            VisibleRoute {
                resource: Arc::clone(&source.resource),
                outline: RouteStyleUniform::new(
                    map_style::ROUTE_OUTLINE_WIDTH,
                    route.opacity.max(map_style::MINIMUM_OUTLINE_OPACITY),
                    0.0,
                    color(route.outline),
                    [0.0; 4],
                ),
                color: RouteStyleUniform::new(
                    map_style::ROUTE_WIDTH,
                    route.opacity,
                    1.0,
                    fallback,
                    [0.0; 4],
                ),
                highlight,
            }
        });
        let visible_tiles = visible.len();
        self.frame.store(Arc::new(Frame {
            camera: CameraUniform::new(center_normalized, viewport_size, world_size),
            visible,
            route,
        }));
        let label_metrics = self.labels.metrics();
        ScenePerf {
            milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
            visible_tiles,
            label_milliseconds: label_metrics.milliseconds,
            label_backlog: label_metrics.backlog,
            stale_work: label_metrics.stale_work,
            queued_upload_bytes: upload.queued_bytes,
            uploaded_bytes: upload.uploaded_bytes,
        }
    }

    pub(in crate::activity) fn paint_labels(
        &mut self,
        frame: crate::activity::map_runtime::LabelFrame<'_>,
    ) {
        let crate::activity::map_runtime::LabelFrame {
            ui,
            memory,
            followed_position,
            viewport,
            backend,
        } = frame;
        while let Some(result) = self.runtime.poll_label() {
            self.labels.apply(result);
        }
        let frame = self.frame.load_full();
        let visible = frame.visible.clone();
        if visible.is_empty() {
            return;
        }

        let view = LabelView::new(memory, followed_position, viewport);
        if !self.labels.request_matches(&visible, view) {
            if let Some(delay) = self.labels.defer_request_for_motion(view, Instant::now()) {
                ui.ctx().request_repaint_after(delay);
            } else {
                let task = self.labels.request(&visible, view, ui.ctx().clone());
                let (ready, discarded) = self.runtime.schedule_label(task, backend);
                if discarded {
                    self.labels.record_discarded_work();
                }
                if let Some(result) = ready {
                    self.labels.apply(result);
                }
            }
        }
        self.labels.paint(ui, view, viewport);
    }

    fn prepare_route(
        &mut self,
        input: &RouteScene<'_>,
        context: &egui::Context,
        backend: &dyn crate::activity::map_runtime::Backend,
    ) {
        while let Some(result) = self.runtime.poll_route() {
            self.route.apply(result);
        }
        if !self.route.begin(input.key) {
            return;
        }
        let speed_bounds = speed_bounds(input.samples);
        let samples = input
            .samples
            .iter()
            .map(|sample| RouteSample {
                coordinate: sample.coordinate.map(|coordinate| {
                    [
                        coordinate.longitude().as_degrees(),
                        coordinate.latitude().as_degrees(),
                    ]
                }),
                speed: sample.speed.and_then(|speed| {
                    speed_bounds.map(|(minimum, maximum)| {
                        ((f64::from(speed.as_millimeters_per_second()) - minimum)
                            / (maximum - minimum))
                            .clamp(0.0, 1.0) as f32
                    })
                }),
            })
            .collect();
        let task = RouteTask {
            key: input.key.to_owned(),
            samples,
            sample_offset: input.sample_offset,
            context: context.clone(),
        };
        self.runtime.schedule_route(task, backend);
    }
}

fn color(color: Color32) -> [f32; 4] {
    color.to_array().map(|channel| f32::from(channel) / 255.0)
}

struct Paint {
    frame: Arc<ArcSwap<Frame>>,
    metrics: crate::activity::map_runtime::MapMetrics,
    surface: Arc<SurfaceGpu>,
}

impl CallbackTrait for Paint {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let started = Instant::now();
        let _span = tracing::trace_span!("activity_map_render_prepare").entered();
        let frame = self.frame.load_full();
        let surface = &self.surface;
        queue.write_buffer(&surface.camera, 0, bytemuck::bytes_of(&frame.camera));
        if let Some(route) = &frame.route {
            queue.write_buffer(
                &surface.outline_style,
                0,
                bytemuck::bytes_of(&route.outline),
            );
            queue.write_buffer(&surface.color_style, 0, bytemuck::bytes_of(&route.color));
            if let Some(highlight) = &route.highlight {
                queue.write_buffer(
                    &surface.highlight_outline_style,
                    0,
                    bytemuck::bytes_of(&highlight.outline),
                );
                queue.write_buffer(
                    &surface.highlight_color_style,
                    0,
                    bytemuck::bytes_of(&highlight.color),
                );
            }
        }
        self.metrics
            .record_render(crate::activity::map_runtime::MapRenderPerformanceSample {
                phase: crate::activity::map_runtime::MapRenderPhase::Prepare,
                milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
            });
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let started = Instant::now();
        let _span = tracing::trace_span!("activity_map_draw_submission").entered();
        let frame = self.frame.load_full();
        let Some(resources) = resources.get::<Resources>() else {
            return;
        };
        let surface = &self.surface;
        let clip = map_style::clip_rect(info.viewport, info.clip_rect);
        let clip = egui::epaint::PaintCallbackInfo {
            viewport: clip,
            clip_rect: clip,
            pixels_per_point: info.pixels_per_point,
            screen_size_px: info.screen_size_px,
        }
        .viewport_in_pixels();
        let (Ok(left), Ok(top), Ok(width), Ok(height)) = (
            u32::try_from(clip.left_px),
            u32::try_from(clip.top_px),
            u32::try_from(clip.width_px),
            u32::try_from(clip.height_px),
        ) else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        render_pass.set_scissor_rect(left, top, width, height);
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &surface.camera_bind_group, &[]);
        for tile in &frame.visible {
            let Some(gpu) = tile.tile.gpu.load_full() else {
                continue;
            };
            render_pass.set_bind_group(1, &gpu.bind_group, &[]);
            render_pass.set_vertex_buffer(0, gpu.vertices.slice(..));
            render_pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..gpu.index_count, 0, 0..1);
        }
        if let Some(route) = &frame.route {
            let gpu = &route.resource;
            render_pass.set_pipeline(&resources.route_pipeline);
            render_pass.set_bind_group(0, &surface.camera_bind_group, &[]);
            render_pass.set_bind_group(1, &gpu.bind_group, &[]);
            render_pass.set_bind_group(2, &surface.outline_bind_group, &[]);
            render_pass.draw(0..6, 0..gpu.segment_count);
            render_pass.set_bind_group(2, &surface.color_bind_group, &[]);
            render_pass.draw(0..6, 0..gpu.segment_count);
            if route.highlight.is_some() {
                render_pass.set_bind_group(2, &surface.highlight_outline_bind_group, &[]);
                render_pass.draw(0..6, 0..gpu.segment_count);
                render_pass.set_bind_group(2, &surface.highlight_color_bind_group, &[]);
                render_pass.draw(0..6, 0..gpu.segment_count);
            }
        }
        self.metrics
            .record_render(crate::activity::map_runtime::MapRenderPerformanceSample {
                phase: crate::activity::map_runtime::MapRenderPhase::Draw,
                milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
            });
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct CameraUniform {
    center_high_low: [f32; 4],
    viewport_world_size_padding: [f32; 4],
}

impl CameraUniform {
    fn new(center: [f64; 2], viewport: [f32; 2], world_size: f64) -> Self {
        let (center_x_high, center_x_low) = split_f64(center[0]);
        let (center_y_high, center_y_low) = split_f64(center[1]);
        Self {
            center_high_low: [center_x_high, center_y_high, center_x_low, center_y_low],
            viewport_world_size_padding: [viewport[0], viewport[1], world_size as f32, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TileUniform {
    origin_high_low: [f32; 4],
    normalized_point_scale_padding: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RouteSegment {
    start: [f32; 2],
    end: [f32; 2],
    speed: [f32; 2],
    sample_indices: [f32; 2],
}

#[derive(Clone)]
struct CpuRoute {
    segments: Vec<RouteSegment>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RouteSourceUniform {
    origin_high_low: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RouteStyleUniform {
    width_opacity_mode_padding: [f32; 4],
    fallback: [f32; 4],
    index_range_padding: [f32; 4],
    speed_colors: [[f32; 4]; 5],
}

impl RouteStyleUniform {
    fn new(
        width: f32,
        opacity: f32,
        mode: f32,
        fallback: [f32; 4],
        index_range_padding: [f32; 4],
    ) -> Self {
        Self {
            width_opacity_mode_padding: [width, opacity, mode, 0.0],
            fallback,
            index_range_padding,
            speed_colors: map_style::speed_colors_uniform(),
        }
    }
}

struct GpuTile {
    _source: Arc<CpuTileMesh>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    _uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    index_count: u32,
}

impl GpuTile {
    fn new(context: &UploadContext, id: TileId, source: Arc<CpuTileMesh>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let _creation = context
            .resource_creation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = &context.device;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity map tile vertices"),
            contents: bytemuck::cast_slice(&source.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity map tile indices"),
            contents: bytemuck::cast_slice(&source.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let tile_count = 2.0_f64.powi(i32::from(id.zoom));
        let (origin_x_high, origin_x_low) = split_f64(f64::from(id.x) / tile_count);
        let (origin_y_high, origin_y_low) = split_f64(f64::from(id.y) / tile_count);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity map tile uniform"),
            contents: bytemuck::bytes_of(&TileUniform {
                origin_high_low: [origin_x_high, origin_y_high, origin_x_low, origin_y_low],
                normalized_point_scale_padding: [
                    (1.0 / (tile_count * 512.0)) as f32,
                    0.0,
                    0.0,
                    0.0,
                ],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("activity map tile bind group"),
            layout: &context.tile_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        Self {
            index_count: u32::try_from(source.indices.len()).unwrap_or(u32::MAX),
            _source: source,
            vertices,
            indices,
            _uniform: uniform,
            bind_group,
        }
    }
}

struct GpuRoute {
    _source: Arc<CpuRoute>,
    bind_group: wgpu::BindGroup,
    segment_count: u32,
    _segments: wgpu::Buffer,
    _origin: wgpu::Buffer,
}

impl GpuRoute {
    fn new(context: &UploadContext, origin: [f64; 2], source: Arc<CpuRoute>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let _creation = context
            .resource_creation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = &context.device;
        let segments = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity route segments"),
            contents: bytemuck::cast_slice(&source.segments),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let (origin_x_high, origin_x_low) = split_f64(origin[0]);
        let (origin_y_high, origin_y_low) = split_f64(origin[1]);
        let origin = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity route origin"),
            contents: bytemuck::bytes_of(&RouteSourceUniform {
                origin_high_low: [origin_x_high, origin_y_high, origin_x_low, origin_y_low],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = route_bind_group(device, &context.route_layout, &segments, &origin);
        Self {
            segment_count: u32::try_from(source.segments.len()).unwrap_or(u32::MAX),
            _source: source,
            bind_group,
            _segments: segments,
            _origin: origin,
        }
    }
}

fn route_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    segments: &wgpu::Buffer,
    origin: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("activity route source bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: segments.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: origin.as_entire_binding(),
            },
        ],
    })
}

struct Resources {
    pipeline: wgpu::RenderPipeline,
    route_pipeline: wgpu::RenderPipeline,
}

impl Resources {
    fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
        tile_layout: &wgpu::BindGroupLayout,
        route_layout: &wgpu::BindGroupLayout,
        route_style_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("activity map shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_map.wgsl").into()),
        });
        Self {
            pipeline: tile_pipeline(device, target_format, &shader, camera_layout, tile_layout),
            route_pipeline: route_pipeline(
                device,
                target_format,
                &shader,
                camera_layout,
                route_layout,
                route_style_layout,
            ),
        }
    }
}

struct UploadContext {
    device: wgpu::Device,
    #[cfg(not(target_arch = "wasm32"))]
    resource_creation: std::sync::Mutex<()>,
    camera_layout: wgpu::BindGroupLayout,
    tile_layout: wgpu::BindGroupLayout,
    route_layout: wgpu::BindGroupLayout,
    route_style_layout: wgpu::BindGroupLayout,
}

impl UploadContext {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            resource_creation: std::sync::Mutex::new(()),
            camera_layout: camera_layout(device),
            tile_layout: tile_uniform_layout(device),
            route_layout: route_layout(device),
            route_style_layout: route_style_layout(device),
        }
    }
}

struct SurfaceGpu {
    camera: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    outline_style: wgpu::Buffer,
    outline_bind_group: wgpu::BindGroup,
    color_style: wgpu::Buffer,
    color_bind_group: wgpu::BindGroup,
    highlight_outline_style: wgpu::Buffer,
    highlight_outline_bind_group: wgpu::BindGroup,
    highlight_color_style: wgpu::Buffer,
    highlight_color_bind_group: wgpu::BindGroup,
}

impl SurfaceGpu {
    fn new(context: &UploadContext) -> Self {
        let camera = uniform_buffer::<CameraUniform>(&context.device, "activity map camera");
        let camera_bind_group = single_uniform_bind_group(
            &context.device,
            &context.camera_layout,
            &camera,
            "activity map camera bind group",
        );
        let outline_style =
            uniform_buffer::<RouteStyleUniform>(&context.device, "activity route outline style");
        let outline_bind_group = single_uniform_bind_group(
            &context.device,
            &context.route_style_layout,
            &outline_style,
            "activity route outline style bind group",
        );
        let color_style =
            uniform_buffer::<RouteStyleUniform>(&context.device, "activity route color style");
        let color_bind_group = single_uniform_bind_group(
            &context.device,
            &context.route_style_layout,
            &color_style,
            "activity route color style bind group",
        );
        let highlight_outline_style = uniform_buffer::<RouteStyleUniform>(
            &context.device,
            "activity highlighted route outline style",
        );
        let highlight_outline_bind_group = single_uniform_bind_group(
            &context.device,
            &context.route_style_layout,
            &highlight_outline_style,
            "activity highlighted route outline style bind group",
        );
        let highlight_color_style = uniform_buffer::<RouteStyleUniform>(
            &context.device,
            "activity highlighted route color style",
        );
        let highlight_color_bind_group = single_uniform_bind_group(
            &context.device,
            &context.route_style_layout,
            &highlight_color_style,
            "activity highlighted route color style bind group",
        );
        Self {
            camera,
            camera_bind_group,
            outline_style,
            outline_bind_group,
            color_style,
            color_bind_group,
            highlight_outline_style,
            highlight_outline_bind_group,
            highlight_color_style,
            highlight_color_bind_group,
        }
    }
}

fn uniform_buffer<T>(device: &wgpu::Device, label: &'static str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: core::mem::size_of::<T>() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn single_uniform_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    })
}

fn split_f64(value: f64) -> (f32, f32) {
    let high = value as f32;
    (high, (value - f64::from(high)) as f32)
}

fn camera_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    uniform_layout::<CameraUniform>(
        device,
        "activity map camera layout",
        wgpu::ShaderStages::VERTEX,
    )
}

fn tile_uniform_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    uniform_layout::<TileUniform>(
        device,
        "activity map tile uniform layout",
        wgpu::ShaderStages::VERTEX,
    )
}

fn route_style_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    uniform_layout::<RouteStyleUniform>(
        device,
        "activity route style layout",
        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
    )
}

fn uniform_layout<T>(
    device: &wgpu::Device,
    label: &'static str,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(core::mem::size_of::<T>() as u64),
            },
            count: None,
        }],
    })
}

fn tile_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    camera_layout: &wgpu::BindGroupLayout,
    tile_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("activity map tile pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(tile_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("activity map tile pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: core::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Unorm8x4],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if target_format.is_srgb() {
                "fragment_linear"
            } else {
                "fragment_gamma"
            }),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(color_target(target_format))],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn route_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("activity route layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(core::mem::size_of::<RouteSegment>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(
                        core::mem::size_of::<RouteSourceUniform>() as u64
                    ),
                },
                count: None,
            },
        ],
    })
}

fn route_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    camera_layout: &wgpu::BindGroupLayout,
    route_layout: &wgpu::BindGroupLayout,
    route_style_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("activity route pipeline layout"),
        bind_group_layouts: &[
            Some(camera_layout),
            Some(route_layout),
            Some(route_style_layout),
        ],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("activity route pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("route_vertex"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if target_format.is_srgb() {
                "route_fragment_linear"
            } else {
                "route_fragment_gamma"
            }),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(color_target(target_format))],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn color_target(format: wgpu::TextureFormat) -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        write_mask: wgpu::ColorWrites::ALL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_f32_pair_eq(actual: [f32; 2], expected: [f32; 2]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn map_shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(include_str!("gpu_map.wgsl"))
            .expect("activity map WGSL must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("activity map WGSL must validate");
    }

    #[test]
    fn gpu_pipeline_layouts_match_their_shader_interfaces() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = futures_lite::future::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) else {
            eprintln!("skipping WGPU pipeline validation because no adapter is available");
            return;
        };
        let (device, _queue) = futures_lite::future::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default()),
        )
        .expect("the test adapter must provide a default WGPU device");
        let context = UploadContext::new(&device);

        for format in [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
        ] {
            let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _resources = Resources::new(
                &device,
                format,
                &context.camera_layout,
                &context.tile_layout,
                &context.route_layout,
                &context.route_style_layout,
            );
            let error = futures_lite::future::block_on(error_scope.pop());

            assert!(
                error.is_none(),
                "map pipeline validation failed for {format:?}: {error:?}"
            );
        }
    }

    #[test]
    fn route_cache_types_pending_empty_stale_and_failed_transitions() {
        let mut cache = RouteCache::default();
        assert!(cache.begin("activity-a"));
        assert!(!cache.begin("activity-a"));
        assert!(!cache.apply(RouteResult {
            key: "stale".to_owned(),
            outcome: RouteOutcome::Ready(None),
        }));
        assert!(cache.apply(RouteResult {
            key: "activity-a".to_owned(),
            outcome: RouteOutcome::Ready(None),
        }));
        assert!(!cache.begin("activity-a"));
        assert!(cache.source().is_none());

        assert!(cache.begin("activity-b"));
        assert!(!cache.apply(RouteResult {
            key: "activity-a".to_owned(),
            outcome: RouteOutcome::Failed,
        }));
        assert!(cache.apply(RouteResult {
            key: "activity-b".to_owned(),
            outcome: RouteOutcome::Failed,
        }));
        assert!(cache.begin("activity-b"));
    }

    #[test]
    fn browser_tile_protocol_round_trips_final_gpu_vertices_without_uvs() {
        let vertices = [
            ([1.0, 2.0], [255, 0, 0, 255]),
            ([3.0, 4.0], [0, 255, 0, 255]),
            ([5.0, 6.0], [0, 0, 255, 255]),
        ]
        .into_iter()
        .map(|(position, color)| egui::epaint::Vertex {
            pos: pos2(position[0], position[1]),
            uv: pos2(0.75, 0.25),
            color: Color32::from_rgba_premultiplied(color[0], color[1], color[2], color[3]),
        })
        .collect();
        let tile = Tile::Vector {
            shapes: vec![egui::Shape::mesh(egui::Mesh {
                indices: vec![0, 1, 2],
                vertices,
                texture_id: egui::TextureId::default(),
            })],
            texts: Vec::new(),
        };

        let bytes = encode_browser_tile(tile).unwrap();
        let decoded = decode_browser_tile(&bytes).unwrap().unwrap();

        assert_eq!(decoded.mesh.vertices.len(), 3);
        assert_f32_pair_eq(decoded.mesh.vertices[2].position, [5.0, 6.0]);
        assert_eq!(decoded.mesh.indices, [0, 1, 2]);
        assert!(decoded.gpu.load().is_none());
        assert!(!decoded.is_publishable());
    }

    #[test]
    fn browser_protocol_rejects_unknown_versions_and_invalid_indices() {
        let unknown = BrowserPreparedTile {
            version: BROWSER_TILE_PROTOCOL_VERSION + 1,
            vertices: Vec::new(),
            indices: Vec::new(),
            texts: Vec::new(),
        };
        let bytes = postcard::to_stdvec(&unknown).unwrap();
        assert!(decode_browser_tile(&bytes).is_err());

        let invalid = BrowserPreparedTile {
            version: BROWSER_TILE_PROTOCOL_VERSION,
            vertices: vec![BrowserVertex {
                position: [0.0, 0.0],
                color: [255; 4],
            }],
            indices: vec![0, 1, 2],
            texts: Vec::new(),
        };
        let bytes = postcard::to_stdvec(&invalid).unwrap();
        assert!(decode_browser_tile(&bytes).is_err());
    }

    #[test]
    fn browser_label_protocol_shapes_and_tessellates_text_off_thread() {
        let context = egui::Context::default();
        crate::install_assets(&context);
        let request = LabelRequestWire {
            version: LABEL_PROTOCOL_VERSION,
            generation: 7,
            center: [0.5, 0.5],
            zoom: 12.0,
            viewport: [0.0, 0.0, 320.0, 180.0],
            texts: vec![BrowserText {
                text: "Helsinki".to_owned(),
                position: [160.0, 90.0],
                font_size: 14.0,
                text_color: [240, 240, 240, 255],
                halo_color: [16, 16, 16, 255],
                halo_width: 1.0,
                angle: 0.0,
                line_placement: false,
            }],
        };

        let bytes = postcard::to_stdvec(&request).unwrap();
        let result_bytes = encode_browser_labels(&bytes, &context).unwrap();
        let result: LabelResultWire = postcard::from_bytes(&result_bytes).unwrap();

        assert_eq!(result.version, LABEL_PROTOCOL_VERSION);
        assert_eq!(result.generation, 7);
        assert!(!result.vertices.is_empty());
        assert!(!result.indices.is_empty());
        assert_eq!(
            result.atlas_pixels.len(),
            result.atlas_size[0] * result.atlas_size[1]
        );
    }

    #[test]
    fn browser_label_protocol_rejects_a_wrong_generation_before_publication() {
        let result = LabelResultWire {
            version: LABEL_PROTOCOL_VERSION,
            generation: 8,
            milliseconds: 0.0,
            atlas_size: [0, 0],
            atlas_pixels: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        let bytes = postcard::to_stdvec(&result).unwrap();

        assert!(decode_browser_labels(&bytes, &egui::Context::default(), 7).is_err());
    }

    #[test]
    fn label_requests_reuse_small_same_zoom_translations_but_not_zoom_changes() {
        let viewport = Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(800.0, 600.0));
        let anchor = LabelView {
            center: [0.5, 0.5],
            zoom: 10.0,
            viewport,
        };
        let world_size = f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(anchor.zoom);
        let translated = LabelView {
            center: [anchor.center[0] - 40.0 / world_size, anchor.center[1]],
            ..anchor
        };
        let too_far = LabelView {
            center: [anchor.center[0] - 97.0 / world_size, anchor.center[1]],
            ..anchor
        };
        let zoomed = LabelView {
            zoom: anchor.zoom + 1.0,
            ..anchor
        };

        assert!(anchor.can_reuse_layout(translated));
        assert!(!anchor.can_reuse_layout(too_far));
        assert!(!anchor.can_reuse_layout(zoomed));
    }

    #[test]
    fn label_rebuild_waits_until_camera_motion_settles() {
        let initial = LabelView {
            center: [0.5, 0.5],
            zoom: 10.0,
            viewport: Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
        };
        let moved = LabelView {
            center: [0.51, 0.5],
            ..initial
        };
        let started = Instant::now();
        let mut cache = LabelCache::default();

        assert_eq!(cache.defer_request_for_motion(initial, started), None);
        assert_eq!(
            cache.defer_request_for_motion(moved, started),
            Some(std::time::Duration::from_millis(120))
        );
        assert_eq!(
            cache.defer_request_for_motion(moved, started + std::time::Duration::from_millis(50)),
            Some(std::time::Duration::from_millis(70))
        );
        assert_eq!(
            cache.defer_request_for_motion(moved, started + std::time::Duration::from_millis(120)),
            None
        );
    }

    #[test]
    fn stale_label_generations_never_replace_the_newest_request() {
        let view = LabelView {
            center: [0.5, 0.5],
            zoom: 10.0,
            viewport: Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
        };
        let mut cache = LabelCache::default();
        let first = cache.request(&[], view, egui::Context::default());
        let _newest = cache.request(&[], view, egui::Context::default());

        cache.apply(LabelResult {
            generation: first.generation,
            view,
            outcome: LabelOutcome::Ready {
                milliseconds: 2.0,
                shapes: vec![Shape::circle_filled(pos2(10.0, 10.0), 2.0, Color32::WHITE)],
                texture: None,
            },
        });

        let metrics = cache.metrics();
        assert_eq!(metrics.stale_work, 1);
        assert!(metrics.milliseconds.abs() < f32::EPSILON);
        assert_eq!(metrics.backlog, 1);
    }

    #[test]
    fn label_cache_owns_failure_publication_backlog_and_discard_metrics() {
        let view = LabelView {
            center: [0.5, 0.5],
            zoom: 10.0,
            viewport: Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
        };
        let mut cache = LabelCache::default();
        let failed = cache.request(&[], view, egui::Context::default());
        assert_eq!(cache.metrics().backlog, 1);
        cache.apply(LabelResult {
            generation: failed.generation,
            view,
            outcome: LabelOutcome::Failed,
        });
        assert_eq!(cache.metrics().backlog, 0);

        let ready = cache.request(&[], view, egui::Context::default());
        cache.apply(LabelResult {
            generation: ready.generation,
            view,
            outcome: LabelOutcome::Ready {
                milliseconds: 4.0,
                shapes: Vec::new(),
                texture: None,
            },
        });
        assert!((cache.metrics().milliseconds - 4.0).abs() < f32::EPSILON);
        assert_eq!(cache.metrics().backlog, 0);
        cache.record_discarded_work();
        assert_eq!(cache.metrics().stale_work, 1);
    }

    #[test]
    fn browser_route_protocol_projects_segments_and_preserves_sample_indices() {
        let request = RouteRequestWire {
            version: ROUTE_PROTOCOL_VERSION,
            sample_offset: 40,
            samples: vec![
                BrowserRouteSample {
                    coordinate: Some([-0.1276, 51.5072]),
                    speed: Some(0.25),
                },
                BrowserRouteSample {
                    coordinate: Some([-0.1260, 51.5080]),
                    speed: Some(0.75),
                },
                BrowserRouteSample {
                    coordinate: None,
                    speed: None,
                },
            ],
        };

        let request_bytes = postcard::to_stdvec(&request).unwrap();
        let result_bytes = encode_browser_route(&request_bytes).unwrap();
        let prepared = decode_browser_route(&result_bytes).unwrap().unwrap();

        assert!(prepared.origin.into_iter().all(f64::is_finite));
        assert_eq!(prepared.route.segments.len(), 1);
        assert_f32_pair_eq(prepared.route.segments[0].speed, [0.25, 0.75]);
        assert_f32_pair_eq(prepared.route.segments[0].sample_indices, [40.0, 41.0]);
    }

    #[test]
    fn browser_route_protocol_rejects_unversioned_and_non_finite_geometry() {
        let unknown = RouteRequestWire {
            version: ROUTE_PROTOCOL_VERSION + 1,
            sample_offset: 0,
            samples: Vec::new(),
        };
        assert!(encode_browser_route(&postcard::to_stdvec(&unknown).unwrap()).is_err());

        let invalid = RouteResultWire {
            version: ROUTE_PROTOCOL_VERSION,
            origin: Some([0.5, 0.5]),
            segments: vec![BrowserRouteSegment {
                start: [f32::NAN, 0.0],
                end: [1.0, 1.0],
                speed: [0.0, 1.0],
                sample_indices: [0.0, 1.0],
            }],
        };
        assert!(decode_browser_route(&postcard::to_stdvec(&invalid).unwrap()).is_err());
    }
}
