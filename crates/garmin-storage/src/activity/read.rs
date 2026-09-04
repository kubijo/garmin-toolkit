//! Activity row hydration and reads.

use garmin_fit::{
    CreatorDiagnostics, ManufacturerId, NormalizedActivity, ProductId, ProductName,
    SequencePosition, SerialNumber, SoftwareVersion,
};
use garmin_model::{
    activity::{
        Activity, ActivityDuration, ActivityMetric, ActivityMetrics, ActivitySport,
        ActivitySummary, ActivityTotals, Cadence, Distance, Energy, HeartRate, Lap, Power, Speed,
        Temperature, TimeRange, TimerEvent, TimerState, TrackMeasurements, TrackPoint,
    },
    artifact::NormalizationRunId,
    identity::UserId,
    observation::{FingerprintSchema, ObservationId, SemanticFingerprint},
    route::{Coordinate, Elevation, Latitude, Longitude},
    value::{ComponentVersion, Timestamp},
};
use semver::Version;

use super::{StoredActivity, StoredActivitySummary};
use crate::Storage;

impl Storage {
    /// Lists one user's activity summaries, newest first.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted data.
    pub async fn activities(
        &self,
        owner_id: UserId,
    ) -> Result<Vec<StoredActivitySummary>, crate::Error> {
        let owner_id = owner_id.to_string();
        sqlx::query_file_as!(
            ActivityHeaderRow,
            "queries/activity-summaries.sql",
            owner_id,
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(StoredActivitySummary::try_from)
        .collect()
    }

    /// Loads one complete activity within its owner's scope.
    /// # Errors
    /// [`enum@crate::Error`] for database failures or invalid persisted data.
    pub async fn activity(
        &self,
        owner_id: UserId,
        observation_id: ObservationId,
    ) -> Result<Option<StoredActivity>, crate::Error> {
        let owner_id = owner_id.to_string();
        let observation_id = observation_id.to_string();
        let mut transaction = self.pool.begin().await?;
        let Some(header) = sqlx::query_file_as!(
            ActivityHeaderRow,
            "queries/activity.sql",
            owner_id,
            observation_id,
        )
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(None);
        };

        let laps =
            sqlx::query_file_as!(ActivityLapRow, "queries/activity-laps.sql", observation_id)
                .fetch_all(&mut *transaction)
                .await?
                .into_iter()
                .map(Lap::try_from)
                .collect::<Result<Vec<_>, _>>()?;
        let track = sqlx::query_file_as!(
            ActivityTrackPointRow,
            "queries/activity-track-points.sql",
            observation_id,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(TrackPoint::try_from)
        .collect::<Result<Vec<_>, _>>()?;
        let timer_events = sqlx::query_file_as!(
            ActivityTimerEventRow,
            "queries/activity-timer-events.sql",
            observation_id,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(TimerEvent::try_from)
        .collect::<Result<Vec<_>, _>>()?;
        let header = DecodedHeader::try_from(header)?;
        let activity = Activity::from_parts(header.summary, laps, track, timer_events)
            .map_err(|error| invalid("activity", error))?;
        let fingerprint = activity
            .semantic_fingerprint()
            .map_err(|error| invalid("activity fingerprint", error))?;
        if fingerprint != header.fingerprint {
            return Err(invalid(
                "activity fingerprint",
                "stored digest does not match reconstructed semantics",
            ));
        }
        transaction.commit().await?;

        Ok(Some(StoredActivity {
            observation_id: header.observation_id,
            owner_id: header.owner_id,
            normalization_run_id: header.normalization_run_id,
            sequence_position: header.sequence_position,
            normalized: NormalizedActivity::from_parts(header.creator, activity),
        }))
    }
}

struct ActivityHeaderRow {
    observation_id: String,
    owner_id: String,
    fingerprint_schema_name: String,
    fingerprint_schema_version: String,
    fingerprint_digest: Vec<u8>,
    normalization_run_id: String,
    sequence_position: i64,
    sport: String,
    start_ms: i64,
    end_ms: i64,
    elapsed_ms: i64,
    timer_ms: i64,
    distance_mm: Option<i64>,
    energy_kcal: Option<i64>,
    ascent_mm: Option<i64>,
    descent_mm: Option<i64>,
    average_speed_mm_s: Option<i64>,
    maximum_speed_mm_s: Option<i64>,
    average_heart_rate_bpm: Option<i64>,
    maximum_heart_rate_bpm: Option<i64>,
    average_cadence_rpm: Option<f64>,
    maximum_cadence_rpm: Option<f64>,
    average_power_w: Option<i64>,
    maximum_power_w: Option<i64>,
    manufacturer_id: Option<i64>,
    product_id: Option<i64>,
    serial_number: Option<i64>,
    product_name: Option<String>,
    software_version_hundredths: Option<i64>,
}

struct DecodedHeader {
    observation_id: ObservationId,
    owner_id: UserId,
    normalization_run_id: NormalizationRunId,
    sequence_position: SequencePosition,
    fingerprint: SemanticFingerprint,
    creator: CreatorDiagnostics,
    summary: ActivitySummary,
}

impl TryFrom<ActivityHeaderRow> for DecodedHeader {
    type Error = crate::Error;

    fn try_from(row: ActivityHeaderRow) -> Result<Self, Self::Error> {
        let aggregate = ActivityAggregateRow::from_header(&row);
        Ok(Self {
            observation_id: parse(&row.observation_id, "activity observation ID")?,
            owner_id: parse(&row.owner_id, "activity owner ID")?,
            normalization_run_id: parse(
                &row.normalization_run_id,
                "activity normalization-run ID",
            )?,
            sequence_position: SequencePosition::from_u32(integer(
                row.sequence_position,
                "activity sequence position",
            )?),
            fingerprint: decode_fingerprint(
                row.fingerprint_schema_name,
                &row.fingerprint_schema_version,
                row.fingerprint_digest,
            )?,
            creator: CreatorDiagnostics::from_parts(
                ManufacturerId::from_u16(integer(
                    required(row.manufacturer_id, "FIT manufacturer ID")?,
                    "FIT manufacturer ID",
                )?)
                .map_err(|error| invalid("FIT manufacturer ID", error))?,
                optional_integer(row.product_id, "FIT product ID")?
                    .map(ProductId::from_u16)
                    .transpose()
                    .map_err(|error| invalid("FIT product ID", error))?,
                optional_integer(row.serial_number, "FIT serial number")?
                    .map(SerialNumber::from_u32)
                    .transpose()
                    .map_err(|error| invalid("FIT serial number", error))?,
                row.product_name
                    .map(ProductName::from_string)
                    .transpose()
                    .map_err(|error| invalid("FIT product name", error))?,
                optional_integer(row.software_version_hundredths, "FIT software version")?
                    .map(SoftwareVersion::from_hundredths)
                    .transpose()
                    .map_err(|error| invalid("FIT software version", error))?,
            ),
            summary: ActivitySummary::from_parts(
                decode_sport(&row.sport)?,
                aggregate.time()?,
                aggregate.totals()?,
                aggregate.metrics()?,
            ),
        })
    }
}

fn decode_fingerprint(
    schema_name: String,
    schema_version: &str,
    digest: Vec<u8>,
) -> Result<SemanticFingerprint, crate::Error> {
    let component = ComponentVersion::from_parts(
        schema_name,
        Version::parse(schema_version)
            .map_err(|error| invalid("activity fingerprint schema", error))?,
    )
    .map_err(|error| invalid("activity fingerprint schema", error))?;
    let digest = digest
        .try_into()
        .map_err(|_| invalid("activity fingerprint", "digest is not 32 bytes"))?;
    Ok(SemanticFingerprint::from_parts(
        FingerprintSchema::from_component(component),
        blake3::Hash::from_bytes(digest),
    ))
}

impl TryFrom<ActivityHeaderRow> for StoredActivitySummary {
    type Error = crate::Error;

    fn try_from(row: ActivityHeaderRow) -> Result<Self, Self::Error> {
        let header = DecodedHeader::try_from(row)?;
        Ok(Self {
            observation_id: header.observation_id,
            owner_id: header.owner_id,
            normalization_run_id: header.normalization_run_id,
            sequence_position: header.sequence_position,
            creator: header.creator,
            summary: header.summary,
        })
    }
}

struct ActivityAggregateRow {
    start_ms: i64,
    end_ms: i64,
    elapsed_ms: i64,
    timer_ms: i64,
    distance_mm: Option<i64>,
    energy_kcal: Option<i64>,
    ascent_mm: Option<i64>,
    descent_mm: Option<i64>,
    average_speed_mm_s: Option<i64>,
    maximum_speed_mm_s: Option<i64>,
    average_heart_rate_bpm: Option<i64>,
    maximum_heart_rate_bpm: Option<i64>,
    average_cadence_rpm: Option<f64>,
    maximum_cadence_rpm: Option<f64>,
    average_power_w: Option<i64>,
    maximum_power_w: Option<i64>,
}

impl ActivityAggregateRow {
    const fn from_header(row: &ActivityHeaderRow) -> Self {
        Self {
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            elapsed_ms: row.elapsed_ms,
            timer_ms: row.timer_ms,
            distance_mm: row.distance_mm,
            energy_kcal: row.energy_kcal,
            ascent_mm: row.ascent_mm,
            descent_mm: row.descent_mm,
            average_speed_mm_s: row.average_speed_mm_s,
            maximum_speed_mm_s: row.maximum_speed_mm_s,
            average_heart_rate_bpm: row.average_heart_rate_bpm,
            maximum_heart_rate_bpm: row.maximum_heart_rate_bpm,
            average_cadence_rpm: row.average_cadence_rpm,
            maximum_cadence_rpm: row.maximum_cadence_rpm,
            average_power_w: row.average_power_w,
            maximum_power_w: row.maximum_power_w,
        }
    }

    fn time(&self) -> Result<TimeRange, crate::Error> {
        TimeRange::from_parts(
            timestamp(self.start_ms, "activity start")?,
            timestamp(self.end_ms, "activity end")?,
        )
        .map_err(|error| invalid("activity time range", error))
    }

    fn totals(&self) -> Result<ActivityTotals, crate::Error> {
        ActivityTotals::from_parts(
            ActivityDuration::from_milliseconds(integer(self.elapsed_ms, "elapsed time")?),
            ActivityDuration::from_milliseconds(integer(self.timer_ms, "timer time")?),
            optional_integer(self.distance_mm, "distance")?.map(Distance::from_millimeters),
            optional_integer(self.energy_kcal, "energy")?.map(Energy::from_kilocalories),
            optional_integer(self.ascent_mm, "ascent")?.map(Distance::from_millimeters),
            optional_integer(self.descent_mm, "descent")?.map(Distance::from_millimeters),
        )
        .map_err(|error| invalid("activity totals", error))
    }

    fn metrics(&self) -> Result<ActivityMetrics, crate::Error> {
        Ok(ActivityMetrics::from_parts(
            ActivityMetric::from_parts(
                optional_integer(self.average_speed_mm_s, "average speed")?
                    .map(Speed::from_millimeters_per_second),
                optional_integer(self.maximum_speed_mm_s, "maximum speed")?
                    .map(Speed::from_millimeters_per_second),
            ),
            ActivityMetric::from_parts(
                optional_integer(self.average_heart_rate_bpm, "average heart rate")?
                    .map(HeartRate::from_beats_per_minute),
                optional_integer(self.maximum_heart_rate_bpm, "maximum heart rate")?
                    .map(HeartRate::from_beats_per_minute),
            ),
            ActivityMetric::from_parts(
                self.average_cadence_rpm
                    .map(Cadence::from_revolutions_per_minute)
                    .transpose()
                    .map_err(|error| invalid("average cadence", error))?,
                self.maximum_cadence_rpm
                    .map(Cadence::from_revolutions_per_minute)
                    .transpose()
                    .map_err(|error| invalid("maximum cadence", error))?,
            ),
            ActivityMetric::from_parts(
                optional_integer(self.average_power_w, "average power")?.map(Power::from_watts),
                optional_integer(self.maximum_power_w, "maximum power")?.map(Power::from_watts),
            ),
        ))
    }
}

struct ActivityLapRow {
    start_ms: i64,
    end_ms: i64,
    elapsed_ms: i64,
    timer_ms: i64,
    distance_mm: Option<i64>,
    energy_kcal: Option<i64>,
    ascent_mm: Option<i64>,
    descent_mm: Option<i64>,
    average_speed_mm_s: Option<i64>,
    maximum_speed_mm_s: Option<i64>,
    average_heart_rate_bpm: Option<i64>,
    maximum_heart_rate_bpm: Option<i64>,
    average_cadence_rpm: Option<f64>,
    maximum_cadence_rpm: Option<f64>,
    average_power_w: Option<i64>,
    maximum_power_w: Option<i64>,
}

impl TryFrom<ActivityLapRow> for Lap {
    type Error = crate::Error;

    fn try_from(row: ActivityLapRow) -> Result<Self, Self::Error> {
        let row = ActivityAggregateRow {
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            elapsed_ms: row.elapsed_ms,
            timer_ms: row.timer_ms,
            distance_mm: row.distance_mm,
            energy_kcal: row.energy_kcal,
            ascent_mm: row.ascent_mm,
            descent_mm: row.descent_mm,
            average_speed_mm_s: row.average_speed_mm_s,
            maximum_speed_mm_s: row.maximum_speed_mm_s,
            average_heart_rate_bpm: row.average_heart_rate_bpm,
            maximum_heart_rate_bpm: row.maximum_heart_rate_bpm,
            average_cadence_rpm: row.average_cadence_rpm,
            maximum_cadence_rpm: row.maximum_cadence_rpm,
            average_power_w: row.average_power_w,
            maximum_power_w: row.maximum_power_w,
        };
        Ok(Self::from_parts(row.time()?, row.totals()?, row.metrics()?))
    }
}

struct ActivityTrackPointRow {
    timestamp_ms: i64,
    latitude_degrees: Option<f64>,
    longitude_degrees: Option<f64>,
    elevation_m: Option<f64>,
    distance_mm: Option<i64>,
    speed_mm_s: Option<i64>,
    heart_rate_bpm: Option<i64>,
    cadence_rpm: Option<f64>,
    power_w: Option<i64>,
    temperature_millicelsius: Option<i64>,
}

impl TryFrom<ActivityTrackPointRow> for TrackPoint {
    type Error = crate::Error;

    fn try_from(row: ActivityTrackPointRow) -> Result<Self, Self::Error> {
        let coordinate = match (row.latitude_degrees, row.longitude_degrees) {
            (Some(latitude), Some(longitude)) => Some(Coordinate::from_parts(
                Latitude::from_degrees(latitude).map_err(|error| invalid("latitude", error))?,
                Longitude::from_degrees(longitude).map_err(|error| invalid("longitude", error))?,
            )),
            (None, None) => None,
            _ => return Err(invalid("coordinate", "one component is missing")),
        };
        Ok(Self::from_parts(
            timestamp(row.timestamp_ms, "track timestamp")?,
            coordinate,
            row.elevation_m
                .map(Elevation::from_meters)
                .transpose()
                .map_err(|error| invalid("elevation", error))?,
            optional_integer(row.distance_mm, "track distance")?.map(Distance::from_millimeters),
            TrackMeasurements::from_parts(
                optional_integer(row.speed_mm_s, "track speed")?
                    .map(Speed::from_millimeters_per_second),
                optional_integer(row.heart_rate_bpm, "track heart rate")?
                    .map(HeartRate::from_beats_per_minute),
                row.cadence_rpm
                    .map(Cadence::from_revolutions_per_minute)
                    .transpose()
                    .map_err(|error| invalid("track cadence", error))?,
                optional_integer(row.power_w, "track power")?.map(Power::from_watts),
                optional_integer(row.temperature_millicelsius, "track temperature")?
                    .map(Temperature::from_millicelsius),
            ),
        ))
    }
}

struct ActivityTimerEventRow {
    timestamp_ms: i64,
    state: String,
}

impl TryFrom<ActivityTimerEventRow> for TimerEvent {
    type Error = crate::Error;

    fn try_from(row: ActivityTimerEventRow) -> Result<Self, Self::Error> {
        Ok(Self::from_parts(
            timestamp(row.timestamp_ms, "timer-event timestamp")?,
            decode_timer_state(&row.state)?,
        ))
    }
}

fn timestamp(value: i64, field: &'static str) -> Result<Timestamp, crate::Error> {
    Timestamp::from_unix_milliseconds(value).map_err(|error| invalid(field, error))
}

fn parse<T>(value: &str, field: &'static str) -> Result<T, crate::Error>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value.parse().map_err(|error| invalid(field, error))
}

fn integer<T>(value: i64, field: &'static str) -> Result<T, crate::Error>
where
    T: TryFrom<i64>,
    T::Error: std::fmt::Display,
{
    value.try_into().map_err(|error| invalid(field, error))
}

fn optional_integer<T>(value: Option<i64>, field: &'static str) -> Result<Option<T>, crate::Error>
where
    T: TryFrom<i64>,
    T::Error: std::fmt::Display,
{
    value.map(|value| integer(value, field)).transpose()
}

fn required<T>(value: Option<T>, field: &'static str) -> Result<T, crate::Error> {
    value.ok_or_else(|| invalid(field, "value is missing"))
}

fn invalid(field: &'static str, error: impl std::fmt::Display) -> crate::Error {
    crate::Error::InvalidData {
        field,
        reason: error.to_string(),
    }
}

fn decode_sport(value: &str) -> Result<ActivitySport, crate::Error> {
    match value {
        "running" => Ok(ActivitySport::Running),
        "cycling" => Ok(ActivitySport::Cycling),
        _ => Err(invalid("activity sport", value)),
    }
}

fn decode_timer_state(value: &str) -> Result<TimerState, crate::Error> {
    match value {
        "running" => Ok(TimerState::Running),
        "stopped" => Ok(TimerState::Stopped),
        _ => Err(invalid("timer-event state", value)),
    }
}
