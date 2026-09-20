//! Opt-in remote map boundary: immutable route revisions and small camera/selection updates.
//! Both hosts use the production tile runtime, GPU renderer, labels and marker painting.

use std::sync::Arc;

use egui::{Rect, Ui};
use garmin_service_api::ActivitySampleSnapshot;
use serde::{Deserialize, Serialize};

use super::{
    MapCamera, MapColors, MapProjector, MapSurfaceHandle, MapViewDemand, OverlayInput, gpu_map,
};
use crate::activity::map_runtime::MapRuntimeHandle;

const MAX_ROUTE_BYTES: usize =
    crate::activity::map_runtime::BrowserWorkerTaskKind::Route.result_byte_limit();
const MAX_ROUTE_SAMPLES: usize = MAX_ROUTE_BYTES / std::mem::size_of::<ActivitySampleSnapshot>();
const MAX_VIEW_BYTES: usize = 1536;

/// Main-thread integration. Browser objects must remain outside this typed plugin.
pub trait Host: Send + Sync + 'static {
    /// Replace the immutable route, once per source revision.
    fn route(&self, revision: u32, bytes: &[u8]);
    /// Replace unstarted camera/selection demand; never contains tile or label geometry.
    fn view(&self, json: &str);
    /// Fail only this experimental map, with no main-thread fallback.
    fn fail(&self, reason: &str);
    /// Current map-local failure, leaving the surrounding application usable.
    fn failure(&self) -> String;
}

/// Main-thread publisher. The source identity matches the existing route cache contract.
pub struct RemoteMapPlugin {
    host: Arc<dyn Host>,
    source: Option<(String, usize, usize, usize)>,
    revision: u32,
}

impl RemoteMapPlugin {
    /// Install only for the opt-in single-activity-map experiment.
    pub fn new(host: impl Host) -> Self {
        Self {
            host: Arc::new(host),
            source: None,
            revision: 0,
        }
    }

    pub(super) fn publish(
        &mut self,
        ui: &Ui,
        camera: &MapCamera,
        rect: Rect,
        route: &gpu_map::RouteScene<'_>,
        selected: Option<(f64, f64)>,
    ) {
        let source = (
            route.key.to_owned(),
            route.samples.as_ptr() as usize,
            route.samples.len(),
            route.sample_offset,
        );
        if self.source.as_ref() != Some(&source) {
            if route.samples.len() > MAX_ROUTE_SAMPLES {
                self.host
                    .fail("remote route exceeded its sample retention limit");
                return;
            }
            let Some(revision) = self.revision.checked_add(1) else {
                self.host.fail("remote route revision exhausted");
                return;
            };
            let bytes = match postcard::to_allocvec(route.samples) {
                Ok(bytes) if bytes.len() <= MAX_ROUTE_BYTES => bytes,
                _ => {
                    self.host.fail("remote route exceeded its transfer limit");
                    return;
                }
            };
            self.host.route(revision, &bytes);
            self.source = Some(source);
            self.revision = revision;
        }
        let view = View {
            revision: self.revision,
            center: [camera.center().x(), camera.center().y()],
            zoom: camera.zoom(),
            size: [rect.width(), rect.height()],
            pixels_per_point: ui.ctx().pixels_per_point(),
            dark: ui.visuals().dark_mode,
            offset: route.sample_offset,
            highlighted: route
                .highlighted_range
                .as_ref()
                .map(|r| [*r.start(), *r.end()]),
            selected: selected.map(|(x, y)| [x, y]),
        };
        match serde_json::to_string(&view) {
            Ok(json) if json.len() <= MAX_VIEW_BYTES => self.host.view(&json),
            _ => self
                .host
                .fail("remote map view exceeded its transfer limit"),
        }
    }

    pub(super) fn failure(&self) -> String {
        self.host.failure()
    }
}

