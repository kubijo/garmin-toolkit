//! Bounded HTTP history, sharing the existing filter and retention semantics.

use super::Store;
use garmin_model::logging::{Cursor, Filter};
use garmin_service_api::logging::Batch;

impl Store {
    /// Read the newest matching records, or continue forward from a cursor.
    /// The cursor advances across nonmatching records as well.
    #[must_use]
    pub fn recent(&self, filter: &Filter, after: Option<Cursor>, limit: usize) -> (Batch, bool) {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = state
            .history
            .records
            .front()
            .map_or(state.next, |record| record.sequence);
        let gap = after.is_some_and(|cursor| {
            cursor.epoch != self.0.epoch
                || cursor.sequence.saturating_add(1) < first
                || cursor.sequence >= state.next
        });
        let start = if gap {
            None
        } else {
            after.map(|cursor| cursor.sequence)
        };
        let mut matching: Vec<_> = state
            .history
            .records
            .iter()
            .filter(|record| {
                start.is_none_or(|sequence| record.sequence > sequence) && filter.matches(record)
            })
            .collect();
        let limit = limit.clamp(1, 256);
        if start.is_none() {
            matching.reverse();
        }
        let mut records = Vec::new();
        let mut bytes = 0;
        for record in matching.into_iter().take(limit) {
            let size = serde_json::to_vec(record).map_or(0, |data| data.len());
            if bytes + size > 256 * 1024 {
                break;
            }
            bytes += size;
            records.push(record.clone());
        }
        if start.is_none() {
            records.reverse();
        }
        let last = records
            .last()
            .map_or(start.unwrap_or(state.next - 1), |record| record.sequence);
        let more = state
            .history
            .records
            .iter()
            .any(|record| record.sequence > last && filter.matches(record));
        (
            Batch {
                records,
                cursor: Cursor {
                    epoch: self.0.epoch,
                    sequence: if more { last } else { state.next - 1 },
                },
                gap,
                error: state.error.clone(),
            },
            more,
        )
    }
}
