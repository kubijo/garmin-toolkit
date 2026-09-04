//! Durable application storage without exposing database connections.

use std::{path::Path, time::Duration};

use garmin_color::Color;
use garmin_model::{
    artifact::ArtifactId,
    identity::{
        AvatarArtifactId, Device, DisplayName, LanguagePreference, Profile, ProfilePreferences,
        Role, Source, ThemePreference, UnitSystem, User, UserId,
    },
};
use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use thiserror::Error;

mod activity;
mod avatar;
mod ingestion;
mod route;

pub use activity::{
    ActivityProjection, ActivityProjectionError, StoredActivity, StoredActivitySummary,
};
pub use avatar::{AvatarPersistenceError, ProfileAvatar, StoredProfileAvatar};
pub use ingestion::{Ingestion, IngestionError};
pub use route::{RouteImport, RoutePersistenceError, StoredRoutePlan, StoredRoutePlanSummary};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: u32 = 5;

/// An opened, migrated deployment database.
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    /// Opens or creates a database and applies embedded migrations.
    /// # Errors
    /// [`enum@Error`] when SQLite cannot open the file or a migration fails.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(BUSY_TIMEOUT);
        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Inserts a user or replaces its mutable application-owned data.
    /// # Errors
    /// [`enum@Error`] for database failures or an avatar not imported for this user.
    pub async fn save_user(&self, user: &User) -> Result<(), Error> {
        let id = user.id().to_string();
        let role = encode_role(user.role());
        let display_name = user.profile().display_name().as_str();
        let accent_rgba = user
            .profile()
            .accent()
            .map(|accent| i64::from(accent.as_u32()));
        let avatar_artifact_id = user
            .profile()
            .avatar_artifact_id()
            .map(|avatar| avatar.as_artifact_id().to_string());
        let preferences = user.profile().preferences();
        let unit_system = encode_unit_system(preferences.unit_system());
        let language = encode_language(preferences.language());
        let theme = encode_theme(preferences.theme());
        let stored = sqlx::query_file!(
            "queries/save-user.sql",
            id,
            role,
            display_name,
            accent_rgba,
            avatar_artifact_id,
            unit_system,
            language,
            theme,
            avatar_artifact_id,
            id,
            avatar_artifact_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        if stored.is_none() {
            return Err(Error::ProfileAvatarNotOwned);
        }
        Ok(())
    }

    /// Inserts a device or replaces its mutable label.
    /// # Errors
    /// [`enum@Error`] when SQLite rejects the write.
    pub async fn save_device(&self, device: &Device) -> Result<(), Error> {
        let id = device.id().to_string();
        let label = device.label().as_str();
        sqlx::query_file!("queries/save-device.sql", id, label)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Inserts a connector source or replaces its mutable context.
    ///
    /// Source ownership is immutable.
    /// # Errors
    /// [`enum@Error`] for database failures or an attempted ownership change.
    pub async fn save_source(&self, source: &Source) -> Result<(), Error> {
        let id = source.id().to_string();
        let owner_id = source.owner_id().to_string();
        let label = source.label().as_str();
        let device_id = source.device_id().map(|id| id.to_string());
        let stored = sqlx::query_file!("queries/save-source.sql", id, owner_id, label, device_id,)
            .fetch_optional(&self.pool)
            .await?;
        if stored.is_none() {
            return Err(Error::SourceOwnerConflict);
        }
        Ok(())
    }

    /// Loads one user by its portable application ID.
    /// # Errors
    /// [`enum@Error`] for database failures or invalid persisted values.
    pub async fn user(&self, id: UserId) -> Result<Option<User>, Error> {
        let persisted_id = id.to_string();
        let Some(row) = sqlx::query_file_as!(UserRow, "queries/user.sql", persisted_id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        decode_user(row).map(Some)
    }

    /// Lists all users in stable display order.
    /// # Errors
    /// [`enum@Error`] for database failures or invalid persisted values.
    pub async fn users(&self) -> Result<Vec<User>, Error> {
        sqlx::query_file_as!(UserRow, "queries/users.sql")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(decode_user)
            .collect()
    }

    /// Runs SQLite's full integrity check.
    /// # Errors
    /// [`enum@Error`] when SQLite reports corruption or cannot complete the check.
    pub async fn check_integrity(&self) -> Result<(), Error> {
        let reports = sqlx::query_file_scalar!("queries/integrity-check.sql")
            .fetch_all(&self.pool)
            .await?;
        if matches!(reports.as_slice(), [Some(report)] if report == "ok") {
            Ok(())
        } else {
            Err(Error::Integrity(
                reports
                    .into_iter()
                    .map(|report| report.unwrap_or_else(|| "NULL".to_owned()))
                    .collect(),
            ))
        }
    }

    /// Closes all pooled connections.
    pub async fn close(self) {
        self.pool.close().await;
    }
}

struct UserRow {
    id: String,
    role: String,
    display_name: String,
    accent_rgba: Option<i64>,
    avatar_artifact_id: Option<String>,
    unit_system: String,
    language: String,
    theme: String,
}

fn decode_user(row: UserRow) -> Result<User, Error> {
    let id = row
        .id
        .parse::<UserId>()
        .map_err(|error| Error::InvalidData {
            field: "user ID",
            reason: error.to_string(),
        })?;
    let role = decode_role(&row.role)?;
    let display_name =
        DisplayName::from_string(row.display_name).map_err(|_| Error::InvalidDisplayName)?;
    let accent = row
        .accent_rgba
        .map(|rgba| {
            u32::try_from(rgba)
                .map(Color::from_u32)
                .map_err(|error| Error::InvalidData {
                    field: "profile accent",
                    reason: error.to_string(),
                })
        })
        .transpose()?;
    let avatar_artifact_id = row
        .avatar_artifact_id
        .map(|id| {
            id.parse::<ArtifactId>()
                .map(AvatarArtifactId::from_artifact_id)
                .map_err(|error| Error::InvalidData {
                    field: "profile avatar artifact ID",
                    reason: error.to_string(),
                })
        })
        .transpose()?;
    Ok(User::from_parts(
        id,
        role,
        Profile::from_complete(
            display_name,
            accent,
            avatar_artifact_id,
            ProfilePreferences::from_parts(
                decode_unit_system(&row.unit_system)?,
                decode_language(&row.language)?,
                decode_theme(&row.theme)?,
            ),
        ),
    ))
}

const fn encode_role(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Member => "member",
    }
}

fn decode_role(role: &str) -> Result<Role, Error> {
    match role {
        "owner" => Ok(Role::Owner),
        "member" => Ok(Role::Member),
        _ => Err(Error::InvalidRole(role.to_owned())),
    }
}

const fn encode_unit_system(value: UnitSystem) -> &'static str {
    match value {
        UnitSystem::Metric => "metric",
        UnitSystem::Imperial => "imperial",
    }
}

