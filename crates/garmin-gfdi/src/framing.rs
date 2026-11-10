//! Garmin's leading/trailing-zero COBS framing; see ../README.md for provenance.

use thiserror::Error;

/// A frame rejected without preventing subsequent frames from being decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FrameError {
    /// The encoded body exceeded the caller's configured limit.
    #[error("COBS frame exceeds the encoded-body limit of {limit} bytes")]
    TooLong {
        /// Maximum encoded bytes, excluding delimiters.
        limit: usize,
    },
    /// A COBS code claims bytes that are absent from the frame.
    #[error("malformed COBS frame")]
    Malformed,
}

/// Encode one Garmin frame, including its leading and trailing zero delimiters.
///
/// Garmin's encoder emits a final empty block after a full 254-byte nonzero run.
/// Retain that convention even though a standard COBS decoder accepts its omission.
#[must_use]
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0];
    frame.extend(cobs::encode_vec(payload));
    let final_run = payload.iter().rev().take_while(|&&byte| byte != 0).count();
    if payload.is_empty() || (final_run != 0 && final_run.is_multiple_of(254)) {
        frame.push(1);
    }
    frame.push(0);
    frame
}

/// Incremental framing across arbitrary transport chunks.
/// Only one encoded body is buffered.
/// Completed frames are returned immediately, with no internal output queue.
/// Noise is ignored until a zero delimiter; empty delimiters are ignored.
/// After overflow, bytes are discarded until the next zero.
/// A delimiter also synchronizes the next frame, including shared delimiters.
#[derive(Debug)]
pub struct FrameDecoder {
    encoded: Vec<u8>,
    limit: usize,
    synchronized: bool,
}

impl FrameDecoder {
    /// Set the encoded-body limit, excluding the two delimiters.
    ///
    /// A zero limit rejects every nonempty encoded body.
    /// No input-sized allocation occurs before a byte is admitted under the limit.
    #[must_use]
    pub const fn new(max_encoded_bytes: usize) -> Self {
        Self {
            encoded: Vec::new(),
            limit: max_encoded_bytes,
            synchronized: false,
        }
    }

    /// Consume one byte, returning at most one decoded frame or rejection.
    ///
    /// Iterate over each received chunk and handle every returned event.
    /// Frames can be split or coalesced independently of transport notification boundaries.
    pub fn push(&mut self, byte: u8) -> Option<Result<Vec<u8>, FrameError>> {
        if byte == 0 {
            self.synchronized = true;
            if self.encoded.is_empty() {
                return None;
            }
            let result = cobs::decode_vec(&self.encoded).map_err(|_| FrameError::Malformed);
            self.encoded.clear();
            return Some(result);
        }
        if !self.synchronized {
            return None;
        }
        if self.encoded.len() == self.limit {
            self.reset();
            return Some(Err(FrameError::TooLong { limit: self.limit }));
        }
        self.encoded.push(byte);
        None
    }

    /// Discard partial input and synchronization, for example after disconnect.
    pub fn reset(&mut self) {
        self.encoded.clear();
        self.synchronized = false;
    }
}
