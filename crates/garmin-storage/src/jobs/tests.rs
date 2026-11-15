use super::*;
use futures_lite::future::block_on;
use garmin_model::{
    garmin_connect::sync::{Interval, State, Unit},
    identity::{Profile, Role, User},
};
use sqlx::{Row, sqlite::SqlitePoolOptions};

#[test]
fn credential_change_rolls_back_with_schedule_or_enqueue_failure()
-> Result<(), Box<dyn std::error::Error>> {
    use garmin_model::garmin_connect::PersonalApiKey;
    block_on(async {
        let root = tempfile::tempdir()?;
        let storage = Storage::open(root.path().join("configuration.sqlite3")).await?;
        let user = User::from_parts(
            UserId::new_v4(),
            Role::Owner,
            Profile::from_display_name("Rider".parse()?),
        );
        storage.save_user(&user).await?;
        let old_key = PersonalApiKey::from_string("old-synthetic-key".into())?;
        let new_key = PersonalApiKey::from_string("new-synthetic-key".into())?;
        let mut before = State::default();
        before.configure(
            Interval::new(1, Unit::Day),
            Timestamp::from_unix_seconds(100).expect("timestamp"),
        );
        storage
            .set_garmin_connect_configuration(user.id(), Some(&old_key), &before)
            .await?;
        let mut next = State::default();
        next.configure(
            Interval::new(1, Unit::Day),
            Timestamp::from_unix_seconds(200).expect("timestamp"),
        );
        sqlx::query("DROP TABLE jobs")
            .execute(&storage.pool)
            .await?;
        assert!(
            storage
                .set_garmin_connect_configuration(user.id(), Some(&new_key), &next)
                .await
                .is_err()
        );
        assert_eq!(
            storage
                .garmin_connect_key(user.id())
                .await?
                .expect("old key")
                .expose(),
            old_key.expose()
        );
        assert_eq!(storage.connect_sync_state(user.id()).await?, before);
        sqlx::query("CREATE TRIGGER reject_configuration BEFORE UPDATE ON connect_sync BEGIN SELECT RAISE(ABORT, 'synthetic schedule failure'); END").execute(&storage.pool).await?;
        assert!(
            storage
                .set_garmin_connect_configuration(user.id(), None, &State::default())
                .await
                .is_err()
        );
        assert_eq!(
            storage
                .garmin_connect_key(user.id())
                .await?
                .expect("old key")
                .expose(),
            old_key.expose()
        );
        assert_eq!(storage.connect_sync_state(user.id()).await?, before);
        sqlx::query("DROP TRIGGER reject_configuration")
            .execute(&storage.pool)
            .await?;
        storage
            .set_garmin_connect_configuration(user.id(), None, &State::default())
            .await?;
        assert!(storage.garmin_connect_key(user.id()).await?.is_none());
        assert_eq!(storage.connect_sync_state(user.id()).await?.interval, None);
        storage.close().await;
        Ok(())
    })
}

#[test]
fn exclusive_recovery_reclaims_queued_and_running_claims_even_with_future_heartbeats()
-> Result<(), Box<dyn std::error::Error>> {
    block_on(async {
        let root = tempfile::tempdir()?;
        let storage = Storage::open(root.path().join("recovery.sqlite3")).await?;
        let user = User::from_parts(
            UserId::new_v4(),
            Role::Owner,
            Profile::from_display_name("Rider".parse()?),
        );
        storage.save_user(&user).await?;
        let mut state = State::default();
        state.configure(
            Interval::new(1, Unit::Day),
            Timestamp::from_unix_seconds(100).expect("timestamp"),
        );
        storage.save_connect_sync_state(user.id(), &state).await?;
        sqlx::query("INSERT INTO workers (id, worker_type, storage_name) VALUES ('dead-worker', ?, 'SqliteStorage')").bind(QUEUE).execute(&storage.pool).await?;
        sqlx::query("INSERT INTO workers (id, worker_type, storage_name) VALUES ('other-worker', 'other-queue', 'SqliteStorage')").execute(&storage.pool).await?;
        sqlx::query("INSERT INTO jobs (job, id, job_type, status, lock_by, run_at) SELECT job, '01ARZ3NDEKTSV4RRFFQ69G5FAV', 'other-queue', 'Queued', 'other-worker', run_at FROM jobs").execute(&storage.pool).await?;
        sqlx::query("UPDATE workers SET last_seen = strftime('%s', 'now') + 3600")
            .execute(&storage.pool)
            .await?;
        for status in ["Queued", "Running"] {
            sqlx::query("UPDATE jobs SET status = ?, lock_by = 'dead-worker', lock_at = strftime('%s', 'now') WHERE job_type = ?").bind(status).bind(QUEUE).execute(&storage.pool).await?;
            assert_eq!(storage.recover_jobs().await?, 1);
            let recovered: (String, Option<String>) =
                sqlx::query_as("SELECT status, lock_by FROM jobs WHERE job_type = ?")
                    .bind(QUEUE)
                    .fetch_one(&storage.pool)
                    .await?;
            assert_eq!(recovered, ("Pending".into(), None));
        }
        let other: (String, Option<String>) =
            sqlx::query_as("SELECT status, lock_by FROM jobs WHERE job_type = 'other-queue'")
                .fetch_one(&storage.pool)
                .await?;
        assert_eq!(other, ("Queued".into(), Some("other-worker".into())));
        assert_eq!(
            storage.connect_sync_state(user.id()).await?,
            state,
            "claim recovery must not fabricate an application attempt"
        );
        storage.close().await;
        Ok(())
    })
}

