//! Host-fed `walkers` activity map and linked route overlay.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use cint::ColorInterop;
use egui::{Align2, Layout, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2};
use garmin_model::route::Coordinate;
use garmin_service_api::{ActivityRecordingSnapshot, ActivitySampleSnapshot};
use walkers::{Map, MapMemory, Tiles, lon_lat, sources::Attribution};

use super::{
    map_runtime::{MapRuntimeHandle, MapSurfaceFrame, MapSurfaceHandle},
    map_style,
    route_index::RouteIndex,
};

#[path = "gpu_map.rs"]
pub(super) mod gpu_map;
#[path = "map/tile_store.rs"]
mod tile_store;

pub(super) use gpu_map::PreparedGpuTile;
pub use gpu_map::{WgpuMapHandle, install as install_wgpu_map};
use tile_store::{MAX_VIEW_ZOOM, WALKERS_TILE_SIZE};
pub use tile_store::{MapTileDecoder, prepare_tile_for_browser_worker};
pub(super) use tile_store::{MapTileResponse, PreparedTile, TileStore};

#[cfg(test)]
use tile_store::{DECODED_TILE_LIMIT, Failure, MAX_IN_FLIGHT, MapTilePayload, TileEntry};

const ZOOM_BOUND_EPSILON: f64 = 1.0e-6;
const DEFAULT_ZOOM_SPEED: f64 = 2.0;
const ROUTE_POINT_SPACING: f32 = 1.5;

pub(super) struct Props<'a> {
    pub recording: &'a ActivityRecordingSnapshot,
    pub selected_coordinate: Option<(f64, f64)>,
    pub sample_range: std::ops::RangeInclusive<usize>,
    pub highlighted_range: Option<std::ops::RangeInclusive<usize>>,
    pub fit_key: &'a str,
    pub empty: &'a str,
    pub background_unavailable: &'a str,
    pub height: f32,
}

pub(super) struct ActivityMap {
    surface: MapSurfaceHandle,
    memory: MapMemory,
    fit: FitState,
    frame_timing: FrameTiming,
    route_index: RouteIndexCache,
}

impl ActivityMap {
    pub fn new(runtime: &MapRuntimeHandle) -> Self {
        Self {
            surface: runtime.surface(),
            memory: MapMemory::default(),
            fit: FitState::default(),
            frame_timing: FrameTiming::default(),
            route_index: RouteIndexCache::default(),
        }
    }
    pub fn show(&mut self, ui: &mut Ui, props: &Props<'_>) -> Output {
        let ui_started = Instant::now();
        let _span = tracing::trace_span!("activity_map_ui").entered();
        let size = Vec2::new(ui.available_width(), props.height);
        let (sample_offset, samples) =
            ranged_samples(&props.recording.samples, props.sample_range.clone());
        let Some(center) = center(samples) else {
            let _frame = self.surface.frame(ui.ctx(), ui.visuals().dark_mode);
            return empty_map(ui, size, props.empty);
        };
        self.fit_if_needed(samples, props.fit_key, size);
        self.index_route_if_needed(samples, sample_offset, props.fit_key);
        let mut surface = self.surface.frame(ui.ctx(), ui.visuals().dark_mode);

        let colors = MapColors::new(ui);
        let route_scene = gpu_map::RouteScene {
            key: props.fit_key,
            samples,
            sample_offset,
            highlighted_range: props.highlighted_range.clone(),
            color: colors.route,
            outline: colors.outline,
            opacity: if props.highlighted_range.is_some() {
                map_style::DIMMED_ROUTE_OPACITY
            } else {
                1.0
            },
        };
        let rendered = Self::render_map(
            &mut surface,
            &mut self.memory,
            self.route_index.current(),
            ui,
            MapRenderInput {
                size,
                center,
                route: &route_scene,
                colors,
                selected_coordinate: props.selected_coordinate,
            },
        );
        if surface.scene().background_unavailable() {
            map_status(ui, rendered.rect, props.background_unavailable);
        }

        let scene = surface.scene();
        let performance = self.frame_timing.sample(MapPerfSample {
            ui_elapsed: ui_started.elapsed(),
            scene_milliseconds: rendered.scene.milliseconds,
            route_query_microseconds: rendered.route_query_microseconds,
            label_milliseconds: rendered.scene.label_milliseconds,
            label_backlog: rendered.scene.label_backlog,
            stale_work: rendered.scene.stale_work,
            visible_tiles: rendered.scene.visible_tiles,
            ready_tiles: scene.ready_tiles(),
            pending_tiles: scene.pending_tiles(),
            queued_upload_bytes: rendered.scene.queued_upload_bytes,
            uploaded_bytes: rendered.scene.uploaded_bytes,
        });
        attribution(ui, &scene.attribution(), performance.as_deref());
        Output {
            hovered: rendered.interaction.hovered,
            clicked: rendered.interaction.clicked,
            empty_clicked: rendered.interaction.empty_clicked,
            rect: rendered.rect,
        }
    }

    fn fit_if_needed(&mut self, samples: &[ActivitySampleSnapshot], fit_key: &str, size: Vec2) {
        self.fit
            .apply_if_needed(&mut self.memory, samples, fit_key, size);
    }

    fn index_route_if_needed(
        &mut self,
        samples: &[ActivitySampleSnapshot],
        sample_offset: usize,
        fit_key: &str,
    ) {
        self.route_index.resolve(samples, sample_offset, fit_key);
    }

