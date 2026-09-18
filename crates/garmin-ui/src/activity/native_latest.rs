use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU64, Ordering},
};

pub(super) struct LatestWorker<T> {
    state: Arc<(Mutex<WorkerState<T>>, Condvar)>,
    discarded: AtomicU64,
}

enum WorkerState<T> {
    Running(WorkerQueue<T>),
    Shutdown,
}

struct WorkerQueue<T> {
    pending: Option<(u64, T)>,
    submitted: u64,
    completed: u64,
}

impl<T> WorkerState<T> {
    fn submit(&mut self, task: T) -> bool {
        match self {
            Self::Running(queue) => {
                queue.submitted = queue.submitted.wrapping_add(1);
                queue.pending.replace((queue.submitted, task)).is_some()
            }
            Self::Shutdown => true,
        }
    }
}

impl<T> Default for WorkerState<T> {
    fn default() -> Self {
        Self::Running(WorkerQueue {
            pending: None,
            submitted: 0,
            completed: 0,
        })
    }
}

impl<T: Send + 'static> LatestWorker<T> {
    pub(super) fn spawn(name: &'static str, mut process: impl FnMut(T) + Send + 'static) -> Self {
        let state = Arc::new((Mutex::new(WorkerState::default()), Condvar::new()));
        let worker_state = Arc::clone(&state);
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                loop {
                    let (revision, task) = {
                        let (lock, ready) = &*worker_state;
                        let mut state = lock
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        loop {
                            match &mut *state {
                                WorkerState::Running(queue) => {
                                    if let Some(task) = queue.pending.take() {
                                        break task;
                                    }
                                }
                                WorkerState::Shutdown => return,
                            }
                            state = ready
                                .wait(state)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                    };
                    process(task);
                    let (lock, ready) = &*worker_state;
                    let mut state = lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let WorkerState::Running(queue) = &mut *state {
                        queue.completed = revision;
                        ready.notify_all();
                    }
                }
            })
            .expect("activity map worker must start");
        Self {
            state,
            discarded: AtomicU64::new(0),
        }
    }

    pub(super) fn submit(&self, task: T) -> bool {
        let (lock, ready) = &*self.state;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let discarded = state.submit(task);
        if discarded {
            self.discarded.fetch_add(1, Ordering::Relaxed);
        }
        ready.notify_one();
        discarded
    }

    pub(super) fn discarded(&self) -> u64 {
        self.discarded.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn synchronize(&self) {
        let (lock, ready) = &*self.state;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target = match &*state {
            WorkerState::Running(queue) => queue.submitted,
            WorkerState::Shutdown => return,
        };
        while matches!(&*state, WorkerState::Running(queue) if queue.completed < target) {
            state = ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl<T> Drop for LatestWorker<T> {
    fn drop(&mut self) {
        let (lock, ready) = &*self.state;
        let mut state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = WorkerState::Shutdown;
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn worker_replaces_pending_work_and_counts_the_discard() {
        let (started, wait_for_start) = mpsc::channel();
        let (release, wait_for_release) = mpsc::channel();
        let (executed, results) = mpsc::channel();
        let worker = LatestWorker::spawn("latest-worker-test", move |task| {
            if task == 1 {
                started.send(()).unwrap();
                wait_for_release.recv().unwrap();
            }
            executed.send(task).unwrap();
        });

        assert!(!worker.submit(1));
        wait_for_start.recv().unwrap();
        assert!(!worker.submit(2));
        assert!(worker.submit(3));
        assert_eq!(worker.discarded(), 1);
        release.send(()).unwrap();
        worker.synchronize();
        assert_eq!(results.try_iter().collect::<Vec<_>>(), vec![1, 3]);
    }
}
