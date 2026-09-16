//! Camera interaction, projection, and tile-demand calculation.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "bounded zoom, tile-grid, and screen coordinates intentionally cross numeric domains"
)]

use egui::{PointerButton, Pos2, Rect, Response, Ui, Vec2};
use walkers::{Position, TileId, lon_lat};

use super::{MAX_VIEW_ZOOM, WALKERS_TILE_SIZE, mercator_latitude, mercator_y};

const DEFAULT_ZOOM: f64 = 16.0;
const MIN_ZOOM: f64 = 0.0;
const INERTIA_TAU_SECONDS: f32 = 0.2;
const INERTIA_STOP_PIXELS: f32 = 0.1;
const SOURCE_ZOOM_OFFSET: u8 = 1;
const PREFETCH_RING: i64 = 1;

#[derive(Debug, Clone)]
pub(in crate::activity) struct MapCamera {
    center: Position,
    zoom: f64,
    motion: CameraMotion,
}

#[derive(Debug, Clone, Copy, Default)]
enum CameraMotion {
    #[default]
    Idle,
    Dragging {
        delta: Vec2,
    },
    Inertia {
        direction: Vec2,
        amount: f32,
    },
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CameraInteraction {
    pub(super) active: bool,
    pub(super) changed: bool,
}

impl Default for MapCamera {
    fn default() -> Self {
        Self {
            center: lon_lat(0.0, 0.0),
            zoom: DEFAULT_ZOOM,
            motion: CameraMotion::Idle,
        }
    }
}

impl MapCamera {
    pub(in crate::activity) fn center(&self) -> Position {
        self.center
    }

    pub(in crate::activity) fn zoom(&self) -> f64 {
        self.zoom
    }

    pub(super) fn center_at(&mut self, position: Position) {
        self.center = canonical_position(position);
        self.motion = CameraMotion::Idle;
    }

    pub(super) fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom.clamp(MIN_ZOOM, f64::from(MAX_VIEW_ZOOM));
        self.motion = CameraMotion::Idle;
    }

    pub(super) fn zoom_by(&mut self, delta: f64) {
        self.set_zoom(self.zoom + delta);
    }

    pub(super) fn animating(&self) -> bool {
        matches!(self.motion, CameraMotion::Inertia { .. })
    }

    /// Advance camera-only state before deriving tile demand.
    pub(super) fn interact(
        &mut self,
        ui: &mut Ui,
        response: &Response,
        zoom_delta: f64,
    ) -> CameraInteraction {
        let mut changed = false;
        let mut active = false;

        if zoom_delta.abs() > f64::EPSILON {
            let anchor = ui
                .input(|input| input.multi_touch().map(|touch| touch.center_pos))
                .or_else(|| response.hover_pos())
                .unwrap_or(response.rect.center());
            changed |= self.zoom_around(anchor, response.rect, zoom_delta);
            active |= changed;
        } else if response.dragged_by(PointerButton::Primary) {
            let delta = response.drag_delta();
            self.motion = CameraMotion::Dragging { delta };
            active = true;
            if delta != Vec2::ZERO {
                self.shift_by_screen(delta);
                changed = true;
            }
        } else if response.drag_stopped_by(PointerButton::Primary)
            && let CameraMotion::Dragging { delta } = self.motion
        {
            active = self.finish_drag(delta);
        }

        let delta_time = ui.input(|input| input.stable_dt);
        if self.advance_inertia(delta_time) {
            changed = true;
            active = true;
        }

        if changed {
            ui.ctx().request_repaint();
        }
        CameraInteraction { active, changed }
    }

    fn finish_drag(&mut self, delta: Vec2) -> bool {
        let amount = delta.length();
        self.motion = if amount > INERTIA_STOP_PIXELS {
            CameraMotion::Inertia {
                direction: delta / amount,
                amount,
            }
        } else {
            CameraMotion::Idle
        };
        matches!(self.motion, CameraMotion::Inertia { .. })
    }

