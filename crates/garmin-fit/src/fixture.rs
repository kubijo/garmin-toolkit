//! Deterministic publishable FIT data for demos and cross-crate tests.

use std::{io::Cursor, string::ToString};

use embedded_io_adapters::std::FromStd;
use rustyfit::{
    Encoder,
    profile::{mesgdef, typedef},
    proto::{FIT, Message, ProtocolVersion},
};
use thiserror::Error;

mod source;

pub(crate) const START: i64 = 1_780_000_000;
#[cfg(test)]
pub(crate) const END: i64 = START + 30;

/// Generated activity sport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sport {
    Running,
    Cycling,
    Swimming,
}

/// Publishable activity cases used by mock deployments and tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityCase {
    ForestRun,
    CoastalRun,
    CityRun,
    CityRide,
    OpenWaterSwim,
    IndoorPowerRide,
}

impl ActivityCase {
    /// Complete public demo corpus.
    pub const ALL: [Self; 6] = [
        Self::ForestRun,
        Self::CoastalRun,
        Self::CityRun,
        Self::CityRide,
        Self::OpenWaterSwim,
        Self::IndoorPowerRide,
    ];

    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::ForestRun => "2026-09-05-forest-run.fit",
            Self::CoastalRun => "2026-09-07-coastal-run.fit",
            Self::CityRun => "2026-09-10-city-run.fit",
            Self::CityRide => "2026-09-12-city-ride.fit",
            Self::OpenWaterSwim => "2026-09-14-open-water-swim.fit",
            Self::IndoorPowerRide => "2026-09-09-indoor-power-ride.fit",
        }
    }

    #[must_use]
    pub const fn start(self) -> i64 {
        match self {
            Self::ForestRun => 1_788_593_400,
            Self::CoastalRun => 1_788_803_100,
            Self::CityRun => 1_789_022_400,
            Self::CityRide => 1_789_226_100,
            Self::OpenWaterSwim => 1_789_396_200,
            Self::IndoorPowerRide => 1_788_948_000,
        }
    }

    #[must_use]
    pub fn end(self) -> i64 {
        self.start() + i64::from(self.duration_seconds())
    }

    /// Encodes this case as a complete FIT file.
    /// # Errors
    /// [`EncodingError`] when `rustyfit` rejects the generated messages.
    pub fn encode(self) -> Result<Vec<u8>, EncodingError> {
        encode_recorded_activity(self)
    }

    const fn sport(self) -> typedef::Sport {
        match self {
            Self::ForestRun | Self::CoastalRun | Self::CityRun => typedef::Sport::RUNNING,
            Self::CityRide | Self::IndoorPowerRide => typedef::Sport::CYCLING,
            Self::OpenWaterSwim => typedef::Sport::SWIMMING,
        }
    }

    const fn duration_seconds(self) -> u32 {
        match self {
            Self::ForestRun => 3_931,
            Self::CoastalRun => 1_185,
            Self::CityRun => 5_234,
            Self::CityRide => 11_963,
            Self::OpenWaterSwim => 2_085,
            Self::IndoorPowerRide => 3_600,
        }
    }
}

/// Fixture encoding failure.
#[derive(Debug, Error)]
#[error("fixture FIT encoding failed: {0}")]
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
        Sport::Swimming => typedef::Sport::SWIMMING,
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