impl egui::plugin::Plugin for RemoteMapPlugin {
    fn debug_name(&self) -> &'static str {
        "remote activity map"
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct View {
    revision: u32,
    center: [f64; 2],
    zoom: f64,
    size: [f32; 2],
    pixels_per_point: f32,
    dark: bool,
    offset: usize,
    highlighted: Option<[usize; 2]>,
    selected: Option<[f64; 2]>,
}

impl View {
    fn decode(json: &str) -> Result<Self, String> {
        if json.len() > MAX_VIEW_BYTES {
            return Err("remote view is too large".to_owned());
        }
        let view: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let coordinate = |v: [f64; 2]| {
            v[0].is_finite() && v[1].is_finite() && v[0].abs() <= 180.0 && v[1].abs() <= 90.0
        };
        if view.revision == 0
            || !coordinate(view.center)
            || view.selected.is_some_and(|v| !coordinate(v))
            || !view.zoom.is_finite()
            || !(0.0..=f64::from(super::MAX_VIEW_ZOOM)).contains(&view.zoom)
            || view
                .size
                .iter()
                .any(|v| !v.is_finite() || !(1.0..=32768.0).contains(v))
            || !view.pixels_per_point.is_finite()
            || !(0.25..=8.0).contains(&view.pixels_per_point)
            || view.offset > u32::MAX as usize
            || view
                .highlighted
                .is_some_and(|[a, b]| a > b || b > u32::MAX as usize)
        {
            return Err("invalid remote map view".to_owned());
        }
        Ok(view)
    }
}

/// Worker-owned production map state. No camera interaction or main-thread paint-list transfer.
pub struct MapSurface {
    surface: MapSurfaceHandle,
    samples: Vec<ActivitySampleSnapshot>,
    revision: u32,
    key: String,
    view: Option<View>,
}

impl MapSurface {
    /// Create using the worker's GPU device and direct preparation-worker backend.
    #[must_use]
    pub fn new(runtime: &MapRuntimeHandle) -> Self {
        Self {
            surface: runtime.surface(),
            samples: Vec::new(),
            revision: 0,
            key: String::new(),
            view: None,
        }
    }

    /// Atomically validate and replace a route/view pair; later frames reference its revision.
    /// # Errors
    /// Rejects invalid views, missing route revisions and oversized or malformed route data.
    pub fn update(&mut self, json: &str, route: Option<&[u8]>) -> Result<(), String> {
        let view = View::decode(json)?;
        if let Some(bytes) = route {
            if bytes.len() > MAX_ROUTE_BYTES || view.revision <= self.revision {
                return Err("invalid remote route replacement".to_owned());
            }
            let (count, _) =
                postcard::take_from_bytes::<usize>(bytes).map_err(|e| e.to_string())?;
            if count > MAX_ROUTE_SAMPLES {
                return Err("remote route exceeded its sample retention limit".to_owned());
            }
            let (samples, trailing) =
                postcard::take_from_bytes::<Vec<ActivitySampleSnapshot>>(bytes)
                    .map_err(|e| e.to_string())?;
            if !trailing.is_empty() {
                return Err("remote route has trailing data".to_owned());
            }
            self.samples = samples;
            self.revision = view.revision;
            self.key = format!("remote-route-{}", self.revision);
        } else if view.revision != self.revision {
            return Err("remote view references a missing route".to_owned());
        }
        self.view = Some(view);
        Ok(())
    }

    /// Current logical dimensions and theme for the worker-local egui renderer.
    #[must_use]
    pub fn viewport(&self) -> Option<([f32; 2], f32, bool)> {
        self.view
            .as_ref()
            .map(|v| (v.size, v.pixels_per_point, v.dark))
    }

