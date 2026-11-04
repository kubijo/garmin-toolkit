//! Portable activity GPX compatibility with the published GPX reader.

use garmin_model::{
    activity::{
        Activity, ActivityDuration, ActivityMetrics, ActivitySport, ActivitySummary,
        ActivityTotals, Lap, TimeRange, TimerEvent, TimerState, TrackMeasurements, TrackPoint,
    },
    route::{Coordinate, Elevation, Latitude, Longitude},
    value::Timestamp,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn timestamp(offset: i64) -> Result<Timestamp, Box<dyn std::error::Error>> {
    Ok(Timestamp::from_unix_milliseconds(
        1_780_000_000_123 + offset * 1_000,
    )?)
}

fn recording() -> Result<Activity, Box<dyn std::error::Error>> {
    let time = TimeRange::from_parts(timestamp(0)?, timestamp(7)?)?;
    let totals = ActivityTotals::from_parts(
        ActivityDuration::from_milliseconds(7_000),
        ActivityDuration::from_milliseconds(6_000),
        None,
        None,
        None,
        None,
    )?;
    let points = (0..=7)
        .map(|index| {
            Ok(TrackPoint::from_parts(
                timestamp(index)?,
                (index != 2).then_some(Coordinate::from_parts(
                    Latitude::from_degrees(60.123_456_789)?,
                    Longitude::from_degrees(25.987_654_321)?,
                )),
                (index != 3).then_some(Elevation::from_meters(-2.125)?),
                None,
                TrackMeasurements::default(),
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    Ok(Activity::from_parts(
        ActivitySummary::from_parts(
            ActivitySport::Running,
            time,
            totals,
            ActivityMetrics::default(),
        ),
        vec![Lap::from_parts(time, totals, ActivityMetrics::default())],
        points,
        vec![
            TimerEvent::from_parts(timestamp(4)?, TimerState::Stopped),
            TimerEvent::from_parts(timestamp(5)?, TimerState::Running),
            // Both transitions fall between two GPS samples and still break the line.
            TimerEvent::from_parts(
                Timestamp::from_unix_milliseconds(timestamp(5)?.as_unix_milliseconds() + 100)?,
                TimerState::Stopped,
            ),
            TimerEvent::from_parts(
                Timestamp::from_unix_milliseconds(timestamp(5)?.as_unix_milliseconds() + 200)?,
                TimerState::Running,
            ),
        ],
    )?)
}

#[test]
fn gaps_and_pauses_remain_separate_and_values_round_trip() -> TestResult {
    let activity = recording()?;
    let bytes = garmin_gpx::activity::write(&activity)?;
    let document = gpx_format::read(std::io::Cursor::new(bytes))?;
    assert_eq!(document.version, gpx_format::GpxVersion::Gpx11);
    assert_eq!(document.routes, []);
    assert_eq!(document.tracks.len(), 1);
    let segments = &document.tracks[0].segments;
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.points.len())
            .collect::<Vec<_>>(),
        [2, 1, 1, 2]
    );
    let offsets = [0, 1, 3, 5, 6, 7];
    for (point, offset) in segments
        .iter()
        .flat_map(|segment| &segment.points)
        .zip(offsets)
    {
        assert_eq!(point.point().x(), 25.987_654_321);
        assert_eq!(point.point().y(), 60.123_456_789);
        assert_eq!(point.elevation, (offset != 3).then_some(-2.125));
        let instant = time::OffsetDateTime::from(point.time.ok_or("missing GPX timestamp")?);
        assert_eq!(
            instant.unix_timestamp_nanos() / 1_000_000,
            i128::from(timestamp(offset)?.as_unix_milliseconds())
        );
    }
    Ok(())
}
