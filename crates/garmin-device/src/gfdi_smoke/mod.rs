//! Opt-in Linux GFDI identity, pairing and download experiments;
//! no ordinary discovery path invokes this module.

mod bluez;
mod capture;
mod download_fake;
mod fake;
mod fetch;
mod pairing;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use garmin_gfdi::{
    Characteristic, DeviceInformation, IdentityProbe, LinkMode, ProbeError, Route, select_route,
};
use thiserror::Error;
use tokio::time::{Instant, MissedTickBehavior};

use capture::Capture;

pub use fetch::{DownloadReport, download, simulate_download};
pub use pairing::pair;

/// Explicit live target; the caller must satisfy ADR 0011 before invoking it.
#[derive(Debug)]
pub struct LiveOptions {
    /// Exact powered adapter name;
    /// no default adapter or power changes.
    pub adapter: String,
    /// Exact device address known to the host.
    /// Identity and download require an existing bond; only `pair` may create one.
    pub device: bluer::Address,
    /// New owner-private capture directory,
    /// never an existing path.
    pub capture_dir: PathBuf,
    /// Whole observation timeout plus bounded cleanup:
    /// one to sixty seconds for identity, one to 600 for pairing/download.
    pub timeout: Duration,
    /// Explicit mode request;
    /// never automatically downgraded.
    pub mode: LinkMode,
}

