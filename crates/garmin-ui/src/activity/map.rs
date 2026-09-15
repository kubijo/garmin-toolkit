//! Host-fed `walkers` activity map and linked route overlay.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use cint::ColorInterop;
use egui::{Align2, Layout, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2, pos2};
use garmin_model::route::Coordinate;
use garmin_service_api::{ActivityRecordingSnapshot, ActivitySampleSnapshot};
use walkers::{Map, MapMemory, Tile, TileId, TilePiece, Tiles, lon_lat, sources::Attribution};

const DECODED_TILE_LIMIT: usize = 256;
const MAX_IN_FLIGHT: usize = 6;
const SOURCE_TILE_SIZE: u32 = 512;
const WALKERS_TILE_SIZE: u32 = 256;
const MAX_TILE_ZOOM: u8 = 14;
const MAX_VIEW_ZOOM: u8 = MAX_TILE_ZOOM + 1;
const ROUTE_WIDTH: f32 = 3.0;
const ROUTE_POINT_SPACING: f32 = 1.5;
const SPEED_COLOR_BUCKETS: u8 = 8;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Transport-neutral XYZ request emitted by the shared UI.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MapTileRequest {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
    dark_mode: bool,
}

/// Host result fed back into the shared UI.
pub struct MapTileResponse {
    request: MapTileRequest,
    result: Result<MapTilePayload, String>,
}

enum MapTilePayload {
    Empty,
    Encoded(Vec<u8>),
    Decoded(Tile),
}

impl MapTileResponse {
    /// Preserve encoded bytes for hosts which cannot decode away from the UI thread.
    #[must_use]
    pub fn encoded(request: MapTileRequest, result: Result<Vec<u8>, String>) -> Self {
        Self {
            request,
            result: result.map(|bytes| {
                if bytes.is_empty() {
                    MapTilePayload::Empty
                } else {
                    MapTilePayload::Encoded(bytes)
                }
            }),
        }
    }
}

/// Reusable vector-tile decoder for hosts with a background worker.
pub struct MapTileDecoder {
    dark: walkers::Style,
    light: walkers::Style,
}

impl Default for MapTileDecoder {
    fn default() -> Self {
        Self {
            dark: map_style(true),
            light: map_style(false),
        }
    }
}

impl MapTileDecoder {
    /// Decode a vector tile on the caller's thread before it reaches the UI.
    #[must_use]
    pub fn decode(
        &self,
        request: MapTileRequest,
        result: Result<Vec<u8>, String>,
    ) -> MapTileResponse {
        let result = result.and_then(|bytes| {
            if bytes.is_empty() {
                Ok(MapTilePayload::Empty)
            } else {
                Tile::from_mvt(
                    &bytes,
                    if request.dark_mode {
                        &self.dark
                    } else {
                        &self.light
                    },
                    request.zoom,
                    SOURCE_TILE_SIZE,
                )
                .map(MapTilePayload::Decoded)
                .map_err(|error| error.to_string())
            }
        });
        MapTileResponse { request, result }
    }
}

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
    tiles: TileStore,
    memory: MapMemory,
    fit_key: String,
    fit_size: Vec2,
    force_fit: bool,
    frame_timing: FrameTiming,
}

impl Default for ActivityMap {
    fn default() -> Self {
        Self {
            tiles: TileStore::default(),
            memory: MapMemory::default(),
            fit_key: String::new(),
            fit_size: Vec2::ZERO,
            force_fit: false,
            frame_timing: FrameTiming::default(),
        }
    }
}

