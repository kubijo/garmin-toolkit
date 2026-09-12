//! Normalized recorded activities.

use std::fmt;

use thiserror::Error;

use crate::{
    activity_fingerprint,
    observation::{SemanticFingerprint, SemanticFingerprintError},
    route::{Coordinate, Elevation},
    value::Timestamp,
};

/// Elapsed time stored canonically in milliseconds.
#[garmin_macros::portable(copy, hash, ord)]
pub struct ActivityDuration(u64);

impl ActivityDuration {
    #[must_use]
    pub const fn from_milliseconds(milliseconds: u64) -> Self {
        Self(milliseconds)
    }

    #[must_use]
    pub const fn as_milliseconds(&self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn into_milliseconds(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ActivityDuration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{:03} s", self.0 / 1_000, self.0 % 1_000)
    }
}

/// Distance stored canonically in millimeters.
#[garmin_macros::portable(copy, hash, ord)]
pub struct Distance(u64);

impl Distance {
    #[must_use]
    pub const fn from_millimeters(millimeters: u64) -> Self {
        Self(millimeters)
    }

    #[must_use]
    pub const fn as_millimeters(&self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn into_millimeters(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Distance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{:03} m", self.0 / 1_000, self.0 % 1_000)
    }
}

/// Speed stored canonically in millimeters per second.
#[garmin_macros::portable(copy, hash, ord)]
pub struct Speed(u32);

impl Speed {
    #[must_use]
    pub const fn from_millimeters_per_second(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_millimeters_per_second(&self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn into_millimeters_per_second(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Speed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{:03} m/s", self.0 / 1_000, self.0 % 1_000)
    }
}

/// Heart rate in beats per minute.
#[garmin_macros::portable(copy, hash, ord)]
pub struct HeartRate(u16);

impl HeartRate {
    #[must_use]
    pub const fn from_beats_per_minute(value: u16) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_beats_per_minute(&self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn into_beats_per_minute(self) -> u16 {
        self.0
    }
}

impl fmt::Display for HeartRate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} bpm", self.0)
    }
}

/// Cadence in revolutions per minute.
#[garmin_macros::portable(copy, custom_deserialize)]
pub struct Cadence(f64);

impl<'de> serde::Deserialize<'de> for Cadence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <f64 as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_revolutions_per_minute(value).map_err(serde::de::Error::custom)
    }
}

impl Cadence {
    /// Validates revolutions per minute.
    /// # Errors
    /// [`Error::InvalidCadence`] for a negative or non-finite value.
    pub const fn from_revolutions_per_minute(value: f64) -> Result<Self, Error> {
        if !value.is_finite() || value < 0.0 {
            return Err(Error::InvalidCadence);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_revolutions_per_minute(&self) -> f64 {
        self.0
    }

    #[must_use]
    pub const fn into_revolutions_per_minute(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Cadence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} rpm", self.0)
    }
}

/// Mechanical power in watts.
#[garmin_macros::portable(copy, hash, ord)]
pub struct Power(u32);

impl Power {
    #[must_use]
    pub const fn from_watts(watts: u32) -> Self {
        Self(watts)
    }

    #[must_use]
    pub const fn as_watts(&self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn into_watts(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Power {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} W", self.0)
    }
}

/// Energy in kilocalories.
#[garmin_macros::portable(copy, hash, ord)]
pub struct Energy(u32);

impl Energy {
    #[must_use]
    pub const fn from_kilocalories(kilocalories: u32) -> Self {
        Self(kilocalories)
    }

    #[must_use]
    pub const fn as_kilocalories(&self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn into_kilocalories(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Energy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} kcal", self.0)
    }
}

/// Temperature stored canonically in millidegrees Celsius.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Temperature(i32);

impl Temperature {
    #[must_use]
    pub const fn from_millicelsius(value: i32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_millicelsius(&self) -> i32 {
        self.0
    }

    #[must_use]
    pub const fn into_millicelsius(self) -> i32 {
        self.0
    }
}

impl fmt::Display for Temperature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let magnitude = self.0.unsigned_abs();
        let sign = if self.0 < 0 { "-" } else { "" };
        write!(
            formatter,
            "{sign}{}.{:03} °C",
            magnitude / 1_000,
            magnitude % 1_000
        )
    }
}

/// Supported normalized activity sport.
#[garmin_macros::portable(copy, hash)]
pub enum ActivitySport {
    Running,
    Cycling,
}

impl fmt::Display for ActivitySport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => formatter.write_str("running"),
            Self::Cycling => formatter.write_str("cycling"),
        }
    }
}