    fn render_map(
        surface: &mut MapSurfaceFrame<'_>,
        memory: &mut MapMemory,
        route_index: &RouteIndex,
        ui: &mut Ui,
        input: MapRenderInput<'_>,
    ) -> RenderedMap {
        let MapRenderInput {
            size,
            center,
            route,
            colors,
            selected_coordinate,
        } = input;
        let projection_center_longitude = memory
            .detached()
            .map_or_else(|| center.x(), walkers::Position::x);
        let map_rect = Rect::from_min_size(ui.next_widget_position(), size);
        ui.painter().rect_filled(
            map_rect,
            crate::theme::PANEL_RADIUS,
            crate::theme::palette(ui)
                .surfaces()
                .layer(garmin_color::theme::Level::One)
                .into_cint(),
        );

        let item_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut route_query_microseconds = 0.0;
        let mut scene = gpu_map::ScenePerf::default();
        let inner =
            ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |map_ui| {
                map_ui.set_min_size(size);
                let zoom_policy = zoom_policy(map_ui, memory.zoom(), map_rect);
                let gpu_enabled = surface.gpu_enabled();
                if let Some(callback) = surface.paint_callback(map_rect) {
                    map_ui.painter().add(callback);
                }
                let overlay_input = OverlayInput {
                    center,
                    projection_center_longitude,
                    gpu_enabled,
                    route,
                    colors,
                    selected_coordinate,
                };
                let inner = Map::new(Some(surface as &mut dyn Tiles), memory, center)
                    .zoom_with_ctrl(false)
                    .zoom_gesture(zoom_policy.gesture_enabled)
                    .zoom_speed(zoom_policy.speed)
                    .panning(true)
                    .show(map_ui, |overlay, response, projector, memory| {
                        let output = paint_route_overlay(
                            overlay,
                            response,
                            projector,
                            memory,
                            route_index,
                            overlay_input,
                        );
                        route_query_microseconds = output.query_microseconds;
                        output.interaction
                    });
                if gpu_enabled {
                    surface.paint_labels(map_ui, memory, center, inner.response.rect);
                    scene = surface.update_scene(
                        memory,
                        center,
                        inner.response.rect,
                        map_ui.ctx(),
                        route,
                    );
                }
                inner.inner
            });
        ui.spacing_mut().item_spacing.y = item_spacing;
        RenderedMap {
            interaction: inner.inner,
            rect: inner.response.rect,
            route_query_microseconds,
            scene,
        }
    }

    pub fn zoom_in(&mut self) {
        let zoom = (self.memory.zoom() + 1.0).min(f64::from(MAX_VIEW_ZOOM));
        let _ignored = self.memory.set_zoom(zoom);
    }

    pub fn zoom_out(&mut self) {
        let _ignored = self.memory.zoom_out();
    }

    pub const fn fit(&mut self) {
        self.fit.request();
    }
}

#[derive(Default)]
struct FitState {
    applied: Option<AppliedFit>,
    requested: bool,
    #[cfg(test)]
    applications: u64,
}

struct AppliedFit {
    key: String,
    size: Vec2,
}

impl FitState {
    fn apply_if_needed(
        &mut self,
        memory: &mut MapMemory,
        samples: &[ActivitySampleSnapshot],
        key: &str,
        size: Vec2,
    ) {
        let invalid = self.applied.as_ref().is_none_or(|applied| {
            applied.key != key
                || (applied.size.x - size.x).abs() > 64.0
                || (applied.size.y - size.y).abs() > 64.0
        });
        if !self.requested && !invalid {
            return;
        }
        fit(memory, samples, size);
        self.applied = Some(AppliedFit {
            key: key.to_owned(),
            size,
        });
        self.requested = false;
        #[cfg(test)]
        {
            self.applications += 1;
        }
    }

    const fn request(&mut self) {
        self.requested = true;
    }
}

#[derive(Default)]
struct RouteIndexCache {
    entry: Option<CachedRouteIndex>,
    #[cfg(test)]
    builds: u64,
}

struct CachedRouteIndex {
    key: String,
    value: RouteIndex,
}

impl RouteIndexCache {
    fn resolve(&mut self, samples: &[ActivitySampleSnapshot], sample_offset: usize, key: &str) {
        if self.entry.as_ref().is_some_and(|entry| entry.key == key) {
            return;
        }
        self.entry = Some(CachedRouteIndex {
            key: key.to_owned(),
            value: RouteIndex::new(samples, sample_offset),
        });
        #[cfg(test)]
        {
            self.builds += 1;
        }
    }

    fn current(&self) -> &RouteIndex {
        &self
            .entry
            .as_ref()
            .expect("route index is resolved before map rendering")
            .value
    }
}

#[derive(Clone, Copy)]
struct MapColors {
    route: egui::Color32,
    outline: egui::Color32,
    marker_fill: egui::Color32,
    start: egui::Color32,
    end: egui::Color32,
}

impl MapColors {
    fn new(ui: &Ui) -> Self {
        let palette = crate::theme::palette(ui);
        Self {
            route: crate::theme::color32(crate::theme::selection_accent(ui)),
            outline: egui::Color32::from_black_alpha(190),
            marker_fill: crate::theme::color32(palette.surfaces().background()),
            start: crate::theme::color32(palette.support().success()),
            end: crate::theme::color32(palette.support().error()),
        }
    }
}

#[derive(Clone, Copy)]
struct OverlayInput<'frame, 'recording> {
    center: walkers::Position,
    projection_center_longitude: f64,
    gpu_enabled: bool,
    route: &'frame gpu_map::RouteScene<'recording>,
    colors: MapColors,
    selected_coordinate: Option<(f64, f64)>,
}

#[derive(Clone, Copy)]
struct MapRenderInput<'recording> {
    size: Vec2,
    center: walkers::Position,
    route: &'recording gpu_map::RouteScene<'recording>,
    colors: MapColors,
    selected_coordinate: Option<(f64, f64)>,
}

struct OverlayOutput {
    interaction: RouteInteraction,
    query_microseconds: f32,
}

struct RenderedMap {
    interaction: RouteInteraction,
    rect: Rect,
    route_query_microseconds: f32,
    scene: gpu_map::ScenePerf,
}

