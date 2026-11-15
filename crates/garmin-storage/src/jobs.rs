//! Durable core job payloads and the library-owned execution queue.

use crate::{Error, Storage};
use apalis_core::task::builder::TaskBuilder;
use apalis_sqlite::{Config, SqliteStorage};
use garmin_model::{identity::UserId, value::Timestamp};
use serde::{Deserialize, Serialize};

/// Version the queue when changing the persisted job protocol.
const QUEUE: &str = "garmin-core-v1";

// Exclusive root ownership invalidates every previous lease, including future-dated heartbeats
// copied from a backup or left by a backward clock change. The pinned helper accepts a signed age.
const EXCLUSIVE_RECOVERY_AGE: i64 = i64::MIN;

/// Application jobs contain identities and eligibility, never credentials or downloaded data.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Job {
    ConnectSync { user: UserId, due: Timestamp },
}

#[cfg(test)]
mod tests;

impl Storage {
    /// Reclaims this queue's abandoned deliveries before starting an exclusive deployment worker.
    /// The caller must hold the deployment root lock and have dropped the previous worker.
    /// # Errors
    /// Returns database failures.
    pub async fn recover_jobs(&self) -> Result<u64, Error> {
        Ok(apalis_sqlite::queries::reenqueue_orphaned(
            &self.pool,
            EXCLUSIVE_RECOVERY_AGE,
            QUEUE,
            &apalis_core::backend::WorkerFilter::None,
        )
        .await?)
    }

    /// Returns the shared core's durable queue without exposing its SQLite connection.
    #[must_use]
    pub fn jobs(&self) -> SqliteStorage<Job> {
        SqliteStorage::new(&self.pool).with_config(Config::default().queue(QUEUE).batch_size(1))
    }
}

pub(crate) async fn enqueue_connect_sync(
    connection: &mut sqlx::SqliteConnection,
    user: UserId,
    state: &garmin_model::garmin_connect::sync::State,
) -> Result<(), Error> {
    let Some(due) = state.next_attempt.filter(|_| state.interval.is_some()) else {
        return Ok(());
    };
    if matches!(
        state.outcome,
        garmin_model::garmin_connect::sync::Outcome::Running
    ) {
        return Ok(());
    }
    let job = serde_json::to_vec(&Job::ConnectSync { user, due }).map_err(invalid)?;
    // Apalis stores Unix seconds. Round upward so a subsecond due time is never consumed early.
    let instant = due.as_jiff();
    let seconds = instant
        .as_second()
        .saturating_add(i64::from(instant.subsec_nanosecond() != 0));
    let task = TaskBuilder::new(job)
        .run_at_timestamp(u64::try_from(seconds.max(0)).map_err(invalid)?)
        // Infrastructure failure must not silently exhaust an opted-in recurring schedule.
        // Network/service failures instead persist their classified next attempt in State.
        .max_attempts(usize::try_from(u32::MAX).unwrap_or(usize::MAX))
        .idempotency_key(format!("connect:{user}:{due}"))
        .build();
    apalis_sqlite::queries::push_tasks(connection, QUEUE, &[task])
        .await
        .map_err(invalid)
}

fn invalid(error: impl std::fmt::Display) -> Error {
    Error::InvalidData {
        field: "background jobs",
        reason: error.to_string(),
    }
}