fn encode_recorded_activity(case: ActivityCase) -> Result<Vec<u8>, EncodingError> {
    let recording = source::load(case)?;
    if recording.duration_seconds() != case.duration_seconds() {
        return Err(EncodingError(format!(
            "{}: recording duration does not match its fixture declaration",
            case.file_name()
        )));
    }

    let mut file_id = mesgdef::FileId::new();
    file_id.r#type = typedef::File::ACTIVITY;
    file_id.manufacturer = typedef::Manufacturer::DEVELOPMENT;
    file_id.product = 42;
    file_id.serial_number = 1_234;
    "Demo Track Recorder".clone_into(&mut file_id.product_name);
    file_id.time_created = typedef::DateTime::from_unix_timestamp(case.start());

    let mut creator = mesgdef::DeviceInfo::new();
    creator.device_index = typedef::DeviceIndex::CREATOR;
    creator.manufacturer = typedef::Manufacturer::DEVELOPMENT;
    creator.product = 42;
    creator.serial_number = 1_234;
    creator.software_version = 200;

    let timing = Timing::new(
        case.start(),
        case.duration_seconds(),
        case.duration_seconds(),
    );
    let [started, stopped] = timer_events(timing);
    let lap_ranges = lap_ranges(&recording.samples);
    let mut messages = Vec::with_capacity(recording.samples.len() + lap_ranges.len() + 6);
    messages.extend([
        Message::from(file_id),
        Message::from(creator),
        Message::from(started),
    ]);
    messages.extend(
        recording
            .samples
            .iter()
            .map(|sample| Message::from(record(case.start(), sample))),
    );
    messages.extend(lap_ranges.iter().map(|&(start, end)| {
        Message::from(recorded_lap(
            case.start(),
            case.sport(),
            &recording.samples[start..=end],
        ))
    }));
    messages.extend([
        Message::from(recorded_session(
            case.start(),
            case.sport(),
            &recording.samples,
            lap_ranges.len(),
        )?),
        Message::from(stopped),
        Message::from(activity_record(timing)),
    ]);
    encode(FIT {
        messages,
        ..FIT::default()
    })
}

fn lap_ranges(samples: &[source::Sample]) -> Vec<(usize, usize)> {
    let mut start = 0;
    let mut ranges = Vec::new();
    for (end, sample) in samples.iter().enumerate() {
        if sample.lap_end {
            ranges.push((start, end));
            start = end.saturating_add(1);
        }
    }
    ranges
}

fn record(start: i64, sample: &source::Sample) -> mesgdef::Record {
    let mut record = mesgdef::Record::new();
    record.timestamp = sample_time(start, sample.elapsed_seconds);
    if let (Some(latitude), Some(longitude)) = (sample.latitude_degrees, sample.longitude_degrees) {
        record.set_position_lat_degrees(latitude);
        record.set_position_long_degrees(longitude);
    }
    if let Some(elevation) = sample.elevation_meters {
        record.set_enhanced_altitude_scaled(elevation);
    }
    if let Some(distance) = sample.distance_meters {
        record.set_distance_scaled(distance);
    }
    if let Some(speed) = sample.speed_meters_per_second {
        record.set_enhanced_speed_scaled(speed);
    }
    if let Some(heart_rate) = sample.heart_rate_bpm {
        record.heart_rate = heart_rate;
    }
    if let Some(cadence) = sample.cadence_rpm {
        record.cadence = cadence;
    }
    if let Some(power) = sample.power_watts {
        record.power = power;
    }
    if let Some(temperature) = sample.temperature_celsius {
        record.temperature = temperature;
    }
    record
}

fn recorded_lap(start: i64, sport: typedef::Sport, samples: &[source::Sample]) -> mesgdef::Lap {
    let first = &samples[0];
    let last = &samples[samples.len() - 1];
    let mut lap = mesgdef::Lap::new();
    lap.start_time = sample_time(start, first.elapsed_seconds);
    lap.timestamp = sample_time(start, last.elapsed_seconds);
    lap.sport = sport;
    lap.sub_sport = typedef::SubSport::GENERIC;
    apply_positions_to_lap(&mut lap, first, last);
    apply_lap_summary(&mut lap, samples);
    lap
}

fn recorded_session(
    start: i64,
    sport: typedef::Sport,
    samples: &[source::Sample],
    lap_count: usize,
) -> Result<mesgdef::Session, EncodingError> {
    let first = &samples[0];
    let last = &samples[samples.len() - 1];
    let mut session = mesgdef::Session::new();
    session.start_time = sample_time(start, first.elapsed_seconds);
    session.timestamp = sample_time(start, last.elapsed_seconds);
    session.sport = sport;
    session.sub_sport = typedef::SubSport::GENERIC;
    session.num_laps = u16::try_from(lap_count)
        .map_err(|_| EncodingError("recording has too many laps".to_owned()))?;
    apply_positions_to_session(&mut session, first, last);
    apply_session_summary(&mut session, samples);
    Ok(session)
}

