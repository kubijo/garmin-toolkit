//! Canonical activity fingerprint payload.

use semver::Version;
use serde::Serialize;

use crate::{
    activity::{
        Activity, ActivityMetrics, ActivitySport, ActivityTotals, Distance, Energy, HeartRate, Lap,
        Power, Speed, Temperature, TimerEvent, TimerState, TrackPoint,
    },
    observation::{FingerprintSchema, SemanticFingerprint, SemanticFingerprintError},
    value::ComponentVersion,
};

const SCHEMA_NAME: &str = "activity-known-semantics";

pub fn fingerprint(activity: &Activity) -> Result<SemanticFingerprint, SemanticFingerprintError> {
    let payload = CanonicalActivity::from(activity);
    let bytes = postcard::to_stdvec(&payload)?;
    let schema = FingerprintSchema::from_component(ComponentVersion::from_parts(
        SCHEMA_NAME,
        Version::new(1, 0, 0),
    )?);
    Ok(SemanticFingerprint::from_canonical_bytes(schema, &bytes))
}

#[derive(Serialize)]
struct CanonicalActivity {
    summary: CanonicalSummary,
    laps: Vec<CanonicalLap>,
    track: Vec<CanonicalTrackPoint>,
    timer_events: Vec<CanonicalTimerEvent>,
}

