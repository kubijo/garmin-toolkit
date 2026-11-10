//! Minimal identity observation, with an explicit outgoing-message boundary.

use std::num::NonZeroU64;
use std::time::Duration;

use thiserror::Error;

use crate::{
    Channel, DeviceInformation, FrameDecoder, FrameError, IdentityError, LinkMode, Message,
    MessageError, MlrError, MlrReceiver, Registration, RegistrationError, RegistrationState,
    RouteKind,
};

/// A bounded identity-observation failure.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// Invalid COBS framing.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// Invalid GFDI length or checksum.
    #[error(transparent)]
    Message(#[from] MessageError),
    /// Malformed device information.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// Failed service registration.
    #[error(transparent)]
    Registration(#[from] RegistrationError),
    /// Invalid reliable transport.
    #[error(transparent)]
    Mlr(#[from] MlrError),
    /// The complete observation deadline has expired.
    #[error("GFDI identity observation timed out")]
    Timeout,
    /// Input cannot revive a cancelled or failed observation.
    #[error("GFDI identity observation is stopped")]
    Stopped,
    /// A completed observation does not admit more input.
    #[error("GFDI identity observation is already complete")]
    Complete,
    /// The peer requested an authentication negotiation outside this probe.
    #[error("Garmin authentication is required; identity probe will not pair or authenticate")]
    AuthenticationRequired,
    /// No application data is sent by this probe, so nonzero peer ACKs are unexpected.
    #[error("peer acknowledged outbound MLR data that this probe never sent")]
    UnexpectedPeerAck,
    /// The peer closed the specific handle allocated to this attempt.
    #[error("peer closed the identity probe's GFDI channel")]
    PeerClosed,
    /// The value is empty or exceeds the negotiated ATT value-size limit.
    #[error("invalid GATT notification size")]
    NotificationSize,
}

/// Passive identity observation after an explicit channel registration.
///
/// Outgoing traffic is limited to the initial registration returned by `begin`,
/// receive-side MLR ACKs, and the separately requested close of our known handle.
/// No GFDI application reply is generated, including device/host information, time,
/// configuration, authentication, file acknowledgement, or synchronization.
#[derive(Debug)]
pub struct IdentityProbe {
    registration: Option<Registration>,
    channel: Option<Channel>,
    receiver: Option<MlrReceiver>,
    decoder: FrameDecoder,
    max_packet_size: usize,
    deadline: Duration,
    identity: Option<DeviceInformation>,
    stopped: bool,
    client_id: NonZeroU64,
}

impl IdentityProbe {
    /// Start an offline observation and return its optional management request.
    ///
    /// Direct routes require no channel registration.
    /// The caller must separately authorize any transport action.
    /// Deadline and timestamps share one monotonic origin;
    /// encoded GFDI frames are capped at 10,000 bytes.
    #[must_use]
    pub fn begin(
        route: RouteKind,
        client_id: NonZeroU64,
        mode: LinkMode,
        max_packet_size: usize,
        deadline: Duration,
    ) -> (Self, Option<[u8; 13]>) {
        let (registration, request) = if route == RouteKind::MultiLink {
            let (registration, request) = Registration::begin(client_id, mode, deadline);
            (Some(registration), Some(request))
        } else {
            (None, None)
        };
        (
            Self {
                registration,
                channel: None,
                receiver: None,
                decoder: FrameDecoder::new(10_000),
                max_packet_size,
                deadline,
                identity: None,
                stopped: false,
                client_id,
            },
            request,
        )
    }

    /// Inspect one ATT notification and return only required transport ACKs.
    ///
    /// # Errors
    /// Any malformed matching protocol input, authentication request, or deadline
    /// failure stops the observation. Cleanup of our known handle remains available.
    pub fn receive(&mut self, packet: &[u8], now: Duration) -> Result<Vec<[u8; 2]>, ProbeError> {
        self.check(now)?;
        let result = self.receive_inner(packet, now);
        if result.is_err() {
            self.cancel();
        }
        result
    }

    fn receive_inner(&mut self, packet: &[u8], now: Duration) -> Result<Vec<[u8; 2]>, ProbeError> {
        if packet.is_empty() || packet.len() > self.max_packet_size {
            return Err(ProbeError::NotificationSize);
        }
        if let Some(registration) = &mut self.registration {
            if registration.state() == RegistrationState::Pending {
                if let Some(channel) = registration.receive(packet, now)? {
                    if channel.mode == LinkMode::Reliable {
                        self.receiver =
                            Some(MlrReceiver::new(channel.handle, self.max_packet_size)?);
                    }
                    self.channel = Some(channel);
                }
                return Ok(Vec::new());
            }
            if packet[0] == 0 {
                if packet.len() >= 13
                    && packet[1] == 3
                    && packet[2..10] == self.client_id.get().to_le_bytes()
                    && packet[10..12] == [1, 0]
                    && self
                        .channel
                        .is_some_and(|channel| channel.handle.get() == packet[12])
                {
                    return Err(ProbeError::PeerClosed);
                }
                return Ok(Vec::new());
            }
        }
        let mut outgoing = Vec::new();
        let data = if let Some(channel) = self.channel {
            if let Some(receiver) = &mut self.receiver {
                if packet[0] & 0x80 == 0 || (packet[0] >> 4) & 7 != channel.handle.get() & 7 {
                    return Ok(outgoing);
                }
                let update = receiver.receive(packet, now)?;
                if update.peer_ack != 0 {
                    return Err(ProbeError::UnexpectedPeerAck);
                }
                outgoing.extend(update.ack);
                update.data.unwrap_or_default()
            } else {
                if packet[0] != channel.handle.get() {
                    return Ok(outgoing);
                }
                &packet[1..]
            }
        } else {
            packet
        };
        for &byte in data {
            if let Some(frame) = self.decoder.push(byte) {
                let frame = frame?;
                let message = Message::decode(&frame)?;
                match message.message_id() {
                    5024 => {
                        self.identity = Some(DeviceInformation::decode(message.payload())?);
                        // Finish exactly at identity;
                        // later coalesced messages are not interpreted.
                        break;
                    }
                    5101..=5112 => return Err(ProbeError::AuthenticationRequired),
                    _ => {}
                }
            }
        }
        if self.identity.is_some()
            && let Some(receiver) = &mut self.receiver
        {
            outgoing.extend(receiver.flush_ack()?);
        }
        Ok(outgoing)
    }

    /// Drive the ACK timer even when no notifications arrive.
    ///
    /// # Errors
    /// Returns timeout or a terminal lifecycle error;
    /// no application traffic is emitted.
    pub fn poll(&mut self, now: Duration) -> Result<Option<[u8; 2]>, ProbeError> {
        self.check(now)?;
        if let Some(receiver) = &mut self.receiver {
            return Ok(receiver.poll(now)?);
        }
        Ok(None)
    }

    /// Observed identity, available only after envelope and field validation.
    #[must_use]
    pub const fn identity(&self) -> Option<&DeviceInformation> {
        self.identity.as_ref()
    }

    /// Close request for only our known allocated `MultiLink` handle, including on failure.
    #[must_use]
    pub fn close_request(&self) -> Option<[u8; 13]> {
        self.registration
            .as_ref()
            .and_then(Registration::close_request)
    }

    /// Stop locally; the caller separately sends an owned close request and disconnects.
    pub fn cancel(&mut self) {
        self.stopped = true;
        self.decoder.reset();
        if let Some(registration) = &mut self.registration {
            registration.cancel();
        }
        if let Some(receiver) = &mut self.receiver {
            receiver.close();
        }
    }

    fn check(&mut self, now: Duration) -> Result<(), ProbeError> {
        if self.stopped {
            return Err(ProbeError::Stopped);
        }
        if self.identity.is_some() {
            return Err(ProbeError::Complete);
        }
        if now >= self.deadline {
            self.cancel();
            return Err(ProbeError::Timeout);
        }
        Ok(())
    }
}
