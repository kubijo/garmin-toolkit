//! Backend-owned opt-in refreshes. Scheduled work never imports activities.

use crate::{Application, Error, UserContext, connect_import::cache::Control};
use garmin_model::{
    garmin_connect::{
        ActivityResponse, LoadProgress,
        sync::{Failure, Interval, Outcome, Snapshot},
    },
    value::Timestamp,
};
use garmin_progress::CancellationToken;
use garmin_service_api::ConnectActivityResult;
use std::sync::Arc;

#[cfg(test)]
mod tests;

pub(crate) struct Run {
    id: garmin_model::garmin_connect::sync::JobId,
    cancel: CancellationToken,
    progress: LoadProgress,
}

struct Registration<'a> {
    application: &'a Application,
    user: garmin_model::identity::UserId,
    id: garmin_model::garmin_connect::sync::JobId,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        let mut runs = self
            .application
            .connect_sync_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runs.get(&self.user).is_some_and(|run| run.id == self.id) {
            runs.remove(&self.user);
        }
    }
}

fn now() -> Timestamp {
    Timestamp::from_jiff(jiff::Timestamp::now())
}

impl Application {
    /// Reads a published page without networking or waiting for a refresh job.
    /// # Errors
    /// Returns profile, filter or persistence failures.
    pub async fn read_connect_cache(
        &self,
        user: UserContext,
        start: u32,
        filters: garmin_model::garmin_connect::ActivityFilters,
    ) -> Result<Option<ActivityResponse>, Error> {
        self.garmin_connect_configured(user).await?;
        let Some(key) = self.storage.garmin_connect_key(user.user_id()).await? else {
            return Ok(None);
        };
        let Some(snapshot) = self
            .storage
            .connect_cache_snapshot(
                user.user_id(),
                &crate::connect_import::cache::credential_hash(&key),
            )
            .await?
        else {
            return Ok(None);
        };
        let ConnectActivityResult::State(page) =
            Self::connect_page(snapshot, start, filters, None)?
        else {
            unreachable!("cache page")
        };
        Ok(Some(page))
    }
    pub(crate) fn connect_lock(&self, user: UserContext) -> Arc<tokio::sync::Mutex<()>> {
        self.connect_cache_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(user.user_id())
            .or_default()
            .clone()
    }

