//! Home Assistant host composition.

#![expect(
    clippy::multiple_crate_versions,
    reason = "FIT fixtures and SQLx currently require incompatible transitive releases"
)]

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use garmin_services::Application;
use garmin_storage::Storage;
use thiserror::Error;
use tracing_subscriber::prelude::*;

mod control;
mod devices;
mod mode;
mod server;

pub use mode::DataError;

const DATA_BASE: &str = "/data";
const DATABASE_FILE: &str = "storage.sqlite3";
const DATA_BASE_ENVIRONMENT: &str = "GARMIN_TOOLKIT_HASS_DATA_BASE";

/// Startup-only browser controls, embedded in the uncached entry point.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent startup feature switches"
)]
pub struct BrowserOptions {
    /// Record map upload lifecycle telemetry.
    pub map_upload_telemetry: bool,
    /// Enable isolated worker map rendering.
    pub map_render_worker: bool,
    /// Expose named semantic interaction scenarios in demo builds only.
    pub ui_automation: bool,
    /// Expose authenticated automation routes on the existing HTTP listener (demo only).
    pub control_server: bool,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            map_upload_telemetry: true,
            map_render_worker: true,
            ui_automation: false,
            control_server: false,
        }
    }
}

impl BrowserOptions {
    fn validate(self) -> std::io::Result<()> {
        if (self.ui_automation || self.control_server) && !cfg!(feature = "demo") {
            return Err(std::io::Error::other(
                "automation and control server require a demo build",
            ));
        }
        Ok(())
    }
}

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
pub async fn run(browser: BrowserOptions) -> Result<(), Error> {
    browser.validate()?;
    let data_root = deployment_data_root();
    let logs = garmin_logging::Store::open(data_root.join("logs"), "hass")?;
    logs.install_global();
    let _ = tracing_subscriber::registry()
        .with(logs)
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,garmin=info".into()),
        )
        .with(
            tracing_logfmt::builder()
                .layer()
                .with_writer(std::io::stderr),
        )
        .try_init();
    tracing::info!("HASS starting");
    let storage = prepare_storage(&data_root).await?;
    let devices = devices::Host::new(mode::device_source(&data_root)?, Application::new(storage));
    let map_tiles = garmin_map_tiles::Service::new(data_root.join("cache/activity-map"))?;
    devices.start();
    let result = server::serve(devices, map_tiles, browser).await;
    if let Some(logs) = garmin_logging::Store::global() {
        logs.flush();
    }
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
    #[error(transparent)]
    MapTiles(#[from] garmin_map_tiles::Error),
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use futures_lite::future::block_on;
    use tempfile::tempdir;

    use super::{DATA_BASE, configured_data_base, prepare_storage};

    #[test]
    fn worker_map_is_default_in_both_deployment_modes() {
        let defaults = super::BrowserOptions::default();
        assert!(defaults.validate().is_ok());
        assert!(defaults.map_upload_telemetry);
        assert!(defaults.map_render_worker);
        let rollback = super::BrowserOptions {
            map_render_worker: false,
            ..defaults
        };
        assert!(rollback.validate().is_ok());
    }

    #[test]
    fn configured_data_base_overrides_the_default() {
        assert_eq!(
            configured_data_base(Some("current".into())),
            Path::new("current")
        );
        assert_eq!(configured_data_base(None), Path::new(DATA_BASE));
    }

    #[test]
    fn automation_is_independently_opt_in_and_demo_only() {
        let defaults = super::BrowserOptions::default();
        assert!(!defaults.ui_automation);
        assert!(!defaults.control_server);
        let control = super::BrowserOptions {
            control_server: true,
            ..defaults
        };
        assert_eq!(control.validate().is_ok(), cfg!(feature = "demo"));
        let automation = super::BrowserOptions {
            ui_automation: true,
            ..defaults
        };
        assert!(automation.map_render_worker);
        assert_eq!(automation.validate().is_ok(), cfg!(feature = "demo"));
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
