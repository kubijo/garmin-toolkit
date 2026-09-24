//! Automation commands shared by HTTP adapters; JSON is opaque on the postcard RPC boundary.
#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC request types"
)]

use remoc::rtc;
use serde::{Deserialize, Serialize};

pub const OPERATIONS: &[&str] = &[
    "list", "start", "status", "result", "cancel", "targets", "action", "sequence",
];
pub const ACTIONS: &[&str] = &[
    "click",
    "drag",
    "scroll",
    "wheel",
    "key",
    "text",
    "resize",
    "wait",
    "assert_available",
    "assert_value",
];
pub const MAX_COMMAND_BYTES: usize = 16 * 1024;
pub const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// HTTP-only model. Serialize to JSON before sending through Remoc/postcard.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ControlCommand {
    pub operation: String,
    #[serde(default)]
    pub argument: serde_json::Value,
}

#[must_use]
pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"version": 1, "operations": OPERATIONS, "actions": ACTIONS})
}

#[garmin_macros::portable(eq)]
pub struct ControlSession {
    /// Changes on every transport connection. Never silently retarget a request.
    pub id: String,
    /// Identifies a root browser page across its transport reconnects.
    pub browser: String,
    /// Monotonic milliseconds since this broker started, unrelated to wall clocks.
    pub server_time_ms: u64,
    pub last_request_id: u64,
}

#[garmin_macros::portable(eq)]
pub struct ControlDispatch {
    pub request_id: u64,
    pub command_json: String,
    pub expires_at_ms: u64,
}

#[rtc::remote]
pub trait BrowserControl {
    async fn execute(
        &self,
        request: ControlDispatch,
    ) -> Result<Result<String, String>, rtc::CallError>;
}
