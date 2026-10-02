//! Exercise browser delivery bookkeeping on the native test runner.
#[path = "../src/developer/log_buffer.rs"]
mod log_buffer;

use std::collections::BTreeMap;

use garmin_model::logging::Level;
use log_buffer::Buffer;

fn append(buffer: &mut Buffer, message: &str) {
    let record = buffer.record(42, Level::Info, "test", "main", message, BTreeMap::new());
    buffer.push(record);
}

#[test]
fn retries_keep_identity_and_acknowledgement_preserves_new_records() {
    let mut buffer = Buffer::new("page".into());
    append(&mut buffer, "first");
    let batch = buffer.pending(43);
    let retry = buffer.pending(44);
    assert_eq!(batch.records[0], retry.records[0]);
    append(&mut buffer, "second");
    buffer.acknowledge(&batch);
    let remaining = buffer.pending(45);
    assert_eq!(remaining.records.len(), 1);
    assert_eq!(remaining.records[0].message, "second");
    assert_eq!(remaining.records[0].source_sequence, 2);
    buffer.acknowledge(&remaining);
    assert!(buffer.pending(46).records.is_empty());
}

#[test]
fn worker_records_keep_their_original_identity_and_fields() {
    let mut worker = Buffer::new("worker-session".into());
    let record = worker.record(
        12,
        Level::Warn,
        "map",
        "worker",
        "warning",
        BTreeMap::from([("tile".into(), "1/2/3".into())]),
    );
    let mut page = Buffer::new("page".into());
    page.push(record.clone());
    assert_eq!(page.pending(13).records, vec![record]);
}

#[test]
fn overflow_is_bounded_and_reported_once_across_retries() {
    let mut buffer = Buffer::new("page".into());
    for _ in 0..600 {
        append(&mut buffer, "event");
    }
    let first = buffer.pending(43);
    assert_eq!(first.records.len(), 128);
    assert_eq!(first.records[0].source_sequence, 90);
    assert_eq!(first.records, buffer.pending(44).records);
    let mut records = Vec::new();
    loop {
        let batch = buffer.pending(45);
        if batch.records.is_empty() {
            break;
        }
        records.extend(batch.records.clone());
        buffer.acknowledge(&batch);
    }
    assert_eq!(records.len(), 512);
    let warnings: Vec<_> = records.iter().filter(|r| r.level == Level::Warn).collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.starts_with("89 diagnostics dropped"));
}

#[test]
fn acknowledgement_during_overflow_does_not_remove_unsent_records() {
    let mut buffer = Buffer::new("page".into());
    append(&mut buffer, "in-flight");
    let batch = buffer.pending(43);
    for _ in 0..600 {
        append(&mut buffer, "new");
    }
    buffer.acknowledge(&batch);
    assert_eq!(buffer.pending(44).records[0].source_sequence, 91);
}

#[test]
fn record_limits_preserve_unicode_boundaries() {
    let mut buffer = Buffer::new("page".into());
    let record = buffer.record(
        42,
        Level::Error,
        &"é".repeat(300),
        "main",
        &"é".repeat(5000),
        BTreeMap::new(),
    );
    assert_eq!(record.component.chars().count(), 256);
    assert_eq!(record.message.chars().count(), 4096);
}

#[test]
fn oversized_unicode_escaped_text_and_fields_do_not_block_following_records() {
    let mut buffer = Buffer::new("page".into());
    append(&mut buffer, &"🦀".repeat(4096));
    append(&mut buffer, &"\0".repeat(4096));
    let fields = BTreeMap::from([("detail".into(), "x".repeat(32 * 1024))]);
    let record = buffer.record(42, Level::Error, "test", "main", "huge field", fields);
    buffer.push(record);
    append(&mut buffer, "valid after oversized records");
    let batch = buffer.pending(43);
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.records[0].message, "valid after oversized records");
    assert!(
        batch.records[1]
            .message
            .starts_with("3 invalid or oversized diagnostics dropped")
    );
    assert_eq!(batch.records, buffer.pending(44).records);
    for mut record in batch.records.clone() {
        record.source = format!("browser/{}", record.source);
        record.sequence = u64::MAX;
        assert!(
            serde_json::to_vec(&record).unwrap().len() <= garmin_model::logging::MAX_RECORD_BYTES
        );
    }
    buffer.acknowledge(&batch);
    assert!(buffer.pending(45).records.is_empty());
}

#[test]
fn permanent_backend_rejection_is_reported_without_retrying_the_old_batch() {
    let mut buffer = Buffer::new("page".into());
    append(&mut buffer, "sent");
    let batch = buffer.pending(43);
    buffer.acknowledge(&batch);
    buffer.reject(1);
    append(&mut buffer, "next");
    let next = buffer.pending(44);
    assert_eq!(next.records.len(), 2);
    assert_eq!(next.records[0].message, "next");
    assert!(
        next.records[1]
            .message
            .starts_with("1 invalid or oversized")
    );
    buffer.acknowledge(&next);
    assert!(buffer.pending(45).records.is_empty());
}
