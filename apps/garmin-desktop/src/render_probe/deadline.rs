//! Independent process deadline, including blocked initialization, frames, and teardown.

use std::{io, sync::mpsc, thread, time::Duration};

pub(super) struct Deadline {
    cancel: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Deadline {
    pub fn start(duration: Duration) -> io::Result<Self> {
        Self::spawn(duration, || std::process::exit(124))
    }

    fn spawn(duration: Duration, expired: impl FnOnce() + Send + 'static) -> io::Result<Self> {
        let (cancel, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("probe-deadline".into())
            .spawn(move || {
                if receiver.recv_timeout(duration) == Err(mpsc::RecvTimeoutError::Timeout) {
                    expired();
                }
            })?;
        Ok(Self {
            cancel,
            worker: Some(worker),
        })
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_does_not_depend_on_the_frame_thread() {
        let (sender, receiver) = mpsc::channel();
        let _deadline = Deadline::spawn(Duration::ZERO, move || sender.send(()).unwrap()).unwrap();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn normal_completion_cancels_and_joins_the_watchdog() {
        let (sender, receiver) = mpsc::channel();
        let deadline = Deadline::spawn(Duration::from_secs(3600), move || {
            sender.send(()).unwrap();
        })
        .unwrap();
        drop(deadline);
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }
}
