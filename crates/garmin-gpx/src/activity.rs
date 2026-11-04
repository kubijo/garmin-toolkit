//! Activity track projection for portable exports.

use garmin_model::activity::{Activity, TimerState};
use gpx_format::{Gpx, GpxVersion, Track, TrackSegment, Waypoint};

/// Writes a recorded activity as GPX 1.1 without joining recording gaps.
///
/// Missing coordinates and timer transitions end the current segment. Samples recorded
/// while the timer is stopped remain in the package's CSV rather than the GPX track.
///
/// # Errors
/// [`enum@crate::Error`] for timestamps outside the GPX
/// writer's supported range or writer failures.
pub fn write(activity: &Activity) -> Result<Vec<u8>, crate::Error> {
    let mut track = Track::default();
    let mut segment = TrackSegment::default();
    let mut events = activity.timer_events().iter().peekable();
    let mut running = true;
    for point in activity.track() {
        while let Some(event) = events.next_if(|event| event.timestamp() <= point.timestamp()) {
            let next_running = event.state() == TimerState::Running;
            if next_running != running {
                finish_segment(&mut track, &mut segment);
            }
            running = next_running;
        }
        let Some(coordinate) = point.coordinate().filter(|_| running) else {
            finish_segment(&mut track, &mut segment);
            continue;
        };
        let mut waypoint = Waypoint::new(geo_types::Point::new(
            coordinate.longitude().as_degrees(),
            coordinate.latitude().as_degrees(),
        ));
        waypoint.elevation = point.elevation().map(|value| value.as_meters());
        let nanoseconds = i128::from(point.timestamp().as_unix_milliseconds()) * 1_000_000;
        waypoint.time = Some(time::OffsetDateTime::from_unix_timestamp_nanos(nanoseconds)?.into());
        segment.points.push(waypoint);
    }
    finish_segment(&mut track, &mut segment);
    let document = Gpx {
        version: GpxVersion::Gpx11,
        creator: Some("Garmin Toolkit".to_owned()),
        tracks: vec![track],
        ..Gpx::default()
    };
    let mut bytes = Vec::new();
    gpx_format::write(&document, &mut bytes)?;
    Ok(bytes)
}

fn finish_segment(track: &mut Track, segment: &mut TrackSegment) {
    if !segment.points.is_empty() {
        track.segments.push(std::mem::take(segment));
    }
}