impl From<&Activity> for CanonicalActivity {
    fn from(activity: &Activity) -> Self {
        Self {
            summary: CanonicalSummary::from(activity.summary()),
            laps: activity
                .laps()
                .iter()
                .copied()
                .map(CanonicalLap::from)
                .collect(),
            track: activity
                .track()
                .iter()
                .copied()
                .map(CanonicalTrackPoint::from)
                .collect(),
            timer_events: activity
                .timer_events()
                .iter()
                .copied()
                .map(CanonicalTimerEvent::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct CanonicalSummary {
    sport: CanonicalSport,
    start_ms: i64,
    end_ms: i64,
    totals: CanonicalTotals,
    metrics: CanonicalMetrics,
}

impl From<crate::activity::ActivitySummary> for CanonicalSummary {
    fn from(summary: crate::activity::ActivitySummary) -> Self {
        Self {
            sport: CanonicalSport::from(summary.sport()),
            start_ms: summary.time().start().as_unix_milliseconds(),
            end_ms: summary.time().end().as_unix_milliseconds(),
            totals: CanonicalTotals::from(summary.totals()),
            metrics: CanonicalMetrics::from(summary.metrics()),
        }
    }
}

#[derive(Serialize)]
enum CanonicalSport {
    Running,
    Cycling,
}

impl From<ActivitySport> for CanonicalSport {
    fn from(sport: ActivitySport) -> Self {
        match sport {
            ActivitySport::Running => Self::Running,
            ActivitySport::Cycling => Self::Cycling,
        }
    }
}

#[derive(Serialize)]
struct CanonicalTotals {
    elapsed_ms: u64,
    timer_ms: u64,
    distance_mm: Option<u64>,
    energy_kcal: Option<u32>,
    ascent_mm: Option<u64>,
    descent_mm: Option<u64>,
}

impl From<ActivityTotals> for CanonicalTotals {
    fn from(totals: ActivityTotals) -> Self {
        Self {
            elapsed_ms: totals.elapsed().into_milliseconds(),
            timer_ms: totals.timer().into_milliseconds(),
            distance_mm: totals.distance().map(Distance::into_millimeters),
            energy_kcal: totals.energy().map(Energy::into_kilocalories),
            ascent_mm: totals.ascent().map(Distance::into_millimeters),
            descent_mm: totals.descent().map(Distance::into_millimeters),
        }
    }
}

#[derive(Serialize)]
struct CanonicalMetrics {
    average_speed_mm_s: Option<u32>,
    maximum_speed_mm_s: Option<u32>,
    average_heart_rate_bpm: Option<u16>,
    maximum_heart_rate_bpm: Option<u16>,
    average_cadence_rpm: Option<f64>,
    maximum_cadence_rpm: Option<f64>,
    average_power_w: Option<u32>,
    maximum_power_w: Option<u32>,
}

impl From<ActivityMetrics> for CanonicalMetrics {
    fn from(metrics: ActivityMetrics) -> Self {
        Self {
            average_speed_mm_s: metrics
                .average_speed()
                .map(Speed::into_millimeters_per_second),
            maximum_speed_mm_s: metrics
                .maximum_speed()
                .map(Speed::into_millimeters_per_second),
            average_heart_rate_bpm: metrics
                .average_heart_rate()
                .map(HeartRate::into_beats_per_minute),
            maximum_heart_rate_bpm: metrics
                .maximum_heart_rate()
                .map(HeartRate::into_beats_per_minute),
            average_cadence_rpm: metrics
                .average_cadence()
                .map(|value| canonical_float(value.into_revolutions_per_minute())),
            maximum_cadence_rpm: metrics
                .maximum_cadence()
                .map(|value| canonical_float(value.into_revolutions_per_minute())),
            average_power_w: metrics.average_power().map(Power::into_watts),
            maximum_power_w: metrics.maximum_power().map(Power::into_watts),
        }
    }
}

#[derive(Serialize)]
struct CanonicalLap {
    start_ms: i64,
    end_ms: i64,
    totals: CanonicalTotals,
    metrics: CanonicalMetrics,
}

impl From<Lap> for CanonicalLap {
    fn from(lap: Lap) -> Self {
        Self {
            start_ms: lap.time().start().as_unix_milliseconds(),
            end_ms: lap.time().end().as_unix_milliseconds(),
            totals: CanonicalTotals::from(lap.totals()),
            metrics: CanonicalMetrics::from(lap.metrics()),
        }
    }
}

#[derive(Serialize)]
struct CanonicalTrackPoint {
    timestamp_ms: i64,
    latitude_degrees: Option<f64>,
    longitude_degrees: Option<f64>,
    elevation_m: Option<f64>,
    distance_mm: Option<u64>,
    speed_mm_s: Option<u32>,
    heart_rate_bpm: Option<u16>,
    cadence_rpm: Option<f64>,
    power_w: Option<u32>,
    temperature_millicelsius: Option<i32>,
}

impl From<TrackPoint> for CanonicalTrackPoint {
    fn from(point: TrackPoint) -> Self {
        let (latitude_degrees, longitude_degrees) =
            point.coordinate().map_or((None, None), |coordinate| {
                (
                    Some(canonical_float(coordinate.latitude().into_degrees())),
                    Some(canonical_float(coordinate.longitude().into_degrees())),
                )
            });
        let measurements = point.measurements();
        Self {
            timestamp_ms: point.timestamp().as_unix_milliseconds(),
            latitude_degrees,
            longitude_degrees,
            elevation_m: point
                .elevation()
                .map(|value| canonical_float(value.into_meters())),
            distance_mm: point.distance().map(Distance::into_millimeters),
            speed_mm_s: measurements.speed().map(Speed::into_millimeters_per_second),
            heart_rate_bpm: measurements
                .heart_rate()
                .map(HeartRate::into_beats_per_minute),
            cadence_rpm: measurements
                .cadence()
                .map(|value| canonical_float(value.into_revolutions_per_minute())),
            power_w: measurements.power().map(Power::into_watts),
            temperature_millicelsius: measurements
                .temperature()
                .map(Temperature::into_millicelsius),
        }
    }
}

#[derive(Serialize)]
struct CanonicalTimerEvent {
    timestamp_ms: i64,
    state: CanonicalTimerState,
}

impl From<TimerEvent> for CanonicalTimerEvent {
    fn from(event: TimerEvent) -> Self {
        let (timestamp, state) = event.into_parts();
        Self {
            timestamp_ms: timestamp.as_unix_milliseconds(),
            state: CanonicalTimerState::from(state),
        }
    }
}

#[derive(Serialize)]
enum CanonicalTimerState {
    Running,
    Stopped,
}

impl From<TimerState> for CanonicalTimerState {
    fn from(state: TimerState) -> Self {
        match state {
            TimerState::Running => Self::Running,
            TimerState::Stopped => Self::Stopped,
        }
    }
}

const fn canonical_float(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}
