//! Production UI assembly must capture the current scene, not the preceding frame.

#![expect(
    clippy::cast_possible_truncation,
    reason = "bounded geographic fixture coordinates become egui marker positions"
)]

use super::*;
use crate::activity::map::{ActivityMap, Props};
use egui::{Color32, Pos2, Shape, pos2};
use garmin_model::{
    route::{Coordinate, Latitude, Longitude},
    value::Timestamp,
};
use garmin_service_api::{ActivityRecordingSnapshot, ActivitySampleSnapshot};

const MARKER: Color32 = Color32::from_rgb(12, 34, 56);

struct CapturingPainter(Pos2);

impl ScenePainter for CapturingPainter {
    fn is_gpu(&self) -> bool {
        true
    }

    fn update(&mut self, frame: SceneFrame<'_, '_>) -> super::super::map::gpu_map::ScenePerf {
        self.0 = pos2(
            frame.camera.center().x() as f32,
            frame.camera.center().y() as f32,
        );
        super::super::map::gpu_map::ScenePerf::default()
    }

    fn paint_callback(&self, _rect: egui::Rect) -> Option<Shape> {
        Some(Shape::circle_filled(self.0, 3.0, MARKER))
    }

    fn paint_labels(&mut self, _frame: LabelFrame<'_>) {}
}

struct CapturingRenderer;

impl RendererFactory for CapturingRenderer {
    fn painter(&self, _metrics: MapMetrics) -> Box<dyn ScenePainter> {
        Box::new(CapturingPainter(Pos2::ZERO))
    }

    fn prepare_tile(
        &self,
        id: walkers::TileId,
        tile: walkers::Tile,
    ) -> Result<PreparedTile, String> {
        SoftwareRenderer.prepare_tile(id, tile)
    }

    fn prepare_browser_tile(
        &self,
        packet: BrowserTilePacket,
    ) -> Result<Option<Arc<super::super::map::PreparedGpuTile>>, String> {
        SoftwareRenderer.prepare_browser_tile(packet)
    }
}

struct NoTransport;

impl Backend for NoTransport {
    fn submit(&self, _task: TileTask) {}
}

#[test]
fn activity_map_captures_current_camera_on_first_frame_and_after_refit() {
    let runtime = MapRuntimeHandle::new(NoTransport, Renderer(Arc::new(CapturingRenderer)));
    let mut map = ActivityMap::new(&runtime);
    let context = egui::Context::default();
    for (key, longitude, latitude) in [("first", 10.0, 20.0), ("second", 30.0, 40.0)] {
        let recording = ActivityRecordingSnapshot {
            samples: vec![ActivitySampleSnapshot {
                timestamp: Timestamp::from_unix_milliseconds(0).unwrap(),
                coordinate: Some(Coordinate::from_parts(
                    Latitude::from_degrees(latitude).unwrap(),
                    Longitude::from_degrees(longitude).unwrap(),
                )),
                elevation_meters: None,
                distance: None,
                speed: None,
                heart_rate: None,
                cadence: None,
                power: None,
                temperature_millicelsius: None,
            }],
            laps: Vec::new(),
            timer_events: Vec::new(),
        };
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| {
                map.show(
                    ui,
                    &Props {
                        label: "Activity map",
                        recording: &recording,
                        selected_coordinate: None,
                        sample_range: 0..=0,
                        highlighted_range: None,
                        fit_key: key,
                        empty: "empty",
                        loading_background: "loading",
                        background_unavailable: "unavailable",
                        height: 256.0,
                    },
                );
            },
        );
        let centers: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Circle(circle) if circle.fill == MARKER => Some(circle.center),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        assert_eq!(centers.len(), 1, "one captured scene for {key}");
        assert!(
            centers[0].distance(pos2(longitude as f32, latitude as f32)) < 0.001,
            "{key} captured stale camera {:?}",
            centers[0]
        );
    }
}
