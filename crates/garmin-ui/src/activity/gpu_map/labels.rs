//! Label projection, collision placement, worker protocol, and cache publication.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use egui::{Color32, Rect, Shape, pos2};
use walkers::TileId;
use web_time::Instant;

use super::{BrowserText, CpuTileMesh, VisibleTile};
use crate::activity::map::{WALKERS_TILE_SIZE, camera::MapCamera};

const CAMERA_SETTLE_DELAY: Duration = Duration::from_millis(120);

#[derive(Clone, Copy)]
pub(super) struct LabelView {
    pub(super) center: [f64; 2],
    pub(super) zoom: f64,
    pub(super) viewport: Rect,
}

impl LabelView {
    pub(super) fn new(camera: &MapCamera, viewport: Rect) -> Self {
        Self {
            center: camera.center_normalized(),
            zoom: camera.zoom(),
            viewport,
        }
    }

    pub(super) fn can_reuse_layout(self, other: Self) -> bool {
        const MAX_TRANSLATION: f32 = 96.0;
        self.zoom.to_bits() == other.zoom.to_bits()
            && self.viewport.size().x.to_bits() == other.viewport.size().x.to_bits()
            && self.viewport.size().y.to_bits() == other.viewport.size().y.to_bits()
            && {
                let translation = self.translation_to(other);
                translation.x.abs() <= MAX_TRANSLATION && translation.y.abs() <= MAX_TRANSLATION
            }
    }

    fn same_camera(self, other: Self) -> bool {
        self.center.map(f64::to_bits) == other.center.map(f64::to_bits)
            && self.zoom.to_bits() == other.zoom.to_bits()
            && self.viewport == other.viewport
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "label positions are screen-space f32 values after bounded Mercator projection"
    )]
    pub(super) fn translation_to(self, other: Self) -> egui::Vec2 {
        let world_size = f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(self.zoom);
        let center_x = other.center[0] + (self.center[0] - other.center[0]).round();
        egui::vec2(
            ((self.center[0] - center_x) * world_size) as f32 + other.viewport.center().x
                - self.viewport.center().x,
            ((self.center[1] - other.center[1]) * world_size) as f32 + other.viewport.center().y
                - self.viewport.center().y,
        )
    }
}

#[derive(Default)]
pub(super) struct LabelCache {
    requested: Option<RequestedLabels>,
    published: Option<PublishedLabels>,
    next_generation: u64,
    last_milliseconds: f32,
    stale_work: u64,
    motion: LabelMotion,
}

#[derive(Default)]
struct LabelMotion {
    last_view: Option<LabelView>,
    settle_until: Option<Instant>,
}

impl LabelMotion {
    fn defer_for_motion(&mut self, view: LabelView, now: Instant) -> Option<Duration> {
        if self
            .last_view
            .is_some_and(|previous| !previous.same_camera(view))
        {
            self.settle_until = Some(now + CAMERA_SETTLE_DELAY);
        }
        self.last_view = Some(view);
        let settle_until = self.settle_until?;
        if now < settle_until {
            Some(settle_until.saturating_duration_since(now))
        } else {
            self.settle_until = None;
            None
        }
    }
}

struct RequestedLabels {
    generation: u64,
    anchor: LabelView,
    sources: Vec<(TileId, Arc<CpuTileMesh>)>,
}

struct PublishedLabels {
    generation: u64,
    anchor: LabelView,
    shapes: Vec<Shape>,
    _texture: Option<egui::TextureHandle>,
}

#[derive(Clone, Copy)]
pub(super) struct LabelCacheMetrics {
    pub(super) milliseconds: f32,
    pub(super) backlog: usize,
    pub(super) stale_work: u64,
}

impl LabelCache {
    pub(super) fn request_matches(&self, visible: &[VisibleTile], view: LabelView) -> bool {
        self.requested.as_ref().is_some_and(|requested| {
            requested.anchor.can_reuse_layout(view)
                && requested.sources.len() == visible.len()
                && requested
                    .sources
                    .iter()
                    .zip(visible)
                    .all(|((id, source), tile)| {
                        *id == tile.id && Arc::ptr_eq(source, &tile.tile.mesh)
                    })
        })
    }

