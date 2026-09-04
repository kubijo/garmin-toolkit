//! FIT activity normalization without parser types.

use std::{fmt, io::Cursor, num::NonZeroU32};

use embedded_io_adapters::std::FromStd;
use garmin_model::{
    activity::{
        Activity, ActivityDuration, ActivityMetric, ActivityMetrics, ActivitySport,
        ActivitySummary, ActivityTotals, Cadence, Distance, Energy, HeartRate, Lap, Power, Speed,
        Temperature, TimeRange, TimerEvent, TimerState, TrackMeasurements, TrackPoint,
    },
    route::{Coordinate, Elevation, Latitude, Longitude},
    value::Timestamp,
};
use rustyfit::{
    Decoder,
    profile::{mesgdef, typedef},
    proto::FIT,
};
use thiserror::Error;

pub mod course;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixture;

/// Persisted normalizer name.
pub const NORMALIZER_NAME: &str = "garmin-fit";
/// Persisted normalizer version.
pub const NORMALIZER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Persisted projection schema version.
pub const NORMALIZATION_SCHEMA_VERSION: u32 = 1;

/// A FIT decode failure.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("FIT decoder rejected the input: {0}")]
    Rejected(String),
    #[error("FIT input did not contain a complete sequence")]
    Incomplete,
}

/// A FIT file type without exposing parser types.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileType(u8);

impl FileType {
    /// Activity file type.
    pub const ACTIVITY: Self = Self(typedef::File::ACTIVITY.0);
    /// Course file type.
    pub const COURSE: Self = Self(typedef::File::COURSE.0);
    /// Settings file type.
    pub const SETTINGS: Self = Self(typedef::File::SETTINGS.0);

    #[must_use]
    pub const fn from_u8(value: u8) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u8(&self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn into_u8(self) -> u8 {
        self.0
    }
}

impl fmt::Display for FileType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        typedef::File(self.0).fmt(formatter)
    }
}

/// A zero-based sequence position in one FIT artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SequencePosition(u32);

