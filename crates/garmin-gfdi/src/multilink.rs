//! Bounded GFDI service registration based on Gadgetbridge `CommunicatorV2`.

use std::num::{NonZeroU8, NonZeroU64};
use std::time::Duration;

use thiserror::Error;

/// Requested or accepted `MultiLink` transport mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMode {
    /// Plain `MultiLink` handle prefix; no reliable-transfer framing.
    Plain,
    /// `MultiLink` reliable transport; data transfer needs a separate MLR state machine.
    Reliable,
}

/// A channel accepted by the peer for this registration attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    /// Nonzero peer-assigned handle, preserved without interpreting MLR bit fields.
    pub handle: NonZeroU8,
    /// Accepted mode, always equal to the explicitly requested mode.
    pub mode: LinkMode,
    /// Optional trailing protocol byte; retained without guessing its semantics.
    pub extension: Option<u8>,
}

/// Registration lifecycle; no state transition emits additional requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    /// Awaiting a correlated reply before the fixed deadline.
    Pending,
    /// Registration completed; the caller now owns the returned channel.
    Registered(Channel),
    /// Deadline expired or a matching reply rejected the attempt.
    Failed,
    /// The caller cancelled the pending attempt.
    Cancelled,
}

/// A registration failure or malformed candidate reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RegistrationError {
    /// A malformed reply leaves the attempt pending until its original deadline.
    #[error("malformed MultiLink registration reply")]
    Malformed,
    /// Preserve the raw status, including values unknown to this implementation.
    #[error("MultiLink registration denied with status {status}")]
    Denied {
        /// Peer-supplied status code.
        status: u8,
    },
    /// Do not silently upgrade or downgrade transport framing.
    #[error("MultiLink reply changed the requested transport mode")]
    ModeMismatch {
        /// Explicitly requested mode.
        requested: LinkMode,
        /// Peer-allocated channel, retained for caller-owned cleanup decisions.
        channel: Channel,
    },
    /// The supplied monotonic time reached the fixed deadline.
    #[error("MultiLink registration timed out")]
    Timeout,
    /// Replies cannot revive completed, failed, or cancelled attempts.
    #[error("MultiLink registration is no longer pending")]
    NotPending,
}

/// One explicitly started service registration; `begin` selects GFDI service 1.
///
/// The caller owns the clock and transport. No close-all, authentication, retry,
/// re-registration, time sync, or file-state mutation is generated automatically.
#[derive(Debug)]
pub struct Registration {
    client_id: NonZeroU64,
    service: u16,
    requested_mode: LinkMode,
    deadline: Duration,
    state: RegistrationState,
    allocated_channel: Option<Channel>,
}

impl Registration {
    /// Begin an attempt and return its single 13-byte management request.
    ///
    /// The caller must authorize registration before sending, choose
    /// a client ID distinct from other companions, and use the same
    /// monotonic origin for the deadline and subsequent timestamps.
    ///
    /// Use a fresh client ID for a new attempt after
    /// timeout/cancellation so stale replies cannot match it.
    ///
    /// The ID is only correlation, not authentication.
    /// Reserved companion identities are not allocated by this wire primitive.
    #[must_use]
    pub fn begin(client_id: NonZeroU64, mode: LinkMode, deadline: Duration) -> (Self, [u8; 13]) {
        Self::begin_service(client_id, 1, mode, deadline)
    }

    /// Begin an explicitly selected service registration, including transfer pipes.
    /// The caller owns service selection and authorization; no fallback is performed.
    #[must_use]
    pub fn begin_service(
        client_id: NonZeroU64,
        service: u16,
        mode: LinkMode,
        deadline: Duration,
    ) -> (Self, [u8; 13]) {
        let mut request = [0; 13];
        request[2..10].copy_from_slice(&client_id.get().to_le_bytes());
        request[10..12].copy_from_slice(&service.to_le_bytes());
        request[12] = if mode == LinkMode::Reliable { 2 } else { 0 };
        (
            Self {
                client_id,
                service,
                requested_mode: mode,
                deadline,
                state: RegistrationState::Pending,
                allocated_channel: None,
            },
            request,
        )
    }

    /// Current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> RegistrationState {
        self.state
    }

    /// Check the deadline even when no notifications arrive.
    ///
    /// # Errors
    /// Returns timeout at or after the deadline, or not-pending after completion.
    pub fn poll(&mut self, now: Duration) -> Result<(), RegistrationError> {
        if self.state != RegistrationState::Pending {
            return Err(RegistrationError::NotPending);
        }
        if now >= self.deadline {
            self.state = RegistrationState::Failed;
            return Err(RegistrationError::Timeout);
        }
        Ok(())
    }

    /// Inspect a notification for a matching registration reply.
    ///
    /// Unrelated message types, clients, and services return `Ok(None)`.
    /// Malformed replies do not extend the deadline.
    /// A matching denial or mode mismatch is terminal;
    /// no automatic fallback or channel cleanup is attempted.
    ///
    /// # Errors
    /// Returns malformed, denied, mode mismatch, timeout, or not-pending.
    pub fn receive(
        &mut self,
        packet: &[u8],
        now: Duration,
    ) -> Result<Option<Channel>, RegistrationError> {
        self.poll(now)?;
        if packet.len() < 2 {
            return Err(RegistrationError::Malformed);
        }
        if packet[..2] != [0, 1] {
            return Ok(None);
        }
        if packet.len() < 12 {
            return Err(RegistrationError::Malformed);
        }
        if packet[2..10] != self.client_id.get().to_le_bytes()
            || packet[10..12] != self.service.to_le_bytes()
        {
            return Ok(None);
        }
        let status = *packet.get(12).ok_or(RegistrationError::Malformed)?;
        if status != 0 {
            self.state = RegistrationState::Failed;
            return Err(RegistrationError::Denied { status });
        }
        if !(15..=16).contains(&packet.len()) {
            return Err(RegistrationError::Malformed);
        }
        let handle = NonZeroU8::new(packet[13]).ok_or(RegistrationError::Malformed)?;
        let mode = match packet[14] {
            0 => LinkMode::Plain,
            1 => LinkMode::Reliable,
            _ => return Err(RegistrationError::Malformed),
        };
        let channel = Channel {
            handle,
            mode,
            extension: packet.get(15).copied(),
        };
        self.allocated_channel = Some(channel);
        if mode != self.requested_mode {
            self.state = RegistrationState::Failed;
            return Err(RegistrationError::ModeMismatch {
                requested: self.requested_mode,
                channel,
            });
        }
        self.state = RegistrationState::Registered(channel);
        Ok(Some(channel))
    }

    /// Cancel a pending attempt locally; emits no management packet.
    /// Completed attempts retain their channel state for the caller to manage.
    pub fn cancel(&mut self) {
        if self.state == RegistrationState::Pending {
            self.state = RegistrationState::Cancelled;
        }
    }

    /// Build a close request for only this attempt's known allocated handle.
    ///
    /// It is available after a mode mismatch as well as success. It does not
    /// claim the peer processed it, close unknown handles, or emit close-all.
    #[must_use]
    pub fn close_request(&self) -> Option<[u8; 13]> {
        self.allocated_channel.map(|channel| {
            let mut request = [0; 13];
            request[1] = 2;
            request[2..10].copy_from_slice(&self.client_id.get().to_le_bytes());
            request[10..12].copy_from_slice(&self.service.to_le_bytes());
            request[12] = channel.handle.get();
            request
        })
    }
}
