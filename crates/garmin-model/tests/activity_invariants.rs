//! Normalized activity invariants and value boundaries.

use std::error::Error as StdError;

use garmin_model::{
    activity::{
        Activity, ActivityDuration, ActivityMetric, ActivityMetrics, ActivitySport,
        ActivitySummary, ActivityTotals, Cadence, Distance, Energy, Error, HeartRate, Lap, Power,
        Speed, Temperature, TimeRange, TimerEvent, TimerState, TrackMeasurements, TrackPoint,
    },
    route::{Coordinate, Elevation, Latitude, Longitude},
    value::Timestamp,
};

type TestResult = Result<(), Box<dyn StdError>>;

fn timestamp(milliseconds: i64) -> Result<Timestamp, jiff::Error> {
    Timestamp::from_unix_milliseconds(milliseconds)
}

fn time_range(start: i64, end: i64) -> Result<TimeRange, Box<dyn StdError>> {
    Ok(TimeRange::from_parts(timestamp(start)?, timestamp(end)?)?)
}

const fn totals() -> Result<ActivityTotals, Error> {
    ActivityTotals::from_parts(
        ActivityDuration::from_milliseconds(2_000),
        ActivityDuration::from_milliseconds(1_500),
        Some(Distance::from_millimeters(3_000)),
        Some(Energy::from_kilocalories(4)),
        Some(Distance::from_millimeters(5_000)),
        Some(Distance::from_millimeters(6_000)),
    )
}

fn metrics() -> Result<ActivityMetrics, Error> {
    Ok(ActivityMetrics::from_parts(
        ActivityMetric::from_parts(
            Some(Speed::from_millimeters_per_second(1_000)),
            Some(Speed::from_millimeters_per_second(2_000)),
        ),
        ActivityMetric::from_parts(
            Some(HeartRate::from_beats_per_minute(120)),
            Some(HeartRate::from_beats_per_minute(140)),
        ),
        ActivityMetric::from_parts(
            Some(Cadence::from_revolutions_per_minute(80.5)?),
            Some(Cadence::from_revolutions_per_minute(90.0)?),
        ),
        ActivityMetric::from_parts(Some(Power::from_watts(200)), Some(Power::from_watts(300))),
    ))
}

fn point(milliseconds: i64) -> Result<TrackPoint, jiff::Error> {
    Ok(TrackPoint::from_parts(
        timestamp(milliseconds)?,
        None,
        None,
        Some(Distance::from_millimeters(milliseconds.unsigned_abs())),
        TrackMeasurements::from_parts(
            Some(Speed::from_millimeters_per_second(1_000)),
            Some(HeartRate::from_beats_per_minute(120)),
            None,
            Some(Power::from_watts(200)),
            Some(Temperature::from_millicelsius(-1_250)),
        ),
    ))
}

#[test]
fn scalar_values_round_trip_and_render() {
    let duration = ActivityDuration::from_milliseconds(1_234);
    assert_eq!(duration.as_milliseconds(), 1_234);
    assert_eq!(duration.into_milliseconds(), 1_234);
    assert_eq!(duration.to_string(), "1.234 s");

    let distance = Distance::from_millimeters(2_345);
    assert_eq!(distance.as_millimeters(), 2_345);
    assert_eq!(distance.into_millimeters(), 2_345);
    assert_eq!(distance.to_string(), "2.345 m");

    let temperature = Temperature::from_millicelsius(-1_250);
    assert_eq!(temperature.as_millicelsius(), -1_250);
    assert_eq!(temperature.into_millicelsius(), -1_250);
    assert_eq!(temperature.to_string(), "-1.250 °C");

    assert_eq!(
        Cadence::from_revolutions_per_minute(f64::NAN),
        Err(Error::InvalidCadence)
    );
    assert_eq!(
        Cadence::from_revolutions_per_minute(-1.0),
        Err(Error::InvalidCadence)
    );
}

#[test]
fn activity_summary_round_trips_through_postcard() -> TestResult {
    let summary = ActivitySummary::from_parts(
        ActivitySport::Cycling,
        time_range(1_000, 3_000)?,
        totals()?,
        metrics()?,
    );

    let encoded = postcard::to_stdvec(&summary)?;
    let decoded = postcard::from_bytes::<ActivitySummary>(&encoded)?;

    assert_eq!(decoded, summary);
    Ok(())
}

