//! Direct USB MTP adapter for the Home Assistant app container.

use garmin_device::{attachments::Candidate as _, storage::MtpStorageDevice};

use super::mounted::Adapter;

pub(crate) struct RawAdapter;

impl Adapter for RawAdapter {
    type Backend = garmin_device::RawMtpMonitor;
    type Candidate = garmin_device::RawMtpCandidate;
    type Device = MtpStorageDevice;

    fn backend() -> Self::Backend {
        garmin_device::RawMtpMonitor::new()
    }

    fn inspect(
        _runtime: &tokio::runtime::Handle,
        candidate: &Self::Candidate,
    ) -> Result<garmin_device::attachments::Metadata, String> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?
            .block_on(async {
                let device = MtpStorageDevice::discover(candidate.location_id)
                    .await
                    .map_err(|error| error.to_string())?;
                let metadata = candidate.inspect_async().await;
                Ok(garmin_services::devices::inspect_attachment(
                    &device,
                    candidate.name(),
                    metadata,
                )
                .await)
            })
    }

    fn location(candidate: &Self::Candidate) -> String {
        format!("{:016x}", candidate.location_id)
    }

    fn connector(
        location: String,
    ) -> Result<std::sync::Arc<dyn garmin_services::maps::device::Connector>, String> {
        let location = u64::from_str_radix(&location, 16).map_err(|error| error.to_string())?;
        Ok(std::sync::Arc::new(
            garmin_services::maps::device::RawMtpConnector(location),
        ))
    }

    fn device(
        runtime: &tokio::runtime::Runtime,
        candidate: &Self::Candidate,
    ) -> Result<Self::Device, String> {
        runtime
            .block_on(MtpStorageDevice::discover(candidate.location_id))
            .map_err(|error| error.to_string())
    }
}
