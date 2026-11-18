//! Read-only account-wide trends from the published summary cache.

use garmin_model::artifact::ArtifactDigest;
use garmin_model::garmin_connect::trends::History;

use crate::{Application, Error, UserContext};

impl Application {
    /// Fingerprint the imported measurements available to the current Trends account.
    /// # Errors
    /// Returns profile authorization or cache persistence errors.
    pub async fn connect_trends_import_revision(
        &self,
        user: UserContext,
    ) -> Result<Option<ArtifactDigest>, Error> {
        self.garmin_connect_configured(user).await?;
        let Some(key) = self.storage.garmin_connect_key(user.user_id()).await? else {
            return Ok(None);
        };
        let ascent = self
            .storage
            .connect_trends_ascent(
                user.user_id(),
                &crate::connect_import::cache::credential_hash(&key),
            )
            .await?;
        let mut bytes = Vec::new();
        for (id, distance) in ascent {
            bytes.extend_from_slice(&id.to_le_bytes());
            bytes.extend_from_slice(&distance.as_millimeters().to_le_bytes());
        }
        Ok(Some(ArtifactDigest::from_bytes(&bytes)))
    }
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
            .connect_trends_snapshot(
                user.user_id(),
                &crate::connect_import::cache::credential_hash(&key),
            )
            .await?
            .map(|snapshot| {
                History::from_summaries_with_ascent(&snapshot.activities, &snapshot.ascent)
            }))
    }
}
