use std::sync::{Arc, Mutex, mpsc};

use garmin_model::device::DeviceStateSnapshot;

use crate::ProgressEvent;

/// The latest capacity snapshot or refresh error.
pub type DeviceStateUpdate = Result<DeviceStateSnapshot, String>;

#[derive(Debug, Clone, Default)]
pub(super) struct DeviceStateTracker(Arc<Mutex<Option<DeviceStateUpdate>>>);

impl DeviceStateTracker {
    pub fn set(&self, state: DeviceStateUpdate) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state);
    }

    fn latest(&self) -> Option<DeviceStateUpdate> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Operation history plus latest device state.
pub struct ProgressReceiver {
    operations: mpsc::Receiver<ProgressEvent>,
    device_state: DeviceStateTracker,
}

impl ProgressReceiver {
    pub(super) fn new(
        operations: mpsc::Receiver<ProgressEvent>,
        device_state: DeviceStateTracker,
    ) -> Self {
        Self {
            operations,
            device_state,
        }
    }

    #[must_use]
    pub fn try_iter(&self) -> mpsc::TryIter<'_, ProgressEvent> {
        self.operations.try_iter()
    }

    #[must_use]
    pub fn device_state(&self) -> Option<DeviceStateUpdate> {
        self.device_state.latest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationStage, ProgressReporter};

    #[test]
    fn observers_and_items_share_latest_state_and_invalidate_stale_snapshots() {
        let (reporter, receiver) = ProgressReporter::channel();
        let observed = reporter.observe(|_| {});
        observed.device_state(Ok(DeviceStateSnapshot::default()));
        assert!(receiver.device_state().unwrap().is_ok());
        let item = observed.for_item("upload");
        item.device_state(Err("Device disconnected".to_owned()));
        assert_eq!(
            receiver.device_state(),
            Some(Err("Device disconnected".to_owned()))
        );
        item.completed(OperationStage::Upload, "Uploaded", 1, Some(1));
        assert_eq!(receiver.try_iter().count(), 1);
        observed.cancellation_token().cancel();
        assert!(reporter.is_cancelled());
    }
}
