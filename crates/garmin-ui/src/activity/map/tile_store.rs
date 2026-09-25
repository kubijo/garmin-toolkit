//! Vector-tile decoding, demand scheduling, retry, and bounded scene storage.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use walkers::{Tile, TileId, sources::Attribution};
use web_time::Instant;

use crate::activity::{
    map::camera::MapViewDemand,
    map::gpu_map,
    map_runtime::{BrowserTilePacket, BrowserTileTransfer, Renderer, TileCoordinates},
};

pub(super) const DECODED_TILE_LIMIT: usize = 256;
pub(super) const MAX_IN_FLIGHT: usize = 6;
pub(in crate::activity) const SOURCE_TILE_SIZE: u32 = 512;
pub(in crate::activity) const WALKERS_TILE_SIZE: u32 = 256;
const MAX_TILE_ZOOM: u8 = 14;
pub(super) const MAX_VIEW_ZOOM: u8 = MAX_TILE_ZOOM + 1;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

type MapTileRequest = TileCoordinates;

pub(in crate::activity) struct MapTileResponse {
    pub(super) request: MapTileRequest,
    pub(super) result: Result<MapTilePayload, String>,
}

pub(super) enum MapTilePayload {
    Empty,
    Decoded(PreparedTile),
}

pub(in crate::activity) struct PreparedTile {
    pub(in crate::activity) tile: Tile,
    pub(in crate::activity) gpu: Option<std::sync::Arc<gpu_map::PreparedGpuTile>>,
}

impl PreparedTile {
    fn new(renderer: &Renderer, id: TileId, tile: Tile) -> Result<Self, String> {
        renderer.prepare_tile(id, tile)
    }
}

impl MapTileResponse {
    /// Reconstruct a tile prepared by the browser map worker.
    #[must_use]
    pub(in crate::activity) fn prepared(
        request: MapTileRequest,
        result: Result<BrowserTilePacket, String>,
        renderer: &Renderer,
    ) -> Self {
        let result = result.and_then(|packet| {
            renderer.prepare_browser_tile(packet).map(|gpu| {
                gpu.map_or(MapTilePayload::Empty, |gpu| {
                    MapTilePayload::Decoded(PreparedTile {
                        tile: Tile::Vector {
                            shapes: Vec::new(),
                            texts: Vec::new(),
                        },
                        gpu: Some(gpu),
                    })
                })
            })
        });
        Self { request, result }
    }
}

/// Reusable vector-tile decoder for runtime backends with a preparation worker.
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
    pub(in crate::activity) fn decode(
        &self,
        request: MapTileRequest,
        result: Result<Vec<u8>, String>,
        renderer: &Renderer,
    ) -> MapTileResponse {
        let result = result.and_then(|bytes| {
            if bytes.is_empty() {
                Ok(MapTilePayload::Empty)
            } else {
                super::tile_decode::decode(
                    &bytes,
                    if request.dark_mode() {
                        &self.dark
                    } else {
                        &self.light
                    },
                    request.zoom,
                    SOURCE_TILE_SIZE,
                )
                .and_then(|tile| {
                    let id = TileId {
                        zoom: request.zoom,
                        x: request.x,
                        y: request.y,
                    };
                    PreparedTile::new(renderer, id, tile)
                })
                .map(MapTilePayload::Decoded)
            }
        });
        MapTileResponse { request, result }
    }
}

/// Decode, style, and tessellate an MVT payload into the browser worker wire format.
///
/// # Errors
///
/// Returns an error when the MVT payload is malformed or tessellation cannot be serialized.
pub fn prepare_tile_for_browser_worker(
    zoom: u8,
    dark_mode: bool,
    bytes: &[u8],
) -> Result<BrowserTileTransfer, String> {
    if bytes.is_empty() {
        return gpu_map::encode_browser_tile(Tile::Vector {
            shapes: Vec::new(),
            texts: Vec::new(),
        });
    }
    let style = map_style(dark_mode);
    let tile = super::tile_decode::decode(bytes, &style, zoom, SOURCE_TILE_SIZE)?;
    gpu_map::encode_browser_tile(tile)
}

