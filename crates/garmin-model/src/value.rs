//! Shared scalar value objects.

use std::{fmt, str::FromStr, time::Duration};

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use thiserror::Error;

/// Serde adapter for reports that encode elapsed time as fractional seconds.
pub mod duration_seconds {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};
    use std::time::Duration;

    /// Serializes a duration without changing the report's numeric schema.
    /// # Errors
    /// Returns errors from the output serializer.
    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(value.as_secs_f64())
    }

    /// Reads a duration from fractional seconds.
    /// # Errors
    /// Rejects negative, non-finite, or out-of-range durations.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        Duration::try_from_secs_f64(f64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Serde adapters for performance reports that encode fractional milliseconds.
pub mod duration_milliseconds {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
    use std::time::Duration;

    /// Serializes a duration in milliseconds.
    /// # Errors
    /// Returns errors from the output serializer.
    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(value.as_secs_f64() * 1_000.0)
    }

    /// Serializes an optional duration in milliseconds.
    /// # Errors
    /// Returns errors from the output serializer.
    pub fn serialize_optional<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .map(|elapsed| elapsed.as_secs_f64() * 1_000.0)
            .serialize(serializer)
    }

    /// Serializes a duration in microseconds.
    /// # Errors
    /// Returns errors from the output serializer.
    pub fn serialize_microseconds<S: Serializer>(
        value: &Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(value.as_secs_f64() * 1_000_000.0)
    }

    /// Reads a duration from fractional milliseconds.
    /// # Errors
    /// Rejects negative, non-finite, or out-of-range durations.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        Duration::try_from_secs_f64(f64::deserialize(deserializer)? / 1_000.0)
            .map_err(D::Error::custom)
    }
}

/// A nonnegative Unix epoch time with millisecond precision.
#[garmin_macros::portable(copy, default, ord, hash)]
pub struct UnixMilliseconds(u64);

impl UnixMilliseconds {
    #[must_use]
    pub const fn from_milliseconds_since_epoch(milliseconds: u64) -> Self {
        Self(milliseconds)
    }

    #[must_use]
    pub const fn as_milliseconds_since_epoch(&self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn saturating_from_duration_since_epoch(duration: Duration) -> Self {
        Self(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
    }
}

impl fmt::Display for UnixMilliseconds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A UTC instant with nanosecond precision.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(jiff::Timestamp);

impl Timestamp {
    /// Validates whole seconds since the Unix epoch.
    /// # Errors
    /// a Jiff error when the value is outside its supported range.
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, jiff::Error> {
        jiff::Timestamp::from_second(seconds).map(Self)
    }

    /// Validates milliseconds since the Unix epoch.
    /// # Errors
    /// a Jiff error when the value is outside its supported range.
    pub fn from_unix_milliseconds(milliseconds: i64) -> Result<Self, jiff::Error> {
        jiff::Timestamp::from_millisecond(milliseconds).map(Self)
    }

    #[must_use]
    pub const fn from_jiff(timestamp: jiff::Timestamp) -> Self {
        Self(timestamp)
    }

    #[must_use]
    pub const fn as_jiff(&self) -> &jiff::Timestamp {
        &self.0
    }

    #[must_use]
    pub fn as_unix_seconds(&self) -> i64 {
        self.0.as_second()
    }

    #[must_use]
    pub fn as_unix_milliseconds(&self) -> i64 {
        self.0.as_millisecond()
    }

    #[must_use]
    pub fn is_millisecond_aligned(&self) -> bool {
        self.0.subsec_nanosecond() % 1_000_000 == 0
    }

    #[must_use]
    pub const fn into_jiff(self) -> jiff::Timestamp {
        self.0
    }

    #[must_use]
    pub fn into_unix_seconds(self) -> i64 {
        self.0.as_second()
    }

    #[must_use]
    pub fn into_unix_milliseconds(self) -> i64 {
        self.0.as_millisecond()
    }
}

impl FromStr for Timestamp {
    type Err = jiff::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

/// A named implementation and its semantic version.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComponentVersion {
    name: String,
    version: Version,
}

impl Serialize for ComponentVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.name(), self.version().to_string()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ComponentVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (name, version) = <(String, String)>::deserialize(deserializer)?;
        Self::from_parts(name, version.parse().map_err(D::Error::custom)?).map_err(D::Error::custom)
    }
}

impl ComponentVersion {
    /// Creates a versioned component identity.
    /// # Errors
    /// [`Error::EmptyComponentName`] when `name` is blank.
    pub fn from_parts(name: impl Into<String>, version: Version) -> Result<Self, Error> {
        let name = trimmed_owned(name.into());
        if name.is_empty() {
            return Err(Error::EmptyComponentName);
        }
        Ok(Self { name, version })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn version(&self) -> &Version {
        &self.version
    }

    #[must_use]
    pub fn into_parts(self) -> (String, Version) {
        (self.name, self.version)
    }
}

impl fmt::Display for ComponentVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.name, self.version)
    }
}

/// One named, versioned transformation applied to source data.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Transformation(ComponentVersion);

impl Transformation {
    #[must_use]
    pub const fn from_component(component: ComponentVersion) -> Self {
        Self(component)
    }

    #[must_use]
    pub const fn as_component(&self) -> &ComponentVersion {
        &self.0
    }

    #[must_use]
    pub fn into_component(self) -> ComponentVersion {
        self.0
    }
}

impl fmt::Display for Transformation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Invalid shared value data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("component name cannot be blank")]
    EmptyComponentName,
}

pub(crate) fn trimmed_owned(value: String) -> String {
    let trimmed = value.trim();
    if trimmed.len() == value.len() {
        value
    } else {
        trimmed.to_owned()
    }
}
