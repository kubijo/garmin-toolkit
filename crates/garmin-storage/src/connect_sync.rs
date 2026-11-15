use crate::{Error, Storage};
use garmin_model::{garmin_connect::sync::State, identity::UserId};

impl Storage {
    /// Reads one profile's opt-in cache schedule.
    /// # Errors
    /// Returns database or invalid persisted-value failures.
    pub async fn connect_sync_state(&self, user: UserId) -> Result<State, Error> {
        let user = user.to_string();
        let state = sqlx::query_file_scalar!("queries/connect-sync-get.sql", user)
            .fetch_optional(&self.pool)
            .await?;
        state.map_or_else(
            || Ok(State::default()),
            |state| {
                serde_json::from_str(&state).map_err(|error| Error::InvalidData {
                    field: "Garmin auto-sync",
                    reason: error.to_string(),
                })
            },
        )
    }

    /// Persists configuration and redacted refresh status in the profile database.
    /// # Errors
    /// Returns database or serialization failures.
    pub async fn save_connect_sync_state(&self, user: UserId, state: &State) -> Result<(), Error> {
        let mut transaction = self.pool.begin().await?;
        write_state(&mut transaction, user, state).await?;
        transaction.commit().await?;
        Ok(())
    }
}

pub(crate) async fn write_state(
    connection: &mut sqlx::SqliteConnection,
    user: UserId,
    state: &State,
) -> Result<(), Error> {
    let user_text = user.to_string();
    let encoded = serde_json::to_string(state).map_err(|error| Error::InvalidData {
        field: "Garmin auto-sync",
        reason: error.to_string(),
    })?;
    sqlx::query_file!("queries/connect-sync-save.sql", user_text, encoded)
        .execute(&mut *connection)
        .await?;
    crate::jobs::enqueue_connect_sync(connection, user, state).await?;
    Ok(())
}