    pub(super) fn request(
        &mut self,
        visible: &[VisibleTile],
        view: LabelView,
        context: egui::Context,
    ) -> LabelTask {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.requested = Some(RequestedLabels {
            generation: self.next_generation,
            anchor: view,
            sources: visible
                .iter()
                .map(|tile| (tile.id, Arc::clone(&tile.tile.mesh)))
                .collect(),
        });
        LabelTask {
            generation: self.next_generation,
            visible: visible.to_vec(),
            view,
            context,
        }
    }

    pub(super) fn defer_request_for_motion(
        &mut self,
        view: LabelView,
        now: Instant,
    ) -> Option<Duration> {
        self.motion.defer_for_motion(view, now)
    }

    pub(super) fn apply(&mut self, result: LabelResult) {
        if self
            .requested
            .as_ref()
            .is_none_or(|requested| requested.generation != result.generation)
        {
            self.stale_work = self.stale_work.saturating_add(1);
            return;
        }
        match result.outcome {
            LabelOutcome::Ready {
                milliseconds,
                shapes,
                texture,
            } => {
                self.last_milliseconds = milliseconds;
                self.published = Some(PublishedLabels {
                    generation: result.generation,
                    anchor: result.view,
                    shapes,
                    _texture: texture,
                });
            }
            LabelOutcome::Failed => {
                self.requested = None;
            }
        }
    }

    pub(super) fn record_discarded_work(&mut self) {
        self.stale_work = self.stale_work.saturating_add(1);
    }

    pub(super) fn paint(&self, ui: &egui::Ui, view: LabelView, viewport: Rect) {
        let Some(published) = &self.published else {
            return;
        };
        if (published.anchor.zoom - view.zoom).abs() > f64::EPSILON {
            return;
        }
        let delta = published.anchor.translation_to(view);
        let shapes = published.shapes.iter().cloned().map(|mut shape| {
            shape.translate(delta);
            shape
        });
        ui.painter().with_clip_rect(viewport).extend(shapes);
    }

    pub(super) fn metrics(&self) -> LabelCacheMetrics {
        let backlog = self.requested.as_ref().is_some_and(|requested| {
            self.published
                .as_ref()
                .is_none_or(|published| published.generation != requested.generation)
        });
        LabelCacheMetrics {
            milliseconds: self.last_milliseconds,
            backlog: usize::from(backlog),
            stale_work: self.stale_work,
        }
    }
}

pub(super) struct LabelTask {
    pub(super) generation: u64,
    pub(super) visible: Vec<VisibleTile>,
    pub(super) view: LabelView,
    pub(super) context: egui::Context,
}

pub(super) struct LabelResult {
    pub(super) generation: u64,
    pub(super) view: LabelView,
    pub(super) outcome: LabelOutcome,
}

pub(super) enum LabelOutcome {
    Ready {
        milliseconds: f32,
        shapes: Vec<Shape>,
        texture: Option<egui::TextureHandle>,
    },
    Failed,
}

/// Browser-worker label operation with an opaque request/completion protocol.
pub struct BrowserLabelTask {
    pub(super) task: LabelTask,
    pub(super) results: Arc<Mutex<VecDeque<LabelResult>>>,
}

impl BrowserLabelTask {
    /// Serialize this operation for the dedicated browser worker.
    ///
    /// # Errors
    ///
    /// Returns an error when the bounded worker request cannot be serialized.
    pub fn request(&self) -> Result<Vec<u8>, String> {
        postcard::to_stdvec(&label_request(&self.task)).map_err(|error| error.to_string())
    }

    pub fn complete(self, result: Result<Vec<u8>, String>) {
        let completed = result.and_then(|bytes| {
            decode_browser_labels(&bytes, &self.task.context, self.task.generation)
        });
        let outcome = match completed {
            Ok(payload) => LabelOutcome::Ready {
                milliseconds: payload.milliseconds,
                shapes: payload.shapes,
                texture: Some(payload.texture),
            },
            Err(reason) => {
                tracing::warn!(%reason, "browser map label preparation failed");
                LabelOutcome::Failed
            }
        };
        let result = LabelResult {
            generation: self.task.generation,
            view: self.task.view,
            outcome,
        };
        self.results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(result);
        self.task.context.request_repaint();
    }
}