fn paint_route_overlay(
    overlay: &mut Ui,
    response: &egui::Response,
    projector: &walkers::Projector,
    memory: &MapMemory,
    route_index: &RouteIndex,
    input: OverlayInput<'_, '_>,
) -> OverlayOutput {
    overlay.set_clip_rect(map_style::clip_rect(response.rect, overlay.clip_rect()));
    let pointer = (!response.is_pointer_button_down_on())
        .then(|| response.hover_pos())
        .flatten();
    let query_started = Instant::now();
    let indexed_hover = input.gpu_enabled.then(|| {
        pointer.and_then(|pointer| {
            let center = memory.detached().unwrap_or(input.center);
            let world_pixels = f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(memory.zoom());
            let world_pointer = [
                center.x() / 360.0
                    + 0.5
                    + f64::from(pointer.x - response.rect.center().x) / world_pixels,
                mercator_y(center.y())
                    + f64::from(pointer.y - response.rect.center().y) / world_pixels,
            ];
            route_index.query(world_pointer, world_pixels, 14.0)
        })
    });
    let query_microseconds = query_started.elapsed().as_secs_f32() * 1_000_000.0;
    let mut hit_test = HitTest {
        pointer,
        hovered: indexed_hover.flatten(),
        nearest: 14.0_f32.powi(2),
    };
    if !input.gpu_enabled {
        paint_software_route(overlay, projector, input, &mut hit_test);
    }
    paint_route_markers(overlay, projector, input);
    set_route_cursor(overlay, response, hit_test.hovered);
    OverlayOutput {
        interaction: RouteInteraction {
            hovered: hit_test.hovered,
            clicked: response.clicked().then_some(hit_test.hovered).flatten(),
            empty_clicked: response.clicked() && hit_test.hovered.is_none(),
        },
        query_microseconds,
    }
}

fn paint_software_route(
    overlay: &mut Ui,
    projector: &walkers::Projector,
    input: OverlayInput<'_, '_>,
    hit_test: &mut HitTest,
) {
    let style = RouteStyle {
        width: map_style::ROUTE_WIDTH,
        outline_width: map_style::ROUTE_OUTLINE_WIDTH,
        fallback: input.colors.route,
        outline: input.colors.outline,
        speed_bounds: speed_bounds(input.route.samples),
        opacity: input.route.opacity,
    };
    let mut current = Vec::with_capacity(input.route.samples.len());
    for (offset, sample) in input.route.samples.iter().enumerate() {
        let sample_index = input.route.sample_offset + offset;
        if let Some(coordinate) = sample.coordinate {
            push_route_point(
                &mut current,
                RoutePoint {
                    index: sample_index,
                    position: project_coordinate(
                        projector,
                        coordinate,
                        input.projection_center_longitude,
                    ),
                    speed: sample
                        .speed
                        .map(|speed| f64::from(speed.as_millimeters_per_second())),
                },
            );
        } else {
            paint_route_segment(overlay, &current, style, Some(hit_test));
            current.clear();
        }
    }
    paint_route_segment(overlay, &current, style, Some(hit_test));
    paint_highlighted_route(overlay, projector, input, style);
}

fn paint_highlighted_route(
    overlay: &mut Ui,
    projector: &walkers::Projector,
    input: OverlayInput<'_, '_>,
    base_style: RouteStyle,
) {
    let Some(range) = &input.route.highlighted_range else {
        return;
    };
    let style = RouteStyle {
        width: map_style::HIGHLIGHT_WIDTH,
        outline_width: map_style::HIGHLIGHT_OUTLINE_WIDTH,
        opacity: 1.0,
        ..base_style
    };
    let mut highlighted = Vec::with_capacity(
        range
            .end()
            .saturating_sub(*range.start())
            .saturating_add(1)
            .min(input.route.samples.len()),
    );
    for (offset, sample) in input.route.samples.iter().enumerate() {
        let sample_index = input.route.sample_offset + offset;
        if range.contains(&sample_index)
            && let Some(coordinate) = sample.coordinate
        {
            push_route_point(
                &mut highlighted,
                RoutePoint {
                    index: sample_index,
                    position: project_coordinate(
                        projector,
                        coordinate,
                        input.projection_center_longitude,
                    ),
                    speed: sample
                        .speed
                        .map(|speed| f64::from(speed.as_millimeters_per_second())),
                },
            );
        } else if !highlighted.is_empty() {
            paint_route_segment(overlay, &highlighted, style, None);
            highlighted.clear();
        }
    }
    paint_route_segment(overlay, &highlighted, style, None);
}

fn paint_route_markers(
    overlay: &mut Ui,
    projector: &walkers::Projector,
    input: OverlayInput<'_, '_>,
) {
    if let Some((start, end)) = route_endpoints(input.route.samples) {
        paint_endpoint_marker(
            overlay,
            project_coordinate(projector, start, input.projection_center_longitude),
            Endpoint::Start,
            input.colors.start,
            input.colors.marker_fill,
        );
        paint_endpoint_marker(
            overlay,
            project_coordinate(projector, end, input.projection_center_longitude),
            Endpoint::End,
            input.colors.end,
            input.colors.marker_fill,
        );
    }
    if let Some((longitude, latitude)) = input.selected_coordinate {
        let position = projector
            .project(lon_lat(
                wrapped_longitude(longitude, input.projection_center_longitude),
                latitude,
            ))
            .to_pos2();
        overlay
            .painter()
            .circle_filled(position, 5.0, input.colors.marker_fill);
        overlay
            .painter()
            .circle_stroke(position, 5.0, Stroke::new(2.0, input.colors.route));
    }
}

fn project_coordinate(
    projector: &walkers::Projector,
    coordinate: Coordinate,
    projection_center_longitude: f64,
) -> egui::Pos2 {
    projector
        .project(lon_lat(
            wrapped_longitude(
                coordinate.longitude().as_degrees(),
                projection_center_longitude,
            ),
            coordinate.latitude().as_degrees(),
        ))
        .to_pos2()
}