    /// Paint only map content using the production scene and marker paths.
    pub fn paint(&mut self, ui: &mut Ui) {
        let Some(view) = &self.view else {
            return;
        };
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(view.size[0], view.size[1]));
        ui.set_clip_rect(rect);
        let mut camera = MapCamera::default();
        camera.center_at(walkers::lon_lat(view.center[0], view.center[1]));
        camera.set_zoom(view.zoom);
        let colors = MapColors::new(ui);
        let route = gpu_map::RouteScene {
            key: &self.key,
            samples: &self.samples,
            sample_offset: view.offset,
            highlighted_range: view.highlighted.map(|[a, b]| a..=b),
            color: colors.route,
            outline: colors.outline,
            opacity: if view.highlighted.is_some() {
                crate::activity::map_style::DIMMED_ROUTE_OPACITY
            } else {
                1.0
            },
        };
        self.surface
            .submit_view(ui.ctx(), view.dark, &MapViewDemand::new(&camera, rect));
        let scene = self.surface.scene();
        super::paint_scene(&mut self.surface, &scene, &camera, rect, ui, &route);
        super::paint_route_markers(
            ui,
            &MapProjector::new(&camera, rect),
            OverlayInput {
                projection_center_longitude: camera.center().x(),
                gpu_enabled: true,
                route: &route,
                colors,
                selected_coordinate: view.selected.map(|[x, y]| (x, y)),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::map_runtime::{Backend, Renderer, TileTask};

    struct NoTiles;
    impl Backend for NoTiles {
        fn submit(&self, task: TileTask) {
            task.complete_encoded(Err("no test tiles".to_owned()));
        }
    }

    fn view(revision: u32) -> String {
        serde_json::to_string(&View {
            revision,
            center: [-0.1, 51.5],
            zoom: 10.0,
            size: [600.0, 400.0],
            pixels_per_point: 1.0,
            dark: true,
            offset: 0,
            highlighted: None,
            selected: None,
        })
        .unwrap()
    }

    #[test]
    fn views_require_a_valid_route_revision_and_failed_updates_preserve_the_scene() {
        let runtime = MapRuntimeHandle::new(NoTiles, Renderer::software());
        let mut surface = MapSurface::new(&runtime);
        assert!(surface.update(&view(1), None).is_err());
        let empty = postcard::to_allocvec(&Vec::<ActivitySampleSnapshot>::new()).unwrap();
        surface.update(&view(1), Some(&empty)).unwrap();
        surface.update(&view(1), None).unwrap();
        assert!(surface.update(&view(1), Some(&empty)).is_err());
        assert!(surface.update(&view(2), Some(&[0xff])).is_err());
        assert_eq!(surface.revision, 1);
        assert_eq!(surface.view.as_ref().unwrap().revision, 1);
        surface.update(&view(2), Some(&empty)).unwrap();
        assert_eq!(surface.revision, 2);
        assert!(surface.update(&view(1), None).is_err());
    }

    #[test]
    fn route_length_is_checked_before_allocating_samples() {
        let runtime = MapRuntimeHandle::new(NoTiles, Renderer::software());
        let mut surface = MapSurface::new(&runtime);
        let excessive_count = postcard::to_allocvec(&(MAX_ROUTE_SAMPLES + 1)).unwrap();
        assert!(
            surface
                .update(&view(1), Some(&excessive_count))
                .unwrap_err()
                .contains("retention limit")
        );
        assert!(
            surface
                .update(&view(1), Some(&[0, 42]))
                .unwrap_err()
                .contains("trailing data")
        );
        assert_eq!(surface.revision, 0);
    }

    #[test]
    fn malformed_camera_selection_and_size_are_rejected() {
        for (key, value) in [
            ("revision", serde_json::json!(0)),
            ("center", serde_json::json!([200.0, 51.5])),
            ("selected", serde_json::json!([0.0, 91.0])),
            ("zoom", serde_json::json!(100.0)),
            ("size", serde_json::json!([0.0, 400.0])),
            ("pixels_per_point", serde_json::json!(0.0)),
            ("highlighted", serde_json::json!([8, 2])),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut input: serde_json::Value = serde_json::from_str(&view(1)).unwrap();
            input[key] = value;
            assert!(View::decode(&input.to_string()).is_err(), "accepted {key}");
        }
        assert!(View::decode(&" ".repeat(MAX_VIEW_BYTES + 1)).is_err());
        assert!(view(1).len() < 2048);
    }
}
