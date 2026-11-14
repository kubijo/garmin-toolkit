use garmin_model::{garmin_connect::PersonalApiKey, identity::UserId};

use crate::{Error, Storage};

impl Storage {
    /// Whether this exact acquisition produced activity observations.
    /// # Errors
    /// Returns database failures.
    pub async fn imported_operation(
        &self,
        user: UserId,
        operation: garmin_model::artifact::AcquisitionOperationId,
    ) -> Result<bool, Error> {
        let user = user.to_string();
        let operation = operation.to_string();
        Ok(
            sqlx::query_file_scalar!("queries/imported-operation.sql", user, operation)
                .fetch_one(&self.pool)
                .await?
                != 0,
        )
    }

    /// Remote identities with successfully imported activity observations for this source.
    /// # Errors
    /// Returns database failures.
    pub async fn imported_source_identities(
        &self,
        user: UserId,
        source: garmin_model::identity::SourceId,
    ) -> Result<Vec<String>, Error> {
        let user = user.to_string();
        let source = source.to_string();
        Ok(
            sqlx::query_file_scalar!("queries/imported-source-identities.sql", user, source)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// Whether this profile has a saved key, without loading its contents.
    /// # Errors
    /// Returns database failures.
    pub async fn garmin_connect_configured(&self, user: UserId) -> Result<bool, Error> {
        let user = user.to_string();
        Ok(
            sqlx::query_file_scalar!("queries/garmin-connect-configured.sql", user)
                .fetch_one(&self.pool)
                .await?
                != 0,
        )
    }

    /// Loads a key for authentication, never for profile snapshots.
    /// # Errors
    /// Returns database or persisted-value failures.
    pub async fn garmin_connect_key(&self, user: UserId) -> Result<Option<PersonalApiKey>, Error> {
        let user = user.to_string();
        sqlx::query_file_scalar!("queries/garmin-connect-key.sql", user)
            .fetch_optional(&self.pool)
            .await?
            .map(|value| {
                PersonalApiKey::from_string(value).map_err(|_| Error::InvalidData {
                    field: "Garmin Connect key",
                    reason: "invalid credential".into(),
                })
            })
            .transpose()
    }

    /// Saves, replaces, or removes this profile's key.
    /// # Errors
    /// Returns database failures, including an unknown profile.
    pub async fn set_garmin_connect_key(
        &self,
        user: UserId,
        key: Option<&PersonalApiKey>,
    ) -> Result<(), Error> {
        let user = user.to_string();
        if let Some(key) = key {
            let value = key.expose();
            sqlx::query_file!("queries/save-garmin-connect-key.sql", user, value)
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query_file!("queries/delete-garmin-connect-key.sql", user)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
}
