//! Observation and cancellation contracts shared by long-running operations.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;

mod device_state;
use device_state::DeviceStateTracker;
pub use device_state::{DeviceStateUpdate, ProgressReceiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStage {
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
}

impl OperationStage {
    #[must_use]
    pub const fn progress_unit(self) -> ProgressUnit {
        match self {
            Self::Backup
            | Self::Download
            | Self::Verify
            | Self::Stage
            | Self::Commit
            | Self::Upload
            | Self::DeviceVerify => ProgressUnit::Bytes,
            Self::Inspect
            | Self::Query
            | Self::Plan
            | Self::Authorize
            | Self::Cleanup
            | Self::DeviceFinalize
            | Self::Delete => ProgressUnit::Operations,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressUnit {
    #[default]
    Bytes,
    Operations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressState {
    Started,
    Advanced,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressEvent {
    #[serde(default, skip_serializing_if = "ProgressScope::is_stage")]
    pub scope: ProgressScope,
    pub stage: OperationStage,
    pub state: ProgressState,
    #[serde(default)]
    pub unit: ProgressUnit,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub completed: u64,
    pub total: Option<u64>,
}

/// Stage totals and individual work have independent lifecycles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressScope {
    #[default]
    Stage,
    Item {
        id: String,
    },
}

impl ProgressScope {
    const fn is_stage(&self) -> bool {
        matches!(self, Self::Stage)
    }
}

trait ProgressSink: Send + Sync + fmt::Debug {
    fn send(&self, event: ProgressEvent);
}

#[derive(Debug)]
struct ChannelProgressSink(std::sync::mpsc::Sender<ProgressEvent>);

impl ProgressSink for ChannelProgressSink {
    fn send(&self, event: ProgressEvent) {
        let _ = self.0.send(event);
    }
}

struct HandlerProgressSink<F>(F);

impl<F> fmt::Debug for HandlerProgressSink<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HandlerProgressSink")
    }
}

impl<F> ProgressSink for HandlerProgressSink<F>
where
    F: Fn(ProgressEvent) + Send + Sync,
{
    fn send(&self, event: ProgressEvent) {
        (self.0)(event);
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProgressReporter {
    sink: Option<Arc<dyn ProgressSink>>,
    cancellation: CancellationToken,
    device_state: DeviceStateTracker,
}

#[derive(Debug, Clone, Copy)]
struct ProgressAmount {
    unit: ProgressUnit,
    completed: u64,
    total: Option<u64>,
}

impl ProgressAmount {
    const fn new(unit: ProgressUnit, completed: u64, total: Option<u64>) -> Self {
        Self {
            unit,
            completed,
            total,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl ProgressReporter {
    /// Keep one work identity across stages, labels, and path changes.
    #[must_use]
    pub fn for_item(&self, id: impl Into<String>) -> Self {
        let parent = self.clone();
        let scope = ProgressScope::Item { id: id.into() };
        let mut child = Self::from_handler(move |mut event| {
            event.scope = scope.clone();
            parent.event(event);
        })
        .with_cancellation(self.cancellation_token());
        child.device_state = self.device_state.clone();
        child
    }

    #[must_use]
    pub fn channel() -> (Self, ProgressReceiver) {
        let (sender, receiver) = channel();
        let device_state = DeviceStateTracker::default();
        (
            Self {
                sink: Some(Arc::new(ChannelProgressSink(sender))),
                cancellation: CancellationToken::default(),
                device_state: device_state.clone(),
            },
            ProgressReceiver::new(receiver, device_state),
        )
    }

    #[must_use]
    pub fn from_handler(handler: impl Fn(ProgressEvent) + Send + Sync + 'static) -> Self {
        Self {
            sink: Some(Arc::new(HandlerProgressSink(handler))),
            cancellation: CancellationToken::default(),
            device_state: DeviceStateTracker::default(),
        }
    }

    /// Observe operation events without disconnecting cancellation or device state.
    #[must_use]
    pub fn observe(&self, handler: impl Fn(&ProgressEvent) + Send + Sync + 'static) -> Self {
        let downstream = self.clone();
        let mut reporter = Self::from_handler(move |event| {
            handler(&event);
            downstream.event(event);
        });
        reporter.cancellation = self.cancellation.clone();
        reporter.device_state = self.device_state.clone();
        reporter
    }

    pub fn device_state(&self, state: DeviceStateUpdate) {
        self.device_state.set(state);
    }

    #[must_use]
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub fn started(&self, stage: OperationStage, label: impl Into<String>, total: Option<u64>) {
        self.report(
            stage,
            ProgressState::Started,
            label,
            None,
            ProgressAmount::new(stage.progress_unit(), 0, total),
        );
    }

    pub fn started_operations(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Started,
            label,
            None,
            ProgressAmount::new(ProgressUnit::Operations, 0, total),
        );
    }

    pub fn started_with_path(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        path: impl Into<String>,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Started,
            label,
            Some(path.into()),
            ProgressAmount::new(stage.progress_unit(), 0, total),
        );
    }

    pub fn advanced(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        completed: u64,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Advanced,
            label,
            None,
            ProgressAmount::new(stage.progress_unit(), completed, total),
        );
    }

    pub fn advanced_with_path(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        path: impl Into<String>,
        completed: u64,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Advanced,
            label,
            Some(path.into()),
            ProgressAmount::new(stage.progress_unit(), completed, total),
        );
    }

    pub fn advanced_operations_with_path(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        path: impl Into<String>,
        completed: u64,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Advanced,
            label,
            Some(path.into()),
            ProgressAmount::new(ProgressUnit::Operations, completed, total),
        );
    }

    pub fn completed(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        completed: u64,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Completed,
            label,
            None,
            ProgressAmount::new(stage.progress_unit(), completed, total),
        );
    }

    pub fn completed_operations(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        completed: u64,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Completed,
            label,
            None,
            ProgressAmount::new(ProgressUnit::Operations, completed, total),
        );
    }

    pub fn completed_with_path(
        &self,
        stage: OperationStage,
        label: impl Into<String>,
        path: impl Into<String>,
        completed: u64,
        total: Option<u64>,
    ) {
        self.report(
            stage,
            ProgressState::Completed,
            label,
            Some(path.into()),
            ProgressAmount::new(stage.progress_unit(), completed, total),
        );
    }

    pub fn failed(&self, stage: OperationStage, label: impl Into<String>) {
        self.report(
            stage,
            ProgressState::Failed,
            label,
            None,
            ProgressAmount::new(stage.progress_unit(), 0, None),
        );
    }

    pub fn event(&self, event: ProgressEvent) {
        self.send(event);
    }

    fn send(&self, event: ProgressEvent) {
        if let Some(sink) = &self.sink {
            sink.send(event);
        }
    }

    fn report(
        &self,
        stage: OperationStage,
        progress_state: ProgressState,
        label: impl Into<String>,
        path: Option<String>,
        amount: ProgressAmount,
    ) {
        self.send(ProgressEvent {
            scope: ProgressScope::Stage,
            stage,
            state: progress_state,
            unit: amount.unit,
            label: label.into(),
            path,
            completed: amount.completed,
            total: amount.total,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_reporters_share_cancellation_but_not_identity() {
        let (parent, receiver) = ProgressReporter::channel();
        let first = parent.for_item("first");
        let second = parent.for_item("second");
        first.started(OperationStage::Download, "Downloading", Some(100));
        second.started(OperationStage::Verify, "Verifying", Some(200));
        parent.started(OperationStage::Backup, "Backing up", None);
        let scopes = receiver
            .try_iter()
            .map(|event| event.scope)
            .collect::<Vec<_>>();
        assert_eq!(
            scopes,
            [
                ProgressScope::Item {
                    id: "first".to_owned()
                },
                ProgressScope::Item {
                    id: "second".to_owned()
                },
                ProgressScope::Stage
            ]
        );
        first.cancellation_token().cancel();
        assert!(parent.is_cancelled());
        assert!(second.is_cancelled());
    }
}