fn set_route_cursor(overlay: &Ui, response: &egui::Response, hovered: Option<usize>) {
    let cursor = if response.is_pointer_button_down_on() {
        egui::CursorIcon::Grabbing
    } else if hovered.is_some() {
        egui::CursorIcon::Crosshair
    } else if response.hovered() {
        egui::CursorIcon::Grab
    } else {
        return;
    };
    overlay.ctx().set_cursor_icon(cursor);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ZoomPolicy {
    gesture_enabled: bool,
    speed: f64,
}

fn zoom_policy(ui: &mut Ui, zoom: f64, map_rect: Rect) -> ZoomPolicy {
    let (gesture, from_scroll) = zoom_gesture(ui);
    let pointer_over_map = ui.input(|input| {
        input
            .pointer
            .hover_pos()
            .is_some_and(|pointer| map_rect.contains(pointer))
    });
    let blocked = pointer_over_map && outward_zoom_at_bound(zoom, gesture);

    if blocked && from_scroll {
        ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
    }

    ZoomPolicy {
        gesture_enabled: !blocked,
        speed: clamped_zoom_speed(zoom, gesture, DEFAULT_ZOOM_SPEED),
    }
}

fn zoom_gesture(ui: &Ui) -> (f64, bool) {
    let zoom_delta = f64::from(ui.input(egui::InputState::zoom_delta));
    if (zoom_delta - 1.0).abs() > f64::EPSILON {
        return (zoom_delta - 1.0, false);
    }

    let gesture = f64::from(ui.input(|input| {
        input.smooth_scroll_delta.y
            * input
                .stable_dt
                .clamp(input.predicted_dt * 0.5, input.predicted_dt * 2.0)
            / 4.0
    }));
    (gesture, true)
}

fn outward_zoom_at_bound(zoom: f64, gesture: f64) -> bool {
    (gesture > 0.0 && zoom >= f64::from(MAX_VIEW_ZOOM) - ZOOM_BOUND_EPSILON)
        || (gesture < 0.0 && zoom <= ZOOM_BOUND_EPSILON)
}

fn clamped_zoom_speed(zoom: f64, gesture: f64, default_speed: f64) -> f64 {
    if gesture > 0.0 {
        default_speed.min((f64::from(MAX_VIEW_ZOOM) - zoom).max(0.0) / gesture)
    } else if gesture < 0.0 {
        default_speed.min(zoom.max(0.0) / -gesture)
    } else {
        default_speed
    }
}

fn attribution(ui: &mut Ui, source: &Attribution, performance: Option<&str>) {
    let palette = crate::theme::palette(ui);
    let color = crate::theme::color32(palette.content().text_helper());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        crate::theme::CONTROL_RADIUS,
        palette.surfaces().background().into_cint(),
    );
    let content_rect = Rect::from_min_max(
        rect.min + egui::vec2(4.0, 0.0),
        rect.max - egui::vec2(4.0, 0.0),
    );
    let mut attribution_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::right_to_left(egui::Align::Center)),
    );
    attribution_ui.add(
        egui::Hyperlink::from_label_and_url(
            RichText::new(source.text).size(8.0).color(color),
            source.url,
        )
        .open_in_new_tab(true),
    );
    if let Some(performance) = performance {
        attribution_ui.label(RichText::new(performance).size(8.0).color(color));
    }
}

fn map_status(ui: &mut Ui, map_rect: Rect, label: &str) {
    let size = egui::vec2(map_rect.width().min(240.0), 28.0);
    let rect = Rect::from_min_size(
        egui::pos2(map_rect.left() + 4.0, map_rect.top() + 48.0),
        size,
    );
    let mut status_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(egui::Align::Center)),
    );
    egui::Frame::new()
        .fill(crate::theme::color32(
            crate::theme::palette(&status_ui)
                .surfaces()
                .layer(garmin_color::theme::Level::Two),
        ))
        .corner_radius(crate::theme::CONTROL_RADIUS)
        .inner_margin(4)
        .show(&mut status_ui, |ui| {
            ui.label(RichText::new(label).small());
        });
}

#[derive(Clone, Copy)]
struct RoutePoint {
    index: usize,
    position: egui::Pos2,
    speed: Option<f64>,
}

fn push_route_point(points: &mut Vec<RoutePoint>, point: RoutePoint) {
    if points.len() > 1
        && points.get(points.len() - 2).is_some_and(|anchor| {
            anchor.position.distance_sq(point.position) < ROUTE_POINT_SPACING.powi(2)
        })
    {
        if let Some(last) = points.last_mut() {
            *last = point;
        }
    } else {
        points.push(point);
    }
}

#[derive(Clone, Copy)]
struct RouteStyle {
    width: f32,
    outline_width: f32,
    fallback: egui::Color32,
    outline: egui::Color32,
    speed_bounds: Option<(f64, f64)>,
    opacity: f32,
}

fn paint_route_segment(
    ui: &mut Ui,
    points: &[RoutePoint],
    style: RouteStyle,
    hit_test: Option<&mut HitTest>,
) {
    if points.len() < 2 {
        return;
    }
    paint_route_outline(
        ui,
        points,
        Stroke::new(
            style.outline_width,
            style
                .outline
                .gamma_multiply(style.opacity.max(map_style::MINIMUM_OUTLINE_OPACITY)),
        ),
        hit_test,
    );
    paint_speed_segments(
        ui,
        points,
        style.width,
        style.fallback,
        style.speed_bounds,
        style.opacity,
    );
}

fn paint_route_outline(
    ui: &mut Ui,
    points: &[RoutePoint],
    stroke: Stroke,
    hit_test: Option<&mut HitTest>,
) {
    hit_test_route(ui, points, hit_test, stroke.width);
    let clip = ui.clip_rect().expand(stroke.width);
    let mut visible = Vec::new();
    for pair in points.windows(2) {
        if clip.intersects(Rect::from_two_pos(pair[0].position, pair[1].position)) {
            if visible
                .last()
                .is_none_or(|position| *position != pair[0].position)
            {
                paint_visible_segment(ui, std::mem::take(&mut visible), stroke);
                visible.push(pair[0].position);
            }
            visible.push(pair[1].position);
        } else {
            paint_visible_segment(ui, std::mem::take(&mut visible), stroke);
        }
    }
    paint_visible_segment(ui, visible, stroke);
}

fn hit_test_route(ui: &Ui, points: &[RoutePoint], hit_test: Option<&mut HitTest>, width: f32) {
    let Some(hit_test) = hit_test else {
        return;
    };
    let Some(pointer) = hit_test.pointer else {
        return;
    };
    let clip = ui.clip_rect().expand(width);
    for pair in points.windows(2) {
        if !clip.intersects(Rect::from_two_pos(pair[0].position, pair[1].position)) {
            continue;
        }
        let (index, distance) = closest_endpoint_on_segment(
            pointer,
            (pair[0].index, pair[0].position),
            (pair[1].index, pair[1].position),
        );
        if distance < hit_test.nearest {
            hit_test.nearest = distance;
            hit_test.hovered = Some(index);
        }
    }
}