async fn schema(pool: &sqlx::SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT type, name, sql FROM sqlite_master WHERE lower(name) IN ('jobs', 'workers') OR (type = 'index' AND lower(tbl_name) IN ('jobs', 'workers') AND sql IS NOT NULL) ORDER BY type, lower(name)")
        .fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|row| {
            let sql: String = row.get("sql");
            sql.to_lowercase()
                .split_whitespace()
                .collect::<String>()
                .replace("ifnotexists", "")
        })
        .collect())
}

#[test]
fn embedded_queue_schema_matches_pinned_library_migrations_and_keeps_full_durability()
-> Result<(), Box<dyn std::error::Error>> {
    block_on(async {
        let root = tempfile::tempdir()?;
        let storage = Storage::open(root.path().join("application.sqlite3")).await?;
        let upstream = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        SqliteStorage::migrations().run(&upstream).await?;
        assert_eq!(schema(&storage.pool).await?, schema(&upstream).await?);
        let durability: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&storage.pool)
            .await?;
        assert_eq!(
            durability, 2,
            "queue migration must preserve synchronous=FULL"
        );
        upstream.close().await;
        storage.close().await;
        Ok(())
    })
}

#[test]
fn schedule_and_queue_are_atomic_deduplicated_and_survive_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    block_on(async {
        let root = tempfile::tempdir()?;
        let path = root.path().join("application.sqlite3");
        let storage = Storage::open(&path).await?;
        let user = User::from_parts(
            UserId::new_v4(),
            Role::Owner,
            Profile::from_display_name("Rider".parse()?),
        );
        storage.save_user(&user).await?;
        let mut state = State::default();
        state.configure(
            Interval::new(1, Unit::Day),
            Timestamp::from_unix_seconds(100).expect("timestamp"),
        );
        storage.save_connect_sync_state(user.id(), &state).await?;
        storage.save_connect_sync_state(user.id(), &state).await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM Jobs")
            .fetch_one(&storage.pool)
            .await?;
        assert_eq!(count, 1);
        storage.close().await;
        let storage = Storage::open(&path).await?;
        assert_eq!(storage.connect_sync_state(user.id()).await?, state);
        let job: Vec<u8> = sqlx::query_scalar("SELECT job FROM Jobs")
            .fetch_one(&storage.pool)
            .await?;
        assert!(
            matches!(serde_json::from_slice::<Job>(&job)?, Job::ConnectSync { user: owner, .. } if owner == user.id())
        );
        sqlx::query("DROP TABLE Jobs")
            .execute(&storage.pool)
            .await?;
        let mut changed = state.clone();
        changed.configure(
            Interval::new(2, Unit::Week),
            Timestamp::from_unix_seconds(200).expect("timestamp"),
        );
        assert!(
            storage
                .save_connect_sync_state(user.id(), &changed)
                .await
                .is_err()
        );
        assert_eq!(
            storage.connect_sync_state(user.id()).await?,
            state,
            "failed enqueue must roll back schedule update"
        );
        storage.close().await;
        Ok(())
    })
}
