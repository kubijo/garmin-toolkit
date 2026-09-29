//! Build-selected storage and identity composition.

use garmin_services::deployment::{Deployment, Error};
use std::{path::Path, sync::Arc};

#[cfg(feature = "demo")]
pub const APP_ID: &str = "io.kubijo.GarminToolkit.Demo";
#[cfg(not(feature = "demo"))]
pub const APP_ID: &str = "io.kubijo.GarminToolkit";
#[cfg(feature = "demo")]
pub const WINDOW_TITLE: &str = "Garmin Toolkit Demo";
#[cfg(not(feature = "demo"))]
pub const WINDOW_TITLE: &str = "Garmin Toolkit";

pub async fn open(data_root: &Path) -> Result<Arc<Deployment>, Error> {
    #[cfg(feature = "demo")]
    return Deployment::open_initialized(data_root, async |storage| {
        garmin_fixtures::seed(storage).await?;
        Ok(())
    })
    .await;
    #[cfg(not(feature = "demo"))]
    Deployment::open(data_root).await
}

#[cfg(all(test, feature = "demo"))]
mod tests {
    #[test]
    fn demo_restart_preserves_profiles_instead_of_recreating_storage()
    -> Result<(), Box<dyn std::error::Error>> {
        futures_lite::future::block_on(async {
            let root = tempfile::tempdir()?;
            let deployment = super::open(root.path()).await?;
            let epoch = deployment.epoch();
            let before = deployment.application(epoch).await?.profiles().await?.len();
            deployment
                .application(epoch)
                .await?
                .create_profile("Persisted demo profile".parse()?)
                .await?;
            deployment.close().await;
            drop(deployment);
            let reopened = super::open(root.path()).await?;
            assert_eq!(
                reopened
                    .application(reopened.epoch())
                    .await?
                    .profiles()
                    .await?
                    .len(),
                before + 1
            );
            reopened.close().await;
            Ok(())
        })
    }
}