pub(in crate::activity) struct TileStore {
    pub(super) entries: HashMap<TileId, TileEntry>,
    order: VecDeque<TileId>,
    pub(super) requests: Vec<MapTileRequest>,
    visible: HashSet<TileId>,
    demanded: HashSet<TileId>,
    demand_center: [f64; 2],
    dark_mode: bool,
    generation: u64,
    scene_revision: u64,
}

pub(super) enum TileEntry {
    Ready(Arc<PreparedTile>),
    Empty,
    Desired { attempts: u8 },
    Requested { attempts: u8, generation: u64 },
    Failed(Failure),
}

impl Default for TileStore {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            requests: Vec::new(),
            visible: HashSet::new(),
            demanded: HashSet::new(),
            demand_center: [0.0; 2],
            dark_mode: true,
            generation: 0,
            scene_revision: 0,
        }
    }
}

pub(super) struct Failure {
    pub(super) attempts: u8,
    retry_at: Instant,
    reason: String,
}

impl TileStore {
    pub(in crate::activity) fn set_theme(&mut self, dark_mode: bool) {
        if self.dark_mode == dark_mode {
            return;
        }
        self.dark_mode = dark_mode;
        self.generation = self.generation.wrapping_add(1);
        self.entries.clear();
        self.order.clear();
        self.requests.clear();
        self.visible.clear();
        self.demanded.clear();
        self.demand_center = [0.0; 2];
        self.mark_scene_changed();
    }

    /// Replace the current camera demand and discard work which has not started yet.
    pub(in crate::activity) fn apply_demand(&mut self, demand: &MapViewDemand) {
        let coverage = demand.coverage();
        self.apply_coverage(&coverage.visible, &coverage.requested, coverage.center);
    }

    fn apply_coverage(&mut self, visible: &[TileId], requested: &[TileId], center: [f64; 2]) {
        if self.visible.len() != visible.len()
            || visible.iter().any(|id| !self.visible.contains(id))
        {
            self.mark_scene_changed();
        }
        self.visible.clear();
        self.visible.extend(visible.iter().copied());
        self.demanded.clear();
        self.demanded.extend(requested.iter().copied());
        self.demand_center = center;

        self.entries.retain(|id, entry| {
            !matches!(entry, TileEntry::Desired { .. }) || self.demanded.contains(id)
        });
        for id in requested.iter().copied() {
            self.desire(id);
        }
        for id in visible.iter().copied() {
            if matches!(
                self.entries.get(&id),
                Some(TileEntry::Ready(_) | TileEntry::Empty)
            ) {
                self.promote(id);
            }
        }
    }

    #[cfg(test)]
    pub(in crate::activity) fn apply_exact_demand(&mut self, visible: &[TileId]) {
        let count =
            f64::from(u32::try_from(visible.len()).expect("test tile demand has a bounded length"));
        let center = if count == 0.0 {
            [0.0; 2]
        } else {
            [
                visible.iter().map(|id| f64::from(id.x)).sum::<f64>() / count,
                visible.iter().map(|id| f64::from(id.y)).sum::<f64>() / count,
            ]
        };
        self.apply_coverage(visible, visible, center);
    }