/// An activity or lap time range.
#[garmin_macros::portable(copy, custom_deserialize, eq)]
pub struct TimeRange {
    start: Timestamp,
    end: Timestamp,
}

impl<'de> serde::Deserialize<'de> for TimeRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Wire {
            start: Timestamp,
            end: Timestamp,
        }

        let Wire { start, end } = <Wire as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_parts(start, end).map_err(serde::de::Error::custom)
    }
}

impl TimeRange {
    /// Validates an ordered range.
    /// # Errors
    /// An error for submillisecond endpoints or a reversed range.
    pub fn from_parts(start: Timestamp, end: Timestamp) -> Result<Self, Error> {
        if !start.is_millisecond_aligned() || !end.is_millisecond_aligned() {
            return Err(Error::SubmillisecondTimestamp);
        }
        if end < start {
            return Err(Error::ReversedTimeRange);
        }
        Ok(Self { start, end })
    }

    #[must_use]
    pub const fn start(&self) -> Timestamp {
        self.start
    }

    #[must_use]
    pub const fn end(&self) -> Timestamp {
        self.end
    }

    #[must_use]
    pub const fn into_parts(self) -> (Timestamp, Timestamp) {
        (self.start, self.end)
    }
}

/// Aggregate totals shared by activities and laps.
#[garmin_macros::portable(copy, custom_deserialize, eq)]
pub struct ActivityTotals {
    elapsed: ActivityDuration,
    timer: ActivityDuration,
    distance: Option<Distance>,
    energy: Option<Energy>,
    ascent: Option<Distance>,
    descent: Option<Distance>,
}

impl<'de> serde::Deserialize<'de> for ActivityTotals {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Wire {
            elapsed: ActivityDuration,
            timer: ActivityDuration,
            distance: Option<Distance>,
            energy: Option<Energy>,
            ascent: Option<Distance>,
            descent: Option<Distance>,
        }

        let Wire {
            elapsed,
            timer,
            distance,
            energy,
            ascent,
            descent,
        } = <Wire as serde::Deserialize>::deserialize(deserializer)?;
        Self::from_parts(elapsed, timer, distance, energy, ascent, descent)
            .map_err(serde::de::Error::custom)
    }
}

impl ActivityTotals {
    /// Validates aggregate totals.
    /// # Errors
    /// [`Error::TimerExceedsElapsed`] when active timer time exceeds elapsed time.
    pub const fn from_parts(
        elapsed: ActivityDuration,
        timer: ActivityDuration,
        distance: Option<Distance>,
        energy: Option<Energy>,
        ascent: Option<Distance>,
        descent: Option<Distance>,
    ) -> Result<Self, Error> {
        if timer.0 > elapsed.0 {
            return Err(Error::TimerExceedsElapsed);
        }
        Ok(Self {
            elapsed,
            timer,
            distance,
            energy,
            ascent,
            descent,
        })
    }

    #[must_use]
    pub const fn elapsed(&self) -> ActivityDuration {
        self.elapsed
    }

    #[must_use]
    pub const fn timer(&self) -> ActivityDuration {
        self.timer
    }

    #[must_use]
    pub const fn distance(&self) -> Option<Distance> {
        self.distance
    }

    #[must_use]
    pub const fn energy(&self) -> Option<Energy> {
        self.energy
    }

    #[must_use]
    pub const fn ascent(&self) -> Option<Distance> {
        self.ascent
    }

    #[must_use]
    pub const fn descent(&self) -> Option<Distance> {
        self.descent
    }
}

/// Average and maximum values for one optional aggregate measurement.
#[garmin_macros::portable(copy, eq)]
pub struct ActivityMetric<T> {
    average: Option<T>,
    maximum: Option<T>,
}

impl<T> Default for ActivityMetric<T> {
    fn default() -> Self {
        Self {
            average: None,
            maximum: None,
        }
    }
}

impl<T: Copy> ActivityMetric<T> {
    #[must_use]
    pub const fn from_parts(average: Option<T>, maximum: Option<T>) -> Self {
        Self { average, maximum }
    }

    #[must_use]
    pub const fn average(&self) -> Option<T> {
        self.average
    }

    #[must_use]
    pub const fn maximum(&self) -> Option<T> {
        self.maximum
    }

    #[must_use]
    pub const fn into_parts(self) -> (Option<T>, Option<T>) {
        (self.average, self.maximum)
    }
}

/// Optional summary measurements shared by activities and laps.
#[garmin_macros::portable(copy, default)]
pub struct ActivityMetrics {
    speed: ActivityMetric<Speed>,
    heart_rate: ActivityMetric<HeartRate>,
    cadence: ActivityMetric<Cadence>,
    power: ActivityMetric<Power>,
}

