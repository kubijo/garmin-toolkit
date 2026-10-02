use super::{Arc, DataError, Deployment, Path, PathBuf};

pub const DEPLOYMENT_MODE: garmin_service_api::DeploymentMode =
    garmin_service_api::DeploymentMode::Demo;
pub const PRODUCT_NAME: &str = "Garmin Toolkit Demo";
pub const SOURCE_PROVIDER: crate::devices::demo::Provider = crate::devices::demo::Provider;

pub fn data_root(base: &Path) -> PathBuf {
    base.join("demo")
}

pub async fn open(root: &Path) -> Result<Arc<Deployment>, DataError> {
    Deployment::open_initialized(root, async |storage| {
        garmin_fixtures::seed(storage)
            .await
            .map(|_| ())
            .map_err(Into::into)
    })
    .await
    .map_err(DataError::Deployment)
}

#[cfg(test)]
mod tests {
    use super::data_root;
    use std::path::Path;

    #[test]
    fn owns_a_separate_storage_root() {
        assert_eq!(data_root(Path::new("root")), Path::new("root/demo"));
    }
}