    pub(in crate::activity) fn ready_len(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry, TileEntry::Ready(_)))
            .count()
    }

    pub(in crate::activity) fn pending_len(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry, TileEntry::Requested { .. }))
            .count()
    }

    pub(in crate::activity) fn pending_visible_len(&self) -> usize {
        self.visible
            .iter()
            .filter(|id| {
                matches!(
                    self.entries.get(id),
                    Some(TileEntry::Desired { .. } | TileEntry::Requested { .. })
                )
            })
            .count()
    }

    pub(in crate::activity) const fn scene_revision(&self) -> u64 {
        self.scene_revision
    }

    pub(in crate::activity) fn visible_background_failure(&self) -> Option<String> {
        self.visible
            .iter()
            .filter_map(|id| match self.entries.get(id) {
                Some(TileEntry::Failed(failure)) => Some((id, failure)),
                _ => None,
            })
            .min_by_key(|(id, _)| (id.zoom, id.y, id.x))
            .map(|(id, failure)| format!("Tile {}/{}/{}: {}", id.zoom, id.x, id.y, failure.reason))
    }

    pub(in crate::activity) fn renderable_tiles(
        &self,
    ) -> impl Iterator<Item = (&TileId, &Arc<PreparedTile>)> {
        self.visible_entries().filter_map(|(id, entry)| {
            let TileEntry::Ready(tile) = entry else {
                return None;
            };
            Some((id, tile))
        })
    }

    fn visible_entries(&self) -> impl Iterator<Item = (&TileId, &TileEntry)> {
        self.visible
            .iter()
            .filter_map(|id| self.entries.get_key_value(id))
    }

    pub(in crate::activity) fn resolve(
        &mut self,
        context: &egui::Context,
        response: MapTileResponse,
    ) {
        let id = TileId {
            zoom: response.request.zoom,
            x: response.request.x,
            y: response.request.y,
        };
        if response.request.dark_mode() != self.dark_mode
            || response.request.generation() != self.generation
        {
            return;
        }
        let Some(TileEntry::Requested {
            attempts,
            generation,
        }) = self.entries.get(&id)
        else {
            return;
        };
        if *generation != response.request.generation() {
            return;
        }
        let attempts = *attempts;
        let retry_after = match response.result {
            Ok(MapTilePayload::Empty) => {
                self.entries.insert(id, TileEntry::Empty);
                self.promote(id);
                None
            }
            Ok(MapTilePayload::Decoded(tile)) => {
                self.entries.insert(id, TileEntry::Ready(Arc::new(tile)));
                self.promote(id);
                None
            }
            Err(reason) => {
                tracing::warn!(zoom = id.zoom, x = id.x, y = id.y, %reason, "map tile unavailable");
                let (failure, delay) = Failure::after(attempts, reason);
                self.entries.insert(id, TileEntry::Failed(failure));
                self.prune_failures();
                Some(delay)
            }
        };
        self.mark_scene_changed();
        if let Some(delay) = retry_after {
            context.request_repaint_after(delay);
        }
        context.request_repaint();
    }

    fn prune_failures(&mut self) {
        let failure_count = self
            .entries
            .values()
            .filter(|entry| matches!(entry, TileEntry::Failed(_)))
            .count();
        if failure_count > DECODED_TILE_LIMIT
            && let Some(oldest) = self
                .entries
                .iter()
                .filter_map(|(id, entry)| {
                    let TileEntry::Failed(failure) = entry else {
                        return None;
                    };
                    Some((*id, failure.retry_at))
                })
                .min_by_key(|(_id, retry_at)| *retry_at)
                .map(|(id, _retry_at)| id)
        {
            self.entries.remove(&oldest);
        }
    }

    pub(super) fn promote(&mut self, id: TileId) {
        let mut evicted_completed = false;
        self.order.retain(|candidate| *candidate != id);
        self.order.push_back(id);
        while self.order.len() > DECODED_TILE_LIMIT {
            if let Some(evicted) = self.order.pop_front()
                && matches!(
                    self.entries.get(&evicted),
                    Some(TileEntry::Ready(_) | TileEntry::Empty)
                )
            {
                self.entries.remove(&evicted);
                evicted_completed = true;
            }
        }
        if evicted_completed {
            self.mark_scene_changed();
        }
    }

    pub(in crate::activity) fn take_requests(&mut self) -> Vec<MapTileRequest> {
        std::mem::take(&mut self.requests)
    }

    pub(in crate::activity) fn schedule_requests(&mut self) {
        let capacity = MAX_IN_FLIGHT.saturating_sub(self.pending_len());
        if capacity == 0 {
            return;
        }
        if self.visible.is_empty() {
            return;
        }
        let mut candidates = self
            .demanded
            .iter()
            .filter_map(|id| {
                let TileEntry::Desired { attempts } = self.entries.get(id)? else {
                    return None;
                };
                let tile_count = 2.0_f64.powi(i32::from(id.zoom));
                let x_distance = (f64::from(id.x) - self.demand_center[0]).abs();
                let wrapped_x_distance = x_distance.min((tile_count - x_distance).abs());
                let distance =
                    wrapped_x_distance.powi(2) + (f64::from(id.y) - self.demand_center[1]).powi(2);
                Some((*id, *attempts, self.visible.contains(id), distance))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .2
                .cmp(&left.2)
                .then_with(|| left.3.total_cmp(&right.3))
                .then_with(|| right.0.zoom.cmp(&left.0.zoom))
                .then_with(|| left.0.y.cmp(&right.0.y))
                .then_with(|| left.0.x.cmp(&right.0.x))
        });
        let mut scheduled = false;
        for (id, attempts, _visible, _distance) in candidates.into_iter().take(capacity) {
            self.entries.insert(
                id,
                TileEntry::Requested {
                    attempts,
                    generation: self.generation,
                },
            );
            self.requests.push(TileCoordinates::new(
                id.zoom,
                id.x,
                id.y,
                self.dark_mode,
                self.generation,
            ));
            scheduled = true;
        }
        if scheduled {
            self.mark_scene_changed();
        }
    }

    fn desire(&mut self, tile_id: TileId) {
        if tile_id.zoom > MAX_TILE_ZOOM {
            return;
        }
        match self.entries.get(&tile_id) {
            Some(
                TileEntry::Ready(_)
                | TileEntry::Empty
                | TileEntry::Desired { .. }
                | TileEntry::Requested { .. },
            ) => return,
            Some(TileEntry::Failed(failure)) if Instant::now() < failure.retry_at => return,
            Some(TileEntry::Failed(_)) | None => {}
        }
        let attempts = match self.entries.get(&tile_id) {
            Some(TileEntry::Failed(failure)) => failure.attempts,
            _ => 0,
        };
        self.entries
            .insert(tile_id, TileEntry::Desired { attempts });
        if self.visible.contains(&tile_id) {
            self.mark_scene_changed();
        }
    }

    fn mark_scene_changed(&mut self) {
        self.scene_revision = self.scene_revision.wrapping_add(1);
    }

    pub(in crate::activity) fn attribution() -> Attribution {
        map_attribution()
    }
}