impl ActivityMetrics {
    #[must_use]
    pub const fn from_parts(
        speed: ActivityMetric<Speed>,
        heart_rate: ActivityMetric<HeartRate>,
        cadence: ActivityMetric<Cadence>,
        power: ActivityMetric<Power>,
    ) -> Self {
        Self {
            speed,
            heart_rate,
            cadence,
            power,
        }
    }

    #[must_use]
    pub const fn average_speed(&self) -> Option<Speed> {
        self.speed.average()
    }

    #[must_use]
    pub const fn maximum_speed(&self) -> Option<Speed> {
        self.speed.maximum()
    }

    #[must_use]
    pub const fn average_heart_rate(&self) -> Option<HeartRate> {
        self.heart_rate.average()
    }

    #[must_use]
    pub const fn maximum_heart_rate(&self) -> Option<HeartRate> {
        self.heart_rate.maximum()
    }

    #[must_use]
    pub const fn average_cadence(&self) -> Option<Cadence> {
        self.cadence.average()
    }

    #[must_use]
    pub const fn maximum_cadence(&self) -> Option<Cadence> {
        self.cadence.maximum()
    }

    #[must_use]
    pub const fn average_power(&self) -> Option<Power> {
        self.power.average()
    }

    #[must_use]
    pub const fn maximum_power(&self) -> Option<Power> {
        self.power.maximum()
    }
}

/// Aggregate activity summary.
#[garmin_macros::portable(copy)]
pub struct ActivitySummary {
    sport: ActivitySport,
    time: TimeRange,
    totals: ActivityTotals,
    metrics: ActivityMetrics,
}

impl ActivitySummary {
    #[must_use]
    pub const fn from_parts(
        sport: ActivitySport,
        time: TimeRange,
        totals: ActivityTotals,
        metrics: ActivityMetrics,
    ) -> Self {
        Self {
            sport,
            time,
            totals,
            metrics,
        }
    }

    #[must_use]
    pub const fn sport(&self) -> ActivitySport {
        self.sport
    }

    #[must_use]
    pub const fn time(&self) -> TimeRange {
        self.time
    }

    #[must_use]
    pub const fn totals(&self) -> ActivityTotals {
        self.totals
    }

    #[must_use]
    pub const fn metrics(&self) -> ActivityMetrics {
        self.metrics
    }
}

/// One source-declared activity lap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lap {
    time: TimeRange,
    totals: ActivityTotals,
    metrics: ActivityMetrics,
}

impl Lap {
    #[must_use]
    pub const fn from_parts(
        time: TimeRange,
        totals: ActivityTotals,
        metrics: ActivityMetrics,
    ) -> Self {
        Self {
            time,
            totals,
            metrics,
        }
    }

    #[must_use]
    pub const fn time(&self) -> TimeRange {
        self.time
    }

    #[must_use]
    pub const fn totals(&self) -> ActivityTotals {
        self.totals
    }

    #[must_use]
    pub const fn metrics(&self) -> ActivityMetrics {
        self.metrics
    }
}

/// Independently optional measurements at one track instant.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackMeasurements {
    speed: Option<Speed>,
    heart_rate: Option<HeartRate>,
    cadence: Option<Cadence>,
    power: Option<Power>,
    temperature: Option<Temperature>,
}

impl TrackMeasurements {
    #[must_use]
    pub const fn from_parts(
        speed: Option<Speed>,
        heart_rate: Option<HeartRate>,
        cadence: Option<Cadence>,
        power: Option<Power>,
        temperature: Option<Temperature>,
    ) -> Self {
        Self {
            speed,
            heart_rate,
            cadence,
            power,
            temperature,
        }
    }

    #[must_use]
    pub const fn speed(&self) -> Option<Speed> {
        self.speed
    }

    #[must_use]
    pub const fn heart_rate(&self) -> Option<HeartRate> {
        self.heart_rate
    }

    #[must_use]
    pub const fn cadence(&self) -> Option<Cadence> {
        self.cadence
    }

    #[must_use]
    pub const fn power(&self) -> Option<Power> {
        self.power
    }

    #[must_use]
    pub const fn temperature(&self) -> Option<Temperature> {
        self.temperature
    }
}

/// One timestamped track sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPoint {
    timestamp: Timestamp,
    coordinate: Option<Coordinate>,
    elevation: Option<Elevation>,
    distance: Option<Distance>,
    measurements: TrackMeasurements,
}

