//! Persistent WGPU rendering for decoded vector-map geometry.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::similar_names,
    reason = "bounded GPU coordinates, indices, and grid cells intentionally use f32/i32"
)]

use std::{marker::PhantomData, num::NonZeroU64, sync::Arc};

use arc_swap::ArcSwapOption;
use bytemuck::{Pod, Zeroable};
use egui::{Color32, Rect, Shape, pos2};
use garmin_service_api::ActivitySampleSnapshot;
use walkers::{Tile, TileId};
use web_time::Instant;
use wgpu::util::DeviceExt as _;

use super::{WALKERS_TILE_SIZE, mercator_y, speed_bounds};
use crate::activity::map_style;

#[path = "gpu_map/egui_adapter.rs"]
pub(in crate::activity) mod egui_adapter;
#[path = "gpu_map/labels.rs"]
mod labels;
#[path = "gpu_map/platform.rs"]
mod platform;
#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "gpu_map/render_tests.rs"]
mod render_tests;
#[path = "gpu_map/renderer.rs"]
pub(in crate::activity) mod renderer;
#[path = "gpu_map/route.rs"]
mod route;
#[path = "gpu_map/tile_budget.rs"]
mod tile_budget;
#[cfg(any(target_arch = "wasm32", test))]
#[path = "gpu_map/upload_trace.rs"]
mod upload_trace;
pub use egui_adapter::install;
pub use tile_budget::BrowserTileLimits;
pub(in crate::activity) use tile_budget::{TileBudget, TileDecodeBudget};

pub use labels::{BrowserLabelTask, prepare_labels_for_browser_worker};
use labels::{LabelCache, LabelResult, LabelTask, LabelView};
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
    resources: Arc<Resources>,
}

impl WgpuMapHandle {
    /// Install the production map pipelines on an independently owned render target.
    /// # Panics
    /// Panics unless the target sample count is 1 or 4.
    #[must_use]
    pub fn for_target(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        Self::new(device, format, samples)
    }
}

#[derive(Clone)]
struct CpuTileMesh {
    vertices: MeshBuffer<Vertex>,
    indices: MeshBuffer<u32>,
    texts: Vec<walkers::Text>,
}

#[derive(Clone)]
enum MeshBuffer<T> {
    Typed(Vec<T>),
    Packed {
        bytes: Vec<u8>,
        marker: PhantomData<T>,
    },
}

impl<T: Pod> MeshBuffer<T> {
    fn typed(values: Vec<T>) -> Self {
        Self::Typed(values)
    }

    fn packed(bytes: Vec<u8>) -> Result<Self, String> {
        if !bytes.len().is_multiple_of(std::mem::size_of::<T>()) {
            return Err("packed map geometry had a partial element".to_owned());
        }
        Ok(Self::Packed {
            bytes,
            marker: PhantomData,
        })
    }

    fn len(&self) -> usize {
        self.as_bytes().len() / std::mem::size_of::<T>()
    }

    fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Typed(values) => bytemuck::cast_slice(values),
            Self::Packed { bytes, .. } => bytes,
        }
    }

    fn retained_bytes(&self) -> usize {
        match self {
            Self::Typed(values) => values.capacity().saturating_mul(std::mem::size_of::<T>()),
            Self::Packed { bytes, .. } => bytes.capacity(),
        }
    }

    #[cfg(test)]
    fn get(&self, index: usize) -> Option<T> {
        let width = std::mem::size_of::<T>();
        let start = index.checked_mul(width)?;
        let end = start.checked_add(width)?;
        self.as_bytes()
            .get(start..end)
            .map(bytemuck::pod_read_unaligned)
    }
}

/// One transferable browser-worker tile split into upload-ready geometry and text storage.
pub struct BrowserTileTransfer {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    text_records: Vec<BrowserTextRecord>,
    strings: Vec<u8>,
}

impl BrowserTileTransfer {
    /// Borrow upload-ready bytes without allocating intermediate byte vectors.
    #[must_use]
    pub fn parts(&self) -> [&[u8]; 4] {
        [
            bytemuck::cast_slice(&self.vertices),
            bytemuck::cast_slice(&self.indices),
            bytemuck::cast_slice(&self.text_records),
            &self.strings,
        ]
    }
}

/// Section of a browser tile packet admitted independently on the animation clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserTileSection {
    /// Packed WGPU vertices.
    Vertices,
    /// Packed triangle indices.
    Indices,
    /// Fixed-width text metadata records.
    TextRecords,
    /// UTF-8 storage referenced by the text records.
    Strings,
}

impl BrowserTileSection {
    const ORDER: [Self; 4] = [
        Self::Strings,
        Self::TextRecords,
        Self::Vertices,
        Self::Indices,
    ];
}

/// Validated browser tile ready for atomic runtime publication.
pub struct BrowserTilePacket {
    prepared: Option<Arc<PreparedGpuTile>>,
}

impl BrowserTilePacket {
    pub(in crate::activity) fn into_prepared(self) -> Option<Arc<PreparedGpuTile>> {
        self.prepared
    }
}

/// Incremental owner of browser tile transfer buffers, validation, and text reconstruction.
pub struct BrowserTilePacketBuilder {
    expected: BrowserTileLengths,
    vertices: Vec<u8>,
    indices: Vec<u8>,
    strings: Vec<u8>,
    texts: Vec<walkers::Text>,
    section_index: usize,
    allocation_index: usize,
    decoded_string_bytes: usize,
}

struct BrowserTileLengths {
    vertices: usize,
    indices: usize,
    text_records: usize,
    strings: usize,
}

impl BrowserTileLengths {
    const fn get(&self, section: BrowserTileSection) -> usize {
        match section {
            BrowserTileSection::Vertices => self.vertices,
            BrowserTileSection::Indices => self.indices,
            BrowserTileSection::TextRecords => self.text_records,
            BrowserTileSection::Strings => self.strings,
        }
    }
}

