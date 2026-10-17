//! Verified portable snapshots.
//! Destinations must not already exist.

use std::io;
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tempfile::{NamedTempFile, TempDir};
use thiserror::Error;

use crate::{MIGRATOR, Storage};

mod archive;

const FORMAT: &str = "garmin-toolkit-snapshot";
const FORMAT_VERSION: u32 = 1;
const MAX_MANIFEST: u64 = 16 * 1024;
const WINDOW_LOG: u32 = 23;

/// Resource limits applied before publishing or restoring a snapshot.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub database_bytes: u64,
    pub compressed_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            database_bytes: 1024 * 1024 * 1024,
            compressed_bytes: 1024 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// Checks that limits fit the supported archive format and bounded arithmetic.
    /// # Errors
    /// Rejects zero or unsupported size limits.
    pub fn validate(self) -> Result<(), Error> {
        // Version 1 uses the POSIX base headers without extended size records.
        if self.database_bytes == 0
            || self.database_bytes > 0o77_777_777_777
            || self.compressed_bytes == 0
            || self.compressed_bytes > self.database_bytes + MAX_MANIFEST + 4096
        {
            return Err(Error::Invalid("invalid snapshot limits"));
        }
        Ok(())
    }

    fn archive_bytes(self) -> u64 {
        self.database_bytes + MAX_MANIFEST + 4096
    }
}

/// Versioned archive metadata, verified against the staged database.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub app_version: String,
    pub created_at: i64,
    pub schema: Vec<Migration>,
    pub database_bytes: u64,
    pub database_sha256: String,
}

/// Embedded migration identity, including its content checksum.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    pub version: i64,
    pub checksum: Vec<u8>,
}

fn schema() -> Vec<Migration> {
    MIGRATOR
        .iter()
        .map(|migration| Migration {
            version: migration.version,
            checksum: migration.checksum.to_vec(),
        })
        .collect()
}

impl Manifest {
    fn validate(&self, limits: Limits) -> Result<(), Error> {
        if self.format != FORMAT || self.version != FORMAT_VERSION {
            return Err(Error::Invalid("unsupported snapshot format"));
        }
        if self.schema.is_empty() || !schema().starts_with(&self.schema) {
            return Err(Error::Invalid("incompatible snapshot schema"));
        }
        if semver::Version::parse(&self.app_version).is_err() || self.created_at < 0 {
            return Err(Error::Invalid("invalid snapshot version or timestamp"));
        }
        if self.database_bytes == 0 || self.database_bytes > limits.database_bytes {
            return Err(Error::Invalid("snapshot database exceeds its size limit"));
        }
        Ok(())
    }
}

impl Storage {
    /// Writes and verifies a portable snapshot, then atomically publishes it without replacement.
    /// Exclusive borrowing prevents concurrent writes through this storage handle.
    /// # Errors
    /// Fails on invalid data, limits, I/O, or an existing destination.
    pub async fn snapshot(
        &mut self,
        destination: &Path,
        limits: Limits,
    ) -> Result<Manifest, Error> {
        limits.validate()?;
        let parent = parent(destination)?;
        let staging = tempfile::tempdir_in(parent)?;
        let database = staging.path().join("storage.sqlite3");
        let size = sqlx::query_file_scalar!("queries/snapshot-size.sql")
            .fetch_one(&self.pool)
            .await?
            .ok_or(Error::Invalid("SQLite did not report the database size"))?;
        if u64::try_from(size).map_or(true, |size| size > limits.database_bytes) {
            return Err(Error::Invalid("database exceeds snapshot size limit"));
        }
        let path = database
            .to_str()
            .ok_or(Error::Invalid("snapshot staging path is not UTF-8"))?;
        sqlx::query_file!("queries/snapshot-vacuum.sql", path)
            .execute(&self.pool)
            .await?;
        let mut database_file = File::open(&database)?;
        database_file.sync_all()?;
        let manifest = Manifest {
            format: FORMAT.to_owned(),
            version: FORMAT_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            created_at: time::OffsetDateTime::now_utc().unix_timestamp(),
            schema: schema(),
            database_bytes: database_file.metadata()?.len(),
            database_sha256: digest(&mut database_file)?,
        };
        manifest.validate(limits)?;
        let mut output = NamedTempFile::new_in(parent)?;
        archive::encode(&mut output, &manifest, &database)?;
        output.as_file().sync_all()?;
        output.rewind()?;
        // Read back the actual encoded bytes, including the compression checksum.
        let verified = PreparedRestore::read(&mut output, parent, limits).await?;
        if verified.manifest != manifest {
            return Err(Error::Invalid("snapshot changed during verification"));
        }
        output
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        sync_directory(parent)?;
        Ok(manifest)
    }
}

