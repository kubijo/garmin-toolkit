use super::{DataError, Path, PathBuf, Storage};

pub const DEPLOYMENT_MODE: garmin_service_api::DeploymentMode =
    garmin_service_api::DeploymentMode::Production;
pub const PRODUCT_NAME: &str = "Garmin Toolkit";
pub const SOURCE_PROVIDER: crate::devices::mounted::Provider = crate::devices::mounted::Provider;

pub fn data_root(base: &Path) -> PathBuf {
    base.to_owned()
}

pub async fn open_storage(database: PathBuf) -> Result<Storage, DataError> {
    Storage::open(database).await.map_err(DataError::Storage)
}

#[cfg(test)]
mod tests {
    use super::data_root;
    use std::path::Path;

    #[test]
    fn uses_the_requested_storage_root() {
        assert_eq!(data_root(Path::new("root")), Path::new("root"));
    }
}