fn map_attribution() -> Attribution {
    Attribution {
        text: "OpenFreeMap · © OpenMapTiles · © OpenStreetMap contributors",
        url: "https://openfreemap.org/",
        logo_light: None,
        logo_dark: None,
    }
}

impl Failure {
    pub(super) fn after(previous_attempts: u8, reason: String) -> (Self, Duration) {
        let attempts = previous_attempts.saturating_add(1);
        let shift = u32::from(attempts.saturating_sub(1).min(5));
        let delay = Duration::from_secs(1_u64 << shift).min(MAX_RETRY_DELAY);
        (
            Self {
                attempts,
                retry_at: Instant::now() + delay,
                reason,
            },
            delay,
        )
    }
}

pub(super) fn map_style(dark_mode: bool) -> walkers::Style {
    let mut style = if dark_mode {
        walkers::Style::openmaptiles_basemap_dark()
    } else {
        walkers::Style::openmaptiles_basemap_light()
    };

    for layer in &mut style.layers {
        let Some((source_layer, filter)) = source_layer_and_filter(layer) else {
            continue;
        };
        let Some(visibility) = BasemapVisibility::classify(source_layer, filter.as_ref()) else {
            continue;
        };
        visibility.apply(filter);
    }
    style
}

/// Explicit `OpenMapTiles` visibility floors. Walkers otherwise tessellates fractional-width local
/// detail well before it is legible, which turns city-scale views into a dense hairline mesh.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BasemapVisibility {
    FromZoom(u8),
    MajorRoads,
    MinorRoads,
    Settlements,
}

