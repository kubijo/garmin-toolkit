use super::{Arc, DataError, Deployment, Path, PathBuf};

pub const DEPLOYMENT_MODE: garmin_service_api::DeploymentMode =
    garmin_service_api::DeploymentMode::Production;
pub const PRODUCT_NAME: &str = "Garmin Toolkit";
pub const SOURCE_PROVIDER: Provider = Provider;

pub struct Provider;

impl crate::devices::SourceProvider for Provider {
    fn open(
        self,
        data_root: &Path,
        runtime: tokio::runtime::Handle,
    ) -> Result<Box<dyn crate::devices::Source>, crate::devices::SourceOpenError> {
        match std::env::var("GARMIN_TOOLKIT_HASS_DEVICE_SOURCE").as_deref() {
            Ok("raw-mtp") => Ok(Box::new(crate::devices::mounted::DeviceSource::<
                crate::devices::raw::RawAdapter,
            >::new())),
            Ok("mounted") | Err(std::env::VarError::NotPresent) => {
                crate::devices::SourceProvider::open(
                    crate::devices::mounted::Provider,
                    data_root,
                    runtime,
                )
            }
            other => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid GARMIN_TOOLKIT_HASS_DEVICE_SOURCE: {other:?}"),
            )
            .into()),
        }
    }
}

pub fn data_root(base: &Path) -> PathBuf {
    base.to_owned()
}

pub async fn open(root: &Path) -> Result<Arc<Deployment>, DataError> {
    Deployment::open(root).await.map_err(DataError::Deployment)
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