fn decode_unit_system(value: &str) -> Result<UnitSystem, Error> {
    match value {
        "metric" => Ok(UnitSystem::Metric),
        "imperial" => Ok(UnitSystem::Imperial),
        _ => Err(invalid_preference("unit system", value)),
    }
}

const fn encode_language(value: LanguagePreference) -> &'static str {
    match value {
        LanguagePreference::English => "en",
        LanguagePreference::Czech => "cs",
    }
}

fn decode_language(value: &str) -> Result<LanguagePreference, Error> {
    match value {
        "en" => Ok(LanguagePreference::English),
        "cs" => Ok(LanguagePreference::Czech),
        _ => Err(invalid_preference("language", value)),
    }
}

const fn encode_theme(value: ThemePreference) -> &'static str {
    match value {
        ThemePreference::Auto => "auto",
        ThemePreference::Dark => "dark",
        ThemePreference::Light => "light",
    }
}

fn decode_theme(value: &str) -> Result<ThemePreference, Error> {
    match value {
        "auto" => Ok(ThemePreference::Auto),
        "dark" => Ok(ThemePreference::Dark),
        "light" => Ok(ThemePreference::Light),
        _ => Err(invalid_preference("theme", value)),
    }
}

fn invalid_preference(field: &'static str, value: &str) -> Error {
    Error::InvalidData {
        field,
        reason: value.to_owned(),
    }
}

/// A storage or persisted-data failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("SQLite operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Ingestion(#[from] IngestionError),
    #[error(transparent)]
    Avatar(#[from] AvatarPersistenceError),
    #[error(transparent)]
    Route(#[from] RoutePersistenceError),
    #[error("database integrity check failed: {0:?}")]
    Integrity(Vec<String>),
    #[error("persisted user role is invalid: {0}")]
    InvalidRole(String),
    #[error("persisted user display name is invalid")]
    InvalidDisplayName,
    #[error("persisted {field} is invalid: {reason}")]
    InvalidData {
        /// Persisted field or aggregate.
        field: &'static str,
        /// Validation failure.
        reason: String,
    },
    #[error("connector source ownership is immutable")]
    SourceOwnerConflict,
    #[error("profile avatar artifact is not imported for the user")]
    ProfileAvatarNotOwned,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use futures_lite::future::block_on;
    use tempfile::tempdir;

    use super::Storage;

    #[test]
    fn applies_required_sqlite_configuration() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let root = tempdir()?;
            let storage = Storage::open(root.path().join("configuration.sqlite3")).await?;

            let journal_mode = sqlx::query_file_scalar!("queries/pragma-journal-mode.sql")
                .fetch_one(&storage.pool)
                .await?;
            let synchronous = sqlx::query_file_scalar!("queries/pragma-synchronous.sql")
                .fetch_one(&storage.pool)
                .await?;
            let foreign_keys = sqlx::query_file_scalar!("queries/pragma-foreign-keys.sql")
                .fetch_one(&storage.pool)
                .await?;
            let busy_timeout = sqlx::query_file_scalar!("queries/pragma-busy-timeout.sql")
                .fetch_one(&storage.pool)
                .await?;

            assert_eq!(journal_mode.as_deref(), Some("wal"));
            assert_eq!(synchronous, Some(2));
            assert_eq!(foreign_keys, Some(1));
            assert_eq!(busy_timeout, Some(5_000));
            storage.close().await;
            Ok(())
        })
    }
}
