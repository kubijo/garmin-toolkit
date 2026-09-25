//! Build-selected attachment discovery and file transport.

use std::path::Path;

#[cfg_attr(feature = "demo", path = "device_backend/demo.rs")]
#[cfg_attr(not(feature = "demo"), path = "device_backend/production.rs")]
mod selected;

pub use selected::{Candidate, Device, Platform};

pub fn open(data_root: &Path) -> Result<Platform, Error> {
    selected::open(data_root)
}

#[must_use]
pub fn transport(candidate: &Candidate) -> Device {
    selected::transport(candidate)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(feature = "demo")]
    #[error(transparent)]
    Demo(#[from] garmin_fixtures::device::DeviceError),
}