/// Experimental GFDI command failure.
#[derive(Debug, Error)]
pub enum SmokeError {
    /// Capture filesystem failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Capture serialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Native Bluetooth operation failed.
    #[error(transparent)]
    Bluetooth(#[from] bluer::Error),
    /// Core protocol rejected the exchange.
    #[error(transparent)]
    Protocol(#[from] ProbeError),
    /// `FileAccess` download failed.
    #[error(transparent)]
    Download(#[from] garmin_gfdi::download::DownloadError),
    /// Downloaded bytes failed activity FIT validation.
    #[error(transparent)]
    Fit(#[from] garmin_fit::ActivityError),
    /// The caller's setup or target is outside this harness's boundary.
    #[error("GFDI smoke precondition failed: {0}")]
    Preconditions(&'static str),
    /// No complete known receive/send route was present.
    #[error("no supported complete GFDI characteristic pair")]
    NoRoute,
    /// The complete observation expired.
    #[error("GFDI smoke deadline expired")]
    Timeout,
    /// The caller cancelled the observation.
    #[error("GFDI smoke cancelled")]
    Cancelled,
    /// The notification stream ended before a validated identity arrived.
    #[error("GFDI notification stream ended before identity")]
    Disconnected,
    /// Capture count or raw-byte bounds were exhausted.
    #[error("GFDI smoke capture limit exceeded")]
    TraceLimit,
    /// Cleanup failed; do not report a clean smoke result.
    #[error("GFDI smoke cleanup failed ({cleanup}); observation: {observation}")]
    Cleanup {
        /// Original observation outcome,
        /// retained even when cleanup also fails.
        observation: String,
        /// Cleanup failure.
        cleanup: String,
    },
}

#[async_trait]
trait Transport: Send {
    async fn bond(&mut self) -> Result<(), SmokeError> {
        Err(SmokeError::Preconditions(
            "transport does not support explicit pairing",
        ))
    }
    async fn connect(&mut self) -> Result<(), SmokeError>;
    async fn inventory(&mut self) -> Result<Vec<Characteristic>, SmokeError>;
    async fn subscribe(&mut self, route: Route) -> Result<(), SmokeError>;
    async fn write(&mut self, bytes: &[u8]) -> Result<(), SmokeError>;
    async fn next(&mut self) -> Result<Option<Vec<u8>>, SmokeError>;
    async fn disconnect(&mut self) -> Result<(), SmokeError>;
}

/// Run a bounded native-protocol identity observation
/// on an approved disposable device.
///
/// Never scans, pairs, changes trust/power, or sends GFDI application replies.
/// Refuses already-connected targets at preflight.
///
/// Connection ownership cannot be atomic across independent Bluetooth clients;
/// run without competing host clients.
/// Raw private captures may contain device identity.
///
/// # Errors
/// Rejects invalid setup and reports protocol, timeout, capture, and cleanup failures.
pub async fn live(options: LiveOptions) -> Result<DeviceInformation, SmokeError> {
    validate_timeout(options.timeout)?;
    let mut capture = Capture::create(&options.capture_dir)?;
    capture.record("live_start", None)?;
    capture.metadata(
        "target",
        &serde_json::json!({ "adapter": options.adapter, "device": options.device.to_string() }),
    )?;
    let deadline = Instant::now() + options.timeout;
    // Preconditions are read before the owned connect attempt, under the same bounded setup.
    let mut transport = tokio::time::timeout_at(
        deadline,
        bluez::BluezTransport::open(&options.adapter, options.device),
    )
    .await
    .map_err(|_| SmokeError::Timeout)??;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(SmokeError::Timeout);
    }
    execute(
        &mut transport,
        &mut capture,
        remaining,
        options.mode,
        cancelled(),
    )
    .await
}

/// Run the same driver against a synthetic peer,
/// without any system-bus or radio access.
///
/// # Errors
/// Reports capture, protocol, deadline, and cleanup
/// failures exactly as live mode does.
pub async fn simulate(capture_dir: PathBuf) -> Result<DeviceInformation, SmokeError> {
    let mut capture = Capture::create(&capture_dir)?;
    capture.record("synthetic_start", None)?;
    execute(
        &mut fake::SyntheticPeer::new(),
        &mut capture,
        Duration::from_secs(5),
        LinkMode::Reliable,
        cancelled(),
    )
    .await
}

async fn cancelled() -> Result<(), SmokeError> {
    tokio::signal::ctrl_c().await?;
    Ok(())
}

fn validate_timeout(timeout: Duration) -> Result<(), SmokeError> {
    if !(Duration::from_secs(1)..=Duration::from_secs(60)).contains(&timeout) {
        return Err(SmokeError::Preconditions(
            "timeout must be between one and sixty seconds",
        ));
    }
    Ok(())
}

async fn execute<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    timeout: Duration,
    mode: LinkMode,
    cancellation: impl Future<Output = Result<(), SmokeError>> + Send,
) -> Result<DeviceInformation, SmokeError> {
    let start = Instant::now();
    let mut probe = None;
    let mut connect_attempted = false;
    let result = tokio::select! {
        biased;
        signal = cancellation => signal.and(Err(SmokeError::Cancelled)),
        result = tokio::time::timeout(timeout, observe(transport, capture, start, timeout, mode, &mut probe, &mut connect_attempted)) => {
            result.unwrap_or(Err(SmokeError::Timeout))
        }
    };
    let close = probe.as_ref().and_then(IdentityProbe::close_request);
    if let Some(probe) = &mut probe {
        probe.cancel();
    }
    let close_result = if let Some(close) = close {
        tokio::time::timeout(
            Duration::from_secs(2),
            close_owned(transport, capture, close),
        )
        .await
        .unwrap_or(Err(SmokeError::Timeout))
    } else {
        Ok(())
    };
    // Always attempt disconnect, even after connect failure,
    // cancellation, trace failure, or close failure.
    let disconnect_result = if connect_attempted {
        tokio::time::timeout(Duration::from_secs(3), transport.disconnect())
            .await
            .unwrap_or(Err(SmokeError::Timeout))
    } else {
        Ok(())
    };
    let event = if !connect_attempted {
        "connection_not_started"
    } else if disconnect_result.is_ok() {
        "disconnected"
    } else {
        "disconnect_failed"
    };
    let log_result = capture.record(event, None).and_then(|()| capture.finish());
    let failures: Vec<_> = [close_result, disconnect_result, log_result]
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.to_string())
        .collect();
    if failures.is_empty() {
        result
    } else {
        Err(SmokeError::Cleanup {
            observation: result
                .err()
                .map_or_else(|| "identity observed".to_owned(), |error| error.to_string()),
            cleanup: failures.join("; "),
        })
    }
}

async fn observe<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    start: Instant,
    timeout: Duration,
    mode: LinkMode,
    probe: &mut Option<IdentityProbe>,
    connect_attempted: &mut bool,
) -> Result<DeviceInformation, SmokeError> {
    *connect_attempted = true;
    transport.connect().await?;
    capture.record("connected", None)?;
    let inventory = transport.inventory().await?;
    let route = select_route(&inventory).ok_or(SmokeError::NoRoute)?;
    capture.metadata(
        "route",
        &serde_json::json!({
            "kind": format!("{:?}", route.kind),
            "service": uuid::Uuid::from_u128(route.service).to_string(),
            "receive": uuid::Uuid::from_u128(route.receive).to_string(),
            "send": uuid::Uuid::from_u128(route.send).to_string(),
        }),
    )?;
    transport.subscribe(route).await?;
    capture.record("subscribed", None)?;
    let random = uuid::Uuid::new_v4().as_u128().to_le_bytes();
    let client_id =
        NonZeroU64::new(u64::from_le_bytes(random[..8].try_into().map_err(
            |_| SmokeError::Preconditions("client ID generation failed"),
        )?))
        .ok_or(SmokeError::Preconditions("generated zero client ID"))?;

    // Incoming values are bounded by the ATT attribute-value maximum.
    // Outgoing packets here are <=13 bytes.
    let (engine, request) = IdentityProbe::begin(route.kind, client_id, mode, 512, timeout);
    *probe = Some(engine);
    if let Some(request) = request {
        send(transport, capture, "register", &request).await?;
    }
    let engine = probe
        .as_mut()
        .ok_or(SmokeError::Preconditions("identity engine missing"))?;
    let mut timer = tokio::time::interval(Duration::from_millis(5));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            bytes = transport.next() => {
                let bytes = bytes?.ok_or(SmokeError::Disconnected)?;
                if bytes.len() > 512 { return Err(SmokeError::Preconditions("notification exceeds ATT value limit")); }
                capture.record("receive", Some(&bytes))?;
                for ack in engine.receive(&bytes, start.elapsed())? {
                    send(transport, capture, "transport_ack", &ack).await?;
                }
                if let Some(identity) = engine.identity() { return Ok(identity.clone()); }
            }
            _ = timer.tick() => {
                if let Some(ack) = engine.poll(start.elapsed())? {
                    send(transport, capture, "transport_ack", &ack).await?;
                }
            }
        }
    }
}

async fn send<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    kind: &str,
    bytes: &[u8],
) -> Result<(), SmokeError> {
    capture.record(kind, Some(bytes))?;
    transport.write(bytes).await
}

async fn close_owned<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    request: [u8; 13],
) -> Result<(), SmokeError> {
    // Capture failure must not prevent releasing
    // the one handle already allocated to this attempt.
    let mut logged = capture.record("close_owned", Some(&request));
    transport.write(&request).await?;
    for _ in 0..128 {
        let bytes = transport.next().await?.ok_or(SmokeError::Disconnected)?;
        if let Err(error) = capture.record("cleanup_receive", Some(&bytes))
            && logged.is_ok()
        {
            logged = Err(error);
        }
        if bytes.len() == 14
            && bytes[..2] == [0, 3]
            && bytes[2..13] == request[2..13]
            && bytes[13] == 0
        {
            return logged;
        }
    }
    Err(SmokeError::Preconditions(
        "owned handle close was not confirmed",
    ))
}