fn apply_lap_summary(lap: &mut mesgdef::Lap, samples: &[source::Sample]) {
    let summary = SampleSummary::new(samples);
    lap.set_total_elapsed_time_scaled(summary.elapsed_seconds);
    lap.set_total_timer_time_scaled(summary.elapsed_seconds);
    if let Some(distance) = summary.distance_meters {
        lap.set_total_distance_scaled(distance);
    }
    if let Some(speed) = summary.average_speed {
        lap.set_enhanced_avg_speed_scaled(speed);
    }
    if let Some(speed) = summary.maximum_speed {
        lap.set_enhanced_max_speed_scaled(speed);
    }
    lap.apply_common_summary(summary);
}

fn apply_session_summary(session: &mut mesgdef::Session, samples: &[source::Sample]) {
    let summary = SampleSummary::new(samples);
    session.set_total_elapsed_time_scaled(summary.elapsed_seconds);
    session.set_total_timer_time_scaled(summary.elapsed_seconds);
    if let Some(distance) = samples.last().and_then(|sample| sample.distance_meters) {
        session.set_total_distance_scaled(distance);
        if summary.elapsed_seconds > 0.0 {
            session.set_enhanced_avg_speed_scaled(distance / summary.elapsed_seconds);
        }
    }
    if let Some(speed) = summary.maximum_speed {
        session.set_enhanced_max_speed_scaled(speed);
    }
    session.apply_common_summary(summary);
}

trait CommonSummaryTarget {
    fn apply_common_summary(&mut self, summary: SampleSummary);
}

macro_rules! impl_common_summary_target {
    ($target:ty) => {
        impl CommonSummaryTarget for $target {
            fn apply_common_summary(&mut self, summary: SampleSummary) {
                if let Some(value) = summary.average_heart_rate {
                    self.avg_heart_rate = value;
                }
                if let Some(value) = summary.maximum_heart_rate {
                    self.max_heart_rate = value;
                }
                if let Some(value) = summary.average_cadence {
                    self.avg_cadence = value;
                }
                if let Some(value) = summary.maximum_cadence {
                    self.max_cadence = value;
                }
                if let Some(value) = summary.average_power {
                    self.avg_power = value;
                }
                if let Some(value) = summary.maximum_power {
                    self.max_power = value;
                }
                if let Some(value) = summary.total_ascent {
                    self.total_ascent = value;
                }
                if let Some(value) = summary.total_descent {
                    self.total_descent = value;
                }
            }
        }
    };
}

impl_common_summary_target!(mesgdef::Lap);
impl_common_summary_target!(mesgdef::Session);

#[derive(Clone, Copy)]
struct SampleSummary {
    elapsed_seconds: f64,
    distance_meters: Option<f64>,
    average_speed: Option<f64>,
    maximum_speed: Option<f64>,
    average_heart_rate: Option<u8>,
    maximum_heart_rate: Option<u8>,
    average_cadence: Option<u8>,
    maximum_cadence: Option<u8>,
    average_power: Option<u16>,
    maximum_power: Option<u16>,
    total_ascent: Option<u16>,
    total_descent: Option<u16>,
}

impl SampleSummary {
    fn new(samples: &[source::Sample]) -> Self {
        let first = &samples[0];
        let last = &samples[samples.len() - 1];
        let elapsed_seconds = f64::from(last.elapsed_seconds - first.elapsed_seconds);
        let distance_meters = first
            .distance_meters
            .zip(last.distance_meters)
            .map(|(first, last)| (last - first).max(0.0));
        let average_speed = distance_meters
            .filter(|_| elapsed_seconds > 0.0)
            .map(|distance| distance / elapsed_seconds);
        let (total_ascent, total_descent) = ascent_and_descent(samples);
        Self {
            elapsed_seconds,
            distance_meters,
            average_speed,
            maximum_speed: maximum_f64(samples, |sample| sample.speed_meters_per_second),
            average_heart_rate: average_u8(samples, |sample| sample.heart_rate_bpm),
            maximum_heart_rate: maximum_u8(samples, |sample| sample.heart_rate_bpm),
            average_cadence: average_u8(samples, |sample| sample.cadence_rpm),
            maximum_cadence: maximum_u8(samples, |sample| sample.cadence_rpm),
            average_power: average_u16(samples, |sample| sample.power_watts),
            maximum_power: maximum_u16(samples, |sample| sample.power_watts),
            total_ascent,
            total_descent,
        }
    }
}

