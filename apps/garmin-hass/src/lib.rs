//! Home Assistant host composition.

#![expect(
    clippy::multiple_crate_versions,
    reason = "FIT fixtures and SQLx currently require incompatible transitive releases"
)]

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use garmin_storage::Storage;
use thiserror::Error;

mod devices;
mod mode;
mod server;

pub use mode::DataError;

const DATA_BASE: &str = "/data";
const DATABASE_FILE: &str = "storage.sqlite3";
const DATA_BASE_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_DATA_BASE";

/// Prepares the deployment database.
///
/// Production opens existing user data or creates an empty database. The `demo` build seeds
/// deterministic examples through the production importer; target composition isolates its root.
/// # Errors
/// [`enum@Error`] when the data directory, database, or demo corpus cannot be prepared.
pub async fn prepare_storage(data_root: impl AsRef<Path>) -> Result<Storage, Error> {
    let data_root = data_root.as_ref();
    fs::create_dir_all(data_root)?;
    Ok(mode::open_storage(data_root.join(DATABASE_FILE)).await?)
}

/// Runs the device host and browser service until shutdown.
/// # Errors
/// [`enum@Error`] when persistent state cannot be prepared.
pub async fn run() -> Result<(), Error> {
    let data_root = deployment_data_root();
    let storage = prepare_storage(data_root).await?;
    let devices = devices::Host::new(mode::device_source());
    devices.start();
    let result = server::serve(devices).await;
    storage.close().await;
    result?;
    Ok(())
}

fn deployment_data_root() -> PathBuf {
    let data_base = configured_data_base(env::var_os(DATA_BASE_ENVIRONMENT).map(PathBuf::from));
    mode::data_root(&data_base)
}

fn configured_data_base(configured: Option<PathBuf>) -> PathBuf {
    configured.unwrap_or_else(|| DATA_BASE.into())
}

/// Home Assistant startup failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("could not prepare the Home Assistant data directory: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Data(#[from] DataError),
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use futures_lite::future::block_on;
    use tempfile::tempdir;

    use super::{DATA_BASE, configured_data_base, prepare_storage};

    #[test]
    fn configured_data_base_overrides_the_default() {
        assert_eq!(
            configured_data_base(Some("current".into())),
            Path::new("current")
        );
        assert_eq!(configured_data_base(None), Path::new(DATA_BASE));
    }

    #[test]
    fn prepares_a_migrated_database() {
        block_on(async {
            let directory = tempdir().expect("the test data directory should be created");
            let storage = prepare_storage(directory.path())
                .await
                .expect("the target database should be prepared");

            storage.close().await;
        });
    }
}