impl BrowserTilePacketBuilder {
    /// Validate transfer lengths before allocating or copying worker output.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed element boundaries or a packet above the upload limit.
    pub fn new(
        vertex_bytes: usize,
        index_bytes: usize,
        text_record_bytes: usize,
        string_bytes: usize,
    ) -> Result<Self, String> {
        if !vertex_bytes.is_multiple_of(std::mem::size_of::<Vertex>()) {
            return Err("browser tile vertex storage had a partial element".to_owned());
        }
        if !index_bytes.is_multiple_of(std::mem::size_of::<u32>())
            || !(index_bytes / std::mem::size_of::<u32>()).is_multiple_of(3)
        {
            return Err("browser tile index storage did not contain complete triangles".to_owned());
        }
        if !text_record_bytes.is_multiple_of(std::mem::size_of::<BrowserTextRecord>()) {
            return Err("browser tile text storage had a partial record".to_owned());
        }
        let total = vertex_bytes
            .checked_add(index_bytes)
            .and_then(|bytes| bytes.checked_add(text_record_bytes))
            .and_then(|bytes| bytes.checked_add(string_bytes))
            .ok_or_else(|| "browser tile transfer length overflowed".to_owned())?;
        BrowserTileLimits::check_upload(total)?;
        let mut builder = Self {
            expected: BrowserTileLengths {
                vertices: vertex_bytes,
                indices: index_bytes,
                text_records: text_record_bytes,
                strings: string_bytes,
            },
            vertices: Vec::new(),
            indices: Vec::new(),
            strings: Vec::new(),
            texts: Vec::new(),
            section_index: 0,
            allocation_index: 0,
            decoded_string_bytes: 0,
        };
        builder.skip_empty_sections();
        Ok(builder)
    }