fn paint_speed_segments(
    ui: &Ui,
    points: &[RoutePoint],
    width: f32,
    fallback: egui::Color32,
    speed_bounds: Option<(f64, f64)>,
    opacity: f32,
) {
    let clip = ui.clip_rect().expand(width);
    for pair in points.windows(2) {
        if !clip.intersects(Rect::from_two_pos(pair[0].position, pair[1].position)) {
            continue;
        }
        let color = map_style::speed_fraction(pair[0].speed, pair[1].speed, speed_bounds)
            .map_or(fallback, map_style::speed_color)
            .gamma_multiply(opacity);
        ui.painter().line_segment(
            [pair[0].position, pair[1].position],
            Stroke::new(width, color),
        );
    }
}

fn paint_visible_segment(ui: &Ui, points: Vec<egui::Pos2>, stroke: Stroke) {
    if points.len() >= 2 {
        ui.painter().add(Shape::line(points, stroke));
    }
}

fn speed_bounds(samples: &[ActivitySampleSnapshot]) -> Option<(f64, f64)> {
    let mut speeds = samples.iter().filter_map(|sample| {
        sample
            .speed
            .map(|speed| f64::from(speed.as_millimeters_per_second()))
            .filter(|speed| speed.is_finite())
    });
    let first = speeds.next()?;
    let (minimum, maximum) = speeds.fold((first, first), |(minimum, maximum), speed| {
        (minimum.min(speed), maximum.max(speed))
    });
    (maximum - minimum > f64::EPSILON).then_some((minimum, maximum))
}

fn route_endpoints(samples: &[ActivitySampleSnapshot]) -> Option<(Coordinate, Coordinate)> {
    let start = samples.iter().find_map(|sample| sample.coordinate)?;
    let end = samples.iter().rev().find_map(|sample| sample.coordinate)?;
    Some((start, end))
}

#[derive(Clone, Copy)]
enum Endpoint {
    Start,
    End,
}

fn paint_endpoint_marker(
    ui: &Ui,
    position: egui::Pos2,
    endpoint: Endpoint,
    color: egui::Color32,
    symbol: egui::Color32,
) {
    ui.painter().circle_filled(position, 8.0, symbol);
    ui.painter().circle_filled(position, 6.5, color);
    match endpoint {
        Endpoint::Start => {
            ui.painter().add(Shape::convex_polygon(
                vec![
                    position + egui::vec2(-1.5, -3.0),
                    position + egui::vec2(3.0, 0.0),
                    position + egui::vec2(-1.5, 3.0),
                ],
                egui::Color32::WHITE,
                Stroke::NONE,
            ));
        }
        Endpoint::End => {
            ui.painter().rect_filled(
                Rect::from_center_size(position, Vec2::splat(5.0)),
                0.5,
                egui::Color32::WHITE,
            );
        }
    }
}

#[derive(Default)]
struct FrameTiming {
    previous: Option<Instant>,
    smoothed_milliseconds: Option<f32>,
    ui_milliseconds: VecDeque<f32>,
}

#[derive(Clone, Copy)]
struct MapPerfSample {
    ui_elapsed: Duration,
    scene_milliseconds: f32,
    route_query_microseconds: f32,
    label_milliseconds: f32,
    label_backlog: usize,
    stale_work: u64,
    visible_tiles: usize,
    ready_tiles: usize,
    pending_tiles: usize,
    queued_upload_bytes: usize,
    uploaded_bytes: usize,
}

impl FrameTiming {
    fn sample(&mut self, sample: MapPerfSample) -> Option<String> {
        if !cfg!(debug_assertions) {
            return None;
        }
        let MapPerfSample {
            ui_elapsed,
            scene_milliseconds,
            route_query_microseconds,
            label_milliseconds,
            label_backlog,
            stale_work,
            visible_tiles,
            ready_tiles,
            pending_tiles,
            queued_upload_bytes,
            uploaded_bytes,
        } = sample;
        let now = Instant::now();
        let elapsed = self.previous.replace(now).map(|previous| now - previous);
        if let Some(elapsed) = elapsed.filter(|elapsed| *elapsed <= Duration::from_millis(250)) {
            let milliseconds = elapsed.as_secs_f32() * 1_000.0;
            self.smoothed_milliseconds =
                Some(self.smoothed_milliseconds.map_or(milliseconds, |current| {
                    current.mul_add(0.85, milliseconds * 0.15)
                }));
        }
        self.ui_milliseconds
            .push_back(ui_elapsed.as_secs_f32() * 1_000.0);
        if self.ui_milliseconds.len() > 120 {
            self.ui_milliseconds.pop_front();
        }
        let (ui_p50, ui_p95) = percentiles(&self.ui_milliseconds)?;
        self.smoothed_milliseconds
            .filter(|milliseconds| *milliseconds > 0.0)
            .map(|milliseconds| {
                format!(
                    "{:.0} FPS · UI {ui_p50:.1}/{ui_p95:.1} ms · scene {scene_milliseconds:.2} ms · labels {label_milliseconds:.1} ms/{label_backlog} · route {route_query_microseconds:.0} µs · tiles {visible_tiles}/{ready_tiles}+{pending_tiles} · upload {uploaded_bytes}/{queued_upload_bytes} B · stale {stale_work}",
                    1_000.0 / milliseconds,
                )
            })
    }
}

fn percentiles(samples: &VecDeque<f32>) -> Option<(f32, f32)> {
    let mut sorted = samples.iter().copied().collect::<Vec<_>>();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable_by(f32::total_cmp);
    let value = |percent: usize| {
        let index = (sorted.len() - 1) * percent / 100;
        sorted[index]
    };
    Some((value(50), value(95)))
}

struct HitTest {
    pointer: Option<egui::Pos2>,
    hovered: Option<usize>,
    nearest: f32,
}

