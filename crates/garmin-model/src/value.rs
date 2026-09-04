//! Shared scalar value objects.

use std::{fmt, str::FromStr};

use semver::Version;
use thiserror::Error;

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

/// A named implementation and its semantic version.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComponentVersion {
    name: String,
    version: Version,
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
