//! Activity projection writes.

use garmin_fit::{CreatorDiagnostics, ProductName};
use garmin_model::activity::{
    ActivityMetrics, ActivitySport, ActivitySummary, ActivityTotals, Distance, Lap, TimeRange,
    TimerEvent, TimerState, TrackPoint,
};
use sqlx::{Sqlite, Transaction};

use super::ActivityProjection;
use crate::IngestionError;

pub async fn persist_activity_projections(
    transaction: &mut Transaction<'_, Sqlite>,
    projections: &[ActivityProjection<'_>],
) -> Result<(), crate::Error> {
    for projection in projections {
        persist_activity_projection(transaction, projection).await?;
    }
    Ok(())
}

async fn persist_activity_projection(
    transaction: &mut Transaction<'_, Sqlite>,
    projection: &ActivityProjection<'_>,
) -> Result<(), crate::Error> {
    let observation_id = projection.observation.id().to_string();
    let normalization_run_id = projection.observation.normalization_run_id().to_string();
    let sequence_position = i64::from(projection.sequence_position.as_u32());
    let summary = projection.normalized.activity().summary();
    let sport = encode_sport(summary.sport());
    let values = AggregateBindings::try_from_summary(summary)?;
    let stored = sqlx::query_file!(
        "queries/persist-activity-projection.sql",
        observation_id,
        normalization_run_id,
        sequence_position,
        sport,
        values.start_ms,
        values.end_ms,
        values.elapsed_ms,
        values.timer_ms,
        values.distance_mm,
        values.energy_kcal,
        values.ascent_mm,
        values.descent_mm,
        values.average_speed_mm_s,
        values.maximum_speed_mm_s,
        values.average_heart_rate_bpm,
        values.maximum_heart_rate_bpm,
        values.average_cadence_rpm,
        values.maximum_cadence_rpm,
        values.average_power_w,
        values.maximum_power_w,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if stored.is_none() {
        return Err(IngestionError::ActivityProjectionConflict.into());
    }

    persist_creator(
        transaction,
        &observation_id,
        projection.normalized.creator(),
    )
    .await?;
    for (position, lap) in projection.normalized.activity().laps().iter().enumerate() {
        persist_lap(transaction, &observation_id, position, *lap).await?;
    }
    for (position, point) in projection.normalized.activity().track().iter().enumerate() {
        persist_track_point(transaction, &observation_id, position, *point).await?;
    }
    for (position, event) in projection
        .normalized
        .activity()
        .timer_events()
        .iter()
        .enumerate()
    {
        persist_timer_event(transaction, &observation_id, position, *event).await?;
    }

    let counts = sqlx::query_file!(
        "queries/activity-projection-counts.sql",
        observation_id,
        observation_id,
        observation_id,
        observation_id,
    )
    .fetch_one(&mut **transaction)
    .await?;
    if counts.creator_count != 1
        || counts.lap_count != collection_len(projection.normalized.activity().laps())?
        || counts.track_point_count != collection_len(projection.normalized.activity().track())?
        || counts.timer_event_count
            != collection_len(projection.normalized.activity().timer_events())?
    {
        return Err(IngestionError::ActivityProjectionConflict.into());
    }
    Ok(())
}

async fn persist_creator(
    transaction: &mut Transaction<'_, Sqlite>,
    observation_id: &str,
    creator: &CreatorDiagnostics,
) -> Result<(), crate::Error> {
    let manufacturer_id = i64::from(creator.manufacturer().as_u16());
    let product_id = creator.product().map(|value| i64::from(value.as_u16()));
    let serial_number = creator
        .serial_number()
        .map(|value| i64::from(value.as_u32()));
    let product_name = creator.product_name().map(ProductName::as_str);
    let software_version_hundredths = creator
        .software_version()
        .map(|value| i64::from(value.as_hundredths()));
    let stored = sqlx::query_file!(
        "queries/persist-fit-creator-diagnostics.sql",
        observation_id,
        manufacturer_id,
        product_id,
        serial_number,
        product_name,
        software_version_hundredths,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if stored.is_none() {
        return Err(IngestionError::ActivityProjectionConflict.into());
    }
    Ok(())
}

async fn persist_lap(
    transaction: &mut Transaction<'_, Sqlite>,
    observation_id: &str,
    position: usize,
    lap: Lap,
) -> Result<(), crate::Error> {
    let position = collection_position(position)?;
    let values = AggregateBindings::try_from_lap(lap)?;
    let stored = sqlx::query_file!(
        "queries/persist-activity-lap.sql",
        observation_id,
        position,
        values.start_ms,
        values.end_ms,
        values.elapsed_ms,
        values.timer_ms,
        values.distance_mm,
        values.energy_kcal,
        values.ascent_mm,
        values.descent_mm,
        values.average_speed_mm_s,
        values.maximum_speed_mm_s,
        values.average_heart_rate_bpm,
        values.maximum_heart_rate_bpm,
        values.average_cadence_rpm,
        values.maximum_cadence_rpm,
        values.average_power_w,
        values.maximum_power_w,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if stored.is_none() {
        return Err(IngestionError::ActivityProjectionConflict.into());
    }
    Ok(())
}

async fn persist_track_point(
    transaction: &mut Transaction<'_, Sqlite>,
    observation_id: &str,
    position: usize,
    point: TrackPoint,
) -> Result<(), crate::Error> {
    let position = collection_position(position)?;
    let timestamp_ms = point.timestamp().as_unix_milliseconds();
    let (latitude_degrees, longitude_degrees) = point.coordinate().map_or((None, None), |value| {
        (
            Some(value.latitude().as_degrees()),
            Some(value.longitude().as_degrees()),
        )
    });
    let elevation_m = point.elevation().map(|value| value.as_meters());
    let distance_mm = optional_u64_to_i64(point.distance().map(Distance::into_millimeters))?;
    let measurements = point.measurements();
    let speed_mm_s = measurements
        .speed()
        .map(|value| i64::from(value.as_millimeters_per_second()));
    let heart_rate_bpm = measurements
        .heart_rate()
        .map(|value| i64::from(value.as_beats_per_minute()));
    let cadence_rpm = measurements
        .cadence()
        .map(|value| value.as_revolutions_per_minute());
    let power_w = measurements
        .power()
        .map(|value| i64::from(value.as_watts()));
    let temperature_millicelsius = measurements
        .temperature()
        .map(|value| i64::from(value.as_millicelsius()));
    let stored = sqlx::query_file!(
        "queries/persist-activity-track-point.sql",
        observation_id,
        position,
        timestamp_ms,
        latitude_degrees,
        longitude_degrees,
        elevation_m,
        distance_mm,
        speed_mm_s,
        heart_rate_bpm,
        cadence_rpm,
        power_w,
        temperature_millicelsius,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if stored.is_none() {
        return Err(IngestionError::ActivityProjectionConflict.into());
    }
    Ok(())
}

async fn persist_timer_event(
    transaction: &mut Transaction<'_, Sqlite>,
    observation_id: &str,
    position: usize,
    event: TimerEvent,
) -> Result<(), crate::Error> {
    let position = collection_position(position)?;
    let timestamp_ms = event.timestamp().as_unix_milliseconds();
    let state = encode_timer_state(event.state());
    let stored = sqlx::query_file!(
        "queries/persist-activity-timer-event.sql",
        observation_id,
        position,
        timestamp_ms,
        state,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    if stored.is_none() {
        return Err(IngestionError::ActivityProjectionConflict.into());
    }
    Ok(())
}

struct AggregateBindings {
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

impl AggregateBindings {
    fn try_from_summary(summary: ActivitySummary) -> Result<Self, IngestionError> {
        Self::from_parts(summary.time(), summary.totals(), summary.metrics())
    }

    fn try_from_lap(lap: Lap) -> Result<Self, IngestionError> {
        Self::from_parts(lap.time(), lap.totals(), lap.metrics())
    }

    fn from_parts(
        time: TimeRange,
        totals: ActivityTotals,
        metrics: ActivityMetrics,
    ) -> Result<Self, IngestionError> {
        Ok(Self {
            start_ms: time.start().as_unix_milliseconds(),
            end_ms: time.end().as_unix_milliseconds(),
            elapsed_ms: u64_to_i64(totals.elapsed().as_milliseconds())?,
            timer_ms: u64_to_i64(totals.timer().as_milliseconds())?,
            distance_mm: optional_u64_to_i64(totals.distance().map(Distance::into_millimeters))?,
            energy_kcal: totals
                .energy()
                .map(|value| i64::from(value.as_kilocalories())),
            ascent_mm: optional_u64_to_i64(totals.ascent().map(Distance::into_millimeters))?,
            descent_mm: optional_u64_to_i64(totals.descent().map(Distance::into_millimeters))?,
            average_speed_mm_s: metrics
                .average_speed()
                .map(|value| i64::from(value.as_millimeters_per_second())),
            maximum_speed_mm_s: metrics
                .maximum_speed()
                .map(|value| i64::from(value.as_millimeters_per_second())),
            average_heart_rate_bpm: metrics
                .average_heart_rate()
                .map(|value| i64::from(value.as_beats_per_minute())),
            maximum_heart_rate_bpm: metrics
                .maximum_heart_rate()
                .map(|value| i64::from(value.as_beats_per_minute())),
            average_cadence_rpm: metrics
                .average_cadence()
                .map(|value| value.as_revolutions_per_minute()),
            maximum_cadence_rpm: metrics
                .maximum_cadence()
                .map(|value| value.as_revolutions_per_minute()),
            average_power_w: metrics
                .average_power()
                .map(|value| i64::from(value.as_watts())),
            maximum_power_w: metrics
                .maximum_power()
                .map(|value| i64::from(value.as_watts())),
        })
    }
}

const fn encode_sport(sport: ActivitySport) -> &'static str {
    match sport {
        ActivitySport::Running => "running",
        ActivitySport::Cycling => "cycling",
    }
}

const fn encode_timer_state(state: TimerState) -> &'static str {
    match state {
        TimerState::Running => "running",
        TimerState::Stopped => "stopped",
    }
}

fn collection_position(position: usize) -> Result<i64, IngestionError> {
    i64::try_from(position).map_err(|_| IngestionError::TooManyRecords)
}

fn collection_len<T>(values: &[T]) -> Result<i64, IngestionError> {
    i64::try_from(values.len()).map_err(|_| IngestionError::TooManyRecords)
}

fn u64_to_i64(value: u64) -> Result<i64, IngestionError> {
    i64::try_from(value).map_err(|_| IngestionError::ActivityValueOutOfRange)
}

fn optional_u64_to_i64(value: Option<u64>) -> Result<Option<i64>, IngestionError> {
    value.map(u64_to_i64).transpose()
}
