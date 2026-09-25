//! Structured application diagnostics, independent of their storage and transport.

use std::collections::BTreeMap;

pub const MAX_RECORD_BYTES: usize = 16 * 1024;

/// A sequence is meaningful only within the store incarnation that issued it.
#[garmin_macros::portable(copy, default, eq)]
pub struct Cursor {
    pub epoch: [u8; 16],
    pub sequence: u64,
}

/// Valid records were accepted; permanently invalid records must not be retried.
#[garmin_macros::portable(default)]
pub struct IngestReport {
    pub rejected: u64,
}

#[garmin_macros::portable(copy, default, ord)]
pub enum Level {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

#[garmin_macros::portable]
pub struct Record {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub level: Level,
    pub component: String,
    pub source: String,
    pub session: String,
    pub source_sequence: u64,
    pub message: String,
    pub fields: BTreeMap<String, String>,
}

#[garmin_macros::portable(default, eq)]
pub struct Filter {
    pub minimum: Level,
    pub component: String,
    pub source: String,
    pub session: String,
    pub text: String,
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
}

impl Filter {
    #[must_use]
    pub fn matches(&self, record: &Record) -> bool {
        record.level >= self.minimum
            && record.component.contains(&self.component)
            && record.source.contains(&self.source)
            && record.session.contains(&self.session)
            && self.since_ms.is_none_or(|time| record.timestamp_ms >= time)
            && self.until_ms.is_none_or(|time| record.timestamp_ms <= time)
            && (self.text.is_empty()
                || record
                    .message
                    .to_lowercase()
                    .contains(&self.text.to_lowercase())
                || record.fields.iter().any(|(key, value)| {
                    format!("{key}={value}")
                        .to_lowercase()
                        .contains(&self.text.to_lowercase())
                }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_resume_and_ingest_results_round_trip_through_postcard() {
        let cursor = Cursor {
            epoch: [42; 16],
            sequence: 123,
        };
        let decoded: Cursor = postcard::from_bytes(&postcard::to_stdvec(&cursor).unwrap()).unwrap();
        assert_eq!(decoded, cursor);
        let report = IngestReport { rejected: 2 };
        let decoded: IngestReport =
            postcard::from_bytes(&postcard::to_stdvec(&report).unwrap()).unwrap();
        assert_eq!(decoded, report);
    }

    #[test]
    fn records_and_filters_round_trip_through_postcard() {
        let record = Record {
            sequence: 7,
            timestamp_ms: 1234,
            level: Level::Warn,
            component: "map".into(),
            source: "worker".into(),
            session: "session".into(),
            source_sequence: 2,
            message: "diagnostic".into(),
            fields: BTreeMap::from([("tile".into(), "3/4/5".into())]),
        };
        let decoded: Record = postcard::from_bytes(&postcard::to_stdvec(&record).unwrap()).unwrap();
        assert_eq!(decoded.sequence, record.sequence);
        assert_eq!(decoded.fields, record.fields);
        let filter = Filter {
            minimum: Level::Warn,
            component: "map".into(),
            source: "worker".into(),
            session: "session".into(),
            text: "TILE".into(),
            since_ms: Some(1000),
            until_ms: Some(2000),
        };
        let decoded: Filter = postcard::from_bytes(&postcard::to_stdvec(&filter).unwrap()).unwrap();
        assert_eq!(filter, decoded);
        assert!(decoded.matches(&record));
    }
}
