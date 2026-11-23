//! Read-only account-wide trends from the published summary cache.

use std::collections::{BTreeMap, HashSet};

use garmin_model::artifact::ArtifactDigest;
use garmin_model::garmin_connect::trends::{ActivitySummary, History, ImportedMeasurements, Sport};
use garmin_storage::{CacheSnapshot, StoredActivitySummary};

use crate::{Application, Error, UserContext};

impl Application {
    /// Fingerprint the immutable recordings available to Trends for this profile.
    /// # Errors
    /// Returns profile authorization or cache persistence errors.
    pub async fn connect_trends_import_revision(
        &self,
        user: UserContext,
    ) -> Result<Option<ArtifactDigest>, Error> {
        self.garmin_connect_configured(user).await?;
        let ids = self.storage.trends_observation_ids(user.user_id()).await?;
        let bytes =
            serde_json::to_vec(&ids).map_err(|error| garmin_storage::Error::InvalidData {
                field: "Trends import revision",
                reason: error.to_string(),
            })?;
        Ok(Some(ArtifactDigest::from_bytes(&bytes)))
    }
    /// Reads local recordings and the current account's published history.
    /// # Errors
    /// Returns profile authorization or cache persistence errors.
    pub async fn connect_trends(&self, user: UserContext) -> Result<Option<History>, Error> {
        self.garmin_connect_configured(user).await?;
        if let Some(key) = self.storage.garmin_connect_key(user.user_id()).await?
            && let Some(snapshot) = self
                .storage
                .connect_trends_snapshot(
                    user.user_id(),
                    &crate::connect_import::cache::credential_hash(&key),
                )
                .await?
        {
            return Ok(Some(merge_recordings(snapshot)));
        }
        let recordings = self.storage.activities(user.user_id()).await?;
        let locals = local_recordings(&recordings, &jiff::tz::TimeZone::UTC, HashSet::new());
        Ok((!locals.is_empty()).then(|| History::from_recordings(&[], &BTreeMap::new(), locals)))
    }
}

fn merge_recordings(mut snapshot: CacheSnapshot) -> History {
    let zone = snapshot
        .time_zone
        .as_deref()
        .and_then(|name| jiff::tz::TimeZone::get(name).ok())
        .unwrap_or(jiff::tz::TimeZone::UTC);
    let by_id = snapshot
        .local
        .iter()
        .map(|recording| (recording.observation_id(), recording))
        .collect::<BTreeMap<_, _>>();
    let mut claimed = HashSet::new();
    let mut measurements = BTreeMap::new();
    for cached in &mut snapshot.activities {
        let Some(recording) = snapshot
            .recordings
            .get(&cached.activity.id)
            .and_then(|ids| {
                unambiguous_recording(ids.iter().filter_map(|id| by_id.get(id).copied()))
            })
        else {
            continue;
        };
        let summary = recording.summary();
        claimed.insert(recording.fingerprint());
        let date = cached
            .started_local
            .as_deref()
            .and_then(|text| text.get(..10));
        if date
            .and_then(|date| date.parse::<jiff::civil::Date>().ok())
            .is_none()
        {
            cached.started_local = Some(
                summary
                    .time()
                    .start()
                    .as_jiff()
                    .to_zoned(zone.clone())
                    .date()
                    .to_string(),
            );
        }
        cached.activity.sport = cached.activity.sport.or(Some(summary.sport()));
        cached.activity.distance = cached.activity.distance.or(summary.totals().distance());
        cached.activity.duration = cached.activity.duration.or(Some(summary.totals().timer()));
        measurements.insert(cached.activity.id, fit_measurements(recording));
    }
    let locals = local_recordings(&snapshot.local, &zone, claimed);
    History::from_recordings(&snapshot.activities, &measurements, locals)
}

fn local_recordings<'a>(
    recordings: &'a [StoredActivitySummary],
    zone: &jiff::tz::TimeZone,
    mut claimed: HashSet<&'a garmin_model::observation::SemanticFingerprint>,
) -> Vec<ActivitySummary> {
    recordings
        .iter()
        .filter(|recording| claimed.insert(recording.fingerprint()))
        .map(|recording| {
            let summary = recording.summary();
            let measurements = fit_measurements(recording);
            ActivitySummary {
                id: 0,
                date: summary
                    .time()
                    .start()
                    .as_jiff()
                    .to_zoned(zone.clone())
                    .date(),
                sport: Sport::from(summary.sport()),
                name: String::new(),
                distance: summary.totals().distance(),
                duration: Some(summary.totals().timer()),
                elevation_gain: measurements.ascent,
                average_heart_rate: measurements.average_heart_rate,
                peak_power: measurements.peak_power,
                average_power: measurements.average_power,
                local_observation: Some(recording.observation_id()),
            }
        })
        .collect()
}

fn unambiguous_recording<'a>(
    mut recordings: impl Iterator<Item = &'a StoredActivitySummary>,
) -> Option<&'a StoredActivitySummary> {
    let first = recordings.next()?;
    recordings
        .all(|recording| recording.fingerprint() == first.fingerprint())
        .then_some(first)
}

fn fit_measurements(recording: &StoredActivitySummary) -> ImportedMeasurements {
    let summary = recording.summary();
    ImportedMeasurements {
        ascent: summary.totals().ascent(),
        average_heart_rate: summary.metrics().average_heart_rate(),
        peak_power: summary.metrics().maximum_power(),
        average_power: summary.metrics().average_power(),
    }
}
