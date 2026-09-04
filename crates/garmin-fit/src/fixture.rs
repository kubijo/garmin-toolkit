//! Deterministic synthetic FIT data for cross-crate tests.

use std::{io::Cursor, string::ToString};

use embedded_io_adapters::std::FromStd;
use rustyfit::{
    Encoder,
    profile::{mesgdef, typedef},
    proto::{FIT, Message, ProtocolVersion},
};
use thiserror::Error;

pub(crate) const START: i64 = 1_780_000_000;
#[cfg(test)]
pub(crate) const END: i64 = START + 30;

/// Generated activity sport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sport {
    Running,
    Cycling,
}

/// Publishable activity cases used by mock deployments and tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityCase {
    MorningRun,
    TempoRun,
    TrailRun,
    CommuteRide,
    EnduranceRide,
    RecoveryRide,
}

impl ActivityCase {
    /// Complete deliberately small corpus.
    pub const ALL: [Self; 6] = [
        Self::MorningRun,
        Self::TempoRun,
        Self::TrailRun,
        Self::CommuteRide,
        Self::EnduranceRide,
        Self::RecoveryRide,
    ];

    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::MorningRun => "2026-05-28-morning-run.fit",
            Self::TempoRun => "2026-05-30-tempo-run.fit",
            Self::TrailRun => "2026-06-01-trail-run.fit",
            Self::CommuteRide => "2026-05-29-commute-ride.fit",
            Self::EnduranceRide => "2026-05-31-endurance-ride.fit",
            Self::RecoveryRide => "2026-06-02-recovery-ride.fit",
        }
    }

    #[must_use]
    pub const fn start(self) -> i64 {
        self.definition().timing.start
    }

    #[must_use]
    pub fn end(self) -> i64 {
        let definition = self.definition();
        definition.timing.start + i64::from(definition.timing.elapsed_seconds)
    }

    /// Encodes this case as a complete FIT file.
    /// # Errors
    /// [`EncodingError`] when `rustyfit` rejects the generated messages.
    pub fn encode(self) -> Result<Vec<u8>, EncodingError> {
        encode_activity(self.definition())
    }

    const fn definition(self) -> ActivityDefinition {
        match self {
            Self::MorningRun => ActivityDefinition::new(
                typedef::Sport::RUNNING,
                Timing::new(1_779_949_800, 2_700, 2_580),
                Totals::new(785_000, 512, 74, 72),
                Speeds::new(3_043, 4_620),
                11,
            ),
            Self::TempoRun => ActivityDefinition::new(
                typedef::Sport::RUNNING,
                Timing::new(1_780_162_200, 3_180, 3_060),
                Totals::new(1_087_000, 714, 46, 45),
                Speeds::new(3_552, 5_210),
                12,
            ),
            Self::TrailRun => ActivityDefinition::new(
                typedef::Sport::RUNNING,
                Timing::new(1_780_330_500, 4_680, 4_500),
                Totals::new(1_243_000, 893, 328, 326),
                Speeds::new(2_762, 4_180),
                13,
            ),
            Self::CommuteRide => ActivityDefinition::new(
                typedef::Sport::CYCLING,
                Timing::new(1_780_040_700, 2_160, 2_040),
                Totals::new(1_842_000, 438, 96, 91),
                Speeds::new(9_029, 13_400),
                21,
            ),
            Self::EnduranceRide => ActivityDefinition::new(
                typedef::Sport::CYCLING,
                Timing::new(1_780_214_400, 7_560, 7_200),
                Totals::new(6_742_000, 1_642, 612, 608),
                Speeds::new(9_364, 17_200),
                22,
            ),
            Self::RecoveryRide => ActivityDefinition::new(
                typedef::Sport::CYCLING,
                Timing::new(1_780_423_800, 3_360, 3_180),
                Totals::new(2_706_000, 621, 118, 116),
                Speeds::new(8_509, 12_100),
                23,
            ),
        }
    }
}

/// Fixture encoding failure.
#[derive(Debug, Error)]
#[error("synthetic FIT encoding failed: {0}")]
pub struct EncodingError(String);

/// Encodes one valid activity sequence.
/// # Errors
/// [`EncodingError`] when `rustyfit` rejects the generated messages.
pub fn activity(sport: Sport) -> Result<Vec<u8>, EncodingError> {
    synthetic_activity(fit_sport(sport), true, typedef::File::ACTIVITY)
}

#[derive(Clone, Copy)]
struct ActivityDefinition {
    sport: typedef::Sport,
    timing: Timing,
    totals: Totals,
    speeds: Speeds,
    position_seed: i32,
}

#[derive(Clone, Copy)]
struct Timing {
    start: i64,
    elapsed_seconds: u32,
    timer_seconds: u32,
}

impl Timing {
    const fn new(start: i64, elapsed_seconds: u32, timer_seconds: u32) -> Self {
        Self {
            start,
            elapsed_seconds,
            timer_seconds,
        }
    }
}