pub(super) const LABEL_PROTOCOL_VERSION: u8 = 1;

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct LabelRequestWire {
    pub(super) version: u8,
    pub(super) generation: u64,
    pub(super) center: [f64; 2],
    pub(super) zoom: f64,
    pub(super) viewport: [f32; 4],
    pub(super) texts: Vec<BrowserText>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct LabelResultWire {
    pub(super) version: u8,
    pub(super) generation: u64,
    pub(super) milliseconds: f32,
    pub(super) atlas_size: [usize; 2],
    pub(super) atlas_pixels: Vec<[u8; 4]>,
    pub(super) vertices: Vec<BrowserLabelVertex>,
    pub(super) indices: Vec<u32>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(super) struct BrowserLabelVertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [u8; 4],
}

pub(super) struct DecodedBrowserLabels {
    milliseconds: f32,
    shapes: Vec<Shape>,
    texture: egui::TextureHandle,
}

fn projected_texts(task: &LabelTask) -> Vec<walkers::Text> {
    let world_size = f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(task.view.zoom);
    let expanded = task.view.viewport.expand(96.0);
    let mut texts = Vec::new();
    for tile in &task.visible {
        let placement = crate::activity::map::camera::TilePlacement::new(
            tile.id,
            task.view.center,
            world_size,
            task.view.viewport,
        );
        let scale = placement.size as f32 / 512.0;
        for world in placement.copies(expanded) {
            let origin = placement.origin(world);
            for source in &tile.tile.mesh.texts {
                let position = origin + source.position.to_vec2() * scale;
                if expanded.contains(position) {
                    let mut text = source.clone();
                    text.position = position;
                    texts.push(text);
                }
            }
        }
    }
    texts
}

pub(super) fn label_request(task: &LabelTask) -> LabelRequestWire {
    LabelRequestWire {
        version: LABEL_PROTOCOL_VERSION,
        generation: task.generation,
        center: task.view.center,
        zoom: task.view.zoom,
        viewport: [
            task.view.viewport.min.x,
            task.view.viewport.min.y,
            task.view.viewport.max.x,
            task.view.viewport.max.y,
        ],
        texts: projected_texts(task)
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
    }
}

fn browser_text(text: BrowserText) -> walkers::Text {
    walkers::Text {
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
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn build_label_result(task: &LabelTask) -> LabelResult {
    let started = Instant::now();
    let shapes = place_texts_spatial(projected_texts(task), &task.context);
    LabelResult {
        generation: task.generation,
        view: task.view,
        outcome: LabelOutcome::Ready {
            milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
            shapes,
            texture: None,
        },
    }
}

fn place_texts_spatial(texts: Vec<walkers::Text>, context: &egui::Context) -> Vec<Shape> {
    const CELL_SIZE: f32 = 64.0;
    const MIN_REPEAT_DISTANCE: f32 = 400.0;
    const HALO_RING: [egui::Vec2; 8] = [
        egui::vec2(1.0, 0.0),
        egui::vec2(0.7, 0.7),
        egui::vec2(0.0, 1.0),
        egui::vec2(-0.7, 0.7),
        egui::vec2(-1.0, 0.0),
        egui::vec2(-0.7, -0.7),
        egui::vec2(0.0, -1.0),
        egui::vec2(0.7, -0.7),
    ];

    let mut occupied = HashMap::<(i32, i32), Vec<Rect>>::new();
    let mut repeated = HashMap::<String, Vec<egui::Pos2>>::new();
    let mut shapes = Vec::new();
    context.fonts_mut(|fonts| {
        for text in texts {
            if text.placement == walkers::Placement::Line
                && repeated.get(&text.text).is_some_and(|positions| {
                    positions
                        .iter()
                        .any(|position| position.distance(text.position) < MIN_REPEAT_DISTANCE)
                })
            {
                continue;
            }

            let mut job = egui::text::LayoutJob::default();
            job.append(
                &text.text,
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::proportional(text.font_size),
                    color: text.text_color,
                    ..Default::default()
                },
            );
            let galley = fonts.layout_job(job);
            let half = galley.size() * 0.5;
            let (sine, cosine) = text.angle.sin_cos();
            let x_axis = egui::vec2(half.x * cosine, half.x * sine);
            let y_axis = egui::vec2(-half.y * sine, half.y * cosine);
            let extent = egui::vec2(
                half.x.mul_add(cosine.abs(), half.y * sine.abs()),
                half.x.mul_add(sine.abs(), half.y * cosine.abs()),
            );
            let bounds = Rect::from_center_size(text.position, extent * 2.0);
            let cells = label_cells(bounds, CELL_SIZE);
            if cells.iter().any(|cell| {
                occupied
                    .get(cell)
                    .is_some_and(|areas| areas.iter().any(|area| area.intersects(bounds)))
            }) {
                continue;
            }
            for cell in cells {
                occupied.entry(cell).or_default().push(bounds);
            }
            if text.placement == walkers::Placement::Line {
                repeated
                    .entry(text.text.clone())
                    .or_default()
                    .push(text.position);
            }

            let top_left = text.position - x_axis - y_axis;
            let on_top =
                egui::epaint::TextShape::new(top_left, Arc::clone(&galley), text.text_color)
                    .with_angle(text.angle);
            if text.halo_width <= 0.0 || text.halo_color.a() == 0 {
                shapes.push(on_top.into());
                continue;
            }
            let mut halo = HALO_RING
                .iter()
                .map(|offset| {
                    egui::epaint::TextShape::new(
                        top_left + *offset * text.halo_width,
                        Arc::clone(&galley),
                        text.halo_color,
                    )
                    .with_angle(text.angle)
                    .with_override_text_color(text.halo_color)
                    .into()
                })
                .collect::<Vec<Shape>>();
            halo.push(on_top.into());
            shapes.push(Shape::Vec(halo));
        }
    });
    shapes
}

/// Shape, place, and tessellate a label request inside the dedicated browser worker.
///
/// # Errors
///
/// Returns an error when the request is malformed or its output cannot be serialized.
pub fn prepare_labels_for_browser_worker(bytes: &[u8]) -> Result<Vec<u8>, String> {
    thread_local! {
        static CONTEXT: egui::Context = {
            let context = egui::Context::default();
            crate::install_assets(&context);
            context
        };
    }
    CONTEXT.with(|context| encode_browser_labels(bytes, context))
}

pub(super) fn encode_browser_labels(
    bytes: &[u8],
    context: &egui::Context,
) -> Result<Vec<u8>, String> {
    let request: LabelRequestWire =
        postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
    if request.version != LABEL_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported map label protocol version {}",
            request.version
        ));
    }
    let viewport = Rect::from_min_max(
        pos2(request.viewport[0], request.viewport[1]),
        pos2(request.viewport[2], request.viewport[3]),
    );
    let texts: Vec<walkers::Text> = request.texts.into_iter().map(browser_text).collect();
    let mut shapes = Vec::new();
    let input = egui::RawInput {
        screen_rect: Some(viewport),
        ..egui::RawInput::default()
    };
    let started = Instant::now();
    let mut output = context.run_ui(input, |ui| {
        shapes = place_texts_spatial(texts.clone(), ui.ctx());
    });
    output.textures_delta.clear();
    drop(output);
    let primitives = context.tessellate(
        shapes
            .into_iter()
            .map(|shape| egui::epaint::ClippedShape {
                clip_rect: viewport,
                shape,
            })
            .collect(),
        1.0,
    );
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for primitive in primitives {
        let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive else {
            return Err("browser label worker produced a paint callback".to_owned());
        };
        if mesh.texture_id != egui::TextureId::default() {
            return Err("browser label worker produced an unexpected texture".to_owned());
        }
        let base = u32::try_from(vertices.len())
            .map_err(|_| "browser label vertex count overflowed u32".to_owned())?;
        vertices.extend(mesh.vertices.into_iter().map(|vertex| BrowserLabelVertex {
            position: [vertex.pos.x, vertex.pos.y],
            uv: [vertex.uv.x, vertex.uv.y],
            color: vertex.color.to_array(),
        }));
        indices.extend(mesh.indices.into_iter().map(|index| base + index));
    }
    let atlas = context.fonts(font_image);
    let result = LabelResultWire {
        version: LABEL_PROTOCOL_VERSION,
        generation: request.generation,
        milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
        atlas_size: atlas.size,
        atlas_pixels: atlas
            .pixels
            .into_iter()
            .map(|color| color.to_array())
            .collect(),
        vertices,
        indices,
    };
    postcard::to_stdvec(&result).map_err(|error| error.to_string())
}

