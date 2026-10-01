//! Build-selected attachment discovery and file transport.

use std::path::Path;

#[cfg_attr(feature = "demo", path = "device_backend/demo.rs")]
#[cfg_attr(not(feature = "demo"), path = "device_backend/production.rs")]
mod selected;

pub use selected::map_connector;
pub use selected::{Candidate, Device, Platform};

pub fn open(data_root: &Path) -> Result<Platform, Error> {
    selected::open(data_root)
}

#[must_use]
pub fn transport(candidate: &Candidate) -> Device {
    selected::transport(candidate)
}

pub fn inspect(candidate: &Candidate) -> Result<garmin_device::attachments::Metadata, String> {
    use garmin_device::attachments::Candidate as _;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    Ok(
        runtime.block_on(garmin_services::devices::inspect_attachment(
            &transport(candidate),
            candidate.name(),
            candidate.inspect(),
        )),
    )
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(feature = "demo")]
    #[error(transparent)]
    Demo(#[from] garmin_fixtures::device::DeviceError),
}