#[test]
fn wire_deserialization_revalidates_domain_values() -> TestResult {
    #[derive(serde::Serialize)]
    struct RangeWire {
        start: Timestamp,
        end: Timestamp,
    }

    #[derive(serde::Serialize)]
    struct TotalsWire {
        elapsed: ActivityDuration,
        timer: ActivityDuration,
        distance: Option<Distance>,
        energy: Option<Energy>,
        ascent: Option<Distance>,
        descent: Option<Distance>,
    }

    let cadence = postcard::to_stdvec(&-1.0_f64)?;
    assert!(postcard::from_bytes::<Cadence>(&cadence).is_err());

    let latitude = postcard::to_stdvec(&91.0_f64)?;
    assert!(postcard::from_bytes::<Latitude>(&latitude).is_err());

    let longitude = postcard::to_stdvec(&f64::NAN)?;
    assert!(postcard::from_bytes::<Longitude>(&longitude).is_err());

    let range = postcard::to_stdvec(&RangeWire {
        start: timestamp(2_000)?,
        end: timestamp(1_000)?,
    })?;
    assert!(postcard::from_bytes::<TimeRange>(&range).is_err());

    let totals = postcard::to_stdvec(&TotalsWire {
        elapsed: ActivityDuration::from_milliseconds(1_000),
        timer: ActivityDuration::from_milliseconds(2_000),
        distance: None,
        energy: None,
        ascent: None,
        descent: None,
    })?;
    assert!(postcard::from_bytes::<ActivityTotals>(&totals).is_err());
    Ok(())
}

#[test]
fn aggregates_expose_independent_measurements() -> TestResult {
    let totals = totals()?;
    assert_eq!(totals.elapsed().as_milliseconds(), 2_000);
    assert_eq!(totals.timer().as_milliseconds(), 1_500);
    assert_eq!(
        totals.distance().map(Distance::into_millimeters),
        Some(3_000)
    );
    assert_eq!(totals.energy().map(Energy::into_kilocalories), Some(4));
    assert_eq!(totals.ascent().map(Distance::into_millimeters), Some(5_000));
    assert_eq!(
        totals.descent().map(Distance::into_millimeters),
        Some(6_000)
    );

    let metrics = metrics()?;
    assert_eq!(
        metrics
            .average_speed()
            .map(Speed::into_millimeters_per_second),
        Some(1_000)
    );
    assert_eq!(
        metrics
            .maximum_heart_rate()
            .map(HeartRate::into_beats_per_minute),
        Some(140)
    );
    assert_eq!(metrics.average_power().map(Power::into_watts), Some(200));

    let point = point(1_000)?;
    assert_eq!(point.timestamp().as_unix_milliseconds(), 1_000);
    assert_eq!(
        point.distance().map(Distance::into_millimeters),
        Some(1_000)
    );
    assert_eq!(
        point
            .measurements()
            .temperature()
            .map(Temperature::into_millicelsius),
        Some(-1_250)
    );
    Ok(())
}

#[test]
fn invalid_and_unordered_activity_detail_is_rejected() -> TestResult {
    assert_eq!(
        TimeRange::from_parts(timestamp(1_001)?, timestamp(1_000)?),
        Err(Error::ReversedTimeRange)
    );
    assert_eq!(
        ActivityTotals::from_parts(
            ActivityDuration::from_milliseconds(1),
            ActivityDuration::from_milliseconds(2),
            Some(Distance::from_millimeters(0)),
            None,
            None,
            None,
        ),
        Err(Error::TimerExceedsElapsed)
    );

    let summary = ActivitySummary::from_parts(
        ActivitySport::Running,
        time_range(1_000, 3_000)?,
        totals()?,
        metrics()?,
    );
    let laps = vec![
        Lap::from_parts(time_range(1_000, 2_000)?, totals()?, metrics()?),
        Lap::from_parts(time_range(2_000, 3_000)?, totals()?, metrics()?),
    ];
    let track = vec![point(1_000)?, point(2_000)?];
    let events = vec![
        TimerEvent::from_parts(timestamp(1_000)?, TimerState::Running),
        TimerEvent::from_parts(timestamp(2_000)?, TimerState::Stopped),
    ];
    let activity = Activity::from_parts(summary, laps.clone(), track.clone(), events.clone())?;
    assert_eq!(activity.laps().len(), 2);
    assert_eq!(activity.track().len(), 2);
    assert_eq!(activity.timer_events().len(), 2);

    assert_eq!(
        Activity::from_parts(summary, Vec::new(), track.clone(), events.clone()),
        Err(Error::NoLaps)
    );
    assert_eq!(
        Activity::from_parts(summary, laps.clone(), Vec::new(), events.clone()),
        Err(Error::NoTrackPoints)
    );
    assert_eq!(
        Activity::from_parts(
            summary,
            laps.into_iter().rev().collect(),
            track.clone(),
            events.clone()
        ),
        Err(Error::UnorderedLaps)
    );
    assert_eq!(
        Activity::from_parts(
            summary,
            vec![Lap::from_parts(
                time_range(1_000, 2_000)?,
                totals()?,
                metrics()?
            )],
            track.into_iter().rev().collect(),
            events.clone()
        ),
        Err(Error::UnorderedTrackPoints)
    );
    assert_eq!(
        Activity::from_parts(
            summary,
            vec![Lap::from_parts(
                time_range(1_000, 2_000)?,
                totals()?,
                metrics()?
            )],
            vec![point(1_000)?],
            events.into_iter().rev().collect(),
        ),
        Err(Error::UnorderedTimerEvents)
    );
    Ok(())
}

