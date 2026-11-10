use std::future::Future;
use std::io::Write as _;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::Duration;

use garmin_gfdi::download::{DownloadSession, DownloadedFile};
use garmin_gfdi::{DeviceInformation, LinkMode, RouteKind, select_route};
use sha2::{Digest as _, Sha256};
use tokio::time::{Instant, MissedTickBehavior};

use super::{
    Capture, LiveOptions, SmokeError, Transport, bluez, cancelled, close_owned, download_fake, send,
};

const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

/// Retained, validated activity and its Bluetooth source identity.
#[derive(Debug)]
pub struct DownloadReport {
    /// Validated peer identity.
    pub identity: DeviceInformation,
    /// Newly created private activity FIT artifact.
    pub path: PathBuf,
    /// Original uncompressed byte count.
    pub bytes: usize,
    /// SHA-256 of the retained original bytes.
    pub sha256: String,
}

/// Download one activity using Protobuf `FileAccess` on an explicitly selected existing bond.
/// Uses the same powered/unconnected-device preconditions as the identity command.
///
/// # Errors
/// Reports unsupported routes, protocol/timeout/capture/cleanup failures, invalid FIT,
/// and retention failures. Never publishes an activity before cleanup and FIT validation.
pub async fn download(options: LiveOptions) -> Result<DownloadReport, SmokeError> {
    validate_download_timeout(options.timeout)?;
    let mut capture = Capture::create_download(&options.capture_dir)?;
    capture.metadata(
        "download_target",
        &serde_json::json!({"adapter": options.adapter, "device": options.device.to_string()}),
    )?;
    let deadline = Instant::now() + options.timeout;
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
    let (identity, file) = execute_download(
        &mut transport,
        &mut capture,
        remaining,
        options.mode,
        cancelled(),
    )
    .await?;
    retain(identity, &file, &options.capture_dir, &mut capture)
}

/// Exercise the complete `FileAccess` driver and FIT retention against a synthetic peer.
///
/// # Errors
/// Reports the same protocol, capture, cleanup and FIT-validation failures as live mode.
pub async fn simulate_download(directory: PathBuf) -> Result<DownloadReport, SmokeError> {
    let mut capture = Capture::create_download(&directory)?;
    capture.record("synthetic_download_start", None)?;
    let mut peer = download_fake::FilePeer::new()?;
    let (identity, file) = execute_download(
        &mut peer,
        &mut capture,
        Duration::from_secs(10),
        LinkMode::Reliable,
        cancelled(),
    )
    .await?;
    retain(identity, &file, &directory, &mut capture)
}

fn validate_download_timeout(timeout: Duration) -> Result<(), SmokeError> {
    if !(Duration::from_secs(1)..=Duration::from_secs(600)).contains(&timeout) {
        return Err(SmokeError::Preconditions(
            "download timeout must be between one and 600 seconds",
        ));
    }
    Ok(())
}

pub(super) async fn execute_download<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    timeout: Duration,
    mode: LinkMode,
    cancellation: impl Future<Output = Result<(), SmokeError>> + Send,
) -> Result<(DeviceInformation, DownloadedFile), SmokeError> {
    let start = Instant::now();
    let mut engine = None;
    let mut attempted = false;
    let result = tokio::select! {
        biased;
        signal = cancellation => signal.and(Err(SmokeError::Cancelled)),
        result = tokio::time::timeout(timeout, observe_download(transport, capture, start, timeout, mode, &mut engine, &mut attempted)) => result.unwrap_or(Err(SmokeError::Timeout)),
    };
    let closes = engine
        .as_ref()
        .map(DownloadSession::close_requests)
        .unwrap_or_default();
    if let Some(engine) = &mut engine {
        engine.cancel();
    }
    let mut failures = Vec::new();
    for close in closes {
        if let Err(error) = tokio::time::timeout(
            Duration::from_secs(2),
            close_owned(transport, capture, close),
        )
        .await
        .unwrap_or(Err(SmokeError::Timeout))
        {
            failures.push(error.to_string());
        }
    }
    let disconnected = if attempted {
        tokio::time::timeout(Duration::from_secs(3), transport.disconnect())
            .await
            .unwrap_or(Err(SmokeError::Timeout))
    } else {
        Ok(())
    };
    let event = if !attempted {
        "connection_not_started"
    } else if disconnected.is_ok() {
        "disconnected"
    } else {
        "disconnect_failed"
    };
    if let Err(error) = disconnected {
        failures.push(error.to_string());
    }
    if let Err(error) = capture.record(event, None).and_then(|()| capture.finish()) {
        failures.push(error.to_string());
    }
    if failures.is_empty() {
        result
    } else {
        Err(SmokeError::Cleanup {
            observation: result
                .err()
                .map_or_else(|| "file received".to_owned(), |error| error.to_string()),
            cleanup: failures.join("; "),
        })
    }
}

