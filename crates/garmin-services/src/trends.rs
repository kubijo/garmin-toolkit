//! Read-only account-wide trends from the published summary cache.

use garmin_model::garmin_connect::trends::History;

use crate::{Application, Error, UserContext};

impl Application {
    /// Reads the current credential's published history without networking or importing.
    /// # Errors
    /// Returns profile authorization or cache persistence errors.
    pub async fn connect_trends(&self, user: UserContext) -> Result<Option<History>, Error> {
        self.garmin_connect_configured(user).await?;
        let Some(key) = self.storage.garmin_connect_key(user.user_id()).await? else {
            return Ok(None);
        };
        Ok(self
            .storage
            .connect_cache_snapshot(
                user.user_id(),
                &crate::connect_import::cache::credential_hash(&key),
            )
            .await?
            .map(|snapshot| History::from_summaries(&snapshot.activities)))
    }
}
