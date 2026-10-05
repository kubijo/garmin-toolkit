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
    fn paint_callback(&self, _rect: egui::Rect) -> Option<Shape> {
        Some(Shape::circle_filled(self.0, 3.0, MARKER))
    }

    fn paint_labels(&self, _frame: LabelFrame<'_>) {}
}

impl SceneRuntime for CapturingPainter {
    fn is_gpu(&self) -> bool {
        true
    }

    fn prepare(&mut self, frame: SceneFrame<'_, '_>) -> PreparedScene {
        let position = pos2(
            frame.camera.center().x() as f32,
            frame.camera.center().y() as f32,
        );
        PreparedScene {
            painter: Box::new(Self(position)),
            performance: super::super::map::gpu_map::ScenePerf::default(),
        }
    }
}

struct CapturingRenderer;

impl RendererFactory for CapturingRenderer {
    fn runtime(&self, _metrics: MapMetrics) -> Box<dyn SceneRuntime> {
        Box::new(CapturingPainter(Pos2::ZERO))
    }

    fn prepare_tile(
        &self,
        id: walkers::TileId,
        tile: crate::activity::map::DecodedTile,
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
    fn fetch(&self, _request: TileCoordinates, _reply: TileReply) {}
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

#[test]
fn first_gpu_scene_reports_label_work_before_painting() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = futures_lite::future::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
    )
    .expect("map scene regression requires a WGPU adapter");
    let (device, _queue) =
        futures_lite::future::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .unwrap();
    let renderer = Renderer::wgpu(super::super::map::WgpuMapHandle::for_target(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        1,
    ));
    let id = walkers::TileId {
        zoom: 0,
        x: 0,
        y: 0,
    };
    let tile = renderer
        .prepare_tile(
            id,
            crate::activity::map::DecodedTile {
                shapes: vec![Shape::rect_filled(
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(512.0, 512.0)),
                    0.0,
                    Color32::BLUE,
                )],
                texts: Vec::new(),
            },
        )
        .unwrap();
    let scene = MapScene(Arc::new(MapSceneData {
        tiles: vec![SceneTile {
            id,
            tile: Arc::new(tile),
        }]
        .into_boxed_slice(),
        ready_tiles: 1,
        ..MapSceneData::default()
    }));
    let runtime = MapRuntimeHandle::new(NoTransport, renderer);
    let mut surface = runtime.surface();
    let camera = MapCamera::default();
    let viewport = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 240.0));
    let route = super::super::map::gpu_map::RouteScene {
        key: "empty",
        samples: &[],
        sample_offset: 0,
        highlighted_range: None,
        color: Color32::GREEN,
        outline: Color32::BLACK,
        opacity: 1.0,
    };
    let context = egui::Context::default();
    context
        .run_ui(egui::RawInput::default(), |_| {})
        .drop_without_applying_deltas();
    let performance = surface.update_scene(&scene, &camera, viewport, &context, &route);
    assert!(performance.visible_tiles > 0);
    assert_eq!(
        performance.label_backlog, 1,
        "a new label request belongs to this scene's readiness"
    );
    assert!(surface.paint_callback(viewport).is_some());
}
