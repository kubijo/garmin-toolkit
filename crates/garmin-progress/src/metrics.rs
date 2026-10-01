//! Timing and rate observation shared by terminal and graphical presentations.

use crate::{ProgressUnit, model::StageView};
use std::time::{Duration, SystemTime};

const STALE_BYTE_PROGRESS_AFTER: Duration = Duration::from_secs(10);
const MIN_RATE_SAMPLE_DURATION: Duration = Duration::from_millis(250);

#[must_use]
pub fn stage_elapsed(view: &StageView) -> Option<Duration> {
    view.elapsed
        .or_else(|| {
            view.started_recorded_at
                .and_then(|started| SystemTime::now().duration_since(started).ok())
        })
        .or_else(|| view.started_at.map(|started| started.elapsed()))
}

#[must_use]
pub fn stale_byte_progress(view: &StageView) -> Option<Duration> {
    if view.unit != ProgressUnit::Bytes || !view.is_active() {
        return None;
    }
    view.updated_at
        .map(|updated| updated.elapsed())
        .filter(|idle| *idle >= STALE_BYTE_PROGRESS_AFTER)
}

fn byte_sample_elapsed(view: &StageView) -> Option<Duration> {
    view.started_recorded_at
        .zip(view.updated_recorded_at)
        .and_then(|(started, updated)| updated.duration_since(started).ok())
        .or_else(|| {
            view.started_at
                .zip(view.updated_at)
                .map(|(started, updated)| updated.saturating_duration_since(started))
        })
        .or(view.elapsed)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub elapsed: Option<Duration>,
    pub idle: Option<Duration>,
    pub bytes_per_second: Option<u64>,
    pub remaining: Option<Duration>,
}

#[must_use]
pub fn metrics(view: &StageView) -> Metrics {
    let mut result = Metrics {
        elapsed: stage_elapsed(view),
        idle: stale_byte_progress(view),
        ..Metrics::default()
    };
    if result.idle.is_some() || view.unit != ProgressUnit::Bytes || view.completed == 0 {
        return result;
    }
    let Some(sample) =
        byte_sample_elapsed(view).filter(|sample| *sample >= MIN_RATE_SAMPLE_DURATION)
    else {
        return result;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "transfer rates and ETA are approximate"
    )]
    let rate = view.completed as f64 / sample.as_secs_f64();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "nonnegative display rate saturates at u64::MAX"
    )]
    {
        result.bytes_per_second = Some(rate.max(0.0).round() as u64);
    }
    if view.is_active()
        && let Some(total) = view.total
        && total > view.completed
        && rate.is_finite()
        && rate > 0.0
    {
        #[expect(clippy::cast_precision_loss, reason = "transfer ETA is approximate")]
        let seconds = (total - view.completed) as f64 / rate;
        result.remaining = Duration::try_from_secs_f64(seconds).ok();
    }
    result
}
