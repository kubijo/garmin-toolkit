//! Handling of xdg activation for user attention and input-authorized focus requests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Weak;
use std::time::{Duration, Instant};

use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_seat::WlSeat;
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{delegate_dispatch, Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::xdg::activation::v1::client::xdg_activation_token_v1::{
    Event as ActivationTokenEvent, XdgActivationTokenV1,
};
use sctk::reexports::protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

use sctk::globals::GlobalData;

use crate::event_loop::AsyncRequestSerial;
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::window::WindowRequests;
use crate::platform_impl::WindowId;
use crate::window::ActivationToken;

// Keep user input available through the next application redraw, not indefinitely.
const ACTIVATION_INPUT_LIFETIME: Duration = Duration::from_secs(5);

/// Real input authorizing an activation from one of this application's surfaces.
pub struct ActivationInput {
    pub serial: u32,
    pub seat: WlSeat,
    pub surface: WlSurface,
    source: Weak<WindowRequests>,
    received: Instant,
}

impl ActivationInput {
    pub fn new(
        serial: u32,
        seat: WlSeat,
        surface: WlSurface,
        source: Weak<WindowRequests>,
    ) -> Self {
        Self { serial, seat, surface, source, received: Instant::now() }
    }

    pub fn is_valid(&self) -> bool {
        activation_is_valid(&self.source, self.received)
            && self.seat.is_alive()
            && self.surface.is_alive()
    }
}

fn activation_is_valid(window: &Weak<WindowRequests>, received: Instant) -> bool {
    received.elapsed() < ACTIVATION_INPUT_LIFETIME
        && window.upgrade().is_some_and(|window| !window.closed.load(Ordering::Relaxed))
}

pub struct XdgActivationState {
    xdg_activation: XdgActivationV1,
}

impl XdgActivationState {
    pub fn bind(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        let xdg_activation = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { xdg_activation })
    }

    pub fn global(&self) -> &XdgActivationV1 {
        &self.xdg_activation
    }
}

impl Dispatch<XdgActivationV1, GlobalData, WinitState> for XdgActivationState {
    fn event(
        _state: &mut WinitState,
        _proxy: &XdgActivationV1,
        _event: <XdgActivationV1 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<XdgActivationTokenV1, XdgActivationTokenData, WinitState> for XdgActivationState {
    fn event(
        state: &mut WinitState,
        proxy: &XdgActivationTokenV1,
        event: <XdgActivationTokenV1 as Proxy>::Event,
        data: &XdgActivationTokenData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        let token = match event {
            ActivationTokenEvent::Done { token } => token,
            _ => return,
        };

        let global = state
            .xdg_activation
            .as_ref()
            .expect("got xdg_activation event without global.")
            .global();

        match data {
            XdgActivationTokenData::Focus((surface, target, requested)) => {
                // A token may arrive after the target was closed. Never activate its old surface.
                if activation_is_valid(target, *requested) && surface.is_alive() {
                    global.activate(token, surface);
                }
            },
            XdgActivationTokenData::Attention((surface, fence)) => {
                global.activate(token, surface);
                // Mark that no request attention is in process.
                if let Some(attention_requested) = fence.upgrade() {
                    attention_requested.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            },
            XdgActivationTokenData::Obtain((window_id, serial)) => {
                state.events_sink.push_window_event(
                    crate::event::WindowEvent::ActivationTokenDone {
                        serial: *serial,
                        token: ActivationToken::from_raw(token),
                    },
                    *window_id,
                );
            },
        }

        proxy.destroy();
    }
}

/// The data associated with the activation request.
pub enum XdgActivationTokenData {
    /// Activate an existing window using input from the initiating window.
    Focus((WlSurface, Weak<WindowRequests>, Instant)),
    /// Request user attention for the given surface.
    Attention((WlSurface, Weak<AtomicBool>)),
    /// Get a token to be passed outside of the winit.
    Obtain((WindowId, AsyncRequestSerial)),
}

delegate_dispatch!(WinitState: [ XdgActivationV1: GlobalData] => XdgActivationState);
delegate_dispatch!(WinitState: [ XdgActivationTokenV1: XdgActivationTokenData] => XdgActivationState);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn activation_expires_and_does_not_outlive_its_window() {
        let window = Arc::new(WindowRequests {
            closed: AtomicBool::new(false),
            redraw_requested: AtomicBool::new(false),
        });
        let target = Arc::downgrade(&window);
        let now = Instant::now();
        assert!(activation_is_valid(&target, now));
        assert!(!activation_is_valid(&target, now - ACTIVATION_INPUT_LIFETIME));
        window.closed.store(true, Ordering::Relaxed);
        assert!(!activation_is_valid(&target, now));
        drop(window);
        assert!(!activation_is_valid(&target, now));
    }
}
