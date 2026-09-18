use std::sync::{Arc, Mutex, MutexGuard, mpsc};

use super::super::{
    LabelResult, LabelTask, RouteOutcome, RouteResult, RouteTask, UploadContext, UploadStats,
    VisibleTile, labels::build_label_result, route::build_route,
};
use crate::activity::{map_runtime::Backend, native_latest::LatestWorker};

pub(in crate::activity) struct Executor {
    uploads: UploadController,
    route_worker: RouteWorker,
    label_worker: LabelWorker,
}

pub(in crate::activity) struct ResourceCreationGate(Mutex<()>);

impl ResourceCreationGate {
    pub(in crate::activity) const fn new() -> Self {
        Self(Mutex::new(()))
    }

    pub(in crate::activity) fn enter(&self) -> MutexGuard<'_, ()> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Clone, Default)]
pub(in crate::activity) struct UploadController;

#[expect(
    clippy::unused_self,
    reason = "shares the instance API of the browser upload queue"
)]
impl UploadController {
    pub(super) fn prepare(
        &self,
        _queue: &wgpu::Queue,
        _metrics: &crate::activity::map_runtime::MapMetrics,
    ) {
    }

    fn stats(&self) -> UploadStats {
        UploadStats::default()
    }
}

impl Executor {
    pub(in crate::activity) fn new(context: Arc<UploadContext>) -> Self {
        Self {
            uploads: UploadController,
            route_worker: RouteWorker::spawn(context),
            label_worker: LabelWorker::spawn(),
        }
    }

    pub(in crate::activity) fn upload_controller(&self) -> UploadController {
        self.uploads.clone()
    }

    pub(in crate::activity) fn upload_visible(&self, _visible: &[VisibleTile]) -> UploadStats {
        self.uploads.stats()
    }

    pub(in crate::activity) fn poll_label(&self) -> Option<LabelResult> {
        self.label_worker.try_recv()
    }

    pub(in crate::activity) fn schedule_label(
        &self,
        task: LabelTask,
        _backend: &dyn Backend,
    ) -> (Option<LabelResult>, bool) {
        let discarded = self.label_worker.submit(task);
        (None, discarded)
    }

    pub(in crate::activity) fn poll_route(&self) -> Option<RouteResult> {
        self.route_worker.try_recv()
    }

    pub(in crate::activity) fn schedule_route(&self, task: RouteTask, _backend: &dyn Backend) {
        self.route_worker.submit(task);
    }
}

struct LabelWorker {
    inner: LatestWorker<LabelTask>,
    results: mpsc::Receiver<LabelResult>,
}

impl LabelWorker {
    fn spawn() -> Self {
        let (sender, results) = mpsc::channel();
        Self {
            inner: LatestWorker::spawn("garmin-toolkit-map-labels", move |task: LabelTask| {
                let repaint = task.context.clone();
                let _ = sender.send(build_label_result(&task));
                repaint.request_repaint();
            }),
            results,
        }
    }

    fn submit(&self, task: LabelTask) -> bool {
        self.inner.submit(task)
    }

    fn try_recv(&self) -> Option<LabelResult> {
        self.results.try_recv().ok()
    }
}

struct RouteWorker {
    inner: LatestWorker<RouteTask>,
    results: mpsc::Receiver<RouteResult>,
}

impl RouteWorker {
    fn spawn(context: Arc<UploadContext>) -> Self {
        let (sender, results) = mpsc::channel();
        Self {
            inner: LatestWorker::spawn("garmin-toolkit-map-route", move |task: RouteTask| {
                let route = build_route(&context, &task.samples, task.sample_offset);
                let repaint = task.context;
                let _ = sender.send(RouteResult {
                    key: task.key,
                    outcome: RouteOutcome::Ready(route),
                });
                repaint.request_repaint();
            }),
            results,
        }
    }

    fn submit(&self, task: RouteTask) {
        let _ = self.inner.submit(task);
    }

    fn try_recv(&self) -> Option<RouteResult> {
        self.results.try_recv().ok()
    }
}