#[derive(Clone, Copy)]
struct Totals {
    distance_centimeters: u32,
    calories: u16,
    ascent: u16,
    descent: u16,
}

impl Totals {
    const fn new(distance_centimeters: u32, calories: u16, ascent: u16, descent: u16) -> Self {
        Self {
            distance_centimeters,
            calories,
            ascent,
            descent,
        }
    }
}

#[derive(Clone, Copy)]
struct Speeds {
    average_speed: u16,
    maximum_speed: u16,
}

impl Speeds {
    const fn new(average_speed: u16, maximum_speed: u16) -> Self {
        Self {
            average_speed,
            maximum_speed,
        }
    }
}

impl ActivityDefinition {
    const fn new(
        sport: typedef::Sport,
        timing: Timing,
        totals: Totals,
        speeds: Speeds,
        position_seed: i32,
    ) -> Self {
        Self {
            sport,
            timing,
            totals,
            speeds,
            position_seed,
        }
    }
}

/// Encodes activity, settings, and activity sequences in order.
/// # Errors
/// [`EncodingError`] when `rustyfit` rejects any generated sequence.
pub fn activity_settings_activity() -> Result<Vec<u8>, EncodingError> {
    let mut bytes = synthetic_activity(typedef::Sport::RUNNING, true, typedef::File::ACTIVITY)?;
    bytes.extend(synthetic_file(typedef::File::SETTINGS)?);
    bytes.extend(synthetic_activity(
        typedef::Sport::CYCLING,
        false,
        typedef::File::ACTIVITY,
    )?);
    Ok(bytes)
}

const fn fit_sport(sport: Sport) -> typedef::Sport {
    match sport {
        Sport::Running => typedef::Sport::RUNNING,
        Sport::Cycling => typedef::Sport::CYCLING,
    }
}

pub(crate) fn synthetic_file(file_type: typedef::File) -> Result<Vec<u8>, EncodingError> {
    let mut file_id = mesgdef::FileId::new();
    file_id.r#type = file_type;
    file_id.manufacturer = typedef::Manufacturer::DEVELOPMENT;
    file_id.time_created = typedef::DateTime::from_unix_timestamp(START);
    encode(FIT {
        messages: vec![Message::from(file_id)],
        ..FIT::default()
    })
}

pub(crate) fn synthetic_activity(
    sport: typedef::Sport,
    detailed_measurements: bool,
    file_type: typedef::File,
) -> Result<Vec<u8>, EncodingError> {
    encode_activity_with_file_type(
        ActivityDefinition::new(
            sport,
            Timing::new(START, 30, 25),
            Totals::new(12_345, 12, 3, 2),
            Speeds::new(
                if detailed_measurements { 3_000 } else { 2_000 },
                if detailed_measurements { 4_000 } else { 3_000 },
            ),
            10,
        ),
        detailed_measurements,
        file_type,
    )
}

fn encode_activity(definition: ActivityDefinition) -> Result<Vec<u8>, EncodingError> {
    encode_activity_with_file_type(definition, true, typedef::File::ACTIVITY)
}

fn encode_activity_with_file_type(
    definition: ActivityDefinition,
    detailed_measurements: bool,
    file_type: typedef::File,
) -> Result<Vec<u8>, EncodingError> {
    let [started, stopped] = timer_events(definition.timing);
    let [first_record, second_record] = records(definition, detailed_measurements);
    let [first_lap, second_lap] = laps(definition);
    let mut file_id = mesgdef::FileId::new();
    file_id.r#type = file_type;
    file_id.manufacturer = typedef::Manufacturer::DEVELOPMENT;
    file_id.product = 42;
    file_id.serial_number = 1_234;
    "Synthetic tracker".clone_into(&mut file_id.product_name);
    file_id.time_created = typedef::DateTime::from_unix_timestamp(definition.timing.start);

    let mut creator = mesgdef::DeviceInfo::new();
    creator.device_index = typedef::DeviceIndex::CREATOR;
    creator.manufacturer = typedef::Manufacturer::DEVELOPMENT;
    creator.product = 42;
    creator.serial_number = 1_234;
    creator.software_version = 123;

    encode(FIT {
        messages: vec![
            Message::from(file_id),
            Message::from(creator),
            Message::from(started),
            Message::from(first_record),
            Message::from(second_record),
            Message::from(first_lap),
            Message::from(second_lap),
            Message::from(session(definition, detailed_measurements)),
            Message::from(stopped),
            Message::from(activity_record(definition.timing)),
        ],
        ..FIT::default()
    })
}

