use std::collections::BTreeMap;

use garmin_model::{
    activity::Distance,
    garmin_connect::{AccountFacts, CacheStatus, CachedActivity},
    identity::{SourceId, UserId},
};

use crate::{Error, Storage};

/// Active account snapshot bound to the profile's current credential.
pub struct CachedAccount {
    pub source: SourceId,
    pub generation: String,
    pub facts: AccountFacts,
}

/// One consistent published cache read, independent of an in-progress refresh.
pub struct CacheSnapshot {
    pub activities: Vec<CachedActivity>,
    pub coverage: CacheStatus,
    pub ascent: BTreeMap<i64, Distance>,
}

fn invalid(reason: impl std::fmt::Display) -> Error {
    Error::InvalidData {
        field: "Garmin Connect cache",
        reason: reason.to_string(),
    }
}

impl Storage {
    /// Reads the active generation, import markers and coverage in a single SQLite snapshot.
    /// # Errors
    /// Returns database or invalid cached-value failures.
    pub async fn connect_cache_snapshot(
        &self,
        user: UserId,
        fingerprint: &str,
    ) -> Result<Option<CacheSnapshot>, Error> {
        self.read_connect_cache_snapshot(user, fingerprint, false)
            .await
    }

    /// Read the published cache and its unambiguous imported FIT elevation measurements.
    ///
    /// # Errors
    /// Returns database or invalid cached-value failures.
    pub async fn connect_trends_snapshot(
        &self,
        user: UserId,
        fingerprint: &str,
    ) -> Result<Option<CacheSnapshot>, Error> {
        self.read_connect_cache_snapshot(user, fingerprint, true)
            .await
    }

