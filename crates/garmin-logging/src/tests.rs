use super::*;

fn cursor(store: &Store, sequence: u64) -> Cursor {
    Cursor {
        epoch: store.0.epoch,
        sequence,
    }
}

fn record(sequence: u64, message: &str) -> Record {
    Record {
        sequence: 0,
        timestamp_ms: sequence,
        level: Level::Info,
        component: "test".into(),
        source: "test".into(),
        session: "test-session".into(),
        source_sequence: sequence,
        message: message.into(),
        fields: BTreeMap::new(),
    }
}

#[test]
fn filters_advance_cursors_and_retries_do_not_duplicate_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store
        .append(vec![record(1, "first"), record(2, "second")])
        .unwrap();
    store.append(vec![record(2, "second")]).unwrap();
    let filter = Filter {
        text: "FIRST".into(),
        ..Filter::default()
    };
    let batch = store.history(&filter, None);
    assert_eq!(batch.records.len(), 1);
    assert_eq!(batch.cursor.sequence, 2);
    assert!(
        store
            .history(&filter, Some(batch.cursor))
            .records
            .is_empty()
    );
    assert!(!store.history(&filter, Some(batch.cursor)).gap);
    drop(store);
    let reopened = Store::open(directory.path(), "test").unwrap();
    assert_eq!(reopened.history(&Filter::default(), None).records.len(), 2);
}

#[tokio::test]
async fn rpc_history_live_handoff_and_export_are_ordered() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store.append(vec![record(1, "history")]).unwrap();
    let client = store.client();
    let initial = client.history(Filter::default(), None).await.unwrap();
    store
        .append(vec![record(2, "between query and subscription")])
        .unwrap();
    let mut receiver = client
        .subscribe(Filter::default(), Some(initial.cursor))
        .await
        .unwrap();
    let next = receiver.recv().await.unwrap().unwrap();
    assert_eq!(next.records[0].source_sequence, 2);
    assert!(!next.gap);
    drop(receiver);
    let mut export = client.export(Filter::default()).await.unwrap();
    let batch = export.recv().await.unwrap().unwrap();
    assert_eq!(batch.records.len(), 2);
    assert!(export.recv().await.unwrap().is_none());
}

#[test]
fn expired_and_future_cursors_report_gaps() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store
        .append(vec![record(1, "expired"), record(2, "retained")])
        .unwrap();
    store.0.state.lock().unwrap().history.expire_before(2);
    assert!(
        store
            .history(&Filter::default(), Some(cursor(&store, 0)))
            .gap
    );
    assert!(
        store
            .history(&Filter::default(), Some(cursor(&store, u64::MAX)))
            .gap
    );
}

#[test]
fn tracing_collection_flushes_through_the_bounded_writer() {
    use tracing_subscriber::prelude::*;
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    let subscriber = tracing_subscriber::registry().with(store.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!(answer = 42, "structured diagnostic");
    });
    store.flush();
    let batch = store.history(&Filter::default(), None);
    assert_eq!(batch.records.len(), 1);
    assert_eq!(batch.records[0].fields["answer"], "42");
}

#[test]
fn rotation_expires_the_same_history_on_disk_and_in_the_service() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store.append(vec![record(1, "first segment")]).unwrap();
    for sequence in 2..=5 {
        store.0.state.lock().unwrap().bytes = SEGMENT_BYTES;
        store
            .append(vec![record(sequence, "next segment")])
            .unwrap();
    }
    let batch = store.history(&Filter::default(), Some(cursor(&store, 0)));
    assert!(batch.gap);
    assert_eq!(batch.records.len(), 4);
    assert_eq!(batch.records[0].source_sequence, 2);
    store.flush();
    drop(store);
    let reopened = Store::open(directory.path(), "test").unwrap();
    assert_eq!(reopened.history(&Filter::default(), None).records.len(), 4);
    reopened
        .append(vec![record(5, "retried after restart")])
        .unwrap();
    assert_eq!(reopened.history(&Filter::default(), None).records.len(), 4);
}

#[test]
fn interrupted_last_record_does_not_prevent_future_collection() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("log-0.jsonl"), b"{partial").unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store.append(vec![record(1, "recovered")]).unwrap();
    store.flush();
    drop(store);
    let reopened = Store::open(directory.path(), "test").unwrap();
    let batch = reopened.history(&Filter::default(), None);
    assert_eq!(batch.records.len(), 1);
    assert!(batch.error.is_some());
}