fn closest_endpoint_on_segment(
    pointer: egui::Pos2,
    start: (usize, egui::Pos2),
    end: (usize, egui::Pos2),
) -> (usize, f32) {
    let delta = end.1 - start.1;
    let length_squared = delta.length_sq();
    let fraction = if length_squared > 0.0 {
        ((pointer - start.1).dot(delta) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let projected = start.1 + delta * fraction;
    let index = if fraction < 0.5 { start.0 } else { end.0 };
    (index, pointer.distance_sq(projected))
}

pub(super) struct Output {
    pub hovered: Option<usize>,
    pub clicked: Option<usize>,
    pub empty_clicked: bool,
    pub rect: Rect,
}

struct RouteInteraction {
    hovered: Option<usize>,
    clicked: Option<usize>,
    empty_clicked: bool,
}

fn ranged_samples(
    samples: &[ActivitySampleSnapshot],
    range: std::ops::RangeInclusive<usize>,
) -> (usize, &[ActivitySampleSnapshot]) {
    let start = (*range.start()).min(samples.len());
    let end = range.end().saturating_add(1).min(samples.len()).max(start);
    (start, &samples[start..end])
}

fn center(samples: &[ActivitySampleSnapshot]) -> Option<walkers::Position> {
    let sample = samples.iter().find(|sample| sample.coordinate.is_some())?;
    let coordinate = sample.coordinate?;
    Some(lon_lat(
        coordinate.longitude().as_degrees(),
        coordinate.latitude().as_degrees(),
    ))
}

fn fit(memory: &mut MapMemory, samples: &[ActivitySampleSnapshot], size: Vec2) {
    let mut points = samples.iter().filter_map(|sample| sample.coordinate);
    let Some(first) = points.next() else {
        return;
    };
    let mut previous = first.longitude().as_degrees();
    let mut longitude = previous;
    let mut min_x = longitude / 360.0;
    let mut max_x = min_x;
    let first_y = mercator_y(first.latitude().as_degrees());
    let mut min_y = first_y;
    let mut max_y = first_y;
    for point in points {
        let raw = point.longitude().as_degrees();
        let delta = (raw - previous + 540.0).rem_euclid(360.0) - 180.0;
        longitude += delta;
        previous = raw;
        let x = longitude / 360.0;
        let y = mercator_y(point.latitude().as_degrees());
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let center_x = f64::midpoint(min_x, max_x);
    let center_y = f64::midpoint(min_y, max_y);
    let center_lon = (center_x * 360.0 + 180.0).rem_euclid(360.0) - 180.0;
    let center_lat = mercator_latitude(center_y);
    memory.center_at(lon_lat(center_lon, center_lat));
    let usable_width = f64::from((size.x - 48.0).max(1.0));
    let usable_height = f64::from((size.y - 48.0).max(1.0));
    let scale_x = usable_width / ((max_x - min_x).abs().max(1.0e-9) * f64::from(WALKERS_TILE_SIZE));
    let scale_y =
        usable_height / ((max_y - min_y).abs().max(1.0e-9) * f64::from(WALKERS_TILE_SIZE));
    let zoom = scale_x
        .min(scale_y)
        .log2()
        .clamp(1.0, f64::from(MAX_VIEW_ZOOM));
    let _ignored = memory.set_zoom(zoom);
}

fn mercator_y(latitude: f64) -> f64 {
    let latitude = latitude.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    (1.0 - latitude.tan().asinh() / std::f64::consts::PI) / 2.0
}

fn mercator_latitude(y: f64) -> f64 {
    (std::f64::consts::PI * (1.0 - 2.0 * y))
        .sinh()
        .atan()
        .to_degrees()
}

fn wrapped_longitude(longitude: f64, reference: f64) -> f64 {
    reference + (longitude - reference + 540.0).rem_euclid(360.0) - 180.0
}

fn empty_map(ui: &mut Ui, size: Vec2, message: &str) -> Output {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        rect,
        crate::theme::PANEL_RADIUS,
        palette
            .surfaces()
            .layer(garmin_color::theme::Level::One)
            .into_cint(),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        message,
        egui::TextStyle::Body.resolve(ui.style()),
        crate::theme::color32(palette.content().text_secondary()),
    );
    Output {
        hovered: None,
        clicked: None,
        empty_clicked: response.clicked(),
        rect,
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, MouseWheelUnit, RawInput, Rect, TouchPhase, pos2, vec2};
    use garmin_model::{
        route::{Coordinate, Latitude, Longitude},
        value::Timestamp,
    };
    use garmin_service_api::ActivitySampleSnapshot;
    use walkers::{Map, MapMemory, Tiles as _, lon_lat};

    use super::{
        FitState, RouteIndexCache, RoutePoint, TileStore, center, clamped_zoom_speed,
        closest_endpoint_on_segment, fit, mercator_latitude, mercator_y, outward_zoom_at_bound,
        push_route_point, ranged_samples, wrapped_longitude, zoom_policy,
    };

    fn sample(coordinate: Option<(f64, f64)>) -> ActivitySampleSnapshot {
        ActivitySampleSnapshot {
            timestamp: Timestamp::from_unix_milliseconds(0).unwrap(),
            coordinate: coordinate.map(|(latitude, longitude)| {
                Coordinate::from_parts(
                    Latitude::from_degrees(latitude).unwrap(),
                    Longitude::from_degrees(longitude).unwrap(),
                )
            }),
            elevation_meters: None,
            distance: None,
            speed: None,
            heart_rate: None,
            cadence: None,
            power: None,
            temperature_millicelsius: None,
        }
    }

    #[test]
    fn mercator_fit_round_trips_latitude() {
        for latitude in [-80.0, -45.0, 0.0, 60.0, 80.0] {
            assert!((mercator_latitude(mercator_y(latitude)) - latitude).abs() < 1.0e-9);
        }
    }

    #[test]
    fn fit_state_owns_first_fit_resize_reuse_and_explicit_requests() {
        let samples = [sample(Some((60.0, 24.0))), sample(Some((60.1, 24.2)))];
        let mut memory = MapMemory::default();
        let mut state = FitState::default();

        state.apply_if_needed(&mut memory, &samples, "activity", vec2(400.0, 300.0));
        assert_eq!(state.applications, 1);
        state.apply_if_needed(&mut memory, &samples, "activity", vec2(440.0, 340.0));
        assert_eq!(state.applications, 1);
        state.apply_if_needed(&mut memory, &samples, "activity", vec2(480.0, 380.0));
        assert_eq!(state.applications, 2);
        state.request();
        state.apply_if_needed(&mut memory, &samples, "activity", vec2(480.0, 380.0));
        assert_eq!(state.applications, 3);
        state.apply_if_needed(&mut memory, &samples, "other", vec2(480.0, 380.0));
        assert_eq!(state.applications, 4);
    }

    #[test]
    fn route_index_cache_rebuilds_only_for_a_new_source_identity() {
        let samples = [sample(Some((60.0, 24.0))), sample(Some((60.1, 24.2)))];
        let mut cache = RouteIndexCache::default();

        cache.resolve(&samples, 0, "activity:all");
        assert_eq!(cache.builds, 1);
        let _current = cache.current();
        cache.resolve(&samples, 0, "activity:all");
        assert_eq!(cache.builds, 1);
        cache.resolve(&samples, 4, "activity:lap-2");
        assert_eq!(cache.builds, 2);
    }

    #[test]
    fn route_hit_testing_uses_the_line_between_samples() {
        let (index, distance) = closest_endpoint_on_segment(
            pos2(75.0, 4.0),
            (4, pos2(0.0, 0.0)),
            (5, pos2(100.0, 0.0)),
        );

        assert_eq!(index, 5);
        assert!((distance - 16.0).abs() < f32::EPSILON);
    }

    #[test]
    fn route_simplification_keeps_stable_anchors() {
        let mut points = Vec::new();
        let mut x = 0.0;
        for index in 0..=100 {
            push_route_point(
                &mut points,
                RoutePoint {
                    index,
                    position: pos2(x, 0.0),
                    speed: None,
                },
            );
            x += 1.0;
        }

        assert!(points.len() > 25);
        assert_eq!(points.first().map(|point| point.index), Some(0));
        assert_eq!(points.last().map(|point| point.index), Some(100));
    }

    #[test]
    fn zoom_policy_stops_at_bounds_without_overshooting() {
        let maximum = f64::from(super::MAX_VIEW_ZOOM);
        assert!(outward_zoom_at_bound(maximum, 0.25));
        assert!(outward_zoom_at_bound(0.0, -0.25));
        assert!(!outward_zoom_at_bound(maximum, -0.25));
        assert!(!outward_zoom_at_bound(0.0, 0.25));
        assert!(clamped_zoom_speed(maximum, 0.25, 2.0).abs() < f64::EPSILON);
        assert!(clamped_zoom_speed(0.0, -0.25, 2.0).abs() < f64::EPSILON);
        assert!((clamped_zoom_speed(maximum, -0.25, 2.0) - 2.0).abs() < f64::EPSILON);
        assert!((clamped_zoom_speed(maximum - 0.1, 0.2, 2.0) - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn outward_wheel_at_maximum_is_consumed_without_moving_the_map() {
        let context = egui::Context::default();
        let mut memory = MapMemory::default();
        let map_center = lon_lat(27.2, 60.6);
        memory.center_at(map_center);
        memory.set_zoom(f64::from(super::MAX_VIEW_ZOOM)).unwrap();
        let center_before = memory.detached().unwrap();
        let mut scroll_before_policy = 0.0;
        let mut scroll_after_policy = f32::NAN;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 240.0))),
            events: vec![
                Event::PointerMoved(pos2(160.0, 120.0)),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: vec2(0.0, 120.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: TouchPhase::Move,
                },
            ],
            ..RawInput::default()
        };

        let output = context.run_ui(input, |ui| {
            let map_rect = ui.available_rect_before_wrap();
            scroll_before_policy = ui.input(|input| input.smooth_scroll_delta.y);
            let policy = zoom_policy(ui, memory.zoom(), map_rect);
            scroll_after_policy = ui.input(|input| input.smooth_scroll_delta.y);
            let _map = Map::new(None, &mut memory, map_center)
                .zoom_with_ctrl(false)
                .zoom_gesture(policy.gesture_enabled)
                .zoom_speed(policy.speed)
                .show(ui, |_, _, _, _| ());
        });
        output.drop_without_applying_deltas();

        let center_after = memory.detached().unwrap();
        assert!(scroll_before_policy > 0.0);
        assert!(scroll_after_policy.abs() < f32::EPSILON);
        assert!((memory.zoom() - f64::from(super::MAX_VIEW_ZOOM)).abs() < f64::EPSILON);
        assert!((center_after.x() - center_before.x()).abs() < f64::EPSILON);
        assert!((center_after.y() - center_before.y()).abs() < f64::EPSILON);
    }

    #[test]
    fn route_projection_wraps_longitudes_near_the_fitted_center() {
        assert!((wrapped_longitude(179.0, -180.0) + 181.0).abs() < f64::EPSILON);
        assert!((wrapped_longitude(-179.0, 180.0) - 181.0).abs() < f64::EPSILON);
    }

    #[test]
    fn route_fit_uses_the_short_dateline_span() {
        let samples = vec![sample(Some((60.0, 179.0))), sample(Some((60.1, -179.0)))];
        let (_, samples) = ranged_samples(&samples, 0..=1);
        let mut memory = MapMemory::default();

        fit(&mut memory, samples, egui::vec2(640.0, 320.0));

        let fitted = memory.detached().unwrap();
        assert!((fitted.x().abs() - 180.0).abs() < 1.0e-9);
        assert!(memory.zoom() > 1.0);
    }

    #[test]
    fn recordings_without_coordinates_do_not_create_a_map_center() {
        let samples = vec![sample(None), sample(None)];
        let (_, samples) = ranged_samples(&samples, 0..=1);
        let mut memory = MapMemory::default();

        assert!(center(samples).is_none());
        fit(&mut memory, samples, egui::vec2(640.0, 320.0));
        assert!(memory.detached().is_none());
    }

    #[test]
    fn tile_requests_respect_the_ui_concurrency_cap() {
        let mut tiles = TileStore::default();
        for x in 0..8 {
            let _piece = tiles.at(walkers::TileId { zoom: 4, x, y: 6 });
        }
        tiles.schedule_requests();

        assert_eq!(tiles.requests.len(), super::MAX_IN_FLIGHT);
        assert_eq!(tiles.pending_len(), super::MAX_IN_FLIGHT);
    }

    #[test]
    fn tile_requests_are_coalesced_and_prioritised_from_the_visible_center() {
        let mut tiles = TileStore::default();
        for x in 0..8 {
            let id = walkers::TileId { zoom: 4, x, y: 6 };
            let _first = tiles.at(id);
            let _duplicate = tiles.at(id);
        }

        tiles.schedule_requests();

        let requested_x = tiles
            .requests
            .iter()
            .map(|request| request.x)
            .collect::<Vec<_>>();
        assert_eq!(requested_x.len(), super::MAX_IN_FLIGHT);
        assert_eq!(&requested_x[..2], &[3, 4]);
        assert!(!requested_x.contains(&0));
        assert!(!requested_x.contains(&7));
    }

    #[test]
    fn least_recently_used_completed_tiles_are_evicted_at_the_cache_limit() {
        let mut tiles = TileStore::default();
        let limit = u32::try_from(super::DECODED_TILE_LIMIT).unwrap();
        for x in 0..=limit {
            let id = walkers::TileId { zoom: 9, x, y: 6 };
            tiles.entries.insert(id, super::TileEntry::Empty);
            tiles.promote(id);
        }

        assert_eq!(tiles.entries.len(), super::DECODED_TILE_LIMIT);
        assert!(!tiles.entries.contains_key(&walkers::TileId {
            zoom: 9,
            x: 0,
            y: 6,
        }));
    }

    #[test]
    fn failed_tiles_retry_with_bounded_backoff() {
        let mut attempts = 0;
        let mut delays = Vec::new();
        for _ in 0..11 {
            let (failure, delay) = super::Failure::after(attempts);
            attempts = failure.attempts;
            delays.push(delay);
        }

        assert_eq!(delays[0], std::time::Duration::from_secs(1));
        assert_eq!(delays[1], std::time::Duration::from_secs(2));
        assert_eq!(delays[10], std::time::Duration::from_secs(30));
    }

    #[test]
    fn empty_tiles_are_remembered_without_re_requesting() {
        let mut tiles = TileStore::default();
        let id = walkers::TileId {
            zoom: 1,
            x: 0,
            y: 0,
        };
        assert!(tiles.at(id).is_none());
        tiles.schedule_requests();
        let request = tiles.requests.pop().unwrap();
        tiles.resolve(
            &egui::Context::default(),
            super::MapTileResponse {
                request,
                result: Ok(super::MapTilePayload::Empty),
            },
        );

        assert!(tiles.at(id).is_none());
        assert!(tiles.requests.is_empty());
        assert!(matches!(
            tiles.entries.get(&id),
            Some(super::TileEntry::Empty)
        ));
    }

    #[test]
    fn stale_theme_response_does_not_cancel_the_current_request() {
        let mut tiles = TileStore::default();
        let id = walkers::TileId {
            zoom: 4,
            x: 8,
            y: 6,
        };
        assert!(tiles.at(id).is_none());
        tiles.schedule_requests();
        let stale = tiles.requests.pop().unwrap();

        tiles.set_theme(false);
        assert!(tiles.at(id).is_none());
        tiles.schedule_requests();
        assert_eq!(tiles.pending_len(), 1);
        tiles.resolve(
            &egui::Context::default(),
            super::MapTileResponse {
                request: stale,
                result: Ok(super::MapTilePayload::Empty),
            },
        );

        assert_eq!(tiles.pending_len(), 1);
        assert!(matches!(
            tiles.entries.get(&id),
            Some(super::TileEntry::Requested { .. })
        ));
    }

    #[test]
    fn same_theme_from_an_old_generation_cannot_complete_a_new_request() {
        let mut tiles = TileStore::default();
        let id = walkers::TileId {
            zoom: 4,
            x: 8,
            y: 6,
        };
        assert!(tiles.at(id).is_none());
        tiles.schedule_requests();
        let stale = tiles.requests.pop().unwrap();

        tiles.set_theme(false);
        tiles.set_theme(true);
        assert!(tiles.at(id).is_none());
        tiles.schedule_requests();
        tiles.resolve(
            &egui::Context::default(),
            super::MapTileResponse {
                request: stale,
                result: Ok(super::MapTilePayload::Empty),
            },
        );

        assert_eq!(tiles.pending_len(), 1);
        assert!(matches!(
            tiles.entries.get(&id),
            Some(super::TileEntry::Requested { .. })
        ));
    }

    #[test]
    fn a_success_does_not_hide_another_visible_failed_tile() {
        let mut tiles = TileStore::default();
        let failed_id = walkers::TileId {
            zoom: 4,
            x: 8,
            y: 6,
        };
        let ready_id = walkers::TileId {
            zoom: 4,
            x: 9,
            y: 6,
        };
        assert!(tiles.at(failed_id).is_none());
        assert!(tiles.at(ready_id).is_none());
        tiles.schedule_requests();
        let failed_request = tiles
            .requests
            .iter()
            .copied()
            .find(|request| request.x == failed_id.x)
            .unwrap();
        let ready_request = tiles
            .requests
            .iter()
            .copied()
            .find(|request| request.x == ready_id.x)
            .unwrap();

        tiles.resolve(
            &egui::Context::default(),
            super::MapTileResponse {
                request: failed_request,
                result: Err("provider unavailable".to_owned()),
            },
        );
        assert!(tiles.visible_background_unavailable());
        tiles.resolve(
            &egui::Context::default(),
            super::MapTileResponse {
                request: ready_request,
                result: Ok(super::MapTilePayload::Empty),
            },
        );

        assert!(tiles.visible_background_unavailable());
    }
}