impl BasemapVisibility {
    fn classify(
        source_layer: &walkers::SourceLayer,
        filter: Option<&walkers::Filter>,
    ) -> Option<Self> {
        let contains = |value| filter.is_some_and(|filter| json_contains(&filter.0, value));

        if source_layer.matches("building") {
            return Some(Self::FromZoom(14));
        }

        if source_layer.matches("transportation") {
            return if contains("taxiway") {
                Some(Self::FromZoom(14))
            } else if contains("runway") {
                Some(Self::FromZoom(10))
            } else if ["other", "path", "track"].into_iter().any(contains) {
                Some(Self::FromZoom(14))
            } else if ["minor_road", "minor", "tertiary"]
                .into_iter()
                .any(contains)
            {
                Some(Self::MinorRoads)
            } else if ["major_road", "primary", "secondary"]
                .into_iter()
                .any(contains)
            {
                Some(Self::MajorRoads)
            } else if ["highway", "motorway", "trunk"].into_iter().any(contains) {
                Some(Self::FromZoom(5))
            } else if ["rail", "transit"].into_iter().any(contains) {
                Some(Self::FromZoom(9))
            } else if contains("ramp") {
                Some(Self::FromZoom(11))
            } else if contains("pier") {
                Some(Self::FromZoom(12))
            } else {
                None
            };
        }

        if source_layer.matches("transportation_name") {
            return if contains("oneway") {
                Some(Self::FromZoom(15))
            } else if contains("shield_text") {
                Some(Self::FromZoom(8))
            } else if [
                "minor_road",
                "minor",
                "tertiary",
                "other",
                "path",
                "service",
                "track",
            ]
            .into_iter()
            .any(contains)
            {
                Some(Self::FromZoom(13))
            } else {
                Some(Self::FromZoom(11))
            };
        }

        if source_layer.matches("place") {
            return if ["neighbourhood", "macrohood", "suburb", "quarter"]
                .into_iter()
                .any(contains)
            {
                Some(Self::FromZoom(12))
            } else if ["locality", "city", "town", "village"]
                .into_iter()
                .any(contains)
            {
                Some(Self::Settlements)
            } else {
                None
            };
        }

        if source_layer.matches("poi") {
            return Some(Self::FromZoom(13));
        }

        if source_layer.matches("waterway") {
            return if contains("stream") {
                Some(Self::FromZoom(12))
            } else if contains("river") {
                Some(Self::FromZoom(8))
            } else {
                None
            };
        }

        None
    }

    fn apply(self, filter: &mut Option<walkers::Filter>) {
        let visibility = match self {
            Self::FromZoom(zoom) => walkers::json!([">=", ["zoom"], zoom]),
            Self::MajorRoads => walkers::json!([
                "any",
                [
                    "all",
                    [">=", ["zoom"], 7],
                    ["in", "class", "major_road", "primary"]
                ],
                [
                    "all",
                    [">=", ["zoom"], 9],
                    ["==", ["get", "class"], "secondary"]
                ]
            ]),
            Self::MinorRoads => walkers::json!([
                "any",
                [
                    "all",
                    [">=", ["zoom"], 11],
                    ["==", ["get", "class"], "tertiary"]
                ],
                [
                    "all",
                    [">=", ["zoom"], 13],
                    ["in", "class", "minor_road", "minor"]
                ]
            ]),
            Self::Settlements => walkers::json!([
                "any",
                ["all", [">=", ["zoom"], 4], ["==", ["get", "class"], "city"]],
                ["all", [">=", ["zoom"], 8], ["==", ["get", "class"], "town"]],
                [
                    "all",
                    [">=", ["zoom"], 11],
                    ["in", "class", "locality", "village"]
                ]
            ]),
        };

        *filter = Some(walkers::Filter(match filter.take() {
            Some(existing) => walkers::json!(["all", visibility, existing.0]),
            None => visibility,
        }));
    }
}

fn source_layer_and_filter(
    layer: &mut walkers::Layer,
) -> Option<(&walkers::SourceLayer, &mut Option<walkers::Filter>)> {
    match layer {
        walkers::Layer::Fill {
            source_layer,
            filter,
            ..
        }
        | walkers::Layer::Line {
            source_layer,
            filter,
            ..
        }
        | walkers::Layer::Symbol {
            source_layer,
            filter,
            ..
        }
        | walkers::Layer::Circle {
            source_layer,
            filter,
        } => Some((source_layer, filter)),
        walkers::Layer::Background { .. }
        | walkers::Layer::Raster
        | walkers::Layer::FillExtrusion => None,
    }
}