fn failed_disk_keeps_bounded_history(rotation: bool) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store.append(vec![record(1, "persisted")]).unwrap();
    if rotation {
        // A directory where the oldest segment should be prevents rotation.
        fs::create_dir(directory.path().join("log-3.jsonl")).unwrap();
        store.0.state.lock().unwrap().bytes = SEGMENT_BYTES;
    } else {
        // A read-only handle fails writes deterministically, including as root.
        store.0.state.lock().unwrap().file =
            File::open(directory.path().join("log-0.jsonl")).unwrap();
    }
    let end = u64::try_from(history::RECORD_LIMIT).unwrap() + 257;
    for start in (2..end).step_by(BATCH_LIMIT) {
        store
            .append(
                (start..(start + 256).min(end))
                    .map(|seq| record(seq, "memory only"))
                    .collect(),
            )
            .unwrap();
    }
    let batch = store.history(&Filter::default(), Some(cursor(&store, 0)));
    assert!(batch.gap);
    assert!(
        batch
            .error
            .as_deref()
            .unwrap()
            .contains("persistence stopped")
    );
    assert_eq!(
        store.0.state.lock().unwrap().history.records.len(),
        history::RECORD_LIMIT
    );
    assert_eq!(store.0.state.lock().unwrap().next, end);
    assert!(!store.0.state.lock().unwrap().disk_available);
    // Retries within the retained horizon are still acknowledged exactly once.
    store.append(vec![record(end - 1, "retry")]).unwrap();
    assert_eq!(store.0.state.lock().unwrap().next, end);
    if rotation {
        fs::remove_dir(directory.path().join("log-3.jsonl")).unwrap();
    }
    let previous = cursor(&store, end - 1);
    drop(store);
    let reopened = Store::open(directory.path(), "test").unwrap();
    assert!(reopened.history(&Filter::default(), Some(previous)).gap);
    reopened
        .append(vec![record(end, "disk recovered")])
        .unwrap();
    assert!(reopened.history(&Filter::default(), None).error.is_none());
}

#[test]
fn failed_writes_do_not_grow_memory_forever() {
    failed_disk_keeps_bounded_history(false);
}

#[test]
fn failed_rotation_does_not_grow_memory_forever() {
    failed_disk_keeps_bounded_history(true);
}

#[test]
fn large_records_hit_the_byte_limit_before_the_record_limit() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store.0.state.lock().unwrap().file = File::open(directory.path().join("log-0.jsonl")).unwrap();
    let message = "x".repeat(15 * 1024);
    for seq in 1..=2300 {
        store.append(vec![record(seq, &message)]).unwrap();
    }
    assert!(
        store
            .history(&Filter::default(), Some(cursor(&store, 0)))
            .gap
    );
    let state = store.0.state.lock().unwrap();
    assert!(state.history.records.len() < 2300);
    let bytes: usize = state
        .history
        .records
        .iter()
        .map(|record| serde_json::to_vec(record).unwrap().len() + 1)
        .sum();
    assert!(bytes <= history::BYTE_LIMIT);
}

#[test]
fn source_churn_expires_deduplication_with_retention() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    for seq in 1..=1300 {
        let mut entry = record(1, "new browser session");
        entry.session = format!("session-{seq}");
        // Admit more than the old permanent 1024-source cap, then rotate them out.
        if seq >= 1200 && seq % 20 == 0 {
            store.0.state.lock().unwrap().bytes = SEGMENT_BYTES;
        }
        store.append(vec![entry]).unwrap();
    }
    let mut retained = record(1, "retry latest");
    retained.session = "session-1300".into();
    store.append(vec![retained.clone()]).unwrap();
    assert_eq!(store.0.state.lock().unwrap().next, 1301);
    let mut expired = record(1, "retry outside retention");
    expired.session = "session-1".into();
    store.append(vec![expired]).unwrap();
    assert_eq!(store.0.state.lock().unwrap().next, 1302);
    drop(store);
    let reopened = Store::open(directory.path(), "test").unwrap();
    reopened.append(vec![retained]).unwrap();
    assert_eq!(reopened.0.state.lock().unwrap().next, 1302);
}

#[test]
fn concurrent_stores_rotate_separate_histories_and_reuse_released_slots() {
    let directory = tempfile::tempdir().unwrap();
    let first = Store::open(directory.path(), "first").unwrap();
    let second = Store::open(directory.path(), "second").unwrap();
    let slot = second.directory().to_path_buf();
    assert_ne!(first.directory(), second.directory());
    first.append(vec![record(1, "first")]).unwrap();
    for seq in 1..=6 {
        if seq > 1 {
            second.0.state.lock().unwrap().bytes = SEGMENT_BYTES;
        }
        second.append(vec![record(seq, "second")]).unwrap();
    }
    assert_eq!(
        first.history(&Filter::default(), None).records[0].message,
        "first"
    );
    drop(second);
    let reused = Store::open(directory.path(), "second again").unwrap();
    assert_eq!(reused.directory(), slot);
    let batch = reused.history(&Filter::default(), Some(cursor(&reused, 0)));
    assert!(batch.gap);
    assert_eq!(batch.records.len(), 4);
    assert_eq!(batch.cursor.sequence, 6);
    drop(first);
    let reopened = Store::open(directory.path(), "first again").unwrap();
    assert_eq!(reopened.directory(), directory.path());
    assert_eq!(reopened.history(&Filter::default(), None).records.len(), 1);
}

