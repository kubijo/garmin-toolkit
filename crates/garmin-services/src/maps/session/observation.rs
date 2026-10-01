//! Portable snapshots are reduced on the host;
//! clients never replay an event stream.

use garmin_progress::{
    OperationStage, ProgressReceiver, ProgressUnit,
    metrics::metrics,
    model::{ProgressModel, StageView},
};
use garmin_service_api::maps::{Progress, ProgressStatus, State};

pub(super) struct Observation(ProgressModel);

impl Observation {
    pub fn new() -> Self {
        use OperationStage::{
            Authorize, Backup, Cleanup, Commit, Delete, DeviceFinalize, DeviceVerify, Download,
            Inspect, Plan, Query, Stage, Upload, Verify,
        };
        Self(ProgressModel::new(
            &[
                Inspect,
                Query,
                Plan,
                Backup,
                Download,
                Verify,
                Authorize,
                Stage,
                Commit,
                Cleanup,
                Upload,
                DeviceFinalize,
                DeviceVerify,
                Delete,
            ],
            "Waiting",
        ))
    }

    pub fn update(&mut self, state: &mut State, receiver: &ProgressReceiver) {
        for event in receiver.try_iter() {
            self.0.apply(&event);
        }
        state.progress = self
            .0
            .stages
            .iter()
            .filter(|(_, view)| view.state.is_some())
            .map(|(stage, view)| snapshot(*stage, view))
            .collect();
        state.events = self
            .0
            .history
            .iter()
            .map(|event| Progress {
                stage: event
                    .stage
                    .map_or_else(String::new, |stage| format!("{stage:?}")),
                label: event.label.clone(),
                completed: 0,
                total: None,
                bytes: false,
                elapsed_ms: event.duration.map_or(0, millis),
                bytes_per_second: None,
                remaining_seconds: None,
                stalled: false,
                path: event.path.clone(),
                status: status(event.state),
            })
            .collect();
        state.active = self
            .0
            .active()
            .map(|item| snapshot(item.stage, &item.view))
            .collect();
        match receiver.device_state() {
            Some(Ok(device)) => {
                state.storages = device.storages;
                state.storage_error = None;
            }
            Some(Err(error)) => {
                state.storages.clear();
                state.storage_error = Some(error);
            }
            None => {}
        }
    }
}

fn snapshot(stage: OperationStage, view: &StageView) -> Progress {
    let observation = metrics(view);
    Progress {
        stage: format!("{stage:?}"),
        label: view.label.clone(),
        completed: view.completed,
        total: view.total,
        bytes: view.unit == ProgressUnit::Bytes,
        elapsed_ms: observation.elapsed.map_or(0, millis),
        bytes_per_second: observation.bytes_per_second,
        remaining_seconds: observation.remaining.map(|value| value.as_secs()),
        stalled: observation.idle.is_some(),
        path: view.path.clone(),
        status: status(view.state),
    }
}

fn status(state: Option<garmin_progress::ProgressState>) -> ProgressStatus {
    use garmin_progress::ProgressState;
    match state {
        None => ProgressStatus::Waiting,
        Some(ProgressState::Started | ProgressState::Advanced) => ProgressStatus::Running,
        Some(ProgressState::Completed) => ProgressStatus::Completed,
        Some(ProgressState::Failed) => ProgressStatus::Failed,
    }
}

fn millis(value: std::time::Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}
