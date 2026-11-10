//! Bounded outbound MLR window; no transport or application policy.

use std::collections::VecDeque;
use std::num::NonZeroU8;
use std::time::Duration;

use thiserror::Error;

const WINDOW: usize = 32;
const INITIAL_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_TIMEOUT: Duration = Duration::from_secs(20);

/// Invalid sender configuration, input, or lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MlrSendError {
    /// An ATT value must hold two header bytes and at least one data byte.
    #[error("MLR send packet size must be between 3 and 512 bytes")]
    PacketSize,
    /// Pending payload would exceed the caller's explicit bound.
    #[error("MLR send buffer limit exceeded")]
    BufferLimit,
    /// An ACK is outside the six-bit wire range or acknowledges unsent data.
    #[error("MLR acknowledgement is outside the sent window")]
    InvalidAck,
    /// The explicit retry budget was exhausted without progress.
    #[error("MLR retransmission budget exhausted")]
    RetriesExhausted,
    /// Cancellation or retry exhaustion permanently closes this instance.
    #[error("MLR sender is closed")]
    Closed,
}

/// Outbound six-bit MLR window, initially allowing 32 unacknowledged fragments.
///
/// The caller enqueues already-framed bytes, sends every packet returned by `poll`
/// in order, and feeds validated peer ACKs from `MlrReceiver` to `acknowledge`.
/// On transport failure it must cancel this instance rather than silently drop a
/// returned packet. Polling is not evidence that any packet reached the peer.
/// Timestamps share one monotonic origin. The caller also owns a total deadline;
/// retry limits alone do not bound sessions in which the peer keeps making progress.
#[derive(Debug)]
pub struct MlrSender {
    handle: u8,
    packet_size: usize,
    buffer_limit: usize,
    buffered: usize,
    queued: VecDeque<Vec<u8>>,
    sent: VecDeque<Vec<u8>>,
    next_sequence: u8,
    last_ack: u8,
    window: usize,
    pending_since: Option<Duration>,
    timeout: Duration,
    retries: u8,
    retry_limit: u8,
    closed: bool,
}

impl MlrSender {
    /// Create a sender with explicit payload-memory and consecutive-retry bounds.
    /// A zero retry limit allows the initial send only. Packet size is the complete
    /// ATT value limit; buffer accounting includes queued and unacknowledged payload.
    ///
    /// # Errors
    /// Rejects packet sizes outside 3–512 bytes. A zero buffer limit admits no data.
    pub fn new(
        handle: NonZeroU8,
        packet_size: usize,
        buffer_limit: usize,
        retry_limit: u8,
    ) -> Result<Self, MlrSendError> {
        if !(3..=512).contains(&packet_size) {
            return Err(MlrSendError::PacketSize);
        }
        Ok(Self {
            handle: handle.get() & 7,
            packet_size,
            buffer_limit,
            buffered: 0,
            queued: VecDeque::new(),
            sent: VecDeque::new(),
            next_sequence: 0,
            last_ack: 0,
            window: WINDOW,
            pending_since: None,
            timeout: INITIAL_TIMEOUT,
            retries: 0,
            retry_limit,
            closed: false,
        })
    }

    /// Append bytes atomically; rejection leaves all pending data unchanged.
    /// Empty input is a no-op and never becomes an ACK-only data fragment.
    ///
    /// # Errors
    /// Rejects closed senders and payload beyond the configured buffer bound.
    pub fn enqueue(&mut self, bytes: &[u8]) -> Result<(), MlrSendError> {
        self.ensure_open()?;
        if bytes.len() > self.buffer_limit.saturating_sub(self.buffered) {
            return Err(MlrSendError::BufferLimit);
        }
        self.queued
            .extend(bytes.chunks(self.packet_size - 2).map(<[u8]>::to_vec));
        self.buffered += bytes.len();
        Ok(())
    }

    /// Apply a six-bit cumulative ACK and return newly acknowledged payload bytes.
    /// Duplicate and older ACKs within the sequence half-range do not reset timers
    /// or restore the window. An advancing ACK resets backoff and the retry budget.
    ///
    /// # Errors
    /// Rejects closed senders, out-of-range ACKs, and ACKs for unsent fragments.
    /// Invalid input leaves the send window intact.
    pub fn acknowledge(&mut self, next: u8, now: Duration) -> Result<usize, MlrSendError> {
        self.ensure_open()?;
        if next > 63 {
            return Err(MlrSendError::InvalidAck);
        }
        let count = usize::from(next.wrapping_sub(self.last_ack) & 63);
        if count == 0 || count > WINDOW {
            return Ok(0);
        }
        if count > self.sent.len() {
            return Err(MlrSendError::InvalidAck);
        }
        let mut bytes = 0;
        for fragment in self.sent.drain(..count) {
            bytes += fragment.len() - 2;
        }
        self.buffered -= bytes;
        self.last_ack = next;
        self.window = (self.window + count).min(WINDOW);
        self.timeout = INITIAL_TIMEOUT;
        self.retries = 0;
        self.pending_since = (!self.sent.is_empty()).then_some(now);
        Ok(bytes)
    }

    /// Produce a bounded batch of new packets or exact retransmissions.
    /// `peer_next` is the receiver's current expected sequence, piggybacked on new
    /// data. Retransmissions retain their original headers. A timeout halves the
    /// new-send window and doubles the timer up to twenty seconds.
    ///
    /// # Errors
    /// Rejects closed senders and invalid piggyback ACKs. Exhausting the retry
    /// budget cancels the sender and discards retained data.
    pub fn poll(&mut self, now: Duration, peer_next: u8) -> Result<Vec<Vec<u8>>, MlrSendError> {
        self.ensure_open()?;
        if peer_next > 63 {
            return Err(MlrSendError::InvalidAck);
        }
        if self
            .pending_since
            .is_some_and(|start| now.saturating_sub(start) >= self.timeout)
        {
            if self.retries >= self.retry_limit {
                self.cancel();
                return Err(MlrSendError::RetriesExhausted);
            }
            self.retries += 1;
            self.window = (self.window / 2).max(1);
            self.timeout = (self.timeout * 2).min(MAX_TIMEOUT);
            self.pending_since = Some(now);
            return Ok(self.sent.iter().cloned().collect());
        }
        let mut packets = Vec::new();
        while self.sent.len() < self.window {
            let Some(data) = self.queued.pop_front() else {
                break;
            };
            let mut packet = vec![
                0x80 | self.handle << 4 | peer_next >> 2,
                (peer_next & 3) << 6 | self.next_sequence,
            ];
            packet.extend(data);
            packets.push(packet.clone());
            self.sent.push_back(packet);
            self.next_sequence = (self.next_sequence + 1) & 63;
            self.pending_since.get_or_insert(now);
        }
        Ok(packets)
    }

    /// Payload bytes still queued or awaiting a transport ACK.
    #[must_use]
    pub fn buffered_bytes(&self) -> usize {
        self.buffered
    }

    /// Next sequence expected by the peer according to its last advancing ACK.
    #[must_use]
    pub fn acknowledged_sequence(&self) -> u8 {
        self.last_ack
    }

    /// Drop retained bytes and stop permanently; no peer management packet is sent.
    pub fn cancel(&mut self) {
        self.closed = true;
        self.queued.clear();
        self.sent.clear();
        self.buffered = 0;
        self.pending_since = None;
    }

    fn ensure_open(&self) -> Result<(), MlrSendError> {
        if self.closed {
            Err(MlrSendError::Closed)
        } else {
            Ok(())
        }
    }
}