#[test]
fn concurrent_process_writer() {
    let Some(directory) = std::env::var_os("GARMIN_LOG_TEST_DIRECTORY") else {
        return;
    };
    let store = Store::open(&directory, "child").unwrap();
    assert_ne!(store.directory(), Path::new(&directory));
    for seq in 1..=6 {
        if seq > 1 {
            store.0.state.lock().unwrap().bytes = SEGMENT_BYTES;
        }
        store.append(vec![record(seq, "child")]).unwrap();
    }
}

#[test]
fn process_lock_protects_another_process_history() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "parent").unwrap();
    store.append(vec![record(1, "parent")]).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "tests::concurrent_process_writer", "--nocapture"])
        .env("GARMIN_LOG_TEST_DIRECTORY", directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let child = Store::open(directory.path(), "child history").unwrap();
    assert_eq!(child.history(&Filter::default(), None).records.len(), 4);
    drop(store);
    let parent = Store::open(directory.path(), "parent history").unwrap();
    let batch = parent.history(&Filter::default(), None);
    assert_eq!(batch.records.len(), 1);
    assert_eq!(batch.records[0].message, "parent");
}

#[tokio::test]
async fn ingest_rejects_invalid_records_without_blocking_valid_records_or_retries() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    let client = store.client();
    let records = vec![
        record(1, "before"),
        record(2, &"🦀".repeat(4096)),
        record(0, "invalid identity"),
        record(3, "after"),
    ];
    for _ in 0..2 {
        let report = client.ingest(records.clone()).await.unwrap().unwrap();
        assert_eq!(report.rejected, 2);
        let batch = store.history(&Filter::default(), None);
        assert_eq!(batch.records.len(), 2);
        assert_eq!(batch.records[1].message, "after");
        assert_eq!(batch.cursor.sequence, 2);
    }
}

#[test]
fn reopen_detects_a_stale_cursor_even_after_reusing_its_sequence() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store
        .append((1..=10).map(|seq| record(seq, "persisted")).collect())
        .unwrap();
    store.0.state.lock().unwrap().file = File::open(directory.path().join("log-0.jsonl")).unwrap();
    store
        .append((11..=20).map(|seq| record(seq, "memory only")).collect())
        .unwrap();
    let old = store.history(&Filter::default(), None).cursor;
    assert_eq!(old.sequence, 20);
    drop(store);

    let reopened = Store::open(directory.path(), "test").unwrap();
    reopened
        .append((11..=25).map(|seq| record(seq, "after restart")).collect())
        .unwrap();
    let batch = reopened.history(&Filter::default(), Some(old));
    assert!(batch.gap);
    assert_ne!(batch.cursor.epoch, old.epoch);
    assert_eq!(batch.cursor.sequence, 25);
    assert_eq!(
        batch
            .records
            .iter()
            .filter(|r| r.message == "after restart")
            .count(),
        15
    );
    let resumed = reopened.history(&Filter::default(), Some(batch.cursor));
    assert!(!resumed.gap);
    assert!(resumed.records.is_empty());
}

#[tokio::test]
async fn exports_report_retention_loss_between_batches() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    for start in [1, 201, 401] {
        store
            .append(
                (start..start + 200)
                    .map(|seq| record(seq, "export"))
                    .collect(),
            )
            .unwrap();
    }
    let mut export = store.stream(Filter::default(), None, false);
    let first = export.recv().await.unwrap().unwrap();
    assert_eq!(first.records.len(), BATCH_LIMIT);
    assert!(!first.gap);
    store.0.state.lock().unwrap().history.expire_before(513);
    let next = export.recv().await.unwrap().unwrap();
    assert!(next.gap);
    assert_eq!(next.cursor.sequence, 600);
    assert!(export.recv().await.unwrap().is_none());
}

#[tokio::test]
async fn exports_report_storage_failure_even_with_live_records_available() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), "test").unwrap();
    store.0.state.lock().unwrap().file = File::open(directory.path().join("log-0.jsonl")).unwrap();
    store.append(vec![record(1, "memory only")]).unwrap();
    let mut export = store.stream(Filter::default(), None, false);
    let batch = export.recv().await.unwrap().unwrap();
    assert_eq!(batch.records.len(), 1);
    assert!(
        batch
            .error
            .as_deref()
            .unwrap()
            .contains("persistence stopped")
    );
}
