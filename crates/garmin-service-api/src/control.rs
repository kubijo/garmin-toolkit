//! Automation commands shared by HTTP adapters; JSON is opaque on the postcard RPC boundary.
#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC request types"
)]

use remoc::rtc;
use serde::{Deserialize, Serialize};

pub const OPERATIONS: &[&str] = &[
    "list",
    "start",
    "status",
    "result",
    "cancel",
    "targets",
    "action",
    "sequence",
    "screenshot",
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
pub const MAX_CAPTURE_PIXELS: u32 = 8 * 1024 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;

/// Keep bounded browser replies buffered: Remoc's large-item streaming needs OS threads.
/// Both ends of the browser transport must use this configuration.
#[must_use]
pub fn transport_config() -> remoc::Cfg {
    remoc::Cfg {
        // Allow the PNG limit plus codec metadata and the RPC response envelope.
        max_data_size: MAX_CAPTURE_BYTES + 64 * 1024,
        ..remoc::Cfg::default()
    }
}

/// Capture timing identifies the request and receipt frames, not compositor presentation.
#[garmin_macros::portable]
pub struct CaptureInfo {
    pub width: u32,
    pub height: u32,
    pub pixels_per_point: f32,
    pub requested_frame: u64,
    pub received_frame: u64,
}

#[garmin_macros::portable]
pub struct Capture {
    pub info: CaptureInfo,
    pub png: Vec<u8>,
}

impl Capture {
    /// Validate dimensions and the PNG envelope before sending it to an HTTP client.
    /// # Errors
    /// Rejects invalid metadata, oversized images, and mismatched PNG dimensions.
    pub fn validate(&self) -> Result<(), String> {
        let info = &self.info;
        if info.width == 0
            || info.height == 0
            || u64::from(info.width) * u64::from(info.height) > u64::from(MAX_CAPTURE_PIXELS)
            || !info.pixels_per_point.is_finite()
            || info.pixels_per_point <= 0.0
            || self.png.len() > MAX_CAPTURE_BYTES
            || self.png.len() < 24
            || self.png[..8] != *b"\x89PNG\r\n\x1a\n"
            || self.png[12..16] != *b"IHDR"
            || self.png[16..20] != info.width.to_be_bytes()
            || self.png[20..24] != info.height.to_be_bytes()
        {
            return Err("invalid or oversized screenshot".into());
        }
        Ok(())
    }
}

/// HTTP-only model. Serialize to JSON before sending through Remoc/postcard.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ControlCommand {
    pub operation: String,
    #[serde(default)]
    pub argument: serde_json::Value,
}

#[must_use]
pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"version": 1, "operations": OPERATIONS, "actions": ACTIONS,
        "screenshots": true, "screenshot_max_pixels": MAX_CAPTURE_PIXELS,
        "screenshot_max_bytes": MAX_CAPTURE_BYTES})
}

#[garmin_macros::portable(eq)]
pub struct ControlSession {
    /// Changes on every transport connection. Never silently retarget a request.
    pub id: String,
    /// Identifies a root browser page across its transport reconnects.
    pub browser: String,
    /// Monotonic milliseconds since this broker started, unrelated to wall clocks.
    pub server_time_ms: u64,
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
    async fn capture(&self, expires_at_ms: u64) -> Result<Result<Capture, String>, rtc::CallError>;
}