/// A verified database in a private staging directory.
/// Dropping it cancels the restore.
/// Publish into a new deployment root;
/// retain the old root for rollback.
pub struct PreparedRestore {
    database: NamedTempFile,
    manifest: Manifest,
    _staging: TempDir,
}

impl PreparedRestore {
    /// Stages bounded input and validates it without opening or changing a live store.
    /// `staging_parent` must be on the destination filesystem.
    /// # Errors
    /// Rejects corrupt, incompatible, oversized, or structurally invalid snapshots.
    pub async fn read(
        input: impl Read,
        staging_parent: &Path,
        limits: Limits,
    ) -> Result<Self, Error> {
        limits.validate()?;
        let staging = tempfile::tempdir_in(staging_parent)?;
        let (database, manifest) = archive::decode(input, staging.path(), limits)?;
        verify_database(database.path(), &manifest).await?;
        Ok(Self {
            database,
            manifest,
            _staging: staging,
        })
    }

    #[must_use]
    pub const fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Atomically installs the verified database at a previously absent path.
    /// Existing stores are never replaced; switch roots only after this succeeds.
    /// # Errors
    /// Fails on I/O, an existing destination, or a cross-filesystem destination.
    pub fn publish(self, destination: &Path) -> Result<Manifest, Error> {
        let parent = parent(destination)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = destination.as_os_str().to_owned();
            sidecar.push(suffix);
            match std::fs::symlink_metadata(sidecar) {
                Ok(_) => return Err(Error::Invalid("restore destination has SQLite sidecars")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.database.as_file().sync_all()?;
        self.database
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        sync_directory(parent)?;
        Ok(self.manifest)
    }
}

async fn verify_database(path: &Path, manifest: &Manifest) -> Result<(), Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .pragma("trusted_schema", "OFF");
    let mut connection = SqliteConnection::connect_with(&options).await?;
    let result = verify_connection(&mut connection, manifest).await;
    connection.close().await?;
    result
}

async fn verify_connection(
    connection: &mut SqliteConnection,
    manifest: &Manifest,
) -> Result<(), Error> {
    let reports = sqlx::query_file_scalar!("queries/integrity-check.sql")
        .fetch_all(&mut *connection)
        .await?;
    if !matches!(reports.as_slice(), [Some(report)] if report == "ok") {
        return Err(Error::Invalid("snapshot SQLite integrity check failed"));
    }
    if sqlx::query_file!("queries/snapshot-foreign-keys.sql")
        .fetch_optional(&mut *connection)
        .await?
        .is_some()
    {
        return Err(Error::Invalid("snapshot has broken foreign keys"));
    }
    // This query intentionally runs before migrations: an untrusted store must
    // match a known embedded migration prefix without being upgraded or modified.
    let migrations = sqlx::query_file!("queries/snapshot-migrations.sql")
        .fetch_all(connection)
        .await?;
    let actual = migrations
        .iter()
        .map(|migration| {
            Ok(Migration {
                version: migration
                    .version
                    .ok_or(Error::Invalid("snapshot migration version is NULL"))?,
                checksum: migration.checksum.clone(),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if migrations.iter().any(|migration| !migration.success) || actual != manifest.schema {
        return Err(Error::Invalid(
            "snapshot database migrations do not match its manifest",
        ));
    }
    Ok(())
}

fn digest(file: &mut File) -> Result<String, Error> {
    file.rewind()?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    file.rewind()?;
    Ok(hex::encode(digest.finalize()))
}

fn parent(path: &Path) -> Result<&Path, Error> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(Error::Invalid(
            "snapshot destination requires an explicit parent directory",
        ))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Snapshot validation or persistence failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("snapshot I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("snapshot database failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("snapshot manifest failed: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(&'static str),
}