fn average_u8(
    samples: &[source::Sample],
    value: impl Fn(&source::Sample) -> Option<u8>,
) -> Option<u8> {
    let values = samples.iter().filter_map(value).map(u64::from);
    rounded_average(values).and_then(|value| u8::try_from(value).ok())
}

fn average_u16(
    samples: &[source::Sample],
    value: impl Fn(&source::Sample) -> Option<u16>,
) -> Option<u16> {
    let values = samples.iter().filter_map(value).map(u64::from);
    rounded_average(values).and_then(|value| u16::try_from(value).ok())
}

fn rounded_average(values: impl Iterator<Item = u64>) -> Option<u64> {
    let (sum, count) = values.fold((0_u64, 0_u64), |(sum, count), value| {
        (sum.saturating_add(value), count + 1)
    });
    (count > 0).then(|| sum.saturating_add(count / 2) / count)
}

fn maximum_u8(
    samples: &[source::Sample],
    value: impl Fn(&source::Sample) -> Option<u8>,
) -> Option<u8> {
    samples.iter().filter_map(value).max()
}

fn maximum_u16(
    samples: &[source::Sample],
    value: impl Fn(&source::Sample) -> Option<u16>,
) -> Option<u16> {
    samples.iter().filter_map(value).max()
}

fn maximum_f64(
    samples: &[source::Sample],
    value: impl Fn(&source::Sample) -> Option<f64>,
) -> Option<f64> {
    samples.iter().filter_map(value).reduce(f64::max)
}

fn ascent_and_descent(samples: &[source::Sample]) -> (Option<u16>, Option<u16>) {
    let mut ascent = 0.0_f64;
    let mut descent = 0.0_f64;
    let mut segments = 0_u32;
    for pair in samples.windows(2) {
        if let (Some(first), Some(second)) = (pair[0].elevation_meters, pair[1].elevation_meters) {
            let change = second - first;
            if change > 0.0 {
                ascent += change;
            } else {
                descent -= change;
            }
            segments += 1;
        }
    }
    if segments == 0 {
        (None, None)
    } else {
        (
            Some(saturating_rounded_u16(ascent)),
            Some(saturating_rounded_u16(descent)),
        )
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the preceding bounds check makes the rounded non-negative value fit in u16"
)]
fn saturating_rounded_u16(value: f64) -> u16 {
    if value >= f64::from(u16::MAX) {
        u16::MAX - 1
    } else {
        value.round() as u16
    }
}

fn apply_positions_to_lap(lap: &mut mesgdef::Lap, first: &source::Sample, last: &source::Sample) {
    if let (Some(latitude), Some(longitude)) = (first.latitude_degrees, first.longitude_degrees) {
        lap.set_start_position_lat_degrees(latitude);
        lap.set_start_position_long_degrees(longitude);
    }
    if let (Some(latitude), Some(longitude)) = (last.latitude_degrees, last.longitude_degrees) {
        lap.set_end_position_lat_degrees(latitude);
        lap.set_end_position_long_degrees(longitude);
    }
}

fn apply_positions_to_session(
    session: &mut mesgdef::Session,
    first: &source::Sample,
    last: &source::Sample,
) {
    if let (Some(latitude), Some(longitude)) = (first.latitude_degrees, first.longitude_degrees) {
        session.set_start_position_lat_degrees(latitude);
        session.set_start_position_long_degrees(longitude);
    }
    if let (Some(latitude), Some(longitude)) = (last.latitude_degrees, last.longitude_degrees) {
        session.set_end_position_lat_degrees(latitude);
        session.set_end_position_long_degrees(longitude);
    }
}

fn sample_time(start: i64, elapsed_seconds: u32) -> typedef::DateTime {
    typedef::DateTime::from_unix_timestamp(start + i64::from(elapsed_seconds))
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
    "Mock Track-o-Matic 9000".clone_into(&mut file_id.product_name);
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
