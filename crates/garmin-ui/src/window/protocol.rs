//! Typed envelopes and acknowledgement bookkeeping shared by window consumers.
use serde::{Deserialize, Serialize};

pub const SESSION_PARAMETER: &str = "app-window";
pub const KIND_PARAMETER: &str = "window-kind";

#[must_use]
pub fn valid_session(session: &str) -> bool {
    session.len() == 32 && session.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Deserialize, Serialize)]
pub enum Message<C, S> {
    Poll,
    Command { id: u32, request: C },
    Snapshot(Box<S>),
    Reply { id: u32, error: Option<String> },
}

/// Connection and acknowledgement bookkeeping, independent of browser timers.
#[derive(Default)]
pub struct Link {
    last_seen: Option<f64>,
    pending: Option<(u32, f64)>,
    next_id: u32,
    pub error: Option<String>,
}

impl Link {
    pub fn observe(&mut self, now: f64) {
        self.last_seen = Some(now);
    }

    #[must_use]
    pub fn connected(&self, now: f64) -> bool {
        self.last_seen.is_some_and(|seen| now - seen < 5.0)
    }

    #[must_use]
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn begin(&mut self, now: f64) -> Option<u32> {
        if !self.connected(now) || self.pending() {
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.pending = Some((self.next_id, now));
        self.error = None;
        Some(self.next_id)
    }

    pub fn reply(&mut self, id: u32, error: Option<String>, now: f64) {
        if self.pending.is_some_and(|(pending, _)| pending == id) {
            self.pending = None;
            self.error = error;
            self.observe(now);
        }
    }

    pub fn expire(&mut self, now: f64) {
        if self.pending.is_some_and(|(_, sent)| now - sent > 5.0) {
            self.pending = None;
            self.error = Some(
                "No command acknowledgement from the app tab. Check its status before retrying."
                    .into(),
            );
        }
    }
}