fn font_image(fonts: &egui::epaint::FontsView<'_>) -> egui::ColorImage {
    fonts.image()
}

pub(super) fn decode_browser_labels(
    bytes: &[u8],
    context: &egui::Context,
    expected_generation: u64,
) -> Result<DecodedBrowserLabels, String> {
    if bytes.len() > crate::activity::map_runtime::BrowserWorkerTaskKind::Labels.result_byte_limit()
    {
        return Err("prepared map labels exceeded the 16 MiB browser limit".to_owned());
    }
    let result: LabelResultWire = postcard::from_bytes(bytes).map_err(|error| error.to_string())?;
    if result.version != LABEL_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported map label protocol version {}",
            result.version
        ));
    }
    if result.generation != expected_generation {
        return Err(format!(
            "prepared map labels belonged to generation {}, expected {expected_generation}",
            result.generation
        ));
    }
    if result.atlas_size[0].saturating_mul(result.atlas_size[1]) != result.atlas_pixels.len() {
        return Err("prepared map label atlas dimensions were invalid".to_owned());
    }
    if !result.indices.len().is_multiple_of(3)
        || result
            .indices
            .iter()
            .any(|index| *index as usize >= result.vertices.len())
    {
        return Err("prepared map labels contained invalid triangle indices".to_owned());
    }
    let atlas = egui::ColorImage::new(
        result.atlas_size,
        result
            .atlas_pixels
            .into_iter()
            .map(|color| Color32::from_rgba_premultiplied(color[0], color[1], color[2], color[3]))
            .collect(),
    );
    let texture = context.load_texture(
        "activity-map-worker-label-atlas",
        atlas,
        egui::TextureOptions::LINEAR,
    );
    let mesh = egui::Mesh {
        indices: result.indices,
        vertices: result
            .vertices
            .into_iter()
            .map(|vertex| egui::epaint::Vertex {
                pos: pos2(vertex.position[0], vertex.position[1]),
                uv: pos2(vertex.uv[0], vertex.uv[1]),
                color: Color32::from_rgba_premultiplied(
                    vertex.color[0],
                    vertex.color[1],
                    vertex.color[2],
                    vertex.color[3],
                ),
            })
            .collect(),
        texture_id: texture.id(),
    };
    if !mesh.is_valid() {
        return Err("prepared map label mesh was invalid".to_owned());
    }
    let shapes = (!mesh.indices.is_empty())
        .then(|| Shape::mesh(mesh))
        .into_iter()
        .collect();
    Ok(DecodedBrowserLabels {
        milliseconds: result.milliseconds,
        shapes,
        texture,
    })
}

fn label_cells(bounds: Rect, cell_size: f32) -> Vec<(i32, i32)> {
    let min_x = (bounds.min.x / cell_size).floor() as i32;
    let max_x = (bounds.max.x / cell_size).floor() as i32;
    let min_y = (bounds.min.y / cell_size).floor() as i32;
    let max_y = (bounds.max.y / cell_size).floor() as i32;
    let mut cells =
        Vec::with_capacity(usize::try_from((max_x - min_x + 1) * (max_y - min_y + 1)).unwrap_or(0));
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            cells.push((x, y));
        }
    }
    cells
}
