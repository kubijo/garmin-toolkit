#[path = "../src/files/catalog.rs"]
mod catalog;
#[path = "../src/files/requests.rs"]
mod requests;

#[test]
fn closing_a_window_revokes_its_pending_upload_picker() {
    let mut requests = requests::Requests::default();
    let picker = requests.begin(1).expect("start picker");
    let catalog = requests
        .catalog
        .begin("synthetic-device")
        .expect("start catalog");
    requests.close();
    // The upload callback checks this before starting its RPC.
    assert_ne!(
        requests.active(),
        Some(picker),
        "closed picker must not submit an upload"
    );
    assert!(!requests.catalog.finish(catalog));
    assert_eq!(requests.catalog.loading(), None);
}

#[test]
fn late_picker_cancellation_cannot_finish_a_reopened_windows_operation() {
    let mut requests = requests::Requests::default();
    let old = requests.begin(1).expect("old picker");
    requests.close();
    let current = requests.begin(1).expect("picker after reopen");
    assert!(!requests.finish(old, 1));
    assert_eq!(requests.active(), Some(current));
    assert!(requests.finish(current, 1));
    assert_eq!(requests.active(), None);
}

#[test]
fn reconnect_invalidates_completions_and_admits_only_one_new_operation() {
    let mut requests = requests::Requests::default();
    let old = requests.begin(1).expect("start operation");
    assert!(requests.begin(1).is_none());
    assert!(!requests.finish(old, 2));
    assert!(requests.interrupt());
    assert!(!requests.interrupt());
    let current = requests.begin(2).expect("start reconnected operation");
    assert!(!requests.finish(old, 2));
    assert_eq!(requests.active(), Some(current));
    assert!(requests.finish(current, 2));
}
