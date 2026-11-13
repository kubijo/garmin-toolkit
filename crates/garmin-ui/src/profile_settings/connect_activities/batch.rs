//! One explicitly selected page, processed sequentially through reviewed FIT imports.

use std::collections::{HashSet, VecDeque};

use garmin_model::garmin_connect::{
    ActivityFailure, ActivityRequest, ActivityResponse, RemoteActivity,
};
use garmin_service_api::ConnectActivityResult;

#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub total: usize,
    pub imported: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl Counts {
    #[must_use]
    pub const fn processed(self) -> usize {
        self.imported + self.skipped + self.failed
    }
}

#[derive(Clone, Debug)]
pub enum FailureReason {
    Request(ActivityFailure),
    UnexpectedResponse,
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub name: String,
    pub reason: FailureReason,
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Prepare,
    Import,
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub counts: Counts,
    pub failures: Vec<Failure>,
    queue: VecDeque<RemoteActivity>,
    current: Option<(RemoteActivity, Phase)>,
    cancelled: bool,
    finished: bool,
    stopped: bool,
}

pub struct Step {
    pub request: Option<ActivityRequest>,
    pub receipt: Option<ActivityResponse>,
}

impl Batch {
    /// Snapshots at most one page; imported rows are counted without requesting them.
    #[must_use]
    pub fn new(activities: &[RemoteActivity]) -> Option<Self> {
        if activities.len() > 20 {
            return None;
        }
        let mut ids = HashSet::with_capacity(activities.len());
        if activities
            .iter()
            .any(|activity| activity.id <= 0 || !ids.insert(activity.id))
        {
            return None;
        }
        let mut batch = Self {
            counts: Counts {
                total: activities.len(),
                ..Counts::default()
            },
            failures: Vec::new(),
            queue: VecDeque::new(),
            current: None,
            cancelled: false,
            finished: false,
            stopped: false,
        };
        for activity in activities {
            if activity.imported {
                batch.counts.skipped += 1;
            } else {
                batch.queue.push_back(activity.clone());
            }
        }
        batch.finished = batch.queue.is_empty();
        Some(batch)
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        !self.finished
    }

    #[must_use]
    pub const fn cancelled(&self) -> bool {
        self.cancelled
    }

    #[must_use]
    pub const fn stopped(&self) -> bool {
        self.stopped
    }

    #[must_use]
    pub fn current_name(&self) -> Option<&str> {
        self.current
            .as_ref()
            .map(|(activity, _)| activity.name.as_str())
    }

    pub fn cancel(&mut self) {
        if self.finished {
            return;
        }
        self.cancelled = true;
        if self.current.is_none() {
            self.queue.clear();
            self.finished = true;
        }
    }

    /// Starts the next file only when no request is outstanding.
    pub fn next_request(&mut self) -> Option<ActivityRequest> {
        if self.current.is_some() || self.finished {
            return None;
        }
        if self.cancelled || self.stopped {
            self.queue.clear();
            self.finished = true;
            return None;
        }
        let Some(activity) = self.queue.pop_front() else {
            self.finished = true;
            return None;
        };
        let request = ActivityRequest::Prepare { id: activity.id };
        self.current = Some((activity, Phase::Prepare));
        Some(request)
    }

    /// Consumes one response; preparation receipts contain no recording samples.
    pub fn complete(&mut self, result: Result<ConnectActivityResult, ActivityFailure>) -> Step {
        let Some((activity, phase)) = self.current.take() else {
            return Step {
                request: None,
                receipt: None,
            };
        };
        let mut receipt = None;
        match (phase, result) {
            (
                Phase::Prepare,
                Ok(ConnectActivityResult::Prepared {
                    id,
                    digest,
                    imported,
                }),
            ) if id == activity.id => {
                if imported {
                    self.counts.skipped += 1;
                    receipt = Some(ActivityResponse::Imported {
                        id: activity.id,
                        activities: 0,
                        duplicate: true,
                    });
                } else if !self.cancelled {
                    let request = ActivityRequest::Import {
                        id: activity.id,
                        digest,
                    };
                    self.current = Some((activity, Phase::Import));
                    return Step {
                        request: Some(request),
                        receipt: None,
                    };
                }
            }
            (
                Phase::Import,
                Ok(ConnectActivityResult::State(
                    response @ ActivityResponse::Imported { id, duplicate, .. },
                )),
            ) if id == activity.id => {
                if duplicate {
                    self.counts.skipped += 1;
                } else {
                    self.counts.imported += 1;
                }
                receipt = Some(response);
            }
            (_, Err(reason)) => {
                self.stopped = reason.stops_batch();
                self.fail(activity, FailureReason::Request(reason));
            }
            _ => self.fail(activity, FailureReason::UnexpectedResponse),
        }
        Step {
            request: self.next_request(),
            receipt,
        }
    }

