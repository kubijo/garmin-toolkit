//! Optional host-side capture of typed desktop runtime performance measurements.

use std::{
    env,
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use garmin_ui::activity::map_runtime::{
    MapMetricsSink, MapPerformanceSample, MapRenderPerformanceSample,
};
use serde::Serialize;
use thiserror::Error;

const REPORT_ENVIRONMENT: &str = "GARMIN_TOOLKIT_RUNTIME_METRICS";

/// Cloneable application-owned metrics destination.
#[derive(Clone, Default)]
pub struct RuntimeMetricsRecorder(Option<Arc<Recorder>>);

struct Recorder {
    started: Instant,
    state: Mutex<State>,
}

struct State {
    writer: BufWriter<File>,
    previous_desktop_frame: Option<Instant>,
    events: Vec<RuntimeEvent>,
    finished: bool,
}

#[derive(Serialize)]
struct Header<'a> {
    kind: &'a str,
    schema_version: u8,
    started_unix_milliseconds: f64,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RuntimeEvent {
    DesktopFrame {
        elapsed_milliseconds: f64,
        frame_milliseconds: Option<f32>,
        ui_milliseconds: f32,
    },
    MapFrame {
        elapsed_milliseconds: f64,
        #[serde(flatten)]
        sample: MapPerformanceSample,
    },
    MapRender {
        elapsed_milliseconds: f64,
        #[serde(flatten)]
        sample: MapRenderPerformanceSample,
    },
}

impl RuntimeMetricsRecorder {
    /// Open the destination requested by the profiling harness, if any.
    pub fn from_environment() -> Result<Self, Error> {
        let Some(path) = env::var_os(REPORT_ENVIRONMENT).map(PathBuf::from) else {
            return Ok(Self::default());
        };
        if path.as_os_str().is_empty() {
            return Err(Error::EmptyPath);
        }
        Self::open(&path)
    }

    fn open(path: &Path) -> Result<Self, Error> {
        let started = Instant::now();
        let started_unix_milliseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(Error::Clock)?
            .as_secs_f64()
            * 1_000.0;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| Error::Open {
                path: path.to_path_buf(),
                source,
            })?;
        let mut writer = BufWriter::new(file);
        write_json_line(
            &mut writer,
            &Header {
                kind: "header",
                schema_version: 1,
                started_unix_milliseconds,
            },
        )?;
        writer.flush()?;
        Ok(Self(Some(Arc::new(Recorder {
            started,
            state: Mutex::new(State {
                writer,
                previous_desktop_frame: None,
                events: Vec::new(),
                finished: false,
            }),
        }))))
    }

    /// Flush all measurements and surface any asynchronous recording failure.
    pub fn finish(&self) -> Result<(), Error> {
        let Some(recorder) = &self.0 else {
            return Ok(());
        };
        let mut state = recorder.state.lock().map_err(|_| Error::Poisoned)?;
        if state.finished {
            return Ok(());
        }
        let events = std::mem::take(&mut state.events);
        for event in events {
            write_json_line(&mut state.writer, &event)?;
        }
        state.finished = true;
        state.writer.flush().map_err(Error::Write)
    }

    /// Record the complete desktop immediate-mode UI pass.
    pub fn record_desktop_frame(&self, ui_milliseconds: f32) {
        let Some(recorder) = &self.0 else {
            return;
        };
        let now = Instant::now();
        let Ok(mut state) = recorder.state.lock() else {
            return;
        };
        if state.finished {
            return;
        }
        let frame_milliseconds = state
            .previous_desktop_frame
            .replace(now)
            .map(|previous| now - previous)
            .filter(|elapsed| *elapsed <= std::time::Duration::from_millis(250))
            .map(|elapsed| elapsed.as_secs_f32() * 1_000.0);
        state.events.push(RuntimeEvent::DesktopFrame {
            elapsed_milliseconds: recorder.started.elapsed().as_secs_f64() * 1_000.0,
            frame_milliseconds,
            ui_milliseconds,
        });
    }
}

impl MapMetricsSink for RuntimeMetricsRecorder {
    fn record(&self, sample: MapPerformanceSample) {
        let Some(recorder) = &self.0 else {
            return;
        };
        let Ok(mut state) = recorder.state.lock() else {
            return;
        };
        if state.finished {
            return;
        }
        state.events.push(RuntimeEvent::MapFrame {
            elapsed_milliseconds: recorder.started.elapsed().as_secs_f64() * 1_000.0,
            sample,
        });
    }

    fn record_render(&self, sample: MapRenderPerformanceSample) {
        let Some(recorder) = &self.0 else {
            return;
        };
        let Ok(mut state) = recorder.state.lock() else {
            return;
        };
        if state.finished {
            return;
        }
        state.events.push(RuntimeEvent::MapRender {
            elapsed_milliseconds: recorder.started.elapsed().as_secs_f64() * 1_000.0,
            sample,
        });
    }
}

fn write_json_line(writer: &mut BufWriter<File>, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(io::Error::other)?;
    writer.write_all(b"\n")
}

/// Failure to initialize or finish a requested metrics capture.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{REPORT_ENVIRONMENT} must name a non-empty file")]
    EmptyPath,
    #[error("the system clock is earlier than the Unix epoch: {0}")]
    Clock(#[from] std::time::SystemTimeError),
    #[error("could not create runtime metrics file {path}: {source}")]
    Open { path: PathBuf, source: io::Error },
    #[error("runtime metrics recorder lock was poisoned")]
    Poisoned,
    #[error("could not write runtime metrics: {0}")]
    Write(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MapPerformanceSample {
        MapPerformanceSample {
            frame_milliseconds: Some(16.0),
            ui_milliseconds: 2.0,
            scene_milliseconds: 0.5,
            route_query_microseconds: 12.0,
            label_milliseconds: 0.2,
            label_backlog: 1,
            stale_work: 2,
            visible_tiles: 6,
            ready_tiles: 5,
            pending_tiles: 1,
            queued_upload_bytes: 256,
            uploaded_bytes: 128,
        }
    }

    #[test]
    fn writes_an_immutable_header_and_timed_samples() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metrics.ndjson");
        let recorder = RuntimeMetricsRecorder::open(&path).unwrap();
        recorder.record(sample());
        recorder.record_desktop_frame(4.0);
        recorder.finish().unwrap();

        let lines = std::fs::read_to_string(path).unwrap();
        let rows = lines
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["kind"], "header");
        assert_eq!(rows[0]["schema_version"], 1);
        assert_eq!(rows[1]["kind"], "map_frame");
        assert_eq!(rows[1]["visible_tiles"], 6);
        assert_eq!(rows[2]["kind"], "desktop_frame");
    }

    #[test]
    fn refuses_to_replace_an_existing_raw_metrics_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metrics.ndjson");
        std::fs::write(&path, "evidence").unwrap();

        assert!(matches!(
            RuntimeMetricsRecorder::open(&path),
            Err(Error::Open { .. })
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "evidence");
    }
}
