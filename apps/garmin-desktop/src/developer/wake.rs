//! Pace explicit Wayland redraws while control requests or background runs need them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::{runtime::Handle, sync::Notify, time::Instant};

// Round upward so this never exceeds thirty wake-ups per second.
const FRAME_INTERVAL: Duration = Duration::from_nanos(33_333_334);
const BACKGROUND_LEASE: Duration = Duration::from_secs(5);

trait Redraw: Send + Sync {
    /// Grant one redraw without waiting for a compositor callback; false if unavailable.
    fn request(&self) -> bool;
}

#[cfg(target_os = "linux")]
struct WaylandWindow(std::sync::Weak<winit::window::Window>);

#[cfg(target_os = "linux")]
impl Redraw for WaylandWindow {
    fn request(&self) -> bool {
        use winit::platform::wayland::WindowExtWayland as _;
        self.0
            .upgrade()
            .is_some_and(|window| window.request_redraw_without_frame_callback())
    }
}

#[derive(Clone)]
pub(super) struct Wake(Arc<Inner>);

struct Inner {
    state: Mutex<State>,
    changed: Notify,
    runtime: Handle,
    redraw: Option<Arc<dyn Redraw>>,
}

#[derive(Default)]
struct State {
    requests: usize,
    background_until: Option<Instant>,
    pumping: bool,
    next_frame: Option<Instant>,
}

impl State {
    fn active(&self) -> bool {
        self.requests != 0
            || self
                .background_until
                .is_some_and(|until| Instant::now() < until)
    }
}

impl Wake {
    pub(super) fn new(runtime: Handle, window: Option<&Arc<winit::window::Window>>) -> Self {
        #[cfg(target_os = "linux")]
        let redraw = {
            use winit::platform::wayland::WindowExtWayland as _;
            window
                .filter(|window| window.xdg_toplevel().is_some())
                .map(|window| Arc::new(WaylandWindow(Arc::downgrade(window))) as Arc<dyn Redraw>)
        };
        #[cfg(not(target_os = "linux"))]
        let redraw = {
            let _ = window;
            None
        };
        Self::with_redraw(runtime, redraw)
    }

    fn with_redraw(runtime: Handle, redraw: Option<Arc<dyn Redraw>>) -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            runtime,
            redraw,
        }))
    }

    fn change(&self, change: impl FnOnce(&mut State)) {
        let start = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = (state.requests, state.background_until);
            change(&mut state);
            if previous == (state.requests, state.background_until) {
                return;
            }
            let start = state.active() && !state.pumping && self.0.redraw.is_some();
            state.pumping |= start;
            start
        };
        self.0.changed.notify_one();
        if start {
            let wake = self.clone();
            self.0.runtime.spawn(async move { wake.pump().await });
        }
    }

    pub(super) fn request(&self) -> Request {
        self.change(|state| state.requests += 1);
        Request { wake: self.clone() }
    }

    pub(super) fn background(&self, active: bool) {
        self.change(|state| {
            state.background_until = active.then(|| Instant::now() + BACKGROUND_LEASE);
        });
    }

    async fn pump(&self) {
        loop {
            let deadline = {
                let mut state = self
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.active() {
                    state.pumping = false;
                    return;
                }
                let next_frame = state.next_frame.unwrap_or_else(Instant::now);
                if state.requests == 0 {
                    state
                        .background_until
                        .map_or(next_frame, |until| next_frame.min(until))
                } else {
                    next_frame
                }
            };
            tokio::select! {
                () = self.0.changed.notified() => continue,
                () = tokio::time::sleep_until(deadline) => {}
            }
            {
                let mut state = self
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.active() {
                    state.pumping = false;
                    return;
                }
                // A renewal may race the old lease's expiry timer. It cannot grant an early frame.
                if state.next_frame.is_some_and(|next| Instant::now() < next) {
                    continue;
                }
                // Retain this deadline across bursts; new requests cannot bypass the rate cap.
                state.next_frame = Some(Instant::now() + FRAME_INTERVAL);
            }
            // Native calls can re-enter app code; release the state lock first.
            if !self
                .0
                .redraw
                .as_ref()
                .is_some_and(|redraw| redraw.request())
            {
                self.0
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pumping = false;
                return;
            }
        }
    }
}

pub(super) struct Request {
    wake: Wake,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.wake.change(|state| state.requests -= 1);
    }
}

#[cfg(test)]
mod tests;