async fn observe_download<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    start: Instant,
    timeout: Duration,
    mode: LinkMode,
    engine: &mut Option<DownloadSession>,
    attempted: &mut bool,
) -> Result<(DeviceInformation, DownloadedFile), SmokeError> {
    *attempted = true;
    transport.connect().await?;
    capture.record("connected", None)?;
    let route = select_route(&transport.inventory().await?).ok_or(SmokeError::NoRoute)?;
    if route.kind != RouteKind::MultiLink {
        return Err(SmokeError::Preconditions(
            "FileAccess requires a MultiLink route; no legacy fallback",
        ));
    }
    capture.metadata("route", &serde_json::json!({"service": uuid::Uuid::from_u128(route.service).to_string(), "receive": uuid::Uuid::from_u128(route.receive).to_string(), "send": uuid::Uuid::from_u128(route.send).to_string()}))?;
    transport.subscribe(route).await?;
    let random = uuid::Uuid::new_v4().as_u128().to_le_bytes();
    let client =
        NonZeroU64::new(u64::from_le_bytes(random[..8].try_into().map_err(
            |_| SmokeError::Preconditions("client ID generation failed"),
        )?))
        .ok_or(SmokeError::Preconditions("zero client ID"))?;
    let (session, request) = DownloadSession::begin(client, mode, timeout, MAX_FILE_BYTES);
    *engine = Some(session);
    send(transport, capture, "register", &request).await?;
    let engine = engine
        .as_mut()
        .ok_or(SmokeError::Preconditions("download engine missing"))?;
    let mut timer = tokio::time::interval(Duration::from_millis(5));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut received = 0_usize;
    let mut last_progress = Duration::ZERO;
    loop {
        let outgoing = tokio::select! {
            packet = transport.next() => {
                let packet = packet?.ok_or(SmokeError::Disconnected)?;
                if packet.len() > 512 { return Err(SmokeError::Preconditions("notification exceeds ATT value limit")); }
                received = received.saturating_add(packet.len());
                capture.record("receive", Some(&packet))?;
                engine.receive(&packet, start.elapsed())?
            }
            _ = timer.tick() => engine.poll(start.elapsed())?,
        };
        for packet in outgoing {
            send(transport, capture, "send", &packet).await?;
        }
        if start.elapsed().saturating_sub(last_progress) >= Duration::from_secs(1) {
            eprintln!(
                "Bluetooth download: {} bytes received, {} s elapsed",
                received,
                start.elapsed().as_secs()
            );
            last_progress = start.elapsed();
        }
        if let Some(file) = engine.take_file() {
            let identity = engine.identity().cloned().ok_or(SmokeError::Preconditions(
                "download completed without identity",
            ))?;
            return Ok((identity, file));
        }
    }
}

fn retain(
    identity: DeviceInformation,
    file: &DownloadedFile,
    directory: &Path,
    capture: &mut Capture,
) -> Result<DownloadReport, SmokeError> {
    garmin_fit::normalize_activities(&file.bytes)?;
    let path = directory.join("activity.fit");
    let mut artifact = tempfile::NamedTempFile::new_in(directory)?;
    artifact.write_all(&file.bytes)?;
    artifact.as_file().sync_all()?;
    artifact
        .persist_noclobber(&path)
        .map_err(|error| error.error)?;
    std::fs::File::open(directory)?.sync_all()?;
    let sha256 = hex::encode(Sha256::digest(&file.bytes));
    let uid = file
        .item
        .uid
        .as_option()
        .ok_or(SmokeError::Preconditions("retained item UID missing"))?;
    capture.metadata("retained", &serde_json::json!({"file": "activity.fit", "bytes": file.bytes.len(), "sha256": sha256, "item_uid": format!("{:016x}{:016x}", uid.most_significant, uid.least_significant), "product_number": identity.product_number, "software_version": identity.software_version}))?;
    capture.finish()?;
    Ok(DownloadReport {
        identity,
        path,
        bytes: file.bytes.len(),
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_gfdi::proto::{data_types, file_access as wire};

    fn artifact(bytes: Vec<u8>) -> DownloadedFile {
        DownloadedFile {
            item: wire::ItemReference {
                uid: data_types::UUID::default().into(),
                ..Default::default()
            },
            bytes,
        }
    }

    #[test]
    fn malformed_fit_is_never_published_and_existing_artifact_is_never_replaced() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("capture");
        let mut capture = Capture::create_download(&directory).unwrap();
        let identity = DeviceInformation::decode(&[0; 15]).unwrap();
        assert!(matches!(
            retain(
                identity.clone(),
                &artifact(vec![1, 2, 3]),
                &directory,
                &mut capture
            ),
            Err(SmokeError::Fit(_))
        ));
        assert!(!directory.join("activity.fit").exists());
        std::fs::write(directory.join("activity.fit"), b"keep").unwrap();
        let bytes = super::download_fake::FilePeer::new()
            .unwrap()
            .source()
            .to_vec();
        assert!(matches!(
            retain(identity, &artifact(bytes), &directory, &mut capture),
            Err(SmokeError::Io(_))
        ));
        assert_eq!(
            std::fs::read(directory.join("activity.fit")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn download_deadline_has_an_explicit_upper_and_lower_bound() {
        assert!(validate_download_timeout(Duration::ZERO).is_err());
        assert!(validate_download_timeout(Duration::from_secs(601)).is_err());
        assert!(validate_download_timeout(Duration::from_secs(1)).is_ok());
        assert!(validate_download_timeout(Duration::from_secs(600)).is_ok());
    }
}