fn json_contains(value: &walkers::Value, expected: &str) -> bool {
    match value {
        walkers::Value::String(value) => value == expected,
        walkers::Value::Array(values) => values.iter().any(|value| json_contains(value, expected)),
        walkers::Value::Object(values) => {
            values.values().any(|value| json_contains(value, expected))
        }
        walkers::Value::Null | walkers::Value::Bool(_) | walkers::Value::Number(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visibility(source_layer: &str, filter: walkers::Value) -> Option<BasemapVisibility> {
        BasemapVisibility::classify(
            &walkers::SourceLayer::from(source_layer),
            Some(&walkers::Filter(filter)),
        )
    }

    fn policy_matches(policy: BasemapVisibility, class: &str, zoom: u8) -> bool {
        let mut filter = None;
        policy.apply(&mut filter);
        let context = walkers::Context::new(
            "LineString".to_owned(),
            std::collections::HashMap::from([("class".to_owned(), walkers::json!(class))]),
            zoom,
        );
        filter.is_some_and(|filter| filter.matches(&context))
    }

    #[test]
    fn basemap_detail_policy_delays_dense_geometry() {
        assert_eq!(
            visibility(
                "transportation",
                walkers::json!(["in", "class", "minor_road", "minor", "tertiary"]),
            ),
            Some(BasemapVisibility::MinorRoads)
        );
        assert_eq!(
            visibility(
                "transportation",
                walkers::json!(["in", "class", "other", "path", "service", "track"]),
            ),
            Some(BasemapVisibility::FromZoom(14))
        );
        assert_eq!(
            visibility("building", walkers::json!(["==", "$type", "Polygon"])),
            Some(BasemapVisibility::FromZoom(14))
        );
    }

    #[test]
    fn basemap_detail_policy_delays_local_labels() {
        assert_eq!(
            visibility(
                "transportation_name",
                walkers::json!(["in", "class", "minor", "tertiary"]),
            ),
            Some(BasemapVisibility::FromZoom(13))
        );
        assert_eq!(
            visibility(
                "place",
                walkers::json!(["in", "class", "neighbourhood", "suburb"]),
            ),
            Some(BasemapVisibility::FromZoom(12))
        );
        assert_eq!(
            visibility(
                "place",
                walkers::json!(["in", "class", "city", "town", "village"]),
            ),
            Some(BasemapVisibility::Settlements)
        );
    }

    #[test]
    fn basemap_detail_policy_enforces_hierarchy_at_zoom_boundaries() {
        assert!(policy_matches(BasemapVisibility::MajorRoads, "primary", 7));
        assert!(!policy_matches(
            BasemapVisibility::MajorRoads,
            "secondary",
            8
        ));
        assert!(policy_matches(
            BasemapVisibility::MajorRoads,
            "secondary",
            9
        ));
        assert!(policy_matches(
            BasemapVisibility::MinorRoads,
            "tertiary",
            11
        ));
        assert!(!policy_matches(BasemapVisibility::MinorRoads, "minor", 12));
        assert!(policy_matches(BasemapVisibility::MinorRoads, "minor", 13));
    }

    #[test]
    fn render_scene_excludes_cached_tiles_outside_current_demand() {
        let mut tiles = TileStore::default();
        let current = TileId {
            zoom: 9,
            x: 255,
            y: 170,
        };
        let cached_from_previous_zoom = TileId {
            zoom: 14,
            x: 8_170,
            y: 5_445,
        };
        tiles.entries.insert(current, TileEntry::Empty);
        tiles
            .entries
            .insert(cached_from_previous_zoom, TileEntry::Empty);

        tiles.apply_exact_demand(&[current]);

        assert_eq!(
            tiles
                .visible_entries()
                .map(|(id, _entry)| *id)
                .collect::<Vec<_>>(),
            vec![current]
        );
    }
}