    /// Reads persisted scheduling state plus the current backend job's phase.
    /// # Errors
    /// Returns profile or storage failures.
    pub async fn connect_sync_status(&self, user: UserContext) -> Result<Snapshot, Error> {
        self.garmin_connect_configured(user).await?;
        let state = self.storage.connect_sync_state(user.user_id()).await?;
        let cache_generation =
            if let Some(key) = self.storage.garmin_connect_key(user.user_id()).await? {
                self.storage
                    .connect_cache_account(
                        user.user_id(),
                        &crate::connect_import::cache::credential_hash(&key),
                    )
                    .await?
                    .map(|account| account.generation)
            } else {
                None
            };
        let active = self
            .connect_sync_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&user.user_id())
            .map(|run| (run.id, run.progress.clone()));
        let (job, progress) = active.map_or((None, None), |(id, phase)| (Some(id), Some(phase)));
        Ok(Snapshot {
            job,
            state,
            progress,
            cache_generation,
        })
    }

    /// Replaces the profile's opt-in elapsed schedule and stops its current background refresh.
    /// # Errors
    /// Requires a configured profile when enabling and returns persistence failures.
    pub async fn configure_connect_sync(
        &self,
        user: UserContext,
        interval: Option<Interval>,
    ) -> Result<Snapshot, Error> {
        if !self.garmin_connect_configured(user).await? && interval.is_some() {
            return Err(Error::GarminConnectKeyMissing);
        }
        let refresh_guard = self.interrupt_connect_sync(user).await;
        let state_guard = self.connect_sync_state_lock.lock().await;
        let mut state = self.storage.connect_sync_state(user.user_id()).await?;
        state.configure(interval, now());
        self.storage
            .save_connect_sync_state(user.user_id(), &state)
            .await?;
        drop(state_guard);
        drop(refresh_guard);
        Ok(Snapshot {
            state,
            ..Snapshot::default()
        })
    }

    /// Signals the current background job without tying its lifetime to a browser.
    pub fn cancel_connect_sync(&self, user: UserContext) {
        if let Some(run) = self
            .connect_sync_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&user.user_id())
        {
            run.cancel.cancel();
        }
    }

    pub(crate) async fn interrupt_connect_sync(
        &self,
        user: UserContext,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let acquire = self.connect_lock(user).lock_owned();
        tokio::pin!(acquire);
        loop {
            // Registration follows mutex acquisition. Keep signalling while queued so an
            // attempt that has acquired the mutex but not yet registered cannot be missed.
            self.cancel_connect_sync(user);
            tokio::select! {
                guard = &mut acquire => return guard,
                () = futures_timer::Delay::new(std::time::Duration::from_millis(50)) => {},
            }
        }
    }

    /// Cancels or defers this profile's scheduled refresh, including a just-finished attempt.
    /// # Errors
    /// Returns profile or persistence failures.
    pub async fn cancel_scheduled_connect_sync(
        &self,
        user: UserContext,
        job: garmin_model::garmin_connect::sync::JobId,
    ) -> Result<Snapshot, Error> {
        self.garmin_connect_configured(user).await?;
        {
            let _state_guard = self.connect_sync_state_lock.lock().await;
            let mut state = self.storage.connect_sync_state(user.user_id()).await?;
            if state.last_job == Some(job) {
                if let Some(run) = self
                    .connect_sync_runs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&user.user_id())
                    && run.id == job
                {
                    run.cancel.cancel();
                }
                state.finish(Outcome::Cancelled, now(), None);
                self.storage
                    .save_connect_sync_state(user.user_id(), &state)
                    .await?;
            }
        }
        self.connect_sync_status(user).await
    }

    pub(crate) async fn record_manual_connect_cancellation(
        &self,
        user: UserContext,
    ) -> Result<(), Error> {
        self.finish_connect_sync(user, Outcome::Cancelled, None, None)
            .await
    }

    pub(crate) async fn record_manual_connect_refresh(
        &self,
        user: UserContext,
        result: &Result<garmin_model::garmin_connect::CacheStatus, Error>,
    ) -> Result<(), Error> {
        let (outcome, retry) = match result {
            Ok(cache) => (
                Outcome::Complete {
                    unavailable: cache.unavailable,
                },
                None,
            ),
            Err(error) => classify(error),
        };
        self.finish_connect_sync(user, outcome, retry, None).await
    }

    async fn finish_connect_sync(
        &self,
        user: UserContext,
        outcome: Outcome,
        retry: Option<std::time::Duration>,
        job: Option<garmin_model::garmin_connect::sync::JobId>,
    ) -> Result<(), Error> {
        let _state = self.connect_sync_state_lock.lock().await;
        let mut state = self.storage.connect_sync_state(user.user_id()).await?;
        if job.is_some() && state.last_job != job {
            return Ok(());
        }
        if job.is_some()
            && matches!(state.outcome, Outcome::Cancelled)
            && matches!(outcome, Outcome::Complete { .. })
        {
            // A cancellation persisted after the final download still owns the next due time.
            return Ok(());
        }
        state.last_job = job;
        state.finish(outcome, now(), retry);
        self.storage
            .save_connect_sync_state(user.user_id(), &state)
            .await?;
        Ok(())
    }

    pub(crate) async fn run_connect_sync(
        &self,
        user: UserContext,
        expected_due: Option<Timestamp>,
        stop: &(impl Fn() -> bool + Send + Sync),
    ) -> Result<(), Error> {
        let lock = self.connect_lock(user);
        let _guard = Self::wait_for_scheduled_connect_lock(lock, stop).await?;
        let state_guard = self.connect_sync_state_lock.lock().await;
        let mut state = self.storage.connect_sync_state(user.user_id()).await?;
        if stop() {
            return Err(Error::GarminConnectRefreshCancelled);
        }
        if !eligible(&state, expected_due, now())? {
            return Ok(());
        }
        let Some(key) = self.storage.garmin_connect_key(user.user_id()).await? else {
            drop(state_guard);
            self.finish_connect_sync(
                user,
                Outcome::Failed {
                    kind: Failure::Credentials,
                    message: Error::GarminConnectKeyMissing.to_string(),
                },
                None,
                None,
            )
            .await?;
            return Ok(());
        };
        state.last_attempt = Some(now());
        let id = garmin_model::garmin_connect::sync::JobId::new_v4();
        state.last_job = Some(id);
        state.outcome = Outcome::Running;
        self.storage
            .save_connect_sync_state(user.user_id(), &state)
            .await?;
        let cancel = CancellationToken::default();
        self.connect_sync_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                user.user_id(),
                Run {
                    id,
                    cancel: cancel.clone(),
                    progress: LoadProgress::Waiting,
                },
            );
        let _registration = Registration {
            application: self,
            user: user.user_id(),
            id,
        };
        drop(state_guard);
        let report = |progress| self.report_connect_sync(user, progress);
        let cancelled = || cancel.is_cancelled() || stop();
        let control = Control::new(&report, &cancelled);
        let mut result = self
            .sync_connect_refresh(
                user,
                &key,
                crate::connect_import::cache::Refresh::Recent,
                &control,
            )
            .await;
        if matches!(
            &result,
            Err(Error::GarminConnect(garmin_connect::Error::Rejected {
                status: 401,
                ..
            }))
        ) && !cancelled()
        {
            result = self
                .sync_connect_refresh(
                    user,
                    &key,
                    crate::connect_import::cache::Refresh::Recent,
                    &control,
                )
                .await;
        }
        let (mut outcome, retry) = match result {
            Ok(cache) => (
                Outcome::Complete {
                    unavailable: cache.unavailable,
                },
                None,
            ),
            Err(error) => classify(&error),
        };
        if matches!(outcome, Outcome::Cancelled) && stop() && !cancel.is_cancelled() {
            outcome = Outcome::Interrupted;
        }
        self.finish_connect_sync(user, outcome, retry, Some(id))
            .await
    }

    async fn wait_for_scheduled_connect_lock(
        lock: Arc<tokio::sync::Mutex<()>>,
        stop: &(impl Fn() -> bool + Send + Sync),
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, Error> {
        let acquire = lock.lock_owned();
        tokio::pin!(acquire);
        loop {
            if stop() {
                return Err(Error::GarminConnectRefreshCancelled);
            }
            tokio::select! {
                guard = &mut acquire => return Ok(guard),
                () = futures_timer::Delay::new(std::time::Duration::from_millis(50)) => {},
            }
        }
    }

    fn report_connect_sync(&self, user: UserContext, progress: LoadProgress) {
        if let Some(run) = self
            .connect_sync_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&user.user_id())
        {
            run.progress = progress;
        }
    }
}

