//! Read-only account-wide trends from the published summary cache.

use std::collections::BTreeMap;

use garmin_model::artifact::ArtifactDigest;
use garmin_model::garmin_connect::trends::{History, ImportedMeasurements};

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
        let measurements = self
            .storage
            .connect_trends_measurements(
                user.user_id(),
                &crate::connect_import::cache::credential_hash(&key),
            )
            .await?;
        let revision =
            import_revision(&measurements).map_err(|error| garmin_storage::Error::InvalidData {
                field: "Trends import revision",
                reason: error.to_string(),
            })?;
        Ok(Some(revision))
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
                History::from_summaries_with_measurements(
                    &snapshot.activities,
                    &snapshot.measurements,
                )
            }))
    }
}

fn import_revision(
    measurements: &BTreeMap<i64, ImportedMeasurements>,
) -> Result<ArtifactDigest, serde_json::Error> {
    serde_json::to_vec(measurements).map(|bytes| ArtifactDigest::from_bytes(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_model::activity::{Distance, HeartRate};

    #[test]
    fn changing_only_imported_heart_rate_changes_the_revision() {
        let digest = |average_heart_rate| {
            import_revision(&BTreeMap::from([(
                23,
                ImportedMeasurements {
                    ascent: Some(Distance::from_millimeters(420_000)),
                    average_heart_rate,
                },
            )]))
            .unwrap()
        };
        let rate = |bpm| Some(HeartRate::from_beats_per_minute(bpm));
        assert_ne!(digest(None), digest(rate(144)));
        assert_ne!(digest(rate(144)), digest(rate(145)));
    }
}
