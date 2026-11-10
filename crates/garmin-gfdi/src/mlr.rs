//! Receive-side MLR sequencing and transport acknowledgements.

use std::num::NonZeroU8;
use std::time::Duration;

use thiserror::Error;

const ACK_DELAY: Duration = Duration::from_millis(20);
const ACK_THRESHOLD: u8 = 5;

/// Invalid reliable-transport input or lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MlrError {
    /// A packet cannot contain its header, or the configured limit is too small.
    #[error("MLR packet is shorter than its two-byte header")]
    TooShort,
    /// The packet exceeds the caller's negotiated value-size limit.
    #[error("MLR packet exceeds the configured notification size")]
    TooLong,
    /// A packet lacks the reliable-mode bit.
    #[error("packet is not MLR framed")]
    NotReliable,
    /// The packet belongs to another channel.
    #[error("MLR packet belongs to a different channel")]
    WrongHandle,
    /// No input or timer can revive a closed receiver.
    #[error("MLR receiver is closed")]
    Closed,
}

/// One reliable-transport update, with no internal unbounded queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MlrUpdate<'a> {
    /// Peer acknowledgement for the caller's outbound-data state machine.
    pub peer_ack: u8,
    /// In-order payload only; duplicates and reordered packets are not delivered.
    pub data: Option<&'a [u8]>,
    /// Immediate transport ACK, if the threshold or recovery rule requires one.
    pub ack: Option<[u8; 2]>,
}

/// Receive-side MLR window, independent of timers, Bluetooth, and application ACKs.
///
/// In-order data advances a six-bit sequence. Gaps and duplicates trigger an ACK
/// of the next expected sequence so the peer can retransmit. At most five packets
/// or twenty milliseconds elapse before an ACK is due. The caller must poll the
/// timer and separately validate peer ACKs against its outbound state. This type
/// does not implement an outbound retransmission window or authenticate packets.
#[derive(Debug)]
pub struct MlrReceiver {
    handle: u8,
    max_packet_size: usize,
    next_sequence: u8,
    last_ack: u8,
    pending_since: Option<Duration>,
    closed: bool,
}

impl MlrReceiver {
    /// Construct a receiver for a peer-assigned reliable channel.
    ///
    /// # Errors
    /// Rejects value-size limits smaller than the two-byte header.
    pub fn new(handle: NonZeroU8, max_packet_size: usize) -> Result<Self, MlrError> {
        if max_packet_size < 2 {
            return Err(MlrError::TooShort);
        }
        Ok(Self {
            handle: handle.get() & 7,
            max_packet_size,
            next_sequence: 0,
            last_ack: 0,
            pending_since: None,
            closed: false,
        })
    }

    /// Inspect one complete ATT value with a caller-supplied monotonic timestamp.
    ///
    /// # Errors
    /// Rejects closed receivers, malformed headers, wrong modes/handles, and oversized values.
    pub fn receive<'a>(
        &mut self,
        packet: &'a [u8],
        now: Duration,
    ) -> Result<MlrUpdate<'a>, MlrError> {
        self.ensure_open()?;
        if packet.len() < 2 {
            return Err(MlrError::TooShort);
        }
        if packet.len() > self.max_packet_size {
            return Err(MlrError::TooLong);
        }
        if packet[0] & 0x80 == 0 {
            return Err(MlrError::NotReliable);
        }
        if (packet[0] >> 4) & 7 != self.handle {
            return Err(MlrError::WrongHandle);
        }
        let peer_ack = ((packet[0] & 15) << 2) | (packet[1] >> 6);
        let mut update = MlrUpdate {
            peer_ack,
            data: None,
            ack: None,
        };
        if packet.len() == 2 {
            return Ok(update);
        }
        if packet[1] & 63 != self.next_sequence {
            update.ack = Some(self.make_ack());
            return Ok(update);
        }
        self.next_sequence = (self.next_sequence + 1) & 63;
        update.data = Some(&packet[2..]);
        self.pending_since.get_or_insert(now);
        if self.next_sequence.wrapping_sub(self.last_ack) & 63 >= ACK_THRESHOLD {
            update.ack = Some(self.make_ack());
        }
        Ok(update)
    }

    /// Return an ACK once the oldest unacknowledged data reaches twenty milliseconds.
    ///
    /// # Errors
    /// Returns [`MlrError::Closed`] after closure.
    pub fn poll(&mut self, now: Duration) -> Result<Option<[u8; 2]>, MlrError> {
        self.ensure_open()?;
        if self
            .pending_since
            .is_some_and(|start| now.saturating_sub(start) >= ACK_DELAY)
        {
            return Ok(Some(self.make_ack()));
        }
        Ok(None)
    }

    /// Flush a pending transport ACK before finishing observation.
    ///
    /// # Errors
    /// Returns [`MlrError::Closed`] after closure.
    pub fn flush_ack(&mut self) -> Result<Option<[u8; 2]>, MlrError> {
        self.ensure_open()?;
        Ok(self.pending_since.map(|_| self.make_ack()))
    }

    /// Next expected data sequence, for piggybacking on outbound MLR packets.
    #[must_use]
    pub fn expected_sequence(&self) -> u8 {
        self.next_sequence
    }

    /// Stop locally and discard pending ACK timers; does not close peer handles.
    pub fn close(&mut self) {
        self.closed = true;
        self.pending_since = None;
    }

    fn ensure_open(&self) -> Result<(), MlrError> {
        if self.closed {
            Err(MlrError::Closed)
        } else {
            Ok(())
        }
    }

    fn make_ack(&mut self) -> [u8; 2] {
        let ack = [
            0x80 | (self.handle << 4) | (self.next_sequence >> 2),
            (self.next_sequence & 3) << 6,
        ];
        self.last_ack = self.next_sequence;
        self.pending_since = None;
        ack
    }
}
