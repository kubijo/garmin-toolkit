//! Memory retention and retry deduplication share one bounded horizon.
use garmin_model::{artifact::ByteCount, logging::Record};
use std::collections::{BTreeMap, VecDeque};

pub(super) const BYTE_LIMIT: ByteCount = ByteCount::from_mebibytes(32);
pub(super) const RECORD_LIMIT: usize = 16_384;

#[derive(Default)]
pub(super) struct History {
    pub records: VecDeque<Record>,
    sizes: VecDeque<ByteCount>,
    bytes: ByteCount,
    sources: BTreeMap<(String, String), (u64, u64)>,
}

impl History {
    pub fn contains(&self, record: &Record) -> bool {
        self.sources
            .get(&(record.session.clone(), record.source.clone()))
            .is_some_and(|(sequence, _)| *sequence >= record.source_sequence)
    }

    pub fn push(&mut self, record: Record, bytes: ByteCount) {
        self.sources.insert(
            (record.session.clone(), record.source.clone()),
            (record.source_sequence, record.sequence),
        );
        self.records.push_back(record);
        self.sizes.push_back(bytes);
        self.bytes = self.bytes.saturating_add(bytes);
        while self.records.len() > RECORD_LIMIT || self.bytes > BYTE_LIMIT {
            self.pop();
        }
    }

    pub fn expire_before(&mut self, first: u64) {
        while self
            .records
            .front()
            .is_some_and(|record| record.sequence < first)
        {
            self.pop();
        }
    }

    fn pop(&mut self) {
        if let Some(record) = self.records.pop_front() {
            self.bytes = self
                .bytes
                .saturating_sub(self.sizes.pop_front().expect("every record has a size"));
            let key = (record.session, record.source);
            if self
                .sources
                .get(&key)
                .is_some_and(|(_, last)| *last == record.sequence)
            {
                self.sources.remove(&key);
            }
        }
    }
}
