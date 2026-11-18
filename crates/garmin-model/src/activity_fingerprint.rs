//! Canonical activity fingerprint payload.

use semver::Version;
use serde::{Serialize, Serializer};

use crate::{
    activity::{
        Activity, ActivityDuration, ActivityMetrics, ActivitySport, ActivityTotals, Distance,
        Energy, HeartRate, Lap, Power, Speed, Temperature, TimerEvent, TimerState, TrackPoint,
    },
    observation::{FingerprintSchema, SemanticFingerprint, SemanticFingerprintError},
    value::{ComponentVersion, Timestamp},
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
    #[serde(serialize_with = "timestamp_milliseconds")]
    start_ms: Timestamp,
    #[serde(serialize_with = "timestamp_milliseconds")]
    end_ms: Timestamp,
    totals: CanonicalTotals,
    metrics: CanonicalMetrics,
}

impl From<crate::activity::ActivitySummary> for CanonicalSummary {
    fn from(summary: crate::activity::ActivitySummary) -> Self {
        Self {
            sport: CanonicalSport::from(summary.sport()),
            start_ms: summary.time().start(),
            end_ms: summary.time().end(),
            totals: CanonicalTotals::from(summary.totals()),
            metrics: CanonicalMetrics::from(summary.metrics()),
        }
    }
}

#[derive(Serialize)]
enum CanonicalSport {
    Running,
    Cycling,
    Swimming,
    Walking,
    Hiking,
}

impl From<ActivitySport> for CanonicalSport {
    fn from(sport: ActivitySport) -> Self {
        match sport {
            ActivitySport::Running => Self::Running,
            ActivitySport::Cycling => Self::Cycling,
            ActivitySport::Swimming => Self::Swimming,
            ActivitySport::Walking => Self::Walking,
            ActivitySport::Hiking => Self::Hiking,
        }
    }
}

#[derive(Serialize)]
struct CanonicalTotals {
    elapsed_ms: ActivityDuration,
    timer_ms: ActivityDuration,
    distance_mm: Option<Distance>,
    energy_kcal: Option<Energy>,
    ascent_mm: Option<Distance>,
    descent_mm: Option<Distance>,
}

impl From<ActivityTotals> for CanonicalTotals {
    fn from(totals: ActivityTotals) -> Self {
        Self {
            elapsed_ms: totals.elapsed(),
            timer_ms: totals.timer(),
            distance_mm: totals.distance(),
            energy_kcal: totals.energy(),
            ascent_mm: totals.ascent(),
            descent_mm: totals.descent(),
        }
    }
}

#[derive(Serialize)]
struct CanonicalMetrics {
    average_speed_mm_s: Option<Speed>,
    maximum_speed_mm_s: Option<Speed>,
    average_heart_rate_bpm: Option<HeartRate>,
    maximum_heart_rate_bpm: Option<HeartRate>,
    average_cadence_rpm: Option<f64>,
    maximum_cadence_rpm: Option<f64>,
    average_power_w: Option<Power>,
    maximum_power_w: Option<Power>,
}

impl From<ActivityMetrics> for CanonicalMetrics {
    fn from(metrics: ActivityMetrics) -> Self {
        Self {
            average_speed_mm_s: metrics.average_speed(),
            maximum_speed_mm_s: metrics.maximum_speed(),
            average_heart_rate_bpm: metrics.average_heart_rate(),
            maximum_heart_rate_bpm: metrics.maximum_heart_rate(),
            average_cadence_rpm: metrics
                .average_cadence()
                .map(|value| canonical_float(value.into_revolutions_per_minute())),
            maximum_cadence_rpm: metrics
                .maximum_cadence()
                .map(|value| canonical_float(value.into_revolutions_per_minute())),
            average_power_w: metrics.average_power(),
            maximum_power_w: metrics.maximum_power(),
        }
    }
}

#[derive(Serialize)]
struct CanonicalLap {
    #[serde(serialize_with = "timestamp_milliseconds")]
    start_ms: Timestamp,
    #[serde(serialize_with = "timestamp_milliseconds")]
    end_ms: Timestamp,
    totals: CanonicalTotals,
    metrics: CanonicalMetrics,
}

impl From<Lap> for CanonicalLap {
    fn from(lap: Lap) -> Self {
        Self {
            start_ms: lap.time().start(),
            end_ms: lap.time().end(),
            totals: CanonicalTotals::from(lap.totals()),
            metrics: CanonicalMetrics::from(lap.metrics()),
        }
    }
}

#[derive(Serialize)]
struct CanonicalTrackPoint {
    #[serde(serialize_with = "timestamp_milliseconds")]
    timestamp_ms: Timestamp,
    latitude_degrees: Option<f64>,
    longitude_degrees: Option<f64>,
    elevation_m: Option<f64>,
    distance_mm: Option<Distance>,
    speed_mm_s: Option<Speed>,
    heart_rate_bpm: Option<HeartRate>,
    cadence_rpm: Option<f64>,
    power_w: Option<Power>,
    temperature_millicelsius: Option<Temperature>,
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
            timestamp_ms: point.timestamp(),
            latitude_degrees,
            longitude_degrees,
            elevation_m: point
                .elevation()
                .map(|value| canonical_float(value.into_meters())),
            distance_mm: point.distance(),
            speed_mm_s: measurements.speed(),
            heart_rate_bpm: measurements.heart_rate(),
            cadence_rpm: measurements
                .cadence()
                .map(|value| canonical_float(value.into_revolutions_per_minute())),
            power_w: measurements.power(),
            temperature_millicelsius: measurements.temperature(),
        }
    }
}

#[derive(Serialize)]
struct CanonicalTimerEvent {
    #[serde(serialize_with = "timestamp_milliseconds")]
    timestamp_ms: Timestamp,
    state: CanonicalTimerState,
}

impl From<TimerEvent> for CanonicalTimerEvent {
    fn from(event: TimerEvent) -> Self {
        let (timestamp, state) = event.into_parts();
        Self {
            timestamp_ms: timestamp,
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

fn timestamp_milliseconds<S: Serializer>(
    value: &Timestamp,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_i64(value.as_unix_milliseconds())
}
