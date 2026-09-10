//! Build-selected storage and identity composition.

use std::path::{Path, PathBuf};

use garmin_storage::Storage;
use thiserror::Error;

#[cfg(feature = "demo")]
mod selected {
    use super::{DataError, Path, PathBuf, Storage};

    pub const APP_ID: &str = "io.kubijo.GarminToolkit.Demo";
    pub const WINDOW_TITLE: &str = "Garmin Toolkit Demo";

    pub fn database_path(data_root: &Path) -> PathBuf {
        data_root.join(super::DATABASE_FILE)
    }

    pub async fn open_storage(database: PathBuf) -> Result<Storage, DataError> {
        Ok(garmin_fixtures::recreate(database).await?)
    }
}

#[cfg(not(feature = "demo"))]
mod selected {
    use super::{DataError, Path, PathBuf, Storage};

    pub const APP_ID: &str = "io.kubijo.GarminToolkit";
    pub const WINDOW_TITLE: &str = "Garmin Toolkit";
    pub fn database_path(data_root: &Path) -> PathBuf {
        data_root.join(super::DATABASE_FILE)
    }

    pub async fn open_storage(database: PathBuf) -> Result<Storage, DataError> {
        Ok(Storage::open(database).await?)
    }
}

pub(super) use selected::{APP_ID, WINDOW_TITLE, database_path, open_storage};

const DATABASE_FILE: &str = "storage.sqlite3";

#[derive(Debug, Error)]
pub enum DataError {
    #[error(transparent)]
    Storage(#[from] garmin_storage::Error),
    #[cfg(feature = "demo")]
    #[error(transparent)]
    Fixtures(#[from] garmin_fixtures::SeedError),
}