    fn advance_inertia(&mut self, delta_time: f32) -> bool {
        if let CameraMotion::Inertia { direction, amount } = self.motion {
            if amount < INERTIA_STOP_PIXELS {
                self.motion = CameraMotion::Idle;
            } else {
                self.shift_by_screen(direction * amount);
                let low_pass = INERTIA_TAU_SECONDS / (delta_time + INERTIA_TAU_SECONDS);
                self.motion = CameraMotion::Inertia {
                    direction,
                    amount: amount * low_pass,
                };
                return true;
            }
        }
        false
    }

    fn zoom_around(&mut self, anchor: Pos2, viewport: Rect, delta: f64) -> bool {
        let next_zoom = (self.zoom + delta).clamp(MIN_ZOOM, f64::from(MAX_VIEW_ZOOM));
        if (next_zoom - self.zoom).abs() <= f64::EPSILON {
            return false;
        }
        let before_world_size = world_size(self.zoom);
        let after_world_size = world_size(next_zoom);
        let offset = anchor - viewport.center();
        let center = self.center_normalized();
        let anchor_world = [
            center[0] + f64::from(offset.x) / before_world_size,
            center[1] + f64::from(offset.y) / before_world_size,
        ];
        self.zoom = next_zoom;
        self.set_center_normalized([
            anchor_world[0] - f64::from(offset.x) / after_world_size,
            anchor_world[1] - f64::from(offset.y) / after_world_size,
        ]);
        self.motion = CameraMotion::Idle;
        true
    }

    fn shift_by_screen(&mut self, delta: Vec2) {
        let size = world_size(self.zoom);
        let center = self.center_normalized();
        self.set_center_normalized([
            center[0] - f64::from(delta.x) / size,
            center[1] - f64::from(delta.y) / size,
        ]);
    }

    pub(in crate::activity) fn center_normalized(&self) -> [f64; 2] {
        [self.center.x() / 360.0 + 0.5, mercator_y(self.center.y())]
    }

    fn set_center_normalized(&mut self, center: [f64; 2]) {
        let longitude = ((center[0] - 0.5) * 360.0 + 180.0).rem_euclid(360.0) - 180.0;
        let latitude = mercator_latitude(center[1].clamp(0.0, 1.0));
        self.center = lon_lat(longitude, latitude);
    }
}

fn canonical_position(position: Position) -> Position {
    lon_lat(
        (position.x() + 180.0).rem_euclid(360.0) - 180.0,
        position.y().clamp(-85.051_128_78, 85.051_128_78),
    )
}

pub(in crate::activity) fn world_size(zoom: f64) -> f64 {
    f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(zoom)
}

/// Return the horizontally wrapped copy of a tile nearest the camera center.
pub(in crate::activity) fn tile_x_near_center(id: TileId, center_x: f64) -> f64 {
    let tile_count = 2.0_f64.powi(i32::from(id.zoom));
    let x = f64::from(id.x);
    x + ((center_x * tile_count - x) / tile_count).round() * tile_count
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MapProjector {
    viewport: Rect,
    center: [f64; 2],
    world_size: f64,
}

impl MapProjector {
    pub(super) fn new(camera: &MapCamera, viewport: Rect) -> Self {
        Self {
            viewport,
            center: camera.center_normalized(),
            world_size: world_size(camera.zoom()),
        }
    }

    pub(super) fn project(&self, position: Position) -> Pos2 {
        let x = position.x() / 360.0 + 0.5;
        let x = self.center[0] + (x - self.center[0] + 0.5).rem_euclid(1.0) - 0.5;
        egui::pos2(
            self.viewport.center().x + ((x - self.center[0]) * self.world_size) as f32,
            self.viewport.center().y
                + ((mercator_y(position.y()) - self.center[1]) * self.world_size) as f32,
        )
    }

    #[cfg(test)]
    pub(super) fn unproject(&self, position: Pos2) -> Position {
        let offset = position - self.viewport.center();
        let normalized = [
            self.center[0] + f64::from(offset.x) / self.world_size,
            self.center[1] + f64::from(offset.y) / self.world_size,
        ];
        lon_lat(
            ((normalized[0] - 0.5) * 360.0 + 180.0).rem_euclid(360.0) - 180.0,
            mercator_latitude(normalized[1]),
        )
    }
}

/// Camera snapshot which replaces, rather than appends to, pending tile demand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::activity) struct MapViewDemand {
    center: [f64; 2],
    zoom: f64,
    viewport_size: Vec2,
}