impl SequencePosition {
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u32(&self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn into_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SequencePosition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A FIT profile manufacturer identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ManufacturerId(u16);

impl ManufacturerId {
    /// Validates a raw FIT manufacturer identifier.
    /// # Errors
    /// [`CreatorError::InvalidManufacturer`] for FIT's invalid sentinel.
    pub const fn from_u16(value: u16) -> Result<Self, CreatorError> {
        if value == u16::MAX {
            Err(CreatorError::InvalidManufacturer)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn as_u16(&self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn into_u16(self) -> u16 {
        self.0
    }
}

impl fmt::Display for ManufacturerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        typedef::Manufacturer(self.0).fmt(formatter)
    }
}

/// A manufacturer-scoped FIT product identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProductId(u16);

impl ProductId {
    /// Validates a raw FIT product identifier.
    /// # Errors
    /// [`CreatorError::InvalidProduct`] for FIT's invalid sentinel.
    pub const fn from_u16(value: u16) -> Result<Self, CreatorError> {
        if value == u16::MAX {
            Err(CreatorError::InvalidProduct)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn as_u16(&self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn into_u16(self) -> u16 {
        self.0
    }
}

impl fmt::Display for ProductId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A non-zero FIT device serial number.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SerialNumber(NonZeroU32);

impl SerialNumber {
    /// Validates a raw FIT serial number.
    /// # Errors
    /// [`CreatorError::InvalidSerialNumber`] for FIT's invalid zero sentinel.
    pub const fn from_u32(value: u32) -> Result<Self, CreatorError> {
        match NonZeroU32::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(CreatorError::InvalidSerialNumber),
        }
    }

    #[must_use]
    pub const fn as_u32(&self) -> u32 {
        self.0.get()
    }

    #[must_use]
    pub const fn into_u32(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for SerialNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A non-empty FIT product name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProductName(String);

impl ProductName {
    /// Trims and validates a product name.
    /// # Errors
    /// [`CreatorError::EmptyProductName`] when no visible text remains.
    pub fn from_string(value: String) -> Result<Self, CreatorError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(CreatorError::EmptyProductName);
        }
        if trimmed.len() == value.len() {
            Ok(Self(value))
        } else {
            Ok(Self(trimmed.to_owned()))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ProductName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A FIT `device_info.software_version` field scaled to hundredths, not `SemVer`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SoftwareVersion(u16);

impl SoftwareVersion {
    /// Validates raw version hundredths.
    /// # Errors
    /// [`CreatorError::InvalidSoftwareVersion`] for FIT's invalid sentinel.
    pub const fn from_hundredths(value: u16) -> Result<Self, CreatorError> {
        if value == u16::MAX {
            Err(CreatorError::InvalidSoftwareVersion)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn as_hundredths(&self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn into_hundredths(self) -> u16 {
        self.0
    }
}

impl fmt::Display for SoftwareVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{:02}", self.0 / 100, self.0 % 100)
    }
}

/// Source-reported FIT file creator diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorDiagnostics {
    manufacturer: ManufacturerId,
    product: Option<ProductId>,
    serial_number: Option<SerialNumber>,
    product_name: Option<ProductName>,
    software_version: Option<SoftwareVersion>,
}

impl CreatorDiagnostics {
    #[must_use]
    pub const fn from_parts(
        manufacturer: ManufacturerId,
        product: Option<ProductId>,
        serial_number: Option<SerialNumber>,
        product_name: Option<ProductName>,
        software_version: Option<SoftwareVersion>,
    ) -> Self {
        Self {
            manufacturer,
            product,
            serial_number,
            product_name,
            software_version,
        }
    }

    #[must_use]
    pub const fn manufacturer(&self) -> ManufacturerId {
        self.manufacturer
    }

    #[must_use]
    pub const fn product(&self) -> Option<ProductId> {
        self.product
    }

    #[must_use]
    pub const fn serial_number(&self) -> Option<SerialNumber> {
        self.serial_number
    }

    #[must_use]
    pub const fn product_name(&self) -> Option<&ProductName> {
        self.product_name.as_ref()
    }

    #[must_use]
    pub const fn software_version(&self) -> Option<SoftwareVersion> {
        self.software_version
    }
}

/// An invalid FIT creator diagnostic value.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CreatorError {
    #[error("FIT manufacturer identifier is invalid")]
    InvalidManufacturer,
    #[error("FIT product identifier is invalid")]
    InvalidProduct,
    #[error("FIT serial number is invalid")]
    InvalidSerialNumber,
    #[error("FIT product name cannot be blank")]
    EmptyProductName,
    #[error("FIT software version is invalid")]
    InvalidSoftwareVersion,
}

/// A normalized activity and its source creator diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedActivity {
    creator: CreatorDiagnostics,
    activity: Activity,
}

impl NormalizedActivity {
    #[must_use]
    pub const fn from_parts(creator: CreatorDiagnostics, activity: Activity) -> Self {
        Self { creator, activity }
    }

    #[must_use]
    pub const fn creator(&self) -> &CreatorDiagnostics {
        &self.creator
    }

    #[must_use]
    pub const fn activity(&self) -> &Activity {
        &self.activity
    }

    #[must_use]
    pub fn into_parts(self) -> (CreatorDiagnostics, Activity) {
        (self.creator, self.activity)
    }
}

/// One classified sequence in a FIT activity import.
#[derive(Clone, Debug, PartialEq)]
pub enum ActivitySequence {
    Activity(Box<NormalizedActivity>),
    Preserved(FileType),
}

impl ActivitySequence {
    #[must_use]
    pub const fn as_activity(&self) -> Option<&Activity> {
        match self {
            Self::Activity(activity) => Some(activity.activity()),
            Self::Preserved(_) => None,
        }
    }

    #[must_use]
    pub const fn as_normalized_activity(&self) -> Option<&NormalizedActivity> {
        match self {
            Self::Activity(activity) => Some(activity),
            Self::Preserved(_) => None,
        }
    }

    #[must_use]
    pub const fn preserved_file_type(&self) -> Option<FileType> {
        match self {
            Self::Activity(_) => None,
            Self::Preserved(file_type) => Some(*file_type),
        }
    }
}

/// All classified sequences from one immutable FIT artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityImport {
    sequences: Vec<ActivitySequence>,
    positions: Vec<SequencePosition>,
}

impl ActivityImport {
    #[must_use]
    pub fn sequences(&self) -> &[ActivitySequence] {
        &self.sequences
    }

    /// Iterates over normalized activities in source order.
    pub fn activities(&self) -> impl Iterator<Item = &Activity> {
        self.sequences
            .iter()
            .filter_map(ActivitySequence::as_activity)
    }

    /// Iterates over normalized activities with creator diagnostics in source order.
    pub fn normalized_activities(&self) -> impl Iterator<Item = &NormalizedActivity> {
        self.sequences
            .iter()
            .filter_map(ActivitySequence::as_normalized_activity)
    }

    /// Iterates over normalized activities with their source sequence positions.
    pub fn positioned_activities(
        &self,
    ) -> impl Iterator<Item = (SequencePosition, &NormalizedActivity)> {
        self.positions
            .iter()
            .copied()
            .zip(self.sequences.iter())
            .filter_map(|(position, sequence)| {
                sequence
                    .as_normalized_activity()
                    .map(|activity| (position, activity))
            })
    }

    #[must_use]
    pub fn into_sequences(self) -> Vec<ActivitySequence> {
        self.sequences
    }
}

/// An activity normalization failure.
#[derive(Debug, Error)]
pub enum ActivityError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("FIT file type {0} is unsupported in an activity import")]
    UnsupportedFileType(FileType),
    #[error("FIT input contained no activity sequence")]
    NoActivities,
    #[error("FIT input contained too many chained sequences")]
    TooManySequences,
    #[error("FIT activity requires {0}")]
    MissingField(&'static str),
    #[error("FIT sport {0} is unsupported")]
    UnsupportedSport(u8),
    #[error("FIT record contained a partial coordinate")]
    PartialCoordinate,
    #[error("FIT {name} expected {expected}; found {found}")]
    CountMismatch {
        /// Count name.
        name: &'static str,
        /// Required or declared count.
        expected: usize,
        /// Observed count.
        found: usize,
    },
    #[error("FIT {field} was invalid: {reason}")]
    InvalidValue {
        /// FIT field name.
        field: &'static str,
        /// Validation failure.
        reason: String,
    },
    #[error("normalized activity was invalid: {0}")]
    Activity(#[from] garmin_model::activity::Error),
    #[error(transparent)]
    Creator(#[from] CreatorError),
    #[error("FIT creator declarations disagree about {0}")]
    ConflictingCreatorField(&'static str),
}

/// Classifies every sequence and normalizes single-session running or cycling activities.
///
/// Unknown data remains in the caller-preserved original FIT artifact.
///
/// See Garmin's [activity-file](https://developer.garmin.com/fit/file-types/activity/)
/// and [decoding](https://developer.garmin.com/fit/cookbook/decoding-activity-files/) guidance.
/// # Errors
/// [`ActivityError`] when any sequence is malformed, contradictory, or unsupported.
pub fn normalize_activities(bytes: &[u8]) -> Result<ActivityImport, ActivityError> {
    let decoded = decode_raw(bytes)?;
    let mut sequences = Vec::with_capacity(decoded.len());
    let mut positions = Vec::with_capacity(decoded.len());
    for (position, fit) in decoded.iter().enumerate() {
        positions.push(SequencePosition::from_u32(
            u32::try_from(position).map_err(|_| ActivityError::TooManySequences)?,
        ));
        sequences.push(classify_sequence(fit)?);
    }
    if !sequences
        .iter()
        .any(|sequence| matches!(sequence, ActivitySequence::Activity(_)))
    {
        return Err(ActivityError::NoActivities);
    }
    Ok(ActivityImport {
        sequences,
        positions,
    })
}

fn classify_sequence(fit: &FIT) -> Result<ActivitySequence, ActivityError> {
    let file_id = file_id(fit)?;
    match file_id.r#type {
        typedef::File::ACTIVITY => {
            let creator = creator_diagnostics(fit, &file_id)?;
            normalize_activity(fit)
                .map(|activity| NormalizedActivity::from_parts(creator, activity))
                .map(Box::new)
                .map(ActivitySequence::Activity)
        }
        typedef::File::SETTINGS => Ok(ActivitySequence::Preserved(FileType::SETTINGS)),
        file_type => Err(ActivityError::UnsupportedFileType(FileType::from_u8(
            file_type.0,
        ))),
    }
}

fn creator_diagnostics(
    fit: &FIT,
    file_id: &mesgdef::FileId,
) -> Result<CreatorDiagnostics, ActivityError> {
    let manufacturer = ManufacturerId::from_u16(file_id.manufacturer.0)?;
    let mut product = (file_id.product != u16::MAX)
        .then(|| ProductId::from_u16(file_id.product))
        .transpose()?;
    let mut serial_number = (file_id.serial_number != u32::MIN)
        .then(|| SerialNumber::from_u32(file_id.serial_number))
        .transpose()?;
    let mut product_name = (!file_id.product_name.trim().is_empty())
        .then(|| ProductName::from_string(file_id.product_name.clone()))
        .transpose()?;
    let mut software_version = None;
    for device in fit
        .messages
        .iter()
        .filter(|message| message.num == typedef::MesgNum::DEVICE_INFO)
        .map(mesgdef::DeviceInfo::from)
        .filter(|device| device.device_index == typedef::DeviceIndex::CREATOR)
    {
        if device.manufacturer.0 != u16::MAX
            && ManufacturerId::from_u16(device.manufacturer.0)? != manufacturer
        {
            return Err(ActivityError::ConflictingCreatorField("manufacturer"));
        }
        reconcile_creator_value(
            &mut product,
            (device.product != u16::MAX)
                .then(|| ProductId::from_u16(device.product))
                .transpose()?,
            "product",
        )?;
        reconcile_creator_value(
            &mut serial_number,
            (device.serial_number != u32::MIN)
                .then(|| SerialNumber::from_u32(device.serial_number))
                .transpose()?,
            "serial number",
        )?;
        reconcile_creator_value(
            &mut product_name,
            (!device.product_name.trim().is_empty())
                .then(|| ProductName::from_string(device.product_name))
                .transpose()?,
            "product name",
        )?;
        reconcile_creator_value(
            &mut software_version,
            (device.software_version != u16::MAX)
                .then(|| SoftwareVersion::from_hundredths(device.software_version))
                .transpose()?,
            "software version",
        )?;
    }
    Ok(CreatorDiagnostics::from_parts(
        manufacturer,
        product,
        serial_number,
        product_name,
        software_version,
    ))
}

fn reconcile_creator_value<T: Eq>(
    current: &mut Option<T>,
    candidate: Option<T>,
    field: &'static str,
) -> Result<(), ActivityError> {
    match (current.as_ref(), candidate) {
        (_, None) => Ok(()),
        (None, Some(candidate)) => {
            *current = Some(candidate);
            Ok(())
        }
        (Some(current), Some(candidate)) if current == &candidate => Ok(()),
        (Some(_), Some(_)) => Err(ActivityError::ConflictingCreatorField(field)),
    }
}

fn file_id(fit: &FIT) -> Result<mesgdef::FileId, ActivityError> {
    if fit.messages.first().map(|message| message.num) != Some(typedef::MesgNum::FILE_ID) {
        return Err(ActivityError::InvalidValue {
            field: "file_id",
            reason: "message must be first".to_owned(),
        });
    }
    let file_ids = fit
        .messages
        .iter()
        .filter(|message| message.num == typedef::MesgNum::FILE_ID)
        .map(mesgdef::FileId::from)
        .collect::<Vec<_>>();
    let [file_id] = file_ids.as_slice() else {
        return Err(ActivityError::CountMismatch {
            name: "file ID count",
            expected: 1,
            found: file_ids.len(),
        });
    };
    if file_id.manufacturer.0 == u16::MAX {
        return Err(ActivityError::MissingField("file_id.manufacturer"));
    }
    Ok(file_id.clone())
}

fn normalize_activity(fit: &FIT) -> Result<Activity, ActivityError> {
    let sessions = fit
        .messages
        .iter()
        .filter(|message| message.num == typedef::MesgNum::SESSION)
        .map(mesgdef::Session::from)
        .collect::<Vec<_>>();
    let [session] = sessions.as_slice() else {
        return Err(ActivityError::CountMismatch {
            name: "session count",
            expected: 1,
            found: sessions.len(),
        });
    };
    let activity_messages = fit
        .messages
        .iter()
        .filter(|message| message.num == typedef::MesgNum::ACTIVITY)
        .map(mesgdef::Activity::from)
        .collect::<Vec<_>>();
    let [activity_message] = activity_messages.as_slice() else {
        return Err(ActivityError::CountMismatch {
            name: "activity message count",
            expected: 1,
            found: activity_messages.len(),
        });
    };
    timestamp(activity_message.timestamp, "activity.timestamp")?;
    if activity_message.num_sessions != u16::MAX
        && usize::from(activity_message.num_sessions) != sessions.len()
    {
        return Err(ActivityError::CountMismatch {
            name: "activity session count",
            expected: usize::from(activity_message.num_sessions),
            found: sessions.len(),
        });
    }

    let laps = fit
        .messages
        .iter()
        .filter(|message| message.num == typedef::MesgNum::LAP)
        .map(mesgdef::Lap::from)
        .map(|lap| normalize_lap(&lap))
        .collect::<Result<Vec<_>, _>>()?;
    if session.num_laps != u16::MAX && usize::from(session.num_laps) != laps.len() {
        return Err(ActivityError::CountMismatch {
            name: "session lap count",
            expected: usize::from(session.num_laps),
            found: laps.len(),
        });
    }

    let track = fit
        .messages
        .iter()
        .filter(|message| message.num == typedef::MesgNum::RECORD)
        .map(mesgdef::Record::from)
        .map(|record| normalize_record(&record))
        .collect::<Result<Vec<_>, _>>()?;
    let timer_events = fit
        .messages
        .iter()
        .filter(|message| message.num == typedef::MesgNum::EVENT)
        .map(mesgdef::Event::from)
        .filter_map(|event| normalize_timer_event(&event))
        .collect::<Result<Vec<_>, _>>()?;

    Activity::from_parts(normalize_summary(session)?, laps, track, timer_events)
        .map_err(ActivityError::from)
}

fn decode_raw(bytes: &[u8]) -> Result<Vec<FIT>, DecodeError> {
    let mut decoder = Decoder::new();
    let mut input = FromStd::new(Cursor::new(bytes));
    let mut sequences = Vec::new();
    loop {
        match decoder
            .decode(&mut input)
            .map_err(|error| DecodeError::Rejected(error.to_string()))?
        {
            Some(fit) => sequences.push(fit),
            None if sequences.is_empty() => return Err(DecodeError::Incomplete),
            None => return Ok(sequences),
        }
    }
}

fn normalize_summary(session: &mesgdef::Session) -> Result<ActivitySummary, ActivityError> {
    timestamp(session.timestamp, "session.timestamp")?;
    Ok(ActivitySummary::from_parts(
        sport(session.sport)?,
        time_range(session.start_time, session.total_elapsed_time, "session")?,
        totals(
            session.total_elapsed_time,
            session.total_timer_time,
            session.total_distance,
            session.total_calories,
            session.total_ascent,
            session.total_descent,
            "session",
        )?,
        metrics(MetricFields::from_session(session))?,
    ))
}

fn normalize_lap(lap: &mesgdef::Lap) -> Result<Lap, ActivityError> {
    timestamp(lap.timestamp, "lap.timestamp")?;
    Ok(Lap::from_parts(
        time_range(lap.start_time, lap.total_elapsed_time, "lap")?,
        totals(
            lap.total_elapsed_time,
            lap.total_timer_time,
            lap.total_distance,
            lap.total_calories,
            lap.total_ascent,
            lap.total_descent,
            "lap",
        )?,
        metrics(MetricFields::from_lap(lap))?,
    ))
}

fn normalize_record(record: &mesgdef::Record) -> Result<TrackPoint, ActivityError> {
    let coordinate = match (record.position_lat, record.position_long) {
        (i32::MAX, i32::MAX) => None,
        (i32::MAX, _) | (_, i32::MAX) => return Err(ActivityError::PartialCoordinate),
        (latitude, longitude) => Some(Coordinate::from_parts(
            Latitude::from_degrees(semicircles_to_degrees(latitude)).map_err(|error| {
                ActivityError::InvalidValue {
                    field: "record.position_lat",
                    reason: error.to_string(),
                }
            })?,
            Longitude::from_degrees(semicircles_to_degrees(longitude)).map_err(|error| {
                ActivityError::InvalidValue {
                    field: "record.position_long",
                    reason: error.to_string(),
                }
            })?,
        )),
    };
    let elevation = if record.enhanced_altitude == u32::MAX {
        scaled_elevation(u32::from(record.altitude), u32::from(u16::MAX))?
    } else {
        scaled_elevation(record.enhanced_altitude, u32::MAX)?
    };
    let speed = if record.enhanced_speed != u32::MAX {
        Some(Speed::from_millimeters_per_second(record.enhanced_speed))
    } else if record.speed != u16::MAX {
        Some(Speed::from_millimeters_per_second(u32::from(record.speed)))
    } else {
        None
    };

    Ok(TrackPoint::from_parts(
        timestamp(record.timestamp, "record.timestamp")?,
        coordinate,
        elevation,
        optional_u32(record.distance)
            .map(|value| Distance::from_millimeters(u64::from(value) * 10)),
        TrackMeasurements::from_parts(
            speed,
            optional_u8(record.heart_rate)
                .map(|value| HeartRate::from_beats_per_minute(u16::from(value))),
            cadence(record.cadence, record.fractional_cadence)?,
            optional_u16(record.power).map(|value| Power::from_watts(u32::from(value))),
            (record.temperature != i8::MAX)
                .then(|| Temperature::from_millicelsius(i32::from(record.temperature) * 1_000)),
        ),
    ))
}

fn normalize_timer_event(event: &mesgdef::Event) -> Option<Result<TimerEvent, ActivityError>> {
    if event.event != typedef::Event::TIMER {
        return None;
    }
    let state = match event.event_type {
        typedef::EventType::START => TimerState::Running,
        typedef::EventType::STOP
        | typedef::EventType::STOP_ALL
        | typedef::EventType::STOP_DISABLE
        | typedef::EventType::STOP_DISABLE_ALL => TimerState::Stopped,
        _ => return None,
    };
    Some(
        timestamp(event.timestamp, "event.timestamp")
            .map(|timestamp| TimerEvent::from_parts(timestamp, state)),
    )
}

const fn sport(value: typedef::Sport) -> Result<ActivitySport, ActivityError> {
    match value {
        typedef::Sport::RUNNING => Ok(ActivitySport::Running),
        typedef::Sport::CYCLING => Ok(ActivitySport::Cycling),
        _ => Err(ActivityError::UnsupportedSport(value.0)),
    }
}

fn time_range(
    start: typedef::DateTime,
    elapsed_milliseconds: u32,
    field: &'static str,
) -> Result<TimeRange, ActivityError> {
    // Garmin defines summary spans as start time plus elapsed time.
    // https://developer.garmin.com/fit/cookbook/decoding-activity-files/
    let start = timestamp(start, field)?;
    let elapsed = i64::from(required_u32(elapsed_milliseconds, field)?);
    let end = start
        .as_unix_seconds()
        .checked_mul(1_000)
        .and_then(|start| start.checked_add(elapsed))
        .ok_or_else(|| ActivityError::InvalidValue {
            field,
            reason: "end timestamp overflowed".to_owned(),
        })?;
    let end =
        Timestamp::from_unix_milliseconds(end).map_err(|error| ActivityError::InvalidValue {
            field,
            reason: error.to_string(),
        })?;
    TimeRange::from_parts(start, end).map_err(ActivityError::from)
}

fn timestamp(value: typedef::DateTime, field: &'static str) -> Result<Timestamp, ActivityError> {
    // Values below DateTime::MIN are device-relative, not UTC.
    // https://developer.garmin.com/fit/cookbook/datetime/
    if value.0 < typedef::DateTime::MIN.0 {
        return Err(ActivityError::InvalidValue {
            field,
            reason: "relative device time has no absolute UTC value".to_owned(),
        });
    }
    let seconds = value
        .unix_timestamp()
        .ok_or(ActivityError::MissingField(field))?;
    Timestamp::from_unix_seconds(seconds).map_err(|error| ActivityError::InvalidValue {
        field,
        reason: error.to_string(),
    })
}

fn totals(
    elapsed: u32,
    timer: u32,
    distance: u32,
    calories: u16,
    ascent: u16,
    descent: u16,
    field: &'static str,
) -> Result<ActivityTotals, ActivityError> {
    ActivityTotals::from_parts(
        ActivityDuration::from_milliseconds(u64::from(required_u32(elapsed, field)?)),
        ActivityDuration::from_milliseconds(u64::from(required_u32(timer, field)?)),
        optional_u32(distance).map(|value| Distance::from_millimeters(u64::from(value) * 10)),
        optional_u16(calories).map(|value| Energy::from_kilocalories(u32::from(value))),
        optional_u16(ascent).map(|value| Distance::from_millimeters(u64::from(value) * 1_000)),
        optional_u16(descent).map(|value| Distance::from_millimeters(u64::from(value) * 1_000)),
    )
    .map_err(ActivityError::from)
}

#[derive(Clone, Copy)]
struct MetricFields {
    average_speed: Option<u32>,
    maximum_speed: Option<u32>,
    average_heart_rate: u8,
    maximum_heart_rate: u8,
    average_cadence: u8,
    average_fractional_cadence: u8,
    maximum_cadence: u8,
    maximum_fractional_cadence: u8,
    average_power: u16,
    maximum_power: u16,
}

impl MetricFields {
    const fn from_session(session: &mesgdef::Session) -> Self {
        Self {
            average_speed: preferred_speed(session.enhanced_avg_speed, session.avg_speed),
            maximum_speed: preferred_speed(session.enhanced_max_speed, session.max_speed),
            average_heart_rate: session.avg_heart_rate,
            maximum_heart_rate: session.max_heart_rate,
            average_cadence: session.avg_cadence,
            average_fractional_cadence: session.avg_fractional_cadence,
            maximum_cadence: session.max_cadence,
            maximum_fractional_cadence: session.max_fractional_cadence,
            average_power: session.avg_power,
            maximum_power: session.max_power,
        }
    }

    const fn from_lap(lap: &mesgdef::Lap) -> Self {
        Self {
            average_speed: preferred_speed(lap.enhanced_avg_speed, lap.avg_speed),
            maximum_speed: preferred_speed(lap.enhanced_max_speed, lap.max_speed),
            average_heart_rate: lap.avg_heart_rate,
            maximum_heart_rate: lap.max_heart_rate,
            average_cadence: lap.avg_cadence,
            average_fractional_cadence: lap.avg_fractional_cadence,
            maximum_cadence: lap.max_cadence,
            maximum_fractional_cadence: lap.max_fractional_cadence,
            average_power: lap.avg_power,
            maximum_power: lap.max_power,
        }
    }
}

fn metrics(fields: MetricFields) -> Result<ActivityMetrics, ActivityError> {
    Ok(ActivityMetrics::from_parts(
        ActivityMetric::from_parts(
            fields.average_speed.map(Speed::from_millimeters_per_second),
            fields.maximum_speed.map(Speed::from_millimeters_per_second),
        ),
        ActivityMetric::from_parts(
            optional_u8(fields.average_heart_rate)
                .map(|value| HeartRate::from_beats_per_minute(u16::from(value))),
            optional_u8(fields.maximum_heart_rate)
                .map(|value| HeartRate::from_beats_per_minute(u16::from(value))),
        ),
        ActivityMetric::from_parts(
            cadence(fields.average_cadence, fields.average_fractional_cadence)?,
            cadence(fields.maximum_cadence, fields.maximum_fractional_cadence)?,
        ),
        ActivityMetric::from_parts(
            optional_u16(fields.average_power).map(|value| Power::from_watts(u32::from(value))),
            optional_u16(fields.maximum_power).map(|value| Power::from_watts(u32::from(value))),
        ),
    ))
}

fn cadence(whole: u8, fractional: u8) -> Result<Option<Cadence>, ActivityError> {
    let Some(whole) = optional_u8(whole) else {
        return Ok(None);
    };
    let fractional = optional_u8(fractional).map_or(0.0, |value| f64::from(value) / 128.0);
    Cadence::from_revolutions_per_minute(f64::from(whole) + fractional)
        .map(Some)
        .map_err(ActivityError::from)
}

fn scaled_elevation(value: u32, invalid: u32) -> Result<Option<Elevation>, ActivityError> {
    if value == invalid {
        return Ok(None);
    }
    Elevation::from_meters(f64::from(value) / 5.0 - 500.0)
        .map(Some)
        .map_err(|error| ActivityError::InvalidValue {
            field: "record.altitude",
            reason: error.to_string(),
        })
}

fn required_u32(value: u32, field: &'static str) -> Result<u32, ActivityError> {
    (value != u32::MAX)
        .then_some(value)
        .ok_or(ActivityError::MissingField(field))
}

const fn optional_u8(value: u8) -> Option<u8> {
    if value == u8::MAX { None } else { Some(value) }
}

const fn optional_u16(value: u16) -> Option<u16> {
    if value == u16::MAX { None } else { Some(value) }
}

const fn optional_u32(value: u32) -> Option<u32> {
    if value == u32::MAX { None } else { Some(value) }
}

const fn preferred_speed(enhanced: u32, legacy: u16) -> Option<u32> {
    match optional_u32(enhanced) {
        Some(value) => Some(value),
        None => match optional_u16(legacy) {
            Some(value) => Some(value as u32),
            None => None,
        },
    }
}

fn semicircles_to_degrees(value: i32) -> f64 {
    f64::from(value) * (180.0 / 2_f64.powi(31))
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use rustyfit::{
        profile::{mesgdef, typedef},
        proto::{FIT, Message},
    };

    use super::{
        Activity, ActivityError, ActivitySequence, ActivitySport, DecodeError, FileType, ProductId,
        ProductName, SerialNumber, SoftwareVersion, normalize_activities,
    };
    use crate::fixture::{END, START, encode, synthetic_activity, synthetic_file};

    #[test]
    fn rejects_empty_input() {
        assert!(matches!(
            normalize_activities(&[]),
            Err(ActivityError::Decode(DecodeError::Incomplete))
        ));
    }

    #[test]
    fn rejects_trailing_input() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        let mut trailing = bytes;
        trailing.push(0);
        assert!(matches!(
            normalize_activities(&trailing),
            Err(ActivityError::Decode(DecodeError::Rejected(_)))
        ));

        Ok(())
    }

    #[test]
    fn keeps_chained_activities_separate() -> Result<(), Box<dyn StdError>> {
        let mut chained =
            synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        chained.extend(synthetic_activity(
            typedef::Sport::CYCLING,
            false,
            typedef::File::ACTIVITY,
        )?);

        let import = normalize_activities(&chained)?;
        let sports = import
            .activities()
            .map(|activity| activity.summary().sport())
            .collect::<Vec<_>>();
        assert_eq!(sports, [ActivitySport::Running, ActivitySport::Cycling]);
        assert_eq!(import.sequences().len(), 2);

        Ok(())
    }

    #[test]
    fn preserves_chained_settings_sequence() -> Result<(), Box<dyn StdError>> {
        let mut chained =
            synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        chained.extend(synthetic_file(typedef::File::SETTINGS)?);
        chained.extend(synthetic_activity(
            typedef::Sport::CYCLING,
            false,
            typedef::File::ACTIVITY,
        )?);

        let import = normalize_activities(&chained)?;
        assert_eq!(import.activities().count(), 2);
        assert_eq!(
            import.sequences()[1].preserved_file_type(),
            Some(FileType::SETTINGS)
        );
        assert_eq!(
            import
                .positioned_activities()
                .map(|(position, activity)| {
                    (position.into_u32(), activity.activity().summary().sport())
                })
                .collect::<Vec<_>>(),
            [(0, ActivitySport::Running), (2, ActivitySport::Cycling)]
        );

        Ok(())
    }

    #[test]
    fn rejects_a_chain_without_an_activity() -> Result<(), Box<dyn StdError>> {
        let settings = synthetic_file(typedef::File::SETTINGS)?;
        assert!(matches!(
            normalize_activities(&settings),
            Err(ActivityError::NoActivities)
        ));

        Ok(())
    }

    #[test]
    fn rejects_an_unsupported_later_sequence() -> Result<(), Box<dyn StdError>> {
        let mut chained =
            synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        chained.extend(synthetic_file(typedef::File::COURSE)?);
        assert!(matches!(
            normalize_activities(&chained),
            Err(ActivityError::UnsupportedFileType(_))
        ));

        Ok(())
    }

    #[test]
    fn rejects_a_corrupt_later_sequence() -> Result<(), Box<dyn StdError>> {
        let mut chained =
            synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        let mut corrupt = synthetic_file(typedef::File::SETTINGS)?;
        let checksum = corrupt.last_mut().ok_or("missing synthetic checksum")?;
        *checksum ^= 1;
        chained.extend(corrupt);

        assert!(matches!(
            normalize_activities(&chained),
            Err(ActivityError::Decode(DecodeError::Rejected(_)))
        ));

        Ok(())
    }

    #[test]
    fn normalizes_running_activity() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::RUNNING, true, typedef::File::ACTIVITY)?;
        let import = normalize_activities(&bytes)?;
        let creator = import
            .normalized_activities()
            .next()
            .ok_or("missing normalized activity")?
            .creator();
        assert_eq!(creator.manufacturer().as_u16(), 255);
        assert_eq!(creator.product().map(ProductId::into_u16), Some(42));
        assert_eq!(
            creator.serial_number().map(SerialNumber::into_u32),
            Some(1_234)
        );
        assert_eq!(
            creator.product_name().map(ProductName::as_str),
            Some("Synthetic tracker")
        );
        assert_eq!(
            creator
                .software_version()
                .map(SoftwareVersion::into_hundredths),
            Some(123)
        );
        let activity = normalize_one(&bytes)?;
        assert_eq!(activity.summary().sport(), ActivitySport::Running);
        assert_eq!(activity.summary().time().start().as_unix_seconds(), START);
        assert_eq!(activity.summary().time().end().as_unix_seconds(), END);
        assert_eq!(
            activity
                .summary()
                .totals()
                .distance()
                .ok_or("missing total distance")?
                .as_millimeters(),
            123_450
        );
        assert_eq!(activity.laps().len(), 2);
        assert_eq!(
            activity.laps()[0].time().end().as_unix_seconds(),
            START + 15
        );
        assert_eq!(activity.laps()[1].time().end().as_unix_seconds(), END);
        assert_eq!(activity.track().len(), 2);
        assert!(activity.track()[0].coordinate().is_some());
        assert_eq!(
            activity
                .summary()
                .metrics()
                .average_speed()
                .ok_or("missing average speed")?
                .as_millimeters_per_second(),
            3_000
        );
        assert_eq!(
            activity.track()[1]
                .distance()
                .ok_or("missing distance")?
                .as_millimeters(),
            123_450
        );
        let cadence = activity.track()[0]
            .measurements()
            .cadence()
            .ok_or("missing cadence")?
            .as_revolutions_per_minute();
        assert!((cadence - 81.5).abs() < f64::EPSILON);
        assert!(activity.track()[0].measurements().power().is_some());
        assert_eq!(activity.timer_events().len(), 2);
        Ok(())
    }

    #[test]
    fn preserves_independently_missing_point_measurements() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::CYCLING, false, typedef::File::ACTIVITY)?;
        let activity = normalize_one(&bytes)?;
        assert_eq!(activity.summary().sport(), ActivitySport::Cycling);
        let measurements = activity.track()[0].measurements();
        assert!(measurements.speed().is_none());
        assert!(measurements.cadence().is_none());
        assert!(measurements.power().is_none());
        assert!(measurements.heart_rate().is_some());
        assert_eq!(
            activity
                .summary()
                .metrics()
                .average_speed()
                .ok_or("missing legacy average speed")?
                .as_millimeters_per_second(),
            2_000
        );
        Ok(())
    }

    #[test]
    fn rejects_non_activity_fit_files() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::CYCLING, false, typedef::File::COURSE)?;
        assert!(matches!(
            normalize_activities(&bytes),
            Err(super::ActivityError::UnsupportedFileType(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_invalid_activity_envelope() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;

        let mut misplaced_file_id = decode_one(&bytes)?;
        misplaced_file_id.messages.swap(0, 1);
        assert!(matches!(
            normalize_activities(&encode(misplaced_file_id)?),
            Err(ActivityError::InvalidValue {
                field: "file_id",
                ..
            })
        ));

        let mut missing_manufacturer = decode_one(&bytes)?;
        let file_id = missing_manufacturer
            .messages
            .iter_mut()
            .find(|message| message.num == typedef::MesgNum::FILE_ID)
            .ok_or("missing synthetic file ID")?;
        let mut invalid = mesgdef::FileId::from(&*file_id);
        invalid.manufacturer = typedef::Manufacturer(u16::MAX);
        *file_id = Message::from(invalid);
        assert!(matches!(
            normalize_activities(&encode(missing_manufacturer)?),
            Err(ActivityError::MissingField("file_id.manufacturer"))
        ));

        let mut missing_session_timestamp = decode_one(&bytes)?;
        let session = missing_session_timestamp
            .messages
            .iter_mut()
            .find(|message| message.num == typedef::MesgNum::SESSION)
            .ok_or("missing synthetic session")?;
        let mut invalid = mesgdef::Session::from(&*session);
        invalid.timestamp = typedef::DateTime(u32::MAX);
        *session = Message::from(invalid);
        assert!(matches!(
            normalize_activities(&encode(missing_session_timestamp)?),
            Err(ActivityError::MissingField("session.timestamp"))
        ));
        Ok(())
    }

    #[test]
    fn rejects_conflicting_creator_identity() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        let mut fit = decode_one(&bytes)?;
        let creator = fit
            .messages
            .iter_mut()
            .find(|message| message.num == typedef::MesgNum::DEVICE_INFO)
            .ok_or("missing synthetic creator")?;
        let mut conflicting = mesgdef::DeviceInfo::from(&*creator);
        conflicting.product = 43;
        *creator = Message::from(conflicting);

        assert!(matches!(
            normalize_activities(&encode(fit)?),
            Err(ActivityError::ConflictingCreatorField("product"))
        ));
        Ok(())
    }

    #[test]
    fn rejects_relative_activity_timestamps() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        let mut fit = decode_one(&bytes)?;
        let record = fit
            .messages
            .iter_mut()
            .find(|message| message.num == typedef::MesgNum::RECORD)
            .ok_or("missing synthetic record")?;
        let mut relative = mesgdef::Record::from(&*record);
        relative.timestamp = typedef::DateTime(60);
        *record = Message::from(relative);

        assert!(matches!(
            normalize_activities(&encode(fit)?),
            Err(super::ActivityError::InvalidValue {
                field: "record.timestamp",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn preserves_missing_summary_distance() -> Result<(), Box<dyn StdError>> {
        let bytes = synthetic_activity(typedef::Sport::RUNNING, false, typedef::File::ACTIVITY)?;
        let mut fit = decode_one(&bytes)?;
        for message in &mut fit.messages {
            match message.num {
                typedef::MesgNum::SESSION => {
                    let mut session = mesgdef::Session::from(&*message);
                    session.total_distance = u32::MAX;
                    *message = Message::from(session);
                }
                typedef::MesgNum::LAP => {
                    let mut lap = mesgdef::Lap::from(&*message);
                    lap.total_distance = u32::MAX;
                    *message = Message::from(lap);
                }
                _ => {}
            }
        }

        let activity = normalize_one(&encode(fit)?)?;
        assert!(activity.summary().totals().distance().is_none());
        assert!(
            activity
                .laps()
                .iter()
                .all(|lap| lap.totals().distance().is_none())
        );
        Ok(())
    }

    fn normalize_one(bytes: &[u8]) -> Result<Activity, Box<dyn StdError>> {
        let mut sequences = normalize_activities(bytes)?.into_sequences().into_iter();
        let Some(ActivitySequence::Activity(activity)) = sequences.next() else {
            return Err("expected one activity sequence".into());
        };
        if sequences.next().is_some() {
            return Err("expected one activity sequence".into());
        }
        let (_, activity) = activity.into_parts();
        Ok(activity)
    }

    fn decode_one(bytes: &[u8]) -> Result<FIT, Box<dyn StdError>> {
        let mut sequences = super::decode_raw(bytes)?.into_iter();
        let fit = sequences.next().ok_or("expected one FIT sequence")?;
        if sequences.next().is_some() {
            return Err("expected one FIT sequence".into());
        }
        Ok(fit)
    }
}
