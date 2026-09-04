//! Build-selected storage and identity composition.

use std::path::{Path, PathBuf};

use garmin_storage::Storage;
use thiserror::Error;

#[cfg(feature = "demo")]
mod selected {
    use super::{DataError, Path, PathBuf, Storage};

    pub const APP_ID: &str = "io.github.kubijo.GarminToolkit.Demo";
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

    pub const APP_ID: &str = "io.github.kubijo.GarminToolkit";
    pub const WINDOW_TITLE: &str = "Garmin Toolkit";
    const LEGACY_APP_ID: &str = "io.github.kubijo.nimrag";

    pub fn database_path(data_root: &Path) -> PathBuf {
        super::database_path_with_legacy(data_root, eframe::storage_dir(LEGACY_APP_ID).as_deref())
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

#[cfg(any(not(feature = "demo"), test))]
fn database_path_with_legacy(data_root: &Path, legacy_root: Option<&Path>) -> PathBuf {
    let database = data_root.join(DATABASE_FILE);
    if database.exists() {
        return database;
    }
    // A live SQLite database and its sidecars need locked, atomic migration.
    // Until implemented, retain the legacy location.
    legacy_root
        .map(|root| root.join(DATABASE_FILE))
        .filter(|path| path.is_file())
        .unwrap_or(database)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{DATABASE_FILE, database_path_with_legacy};

    #[test]
    fn existing_installation_keeps_its_database_until_explicit_migration() {
        let directory = tempdir().expect("test data directory");
        let current = directory.path().join("current");
        let legacy = directory.path().join("legacy");
        fs::create_dir_all(&current).expect("current data root");
        fs::create_dir_all(&legacy).expect("legacy data root");
        let legacy_database = legacy.join(DATABASE_FILE);
        fs::write(&legacy_database, []).expect("legacy database");

        assert_eq!(
            database_path_with_legacy(&current, Some(&legacy)),
            legacy_database
        );

        let current_database = current.join(DATABASE_FILE);
        fs::write(&current_database, []).expect("current database");
        assert_eq!(
            database_path_with_legacy(&current, Some(&legacy)),
            current_database
        );
    }
}
