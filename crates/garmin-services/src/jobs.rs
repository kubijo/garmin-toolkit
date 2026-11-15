//! The shared application's durable job engine. Hosts only retain its lifetime guard.

use crate::{Application, UserContext, deployment::Deployment};
use apalis::prelude::*;
use garmin_progress::CancellationToken;
use garmin_storage::jobs::Job;
use std::{
    sync::{Arc, Weak, atomic::Ordering},
    time::Duration,
};

#[cfg(test)]
mod tests;

/// Owns the core worker's continuously driven runtime, including in an idle desktop host.
pub struct Runtime {
    stop: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
    deployment: Weak<Deployment>,
}

impl Deployment {
    /// Starts one library-backed job engine for this deployment, shared by all modalities.
    /// # Errors
    /// Fails if an engine already owns the deployment or its runtime cannot start.
    pub fn start_jobs(self: &Arc<Self>) -> std::io::Result<Runtime> {
        Runtime::start(self)
    }
}

impl Runtime {
    fn start(deployment: &Arc<Deployment>) -> std::io::Result<Self> {
        if deployment.jobs_started.swap(true, Ordering::AcqRel) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Core jobs already running",
            ));
        }
        let result = Self::spawn(deployment);
        if result.is_err() {
            deployment.jobs_started.store(false, Ordering::Release);
        }
        result
    }

    fn spawn(deployment: &Arc<Deployment>) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let stop = CancellationToken::default();
        let thread_stop = stop.clone();
        let weak = Arc::downgrade(deployment);
        let thread_weak = weak.clone();
        let thread = std::thread::Builder::new()
            .name("garmin-core-jobs".into())
            .spawn(move || runtime.block_on(run(thread_weak, thread_stop)))?;
        Ok(Self {
            stop,
            thread: Some(thread),
            deployment: weak,
        })
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(deployment) = self.deployment.upgrade() {
            deployment.jobs_changed.notify_waiters();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(deployment) = self.deployment.upgrade() {
            deployment.jobs_started.store(false, Ordering::Release);
        }
    }
}

async fn run(deployment: Weak<Deployment>, stopped: CancellationToken) {
    while let Some(owner) = deployment.upgrade() {
        let changed = owner.jobs_changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if stopped.is_cancelled() || owner.jobs_shutdown.is_cancelled() {
            break;
        }
        if owner.jobs_pauses.load(Ordering::Acquire) != 0 {
            changed.await;
        } else {
            let stop = owner
                .jobs_stop
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Err(error) = run_generation(Arc::clone(&owner), stopped.clone(), stop).await {
                tracing::warn!(%error, "Core job engine stopped; reopening its durable queue");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn run_generation(
    deployment: Arc<Deployment>,
    stopped: CancellationToken,
    replaced: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let epoch = deployment.epoch();
    let application = Arc::new(deployment.application_owned(epoch).await?);
    application.prepare_jobs().await?;
    // The root lock and returned previous generation guarantee no live worker still owns these claims.
    application.storage.recover_jobs().await?;
    let backend = application
        .storage
        .jobs()
        .poll_with_interval(Duration::from_secs(1));
    let service = Arc::clone(&application);
    let owner = Arc::clone(&deployment);
    let handler_stop = stopped.clone();
    let handler_replaced = replaced.clone();
    let worker = WorkerBuilder::new(format!("garmin-core:{epoch}:{}", uuid::Uuid::new_v4()))
        .backend(backend)
        .concurrency(4)
        .catch_panic()
        .on_event(|_, event| {
            tracing::debug!(%event, "Core job worker lifecycle");
            if let Some(error) = event.as_error() {
                tracing::warn!(%error, "Core job delivery failed; retained by the durable queue");
            }
        })
        .build(move |job: Job| {
            let application = Arc::clone(&service);
            let owner = Arc::clone(&owner);
            let stopped = handler_stop.clone();
            let replaced = handler_replaced.clone();
            async move {
                application
                    .dispatch_job(job, &|| {
                        stopped.is_cancelled()
                            || replaced.is_cancelled()
                            || owner.jobs_shutdown.is_cancelled()
                    })
                    .await
            }
        });
    worker
        .run_with_ctx(move |worker| {
            let deployment = Arc::clone(&deployment);
            let stopped = stopped.clone();
            let replaced = replaced.clone();
            async move {
                loop {
                    let changed = deployment.jobs_changed.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if stopped.is_cancelled()
                        || replaced.is_cancelled()
                        || deployment.jobs_shutdown.is_cancelled()
                    {
                        tracing::debug!("Draining core job handlers before worker shutdown");
                        return drain(worker).await;
                    }
                    changed.await;
                }
            }
        })
        .await?;
    tracing::debug!("Core job worker drained and acknowledged");
    Ok(())
}

async fn drain(worker: WorkerContext) -> Result<(), WorkerError> {
    // Shutdown may arrive before run_with_ctx has polled the worker's startup branch.
    while worker.is_pending() {
        tokio::task::yield_now().await;
    }
    // Admission is already closed by the deployment stop token. Keep the worker fully driven:
    // both stop and pause can strand backend transactions that active handlers need to finish.
    while worker.has_pending_tasks() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    worker.stop()
}

impl Application {
    async fn prepare_jobs(&self) -> Result<(), crate::Error> {
        let _state = self.connect_sync_state_lock.lock().await;
        for user in self.profiles().await? {
            let mut state = self.storage.connect_sync_state(user.id()).await?;
            if matches!(
                state.outcome,
                garmin_model::garmin_connect::sync::Outcome::Running
            ) {
                state.finish(
                    garmin_model::garmin_connect::sync::Outcome::Interrupted,
                    garmin_model::value::Timestamp::from_jiff(jiff::Timestamp::now()),
                    None,
                );
            }
            self.storage
                .save_connect_sync_state(user.id(), &state)
                .await?;
        }
        Ok(())
    }

    async fn dispatch_job(
        &self,
        job: Job,
        stopped: &(impl Fn() -> bool + Send + Sync),
    ) -> Result<(), crate::Error> {
        if stopped() {
            return Err(crate::Error::BackgroundJobDeferred);
        }
        match job {
            Job::ConnectSync { user, due } => {
                self.run_connect_sync(UserContext::new(user), Some(due), stopped)
                    .await
            }
        }
    }
}
