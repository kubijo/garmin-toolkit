//! FIT Course encoding and semantic verification.

use std::{fmt, io::Cursor, num::NonZeroU32};

use embedded_io_adapters::std::FromStd;
use garmin_model::route::{
    Coordinate, CueText, Elevation, Latitude, Longitude, NavigationCue, RouteName,
    RoutePlanRevision, RoutePoint, RoutePointIndex, RouteShape, RouteSport,
};
use geographiclib_rs::{Geodesic, InverseGeodesic};
use rustyfit::{
    Decoder, Encoder,
    profile::{mesgdef, typedef},
    proto::{FIT, Message, ProtocolVersion},
};
use thiserror::Error;

const PRODUCT_ID: u16 = 1;
const PRODUCT_NAME: &str = "Garmin Toolkit";

/// A nonzero serial assigned to one generated FIT Course artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SerialNumber(NonZeroU32);

impl SerialNumber {
    /// Validates a FIT serial number.
    /// # Errors
    /// [`Error::ZeroSerialNumber`] for zero, FIT's invalid `uint32z` value.
    pub const fn from_u32(value: u32) -> Result<Self, Error> {
        match NonZeroU32::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(Error::ZeroSerialNumber),
        }
    }

    #[must_use]
    pub const fn from_nonzero(value: NonZeroU32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(&self) -> u32 {
        self.0.get()
    }

    #[must_use]
    pub const fn as_nonzero(&self) -> &NonZeroU32 {
        &self.0
    }

    #[must_use]
    pub const fn into_u32(self) -> u32 {
        self.0.get()
    }

    #[must_use]
    pub const fn into_nonzero(self) -> NonZeroU32 {
        self.0
    }
}

impl fmt::Display for SerialNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Source-neutral semantics recovered from a FIT Course artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedCourse {
    name: RouteName,
    sport: RouteSport,
    points: Vec<RoutePoint>,
    cues: Vec<NavigationCue>,
}

impl DecodedCourse {
    #[must_use]
    pub const fn name(&self) -> &RouteName {
        &self.name
    }

    #[must_use]
    pub const fn sport(&self) -> RouteSport {
        self.sport
    }

    #[must_use]
    pub fn points(&self) -> &[RoutePoint] {
        &self.points
    }

    #[must_use]
    pub fn cues(&self) -> &[NavigationCue] {
        &self.cues
    }
}

