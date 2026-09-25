use super::*;

#[test]
fn stopping_collection_releases_the_sink_and_allows_a_fresh_observer() {
    let context = Context::default();
    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = collected.clone();
    let observer = install(&context, move |item| sink.lock().unwrap().push(item));
    publish(&context, item("window", "open"));
    assert_eq!(collected.lock().unwrap().len(), 1);
    observer.stop();
    assert_eq!(Arc::strong_count(&collected), 1);
    publish(&context, item("automation", "passed"));
    assert_eq!(collected.lock().unwrap().len(), 1);
    let sink = collected.clone();
    let restarted = install(&context, move |item| sink.lock().unwrap().push(item));
    observer.stop();
    publish(&context, item("window", "reopened"));
    assert_eq!(collected.lock().unwrap().len(), 2);
    restarted.stop();
    assert_eq!(Arc::strong_count(&collected), 1);
}

fn item(kind: &str, value: &str) -> Observation {
    Observation {
        kind: kind.into(),
        window: "child".into(),
        fields: BTreeMap::from([("state".into(), value.into())]),
        removed: false,
    }
}

#[test]
fn source_journal_retains_terminal_transitions_and_removes_child_state() {
    let journal = Journal::default();
    journal.publish(item("window", "open"));
    journal.publish(item("automation", "passed"));
    for completed in 0..1000 {
        let mut progress = item("automation", "running");
        progress
            .fields
            .insert("completed".into(), completed.to_string());
        journal.publish(progress);
    }
    let update = journal.since(0);
    assert!(!update.gap);
    assert_eq!(update.changes.len(), 3);
    assert_eq!(update.changes[1].fields["state"], "passed");
    let mut closed = item("window", "closed");
    closed.removed = true;
    journal.publish(closed);
    let update = journal.since(update.revision);
    assert!(update.state.is_empty());
    assert_eq!(update.changes.len(), 2);
    assert!(update.changes.iter().all(|item| item.removed));
}

#[test]
fn retention_gap_includes_a_fresh_snapshot() {
    let journal = Journal::default();
    for index in 0..140 {
        journal.publish(item("renderer", &index.to_string()));
    }
    let update = journal.since(1);
    assert!(update.gap);
    assert_eq!(update.state[0].fields["state"], "139");
    assert_eq!(update.changes.len(), 128);
}

#[test]
fn rejected_state_advances_revision_and_marks_snapshots_incomplete() {
    let journal = Journal::default();
    for index in 0..128 {
        let mut observation = item("window", "open");
        observation.window = index.to_string();
        journal.publish(observation);
    }
    let full = journal.since(0);
    assert!(!full.incomplete);
    journal.publish(item("window", "open"));
    let rejected = journal.since(full.revision);
    assert!(rejected.incomplete);
    assert!(rejected.revision > full.revision);
    assert_eq!(rejected.state.len(), 128);
    assert!(rejected.changes.is_empty());
    assert!(journal.since(rejected.revision).incomplete);
    journal.publish(item("window", "open"));
    assert_eq!(journal.since(0).revision, rejected.revision);
}