impl ActivityMap {
    #[expect(
        clippy::too_many_lines,
        reason = "map composition keeps the base map and its synchronized route overlay in one render pass"
    )]
    pub fn show(&mut self, ui: &mut Ui, props: &Props<'_>) -> Output {
        let size = Vec2::new(ui.available_width(), props.height);
        self.tiles.set_theme(ui.visuals().dark_mode);
        let (sample_offset, samples) =
            ranged_samples(&props.recording.samples, props.sample_range.clone());
        let Some(center) = center(samples) else {
            return empty_map(ui, size, props.empty);
        };
        if self.force_fit
            || self.fit_key != props.fit_key
            || (self.fit_size.x - size.x).abs() > 64.0
            || (self.fit_size.y - size.y).abs() > 64.0
        {
            fit(&mut self.memory, samples, size);
            self.fit_key.clear();
            self.fit_key.push_str(props.fit_key);
            self.fit_size = size;
            self.force_fit = false;
        }

        let route_color = crate::theme::color32(crate::theme::selection_accent(ui));
        let marker_fill = crate::theme::color32(crate::theme::palette(ui).surfaces().background());
        let route_outline = egui::Color32::from_black_alpha(190);
        let start_color = crate::theme::color32(crate::theme::palette(ui).support().success());
        let end_color = crate::theme::color32(crate::theme::palette(ui).support().error());
        let speed_bounds = speed_bounds(samples);
        let projection_center_longitude = self
            .memory
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
        let inner =
            ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |map_ui| {
                map_ui.set_min_size(size);
                Map::new(Some(&mut self.tiles), &mut self.memory, center)
                    .zoom_with_ctrl(false)
                    .panning(true)
                    .show(map_ui, |overlay, response, projector, _memory| {
                        overlay.set_clip_rect(overlay.clip_rect().intersect(response.rect));
                        let pointer = response.hover_pos();
                        let mut hit_test = HitTest {
                            pointer,
                            hovered: None,
                            nearest: 14.0_f32.powi(2),
                        };
                        let route_style = RouteStyle {
                            width: ROUTE_WIDTH,
                            fallback: route_color,
                            outline: route_outline,
                            speed_bounds,
                            opacity: if props.highlighted_range.is_some() {
                                0.32
                            } else {
                                1.0
                            },
                        };
                        let mut current = Vec::with_capacity(samples.len());
                        for (offset, sample) in samples.iter().enumerate() {
                            let sample_index = sample_offset + offset;
                            if let Some(coordinate) = sample.coordinate {
                                let position = projector
                                    .project(lon_lat(
                                        wrapped_longitude(
                                            coordinate.longitude().as_degrees(),
                                            projection_center_longitude,
                                        ),
                                        coordinate.latitude().as_degrees(),
                                    ))
                                    .to_pos2();
                                push_route_point(
                                    &mut current,
                                    RoutePoint {
                                        index: sample_index,
                                        position,
                                        speed: sample.speed.map(|speed| {
                                            f64::from(speed.as_millimeters_per_second())
                                        }),
                                    },
                                );
                            } else {
                                paint_route_segment(
                                    overlay,
                                    &current,
                                    route_style,
                                    Some(&mut hit_test),
                                );
                                current.clear();
                            }
                        }
                        paint_route_segment(overlay, &current, route_style, Some(&mut hit_test));
                        if let Some(highlighted_range) = &props.highlighted_range {
                            let mut highlighted = Vec::with_capacity(
                                highlighted_range
                                    .end()
                                    .saturating_sub(*highlighted_range.start())
                                    .saturating_add(1)
                                    .min(samples.len()),
                            );
                            for (offset, sample) in samples.iter().enumerate() {
                                let sample_index = sample_offset + offset;
                                if highlighted_range.contains(&sample_index) {
                                    if let Some(coordinate) = sample.coordinate {
                                        push_route_point(
                                            &mut highlighted,
                                            RoutePoint {
                                                index: sample_index,
                                                position: projector
                                                    .project(lon_lat(
                                                        wrapped_longitude(
                                                            coordinate.longitude().as_degrees(),
                                                            projection_center_longitude,
                                                        ),
                                                        coordinate.latitude().as_degrees(),
                                                    ))
                                                    .to_pos2(),
                                                speed: sample.speed.map(|speed| {
                                                    f64::from(speed.as_millimeters_per_second())
                                                }),
                                            },
                                        );
                                    } else {
                                        paint_route_segment(
                                            overlay,
                                            &highlighted,
                                            RouteStyle {
                                                width: ROUTE_WIDTH + 1.5,
                                                opacity: 1.0,
                                                ..route_style
                                            },
                                            None,
                                        );
                                        highlighted.clear();
                                    }
                                } else if !highlighted.is_empty() {
                                    paint_route_segment(
                                        overlay,
                                        &highlighted,
                                        RouteStyle {
                                            width: ROUTE_WIDTH + 1.5,
                                            opacity: 1.0,
                                            ..route_style
                                        },
                                        None,
                                    );
                                    highlighted.clear();
                                }
                            }
                            paint_route_segment(
                                overlay,
                                &highlighted,
                                RouteStyle {
                                    width: ROUTE_WIDTH + 1.5,
                                    opacity: 1.0,
                                    ..route_style
                                },
                                None,
                            );
                        }
                        if let Some((start, end)) = route_endpoints(samples) {
                            let start = projector
                                .project(lon_lat(
                                    wrapped_longitude(
                                        start.longitude().as_degrees(),
                                        projection_center_longitude,
                                    ),
                                    start.latitude().as_degrees(),
                                ))
                                .to_pos2();
                            let end = projector
                                .project(lon_lat(
                                    wrapped_longitude(
                                        end.longitude().as_degrees(),
                                        projection_center_longitude,
                                    ),
                                    end.latitude().as_degrees(),
                                ))
                                .to_pos2();
                            paint_endpoint_marker(
                                overlay,
                                start,
                                Endpoint::Start,
                                start_color,
                                marker_fill,
                            );
                            paint_endpoint_marker(
                                overlay,
                                end,
                                Endpoint::End,
                                end_color,
                                marker_fill,
                            );
                        }
                        if let Some((longitude, latitude)) = props.selected_coordinate {
                            let position = projector
                                .project(lon_lat(
                                    wrapped_longitude(longitude, projection_center_longitude),
                                    latitude,
                                ))
                                .to_pos2();
                            overlay.painter().circle_filled(position, 5.0, marker_fill);
                            overlay.painter().circle_stroke(
                                position,
                                5.0,
                                Stroke::new(2.0, route_color),
                            );
                        }
                        if response.is_pointer_button_down_on() {
                            overlay.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        } else if hit_test.hovered.is_some() {
                            overlay.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                        } else if response.hovered() {
                            overlay.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                        }
                        RouteInteraction {
                            hovered: hit_test.hovered,
                            clicked: response.clicked().then_some(hit_test.hovered).flatten(),
                            empty_clicked: response.clicked() && hit_test.hovered.is_none(),
                        }
                    })
                    .inner
            });
        if self.memory.zoom() > f64::from(MAX_VIEW_ZOOM) {
            let _ignored = self.memory.set_zoom(f64::from(MAX_VIEW_ZOOM));
            ui.ctx().request_repaint();
        }
        let rect = inner.response.rect;
        if self.tiles.background_unavailable {
            map_status(ui, rect, props.background_unavailable);
        }
        let performance = self.frame_timing.sample();
        attribution(ui, &self.tiles.attribution(), performance.as_deref());
        ui.spacing_mut().item_spacing.y = item_spacing;
        Output {
            hovered: inner.inner.hovered,
            clicked: inner.inner.clicked,
            empty_clicked: inner.inner.empty_clicked,
            rect,
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
        self.force_fit = true;
    }

    pub fn take_requests(&mut self) -> Vec<MapTileRequest> {
        std::mem::take(&mut self.tiles.requests)
    }

    pub fn resolve(&mut self, context: &egui::Context, response: MapTileResponse) {
        self.tiles.resolve(context, response);
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
        && points.last().is_some_and(|last| {
            last.position.distance_sq(point.position) < ROUTE_POINT_SPACING.powi(2)
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
            style.width + 2.0,
            style.outline.gamma_multiply(style.opacity.max(0.6)),
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
    mut hit_test: Option<&mut HitTest>,
) {
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
            if let Some(hit_test) = hit_test.as_deref_mut()
                && let Some(pointer) = hit_test.pointer
            {
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
        } else {
            paint_visible_segment(ui, std::mem::take(&mut visible), stroke);
        }
    }
    paint_visible_segment(ui, visible, stroke);
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
    let mut bucket = None;
    let mut visible = Vec::new();
    for pair in points.windows(2) {
        if !clip.intersects(Rect::from_two_pos(pair[0].position, pair[1].position)) {
            paint_speed_run(
                ui,
                std::mem::take(&mut visible),
                bucket,
                width,
                fallback,
                opacity,
            );
            bucket = None;
            continue;
        }
        let next_bucket = route_speed_bucket(pair[0].speed, pair[1].speed, speed_bounds);
        if bucket != Some(next_bucket)
            || visible
                .last()
                .is_some_and(|position| *position != pair[0].position)
        {
            paint_speed_run(
                ui,
                std::mem::take(&mut visible),
                bucket,
                width,
                fallback,
                opacity,
            );
            bucket = Some(next_bucket);
            visible.push(pair[0].position);
        }
        visible.push(pair[1].position);
    }
    paint_speed_run(ui, visible, bucket, width, fallback, opacity);
}

fn paint_speed_run(
    ui: &Ui,
    points: Vec<egui::Pos2>,
    bucket: Option<u8>,
    width: f32,
    fallback: egui::Color32,
    opacity: f32,
) {
    if points.len() < 2 {
        return;
    }
    let color = bucket
        .filter(|bucket| *bucket < SPEED_COLOR_BUCKETS)
        .map_or(fallback, speed_color)
        .gamma_multiply(opacity);
    ui.painter()
        .add(Shape::line(points, Stroke::new(width, color)));
}

fn paint_visible_segment(ui: &Ui, points: Vec<egui::Pos2>, stroke: Stroke) {
    if points.len() >= 2 {
        ui.painter().add(Shape::line(points, stroke));
    }
}

fn route_speed_bucket(start: Option<f64>, end: Option<f64>, bounds: Option<(f64, f64)>) -> u8 {
    let Some((minimum, maximum)) = bounds else {
        return SPEED_COLOR_BUCKETS;
    };
    let speed = match (start, end) {
        (Some(start), Some(end)) => f64::midpoint(start, end),
        (Some(speed), None) | (None, Some(speed)) => speed,
        (None, None) => return SPEED_COLOR_BUCKETS,
    };
    let fraction = ((speed - minimum) / (maximum - minimum)).clamp(0.0, 1.0);
    let position = fraction * f64::from(SPEED_COLOR_BUCKETS - 1);
    for bucket in 0..SPEED_COLOR_BUCKETS - 1 {
        if position < f64::from(bucket) + 0.5 {
            return bucket;
        }
    }
    SPEED_COLOR_BUCKETS - 1
}

fn speed_color(bucket: u8) -> egui::Color32 {
    const COLORS: [[u8; 3]; SPEED_COLOR_BUCKETS as usize] = [
        [45, 132, 255],
        [38, 162, 210],
        [39, 185, 165],
        [68, 188, 116],
        [129, 190, 80],
        [221, 190, 56],
        [240, 139, 58],
        [235, 72, 67],
    ];
    let [red, green, blue] = COLORS[usize::from(bucket.min(SPEED_COLOR_BUCKETS - 1))];
    egui::Color32::from_rgb(red, green, blue)
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
}

impl FrameTiming {
    fn sample(&mut self) -> Option<String> {
        if !cfg!(debug_assertions) {
            return None;
        }
        let now = Instant::now();
        let elapsed = self.previous.replace(now).map(|previous| now - previous);
        if let Some(elapsed) = elapsed.filter(|elapsed| *elapsed <= Duration::from_millis(250)) {
            let milliseconds = elapsed.as_secs_f32() * 1_000.0;
            self.smoothed_milliseconds =
                Some(self.smoothed_milliseconds.map_or(milliseconds, |current| {
                    current.mul_add(0.85, milliseconds * 0.15)
                }));
        }
        self.smoothed_milliseconds
            .filter(|milliseconds| *milliseconds > 0.0)
            .map(|milliseconds| format!("{:.0} FPS · {milliseconds:.1} ms", 1_000.0 / milliseconds))
    }
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

struct TileStore {
    tiles: HashMap<TileId, Tile>,
    empty: HashSet<TileId>,
    order: VecDeque<TileId>,
    pending: HashSet<TileId>,
    failed: HashMap<TileId, Failure>,
    requests: Vec<MapTileRequest>,
    style: walkers::Style,
    dark_mode: bool,
    background_unavailable: bool,
}

impl Default for TileStore {
    fn default() -> Self {
        Self {
            tiles: HashMap::new(),
            empty: HashSet::new(),
            order: VecDeque::new(),
            pending: HashSet::new(),
            failed: HashMap::new(),
            requests: Vec::new(),
            style: map_style(true),
            dark_mode: true,
            background_unavailable: false,
        }
    }
}

struct Failure {
    attempts: u8,
    retry_at: Instant,
}

impl TileStore {
    fn set_theme(&mut self, dark_mode: bool) {
        if self.dark_mode == dark_mode {
            return;
        }
        self.dark_mode = dark_mode;
        self.style = map_style(dark_mode);
        self.tiles.clear();
        self.empty.clear();
        self.order.clear();
        self.pending.clear();
        self.failed.clear();
        self.requests.clear();
    }

    fn resolve(&mut self, context: &egui::Context, response: MapTileResponse) {
        let id = TileId {
            zoom: response.request.zoom,
            x: response.request.x,
            y: response.request.y,
        };
        if response.request.dark_mode != self.dark_mode {
            return;
        }
        self.pending.remove(&id);
        let retry_after = match response.result {
            Ok(MapTilePayload::Empty) => {
                self.background_unavailable = false;
                self.failed.remove(&id);
                self.tiles.remove(&id);
                self.empty.insert(id);
                self.promote(id);
                None
            }
            Ok(MapTilePayload::Encoded(bytes)) => {
                match Tile::from_mvt(&bytes, &self.style, id.zoom, SOURCE_TILE_SIZE) {
                    Ok(tile) => {
                        self.background_unavailable = false;
                        self.failed.remove(&id);
                        self.empty.remove(&id);
                        self.tiles.insert(id, tile);
                        self.promote(id);
                        None
                    }
                    Err(_error) => {
                        self.background_unavailable = true;
                        Some(self.record_failure(id))
                    }
                }
            }
            Ok(MapTilePayload::Decoded(tile)) => {
                self.background_unavailable = false;
                self.failed.remove(&id);
                self.empty.remove(&id);
                self.tiles.insert(id, tile);
                self.promote(id);
                None
            }
            Err(_reason) => {
                self.background_unavailable = true;
                Some(self.record_failure(id))
            }
        };
        if let Some(delay) = retry_after {
            context.request_repaint_after(delay);
        }
        context.request_repaint();
    }

    fn record_failure(&mut self, id: TileId) -> Duration {
        let attempts = self
            .failed
            .get(&id)
            .map_or(1, |failure| failure.attempts.saturating_add(1));
        let shift = u32::from(attempts.saturating_sub(1).min(5));
        let delay = Duration::from_secs(1_u64 << shift).min(MAX_RETRY_DELAY);
        self.failed.insert(
            id,
            Failure {
                attempts,
                retry_at: Instant::now() + delay,
            },
        );
        if self.failed.len() > DECODED_TILE_LIMIT
            && let Some(oldest) = self
                .failed
                .iter()
                .min_by_key(|(_, failure)| failure.retry_at)
                .map(|(id, _)| *id)
        {
            self.failed.remove(&oldest);
        }
        delay
    }

    fn promote(&mut self, id: TileId) {
        self.order.retain(|candidate| *candidate != id);
        self.order.push_back(id);
        while self.order.len() > DECODED_TILE_LIMIT {
            if let Some(evicted) = self.order.pop_front() {
                self.tiles.remove(&evicted);
                self.empty.remove(&evicted);
            }
        }
    }
}

fn map_style(dark_mode: bool) -> walkers::Style {
    if dark_mode {
        walkers::Style::openmaptiles_basemap_dark()
    } else {
        walkers::Style::openmaptiles_basemap_light()
    }
}

impl Tiles for TileStore {
    fn at(&mut self, tile_id: TileId) -> Option<TilePiece> {
        if tile_id.zoom > MAX_TILE_ZOOM {
            return None;
        }
        if let Some(tile) = self.tiles.get(&tile_id).cloned() {
            self.promote(tile_id);
            return Some(TilePiece::new(
                tile,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            ));
        }
        if self.empty.contains(&tile_id) {
            self.promote(tile_id);
            return None;
        }
        let retry_ready = self
            .failed
            .get(&tile_id)
            .is_none_or(|failure| Instant::now() >= failure.retry_at);
        if self.pending.len() < MAX_IN_FLIGHT && !self.pending.contains(&tile_id) && retry_ready {
            self.pending.insert(tile_id);
            self.requests.push(MapTileRequest {
                zoom: tile_id.zoom,
                x: tile_id.x,
                y: tile_id.y,
                dark_mode: self.dark_mode,
            });
        }
        None
    }

    fn attribution(&self) -> Attribution {
        Attribution {
            text: "OpenFreeMap · © OpenMapTiles · © OpenStreetMap contributors",
            url: "https://openfreemap.org/",
            logo_light: None,
            logo_dark: None,
        }
    }

    fn tile_size(&self) -> u32 {
        SOURCE_TILE_SIZE
    }
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
    use egui::pos2;
    use garmin_model::{
        route::{Coordinate, Latitude, Longitude},
        value::Timestamp,
    };
    use garmin_service_api::ActivitySampleSnapshot;
    use walkers::{MapMemory, Tiles as _};

    use super::{
        TileStore, center, closest_endpoint_on_segment, fit, mercator_latitude, mercator_y,
        ranged_samples, wrapped_longitude,
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

        assert_eq!(tiles.requests.len(), super::MAX_IN_FLIGHT);
        assert_eq!(tiles.pending.len(), super::MAX_IN_FLIGHT);
    }

    #[test]
    fn failed_tiles_retry_with_bounded_backoff() {
        let mut tiles = TileStore::default();
        let id = walkers::TileId {
            zoom: 4,
            x: 8,
            y: 6,
        };

        assert_eq!(tiles.record_failure(id), std::time::Duration::from_secs(1));
        assert_eq!(tiles.record_failure(id), std::time::Duration::from_secs(2));
        for _ in 0..8 {
            let _ignored = tiles.record_failure(id);
        }
        assert_eq!(tiles.record_failure(id), std::time::Duration::from_secs(30));
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
        let request = tiles.requests.pop().unwrap();
        tiles.resolve(
            &egui::Context::default(),
            super::MapTileResponse::encoded(request, Ok(Vec::new())),
        );

        assert!(tiles.at(id).is_none());
        assert!(tiles.requests.is_empty());
        assert!(tiles.empty.contains(&id));
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
        let stale = tiles.requests.pop().unwrap();

        tiles.set_theme(false);
        assert!(tiles.at(id).is_none());
        assert!(tiles.pending.contains(&id));
        tiles.resolve(
            &egui::Context::default(),
            super::MapTileResponse::encoded(stale, Ok(Vec::new())),
        );

        assert!(tiles.pending.contains(&id));
        assert!(!tiles.empty.contains(&id));
    }
}
