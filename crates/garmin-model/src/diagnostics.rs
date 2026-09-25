//! Compact, postcard-compatible diagnostic observations.
//! Sources own revisions;
//! the HTTP broker assigns its own cursor independently.

use std::collections::BTreeMap;

#[garmin_macros::portable(eq)]
pub struct Observation {
    pub kind: String,
    pub window: String,
    pub fields: BTreeMap<String, String>,
    pub removed: bool,
}

#[garmin_macros::portable(default)]
pub struct Update {
    pub revision: u64,
    pub state: Vec<Observation>,
    pub changes: Vec<Observation>,
    pub gap: bool,
    pub incomplete: bool,
}

impl Observation {
    #[must_use]
    pub fn valid(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "connection" | "window" | "automation" | "renderer"
        ) && self.window.len() <= 128
            && self.fields.len() <= 16
            && self
                .fields
                .iter()
                .all(|(key, value)| key.len() <= 64 && value.len() <= 2048)
            && self
                .fields
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
                <= 3000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_updates_round_trip_over_postcard() {
        let observation = Observation {
            kind: "renderer".into(),
            window: "root".into(),
            fields: BTreeMap::from([("status".into(), "ready".into())]),
            removed: false,
        };
        let update = Update {
            revision: 42,
            state: vec![observation.clone()],
            changes: vec![observation],
            gap: true,
            incomplete: true,
        };
        let decoded: Update = postcard::from_bytes(&postcard::to_stdvec(&update).unwrap()).unwrap();
        assert_eq!(decoded.revision, 42);
        assert_eq!(decoded.state, update.state);
        assert_eq!(decoded.changes, update.changes);
        assert!(decoded.gap);
        assert!(decoded.incomplete);
    }
}
