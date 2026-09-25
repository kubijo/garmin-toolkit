//! Typed HTTP envelopes and cursor conversion;
//! these types do not cross Remoc.
use garmin_model::logging;
use serde::{Serialize, Serializer};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub epoch: uuid::Uuid,
    pub sequence: u64,
}

impl fmt::Display for Cursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.epoch.simple(), self.sequence)
    }
}

impl Serialize for Cursor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl FromStr for Cursor {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (epoch, sequence) = value.split_once(':').ok_or("invalid cursor")?;
        Ok(Self {
            epoch: uuid::Uuid::parse_str(epoch).map_err(|_| "invalid cursor epoch")?,
            sequence: sequence.parse().map_err(|_| "invalid cursor sequence")?,
        })
    }
}

impl From<logging::Cursor> for Cursor {
    fn from(value: logging::Cursor) -> Self {
        Self {
            epoch: uuid::Uuid::from_bytes(value.epoch),
            sequence: value.sequence,
        }
    }
}

impl From<Cursor> for logging::Cursor {
    fn from(value: Cursor) -> Self {
        Self {
            epoch: *value.epoch.as_bytes(),
            sequence: value.sequence,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Batch<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Vec<T>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_revision: Option<Cursor>,
    pub records: Vec<T>,
    pub cursor: Cursor,
    pub gap: bool,
    pub more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Reply {
    Logs(Batch<logging::Record>),
    Events(Batch<super::events::Event>),
}

impl Reply {
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Logs(batch) => batch.error.as_deref(),
            Self::Events(batch) => batch.error.as_deref(),
        }
    }

    pub fn cursor(&self) -> Cursor {
        match self {
            Self::Logs(batch) => batch.cursor,
            Self::Events(batch) => batch.cursor,
        }
    }

    pub fn has_updates(&self) -> bool {
        match self {
            Self::Logs(batch) => !batch.records.is_empty() || batch.gap,
            Self::Events(batch) => batch.state.is_some() || !batch.records.is_empty() || batch.gap,
        }
    }

    pub fn omit_unchanged_snapshot(&mut self, previous: &mut Option<Vec<super::events::Event>>) {
        if let Self::Events(batch) = self {
            if batch.state == *previous && !batch.gap {
                batch.state = None;
            } else {
                previous.clone_from(&batch.state);
            }
        }
    }
}
