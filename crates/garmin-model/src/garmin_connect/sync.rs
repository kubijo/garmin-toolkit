//! Elapsed-time cache refresh schedules; imports always remain explicit.

use crate::value::Timestamp;

define_id!(
    JobKind,
    JobId,
    "connect-sync-job",
    "Type marker for a scheduled Garmin refresh.",
    "Identity of one backend-owned scheduled refresh."
);

#[garmin_macros::portable(copy, default, hash)]
pub enum Unit {
    #[default]
    Hour,
    Day,
    Week,
}

/// A positive elapsed interval retaining the user's chosen unit.
#[garmin_macros::portable(copy, hash)]
pub struct Interval {
    count: std::num::NonZeroU32,
    unit: Unit,
}

impl Interval {
    #[must_use]
    pub const fn new(count: u32, unit: Unit) -> Option<Self> {
        match std::num::NonZeroU32::new(count) {
            Some(count) => Some(Self { count, unit }),
            None => None,
        }
    }

    #[must_use]
    pub const fn count(self) -> u32 {
        self.count.get()
    }
    #[must_use]
    pub const fn unit(self) -> Unit {
        self.unit
    }
    #[must_use]
    pub fn duration(self) -> std::time::Duration {
        let hours = match self.unit {
            Unit::Hour => 1,
            Unit::Day => 24,
            Unit::Week => 7 * 24,
        };
        std::time::Duration::from_hours(u64::from(self.count.get()) * hours)
    }
    #[must_use]
    pub fn after(self, timestamp: Timestamp) -> Timestamp {
        timestamp
            .as_jiff()
            .checked_add(self.duration())
            .map_or_else(
                |_| Timestamp::from_jiff(jiff::Timestamp::MAX),
                Timestamp::from_jiff,
            )
    }
}

#[garmin_macros::portable(eq)]
pub enum Failure {
    Credentials,
    RateLimit,
    Network,
    Other,
}

#[garmin_macros::portable(default, eq)]
pub enum Outcome {
    #[default]
    Never,
    Running,
    Complete {
        unavailable: usize,
    },
    Cancelled,
    Interrupted,
    Failed {
        kind: Failure,
        message: String,
    },
}

/// Persisted schedule and completion state. An interrupted Running state is retried once on startup.
#[garmin_macros::portable(default, eq)]
pub struct State {
    pub last_job: Option<JobId>,
    pub interval: Option<Interval>,
    pub last_attempt: Option<Timestamp>,
    pub last_success: Option<Timestamp>,
    pub next_attempt: Option<Timestamp>,
    pub deferred_until: Option<Timestamp>,
    pub retry_not_before: Option<Timestamp>,
    pub failures: u32,
    pub outcome: Outcome,
}

#[garmin_macros::portable(default, eq)]
pub struct Snapshot {
    pub job: Option<JobId>,
    pub state: State,
    pub progress: Option<super::LoadProgress>,
    pub cache_generation: Option<String>,
}

/// Cache publication and completion revision observed by a client page.
#[garmin_macros::portable(default, eq)]
pub struct Revision {
    pub generation: Option<String>,
    pub completed: Option<Timestamp>,
}

impl Snapshot {
    #[must_use]
    pub fn revision(&self) -> Revision {
        Revision {
            generation: self.cache_generation.clone(),
            completed: if matches!(self.state.outcome, Outcome::Running) {
                self.state.last_success
            } else {
                self.state.last_attempt
            },
        }
    }
}

#[garmin_macros::portable(copy)]
pub enum Request {
    Status,
    Configure(Option<Interval>),
    Cancel(JobId),
}

impl State {
    pub fn configure(&mut self, interval: Option<Interval>, now: Timestamp) {
        self.last_job = None;
        let credentials_paused = matches!(
            self.outcome,
            Outcome::Failed {
                kind: Failure::Credentials,
                ..
            }
        );
        self.interval = interval;
        self.failures = 0;
        if !credentials_paused {
            self.outcome = Outcome::Never;
        }
        self.next_attempt = interval.filter(|_| !credentials_paused).map(|interval| {
            self.last_success
                .map_or(now, |last| interval.after(last))
                .max(self.deferred_until.unwrap_or(now))
                .max(self.retry_not_before.unwrap_or(now))
        });
    }

    #[must_use]
    pub fn due(&self, now: Timestamp) -> bool {
        self.interval.is_some()
            && !matches!(
                self.outcome,
                Outcome::Failed {
                    kind: Failure::Credentials,
                    ..
                }
            )
            && self.next_attempt.is_none_or(|next| next <= now)
    }