impl MapViewDemand {
    pub(super) fn new(camera: &MapCamera, viewport: Rect) -> Self {
        Self {
            center: camera.center_normalized(),
            zoom: camera.zoom(),
            viewport_size: viewport.size(),
        }
    }

    pub(in crate::activity) fn coverage(self) -> MapTileCoverage {
        let rounded_zoom = self.zoom.round().clamp(0.0, f64::from(MAX_VIEW_ZOOM)) as u8;
        let source_zoom = rounded_zoom
            .saturating_sub(SOURCE_ZOOM_OFFSET)
            .min(MAX_VIEW_ZOOM - 1);
        let tile_count = i64::from(1_u32 << u32::from(source_zoom));
        let tile_screen_size =
            f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(self.zoom - f64::from(source_zoom));
        let center_tile = [
            self.center[0] * tile_count as f64,
            self.center[1] * tile_count as f64,
        ];
        let half = [
            f64::from(self.viewport_size.x) / (2.0 * tile_screen_size),
            f64::from(self.viewport_size.y) / (2.0 * tile_screen_size),
        ];
        let bounds = TileBounds {
            min_x: (center_tile[0] - half[0]).floor() as i64,
            max_x: (center_tile[0] + half[0]).floor() as i64,
            min_y: (center_tile[1] - half[1]).floor() as i64,
            max_y: (center_tile[1] + half[1]).floor() as i64,
        };
        let visible = bounds.tiles(source_zoom, tile_count);
        let requested = bounds.expand(PREFETCH_RING).tiles(source_zoom, tile_count);
        MapTileCoverage {
            visible,
            requested,
            center: center_tile,
        }
    }

    #[cfg(test)]
    pub(in crate::activity) fn from_tiles(visible: &[TileId]) -> Self {
        let first = visible.first().copied().unwrap_or(TileId {
            zoom: 0,
            x: 0,
            y: 0,
        });
        let tile_count = 2.0_f64.powi(i32::from(first.zoom));
        let min_x = visible.iter().map(|tile| tile.x).min().unwrap_or(0);
        let max_x = visible.iter().map(|tile| tile.x).max().unwrap_or(0);
        let min_y = visible.iter().map(|tile| tile.y).min().unwrap_or(0);
        let max_y = visible.iter().map(|tile| tile.y).max().unwrap_or(0);
        Self {
            center: [
                (f64::midpoint(f64::from(min_x), f64::from(max_x)) + 0.5) / tile_count,
                (f64::midpoint(f64::from(min_y), f64::from(max_y)) + 0.5) / tile_count,
            ],
            zoom: f64::from(first.zoom + SOURCE_ZOOM_OFFSET),
            viewport_size: Vec2::new(
                (max_x - min_x + 1) as f32 * 256.0 * 0.99,
                (max_y - min_y + 1) as f32 * 256.0 * 0.99,
            ),
        }
    }
}

pub(in crate::activity) struct MapTileCoverage {
    pub(in crate::activity) visible: Vec<TileId>,
    pub(in crate::activity) requested: Vec<TileId>,
    pub(in crate::activity) center: [f64; 2],
}

#[derive(Debug, Clone, Copy)]
struct TileBounds {
    min_x: i64,
    max_x: i64,
    min_y: i64,
    max_y: i64,
}

impl TileBounds {
    fn expand(self, amount: i64) -> Self {
        Self {
            min_x: self.min_x - amount,
            max_x: self.max_x + amount,
            min_y: self.min_y - amount,
            max_y: self.max_y + amount,
        }
    }

