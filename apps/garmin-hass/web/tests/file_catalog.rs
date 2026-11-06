#[path = "../src/files/catalog.rs"]
mod catalog;

#[test]
fn closing_and_reopening_the_same_device_rejects_the_closed_windows_reply() {
    let mut request = catalog::Request::default();
    let closed = request.begin("watch").expect("open files");
    request.invalidate();
    let reopened = request.begin("watch").expect("reopen files");
    assert!(!request.finish(closed));
    assert_eq!(request.loading(), Some("watch"));
    assert!(request.finish(reopened));
    assert!(!request.finish(closed));
}

#[test]
fn switching_devices_supersedes_pending_catalogues_in_either_completion_order() {
    for old_first in [true, false] {
        let mut request = catalog::Request::default();
        let a = request.begin("a").expect("start A");
        assert!(request.begin("a").is_none());
        let b = request.begin("b").expect("start B while A is loading");
        if old_first {
            assert!(!request.finish(a));
            assert_eq!(request.loading(), Some("b"));
        }
        assert!(request.finish(b));
        assert_eq!(request.loading(), None);
        assert!(!request.finish(a));
        assert!(!request.finish(b));
    }
}

#[test]
fn logout_disconnect_and_returning_to_a_device_invalidate_old_requests() {
    let mut request = catalog::Request::default();
    let original = request.begin("a").expect("start A");
    let b = request.begin("b").expect("start B");
    let latest = request.begin("a").expect("return to A");
    assert!(!request.finish(original));
    assert!(!request.finish(b));
    assert_eq!(request.loading(), Some("a"));
    request.invalidate();
    assert!(!request.finish(latest));
    let reopened = request.begin("a").expect("reopen after invalidation");
    assert!(!request.finish(latest));
    assert!(request.finish(reopened));
}
