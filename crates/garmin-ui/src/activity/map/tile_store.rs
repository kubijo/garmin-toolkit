//! Vector-tile decoding, demand scheduling, retry, and bounded scene storage.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use egui::{Rect, pos2};
use walkers::{Tile, TileId, TilePiece, Tiles, sources::Attribution};

use crate::activity::{
    map::gpu_map,
    map_runtime::{Renderer, TileCoordinates},
};

pub(super) const DECODED_TILE_LIMIT: usize = 256;
pub(super) const MAX_IN_FLIGHT: usize = 6;
const SOURCE_TILE_SIZE: u32 = 512;
pub(super) const WALKERS_TILE_SIZE: u32 = 256;
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
    fn new(renderer: &Renderer, id: TileId, tile: Tile) -> Self {
        renderer.prepare_tile(id, tile)
    }
}

impl MapTileResponse {
    /// Reconstruct a tile prepared by the browser map worker.
    #[must_use]
    pub(in crate::activity) fn prepared(
        request: MapTileRequest,
        result: Result<Vec<u8>, String>,
        renderer: &Renderer,
    ) -> Self {
        let result = result.and_then(|bytes| {
            if bytes.is_empty() {
                Ok(MapTilePayload::Empty)
            } else {
                renderer.prepare_browser_tile(&bytes).map(|gpu| {
                    MapTilePayload::Decoded(PreparedTile {
                        tile: Tile::Vector {
                            shapes: Vec::new(),
                            texts: Vec::new(),
                        },
                        gpu,
                    })
                })
            }
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
                Tile::from_mvt(
                    &bytes,
                    if request.dark_mode() {
                        &self.dark
                    } else {
                        &self.light
                    },
                    request.zoom,
                    SOURCE_TILE_SIZE,
                )
                .map(|tile| {
                    let id = TileId {
                        zoom: request.zoom,
                        x: request.x,
                        y: request.y,
                    };
                    PreparedTile::new(renderer, id, tile)
                })
                .map(MapTilePayload::Decoded)
                .map_err(|error| error.to_string())
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
) -> Result<Vec<u8>, String> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let style = map_style(dark_mode);
    let tile =
        Tile::from_mvt(bytes, &style, zoom, SOURCE_TILE_SIZE).map_err(|error| error.to_string())?;
    gpu_map::encode_browser_tile(tile)
}

pub(in crate::activity) struct TileStore {
    pub(super) entries: HashMap<TileId, TileEntry>,
    order: VecDeque<TileId>,
    pub(super) requests: Vec<MapTileRequest>,
    visible: HashSet<TileId>,
    dark_mode: bool,
    generation: u64,
}

pub(super) enum TileEntry {
    Ready(PreparedTile),
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
            dark_mode: true,
            generation: 0,
        }
    }
}

pub(super) struct Failure {
    pub(super) attempts: u8,
    retry_at: Instant,
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
    }

    pub(in crate::activity) fn begin_frame(&mut self) {
        self.visible.clear();
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

    pub(in crate::activity) fn visible_background_unavailable(&self) -> bool {
        self.visible
            .iter()
            .any(|id| matches!(self.entries.get(id), Some(TileEntry::Failed(_))))
    }

    pub(in crate::activity) fn gpu_tiles(
        &self,
    ) -> impl Iterator<Item = (&TileId, &std::sync::Arc<gpu_map::PreparedGpuTile>)> {
        self.entries.iter().filter_map(|(id, entry)| {
            let TileEntry::Ready(PreparedTile {
                gpu: Some(tile), ..
            }) = entry
            else {
                return None;
            };
            Some((id, tile))
        })
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
                self.entries.insert(id, TileEntry::Ready(tile));
                self.promote(id);
                None
            }
            Err(_reason) => {
                let (failure, delay) = Failure::after(attempts);
                self.entries.insert(id, TileEntry::Failed(failure));
                self.prune_failures();
                Some(delay)
            }
        };
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
            }
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
        let visible_count = f64::from(
            u32::try_from(self.visible.len())
                .expect("visible tile count is bounded by the viewport"),
        );
        if visible_count == 0.0 {
            return;
        }
        let center_x = self.visible.iter().map(|id| f64::from(id.x)).sum::<f64>() / visible_count;
        let center_y = self.visible.iter().map(|id| f64::from(id.y)).sum::<f64>() / visible_count;
        let mut candidates = self
            .visible
            .iter()
            .filter_map(|id| {
                let TileEntry::Desired { attempts } = self.entries.get(id)? else {
                    return None;
                };
                let distance =
                    (f64::from(id.x) - center_x).powi(2) + (f64::from(id.y) - center_y).powi(2);
                Some((*id, *attempts, distance))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            left.2
                .total_cmp(&right.2)
                .then_with(|| right.0.zoom.cmp(&left.0.zoom))
                .then_with(|| left.0.y.cmp(&right.0.y))
                .then_with(|| left.0.x.cmp(&right.0.x))
        });
        for (id, attempts, _distance) in candidates.into_iter().take(capacity) {
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
        }
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
    pub(super) fn after(previous_attempts: u8) -> (Self, Duration) {
        let attempts = previous_attempts.saturating_add(1);
        let shift = u32::from(attempts.saturating_sub(1).min(5));
        let delay = Duration::from_secs(1_u64 << shift).min(MAX_RETRY_DELAY);
        (
            Self {
                attempts,
                retry_at: Instant::now() + delay,
            },
            delay,
        )
    }
}

fn map_style(dark_mode: bool) -> walkers::Style {
    let mut style = if dark_mode {
        walkers::Style::openmaptiles_basemap_dark()
    } else {
        walkers::Style::openmaptiles_basemap_light()
    };
    for layer in &mut style.layers {
        let walkers::Layer::Symbol {
            source_layer,
            filter,
            ..
        } = layer
        else {
            continue;
        };
        let minimum_zoom = if source_layer.matches("building") {
            Some(14)
        } else if filter
            .as_ref()
            .is_some_and(|filter| json_contains(&filter.0, "minor_road"))
        {
            Some(12)
        } else {
            None
        };
        if let Some(minimum_zoom) = minimum_zoom {
            let zoom_filter = walkers::json!([">=", ["zoom"], minimum_zoom]);
            *filter = Some(walkers::Filter(match filter.take() {
                Some(existing) => walkers::json!(["all", zoom_filter, existing.0]),
                None => zoom_filter,
            }));
        }
    }
    style
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

impl Tiles for TileStore {
    fn at(&mut self, tile_id: TileId) -> Option<TilePiece> {
        if tile_id.zoom > MAX_TILE_ZOOM {
            return None;
        }
        self.visible.insert(tile_id);
        match self.entries.get(&tile_id) {
            Some(TileEntry::Ready(prepared)) => {
                let tile = prepared.tile.clone();
                self.promote(tile_id);
                return Some(TilePiece::new(
                    tile,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                ));
            }
            Some(TileEntry::Empty) => {
                self.promote(tile_id);
                return None;
            }
            Some(TileEntry::Desired { .. } | TileEntry::Requested { .. }) => return None,
            Some(TileEntry::Failed(failure)) if Instant::now() < failure.retry_at => return None,
            Some(TileEntry::Failed(_)) | None => {}
        }
        let attempts = match self.entries.get(&tile_id) {
            Some(TileEntry::Failed(failure)) => failure.attempts,
            _ => 0,
        };
        self.entries
            .insert(tile_id, TileEntry::Desired { attempts });
        None
    }

    fn attribution(&self) -> Attribution {
        map_attribution()
    }

    fn tile_size(&self) -> u32 {
        SOURCE_TILE_SIZE
    }
}