    fn fail(&mut self, activity: RemoteActivity, reason: FailureReason) {
        self.counts.failed += 1;
        self.failures.push(Failure {
            name: activity.name,
            reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, imported: bool) -> RemoteActivity {
        RemoteActivity {
            id,
            imported,
            name: format!("Ride {id}"),
            sport: None,
            started_at: None,
            distance_metres: None,
            duration_seconds: None,
        }
    }

    fn preview(id: i64, imported: bool) -> ConnectActivityResult {
        ConnectActivityResult::Prepared {
            id,
            imported,
            digest: "reviewed-bytes".into(),
        }
    }

    fn imported(id: i64) -> ConnectActivityResult {
        ConnectActivityResult::State(ActivityResponse::Imported {
            id,
            activities: 1,
            duplicate: false,
        })
    }

    #[test]
    fn skips_existing_and_imports_only_the_snapshotted_page_through_review_digests() {
        let mut rows = [row(1, true), row(2, false), row(3, false)];
        let mut batch = Batch::new(&rows).unwrap();
        rows[1].id = 99;
        assert_eq!(
            batch.next_request(),
            Some(ActivityRequest::Prepare { id: 2 })
        );
        assert!(
            batch.next_request().is_none(),
            "only one request may be outstanding"
        );
        assert_eq!(
            batch.complete(Ok(preview(2, false))).request,
            Some(ActivityRequest::Import {
                id: 2,
                digest: "reviewed-bytes".into()
            })
        );
        let step = batch.complete(Ok(imported(2)));
        assert_eq!(step.request, Some(ActivityRequest::Prepare { id: 3 }));
        assert!(matches!(
            step.receipt,
            Some(ActivityResponse::Imported { id: 2, .. })
        ));
        let step = batch.complete(Ok(preview(3, true)));
        assert!(step.request.is_none());
        assert!(matches!(
            step.receipt,
            Some(ActivityResponse::Imported {
                id: 3,
                duplicate: true,
                ..
            })
        ));
        assert_eq!(
            (
                batch.counts.imported,
                batch.counts.skipped,
                batch.counts.failed
            ),
            (1, 2, 0)
        );
        assert!(!batch.running());
    }

    #[test]
    fn cancellation_during_preview_never_starts_an_import() {
        let mut batch = Batch::new(&[row(1, false), row(2, false)]).unwrap();
        assert!(batch.next_request().is_some());
        batch.cancel();
        assert!(batch.complete(Ok(preview(1, false))).request.is_none());
        assert_eq!(batch.counts.processed(), 0);
        assert!(batch.cancelled());
        assert!(!batch.running());
    }

    #[test]
    fn failures_continue_and_cancellation_preserves_an_in_flight_import() {
        let mut batch = Batch::new(&[row(1, false), row(2, false), row(3, false)]).unwrap();
        assert!(batch.next_request().is_some());
        assert_eq!(
            batch
                .complete(Err(ActivityFailure {
                    message: "invalid FIT".into(),
                    kind: garmin_model::garmin_connect::ActivityFailureKind::Activity
                }))
                .request,
            Some(ActivityRequest::Prepare { id: 2 })
        );
        assert!(matches!(
            batch.complete(Ok(preview(2, false))).request,
            Some(ActivityRequest::Import { id: 2, .. })
        ));
        batch.cancel();
        assert!(batch.complete(Ok(imported(2))).request.is_none());
        assert_eq!(
            (
                batch.counts.imported,
                batch.counts.failed,
                batch.counts.total
            ),
            (1, 1, 3)
        );
        assert_eq!(batch.failures.len(), 1);
        assert!(!batch.running());
    }

    #[test]
    fn rejects_scope_expansion_and_mismatched_preview_responses() {
        assert!(Batch::new(&(1..=21).map(|id| row(id, false)).collect::<Vec<_>>()).is_none());
        assert!(Batch::new(&[row(1, false), row(1, false)]).is_none());
        let mut batch = Batch::new(&[row(1, false), row(2, false)]).unwrap();
        assert!(batch.next_request().is_some());
        assert_eq!(
            batch.complete(Ok(preview(99, false))).request,
            Some(ActivityRequest::Prepare { id: 2 })
        );
        assert_eq!(batch.counts.failed, 1);
    }

    #[test]
    fn session_failure_stops_without_requesting_the_remaining_page() {
        let mut batch = Batch::new(&[row(1, false), row(2, false), row(3, false)]).unwrap();
        assert!(batch.next_request().is_some());
        assert!(batch.complete(Ok(preview(1, false))).request.is_some());
        assert_eq!(
            batch.complete(Ok(imported(1))).request,
            Some(ActivityRequest::Prepare { id: 2 })
        );
        let failure = ActivityFailure {
            message: "Rate limited".into(),
            kind: garmin_model::garmin_connect::ActivityFailureKind::Session,
        };
        assert!(batch.complete(Err(failure)).request.is_none());
        assert!(batch.stopped());
        assert!(!batch.running());
        assert_eq!(
            (
                batch.counts.imported,
                batch.counts.failed,
                batch.counts.total
            ),
            (1, 1, 3)
        );
    }
}
