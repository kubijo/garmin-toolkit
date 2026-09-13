use std::path::Path;

pub type Candidate = garmin_device::MountedMtpCandidate;
pub type Device = garmin_device::MountedMtpDevice;
pub type Platform = garmin_device::MountedMtpMonitor;

#[expect(
    clippy::unnecessary_wraps,
    reason = "all build-selected backends share the demo backend's fallible constructor"
)]
pub fn open(_data_root: &Path) -> Result<Platform, super::Error> {
    Ok(Platform::new())
}

#[must_use]
pub fn transport(candidate: &Candidate) -> Device {
    Device::new(candidate.mount_id.clone())
}