impl TrackPoint {
    #[must_use]
    pub const fn from_parts(
        timestamp: Timestamp,
        coordinate: Option<Coordinate>,
        elevation: Option<Elevation>,
        distance: Option<Distance>,
        measurements: TrackMeasurements,
    ) -> Self {
        Self {
            timestamp,
            coordinate,
            elevation,
            distance,
            measurements,
        }
    }

    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    #[must_use]
    pub const fn coordinate(&self) -> Option<Coordinate> {
        self.coordinate
    }

    #[must_use]
    pub const fn elevation(&self) -> Option<Elevation> {
        self.elevation
    }

    #[must_use]
    pub const fn distance(&self) -> Option<Distance> {
        self.distance
    }

    #[must_use]
    pub const fn measurements(&self) -> TrackMeasurements {
        self.measurements
    }
}

/// A normalized timer transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerEvent {
    timestamp: Timestamp,
    state: TimerState,
}

impl TimerEvent {
    #[must_use]
    pub const fn from_parts(timestamp: Timestamp, state: TimerState) -> Self {
        Self { timestamp, state }
    }

    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    #[must_use]
    pub const fn state(&self) -> TimerState {
        self.state
    }

    #[must_use]
    pub const fn into_parts(self) -> (Timestamp, TimerState) {
        (self.timestamp, self.state)
    }
}

/// Timer state after a source transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerState {
    Running,
    Stopped,
}

/// One normalized recorded activity.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    summary: ActivitySummary,
    laps: Vec<Lap>,
    track: Vec<TrackPoint>,
    timer_events: Vec<TimerEvent>,
}

impl Activity {
    /// Validates ordered activity detail.
    /// # Errors
    /// An error for missing laps or points, or for out-of-order sequences.
    pub fn from_parts(
        summary: ActivitySummary,
        laps: Vec<Lap>,
        track: Vec<TrackPoint>,
        timer_events: Vec<TimerEvent>,
    ) -> Result<Self, Error> {
        if laps.is_empty() {
            return Err(Error::NoLaps);
        }
        if track.is_empty() {
            return Err(Error::NoTrackPoints);
        }
        if track
            .iter()
            .any(|point| !point.timestamp.is_millisecond_aligned())
            || timer_events
                .iter()
                .any(|event| !event.timestamp.is_millisecond_aligned())
        {
            return Err(Error::SubmillisecondTimestamp);
        }
        if laps
            .windows(2)
            .any(|pair| pair[1].time.start < pair[0].time.start)
        {
            return Err(Error::UnorderedLaps);
        }
        if track
            .windows(2)
            .any(|pair| pair[1].timestamp < pair[0].timestamp)
        {
            return Err(Error::UnorderedTrackPoints);
        }
        if timer_events
            .windows(2)
            .any(|pair| pair[1].timestamp < pair[0].timestamp)
        {
            return Err(Error::UnorderedTimerEvents);
        }
        Ok(Self {
            summary,
            laps,
            track,
            timer_events,
        })
    }

    #[must_use]
    pub const fn summary(&self) -> ActivitySummary {
        self.summary
    }

    #[must_use]
    pub fn laps(&self) -> &[Lap] {
        &self.laps
    }

    #[must_use]
    pub fn track(&self) -> &[TrackPoint] {
        &self.track
    }

    #[must_use]
    pub fn timer_events(&self) -> &[TimerEvent] {
        &self.timer_events
    }

    /// Fingerprints the complete normalized activity
    /// under the current semantic schema.
    ///
    /// Source provenance and diagnostics
    /// are intentionally excluded.
    /// # Errors
    /// [`SemanticFingerprintError`] if encoding the canonical payload fails.
    pub fn semantic_fingerprint(&self) -> Result<SemanticFingerprint, SemanticFingerprintError> {
        activity_fingerprint::fingerprint(self)
    }
}

/// Invalid normalized activity data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("cadence must be finite and non-negative")]
    InvalidCadence,
    #[error("activity timestamps require millisecond resolution")]
    SubmillisecondTimestamp,
    #[error("time range cannot end before it starts")]
    ReversedTimeRange,
    #[error("timer time cannot exceed elapsed time")]
    TimerExceedsElapsed,
    #[error("activity requires at least one lap")]
    NoLaps,
    #[error("activity requires at least one track point")]
    NoTrackPoints,
    #[error("activity laps must be ordered by start time")]
    UnorderedLaps,
    #[error("activity track points must be ordered by timestamp")]
    UnorderedTrackPoints,
    #[error("activity timer events must be ordered by timestamp")]
    UnorderedTimerEvents,
}