    fn tiles(self, zoom: u8, tile_count: i64) -> Vec<TileId> {
        let mut tiles = Vec::new();
        for y in self.min_y.max(0)..=self.max_y.min(tile_count - 1) {
            for x in self.min_x..=self.max_x {
                let wrapped_x = x.rem_euclid(tile_count);
                let id = TileId {
                    zoom,
                    x: u32::try_from(wrapped_x).expect("wrapped tile x fits u32"),
                    y: u32::try_from(y).expect("clamped tile y fits u32"),
                };
                if !tiles.contains(&id) {
                    tiles.push(id);
                }
            }
        }
        tiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips_across_the_dateline() {
        let mut camera = MapCamera::default();
        camera.center_at(lon_lat(179.5, 60.0));
        camera.set_zoom(8.0);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 320.0));
        let projector = MapProjector::new(&camera, viewport);
        let original = lon_lat(-179.8, 60.1);

        let round_trip = projector.unproject(projector.project(original));

        assert!((round_trip.x() - original.x()).abs() < 1.0e-5);
        assert!((round_trip.y() - original.y()).abs() < 1.0e-5);
    }

    #[test]
    fn demand_adds_one_prefetch_ring_without_duplicates() {
        let mut camera = MapCamera::default();
        camera.center_at(lon_lat(0.0, 0.0));
        camera.set_zoom(5.0);
        let coverage = MapViewDemand::new(
            &camera,
            Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)),
        )
        .coverage();

        assert!(!coverage.visible.is_empty());
        assert!(coverage.requested.len() > coverage.visible.len());
        assert_eq!(
            coverage
                .requested
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            coverage.requested.len()
        );
        assert!(
            coverage
                .visible
                .iter()
                .all(|tile| coverage.requested.contains(tile))
        );
    }

    #[test]
    fn zoom_around_pointer_preserves_the_anchored_coordinate() {
        let mut camera = MapCamera::default();
        camera.center_at(lon_lat(24.0, 60.0));
        camera.set_zoom(10.0);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 320.0));
        let pointer = egui::pos2(510.0, 90.0);
        let before = MapProjector::new(&camera, viewport).unproject(pointer);

        assert!(camera.zoom_around(pointer, viewport, 1.25));

        let after = MapProjector::new(&camera, viewport).unproject(pointer);
        assert!((before.x() - after.x()).abs() < 1.0e-9);
        assert!((before.y() - after.y()).abs() < 1.0e-9);
    }

    #[test]
    fn tile_projection_uses_the_nearest_dateline_copy() {
        let western_tile = TileId {
            zoom: 4,
            x: 0,
            y: 7,
        };

        assert!((tile_x_near_center(western_tile, 0.99) - 16.0).abs() < f64::EPSILON);
        assert!(tile_x_near_center(western_tile, 0.01).abs() < f64::EPSILON);
    }

    #[test]
    fn inertia_moves_and_decays_without_tile_state() {
        let mut camera = MapCamera::default();
        camera.center_at(lon_lat(24.0, 60.0));
        camera.set_zoom(10.0);
        camera.motion = CameraMotion::Inertia {
            direction: Vec2::X,
            amount: 20.0,
        };
        let before = camera.center();

        assert!(camera.advance_inertia(1.0 / 60.0));

        assert_ne!(camera.center(), before);
        let CameraMotion::Inertia { amount, .. } = camera.motion else {
            panic!("camera stopped before inertia decayed");
        };
        assert!(amount < 20.0);
    }

    #[test]
    fn a_stationary_drag_release_does_not_extend_interaction_telemetry() {
        let mut camera = MapCamera::default();

        assert!(!camera.finish_drag(Vec2::ZERO));
        assert!(matches!(camera.motion, CameraMotion::Idle));
        assert!(camera.finish_drag(Vec2::splat(1.0)));
        assert!(matches!(camera.motion, CameraMotion::Inertia { .. }));
    }
}