    async fn read_connect_cache_snapshot(
        &self,
        user: UserId,
        fingerprint: &str,
        include_ascent: bool,
    ) -> Result<Option<CacheSnapshot>, Error> {
        let user = user.to_string();
        let mut transaction = self.pool.begin().await?;
        let Some(account) =
            sqlx::query_file!("queries/connect-cache-account.sql", user, fingerprint)
                .fetch_optional(&mut *transaction)
                .await?
        else {
            return Ok(None);
        };
        let source = account.source_id;
        let generation = account.generation;
        let summaries = sqlx::query_file_scalar!(
            "queries/connect-cache-summaries.sql",
            user,
            source,
            generation
        )
        .fetch_all(&mut *transaction)
        .await?;
        let imported =
            sqlx::query_file_scalar!("queries/imported-source-identities.sql", user, source)
                .fetch_all(&mut *transaction)
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
        let counts =
            sqlx::query_file!("queries/connect-cache-status.sql", user, source, generation)
                .fetch_one(&mut *transaction)
                .await?;
        let ascent = if include_ascent {
            imported_ascent(&mut transaction, &user, &source).await?
        } else {
            BTreeMap::new()
        };
        transaction.commit().await?;
        let mut activities = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let mut summary: CachedActivity = serde_json::from_str(&summary).map_err(invalid)?;
            summary.activity.imported = imported.contains(&summary.activity.id.to_string());
            activities.push(summary);
        }
        Ok(Some(CacheSnapshot {
            activities,
            ascent,
            coverage: CacheStatus {
                total: usize::try_from(counts.total).map_err(invalid)?,
                downloaded: usize::try_from(counts.downloaded).map_err(invalid)?,
                unavailable: usize::try_from(counts.unavailable).map_err(invalid)?,
            },
        }))
    }
    /// Checks download coverage without reading the cached archive.
    /// # Errors
    /// Returns database failures.
    pub async fn connect_has_original(
        &self,
        user: UserId,
        source: SourceId,
        id: i64,
    ) -> Result<bool, Error> {
        let user = user.to_string();
        let source = source.to_string();
        Ok(
            sqlx::query_file_scalar!("queries/connect-cache-has-original.sql", user, source, id)
                .fetch_one(&self.pool)
                .await?
                != 0,
        )
    }

    /// Reads a cache only when it belongs to this profile and credential.
    /// # Errors
    /// Returns database or cached-value failures.
    pub async fn connect_cache_account(
        &self,
        user: UserId,
        credential_hash: &str,
    ) -> Result<Option<CachedAccount>, Error> {
        let user = user.to_string();
        sqlx::query_file!("queries/connect-cache-account.sql", user, credential_hash)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| {
                Ok(CachedAccount {
                    source: row.source_id.parse().map_err(invalid)?,
                    generation: row.generation,
                    facts: serde_json::from_str(&row.facts).map_err(invalid)?,
                })
            })
            .transpose()
    }

    /// Stages one page without changing the currently searchable snapshot.
    /// # Errors
    /// Returns database or serialization failures.
    pub async fn stage_connect_summaries(
        &self,
        user: UserId,
        source: SourceId,
        generation: &str,
        activities: &[CachedActivity],
    ) -> Result<(), Error> {
        let user = user.to_string();
        let source = source.to_string();
        let mut transaction = self.pool.begin().await?;
        for activity in activities {
            let id = activity.activity.id;
            let summary = serde_json::to_string(activity).map_err(invalid)?;
            sqlx::query_file!(
                "queries/connect-cache-save-summary.sql",
                user,
                source,
                generation,
                id,
                summary
            )
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Discards one abandoned staging generation while preserving published snapshots.
    /// # Errors
    /// Returns database failures.
    pub async fn discard_connect_summaries(
        &self,
        user: UserId,
        source: SourceId,
        generation: &str,
    ) -> Result<(), Error> {
        let user = user.to_string();
        let source = source.to_string();
        sqlx::query_file!(
            "queries/connect-cache-discard-summaries.sql",
            user,
            source,
            generation
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically publishes a complete metadata snapshot and removes stale cache entries.
    /// # Errors
    /// Returns database or serialization failures.
    pub async fn publish_connect_cache(
        &self,
        user: UserId,
        credential_hash: &str,
        account: &CachedAccount,
    ) -> Result<(), Error> {
        let user = user.to_string();
        let source = account.source.to_string();
        let generation = &account.generation;
        let facts = serde_json::to_string(&account.facts).map_err(invalid)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query_file!(
            "queries/connect-cache-save-account.sql",
            user,
            credential_hash,
            source,
            generation,
            facts
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query_file!(
            "queries/connect-cache-prune-originals.sql",
            user,
            source,
            user,
            source,
            generation
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query_file!(
            "queries/connect-cache-prune-summaries.sql",
            user,
            source,
            generation
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Loads searchable summaries without reading any FIT archives.
    /// # Errors
    /// Returns database or cached-value failures.
    pub async fn connect_cache_summaries(
        &self,
        user: UserId,
        account: &CachedAccount,
    ) -> Result<Vec<CachedActivity>, Error> {
        let user = user.to_string();
        let source = account.source.to_string();
        let generation = &account.generation;
        sqlx::query_file_scalar!(
            "queries/connect-cache-summaries.sql",
            user,
            source,
            generation
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|text| serde_json::from_str(&text).map_err(invalid))
        .collect()
    }

    /// Reads an original or its last download failure, scoped to one profile and account.
    /// # Errors
    /// Returns database or cached-value failures.
    pub async fn connect_cached_original(
        &self,
        user: UserId,
        source: SourceId,
        id: i64,
    ) -> Result<Option<Result<Vec<u8>, String>>, Error> {
        let user = user.to_string();
        let source = source.to_string();
        sqlx::query_file!("queries/connect-cache-original.sql", user, source, id)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| match (row.archive, row.failure) {
                (Some(bytes), None) => Ok(Ok(bytes)),
                (None, Some(failure)) => Ok(Err(failure)),
                _ => Err(invalid("invalid original cache entry")),
            })
            .transpose()
    }

    /// Caches an original independently of activity acquisition and import.
    /// # Errors
    /// Returns database failures.
    pub async fn cache_connect_original(
        &self,
        user: UserId,
        source: SourceId,
        id: i64,
        result: &Result<Vec<u8>, String>,
    ) -> Result<(), Error> {
        let user = user.to_string();
        let source = source.to_string();
        let archive = result.as_ref().ok().map(Vec::as_slice);
        let failure = result.as_ref().err().map(String::as_str);
        sqlx::query_file!(
            "queries/connect-cache-save-original.sql",
            user,
            source,
            id,
            archive,
            failure
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Counts whole-account download coverage without loading file bytes.
    /// # Errors
    /// Returns database or cached-value failures.
    pub async fn connect_cache_status(
        &self,
        user: UserId,
        account: &CachedAccount,
    ) -> Result<CacheStatus, Error> {
        let user = user.to_string();
        let source = account.source.to_string();
        let generation = &account.generation;
        let row = sqlx::query_file!("queries/connect-cache-status.sql", user, source, generation)
            .fetch_one(&self.pool)
            .await?;
        Ok(CacheStatus {
            total: usize::try_from(row.total).map_err(invalid)?,
            downloaded: usize::try_from(row.downloaded).map_err(invalid)?,
            unavailable: usize::try_from(row.unavailable).map_err(invalid)?,
        })
    }
}

async fn imported_ascent(
    connection: &mut sqlx::SqliteConnection,
    user: &str,
    source: &str,
) -> Result<BTreeMap<i64, Distance>, Error> {
    let rows = sqlx::query_file!("queries/connect-imported-ascent.sql", user, source)
        .fetch_all(connection)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let id = row.source_identity?.parse().ok()?;
            let millimetres = u64::try_from(row.ascent_mm?).ok()?;
            Some((id, Distance::from_millimeters(millimetres)))
        })
        .collect())
}

impl Storage {
    /// Read the elevation measurements that can affect the current Trends history.
    /// # Errors
    /// Returns database read failures.
    pub async fn connect_trends_ascent(
        &self,
        user: UserId,
        fingerprint: &str,
    ) -> Result<BTreeMap<i64, Distance>, Error> {
        let user = user.to_string();
        let mut transaction = self.pool.begin().await?;
        let account = sqlx::query_file!("queries/connect-cache-account.sql", user, fingerprint)
            .fetch_optional(&mut *transaction)
            .await?;
        let ascent = match account {
            Some(account) => imported_ascent(&mut transaction, &user, &account.source_id).await?,
            None => BTreeMap::new(),
        };
        transaction.commit().await?;
        Ok(ascent)
    }
}
