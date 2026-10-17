//! Transition-only upload telemetry; no per-frame history or unbounded identity registry.

use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use web_time::Instant;

use crate::activity::map_runtime::{MapMetrics, MapUploadEvent, MapUploadPhase};

static NEXT_UPLOAD: AtomicUsize = AtomicUsize::new(1);

pub(super) struct UploadTrace {
    id: usize,
    tile: walkers::TileId,
    metrics: MapMetrics,
    started: Instant,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    hidden: bool,
    phase: Phase,
    work_ms: f64,
    bytes: usize,
    steps: usize,
}

#[derive(Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Queued,
    Working,
    Published,
    Drawn,
}

impl UploadTrace {
    pub(super) fn new(tile: walkers::TileId, metrics: MapMetrics) -> Self {
        let trace = Self {
            id: NEXT_UPLOAD.fetch_add(1, Ordering::Relaxed),
            tile,
            metrics,
            started: Instant::now(),
            state: Mutex::new(State::default()),
        };
        trace.emit(MapUploadPhase::Queued, 0.0, 0.0, 0);
        trace
    }

    fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1_000.0
    }

    #[cfg(test)]
    pub(super) fn advance_clock(&mut self, duration: std::time::Duration) {
        self.started -= duration;
    }

    fn emit(&self, event: MapUploadPhase, elapsed_ms: f64, work_ms: f64, bytes: usize) {
        self.metrics.record_upload_event(MapUploadEvent {
            version: 1,
            upload_id: self.id,
            zoom: self.tile.zoom,
            x: self.tile.x,
            y: self.tile.y,
            elapsed_ms,
            event,
            work_ms,
            bytes,
        });
    }

    pub(super) fn visible(&self, visible: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(state.phase, Phase::Queued | Phase::Working) && state.hidden == visible {
            state.hidden = !visible;
            self.emit(
                if visible {
                    MapUploadPhase::Visible
                } else {
                    MapUploadPhase::Hidden
                },
                self.elapsed(),
                0.0,
                0,
            );
        }
    }

    pub(super) fn begin_work(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.phase == Phase::Queued {
            state.phase = Phase::Working;
            self.emit(MapUploadPhase::FirstWork, self.elapsed(), 0.0, 0);
        }
    }

    pub(super) fn work(&self, milliseconds: f64, bytes: usize) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.work_ms += milliseconds;
        state.bytes += bytes;
        state.steps += 1;
    }

    pub(super) fn flush(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.steps != 0 {
            self.emit(
                MapUploadPhase::Progress,
                self.elapsed(),
                state.work_ms,
                state.bytes,
            );
            state.work_ms = 0.0;
            state.bytes = 0;
            state.steps = 0;
        }
    }

    pub(super) fn published(&self) {
        self.flush();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.phase == Phase::Working {
            state.phase = Phase::Published;
            self.emit(MapUploadPhase::Published, self.elapsed(), 0.0, 0);
        }
    }

    pub(super) fn drawn(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.phase == Phase::Published {
            state.phase = Phase::Drawn;
            self.emit(MapUploadPhase::FirstDraw, self.elapsed(), 0.0, 0);
        }
    }
}

impl Drop for UploadTrace {
    fn drop(&mut self) {
        self.flush();
        if self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase
            != Phase::Drawn
        {
            self.emit(MapUploadPhase::Released, self.elapsed(), 0.0, 0);
        }
    }
}

#[cfg(test)]
pub(in crate::activity) mod tests {
    use super::*;
    use crate::activity::map_runtime::{MapMetricsSink, MapPerformanceSample};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    pub(in crate::activity) struct Capture(pub(in crate::activity) Arc<Mutex<Vec<MapUploadEvent>>>);

    impl MapMetricsSink for Capture {
        fn record(&self, _: MapPerformanceSample) {}
        fn record_upload_event(&self, event: MapUploadEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn lifecycle_is_correlated_and_progress_is_batched() {
        let capture = Capture::default();
        let trace = UploadTrace::new(
            walkers::TileId {
                zoom: 3,
                x: 4,
                y: 2,
            },
            MapMetrics::new(capture.clone()),
        );
        trace.visible(true);
        trace.begin_work();
        trace.begin_work();
        trace.work(0.25, 256);
        trace.work(0.5, 128);
        trace.flush();
        trace.flush();
        trace.visible(false);
        trace.visible(false);
        trace.visible(true);
        trace.published();
        trace.published();
        trace.drawn();
        trace.drawn();
        drop(trace);
        let events = capture.0.lock().unwrap();
        assert_eq!(
            events.iter().map(|event| event.event).collect::<Vec<_>>(),
            [
                MapUploadPhase::Queued,
                MapUploadPhase::FirstWork,
                MapUploadPhase::Progress,
                MapUploadPhase::Hidden,
                MapUploadPhase::Visible,
                MapUploadPhase::Published,
                MapUploadPhase::FirstDraw,
            ]
        );
        assert!(
            events
                .iter()
                .all(|event| event.upload_id == events[0].upload_id
                    && (event.zoom, event.x, event.y) == (3, 4, 2))
        );
        assert_eq!(events[2].bytes, 384);
        assert!((events[2].work_ms - 0.75).abs() < f64::EPSILON);
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].elapsed_ms <= pair[1].elapsed_ms)
        );
    }

    #[test]
    fn replacement_gets_a_new_identity_and_unfinished_work_is_released() {
        let capture = Capture::default();
        let tile = walkers::TileId {
            zoom: 1,
            x: 0,
            y: 0,
        };
        let first = UploadTrace::new(tile, MapMetrics::new(capture.clone()));
        let second = UploadTrace::new(tile, MapMetrics::new(capture.clone()));
        assert_ne!(first.id, second.id);
        drop(first);
        second.begin_work();
        second.published();
        drop(second);
        let events = capture.0.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == MapUploadPhase::Released)
                .count(),
            2
        );
        assert!(
            !events
                .iter()
                .any(|event| event.event == MapUploadPhase::FirstDraw)
        );
    }
}
