use std::collections::VecDeque;

pub(super) struct Limits {
    chunk_bytes: usize,
    frame_bytes: usize,
    frame_milliseconds: f64,
    retained_bytes: usize,
    pending: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            chunk_bytes: 256 * 1024,
            frame_bytes: 4 * 1024 * 1024,
            frame_milliseconds: 1.5,
            retained_bytes: 64 * 1024 * 1024,
            pending: 64,
        }
    }
}

pub(super) enum Step {
    Allocated,
    /// The next indivisible element does not fit the remaining frame budget.
    Deferred,
    Copied {
        bytes: usize,
        text_milliseconds: f64,
    },
}

pub(super) trait Work {
    fn retained_bytes(&self) -> usize;
    fn is_complete(&self) -> bool;
    fn advance(&mut self, maximum: usize, scratch: &mut Vec<u8>) -> Result<Step, String>;
}

pub(super) struct Frame<T> {
    pub completed: Vec<(T, Result<(), String>)>,
    pub text_milliseconds: f64,
    pub did_work: bool,
}

pub(super) struct Admission<T> {
    limits: Limits,
    pending: VecDeque<T>,
    scratch: Vec<u8>,
    retained: usize,
    scheduled: bool,
}

impl<T> Default for Admission<T> {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            pending: VecDeque::new(),
            scratch: Vec::new(),
            retained: 0,
            scheduled: false,
        }
    }
}

impl<T: Work> Admission<T> {
    pub fn enqueue(&mut self, work: T) -> Result<(), T> {
        let retained = self.retained.checked_add(work.retained_bytes());
        if self.pending.len() >= self.limits.pending
            || retained.is_none_or(|bytes| bytes > self.limits.retained_bytes)
        {
            return Err(work);
        }
        self.retained = retained.expect("checked above");
        self.pending.push_back(work);
        Ok(())
    }

    pub fn schedule(&mut self, other_work: bool) -> bool {
        if self.scheduled || (self.pending.is_empty() && !other_work) {
            return false;
        }
        self.scheduled = true;
        true
    }

    fn pop(&mut self) -> T {
        let work = self.pending.pop_front().expect("front was present");
        self.retained -= work.retained_bytes();
        work
    }

