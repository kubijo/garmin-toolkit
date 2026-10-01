//! Per-device exclusion shared by map mutations and browser writes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Default)]
pub struct MutationLocks(Mutex<HashMap<String, Arc<AsyncMutex<()>>>>);

impl MutationLocks {
    /// Acquire mutation ownership without queuing a stale user decision.
    ///
    /// # Errors
    /// Another operation still owns the selected device.
    pub fn acquire(&self, device: &str) -> Result<OwnedMutexGuard<()>, &'static str> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(device.to_owned())
            .or_default()
            .clone()
            .try_lock_owned()
            .map_err(|_| "another operation is modifying this device")
    }
}
