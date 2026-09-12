use std::path::{Path, PathBuf};

use garmin_storage::Storage;
use thiserror::Error;

#[cfg(feature = "demo")]
mod selected {
    use super::{DataError, Path, PathBuf, Storage};

    pub const DEPLOYMENT_MODE: garmin_service_api::DeploymentMode =
        garmin_service_api::DeploymentMode::Demo;
    pub const PRODUCT_NAME: &str = "Garmin Toolkit Demo";

    pub fn device_source() -> Box<dyn crate::devices::Source> {
        Box::new(crate::devices::DemoSource::new())
    }

    pub fn data_root(base: &Path) -> PathBuf {
        base.join("demo")
    }

    pub async fn open_storage(database: PathBuf) -> Result<Storage, DataError> {
        Ok(garmin_fixtures::recreate(database).await?)
    }
}

#[cfg(not(feature = "demo"))]
mod selected {
    use super::{DataError, Path, PathBuf, Storage};

    pub const DEPLOYMENT_MODE: garmin_service_api::DeploymentMode =
        garmin_service_api::DeploymentMode::Production;
    pub const PRODUCT_NAME: &str = "Garmin Toolkit";

    pub fn device_source() -> Box<dyn crate::devices::Source> {
        Box::new(crate::devices::MountedSource::new())
    }

    pub fn data_root(base: &Path) -> PathBuf {
        base.to_owned()
    }

    pub async fn open_storage(database: PathBuf) -> Result<Storage, DataError> {
        Ok(Storage::open(database).await?)
    }
}

pub(super) use selected::{DEPLOYMENT_MODE, PRODUCT_NAME, data_root, device_source, open_storage};

#[derive(Debug, Error)]
pub enum DataError {
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
    #[cfg(feature = "demo")]
    #[error(transparent)]
    Fixtures(#[from] garmin_fixtures::SeedError),
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::data_root;

    #[test]
    fn mode_owns_its_storage_root() {
        #[cfg(feature = "demo")]
        assert_eq!(data_root(Path::new("root")), Path::new("root/demo"));
        #[cfg(not(feature = "demo"))]
        assert_eq!(data_root(Path::new("root")), Path::new("root"));
    }
}