fn eligible(
    state: &garmin_model::garmin_connect::sync::State,
    expected: Option<Timestamp>,
    now: Timestamp,
) -> Result<bool, Error> {
    if expected.is_some_and(|due| state.next_attempt != Some(due)) {
        return Ok(false);
    }
    if state.due(now) {
        return Ok(true);
    }
    if expected.is_some()
        && state.interval.is_some()
        && state.next_attempt.is_some_and(|due| due > now)
    {
        // A clock change while this job waited for manual work must not acknowledge it as done.
        return Err(Error::BackgroundJobDeferred);
    }
    Ok(false)
}

fn classify(error: &Error) -> (Outcome, Option<std::time::Duration>) {
    use garmin_connect::Error as Remote;
    if matches!(
        error,
        Error::GarminConnectRefreshCancelled | Error::GarminConnect(Remote::Cancelled)
    ) {
        return (Outcome::Cancelled, None);
    }
    let (kind, retry) = match error {
        Error::GarminConnectKeyMissing
        | Error::GarminConnect(
            Remote::Rejected {
                status: 401 | 403, ..
            }
            | Remote::InvalidToken,
        ) => (Failure::Credentials, None),
        Error::GarminConnect(Remote::RateLimited { retry_after, .. }) => {
            (Failure::RateLimit, *retry_after)
        }
        Error::GarminConnect(Remote::Rejected { status: 429, .. }) => (Failure::RateLimit, None),
        Error::GarminConnect(Remote::Network | Remote::Timeout) => (Failure::Network, None),
        _ => (Failure::Other, None),
    };
    (
        Outcome::Failed {
            kind,
            message: error.to_string(),
        },
        retry,
    )
}
