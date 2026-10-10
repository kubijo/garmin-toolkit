use std::sync::{
    Weak,
    atomic::{AtomicBool, Ordering},
};

use super::*;

#[derive(Default)]
struct Probe {
    state: Mutex<Weak<Inner>>,
    calls: Mutex<Vec<Instant>>,
    unavailable: AtomicBool,
}

impl Redraw for Probe {
    fn request(&self) -> bool {
        let inner = self.state.lock().unwrap().upgrade().unwrap();
        assert!(
            inner.state.try_lock().is_ok(),
            "native call held the state lock"
        );
        self.calls.lock().unwrap().push(Instant::now());
        !self.unavailable.load(Ordering::Relaxed)
    }
}

fn fixture() -> (Wake, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let wake = Wake::with_redraw(Handle::current(), Some(probe.clone()));
    *probe.state.lock().unwrap() = Arc::downgrade(&wake.0);
    (wake, probe)
}

async fn settle() {
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
}

async fn advance(milliseconds: u64) {
    tokio::time::advance(Duration::from_millis(milliseconds)).await;
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn native_calls_are_unlocked_and_request_churn_cannot_bypass_pacing() {
    let (wake, probe) = fixture();
    let first = wake.request();
    settle().await;
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
    drop(first);
    settle().await;
    for _ in 0..10 {
        let request = wake.request();
        advance(1).await;
        drop(request);
        settle().await;
    }
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
    let request = wake.request();
    advance(24).await;
    let calls = probe.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert!(calls[1] - calls[0] >= FRAME_INTERVAL);
    drop(request);
    settle().await;
    advance(1000).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 2);
    assert!(!wake.0.state.lock().unwrap().pumping);
}

#[tokio::test(start_paused = true)]
async fn requests_and_background_work_keep_waking_until_the_last_owner_releases() {
    let (wake, probe) = fixture();
    let first = wake.request();
    let second = wake.request();
    settle().await;
    drop(first);
    advance(34).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 2);
    wake.background(true);
    drop(second);
    advance(34).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 3);
    wake.background(false);
    settle().await;
    advance(1000).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 3);
    assert!(!wake.0.state.lock().unwrap().pumping);
}

#[tokio::test(start_paused = true)]
async fn stalled_background_run_expires_without_another_ui_update() {
    let (wake, probe) = fixture();
    wake.background(true);
    settle().await;
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
    tokio::time::advance(BACKGROUND_LEASE).await;
    settle().await;
    assert!(!wake.0.state.lock().unwrap().active());
    assert!(!wake.0.state.lock().unwrap().pumping);
    advance(1000).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 1);

    // A later HTTP request must not resurrect the expired background permission.
    let request = wake.request();
    settle().await;
    assert_eq!(probe.calls.lock().unwrap().len(), 2);
    drop(request);
    settle().await;
    advance(1000).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 2);
    assert!(!wake.0.state.lock().unwrap().pumping);
}

#[tokio::test(start_paused = true)]
async fn root_frames_renew_background_permission_without_bypassing_pacing() {
    let (wake, probe) = fixture();
    wake.background(true);
    settle().await;
    advance(1).await;
    wake.background(true);
    settle().await;
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
    for _ in 0..3 {
        tokio::time::advance(BACKGROUND_LEASE / 2).await;
        settle().await;
        assert!(wake.0.state.lock().unwrap().pumping);
        wake.background(true);
        settle().await;
    }
    assert_eq!(probe.calls.lock().unwrap().len(), 4);
    tokio::time::advance(BACKGROUND_LEASE).await;
    settle().await;
    assert!(!wake.0.state.lock().unwrap().pumping);
    assert_eq!(probe.calls.lock().unwrap().len(), 4);
}

#[tokio::test(start_paused = true)]
async fn background_expiry_does_not_release_a_pending_request() {
    let (wake, probe) = fixture();
    wake.background(true);
    let request = wake.request();
    settle().await;
    tokio::time::advance(BACKGROUND_LEASE).await;
    settle().await;
    assert!(wake.0.state.lock().unwrap().pumping);
    assert_eq!(probe.calls.lock().unwrap().len(), 2);
    drop(request);
    settle().await;
    advance(1000).await;
    assert!(!wake.0.state.lock().unwrap().pumping);
    assert_eq!(probe.calls.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn timed_out_response_stops_native_wakeups() {
    let (wake, probe) = fixture();
    let request = wake.request();
    let response = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(100), async move {
            let _request = request;
            std::future::pending::<()>().await;
        })
        .await
    });
    settle().await;
    advance(101).await;
    assert!(response.await.unwrap().is_err());
    settle().await;
    let count = probe.calls.lock().unwrap().len();
    advance(1000).await;
    assert_eq!(probe.calls.lock().unwrap().len(), count);
    assert!(!wake.0.state.lock().unwrap().pumping);
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_polled_response_stops_native_wakeups() {
    let (wake, probe) = fixture();
    let request = wake.request();
    let response = tokio::spawn(async move {
        let _request = request;
        std::future::pending::<()>().await;
    });
    settle().await;
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
    response.abort();
    assert!(response.await.unwrap_err().is_cancelled());
    settle().await;
    advance(1000).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
    assert!(!wake.0.state.lock().unwrap().active());
    assert!(!wake.0.state.lock().unwrap().pumping);
}

#[tokio::test(start_paused = true)]
async fn unavailable_native_window_does_not_leave_a_timer_running() {
    let (wake, probe) = fixture();
    probe.unavailable.store(true, Ordering::Relaxed);
    let request = wake.request();
    settle().await;
    advance(1000).await;
    assert_eq!(probe.calls.lock().unwrap().len(), 1);
    assert!(!wake.0.state.lock().unwrap().pumping);
    drop(request);
}

#[tokio::test(start_paused = true)]
async fn platforms_without_a_wayland_backend_never_start_a_wake_task() {
    let wake = Wake::with_redraw(Handle::current(), None);
    let request = wake.request();
    wake.background(true);
    advance(1000).await;
    assert!(!wake.0.state.lock().unwrap().pumping);
    drop(request);
    wake.background(false);
}