fn session(definition: ActivityDefinition, detailed: bool) -> mesgdef::Session {
    let mut session = mesgdef::Session::new();
    session.start_time = typedef::DateTime::from_unix_timestamp(definition.timing.start);
    session.timestamp = typedef::DateTime::from_unix_timestamp(end(definition.timing));
    session.sport = definition.sport;
    session.sub_sport = typedef::SubSport::GENERIC;
    session.total_elapsed_time = milliseconds(definition.timing.elapsed_seconds);
    session.total_timer_time = milliseconds(definition.timing.timer_seconds);
    session.total_distance = definition.totals.distance_centimeters;
    session.total_calories = definition.totals.calories;
    session.total_ascent = definition.totals.ascent;
    session.total_descent = definition.totals.descent;
    session.avg_heart_rate = 120;
    session.max_heart_rate = 140;
    session.num_laps = 2;
    if detailed {
        session.enhanced_avg_speed = u32::from(definition.speeds.average_speed);
        session.enhanced_max_speed = u32::from(definition.speeds.maximum_speed);
    } else {
        session.avg_speed = definition.speeds.average_speed;
        session.max_speed = definition.speeds.maximum_speed;
    }
    session
}

fn laps(definition: ActivityDefinition) -> [mesgdef::Lap; 2] {
    let timing = definition.timing;
    let totals = definition.totals;
    let mut first = mesgdef::Lap::new();
    first.start_time = typedef::DateTime::from_unix_timestamp(timing.start);
    first.total_elapsed_time = milliseconds(timing.elapsed_seconds) / 2;
    first.timestamp = typedef::DateTime::from_unix_timestamp(
        timing.start + i64::from(timing.elapsed_seconds / 2),
    );
    first.total_timer_time = milliseconds(timing.timer_seconds) / 2;
    first.total_distance = totals.distance_centimeters / 2;
    first.total_calories = totals.calories / 2;
    first.total_ascent = totals.ascent / 2;
    first.total_descent = totals.descent / 2;
    first.avg_heart_rate = 115;
    first.max_heart_rate = 135;
    first.avg_speed = definition.speeds.average_speed;
    first.max_speed = definition.speeds.maximum_speed;

    let mut second = first.clone();
    second.start_time = first.timestamp;
    second.timestamp = typedef::DateTime::from_unix_timestamp(end(timing));
    second.total_elapsed_time = milliseconds(timing.elapsed_seconds) - first.total_elapsed_time;
    second.total_timer_time = milliseconds(timing.timer_seconds) - first.total_timer_time;
    second.total_distance = totals.distance_centimeters - first.total_distance;
    second.total_calories = totals.calories - first.total_calories;
    second.total_ascent = totals.ascent - first.total_ascent;
    second.total_descent = totals.descent - first.total_descent;
    [first, second]
}

fn records(definition: ActivityDefinition, detailed: bool) -> [mesgdef::Record; 2] {
    let mut first = mesgdef::Record::new();
    first.timestamp = typedef::DateTime::from_unix_timestamp(definition.timing.start);
    first.position_lat = definition.position_seed.saturating_mul(100_000);
    first.position_long = definition.position_seed.saturating_mul(-50_000);
    first.enhanced_altitude = 2_750;
    first.heart_rate = 123;
    first.distance = 0;
    if detailed {
        first.enhanced_speed = u32::from(definition.speeds.average_speed);
        first.cadence = 81;
        first.fractional_cadence = 64;
        first.power = 240;
    }

    let mut second = first.clone();
    second.timestamp = typedef::DateTime::from_unix_timestamp(end(definition.timing));
    second.position_lat = first.position_lat.saturating_add(5_000);
    second.position_long = first.position_long.saturating_add(5_000);
    second.distance = definition.totals.distance_centimeters;
    [first, second]
}

fn timer_events(timing: Timing) -> [mesgdef::Event; 2] {
    let mut started = mesgdef::Event::new();
    started.timestamp = typedef::DateTime::from_unix_timestamp(timing.start);
    started.event = typedef::Event::TIMER;
    started.event_type = typedef::EventType::START;

    let mut stopped = mesgdef::Event::new();
    stopped.timestamp = typedef::DateTime::from_unix_timestamp(end(timing));
    stopped.event = typedef::Event::TIMER;
    stopped.event_type = typedef::EventType::STOP_ALL;
    [started, stopped]
}

fn activity_record(timing: Timing) -> mesgdef::Activity {
    let mut activity = mesgdef::Activity::new();
    activity.timestamp = typedef::DateTime::from_unix_timestamp(end(timing));
    activity.total_timer_time = milliseconds(timing.timer_seconds);
    activity.num_sessions = 1;
    activity
}

fn end(timing: Timing) -> i64 {
    timing.start + i64::from(timing.elapsed_seconds)
}

const fn milliseconds(seconds: u32) -> u32 {
    seconds.saturating_mul(1_000)
}

pub(crate) fn encode(mut fit: FIT) -> Result<Vec<u8>, EncodingError> {
    let mut output = FromStd::new(Cursor::new(Vec::new()));
    Encoder::builder()
        .protocol_version(ProtocolVersion::V1)
        .build()
        .encode(&mut output, &mut fit)
        .map_err(|error| EncodingError(error.to_string()))?;
    Ok(output.into_inner().into_inner())
}