#[test]
fn activity_rejects_submillisecond_instants() -> TestResult {
    let fractional = "1970-01-01T00:00:01.000000001Z".parse::<Timestamp>()?;
    assert!(!fractional.is_millisecond_aligned());
    assert_eq!(
        TimeRange::from_parts(fractional, timestamp(2_000)?),
        Err(Error::SubmillisecondTimestamp)
    );

    let summary = ActivitySummary::from_parts(
        ActivitySport::Running,
        time_range(1_000, 3_000)?,
        totals()?,
        metrics()?,
    );
    let laps = vec![Lap::from_parts(
        time_range(1_000, 3_000)?,
        totals()?,
        metrics()?,
    )];
    let track = vec![TrackPoint::from_parts(
        fractional,
        None,
        None,
        None,
        TrackMeasurements::default(),
    )];
    assert_eq!(
        Activity::from_parts(summary, laps.clone(), track, Vec::new()),
        Err(Error::SubmillisecondTimestamp)
    );
    assert_eq!(
        Activity::from_parts(
            summary,
            laps,
            vec![point(1_000)?],
            vec![TimerEvent::from_parts(fractional, TimerState::Running)],
        ),
        Err(Error::SubmillisecondTimestamp)
    );
    Ok(())
}

#[test]
fn activity_fingerprint_schema_has_a_golden_vector() -> TestResult {
    let summary = ActivitySummary::from_parts(
        ActivitySport::Cycling,
        time_range(1_700_000_000_123, 1_700_000_003_456)?,
        totals()?,
        metrics()?,
    );
    let lap = Lap::from_parts(
        time_range(1_700_000_000_123, 1_700_000_003_456)?,
        totals()?,
        metrics()?,
    );
    let coordinate = Coordinate::from_parts(
        Latitude::from_degrees(60.1699)?,
        Longitude::from_degrees(24.9384)?,
    );
    let point = TrackPoint::from_parts(
        timestamp(1_700_000_001_234)?,
        Some(coordinate),
        Some(Elevation::from_meters(12.5)?),
        Some(Distance::from_millimeters(3_000)),
        TrackMeasurements::from_parts(
            Some(Speed::from_millimeters_per_second(1_000)),
            Some(HeartRate::from_beats_per_minute(120)),
            Some(Cadence::from_revolutions_per_minute(80.5)?),
            Some(Power::from_watts(200)),
            Some(Temperature::from_millicelsius(-1_250)),
        ),
    );
    let event = TimerEvent::from_parts(timestamp(1_700_000_000_123)?, TimerState::Running);
    let activity = Activity::from_parts(summary, vec![lap], vec![point], vec![event])?;

    let fingerprint = activity.semantic_fingerprint()?;
    assert_eq!(
        fingerprint.schema().to_string(),
        "activity-known-semantics@1.0.0"
    );
    assert_eq!(
        fingerprint.digest().to_hex().as_str(),
        "09e5f7bf40b22016ae8f70771f5a42597338cfa49fe355cce5ac21732fb036ea"
    );
    Ok(())
}