/// FIT Course conversion failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("FIT Course serial number cannot be zero")]
    ZeroSerialNumber,
    #[error("FIT Course encoding requires exact route geometry")]
    UnresolvedGeometry,
    #[error("route contains too many navigation cues for FIT")]
    TooManyCues,
    #[error("route revision time cannot be represented by FIT")]
    TimestampOutOfRange,
    #[error("route duration cannot be represented by FIT")]
    DurationOutOfRange,
    #[error("route distance cannot be represented by FIT")]
    DistanceOutOfRange,
    #[error("route elevation cannot be represented by FIT")]
    ElevationOutOfRange,
    #[error("FIT Course encoding failed: {0}")]
    Encoding(String),
    #[error("FIT Course decoding failed: {0}")]
    Decoding(String),
    #[error("input is not one standalone FIT Course")]
    NotCourse,
    #[error("invalid FIT Course structure: {0}")]
    InvalidStructure(&'static str),
    #[error("invalid FIT Course value: {0}")]
    InvalidValue(#[from] garmin_model::route::Error),
}

/// Encodes exact route geometry as a Garmin FIT Course.
///
/// The caller assigns the artifact serial so deployment code can guarantee uniqueness.
/// # Errors
/// [`enum@Error`] when the revision is unresolved or cannot be represented by FIT.
pub fn encode(revision: &RoutePlanRevision, serial: SerialNumber) -> Result<Vec<u8>, Error> {
    let RouteShape::Geometry(points) = revision.shape() else {
        return Err(Error::UnresolvedGeometry);
    };
    let start_seconds = revision.created_at().as_unix_seconds();
    let end_offset = i64::try_from(points.len() - 1).map_err(|_| Error::TimestampOutOfRange)?;
    let duration_seconds = u32::try_from(end_offset).map_err(|_| Error::DurationOutOfRange)?;
    let end_seconds = start_seconds
        .checked_add(end_offset)
        .ok_or(Error::TimestampOutOfRange)?;
    let start_time = fit_time(start_seconds)?;
    let end_time = fit_time(end_seconds)?;
    let distances = cumulative_distances(points)?;
    let total_distance = *distances
        .last()
        .ok_or(Error::InvalidStructure("geometry has no points"))?;
    let sport = encode_sport(revision.sport());

    let mut file_id = mesgdef::FileId::new();
    file_id.r#type = typedef::File::COURSE;
    file_id.manufacturer = typedef::Manufacturer::DEVELOPMENT;
    file_id.product = PRODUCT_ID;
    file_id.serial_number = serial.into_u32();
    file_id.time_created = start_time;
    PRODUCT_NAME.clone_into(&mut file_id.product_name);

    let mut course = mesgdef::Course::new();
    course.sport = sport;
    revision.name().as_str().clone_into(&mut course.name);
    course.sub_sport = typedef::SubSport::GENERIC;
    course.capabilities = typedef::CourseCapabilities(
        typedef::CourseCapabilities::VALID.0
            | typedef::CourseCapabilities::DISTANCE.0
            | typedef::CourseCapabilities::POSITION.0
            | typedef::CourseCapabilities::NAVIGATION.0,
    );

    let mut lap = mesgdef::Lap::new();
    lap.message_index = typedef::MessageIndex(0);
    lap.timestamp = end_time;
    lap.start_time = start_time;
    lap.event = typedef::Event::LAP;
    lap.event_type = typedef::EventType::STOP;
    lap.sport = sport;
    set_lap_positions(&mut lap, points);
    lap.set_total_elapsed_time_scaled(f64::from(duration_seconds));
    lap.set_total_timer_time_scaled(f64::from(duration_seconds));
    lap.set_total_distance_scaled(total_distance);
    if lap.total_elapsed_time == u32::MAX || lap.total_timer_time == u32::MAX {
        return Err(Error::DurationOutOfRange);
    }
    if lap.total_distance == u32::MAX {
        return Err(Error::DistanceOutOfRange);
    }

    let mut messages = vec![
        Message::from(file_id),
        Message::from(course),
        Message::from(lap),
    ];
    messages.push(Message::from(timer_event(
        start_time,
        typedef::EventType::START,
    )));
    for ((point, distance), offset) in points.iter().zip(&distances).zip(0_i64..) {
        messages.push(Message::from(record(
            point,
            *distance,
            fit_time(start_seconds + offset)?,
        )?));
    }
    for (index, cue) in revision.cues().iter().enumerate() {
        let message_index = u16::try_from(index).map_err(|_| Error::TooManyCues)?;
        if message_index > typedef::MessageIndex::MASK.0 {
            return Err(Error::TooManyCues);
        }
        let point_index = cue.point_index().as_usize();
        let timestamp_offset =
            i64::try_from(point_index).map_err(|_| Error::TimestampOutOfRange)?;
        let mut course_point = mesgdef::CoursePoint::new();
        course_point.message_index = typedef::MessageIndex(message_index);
        course_point.timestamp = fit_time(start_seconds + timestamp_offset)?;
        course_point.r#type = typedef::CoursePoint::GENERIC;
        cue.text().as_str().clone_into(&mut course_point.name);
        course_point.set_distance_scaled(distances[point_index]);
        if course_point.distance == u32::MAX {
            return Err(Error::DistanceOutOfRange);
        }
        messages.push(Message::from(course_point));
    }
    messages.push(Message::from(timer_event(
        end_time,
        typedef::EventType::STOP_ALL,
    )));

    encode_fit(FIT {
        messages,
        ..FIT::default()
    })
}

/// Decodes one Garmin FIT Course into source-neutral route semantics.
/// # Errors
/// [`enum@Error`] for malformed input, a different FIT file type, or missing course semantics.
pub fn decode(bytes: &[u8]) -> Result<DecodedCourse, Error> {
    let mut input = FromStd::new(Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let fit = decoder
        .decode(&mut input)
        .map_err(|error| Error::Decoding(error.to_string()))?
        .ok_or(Error::NotCourse)?;
    if decoder
        .decode(&mut input)
        .map_err(|error| Error::Decoding(error.to_string()))?
        .is_some()
    {
        return Err(Error::NotCourse);
    }
    decode_fit(&fit)
}

fn decode_fit(fit: &FIT) -> Result<DecodedCourse, Error> {
    let mut file_id = None;
    let mut course = None;
    let mut laps = Vec::new();
    let mut records = Vec::new();
    let mut course_points = Vec::new();
    let mut start = None;
    let mut stop = None;

    for (index, message) in fit.messages.iter().enumerate() {
        match message.num {
            typedef::MesgNum::FILE_ID => {
                if file_id
                    .replace((index, mesgdef::FileId::from(message)))
                    .is_some()
                {
                    return Err(Error::InvalidStructure("multiple file-id messages"));
                }
            }
            typedef::MesgNum::COURSE => {
                if course
                    .replace((index, mesgdef::Course::from(message)))
                    .is_some()
                {
                    return Err(Error::InvalidStructure("multiple course messages"));
                }
            }
            typedef::MesgNum::LAP => laps.push(index),
            typedef::MesgNum::RECORD => records.push((index, mesgdef::Record::from(message))),
            typedef::MesgNum::COURSE_POINT => {
                course_points.push((index, mesgdef::CoursePoint::from(message)));
            }
            typedef::MesgNum::EVENT => {
                let event = mesgdef::Event::from(message);
                if event.event == typedef::Event::TIMER {
                    match event.event_type {
                        typedef::EventType::START => {
                            assign_once(&mut start, index, "multiple timer-start events")?;
                        }
                        typedef::EventType::STOP | typedef::EventType::STOP_ALL => {
                            assign_once(&mut stop, index, "multiple timer-stop events")?;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let (file_index, file_id) = file_id.ok_or(Error::NotCourse)?;
    if file_id.r#type != typedef::File::COURSE {
        return Err(Error::NotCourse);
    }
    let (course_index, course) =
        course.ok_or(Error::InvalidStructure("course message is missing"))?;
    let start = start.ok_or(Error::InvalidStructure("timer-start event is missing"))?;
    let stop = stop.ok_or(Error::InvalidStructure("timer-stop event is missing"))?;
    if laps.is_empty() || records.len() < 2 {
        return Err(Error::InvalidStructure(
            "required course messages are missing",
        ));
    }
    validate_message_order(
        file_index,
        course_index,
        &laps,
        start,
        &records,
        &course_points,
        stop,
    )?;
    let name = RouteName::from_string(course.name)?;
    let sport = decode_sport(course.sport)?;
    let points = records
        .iter()
        .map(|(_, record)| decode_record(record))
        .collect::<Result<Vec<_>, _>>()?;
    validate_record_order(&records)?;
    let cues = course_points
        .iter()
        .map(|(_, point)| decode_cue(point, &records))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DecodedCourse {
        name,
        sport,
        points,
        cues,
    })
}

fn validate_message_order(
    file: usize,
    course: usize,
    laps: &[usize],
    start: usize,
    records: &[(usize, mesgdef::Record)],
    course_points: &[(usize, mesgdef::CoursePoint)],
    stop: usize,
) -> Result<(), Error> {
    let ordered = file < course
        && laps.iter().all(|index| course < *index && *index < start)
        && records
            .iter()
            .all(|(index, _)| start < *index && *index < stop)
        && course_points.iter().all(|(index, _)| {
            records.last().is_some_and(|(record, _)| record < index) && *index < stop
        });
    if ordered {
        Ok(())
    } else {
        Err(Error::InvalidStructure("course messages are out of order"))
    }
}

const fn assign_once(
    slot: &mut Option<usize>,
    index: usize,
    duplicate: &'static str,
) -> Result<(), Error> {
    if slot.replace(index).is_some() {
        Err(Error::InvalidStructure(duplicate))
    } else {
        Ok(())
    }
}

fn cumulative_distances(points: &[RoutePoint]) -> Result<Vec<f64>, Error> {
    let geodesic = Geodesic::wgs84();
    let mut distances = Vec::with_capacity(points.len());
    distances.push(0.0);
    for pair in points.windows(2) {
        let start = pair[0].coordinate();
        let end = pair[1].coordinate();
        let distance: f64 = geodesic.inverse(
            start.latitude().as_degrees(),
            start.longitude().as_degrees(),
            end.latitude().as_degrees(),
            end.longitude().as_degrees(),
        );
        let cumulative = distances
            .last()
            .copied()
            .ok_or(Error::InvalidStructure("geometry has no first point"))?
            + distance;
        if !cumulative.is_finite() || cumulative * 100.0 >= f64::from(u32::MAX) {
            return Err(Error::DistanceOutOfRange);
        }
        distances.push(cumulative);
    }
    Ok(distances)
}

fn record(
    point: &RoutePoint,
    distance: f64,
    timestamp: typedef::DateTime,
) -> Result<mesgdef::Record, Error> {
    let mut record = mesgdef::Record::new();
    record.timestamp = timestamp;
    set_record_position(&mut record, point.coordinate());
    record.set_distance_scaled(distance);
    if record.distance == u32::MAX {
        return Err(Error::DistanceOutOfRange);
    }
    if let Some(elevation) = point.elevation() {
        let meters = elevation.as_meters();
        let maximum = f64::from(u32::MAX) / 5.0 - 500.0;
        if meters < -500.0 || meters >= maximum {
            return Err(Error::ElevationOutOfRange);
        }
        record.set_enhanced_altitude_scaled(meters);
        if record.enhanced_altitude == u32::MAX {
            return Err(Error::ElevationOutOfRange);
        }
    }
    Ok(record)
}

const fn timer_event(
    timestamp: typedef::DateTime,
    event_type: typedef::EventType,
) -> mesgdef::Event {
    let mut event = mesgdef::Event::new();
    event.timestamp = timestamp;
    event.event = typedef::Event::TIMER;
    event.event_type = event_type;
    event
}

fn set_record_position(record: &mut mesgdef::Record, coordinate: Coordinate) {
    record.set_position_lat_degrees(coordinate.latitude().as_degrees());
    record.set_position_long_degrees(fit_longitude(coordinate.longitude()));
}

fn set_lap_positions(lap: &mut mesgdef::Lap, points: &[RoutePoint]) {
    let start = points[0].coordinate();
    let end = points[points.len() - 1].coordinate();
    lap.set_start_position_lat_degrees(start.latitude().as_degrees());
    lap.set_start_position_long_degrees(fit_longitude(start.longitude()));
    lap.set_end_position_lat_degrees(end.latitude().as_degrees());
    lap.set_end_position_long_degrees(fit_longitude(end.longitude()));
}

fn fit_longitude(longitude: Longitude) -> f64 {
    if longitude.as_degrees() >= 180.0 {
        -180.0
    } else {
        longitude.as_degrees()
    }
}

fn fit_time(unix_seconds: i64) -> Result<typedef::DateTime, Error> {
    let timestamp = typedef::DateTime::from_unix_timestamp(unix_seconds);
    if timestamp.0 < typedef::DateTime::MIN.0 || timestamp.0 == u32::MAX {
        Err(Error::TimestampOutOfRange)
    } else {
        Ok(timestamp)
    }
}

fn encode_fit(mut fit: FIT) -> Result<Vec<u8>, Error> {
    let mut output = FromStd::new(Cursor::new(Vec::new()));
    Encoder::builder()
        .protocol_version(ProtocolVersion::V1)
        .build()
        .encode(&mut output, &mut fit)
        .map_err(|error| Error::Encoding(error.to_string()))?;
    Ok(output.into_inner().into_inner())
}

const fn encode_sport(sport: RouteSport) -> typedef::Sport {
    match sport {
        RouteSport::Cycling => typedef::Sport::CYCLING,
        RouteSport::Running => typedef::Sport::RUNNING,
    }
}

const fn decode_sport(sport: typedef::Sport) -> Result<RouteSport, Error> {
    match sport {
        typedef::Sport::CYCLING => Ok(RouteSport::Cycling),
        typedef::Sport::RUNNING => Ok(RouteSport::Running),
        _ => Err(Error::InvalidStructure("unsupported course sport")),
    }
}

fn decode_record(record: &mesgdef::Record) -> Result<RoutePoint, Error> {
    let latitude = record
        .position_lat_degrees()
        .ok_or(Error::InvalidStructure("record latitude is missing"))?;
    let longitude = record
        .position_long_degrees()
        .ok_or(Error::InvalidStructure("record longitude is missing"))?;
    let elevation = record
        .enhanced_altitude_scaled()
        .map(Elevation::from_meters)
        .transpose()?;
    Ok(RoutePoint::from_parts(
        Coordinate::from_parts(
            Latitude::from_degrees(latitude)?,
            Longitude::from_degrees(longitude)?,
        ),
        elevation,
    ))
}

fn validate_record_order(records: &[(usize, mesgdef::Record)]) -> Result<(), Error> {
    let valid = records
        .iter()
        .all(|(_, record)| record.timestamp.0 >= typedef::DateTime::MIN.0)
        && records.windows(2).all(|pair| {
            pair[0].1.timestamp.0 < pair[1].1.timestamp.0
                && pair[0].1.distance_scaled().is_some_and(|first| {
                    pair[1]
                        .1
                        .distance_scaled()
                        .is_some_and(|second| first <= second)
                })
        });
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidStructure(
            "record timestamps or distances are not ordered",
        ))
    }
}

fn decode_cue(
    point: &mesgdef::CoursePoint,
    records: &[(usize, mesgdef::Record)],
) -> Result<NavigationCue, Error> {
    let point_index = records
        .iter()
        .position(|(_, record)| record.timestamp == point.timestamp)
        .ok_or(Error::InvalidStructure(
            "course point does not address a record",
        ))?;
    Ok(NavigationCue::from_parts(
        RoutePointIndex::from_usize(point_index),
        CueText::from_string(point.name.clone())?,
    ))
}

#[cfg(test)]
mod tests {
    use garmin_model::{
        route::{RevisionProvenance, RevisionSource, RoutePlanId, RoutePlanRevisionId},
        value::Timestamp,
    };

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn course_round_trip_preserves_route_semantics() -> TestResult {
        let revision = revision(RouteShape::from_geometry(points())?)?;
        let bytes = encode(&revision, SerialNumber::from_u32(42)?)?;
        let decoded = decode(&bytes)?;

        assert_eq!(decoded.name(), revision.name());
        assert_eq!(decoded.sport(), revision.sport());
        assert_eq!(decoded.cues(), revision.cues());
        assert_eq!(decoded.points().len(), revision.shape().points().len());
        for (actual, expected) in decoded.points().iter().zip(revision.shape().points()) {
            assert_coordinate(actual.coordinate(), expected.coordinate());
            assert_elevation(actual.elevation(), expected.elevation());
        }
        Ok(())
    }

    #[test]
    fn encoded_course_has_the_official_message_anatomy() -> TestResult {
        let bytes = encode(
            &revision(RouteShape::from_geometry(points())?)?,
            SerialNumber::from_u32(7)?,
        )?;
        let mut decoder = Decoder::new();
        let mut input = FromStd::new(Cursor::new(bytes));
        let fit = decoder
            .decode(&mut input)?
            .ok_or("encoded course was empty")?;
        let message_numbers = fit
            .messages
            .iter()
            .map(|message| message.num.0)
            .collect::<Vec<_>>();

        assert_eq!(
            message_numbers,
            [
                typedef::MesgNum::FILE_ID.0,
                typedef::MesgNum::COURSE.0,
                typedef::MesgNum::LAP.0,
                typedef::MesgNum::EVENT.0,
                typedef::MesgNum::RECORD.0,
                typedef::MesgNum::RECORD.0,
                typedef::MesgNum::RECORD.0,
                typedef::MesgNum::COURSE_POINT.0,
                typedef::MesgNum::EVENT.0,
            ]
        );
        Ok(())
    }

    #[test]
    fn unresolved_geometry_is_not_silently_connected() -> TestResult {
        let revision = revision(RouteShape::from_control_points(points())?)?;
        let result = encode(&revision, SerialNumber::from_u32(1)?);
        assert!(matches!(result, Err(Error::UnresolvedGeometry)));
        Ok(())
    }

    #[test]
    fn corrupted_course_is_rejected() -> TestResult {
        let mut bytes = encode(
            &revision(RouteShape::from_geometry(points())?)?,
            SerialNumber::from_u32(1)?,
        )?;
        bytes.truncate(bytes.len() / 2);
        assert!(matches!(decode(&bytes), Err(Error::Decoding(_))));
        Ok(())
    }

    fn revision(shape: RouteShape) -> Result<RoutePlanRevision, garmin_model::route::Error> {
        RoutePlanRevision::from_parts(
            RoutePlanRevisionId::new_v4(),
            RoutePlanId::new_v4(),
            None,
            Timestamp::from_unix_seconds(1_780_000_000)
                .expect("the fixture timestamp is in Jiff's supported range"),
            RouteName::from_string("Helsinki loop".to_owned())?,
            RouteSport::Cycling,
            shape,
            vec![NavigationCue::from_parts(
                RoutePointIndex::from_usize(1),
                CueText::from_string("Turn right".to_owned())?,
            )],
            RevisionProvenance::from_parts(RevisionSource::Freehand, Vec::new()),
        )
    }

    fn points() -> Vec<RoutePoint> {
        vec![
            point(60.1699, 24.9384, 12.4),
            point(60.1708, 24.9410, 13.1),
            point(60.1720, 24.9442, 11.8),
        ]
    }

    fn point(latitude: f64, longitude: f64, elevation: f64) -> RoutePoint {
        RoutePoint::from_parts(
            Coordinate::from_parts(
                Latitude::from_degrees(latitude)
                    .expect("fixture latitude is inside the geographic range"),
                Longitude::from_degrees(longitude)
                    .expect("fixture longitude is inside the geographic range"),
            ),
            Some(Elevation::from_meters(elevation).expect("fixture elevation is finite")),
        )
    }

    fn assert_coordinate(actual: Coordinate, expected: Coordinate) {
        assert!((actual.latitude().as_degrees() - expected.latitude().as_degrees()).abs() < 1e-6);
        assert!((actual.longitude().as_degrees() - expected.longitude().as_degrees()).abs() < 1e-6);
    }

    fn assert_elevation(actual: Option<Elevation>, expected: Option<Elevation>) {
        match (actual, expected) {
            (Some(actual), Some(expected)) => {
                assert!((actual.as_meters() - expected.as_meters()).abs() <= 0.2);
            }
            (None, None) => {}
            _ => panic!("elevation presence changed during FIT round-trip"),
        }
    }
}
