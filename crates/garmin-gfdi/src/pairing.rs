//! First-companion setup adapted from Gadgetbridge's Garmin initialization.

use std::num::NonZeroU64;
use std::time::Duration;

use crate::download::{DataChannel, DownloadError};
use crate::{DeviceInformation, FrameDecoder, LinkMode, Message, encode_frame, encode_message};

// Gadgetbridge SystemEventMessage ordinals: PAIR_COMPLETE, SYNC_COMPLETE, SETUP_WIZARD_COMPLETE.
const EVENTS: [u8; 3] = [4, 0, 14];

/// Bounded first-pairing handshake. The caller owns the OS bond and every write.
///
/// Exchanges identity and capabilities, acknowledges auth negotiation as upstream does,
/// and sends only the three setup-completion events. Does not sync settings, time or files.
/// Success requires matching sequenced GFDI acknowledgements, not just transport delivery.
pub struct PairingSession {
    channel: DataChannel,
    decoder: FrameDecoder,
    identity: Option<DeviceInformation>,
    deadline: Duration,
    configured: bool,
    pending: Option<(u16, u8)>,
    acknowledged: u8,
    stopped: bool,
}

impl PairingSession {
    /// Register one GFDI channel under an explicit deadline.
    #[must_use]
    pub fn begin(client: NonZeroU64, mode: LinkMode, deadline: Duration) -> (Self, [u8; 13]) {
        let (channel, request) = DataChannel::begin(client, 1, mode, deadline);
        (
            Self {
                channel,
                decoder: FrameDecoder::new(10_000),
                identity: None,
                deadline,
                configured: false,
                pending: None,
                acknowledged: 0,
                stopped: false,
            },
            request,
        )
    }

    /// Consume one notification; failure permanently stops this handshake.
    ///
    /// # Errors
    /// Rejects malformed data, failed setup ACKs, unsupported encryption and expired sessions.
    pub fn receive(&mut self, packet: &[u8], now: Duration) -> Result<Vec<Vec<u8>>, DownloadError> {
        let result = self.receive_inner(packet, now);
        if result.is_err() {
            self.stopped = true;
        }
        result
    }

    fn receive_inner(
        &mut self,
        packet: &[u8],
        now: Duration,
    ) -> Result<Vec<Vec<u8>>, DownloadError> {
        self.check(now)?;
        if packet.is_empty() || packet.len() > 512 {
            return Err(DownloadError::Protocol("invalid pairing notification size"));
        }
        let mut outgoing = Vec::new();
        let input = self.channel.receive(packet, now, &mut outgoing)?;
        if input.closed {
            return Err(DownloadError::Protocol("pairing channel closed"));
        }
        if let Some(bytes) = input.data {
            for byte in bytes {
                if let Some(frame) = self.decoder.push(byte) {
                    self.message(Message::decode(&frame?)?, &mut outgoing)?;
                }
            }
        }
        self.channel.poll(now, &mut outgoing)?;
        Ok(outgoing)
    }

