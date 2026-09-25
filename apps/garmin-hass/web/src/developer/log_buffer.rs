//! Bounded delivery state, independent of browser APIs and backend storage.
use std::collections::{BTreeMap, VecDeque};

use garmin_model::logging::{Level, MAX_RECORD_BYTES, Record};

const CAPACITY: usize = 512;
const BATCH_SIZE: usize = 128;

pub(super) struct Batch {
    through: u64,
    pub records: Vec<Record>,
}

pub(super) struct Buffer {
    session: String,
    sequence: u64,
    next_id: u64,
    dropped: u64,
    rejected: u64,
    queue: VecDeque<(u64, Record)>,
}

impl Buffer {
    pub fn new(session: String) -> Self {
        Self {
            session,
            sequence: 0,
            next_id: 0,
            dropped: 0,
            rejected: 0,
            queue: VecDeque::new(),
        }
    }

    pub fn record(
        &mut self,
        timestamp_ms: u64,
        level: Level,
        component: &str,
        source: &str,
        message: &str,
        fields: BTreeMap<String, String>,
    ) -> Record {
        self.sequence += 1;
        Record {
            sequence: 0,
            timestamp_ms,
            level,
            component: component.chars().take(256).collect(),
            source: source.into(),
            session: self.session.clone(),
            source_sequence: self.sequence,
            message: message.chars().take(4096).collect(),
            fields,
        }
    }

    pub fn push(&mut self, record: Record) {
        // Budget the complete backend representation, including its source prefix
        // and largest sequence. Character limits alone do not bound JSON bytes.
        let mut encoded_record = record.clone();
        encoded_record.sequence = u64::MAX;
        encoded_record.source = format!("browser/{}", record.source.trim_start_matches("browser/"));
        if record.source_sequence == 0
            || !matches!(serde_json::to_vec(&encoded_record), Ok(bytes) if bytes.len() <= MAX_RECORD_BYTES)
        {
            self.rejected += 1;
            return;
        }
        if self.queue.len() == CAPACITY {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.next_id += 1;
        self.queue.push_back((self.next_id, record));
    }

    pub fn pending(&mut self, timestamp_ms: u64) -> Batch {
        if self.rejected > 0 {
            let count = std::mem::take(&mut self.rejected);
            let warning = self.record(
                timestamp_ms,
                Level::Warn,
                "logging",
                "main",
                &format!("{count} invalid or oversized diagnostics dropped"),
                BTreeMap::new(),
            );
            self.push(warning);
        }
        if self.dropped > 0 {
            if self.queue.len() == CAPACITY {
                self.queue.pop_front();
                self.dropped += 1;
            }
            let count = std::mem::take(&mut self.dropped);
            let warning = self.record(
                timestamp_ms,
                Level::Warn,
                "logging",
                "main",
                &format!("{count} diagnostics dropped while the backend was unavailable"),
                BTreeMap::new(),
            );
            self.push(warning);
        }
        let entries: Vec<_> = self.queue.iter().take(BATCH_SIZE).collect();
        Batch {
            through: entries.last().map_or(0, |(id, _)| *id),
            records: entries
                .into_iter()
                .map(|(_, record)| record.clone())
                .collect(),
        }
    }

    // Permanent backend rejection consumes the batch and becomes a visible warning.
    // Transport failures do not call this or acknowledge, preserving retry identity.
    pub fn reject(&mut self, count: u64) {
        self.rejected = self.rejected.saturating_add(count);
    }

    pub fn acknowledge(&mut self, batch: &Batch) {
        while self
            .queue
            .front()
            .is_some_and(|(id, _)| *id <= batch.through)
        {
            self.queue.pop_front();
        }
    }
}
