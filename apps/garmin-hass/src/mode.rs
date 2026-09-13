use std::path::{Path, PathBuf};

use garmin_storage::Storage;
use thiserror::Error;

use crate::devices::SourceProvider as _;

#[cfg_attr(feature = "demo", path = "mode/demo.rs")]
#[cfg_attr(not(feature = "demo"), path = "mode/production.rs")]
mod selected;

pub(super) use selected::{DEPLOYMENT_MODE, PRODUCT_NAME, data_root, open_storage};

pub(super) fn device_source(
    data_root: &Path,
) -> Result<Box<dyn crate::devices::Source>, DataError> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|error| DataError::DeviceSource(Box::new(error)))?;
    selected::SOURCE_PROVIDER
        .open(data_root, runtime)
        .map_err(DataError::DeviceSource)
}

#[derive(Debug, Error)]
pub enum DataError {
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
    #[error("could not prepare deployment data: {0}")]
    Deployment(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
    #[error("could not prepare the device source: {0}")]
    DeviceSource(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DataError, device_source};

    #[test]
    fn device_source_reports_a_missing_runtime() {
        let Err(DataError::DeviceSource(source)) = device_source(Path::new("unused")) else {
            panic!("a missing runtime must retain its typed source error");
        };
        assert!(
            source
                .downcast_ref::<tokio::runtime::TryCurrentError>()
                .is_some()
        );
    }
}