    fn message(
        &mut self,
        message: Message<'_>,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        match message.message_id() {
            5024 if self.identity.is_none() => {
                let identity = DeviceInformation::decode(message.payload())?;
                if identity.max_packet_size < 64 {
                    return Err(DownloadError::Protocol("GFDI packet size too small"));
                }
                self.channel.write(
                    &crate::identity::host_information(message, &identity)?,
                    outgoing,
                )?;
                self.identity = Some(identity);
            }
            5050 if self.identity.is_some() && !self.configured => {
                let payload = message.payload();
                if payload
                    .first()
                    .is_none_or(|length| usize::from(*length) + 1 != payload.len())
                {
                    return Err(DownloadError::Protocol("invalid capability length"));
                }
                self.reply(message, &[0], outgoing)?;
                // Advertise FIT link and MultiLink only; no unsupported phone/cloud features.
                let mut capabilities = [0_u8; 11];
                capabilities[0] = 10;
                capabilities[1] = 1;
                capabilities[10] = 1 << 4; // MULTI_LINK_SERVICE, ordinal 76.
                self.command(5050, 0, &capabilities, outgoing)?;
                self.configured = true;
            }
            5000 => self.status(message, outgoing)?,
            5101 => {
                if message.payload().len() != 5 {
                    return Err(DownloadError::Protocol("invalid auth negotiation length"));
                }
                // Upstream echoes the opaque flags; it does not implement a cryptographic exchange.
                let mut response = vec![0, 0];
                response.extend(message.payload());
                self.reply(message, &response, outgoing)?;
            }
            5102..=5112 => {
                return Err(DownloadError::Protocol(
                    "device requires unsupported Garmin encryption",
                ));
            }
            5052 => self.reply(message, &[2], outgoing)?, // UNSUPPORTED: do not change time.
            _ => {}
        }
        Ok(())
    }

    fn status(
        &mut self,
        message: Message<'_>,
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        let Some((kind, sequence)) = self.pending else {
            return Ok(());
        };
        let payload = message.payload();
        if payload.len() < 3 {
            return Err(DownloadError::Protocol("truncated pairing status"));
        }
        if payload[..2] != kind.to_le_bytes() || message.sequence() != Some(sequence) {
            return Ok(());
        }
        if payload != [kind.to_le_bytes()[0], kind.to_le_bytes()[1], 0] {
            return Err(DownloadError::Protocol("device rejected pairing command"));
        }
        self.pending = None;
        if kind == 5030 {
            self.acknowledged += 1;
        }
        if let Some(event) = EVENTS.get(usize::from(self.acknowledged)) {
            self.command(5030, self.acknowledged + 1, &[*event, 0], outgoing)?;
        }
        Ok(())
    }

    fn reply(
        &mut self,
        message: Message<'_>,
        status: &[u8],
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        let mut payload = message.message_id().to_le_bytes().to_vec();
        payload.extend(status);
        let kind = message
            .sequence()
            .map_or(5000, |sequence| 0x8000 | u16::from(sequence) << 8);
        self.channel
            .write(&encode_frame(&encode_message(kind, &payload)?), outgoing)
    }

    fn command(
        &mut self,
        kind: u16,
        sequence: u8,
        payload: &[u8],
        outgoing: &mut Vec<Vec<u8>>,
    ) -> Result<(), DownloadError> {
        self.channel.write(
            &encode_frame(&encode_message(
                0x8000 | u16::from(sequence) << 8 | (kind - 5000),
                payload,
            )?),
            outgoing,
        )?;
        self.pending = Some((kind, sequence));
        Ok(())
    }

    /// Pump reliable transport ACKs and retries.
    ///
    /// # Errors
    /// Stops on an expired deadline or transport failure.
    pub fn poll(&mut self, now: Duration) -> Result<Vec<Vec<u8>>, DownloadError> {
        let result = (|| {
            self.check(now)?;
            let mut outgoing = Vec::new();
            self.channel.poll(now, &mut outgoing)?;
            Ok(outgoing)
        })();
        if result.is_err() {
            self.stopped = true;
        }
        result
    }

    /// Validated identity only after all setup events are acknowledged and sends drained.
    #[must_use]
    pub fn completed(&self) -> Option<&DeviceInformation> {
        (!self.stopped && self.acknowledged == 3 && self.channel.drained())
            .then_some(self.identity.as_ref())
            .flatten()
    }

    /// Close only this instance's allocated GFDI handle, including after failure.
    #[must_use]
    pub fn close_request(&self) -> Option<[u8; 13]> {
        self.channel.close_request()
    }

    /// Current setup phase for caller-owned progress reporting.
    #[must_use]
    pub fn progress(&self) -> &'static str {
        if self.stopped {
            "stopped"
        } else if self.identity.is_none() {
            "waiting for Garmin identity"
        } else if !self.configured {
            "waiting for Garmin capabilities"
        } else if self.pending == Some((5050, 0)) {
            "waiting for capability acknowledgement"
        } else if self.acknowledged < 3 {
            "waiting for setup acknowledgement"
        } else {
            "Garmin setup acknowledged"
        }
    }

    fn check(&self, now: Duration) -> Result<(), DownloadError> {
        if self.stopped || now >= self.deadline {
            return Err(DownloadError::Protocol(
                "pairing stopped or deadline expired",
            ));
        }
        Ok(())
    }
}