    pub fn advance_frame(&mut self, started: f64, mut now: impl FnMut() -> f64) -> Frame<T> {
        self.scheduled = false;
        let mut frame = Frame {
            completed: Vec::new(),
            text_milliseconds: 0.0,
            did_work: false,
        };
        let mut admitted = 0;
        while admitted < self.limits.frame_bytes && now() - started < self.limits.frame_milliseconds
        {
            let Some(work) = self.pending.front_mut() else {
                break;
            };
            frame.did_work = true;
            if work.is_complete() {
                frame.completed.push((self.pop(), Ok(())));
                continue;
            }
            let maximum = self
                .limits
                .chunk_bytes
                .min(self.limits.frame_bytes - admitted);
            match work.advance(maximum, &mut self.scratch) {
                Ok(Step::Allocated) => {}
                Ok(Step::Deferred) if admitted > 0 => break,
                Ok(Step::Copied {
                    bytes,
                    text_milliseconds,
                }) if bytes > 0 && bytes <= maximum => {
                    admitted += bytes;
                    frame.text_milliseconds += text_milliseconds;
                }
                result => {
                    let error = result
                        .err()
                        .unwrap_or_else(|| "tile admission made invalid progress".to_owned());
                    frame.completed.push((self.pop(), Err(error)));
                }
            }
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    struct Job {
        remaining: usize,
        retained: usize,
        allocation: bool,
        fail: bool,
        alignment: usize,
        clock: Rc<Cell<f64>>,
        cost: f64,
    }
    impl Work for Job {
        fn retained_bytes(&self) -> usize {
            self.retained
        }
        fn is_complete(&self) -> bool {
            !self.allocation && self.remaining == 0
        }
        fn advance(&mut self, maximum: usize, _: &mut Vec<u8>) -> Result<Step, String> {
            self.clock.set(self.clock.get() + self.cost);
            if self.fail {
                return Err("injected failure".to_owned());
            }
            if std::mem::take(&mut self.allocation) {
                return Ok(Step::Allocated);
            }
            let bytes = maximum.min(self.remaining);
            let bytes = bytes - bytes % self.alignment;
            if bytes == 0 {
                return Ok(Step::Deferred);
            }
            self.remaining -= bytes;
            Ok(Step::Copied {
                bytes,
                text_milliseconds: 0.25,
            })
        }
    }
    fn job(clock: &Rc<Cell<f64>>, bytes: usize) -> Job {
        Job {
            remaining: bytes,
            retained: bytes,
            allocation: false,
            fail: false,
            alignment: 1,
            clock: Rc::clone(clock),
            cost: 0.0,
        }
    }
    fn queue() -> Admission<Job> {
        Admission {
            limits: Limits {
                chunk_bytes: 4,
                frame_bytes: 8,
                frame_milliseconds: 1.5,
                retained_bytes: 32,
                pending: 2,
            },
            ..Default::default()
        }
    }

    #[test]
    fn default_budget_uses_headroom_but_stops_at_bytes_or_time() {
        let clock = Rc::new(Cell::new(0.0));
        let mut fast = Admission::default();
        assert!(fast.enqueue(job(&clock, 5 * 1024 * 1024)).is_ok());
        let frame = fast.advance_frame(0.0, || clock.get());
        assert!(frame.completed.is_empty());
        assert_eq!(fast.pending.front().unwrap().remaining, 1024 * 1024);

        let mut slow = Admission::default();
        let mut work = job(&clock, 5 * 1024 * 1024);
        work.cost = 1.0;
        assert!(slow.enqueue(work).is_ok());
        let frame = slow.advance_frame(0.0, || clock.get());
        assert!(frame.completed.is_empty());
        assert_eq!(
            slow.pending.front().unwrap().remaining,
            5 * 1024 * 1024 - 512 * 1024
        );
    }

    #[test]
    fn partial_frame_defers_an_indivisible_record_without_losing_work() {
        let clock = Rc::new(Cell::new(0.0));
        let mut queue = queue();
        let mut record = job(&clock, 4);
        record.alignment = 4;
        assert!(queue.enqueue(job(&clock, 7)).is_ok());
        assert!(queue.enqueue(record).is_ok());
        let frame = queue.advance_frame(0.0, || clock.get());
        assert_eq!(frame.completed.len(), 1);
        assert!(frame.completed[0].1.is_ok());
        assert_eq!(queue.retained, 4);
        assert!(queue.schedule(false));
        let frame = queue.advance_frame(0.0, || clock.get());
        assert_eq!(frame.completed.len(), 1);
        assert!(frame.completed[0].1.is_ok());
        assert_eq!(queue.retained, 0);
    }

    #[test]
    fn work_that_cannot_fit_a_fresh_frame_fails_instead_of_rescheduling_forever() {
        let clock = Rc::new(Cell::new(0.0));
        let mut queue = queue();
        let mut work = job(&clock, 8);
        work.alignment = 8;
        assert!(queue.enqueue(work).is_ok());
        let frame = queue.advance_frame(0.0, || clock.get());
        assert_eq!(frame.completed.len(), 1);
        assert!(frame.completed[0].1.is_err());
        assert_eq!(queue.retained, 0);
        assert!(!queue.schedule(false));
    }

    #[test]
    fn byte_budget_reschedules_and_completes_exactly_once() {
        let clock = Rc::new(Cell::new(0.0));
        let mut queue = queue();
        assert!(queue.enqueue(job(&clock, 10)).is_ok());
        assert!(queue.schedule(false));
        assert!(!queue.schedule(false));
        let first = queue.advance_frame(0.0, || clock.get());
        assert!(first.completed.is_empty());
        assert_eq!(queue.pending.front().unwrap().remaining, 2);
        assert!(
            (first.text_milliseconds - 0.5).abs() < f64::EPSILON,
            "expected 0.5 ms of text work, got {}",
            first.text_milliseconds
        );
        assert!(queue.schedule(false));
        let second = queue.advance_frame(0.0, || clock.get());
        assert_eq!(second.completed.len(), 1);
        assert!(second.completed[0].1.is_ok());
        assert_eq!(queue.retained, 0);
        assert!(!queue.schedule(false));
        assert!(
            queue
                .advance_frame(0.0, || clock.get())
                .completed
                .is_empty()
        );
    }

    #[test]
    fn allocation_and_prior_data_work_consume_the_time_budget() {
        let clock = Rc::new(Cell::new(0.0));
        let mut queue = queue();
        let mut work = job(&clock, 8);
        work.allocation = true;
        work.cost = 2.0;
        assert!(queue.enqueue(work).is_ok());
        assert!(
            queue
                .advance_frame(0.0, || clock.get())
                .completed
                .is_empty()
        );
        assert_eq!(queue.pending.front().unwrap().remaining, 8);
        assert!(!queue.pending.front().unwrap().allocation);
        assert!(
            queue
                .advance_frame(0.0, || clock.get())
                .completed
                .is_empty()
        );
        assert_eq!(queue.pending.front().unwrap().remaining, 8);
        assert!(queue.schedule(true));
    }

    #[test]
    fn data_only_or_exhausted_frame_does_not_report_tile_work() {
        let clock = Rc::new(Cell::new(0.0));
        let mut queue = queue();
        assert!(queue.schedule(true));
        assert!(!queue.advance_frame(0.0, || clock.get()).did_work);
        assert!(queue.enqueue(job(&clock, 4)).is_ok());
        clock.set(2.0);
        assert!(!queue.advance_frame(0.0, || clock.get()).did_work);
        let frame = queue.advance_frame(2.0, || clock.get());
        assert!(frame.did_work);
        assert_eq!(frame.completed.len(), 1);
    }

    #[test]
    fn queue_limits_and_failed_work_release_retention() {
        let clock = Rc::new(Cell::new(0.0));
        let mut queue = queue();
        let mut failed = job(&clock, 20);
        failed.fail = true;
        assert!(queue.enqueue(failed).is_ok());
        assert!(queue.enqueue(job(&clock, 13)).is_err());
        assert!(queue.enqueue(job(&clock, 0)).is_ok());
        assert!(queue.enqueue(job(&clock, 0)).is_err());
        let frame = queue.advance_frame(0.0, || clock.get());
        assert_eq!(frame.completed.len(), 2);
        assert_eq!(
            frame.completed[0].1.as_ref().unwrap_err(),
            "injected failure"
        );
        assert!(frame.completed[1].1.is_ok());
        assert_eq!(queue.retained, 0);
        assert!(queue.enqueue(job(&clock, 32)).is_ok());
    }
}
