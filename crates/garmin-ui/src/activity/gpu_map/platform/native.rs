use std::sync::{Arc, Condvar, Mutex, mpsc};

use super::super::{
    LabelResult, LabelTask, RouteOutcome, RouteResult, RouteTask, UploadContext, UploadStats,
    VisibleTile, build_label_result, build_route,
};
use crate::activity::map_runtime::Backend;

pub(in crate::activity) struct Executor {
    route_worker: RouteWorker,
    label_worker: LabelWorker,
}

impl Executor {
    pub(in crate::activity) fn new(context: Arc<UploadContext>) -> Self {
        Self {
            route_worker: RouteWorker::spawn(context),
            label_worker: LabelWorker::spawn(),
        }
    }

    pub(in crate::activity) fn upload_visible(_visible: &[VisibleTile]) -> UploadStats {
        UploadStats::default()
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

struct LatestWorker<T, R> {
    state: Arc<(Mutex<LatestWorkerState<T>>, Condvar)>,
    results: mpsc::Receiver<R>,
}

enum LatestWorkerState<T> {
    Running(Option<T>),
    Shutdown,
}

enum WorkerPoll<T> {
    Pending,
    Task(T),
    Shutdown,
}

impl<T> LatestWorkerState<T> {
    fn replace(&mut self, task: T) -> bool {
        match self {
            Self::Running(pending) => pending.replace(task).is_some(),
            Self::Shutdown => true,
        }
    }

    fn poll(&mut self) -> WorkerPoll<T> {
        match self {
            Self::Running(pending) => pending.take().map_or(WorkerPoll::Pending, WorkerPoll::Task),
            Self::Shutdown => WorkerPoll::Shutdown,
        }
    }

    fn shutdown(&mut self) {
        *self = Self::Shutdown;
    }
}

impl<T> Default for LatestWorkerState<T> {
    fn default() -> Self {
        Self::Running(None)
    }
}

impl<T, R> LatestWorker<T, R>
where
    T: Send + 'static,
    R: Send + 'static,
{
    fn spawn(name: &'static str, build: impl Fn(T) -> (R, egui::Context) + Send + 'static) -> Self {
        let state = Arc::new((Mutex::new(LatestWorkerState::default()), Condvar::new()));
        let worker_state = Arc::clone(&state);
        let (sender, results) = mpsc::channel();
        let _worker = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                loop {
                    let task = {
                        let (lock, ready) = &*worker_state;
                        let mut state = lock
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        loop {
                            match state.poll() {
                                WorkerPoll::Task(task) => break task,
                                WorkerPoll::Shutdown => return,
                                WorkerPoll::Pending => {
                                    state = ready
                                        .wait(state)
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                }
                            }
                        }
                    };
                    let (result, repaint) = build(task);
                    if sender.send(result).is_err() {
                        return;
                    }
                    repaint.request_repaint();
                }
            })
            .expect("activity map worker must start");
        Self { state, results }
    }

    fn submit(&self, task: T) -> bool {
        let (lock, ready) = &*self.state;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let discarded = state.replace(task);
        ready.notify_one();
        discarded
    }

    fn try_recv(&self) -> Option<R> {
        self.results.try_recv().ok()
    }
}

impl<T, R> Drop for LatestWorker<T, R> {
    fn drop(&mut self) {
        let (lock, ready) = &*self.state;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.shutdown();
        ready.notify_one();
    }
}

struct LabelWorker {
    inner: LatestWorker<LabelTask, LabelResult>,
}

impl LabelWorker {
    fn spawn() -> Self {
        Self {
            inner: LatestWorker::spawn("garmin-toolkit-map-labels", |task: LabelTask| {
                let repaint = task.context.clone();
                (build_label_result(&task), repaint)
            }),
        }
    }

    fn submit(&self, task: LabelTask) -> bool {
        self.inner.submit(task)
    }

    fn try_recv(&self) -> Option<LabelResult> {
        self.inner.try_recv()
    }
}

struct RouteWorker {
    inner: LatestWorker<RouteTask, RouteResult>,
}

impl RouteWorker {
    fn spawn(context: Arc<UploadContext>) -> Self {
        Self {
            inner: LatestWorker::spawn("garmin-toolkit-map-route", move |task: RouteTask| {
                let route = build_route(&context, &task.samples, task.sample_offset);
                let repaint = task.context;
                (
                    RouteResult {
                        key: task.key,
                        outcome: RouteOutcome::Ready(route),
                    },
                    repaint,
                )
            }),
        }
    }

    fn submit(&self, task: RouteTask) {
        let _discarded = self.inner.submit(task);
    }

    fn try_recv(&self) -> Option<RouteResult> {
        self.inner.try_recv()
    }
}

#[cfg(test)]
mod tests {
    use super::{LatestWorkerState, WorkerPoll};

    #[test]
    fn latest_worker_state_replaces_takes_and_discards_by_transition() {
        let mut state = LatestWorkerState::default();
        assert!(matches!(state.poll(), WorkerPoll::Pending));
        assert!(!state.replace(1));
        assert!(state.replace(2));
        assert!(matches!(state.poll(), WorkerPoll::Task(2)));
        assert!(matches!(state.poll(), WorkerPoll::Pending));

        assert!(!state.replace(3));
        state.shutdown();
        assert!(matches!(state.poll(), WorkerPoll::Shutdown));
        assert!(state.replace(4));
        assert!(matches!(state.poll(), WorkerPoll::Shutdown));
    }
}
