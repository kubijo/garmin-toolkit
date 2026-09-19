use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use web_time::Instant;

use super::super::upload_trace::UploadTrace;
use super::super::{
    BrowserLabelTask, BrowserRouteTask, CpuTileMesh, GpuTile, LabelResult, LabelTask,
    PreparedGpuTile, RouteResult, RouteTask, UploadContext, UploadStats, VisibleTile, tile_binding,
};
use crate::activity::map_runtime::Backend;
use crate::activity::map_runtime::MapMetrics;

#[cfg(test)]
use super::super::Vertex;

const BROWSER_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
const BROWSER_UPLOAD_FRAME_BYTES: usize = 4 * 1024 * 1024;
const BROWSER_UPLOAD_FRAME_TIME: Duration = Duration::from_micros(1_500);
const RETAINED_UPLOAD_LIMIT: usize = 32;
const RETAINED_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

pub(in crate::activity) struct ResourceCreationGate;

impl ResourceCreationGate {
    pub(in crate::activity) const fn new() -> Self {
        Self
    }

    pub(in crate::activity) const fn enter(&self) -> &Self {
        self
    }
}

struct GpuTileUpload {
    source: Arc<CpuTileMesh>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    cursor: UploadCursor,
}

impl GpuTileUpload {
    fn allocate_vertices(context: &UploadContext, source: &CpuTileMesh) -> wgpu::Buffer {
        context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("activity map tile vertices"),
            size: u64::try_from(vertex_bytes(source)).unwrap_or(u64::MAX),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn allocate_indices(context: &UploadContext, source: &CpuTileMesh) -> wgpu::Buffer {
        context.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("activity map tile indices"),
            size: u64::try_from(index_bytes(source)).unwrap_or(u64::MAX),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn finish_allocation(
        context: &UploadContext,
        id: walkers::TileId,
        source: Arc<CpuTileMesh>,
        vertices: wgpu::Buffer,
        indices: wgpu::Buffer,
    ) -> Self {
        let (uniform, bind_group) = tile_binding(context, id);
        let cursor = UploadCursor::new(vertex_bytes(&source), index_bytes(&source));
        Self {
            source,
            vertices,
            indices,
            uniform,
            bind_group,
            cursor,
        }
    }

    fn remaining_bytes(&self) -> usize {
        self.cursor.remaining_bytes()
    }

    fn write_next(&mut self, queue: &wgpu::Queue, maximum_bytes: usize) -> usize {
        let Some(chunk) = self.cursor.take(maximum_bytes) else {
            return 0;
        };
        let (buffer, source) = match chunk.buffer {
            TileBuffer::Vertices => (&self.vertices, self.source.vertices.as_bytes()),
            TileBuffer::Indices => (&self.indices, self.source.indices.as_bytes()),
        };
        debug_assert_eq!(chunk.offset % wgpu::COPY_BUFFER_ALIGNMENT as usize, 0);
        debug_assert_eq!(chunk.length % wgpu::COPY_BUFFER_ALIGNMENT as usize, 0);
        queue.write_buffer(
            buffer,
            u64::try_from(chunk.offset).unwrap_or(u64::MAX),
            &source[chunk.offset..chunk.offset + chunk.length],
        );
        chunk.length
    }

    fn finish(self, trace: Option<Arc<UploadTrace>>) -> GpuTile {
        GpuTile {
            first_draw: trace.map(|trace| Mutex::new(Some(trace))),
            index_count: u32::try_from(self.source.indices.len()).unwrap_or(u32::MAX),
            _source: self.source,
            vertices: self.vertices,
            indices: self.indices,
            _uniform: self.uniform,
            bind_group: self.bind_group,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TileBuffer {
    Vertices,
    Indices,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UploadChunk {
    buffer: TileBuffer,
    offset: usize,
    length: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UploadCursor {
    vertex_bytes: usize,
    index_bytes: usize,
    vertex_offset: usize,
    index_offset: usize,
}

impl UploadCursor {
    const fn new(vertex_bytes: usize, index_bytes: usize) -> Self {
        Self {
            vertex_bytes,
            index_bytes,
            vertex_offset: 0,
            index_offset: 0,
        }
    }

    fn remaining_bytes(self) -> usize {
        self.vertex_bytes
            .saturating_sub(self.vertex_offset)
            .saturating_add(self.index_bytes.saturating_sub(self.index_offset))
    }

    fn take(&mut self, maximum_bytes: usize) -> Option<UploadChunk> {
        if self.vertex_offset < self.vertex_bytes {
            let length = self
                .vertex_bytes
                .saturating_sub(self.vertex_offset)
                .min(maximum_bytes);
            let chunk = UploadChunk {
                buffer: TileBuffer::Vertices,
                offset: self.vertex_offset,
                length,
            };
            self.vertex_offset = self.vertex_offset.saturating_add(length);
            return (length > 0).then_some(chunk);
        }
        let length = self
            .index_bytes
            .saturating_sub(self.index_offset)
            .min(maximum_bytes);
        let chunk = UploadChunk {
            buffer: TileBuffer::Indices,
            offset: self.index_offset,
            length,
        };
        self.index_offset = self.index_offset.saturating_add(length);
        (length > 0).then_some(chunk)
    }
}

fn upload_bytes(source: &CpuTileMesh) -> usize {
    vertex_bytes(source).saturating_add(index_bytes(source))
}

fn vertex_bytes(source: &CpuTileMesh) -> usize {
    source.vertices.as_bytes().len()
}

fn index_bytes(source: &CpuTileMesh) -> usize {
    source.indices.as_bytes().len()
}

fn source_retained_bytes(source: &CpuTileMesh) -> usize {
    source
        .vertices
        .retained_bytes()
        .saturating_add(source.indices.retained_bytes())
        .saturating_add(
            source
                .texts
                .capacity()
                .saturating_mul(std::mem::size_of::<walkers::Text>()),
        )
        .saturating_add(source.texts.iter().fold(0_usize, |bytes, text| {
            bytes.saturating_add(text.text.capacity())
        }))
}

pub(in crate::activity) struct Executor {
    uploads: UploadController,
    label_results: Arc<Mutex<VecDeque<LabelResult>>>,
    route_results: Arc<Mutex<VecDeque<RouteResult>>>,
}

impl Executor {
    pub(in crate::activity) fn new(context: Arc<UploadContext>, metrics: MapMetrics) -> Self {
        Self {
            uploads: UploadController::new(context, metrics),
            label_results: Arc::new(Mutex::new(VecDeque::new())),
            route_results: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub(in crate::activity) fn upload_controller(&self) -> UploadController {
        self.uploads.clone()
    }

    pub(in crate::activity) fn upload_visible(&self, visible: &[VisibleTile]) -> UploadStats {
        self.uploads.reconcile(visible)
    }

    pub(in crate::activity) fn poll_label(&self) -> Option<LabelResult> {
        self.label_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    pub(in crate::activity) fn schedule_label(
        &self,
        task: LabelTask,
        backend: &dyn Backend,
    ) -> (Option<LabelResult>, bool) {
        backend.submit_labels(BrowserLabelTask {
            task,
            results: Arc::clone(&self.label_results),
        });
        (None, false)
    }

    pub(in crate::activity) fn poll_route(&self) -> Option<RouteResult> {
        self.route_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    pub(in crate::activity) fn schedule_route(&self, task: RouteTask, backend: &dyn Backend) {
        backend.submit_route(BrowserRouteTask {
            task,
            results: Arc::clone(&self.route_results),
            upload: Arc::clone(&self.uploads.context),
        });
    }
}

#[derive(Clone)]
pub(in crate::activity) struct UploadController {
    context: Arc<UploadContext>,
    queue: Arc<Mutex<BrowserUploadQueue>>,
}

impl UploadController {
    fn new(context: Arc<UploadContext>, metrics: MapMetrics) -> Self {
        Self {
            context,
            queue: Arc::new(Mutex::new(BrowserUploadQueue {
                metrics,
                ..BrowserUploadQueue::default()
            })),
        }
    }

    fn reconcile(&self, visible: &[VisibleTile]) -> UploadStats {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reconcile(visible)
    }

    pub(super) fn prepare(
        &self,
        queue: &wgpu::Queue,
        metrics: &crate::activity::map_runtime::MapMetrics,
    ) {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .advance(&self.context, queue, metrics);
    }
}

#[derive(Default)]
struct BrowserUploadQueue {
    metrics: MapMetrics,
    entries: HashMap<walkers::TileId, TileUpload>,
    priority: VecDeque<walkers::TileId>,
    current: HashSet<walkers::TileId>,
    generation: u64,
    last_frame: UploadFrameStats,
    budget_overruns: u64,
}

impl BrowserUploadQueue {
    fn reconcile(&mut self, visible: &[VisibleTile]) -> UploadStats {
        self.generation = self.generation.saturating_add(1);
        let mut current = std::mem::take(&mut self.current);
        current.clear();
        current.reserve(visible.len());
        let mut priority = std::mem::take(&mut self.priority);
        priority.clear();
        priority.reserve(visible.len());
        for candidate in visible {
            if !needs_upload(&candidate.tile) || !current.insert(candidate.id) {
                continue;
            }
            priority.push_back(candidate.id);
            match self.entries.get_mut(&candidate.id) {
                Some(upload) if Arc::ptr_eq(&upload.tile, &candidate.tile) => {
                    upload.last_visible = self.generation;
                }
                Some(upload) => {
                    *upload = TileUpload::new(
                        Arc::clone(&candidate.tile),
                        self.generation,
                        candidate.id,
                        self.metrics.clone(),
                    );
                }
                None => {
                    self.entries.insert(
                        candidate.id,
                        TileUpload::new(
                            Arc::clone(&candidate.tile),
                            self.generation,
                            candidate.id,
                            self.metrics.clone(),
                        ),
                    );
                }
            }
        }
        for (id, upload) in &self.entries {
            if let Some(trace) = &upload.trace {
                trace.visible(current.contains(id));
            }
        }
        self.priority = priority;
        self.prune_retained(&current);
        self.current = current;
        self.stats()
    }

    fn advance(
        &mut self,
        context: &UploadContext,
        queue: &wgpu::Queue,
        metrics: &crate::activity::map_runtime::MapMetrics,
    ) {
        let started = Instant::now();
        let mut budget = UploadBudget::default();
        let mut completed_tiles = 0;
        while budget.can_advance(started.elapsed()) {
            let Some(id) = self.priority.front().copied() else {
                break;
            };
            let Some(upload) = self.entries.get_mut(&id) else {
                self.priority.pop_front();
                continue;
            };
            let work_started = upload.trace.as_ref().map(|trace| {
                trace.begin_work();
                Instant::now()
            });
            let result = upload.advance(context, queue, id, budget.chunk_bytes());
            if let (Some(trace), Some(work_started)) = (&upload.trace, work_started) {
                trace.work(
                    work_started.elapsed().as_secs_f64() * 1_000.0,
                    if let UploadAdvance::Wrote(bytes) = result {
                        bytes
                    } else {
                        0
                    },
                );
            }
            match result {
                UploadAdvance::Advanced => {}
                UploadAdvance::Wrote(0) => break,
                UploadAdvance::Wrote(bytes) => budget.record(bytes),
                UploadAdvance::Published => {
                    if let Some(trace) = &upload.trace {
                        trace.published();
                    }
                    metrics.record_upload(upload.queued_at.elapsed().as_secs_f64() * 1_000.0);
                    completed_tiles += 1;
                    self.entries.remove(&id);
                    self.priority.pop_front();
                }
            }
        }
        for upload in self.entries.values() {
            if let Some(trace) = &upload.trace {
                trace.flush();
            }
        }
        let elapsed = started.elapsed();
        if elapsed > BROWSER_UPLOAD_FRAME_TIME {
            self.budget_overruns = self.budget_overruns.saturating_add(1);
        }
        self.last_frame = UploadFrameStats {
            uploaded_bytes: budget.uploaded_bytes,
            completed_tiles,
            milliseconds: elapsed.as_secs_f32() * 1_000.0,
        };
    }

    fn stats(&self) -> UploadStats {
        let mut queued_bytes = 0;
        let mut pending_tiles = 0;
        let mut partial_tiles = 0;
        for upload in self.priority.iter().filter_map(|id| self.entries.get(id)) {
            queued_bytes += upload.remaining_bytes();
            pending_tiles += 1;
            partial_tiles += usize::from(upload.is_partial());
        }
        UploadStats {
            queued_bytes,
            uploaded_bytes: self.last_frame.uploaded_bytes,
            pending_tiles,
            partial_tiles,
            completed_tiles: self.last_frame.completed_tiles,
            milliseconds: self.last_frame.milliseconds,
            budget_overruns: self.budget_overruns,
        }
    }

    fn prune_retained(&mut self, current: &HashSet<walkers::TileId>) {
        let (retained_count, retained_bytes) = self
            .entries
            .iter()
            .filter(|(id, _upload)| !current.contains(id))
            .fold((0_usize, 0_usize), |(count, bytes), (_id, upload)| {
                (
                    count.saturating_add(1),
                    bytes.saturating_add(upload.retained_bytes()),
                )
            });
        if retained_count <= RETAINED_UPLOAD_LIMIT && retained_bytes <= RETAINED_UPLOAD_BYTES {
            return;
        }
        let mut retained = self
            .entries
            .iter()
            .filter(|(id, _upload)| !current.contains(id))
            .map(|(id, upload)| {
                (
                    *id,
                    upload.is_partial(),
                    upload.last_visible,
                    upload.retained_bytes(),
                )
            })
            .collect::<Vec<_>>();
        retained.sort_unstable_by_key(|(id, is_partial, generation, _bytes)| {
            (*is_partial, *generation, id.zoom, id.y, id.x)
        });
        let mut retained_bytes = retained_bytes;
        let mut retained_count = retained_count;
        for (id, _is_partial, _generation, bytes) in retained {
            if retained_count <= RETAINED_UPLOAD_LIMIT && retained_bytes <= RETAINED_UPLOAD_BYTES {
                break;
            }
            self.entries.remove(&id);
            retained_count = retained_count.saturating_sub(1);
            retained_bytes = retained_bytes.saturating_sub(bytes);
        }
    }
}

#[derive(Default)]
struct UploadBudget {
    uploaded_bytes: usize,
}

impl UploadBudget {
    fn can_advance(&self, elapsed: Duration) -> bool {
        self.uploaded_bytes < BROWSER_UPLOAD_FRAME_BYTES && elapsed < BROWSER_UPLOAD_FRAME_TIME
    }

    fn chunk_bytes(&self) -> usize {
        BROWSER_UPLOAD_CHUNK_BYTES
            .min(BROWSER_UPLOAD_FRAME_BYTES.saturating_sub(self.uploaded_bytes))
    }

    fn record(&mut self, bytes: usize) {
        debug_assert!(bytes <= self.chunk_bytes());
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(bytes);
    }
}

#[derive(Clone, Copy, Default)]
struct UploadFrameStats {
    uploaded_bytes: usize,
    completed_tiles: usize,
    milliseconds: f32,
}

struct TileUpload {
    trace: Option<Arc<UploadTrace>>,
    tile: Arc<PreparedGpuTile>,
    state: TileUploadState,
    last_visible: u64,
    queued_at: Instant,
}

impl TileUpload {
    fn new(
        tile: Arc<PreparedGpuTile>,
        last_visible: u64,
        id: walkers::TileId,
        metrics: MapMetrics,
    ) -> Self {
        Self {
            trace: metrics
                .upload_events_enabled()
                .then(|| Arc::new(UploadTrace::new(id, metrics))),
            tile,
            state: TileUploadState::AllocateVertices,
            last_visible,
            queued_at: Instant::now(),
        }
    }

    fn retained_bytes(&self) -> usize {
        let partial = if self.is_partial() {
            upload_bytes(&self.tile.mesh)
        } else {
            0
        };
        source_retained_bytes(&self.tile.mesh).saturating_add(partial)
    }

    fn remaining_bytes(&self) -> usize {
        match &self.state {
            TileUploadState::AllocateVertices
            | TileUploadState::AllocateIndices { .. }
            | TileUploadState::AllocateBindings { .. } => upload_bytes(&self.tile.mesh),
            TileUploadState::Uploading(upload) | TileUploadState::Finalizing(upload) => {
                upload.remaining_bytes()
            }
            TileUploadState::Ready => 0,
        }
    }

    fn is_partial(&self) -> bool {
        !matches!(
            &self.state,
            TileUploadState::AllocateVertices | TileUploadState::Ready
        )
    }

    fn advance(
        &mut self,
        context: &UploadContext,
        queue: &wgpu::Queue,
        id: walkers::TileId,
        maximum_bytes: usize,
    ) -> UploadAdvance {
        let state = std::mem::replace(&mut self.state, TileUploadState::Ready);
        match state {
            TileUploadState::AllocateVertices => {
                self.state = TileUploadState::AllocateIndices {
                    vertices: GpuTileUpload::allocate_vertices(context, &self.tile.mesh),
                };
                UploadAdvance::Advanced
            }
            TileUploadState::AllocateIndices { vertices } => {
                self.state = TileUploadState::AllocateBindings {
                    vertices,
                    indices: GpuTileUpload::allocate_indices(context, &self.tile.mesh),
                };
                UploadAdvance::Advanced
            }
            TileUploadState::AllocateBindings { vertices, indices } => {
                self.state = TileUploadState::Uploading(GpuTileUpload::finish_allocation(
                    context,
                    id,
                    Arc::clone(&self.tile.mesh),
                    vertices,
                    indices,
                ));
                UploadAdvance::Advanced
            }
            TileUploadState::Uploading(mut upload) if upload.remaining_bytes() > 0 => {
                let written = upload.write_next(queue, maximum_bytes);
                self.state = TileUploadState::Uploading(upload);
                UploadAdvance::Wrote(written)
            }
            TileUploadState::Uploading(upload) => {
                self.state = TileUploadState::Finalizing(upload);
                UploadAdvance::Advanced
            }
            TileUploadState::Finalizing(upload) => {
                self.tile
                    .gpu
                    .store(Some(Arc::new(upload.finish(self.trace.clone()))));
                UploadAdvance::Published
            }
            TileUploadState::Ready => UploadAdvance::Published,
        }
    }
}

enum TileUploadState {
    AllocateVertices,
    AllocateIndices {
        vertices: wgpu::Buffer,
    },
    AllocateBindings {
        vertices: wgpu::Buffer,
        indices: wgpu::Buffer,
    },
    Uploading(GpuTileUpload),
    Finalizing(GpuTileUpload),
    Ready,
}

enum UploadAdvance {
    Advanced,
    Wrote(usize),
    Published,
}

fn needs_upload(tile: &PreparedGpuTile) -> bool {
    !tile.mesh.indices.is_empty() && tile.gpu.load().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::map::gpu_map::MeshBuffer;
    use arc_swap::ArcSwapOption;

    fn tile(id: u32, bytes: usize) -> VisibleTile {
        let vertex_count = bytes / std::mem::size_of::<Vertex>();
        VisibleTile {
            instances: 0..1,
            id: walkers::TileId {
                x: id,
                y: 0,
                zoom: 1,
            },
            tile: Arc::new(PreparedGpuTile {
                mesh: Arc::new(CpuTileMesh {
                    vertices: MeshBuffer::typed(vec![
                        Vertex {
                            position: [0.0; 2],
                            color: [0; 4],
                        };
                        vertex_count.max(1)
                    ]),
                    indices: MeshBuffer::typed(vec![0, 0, 0]),
                    texts: Vec::new(),
                }),
                gpu: ArcSwapOption::empty(),
            }),
        }
    }

    #[test]
    fn reconciliation_preserves_the_callers_center_first_order() {
        let mut queue = BrowserUploadQueue::default();
        let visible = [tile(4, 64), tile(2, 64), tile(8, 64)];

        let stats = queue.reconcile(&visible);

        assert_eq!(
            queue.priority.iter().map(|id| id.x).collect::<Vec<_>>(),
            [4, 2, 8]
        );
        assert_eq!(stats.pending_tiles, 3);
    }

    #[test]
    fn reconciliation_retains_hidden_work_and_replaces_stale_tiles() {
        let mut queue = BrowserUploadQueue::default();
        let original = tile(1, 64);
        let retained = tile(2, 64);
        queue.reconcile(&[original.clone(), retained.clone()]);
        let replacement = tile(2, 128);

        queue.reconcile(std::slice::from_ref(&replacement));

        assert_eq!(queue.entries.len(), 2);
        assert!(Arc::ptr_eq(
            &queue.entries[&replacement.id].tile,
            &replacement.tile
        ));
        assert!(queue.entries.contains_key(&original.id));
        assert_eq!(
            queue.priority.iter().map(|id| id.x).collect::<Vec<_>>(),
            [2]
        );
    }

    #[test]
    fn reconciliation_traces_hidden_returned_and_replaced_uploads() {
        use crate::activity::map::gpu_map::upload_trace::tests::Capture;
        use crate::activity::map_runtime::MapUploadPhase;
        let capture = Capture::default();
        let mut queue = BrowserUploadQueue {
            metrics: MapMetrics::new(capture.clone()),
            ..BrowserUploadQueue::default()
        };
        let original = tile(1, 64);
        queue.reconcile(&[original.clone(), original.clone()]);
        queue.reconcile(&[]);
        queue.reconcile(std::slice::from_ref(&original));
        queue.reconcile(&[tile(1, 128)]);
        drop(queue);
        let events = capture.0.lock().unwrap();
        let first = events[0].upload_id;
        assert_eq!(
            events
                .iter()
                .filter(|event| event.upload_id == first)
                .map(|event| event.event)
                .collect::<Vec<_>>(),
            [
                MapUploadPhase::Queued,
                MapUploadPhase::Hidden,
                MapUploadPhase::Visible,
                MapUploadPhase::Released,
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.upload_id != first)
                .map(|event| event.event)
                .collect::<Vec<_>>(),
            [MapUploadPhase::Queued, MapUploadPhase::Released,]
        );
    }

    #[test]
    fn upload_budget_stops_at_the_byte_and_time_limits() {
        let mut budget = UploadBudget::default();

        assert_eq!(budget.chunk_bytes(), BROWSER_UPLOAD_CHUNK_BYTES);
        assert!(budget.can_advance(Duration::from_micros(1_499)));
        assert!(!budget.can_advance(BROWSER_UPLOAD_FRAME_TIME));

        while budget.can_advance(Duration::ZERO) {
            let chunk = budget.chunk_bytes();
            assert!(chunk > 0 && chunk <= 256 * 1024);
            budget.record(chunk);
        }

        assert_eq!(budget.uploaded_bytes, 4 * 1024 * 1024);
        assert_eq!(budget.chunk_bytes(), 0);
        assert!(!budget.can_advance(Duration::ZERO));
    }

    #[test]
    fn disabled_telemetry_still_uploads_and_publishes_with_latency_measurement() {
        use crate::activity::map_runtime::{MapMetricsSink, MapPerformanceSample, MapUploadEvent};
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone, Default)]
        struct Disabled(Arc<AtomicUsize>);
        impl MapMetricsSink for Disabled {
            fn record(&self, _: MapPerformanceSample) {}
            fn upload_events_enabled(&self) -> bool {
                false
            }
            fn record_upload_event(&self, _: MapUploadEvent) {
                panic!("disabled telemetry emitted an event");
            }
            fn record_upload(&self, milliseconds: f64) {
                assert!(milliseconds.is_finite() && milliseconds >= 0.0);
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = futures_lite::future::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .expect("upload telemetry regression requires a WGPU adapter");
        let (device, gpu_queue) = futures_lite::future::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default()),
        )
        .unwrap();
        let context = UploadContext::new(&device);
        let sink = Disabled::default();
        let metrics = MapMetrics::new(sink.clone());
        let mut uploads = BrowserUploadQueue {
            metrics: metrics.clone(),
            ..BrowserUploadQueue::default()
        };
        let candidate = tile(1, BROWSER_UPLOAD_CHUNK_BYTES + 16);
        uploads.reconcile(std::slice::from_ref(&candidate));
        assert!(uploads.entries[&candidate.id].trace.is_none());
        uploads.reconcile(&[]);
        uploads.advance(&context, &gpu_queue, &metrics);
        assert_eq!(uploads.stats().uploaded_bytes, 0);
        uploads.reconcile(&[candidate.clone(), candidate.clone()]);
        let mut written = 0;
        for _ in 0..128 {
            uploads.advance(&context, &gpu_queue, &metrics);
            written += uploads.stats().uploaded_bytes;
            if candidate.tile.is_publishable() {
                break;
            }
        }
        assert!(candidate.tile.is_publishable());
        assert_eq!(written, upload_bytes(&candidate.tile.mesh));
        assert_eq!(sink.0.load(Ordering::Relaxed), 1);
        assert!(candidate.tile.gpu.load_full().unwrap().first_draw.is_none());
        assert!(uploads.entries.is_empty());
    }

    #[test]
    fn partially_written_upload_survives_hidden_frames_and_resumes_without_reupload() {
        use crate::activity::map::gpu_map::upload_trace::tests::Capture;
        use crate::activity::map_runtime::MapUploadPhase;

        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = futures_lite::future::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .expect("upload retention regression requires a WGPU adapter");
        let (device, gpu_queue) = futures_lite::future::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default()),
        )
        .unwrap();
        let context = UploadContext::new(&device);
        let capture = Capture::default();
        let metrics = MapMetrics::new(capture.clone());
        let mut uploads = BrowserUploadQueue {
            metrics: metrics.clone(),
            ..BrowserUploadQueue::default()
        };
        // Exceed a frame's byte allowance, independently of machine speed.
        let candidate = tile(1, BROWSER_UPLOAD_FRAME_BYTES + BROWSER_UPLOAD_CHUNK_BYTES);
        let total = upload_bytes(&candidate.tile.mesh);
        uploads.reconcile(std::slice::from_ref(&candidate));
        for _ in 0..128 {
            uploads.advance(&context, &gpu_queue, &metrics);
            if uploads.stats().uploaded_bytes > 0 {
                break;
            }
        }
        let remaining = uploads.entries[&candidate.id].remaining_bytes();
        assert!(remaining > 0 && remaining < total);
        assert!(!candidate.tile.is_publishable());

        uploads.reconcile(&[]);
        let before_hidden_frames = capture.0.lock().unwrap().len();
        Arc::get_mut(
            uploads
                .entries
                .get_mut(&candidate.id)
                .unwrap()
                .trace
                .as_mut()
                .unwrap(),
        )
        .unwrap()
        .advance_clock(Duration::from_millis(2_600));
        for _ in 0..10 {
            uploads.advance(&context, &gpu_queue, &metrics);
            assert_eq!(uploads.stats().uploaded_bytes, 0);
            assert_eq!(uploads.stats().pending_tiles, 0);
        }
        assert_eq!(capture.0.lock().unwrap().len(), before_hidden_frames);
        assert_eq!(uploads.entries[&candidate.id].remaining_bytes(), remaining);
        assert!(!candidate.tile.is_publishable());

        // Wrapped copies must resume the same upload, not enqueue duplicates.
        uploads.reconcile(&[candidate.clone(), candidate.clone()]);
        assert_eq!(uploads.stats().pending_tiles, 1);
        assert_eq!(uploads.entries[&candidate.id].remaining_bytes(), remaining);
        for _ in 0..128 {
            uploads.advance(&context, &gpu_queue, &metrics);
            if candidate.tile.is_publishable() {
                break;
            }
        }
        assert!(candidate.tile.is_publishable());
        assert!(uploads.entries.is_empty());
        let events = capture.0.lock().unwrap();
        assert!(
            events
                .iter()
                .all(|event| event.upload_id == events[0].upload_id)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event != MapUploadPhase::Progress)
                .map(|event| event.event)
                .collect::<Vec<_>>(),
            [
                MapUploadPhase::Queued,
                MapUploadPhase::FirstWork,
                MapUploadPhase::Hidden,
                MapUploadPhase::Visible,
                MapUploadPhase::Published,
            ]
        );
        assert_eq!(events.iter().map(|event| event.bytes).sum::<usize>(), total);
        let hidden = events
            .iter()
            .find(|event| event.event == MapUploadPhase::Hidden)
            .unwrap();
        let returned = events
            .iter()
            .find(|event| event.event == MapUploadPhase::Visible)
            .unwrap();
        let published = events.last().unwrap();
        assert!(returned.elapsed_ms - hidden.elapsed_ms >= 2_600.0);
        assert!(published.elapsed_ms >= returned.elapsed_ms);
    }

    #[test]
    fn slow_uploads_stop_before_the_byte_ceiling() {
        let mut budget = UploadBudget::default();
        let mut elapsed = Duration::ZERO;
        while budget.can_advance(elapsed) {
            budget.record(budget.chunk_bytes());
            elapsed += Duration::from_millis(1);
        }
        assert_eq!(budget.uploaded_bytes, 512 * 1024);
        assert!(!budget.can_advance(elapsed));
        assert!(budget.can_advance(Duration::ZERO));
    }

    #[test]
    fn upload_cursor_never_crosses_a_buffer_or_chunk_boundary() {
        let vertex_bytes = BROWSER_UPLOAD_CHUNK_BYTES + 16;
        let mut cursor = UploadCursor::new(vertex_bytes, 32);

        assert_eq!(
            cursor.take(BROWSER_UPLOAD_CHUNK_BYTES),
            Some(UploadChunk {
                buffer: TileBuffer::Vertices,
                offset: 0,
                length: BROWSER_UPLOAD_CHUNK_BYTES,
            })
        );
        assert_eq!(
            cursor.take(BROWSER_UPLOAD_CHUNK_BYTES),
            Some(UploadChunk {
                buffer: TileBuffer::Vertices,
                offset: BROWSER_UPLOAD_CHUNK_BYTES,
                length: 16,
            })
        );
        assert_eq!(
            cursor.take(BROWSER_UPLOAD_CHUNK_BYTES),
            Some(UploadChunk {
                buffer: TileBuffer::Indices,
                offset: 0,
                length: 32,
            })
        );
        assert_eq!(cursor.take(BROWSER_UPLOAD_CHUNK_BYTES), None);
        assert_eq!(cursor.remaining_bytes(), 0);
    }

    #[test]
    fn newly_queued_tile_is_not_publishable() {
        let candidate = tile(1, BROWSER_UPLOAD_CHUNK_BYTES + 64);
        let mut queue = BrowserUploadQueue::default();

        queue.reconcile(std::slice::from_ref(&candidate));

        assert!(!candidate.tile.is_publishable());
        assert_eq!(queue.entries.len(), 1);
        assert!(matches!(
            &queue.entries[&candidate.id].state,
            TileUploadState::AllocateVertices
        ));
        assert!(queue.stats().queued_bytes > BROWSER_UPLOAD_CHUNK_BYTES);
    }

    #[test]
    fn hidden_upload_retention_is_bounded() {
        let mut queue = BrowserUploadQueue::default();
        let visible = (0..(RETAINED_UPLOAD_LIMIT as u32 + 4))
            .map(|id| tile(id, 64))
            .collect::<Vec<_>>();
        queue.reconcile(&visible);

        let stats = queue.reconcile(&[]);

        assert_eq!(queue.entries.len(), RETAINED_UPLOAD_LIMIT);
        assert_eq!(stats.pending_tiles, 0);
        assert_eq!(stats.queued_bytes, 0);
    }
}