    /// Payload retention allowance: transferred sources, destination vectors, and decoded strings.
    /// Allocator metadata, capacity rounding, and transport objects are not included.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        let geometry = self.expected.vertices + self.expected.indices;
        let texts = self.expected.text_records / std::mem::size_of::<BrowserTextRecord>();
        2 * geometry
            + self.expected.text_records
            + 3 * self.expected.strings
            + texts * std::mem::size_of::<walkers::Text>()
    }

    /// Reserve one destination vector during timed admission, returning its requested byte size.
    /// An allocation is indivisible and may exceed the admission time target.
    ///
    /// # Errors
    ///
    /// Returns an error if the allocator cannot reserve the requested capacity.
    pub fn allocate_next(&mut self) -> Result<Option<usize>, String> {
        while let Some(section) = BrowserTileSection::ORDER
            .get(self.allocation_index)
            .copied()
        {
            let bytes = self.expected.get(section);
            if bytes == 0 {
                self.allocation_index += 1;
                continue;
            }
            let (result, requested) = match section {
                BrowserTileSection::Strings => (self.strings.try_reserve_exact(bytes), bytes),
                BrowserTileSection::Vertices => (self.vertices.try_reserve_exact(bytes), bytes),
                BrowserTileSection::Indices => (self.indices.try_reserve_exact(bytes), bytes),
                BrowserTileSection::TextRecords => {
                    let count = bytes / std::mem::size_of::<BrowserTextRecord>();
                    (
                        self.texts.try_reserve_exact(count),
                        count * std::mem::size_of::<walkers::Text>(),
                    )
                }
            };
            result.map_err(|error| format!("could not allocate browser tile: {error}"))?;
            self.allocation_index += 1;
            return Ok(Some(requested));
        }
        Ok(None)
    }

    /// Return the next section that must be admitted and the bytes still outstanding.
    #[must_use]
    pub fn next_section(&self) -> Option<(BrowserTileSection, usize)> {
        let section = *BrowserTileSection::ORDER.get(self.section_index)?;
        Some((section, self.remaining(section)))
    }

    /// Choose a bounded chunk without splitting a fixed-width wire element.
    #[must_use]
    pub fn next_chunk(&self, maximum_bytes: usize) -> Option<(BrowserTileSection, usize)> {
        let (section, remaining) = self.next_section()?;
        let alignment = match section {
            BrowserTileSection::Strings => 1,
            BrowserTileSection::TextRecords => std::mem::size_of::<BrowserTextRecord>(),
            BrowserTileSection::Vertices => std::mem::align_of::<Vertex>(),
            BrowserTileSection::Indices => std::mem::size_of::<u32>(),
        };
        let bounded = if section == BrowserTileSection::TextRecords {
            remaining.min(maximum_bytes).min(alignment)
        } else {
            remaining.min(maximum_bytes)
        };
        let length = if bounded == remaining {
            bounded
        } else {
            bounded - bounded % alignment
        };
        (length > 0).then_some((section, length))
    }

    /// Append one bounded chunk to the current section.
    ///
    /// Text records are reconstructed immediately, after their UTF-8 storage has arrived.
    ///
    /// # Errors
    ///
    /// Returns an error for out-of-order, oversized, malformed, or invalid data.
    pub fn append(&mut self, section: BrowserTileSection, bytes: &[u8]) -> Result<(), String> {
        let remaining = self.check_append(section, bytes.len())?;
        match section {
            BrowserTileSection::Strings => self.strings.extend_from_slice(bytes),
            BrowserTileSection::TextRecords => self.append_text_records(bytes)?,
            BrowserTileSection::Vertices => self.vertices.extend_from_slice(bytes),
            BrowserTileSection::Indices => {
                Self::validate_indices(
                    bytes,
                    self.expected.vertices / std::mem::size_of::<Vertex>(),
                )?;
                self.indices.extend_from_slice(bytes);
            }
        }
        self.advance_section(bytes.len(), remaining);
        Ok(())
    }

    /// Copy geometry or UTF-8 directly into its reserved destination, without a scratch copy.
    ///
    /// # Errors
    /// Returns an error for invalid lengths, section order, or triangle indices.
    pub fn append_from(
        &mut self,
        section: BrowserTileSection,
        length: usize,
        copy: impl FnOnce(&mut [u8]),
    ) -> Result<(), String> {
        let remaining = self.check_append(section, length)?;
        let destination = match section {
            BrowserTileSection::Strings => &mut self.strings,
            BrowserTileSection::Vertices => &mut self.vertices,
            BrowserTileSection::Indices => &mut self.indices,
            BrowserTileSection::TextRecords => {
                return Err("text records require reconstruction".to_owned());
            }
        };
        let start = destination.len();
        destination.resize(start + length, 0);
        copy(&mut destination[start..]);
        if section == BrowserTileSection::Indices
            && let Err(error) = Self::validate_indices(
                &destination[start..],
                self.expected.vertices / std::mem::size_of::<Vertex>(),
            )
        {
            destination.truncate(start);
            return Err(error);
        }
        self.advance_section(length, remaining);
        Ok(())
    }

    fn check_append(&self, section: BrowserTileSection, length: usize) -> Result<usize, String> {
        if self.allocation_index != BrowserTileSection::ORDER.len() {
            return Err("browser tile destinations have not been allocated".to_owned());
        }
        let Some((expected_section, remaining)) = self.next_section() else {
            return Err("browser tile packet received trailing data".to_owned());
        };
        if section != expected_section {
            return Err("browser tile packet sections arrived out of order".to_owned());
        }
        if length == 0 || length > remaining {
            return Err("browser tile packet chunk had an invalid length".to_owned());
        }
        Ok(remaining)
    }

    fn advance_section(&mut self, length: usize, remaining: usize) {
        if length == remaining {
            self.section_index += 1;
            self.skip_empty_sections();
        }
    }

    /// Finish the fully admitted packet and make it eligible for publication.
    ///
    /// # Errors
    ///
    /// Returns an error while any declared bytes remain outstanding.
    pub fn finish(self) -> Result<BrowserTilePacket, String> {
        if self.next_section().is_some() {
            return Err("browser tile packet was incomplete".to_owned());
        }
        let vertices = MeshBuffer::packed(self.vertices)?;
        let indices = MeshBuffer::packed(self.indices)?;
        let prepared = (!indices.is_empty() || !self.texts.is_empty()).then(|| {
            Arc::new(PreparedGpuTile {
                mesh: Arc::new(CpuTileMesh {
                    vertices,
                    indices,
                    texts: self.texts,
                }),
                gpu: ArcSwapOption::empty(),
            })
        });
        Ok(BrowserTilePacket { prepared })
    }

    fn append_text_records(&mut self, bytes: &[u8]) -> Result<(), String> {
        let width = std::mem::size_of::<BrowserTextRecord>();
        if !bytes.len().is_multiple_of(width) {
            return Err("browser tile admission split a text record".to_owned());
        }
        for record in bytes.chunks_exact(width) {
            let record: BrowserTextRecord = bytemuck::pod_read_unaligned(record);
            let decoded = self
                .decoded_string_bytes
                .checked_add(record.string_length as usize)
                .filter(|&bytes| bytes <= self.expected.strings)
                .ok_or_else(|| {
                    "browser tile decoded strings exceeded transferred storage".to_owned()
                })?;
            self.texts.push(record.to_text(&self.strings)?);
            self.decoded_string_bytes = decoded;
        }
        Ok(())
    }

    fn validate_indices(bytes: &[u8], vertex_count: usize) -> Result<(), String> {
        if !bytes.len().is_multiple_of(std::mem::size_of::<u32>()) {
            return Err("browser tile admission split an index".to_owned());
        }
        let (indices, remainder) = bytes.as_chunks::<{ std::mem::size_of::<u32>() }>();
        debug_assert!(remainder.is_empty());
        if indices
            .iter()
            .map(|bytes| u32::from_le_bytes(*bytes))
            .any(|index| index as usize >= vertex_count)
        {
            return Err("prepared map tile contained invalid triangle indices".to_owned());
        }
        Ok(())
    }

    fn remaining(&self, section: BrowserTileSection) -> usize {
        let admitted = match section {
            BrowserTileSection::Strings => self.strings.len(),
            BrowserTileSection::TextRecords => self
                .texts
                .len()
                .saturating_mul(std::mem::size_of::<BrowserTextRecord>()),
            BrowserTileSection::Vertices => self.vertices.len(),
            BrowserTileSection::Indices => self.indices.len(),
        };
        self.expected.get(section).saturating_sub(admitted)
    }

    fn skip_empty_sections(&mut self) {
        while BrowserTileSection::ORDER
            .get(self.section_index)
            .is_some_and(|section| self.expected.get(*section) == 0)
        {
            self.section_index += 1;
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BrowserTextRecord {
    string_offset: u32,
    string_length: u32,
    position: [f32; 2],
    font_size: f32,
    text_color: [u8; 4],
    halo_color: [u8; 4],
    halo_width: f32,
    angle: f32,
    line_placement: u32,
}

impl BrowserTextRecord {
    fn to_text(self, strings: &[u8]) -> Result<walkers::Text, String> {
        BrowserTileLimits::check_text(self.string_length as usize)?;
        let start = self.string_offset as usize;
        let end = start
            .checked_add(self.string_length as usize)
            .filter(|end| *end <= strings.len())
            .ok_or_else(|| {
                "browser tile text referenced bytes outside its string table".to_owned()
            })?;
        let text = std::str::from_utf8(&strings[start..end])
            .map_err(|_| "browser tile text was not valid UTF-8".to_owned())?
            .to_owned();
        let placement = match self.line_placement {
            0 => walkers::Placement::Point,
            1 => walkers::Placement::Line,
            _ => return Err("browser tile text had an invalid placement".to_owned()),
        };
        Ok(walkers::Text {
            text,
            position: pos2(self.position[0], self.position[1]),
            font_size: self.font_size,
            text_color: Color32::from_rgba_premultiplied(
                self.text_color[0],
                self.text_color[1],
                self.text_color[2],
                self.text_color[3],
            ),
            halo_color: Color32::from_rgba_premultiplied(
                self.halo_color[0],
                self.halo_color[1],
                self.halo_color[2],
                self.halo_color[3],
            ),
            halo_width: self.halo_width,
            angle: self.angle,
            placement,
        })
    }
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

struct BrowserTileGeometry {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    texts: Vec<walkers::Text>,
}

fn tessellate_browser_tile(tile: Tile) -> Result<BrowserTileGeometry, String> {
    let Tile::Vector { shapes, texts } = tile else {
        return Err("the vector map worker received a raster tile".to_owned());
    };
    let mut budget = TileBudget::default();
    budget.shapes(&shapes)?;
    for text in &texts {
        budget.text(text.text.len())?;
    }
    let (vertices, indices) = tessellate_tile(shapes, |mesh| budget.mesh(mesh))?;
    Ok(BrowserTileGeometry {
        vertices,
        indices,
        texts,
    })
}

/// Tessellate a worker tile into independently transferable WGPU and text buffers.
pub(in crate::activity) fn encode_browser_tile(tile: Tile) -> Result<BrowserTileTransfer, String> {
    let BrowserTileGeometry {
        vertices,
        indices,
        texts,
    } = tessellate_browser_tile(tile)?;
    let mut strings = Vec::new();
    let mut text_records = Vec::with_capacity(texts.len());
    for text in texts {
        let string_offset = u32::try_from(strings.len())
            .map_err(|_| "browser tile string table exceeded 4 GiB".to_owned())?;
        let string_length = u32::try_from(text.text.len())
            .map_err(|_| "browser tile text exceeded 4 GiB".to_owned())?;
        strings.extend_from_slice(text.text.as_bytes());
        text_records.push(BrowserTextRecord {
            string_offset,
            string_length,
            position: [text.position.x, text.position.y],
            font_size: text.font_size,
            text_color: text.text_color.to_array(),
            halo_color: text.halo_color.to_array(),
            halo_width: text.halo_width,
            angle: text.angle,
            line_placement: u32::from(text.placement == walkers::Placement::Line),
        });
    }
    Ok(BrowserTileTransfer {
        vertices,
        indices,
        text_records,
        strings,
    })
}

#[cfg(any(target_arch = "wasm32", test))]
pub(in crate::activity) fn prepare_local_browser_tile(
    tile: Tile,
) -> Result<BrowserTilePacket, String> {
    let BrowserTileGeometry {
        vertices,
        indices,
        texts,
    } = tessellate_browser_tile(tile)?;
    let prepared = (!indices.is_empty() || !texts.is_empty()).then(|| {
        Arc::new(PreparedGpuTile {
            mesh: Arc::new(CpuTileMesh {
                vertices: MeshBuffer::typed(vertices),
                indices: MeshBuffer::typed(indices),
                texts,
            }),
            gpu: ArcSwapOption::empty(),
        })
    });
    Ok(BrowserTilePacket { prepared })
}

fn tessellate_tile<E>(
    shapes: Vec<Shape>,
    mut check: impl FnMut(&egui::Mesh) -> Result<(), E>,
) -> Result<(Vec<Vertex>, Vec<u32>), E> {
    let mut tessellator = egui::epaint::tessellator::Tessellator::new(
        1.0,
        egui::epaint::tessellator::TessellationOptions::default(),
        [1, 1],
        Vec::new(),
    );
    let mut mesh = egui::Mesh::default();
    for shape in shapes {
        tessellator.tessellate_shape(shape, &mut mesh);
        check(&mesh)?;
    }
    let vertices = mesh
        .vertices
        .into_iter()
        .map(|vertex| Vertex {
            position: [vertex.pos.x, vertex.pos.y],
            color: vertex.color.to_array(),
        })
        .collect();
    Ok((vertices, mesh.indices))
}

/// Tessellate tile-local shapes once and retain only text in the walkers fallback tile.
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::activity) fn prepare_tile(
    handle: &WgpuMapHandle,
    id: TileId,
    tile: Tile,
) -> (Tile, Option<Arc<PreparedGpuTile>>) {
    match tile {
        Tile::Vector { shapes, texts } => {
            let (vertices, indices) =
                tessellate_tile(shapes, |_| Ok::<_, std::convert::Infallible>(()))
                    .unwrap_or_else(|never| match never {});
            let prepared = (!indices.is_empty() || !texts.is_empty()).then(|| {
                let mesh = Arc::new(CpuTileMesh {
                    vertices: MeshBuffer::typed(vertices),
                    indices: MeshBuffer::typed(indices),
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
    visible_tiles_settling: bool,
}

#[derive(Clone)]
struct VisibleTile {
    id: TileId,
    tile: Arc<PreparedGpuTile>,
    // Relative to the camera uniform's first visible world,
    // not duplicated tile resources.
    instances: std::ops::Range<u32>,
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

    fn visible(&self, scene: &RouteScene<'_>) -> Option<VisibleRoute> {
        self.source().map(|source| {
            let fallback = color(scene.color);
            let highlight = scene.highlighted_range.as_ref().map(|range| {
                let index_range_padding = [*range.start() as f32, *range.end() as f32, 0.0, 0.0];
                HighlightStyles {
                    outline: RouteStyleUniform::new(
                        map_style::HIGHLIGHT_OUTLINE_WIDTH,
                        1.0,
                        2.0,
                        color(scene.outline),
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
                    scene.opacity.max(map_style::MINIMUM_OUTLINE_OPACITY),
                    0.0,
                    color(scene.outline),
                    [0.0; 4],
                ),
                color: RouteStyleUniform::new(
                    map_style::ROUTE_WIDTH,
                    scene.opacity,
                    1.0,
                    fallback,
                    [0.0; 4],
                ),
                highlight,
            }
        })
    }
}

pub(in crate::activity) struct GpuMap {
    frame: Arc<Frame>,
    runtime: WgpuRuntime,
    labels: LabelCache,
    route: RouteCache,
}

struct WgpuRuntime {
    renderer: Arc<renderer::Renderer>,
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
    pending_tiles: usize,
    partial_tiles: usize,
    completed_tiles: usize,
    milliseconds: f32,
    budget_overruns: u64,
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
    pub route_ready: bool,
    pub milliseconds: f32,
    pub visible_tiles: usize,
    pub label_milliseconds: f32,
    pub label_backlog: usize,
    pub stale_work: u64,
    pub queued_upload_bytes: usize,
    pub uploaded_bytes: usize,
    pub pending_upload_tiles: usize,
    pub partial_upload_tiles: usize,
    pub completed_upload_tiles: usize,
    pub upload_milliseconds: f32,
    pub upload_budget_overruns: u64,
}

struct TileFrameAssembly {
    camera: CameraUniform,
    visible: Vec<VisibleTile>,
    upload_candidates: Vec<VisibleTile>,
}

fn assemble_tile_frame<'a>(
    tiles: impl IntoIterator<Item = (&'a TileId, &'a Arc<PreparedGpuTile>)>,
    camera: &super::camera::MapCamera,
    viewport: Rect,
) -> TileFrameAssembly {
    let center = camera.center();
    let zoom = camera.zoom();
    let world_size = f64::from(WALKERS_TILE_SIZE) * 2.0_f64.powf(zoom);
    let center_normalized = [center.x() / 360.0 + 0.5, mercator_y(center.y())];
    let viewport_size = [viewport.width().max(1.0), viewport.height().max(1.0)];
    let first_world =
        super::camera::first_visible_world(center_normalized[0], world_size, viewport_size[0]);
    let mut visible = Vec::new();
    let mut upload_candidates = Vec::new();

    for (id, tile) in tiles {
        let placement =
            super::camera::TilePlacement::new(*id, center_normalized, world_size, viewport);
        let copies = placement.copies(viewport);
        if copies.is_empty() {
            continue;
        }
        let visible_tile = VisibleTile {
            id: *id,
            tile: Arc::clone(tile),
            instances: u32::try_from(copies.start - first_world)
                .expect("visible copy follows first world")
                ..u32::try_from(copies.end - first_world)
                    .expect("visible copy follows first world"),
        };
        upload_candidates.push((
            copies
                .map(|world| {
                    placement
                        .rect(world)
                        .center()
                        .distance_sq(viewport.center())
                })
                .fold(f32::INFINITY, f32::min),
            visible_tile.clone(),
        ));
        visible.push(visible_tile);
    }

    upload_candidates.sort_unstable_by(|left, right| {
        left.0.total_cmp(&right.0).then_with(|| {
            (left.1.id.zoom, left.1.id.y, left.1.id.x).cmp(&(
                right.1.id.zoom,
                right.1.id.y,
                right.1.id.x,
            ))
        })
    });
    TileFrameAssembly {
        camera: CameraUniform::new(center_normalized, viewport_size, world_size, first_world),
        visible,
        upload_candidates: upload_candidates
            .into_iter()
            .map(|(_distance, tile)| tile)
            .collect(),
    }
}

impl GpuMap {
    pub(in crate::activity) fn new(
        handle: &WgpuMapHandle,
        metrics: crate::activity::map_runtime::MapMetrics,
    ) -> Self {
        let context = Arc::clone(&handle.context);
        let executor = platform::Executor::new(Arc::clone(&context), metrics.clone());
        let renderer = Arc::new(renderer::Renderer::new(
            handle,
            executor.upload_controller(),
            metrics,
        ));
        Self {
            frame: Arc::new(Frame::default()),
            runtime: WgpuRuntime { renderer, executor },
            labels: LabelCache::default(),
            route: RouteCache::default(),
        }
    }

    pub(in crate::activity) fn paint_callback(&self, rect: Rect) -> Shape {
        egui_adapter::paint_callback(
            rect,
            Arc::clone(&self.frame),
            Arc::clone(&self.runtime.renderer),
        )
    }

    pub(in crate::activity) fn update(
        &mut self,
        frame: crate::activity::map_runtime::SceneFrame<'_, '_>,
    ) -> ScenePerf {
        let crate::activity::map_runtime::SceneFrame {
            scene,
            camera,
            viewport,
            context,
            route,
            backend,
        } = frame;
        let started = Instant::now();
        let _span = tracing::trace_span!("activity_map_frame_assembly").entered();
        self.prepare_route(route, context, backend);
        let TileFrameAssembly {
            camera,
            mut visible,
            upload_candidates,
        } = assemble_tile_frame(scene.renderable_gpu_tiles(), camera, viewport);
        let upload = self.runtime.upload_visible(&upload_candidates);
        if upload.pending_tiles > 0 {
            context.request_repaint();
        }
        let visible_tiles_settling = upload.pending_tiles > 0;
        visible.retain(|tile| tile.tile.is_publishable());
        let route_ready = matches!(self.route.state, RoutePreparation::Ready { .. });
        let route = self.route.visible(route);
        let visible_tiles = visible.len();
        self.frame = Arc::new(Frame {
            camera,
            visible,
            route,
            visible_tiles_settling,
        });
        let label_metrics = self.labels.metrics();
        ScenePerf {
            route_ready,
            milliseconds: started.elapsed().as_secs_f32() * 1_000.0,
            visible_tiles,
            label_milliseconds: label_metrics.milliseconds,
            label_backlog: label_metrics.backlog,
            stale_work: label_metrics.stale_work,
            queued_upload_bytes: upload.queued_bytes,
            uploaded_bytes: upload.uploaded_bytes,
            pending_upload_tiles: upload.pending_tiles,
            partial_upload_tiles: upload.partial_tiles,
            completed_upload_tiles: upload.completed_tiles,
            upload_milliseconds: upload.milliseconds,
            upload_budget_overruns: upload.budget_overruns,
        }
    }

    pub(in crate::activity) fn paint_labels(
        &mut self,
        frame: crate::activity::map_runtime::LabelFrame<'_>,
    ) {
        let crate::activity::map_runtime::LabelFrame {
            ui,
            scene,
            camera,
            viewport,
            backend,
        } = frame;
        while let Some(result) = self.runtime.poll_label() {
            self.labels.apply(result);
        }
        let frame = &self.frame;
        let visible = frame.visible.clone();
        if visible.is_empty() {
            return;
        }

        let view = LabelView::new(camera, viewport);
        if scene.pending_visible_tiles() > 0 || frame.visible_tiles_settling {
            self.labels.paint(ui, view, viewport);
            return;
        }
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

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct CameraUniform {
    center_high_low: [f32; 4],
    viewport_world_size_first_world: [f32; 4],
    projection_scale_offset: [f32; 4],
}

impl CameraUniform {
    fn new(center: [f64; 2], viewport: [f32; 2], world_size: f64, first_world: i32) -> Self {
        let (center_x_high, center_x_low) = split_f64(center[0]);
        let (center_y_high, center_y_low) = split_f64(center[1]);
        Self {
            center_high_low: [center_x_high, center_y_high, center_x_low, center_y_low],
            projection_scale_offset: [1.0, 1.0, 0.0, 0.0],
            viewport_world_size_first_world: [
                viewport[0],
                viewport[1],
                world_size as f32,
                first_world as f32,
            ],
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
    start_join: [f32; 2],
    end_join: [f32; 2],
    caps: [f32; 2],
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
    #[cfg(any(target_arch = "wasm32", test))]
    first_draw: Option<std::sync::Mutex<Option<Arc<upload_trace::UploadTrace>>>>,
    _source: Arc<CpuTileMesh>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    _uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    index_count: u32,
}

impl GpuTile {
    #[cfg(not(target_arch = "wasm32"))]
    fn new(context: &UploadContext, id: TileId, source: Arc<CpuTileMesh>) -> Self {
        debug_assert!(source.vertices.retained_bytes() >= source.vertices.as_bytes().len());
        debug_assert!(source.indices.retained_bytes() >= source.indices.as_bytes().len());
        let _creation = context.resource_creation.enter();
        let device = &context.device;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity map tile vertices"),
            contents: source.vertices.as_bytes(),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity map tile indices"),
            contents: source.indices.as_bytes(),
            usage: wgpu::BufferUsages::INDEX,
        });
        let (uniform, bind_group) = tile_binding(context, id);
        Self {
            index_count: u32::try_from(source.indices.len()).unwrap_or(u32::MAX),
            #[cfg(test)]
            first_draw: None,
            _source: source,
            vertices,
            indices,
            _uniform: uniform,
            bind_group,
        }
    }
}

fn tile_binding(context: &UploadContext, id: TileId) -> (wgpu::Buffer, wgpu::BindGroup) {
    let device = &context.device;
    let tile_count = 2.0_f64.powi(i32::from(id.zoom));
    let (origin_x_high, origin_x_low) = split_f64(f64::from(id.x) / tile_count);
    let (origin_y_high, origin_y_low) = split_f64(f64::from(id.y) / tile_count);
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("activity map tile uniform"),
        contents: bytemuck::bytes_of(&TileUniform {
            origin_high_low: [origin_x_high, origin_y_high, origin_x_low, origin_y_low],
            normalized_point_scale_padding: [(1.0 / (tile_count * 512.0)) as f32, 0.0, 0.0, 0.0],
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
    (uniform, bind_group)
}

struct GpuRoute {
    _source: Arc<CpuRoute>,
    origin_bind_group: wgpu::BindGroup,
    segment_count: u32,
    segments: wgpu::Buffer,
    _origin: wgpu::Buffer,
}

impl GpuRoute {
    fn new(context: &UploadContext, origin: [f64; 2], source: Arc<CpuRoute>) -> Self {
        let _creation = context.resource_creation.enter();
        let device = &context.device;
        let segments = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("activity route segments"),
            contents: bytemuck::cast_slice(&source.segments),
            usage: wgpu::BufferUsages::VERTEX,
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
        let origin_bind_group =
            route_origin_bind_group(device, &context.route_source_layout, &origin);
        Self {
            segment_count: u32::try_from(source.segments.len()).unwrap_or(u32::MAX),
            _source: source,
            origin_bind_group,
            segments,
            _origin: origin,
        }
    }
}

fn route_origin_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    origin: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("activity route origin bind group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: origin.as_entire_binding(),
        }],
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
        sample_count: u32,
        camera_layout: &wgpu::BindGroupLayout,
        tile_layout: &wgpu::BindGroupLayout,
        route_source_layout: &wgpu::BindGroupLayout,
        route_style_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("activity map shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_map.wgsl").into()),
        });
        Self {
            pipeline: tile_pipeline(
                device,
                target_format,
                sample_count,
                &shader,
                camera_layout,
                tile_layout,
            ),
            route_pipeline: route_pipeline(
                device,
                target_format,
                sample_count,
                &shader,
                camera_layout,
                route_source_layout,
                route_style_layout,
            ),
        }
    }
}

struct UploadContext {
    device: wgpu::Device,
    resource_creation: platform::ResourceCreationGate,
    camera_layout: wgpu::BindGroupLayout,
    tile_layout: wgpu::BindGroupLayout,
    route_source_layout: wgpu::BindGroupLayout,
    route_style_layout: wgpu::BindGroupLayout,
}

impl UploadContext {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            device: device.clone(),
            resource_creation: platform::ResourceCreationGate::new(),
            camera_layout: camera_layout(device),
            tile_layout: tile_uniform_layout(device),
            route_source_layout: route_source_layout(device),
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
    sample_count: u32,
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
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn route_source_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    uniform_layout::<RouteSourceUniform>(
        device,
        "activity route source layout",
        wgpu::ShaderStages::VERTEX,
    )
}

fn route_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    sample_count: u32,
    shader: &wgpu::ShaderModule,
    camera_layout: &wgpu::BindGroupLayout,
    route_source_layout: &wgpu::BindGroupLayout,
    route_style_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("activity route pipeline layout"),
        bind_group_layouts: &[
            Some(camera_layout),
            Some(route_source_layout),
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
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: core::mem::size_of::<RouteSegment>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x2,
                    1 => Float32x2,
                    2 => Float32x2,
                    3 => Float32x2,
                    4 => Float32x2,
                    5 => Float32x2,
                    6 => Float32x2
                ],
            })],
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
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
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

    fn labelled_world_tile() -> Arc<PreparedGpuTile> {
        prepare_local_browser_tile(Tile::Vector {
            shapes: vec![Shape::rect_filled(
                Rect::from_min_max(pos2(0.0, 0.0), pos2(512.0, 512.0)),
                0.0,
                Color32::BLUE,
            )],
            texts: vec![walkers::Text {
                text: "World".to_owned(),
                position: pos2(256.0, 256.0),
                font_size: 12.0,
                text_color: Color32::WHITE,
                halo_color: Color32::BLACK,
                halo_width: 0.0,
                angle: 0.0,
                placement: walkers::Placement::Point,
            }],
        })
        .unwrap()
        .into_prepared()
        .unwrap()
    }

    #[test]
    fn world_copies_share_one_upload_and_repeat_labels_with_geometry() {
        let id = TileId {
            zoom: 0,
            x: 0,
            y: 0,
        };
        let tile = labelled_world_tile();
        let mut camera = super::super::camera::MapCamera::default();
        camera.set_zoom(0.0);
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(1024.0, 700.0));
        let frame = assemble_tile_frame([(&id, &tile)], &camera, viewport);
        assert_eq!(frame.visible.len(), 1);
        assert_eq!(frame.upload_candidates.len(), 1);
        assert!(Arc::ptr_eq(&frame.visible[0].tile, &tile));
        assert!(Arc::ptr_eq(&frame.upload_candidates[0].tile, &tile));
        assert_eq!(frame.visible[0].instances, 0..5);
        assert!((frame.camera.viewport_world_size_first_world[3] + 2.0).abs() < f32::EPSILON);

        let mut cache = LabelCache::default();
        let task = cache.request(
            &frame.visible,
            LabelView::new(&camera, viewport),
            egui::Context::default(),
        );
        let request = labels::label_request(&task);
        assert_eq!(request.texts.len(), 5);
        for (text, x) in request.texts.iter().zip([0.0, 256.0, 512.0, 768.0, 1024.0]) {
            assert_eq!(text.text, "World");
            assert_f32_pair_eq(text.position, [x, 350.0]);
        }
    }

    #[test]
    fn dateline_tiles_select_different_instances_without_duplicate_uploads() {
        let west = TileId {
            zoom: 2,
            x: 0,
            y: 1,
        };
        let east = TileId {
            zoom: 2,
            x: 3,
            y: 1,
        };
        let tile = labelled_world_tile();
        let mut camera = super::super::camera::MapCamera::default();
        camera.center_at(walkers::lon_lat(176.4, 0.0));
        camera.set_zoom(3.0);
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(256.0, 512.0));
        let frame = assemble_tile_frame([(&west, &tile), (&east, &tile)], &camera, viewport);
        assert_eq!(frame.visible.len(), 2);
        assert_eq!(frame.upload_candidates.len(), 2);
        assert_eq!(frame.visible[0].instances, 1..2);
        assert_eq!(frame.visible[1].instances, 0..1);
    }

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

        for sample_count in [1, 4] {
            for format in [
                wgpu::TextureFormat::Rgba8UnormSrgb,
                wgpu::TextureFormat::Rgba8Unorm,
            ] {
                let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let _resources = Resources::new(
                    &device,
                    format,
                    sample_count,
                    &context.camera_layout,
                    &context.tile_layout,
                    &context.route_source_layout,
                    &context.route_style_layout,
                );
                let error = futures_lite::future::block_on(error_scope.pop());

                assert!(
                    error.is_none(),
                    "map pipeline validation failed for {sample_count}x {format:?}: {error:?}"
                );
            }
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
            texts: vec![walkers::Text {
                text: "Helsinki 🚲".to_owned(),
                position: pos2(7.0, 8.0),
                font_size: 13.0,
                text_color: Color32::WHITE,
                halo_color: Color32::BLACK,
                halo_width: 1.0,
                angle: 0.25,
                placement: walkers::Placement::Line,
            }],
        };

        let transfer = encode_browser_tile(tile).unwrap();
        let decoded = admit_browser_tile(&transfer, 64)
            .unwrap()
            .into_prepared()
            .unwrap();

        assert_eq!(decoded.mesh.vertices.len(), 3);
        assert_f32_pair_eq(decoded.mesh.vertices.get(2).unwrap().position, [5.0, 6.0]);
        assert_eq!(decoded.mesh.indices.get(0), Some(0));
        assert_eq!(decoded.mesh.indices.get(2), Some(2));
        assert_eq!(decoded.mesh.texts[0].text, "Helsinki 🚲");
        assert_eq!(decoded.mesh.texts[0].placement, walkers::Placement::Line);
        assert!(decoded.gpu.load().is_none());
        assert!(!decoded.is_publishable());
    }

    #[test]
    fn browser_producer_rejects_complexity_and_text_before_encoding() {
        let tile = |shapes, texts| Tile::Vector { shapes, texts };
        assert!(
            encode_browser_tile(tile(
                vec![
                    Shape::Noop;
                    BrowserTileLimits::BROWSER.prepared_bytes / std::mem::size_of::<Shape>() + 1
                ],
                vec![]
            ))
            .is_err()
        );
        let path = Shape::line(
            vec![pos2(0.0, 0.0); BrowserTileLimits::BROWSER.path_points + 1],
            egui::Stroke::new(1.0, Color32::WHITE),
        );
        assert!(encode_browser_tile(tile(vec![path], vec![])).is_err());
        let text = walkers::Text {
            text: "x".repeat(BrowserTileLimits::BROWSER.text_bytes + 1),
            position: pos2(0.0, 0.0),
            font_size: 12.0,
            text_color: Color32::WHITE,
            halo_color: Color32::BLACK,
            halo_width: 1.0,
            angle: 0.0,
            placement: walkers::Placement::Point,
        };
        assert!(encode_browser_tile(tile(vec![], vec![text.clone()])).is_err());
        assert!(prepare_local_browser_tile(tile(vec![], vec![text])).is_err());
    }

    #[test]
    fn browser_local_preparation_matches_worker_and_leaves_upload_pending() {
        let tile = Tile::Vector {
            shapes: vec![Shape::rect_filled(
                Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)),
                0.0,
                Color32::WHITE,
            )],
            texts: Vec::new(),
        };
        let local = prepare_local_browser_tile(tile.clone())
            .unwrap()
            .into_prepared()
            .unwrap();
        let worker = admit_browser_tile(&encode_browser_tile(tile).unwrap(), 64)
            .unwrap()
            .into_prepared()
            .unwrap();
        assert_eq!(
            local.mesh.vertices.as_bytes(),
            worker.mesh.vertices.as_bytes()
        );
        assert_eq!(
            local.mesh.indices.as_bytes(),
            worker.mesh.indices.as_bytes()
        );
        assert!(!local.mesh.indices.is_empty());
        assert!(local.gpu.load().is_none());
        assert!(!local.is_publishable());
    }

    #[test]
    fn browser_protocol_rejects_malformed_lengths_and_indices() {
        assert!(BrowserTilePacketBuilder::new(1, 0, 0, 0).is_err());
        assert!(BrowserTilePacketBuilder::new(0, 4, 0, 0).is_err());
        assert!(BrowserTilePacketBuilder::new(0, 0, 1, 0).is_err());

        let vertex = Vertex {
            position: [0.0; 2],
            color: [255; 4],
        };
        let indices = [0_u32, 1, 2];
        let mut builder = BrowserTilePacketBuilder::new(
            std::mem::size_of::<Vertex>(),
            std::mem::size_of_val(&indices),
            0,
            0,
        )
        .unwrap();
        allocate_browser_tile(&mut builder);
        builder
            .append(BrowserTileSection::Vertices, bytemuck::bytes_of(&vertex))
            .unwrap();
        assert!(
            builder
                .append(BrowserTileSection::Indices, bytemuck::cast_slice(&indices))
                .is_err()
        );
    }

    #[test]
    fn browser_direct_copy_validates_indices_without_advancing_on_failure() {
        let mut builder = BrowserTilePacketBuilder::new(12, 12, 0, 0).unwrap();
        allocate_browser_tile(&mut builder);
        builder
            .append_from(BrowserTileSection::Vertices, 12, |out| out.fill(0))
            .unwrap();
        assert!(
            builder
                .append_from(BrowserTileSection::Indices, 12, |out| out.fill(255))
                .is_err()
        );
        assert_eq!(
            builder.next_section(),
            Some((BrowserTileSection::Indices, 12))
        );
        builder
            .append_from(BrowserTileSection::Indices, 12, |out| out.fill(0))
            .unwrap();
        let mesh = builder.finish().unwrap().into_prepared().unwrap();
        assert_eq!(mesh.mesh.indices.get(0), Some(0));
        assert_eq!(mesh.mesh.indices.len(), 3);
    }

    #[test]
    fn browser_protocol_rejects_oversized_worker_completions_before_decoding() {
        assert_eq!(
            BrowserTilePacketBuilder::new(
                BrowserTileLimits::BROWSER.upload_bytes
                    - BrowserTileLimits::BROWSER.upload_bytes % std::mem::size_of::<Vertex>()
                    + std::mem::size_of::<Vertex>(),
                0,
                0,
                0
            )
            .err()
            .as_deref(),
            Some("prepared map tile exceeded the 16 MiB upload limit")
        );
    }

    #[test]
    fn browser_protocol_rejects_invalid_utf8_ranges_and_oversized_text() {
        let invalid_utf8 = BrowserTextRecord {
            string_offset: 0,
            string_length: 1,
            position: [0.0; 2],
            font_size: 12.0,
            text_color: [255; 4],
            halo_color: [0; 4],
            halo_width: 1.0,
            angle: 0.0,
            line_placement: 0,
        };
        let mut builder =
            BrowserTilePacketBuilder::new(0, 0, std::mem::size_of::<BrowserTextRecord>(), 1)
                .unwrap();
        allocate_browser_tile(&mut builder);
        builder
            .append(BrowserTileSection::Strings, &[0xff])
            .unwrap();
        assert!(
            builder
                .append(
                    BrowserTileSection::TextRecords,
                    bytemuck::bytes_of(&invalid_utf8)
                )
                .is_err()
        );

        let outside = BrowserTextRecord {
            string_offset: 1,
            ..invalid_utf8
        };
        let mut builder =
            BrowserTilePacketBuilder::new(0, 0, std::mem::size_of::<BrowserTextRecord>(), 1)
                .unwrap();
        allocate_browser_tile(&mut builder);
        builder.append(BrowserTileSection::Strings, b"a").unwrap();
        assert!(
            builder
                .append(
                    BrowserTileSection::TextRecords,
                    bytemuck::bytes_of(&outside)
                )
                .is_err()
        );

        let oversized = BrowserTextRecord {
            string_offset: 0,
            string_length: u32::try_from(BrowserTileLimits::BROWSER.text_bytes + 1).unwrap(),
            ..invalid_utf8
        };
        let strings = vec![b'a'; BrowserTileLimits::BROWSER.text_bytes + 1];
        let mut builder = BrowserTilePacketBuilder::new(
            0,
            0,
            std::mem::size_of::<BrowserTextRecord>(),
            strings.len(),
        )
        .unwrap();
        allocate_browser_tile(&mut builder);
        builder
            .append(BrowserTileSection::Strings, &strings)
            .unwrap();
        assert!(
            builder
                .append(
                    BrowserTileSection::TextRecords,
                    bytemuck::bytes_of(&oversized)
                )
                .is_err()
        );
    }

    #[test]
    fn browser_admission_is_bounded_and_publishes_only_complete_packets() {
        let vertices = vec![0; std::mem::size_of::<Vertex>() * 100];
        let builder = BrowserTilePacketBuilder::new(vertices.len(), 0, 0, 0).unwrap();
        assert!(builder.finish().is_err());

        let mut builder = BrowserTilePacketBuilder::new(vertices.len(), 0, 0, 0).unwrap();
        allocate_browser_tile(&mut builder);
        while let Some((section, length)) = builder.next_chunk(64) {
            assert!(length <= 64);
            let offset = vertices.len() - builder.next_section().unwrap().1;
            builder
                .append(section, &vertices[offset..offset + length])
                .unwrap();
        }
        assert!(builder.finish().is_ok());

        assert!(
            BrowserTilePacketBuilder::new(0, 0, 0, 0)
                .unwrap()
                .finish()
                .unwrap()
                .into_prepared()
                .is_none()
        );
    }

    #[test]
    fn typed_and_browser_packed_mesh_buffers_have_identical_gpu_bytes() {
        let vertices = vec![
            Vertex {
                position: [1.0, 2.0],
                color: [1, 2, 3, 4],
            },
            Vertex {
                position: [3.0, 4.0],
                color: [5, 6, 7, 8],
            },
        ];
        let typed = MeshBuffer::typed(vertices.clone());
        let packed =
            MeshBuffer::<Vertex>::packed(bytemuck::cast_slice(&vertices).to_vec()).unwrap();

        assert_eq!(typed.len(), packed.len());
        assert_eq!(typed.as_bytes(), packed.as_bytes());
    }

    fn allocate_browser_tile(builder: &mut BrowserTilePacketBuilder) {
        while builder.allocate_next().unwrap().is_some() {}
    }

    #[test]
    fn browser_packet_construction_defers_all_destination_allocations() {
        let mut builder = BrowserTilePacketBuilder::new(12 * 100_000, 12, 40, 4).unwrap();
        assert_eq!(builder.vertices.capacity(), 0);
        assert_eq!(builder.indices.capacity(), 0);
        assert_eq!(builder.strings.capacity(), 0);
        assert_eq!(builder.texts.capacity(), 0);
        assert!(
            builder
                .append(BrowserTileSection::Strings, b"text")
                .is_err()
        );
        assert_eq!(
            builder.retained_bytes(),
            2 * (12 * 100_000 + 12) + 40 + 12 + std::mem::size_of::<walkers::Text>()
        );
        assert_eq!(builder.allocate_next().unwrap(), Some(4));
        assert_eq!(builder.vertices.capacity(), 0);
        assert_eq!(builder.texts.capacity(), 0);
        assert_eq!(
            builder.allocate_next().unwrap(),
            Some(std::mem::size_of::<walkers::Text>())
        );
        assert_eq!(builder.vertices.capacity(), 0);
        assert_eq!(builder.allocate_next().unwrap(), Some(12 * 100_000));
        assert_eq!(builder.indices.capacity(), 0);
        assert_eq!(builder.allocate_next().unwrap(), Some(12));
        assert_eq!(builder.allocate_next().unwrap(), None);
        builder
            .append(BrowserTileSection::Strings, b"text")
            .unwrap();
    }

    #[test]
    fn browser_packet_rejects_string_expansion_beyond_retention_allowance() {
        let record = BrowserTextRecord {
            string_offset: 0,
            string_length: 1,
            position: [0.0; 2],
            font_size: 12.0,
            text_color: [255; 4],
            halo_color: [0; 4],
            halo_width: 1.0,
            angle: 0.0,
            line_placement: 0,
        };
        let mut builder = BrowserTilePacketBuilder::new(0, 0, 80, 1).unwrap();
        allocate_browser_tile(&mut builder);
        builder.append(BrowserTileSection::Strings, b"a").unwrap();
        builder
            .append(BrowserTileSection::TextRecords, bytemuck::bytes_of(&record))
            .unwrap();
        assert_eq!(
            builder
                .append(BrowserTileSection::TextRecords, bytemuck::bytes_of(&record))
                .unwrap_err(),
            "browser tile decoded strings exceeded transferred storage"
        );
    }

    fn admit_browser_tile(
        transfer: &BrowserTileTransfer,
        maximum_chunk: usize,
    ) -> Result<BrowserTilePacket, String> {
        let sources = transfer.parts();
        let [vertices, indices, text_records, strings] = sources;
        let mut offsets = [0; 4];
        let mut builder = BrowserTilePacketBuilder::new(
            vertices.len(),
            indices.len(),
            text_records.len(),
            strings.len(),
        )?;
        allocate_browser_tile(&mut builder);
        while let Some((section, length)) = builder.next_chunk(maximum_chunk) {
            let slot = match section {
                BrowserTileSection::Vertices => 0,
                BrowserTileSection::Indices => 1,
                BrowserTileSection::TextRecords => 2,
                BrowserTileSection::Strings => 3,
            };
            let start = offsets[slot];
            let end = start + length;
            builder.append(section, &sources[slot][start..end])?;
            offsets[slot] = end;
        }
        builder.finish()
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
                    coordinate: Some([-0.1248, 51.5075]),
                    speed: Some(0.5),
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
        assert_eq!(prepared.route.segments.len(), 2);
        assert_f32_pair_eq(prepared.route.segments[0].speed, [0.25, 0.75]);
        assert_f32_pair_eq(prepared.route.segments[0].sample_indices, [40.0, 41.0]);
        assert_f32_pair_eq(prepared.route.segments[0].caps, [1.0, 0.0]);
        assert_f32_pair_eq(prepared.route.segments[1].caps, [0.0, 1.0]);
        assert_f32_pair_eq(
            prepared.route.segments[0].end_join,
            prepared.route.segments[1].start_join,
        );
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
                start_join: [0.0, 1.0],
                end_join: [0.0, 1.0],
                caps: [1.0, 1.0],
            }],
        };
        assert!(decode_browser_route(&postcard::to_stdvec(&invalid).unwrap()).is_err());
    }
}