    pub fn finish(
        &mut self,
        outcome: Outcome,
        now: Timestamp,
        retry_after: Option<std::time::Duration>,
    ) {
        self.last_attempt = Some(now);
        match &outcome {
            Outcome::Complete { .. } => {
                self.last_success = Some(now);
                self.failures = 0;
                self.deferred_until = None;
                self.retry_not_before = None;
                self.next_attempt = self.interval.map(|interval| interval.after(now));
            }
            Outcome::Cancelled => {
                self.deferred_until = self.interval.map(|interval| {
                    interval
                        .after(now)
                        .max(self.retry_not_before.unwrap_or(now))
                });
                self.next_attempt = self.deferred_until;
            }
            Outcome::Interrupted => {
                self.next_attempt = Some(now.max(self.retry_not_before.unwrap_or(now)));
            }
            Outcome::Failed {
                kind: Failure::Credentials,
                ..
            } => {
                self.next_attempt = None;
            }
            Outcome::Failed { .. } => {
                self.failures = self.failures.saturating_add(1);
                let backoff = std::time::Duration::from_mins(1_u64 << self.failures.min(6));
                let delay = retry_after.map_or(backoff, |retry| retry.max(backoff));
                let next = now.as_jiff().checked_add(delay).map_or_else(
                    |_| Timestamp::from_jiff(jiff::Timestamp::MAX),
                    Timestamp::from_jiff,
                );
                let next = next.max(self.retry_not_before.unwrap_or(next));
                let next = next.max(self.deferred_until.unwrap_or(next));
                self.retry_not_before = Some(next);
                self.next_attempt = Some(next);
            }
            Outcome::Never | Outcome::Running => {}
        }
        self.outcome = outcome;
        if self.interval.is_none() {
            self.next_attempt = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds).expect("test instant")
    }
    fn daily() -> Interval {
        Interval::new(1, Unit::Day).expect("positive interval")
    }

    #[test]
    fn intervals_are_positive_elapsed_durations_and_keep_the_unit() {
        assert!(Interval::new(0, Unit::Week).is_none());
        assert_eq!(
            Interval::new(2, Unit::Hour)
                .expect("interval")
                .duration()
                .as_secs(),
            7_200
        );
        assert_eq!(daily().duration().as_secs(), 86_400);
        assert_eq!(
            Interval::new(3, Unit::Week)
                .expect("interval")
                .duration()
                .as_secs(),
            1_814_400
        );
        assert_eq!(daily().unit(), Unit::Day);
        assert_eq!(
            Interval::new(u32::MAX, Unit::Week)
                .expect("interval")
                .after(at(0)),
            Timestamp::from_jiff(jiff::Timestamp::MAX)
        );
    }

    #[test]
    fn cancelled_partial_downloads_change_the_client_cache_revision() {
        let mut snapshot = Snapshot::default();
        snapshot.state.configure(Some(daily()), at(0));
        snapshot
            .state
            .finish(Outcome::Complete { unavailable: 0 }, at(10), None);
        let previous = snapshot.revision();
        snapshot.state.outcome = Outcome::Running;
        snapshot.state.last_attempt = Some(at(20));
        assert_eq!(snapshot.revision(), previous);
        snapshot.state.finish(Outcome::Cancelled, at(30), None);
        assert_ne!(snapshot.revision(), previous);
        assert_eq!(snapshot.state.last_success, Some(at(10)));
    }

    #[test]
    fn restart_and_forward_clock_jump_catch_up_once_without_a_backlog() {
        let mut state = State::default();
        state.configure(Some(daily()), at(0));
        assert!(state.due(at(0)));
        state.finish(Outcome::Complete { unavailable: 2 }, at(100), None);
        assert!(!state.due(at(-100)));
        assert!(state.due(at(8_640_000)));
        state.finish(Outcome::Complete { unavailable: 0 }, at(8_640_000), None);
        assert!(!state.due(at(8_640_000)));
        assert_eq!(state.next_attempt, Some(at(8_640_000 + 86_400)));
    }

    #[test]
    fn cancellation_defers_and_interval_edits_preserve_that_deferral() {
        let mut state = State::default();
        state.configure(Some(daily()), at(0));
        state.finish(Outcome::Cancelled, at(10), None);
        assert_eq!(state.last_success, None);
        state.configure(Interval::new(1, Unit::Hour), at(20));
        assert_eq!(state.next_attempt, Some(at(86_410)));
        assert!(!state.due(at(30)));
    }

    #[test]
    fn failures_honor_server_delay_and_credentials_pause_until_reconfigured() {
        let mut state = State::default();
        state.configure(Some(daily()), at(0));
        state.finish(
            Outcome::Failed {
                kind: Failure::RateLimit,
                message: "limited".into(),
            },
            at(10),
            Some(std::time::Duration::from_mins(15)),
        );
        assert_eq!(state.next_attempt, Some(at(910)));
        state.configure(Interval::new(1, Unit::Hour), at(20));
        assert_eq!(
            state.next_attempt,
            Some(at(910)),
            "interval edits cannot bypass Retry-After"
        );
        assert_eq!(state.last_success, None);
        state.finish(
            Outcome::Failed {
                kind: Failure::Credentials,
                message: "rejected".into(),
            },
            at(20),
            None,
        );
        assert!(!state.due(at(8_640_000)));
        state.configure(Some(daily()), at(30));
        assert!(
            !state.due(at(30)),
            "interval edits cannot unpause rejected credentials"
        );
        state = State::default();
        state.configure(Some(daily()), at(30));
        assert!(
            state.due(at(30)),
            "repaired credentials start with fresh state"
        );
        state.configure(None, at(40));
        state.finish(
            Outcome::Failed {
                kind: Failure::Network,
                message: "offline".into(),
            },
            at(50),
            None,
        );
        assert_eq!(state.next_attempt, None);
        assert!(!state.due(at(8_640_000)));
    }
}
