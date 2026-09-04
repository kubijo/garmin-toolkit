pub mod shadow;
pub mod virtual_device;

use anyhow::{Context, Result, bail};
use axum::body::{Body, Bytes};
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use garmin_update::DeviceTiming;
use md5::{Digest, Md5};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use url::Url;

pub const MOCK_DEVICE_MARKER: &str = ".garmin-cli-mock-device";
pub const MOCK_DEVICE_MARKER_CONTENT: &[u8] = b"garmin-cli mock fixture\n";

const DOWNLOAD_CHUNK_BYTES: usize = 256 * 1024;
const DOWNLOAD_CHUNK_DELAY: Duration = Duration::from_millis(200);
const API_RESPONSE_LATENCY: Duration = Duration::from_millis(500);
const EXPECTED_USER_AGENT: &str = "RestSharp/112.0.0.0";
const DEVICE_BYTES_PER_SECOND: NonZeroU64 = NonZeroU64::new(3_000_000).unwrap();
const DEVICE_OPERATION_LATENCY: Duration = Duration::from_millis(300);

/// Requires the exact marker written by the synthetic-device builder.
/// # Errors
/// [`MockDeviceRootError`] when the root or marker could redirect
/// through a symbolic link, or when the marker has unexpected contents.
pub fn require_mock_device_root(root: &Path) -> Result<(), MockDeviceRootError> {
    let root_metadata = fs::symlink_metadata(root).map_err(MockDeviceRootError::RootMetadata)?;
    if !root_metadata.file_type().is_dir() || root_metadata.file_type().is_symlink() {
        return Err(MockDeviceRootError::UnsafeRoot(root.to_owned()));
    }

    let marker = root.join(MOCK_DEVICE_MARKER);
    let marker_metadata = fs::symlink_metadata(&marker).map_err(MockDeviceRootError::MarkerRead)?;
    if !marker_metadata.file_type().is_file() || marker_metadata.file_type().is_symlink() {
        return Err(MockDeviceRootError::UnsafeMarker(marker));
    }
    let contents = fs::read(&marker).map_err(MockDeviceRootError::MarkerRead)?;
    if contents != MOCK_DEVICE_MARKER_CONTENT {
        return Err(MockDeviceRootError::InvalidMarker(marker));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum MockDeviceRootError {
    #[error("cannot inspect mock device root: {0}")]
    RootMetadata(std::io::Error),
    #[error("mock device root is not a real directory: {0}")]
    UnsafeRoot(PathBuf),
    #[error("cannot read mock device marker: {0}")]
    MarkerRead(std::io::Error),
    #[error("mock device marker is not a regular file: {0}")]
    UnsafeMarker(PathBuf),
    #[error("mock device marker has unexpected contents: {0}")]
    InvalidMarker(PathBuf),
}

#[derive(Debug, Clone)]
struct AppState {
    base: Url,
    files: Arc<HashMap<String, Bytes>>,
    authorization: MockAuthorization,
    active_downloads: Arc<AtomicUsize>,
    max_parallel_downloads: Arc<AtomicUsize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateRequest {
    garmin_device_xml: String,
    options: UpdateOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateOptions {
    grouping_mode: String,
    extraction_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivationRequest {
    garmin_device_xml: String,
    installed_maps: Vec<ActivationMapIdentifier>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ActivationMapIdentifier {
    region_part_number: Option<String>,
    map_image_part_number: Option<String>,
    activation_request_code: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReadyFile<'a> {
    base_url: &'a Url,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MockStats {
    pub active_downloads: usize,
    pub max_parallel_downloads: usize,
}

#[derive(Debug, Serialize)]
pub struct FixtureVerification {
    pub fixture: PathBuf,
    pub committed_update_verified: bool,
    pub files_verified: usize,
    pub obsolete_files_removed: usize,
    pub transaction_artifacts_removed: bool,
}

struct ActiveDownload(Arc<AtomicUsize>);

impl Drop for ActiveDownload {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct MockServer {
    base_url: Url,
    state: AppState,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), std::io::Error>>,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum MockAuthorization {
    #[default]
    Supported,
    SignedStorageData,
}

impl MockServer {
    /// Start an in-process mock service on an ephemeral IPv4 loopback port.
    /// # Errors
    /// An error if the listener cannot be bound or its URL is invalid.
    pub async fn start() -> Result<Self> {
        Self::start_with_authorization(MockAuthorization::Supported).await
    }

    /// Start with a selected authorization response.
    /// # Errors
    /// An error if the listener cannot be bound or its URL is invalid.
    pub async fn start_with_authorization(authorization: MockAuthorization) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = loopback_base_url(listener.local_addr()?)?;
        let state = app_state(&base_url, authorization);
        let router = router(state.clone());
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = receiver.await;
                })
                .await
        });
        Ok(Self {
            base_url,
            state,
            shutdown: Some(shutdown),
            task,
        })
    }

    #[must_use]
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    #[must_use]
    pub fn stats(&self) -> MockStats {
        MockStats {
            active_downloads: self.state.active_downloads.load(Ordering::SeqCst),
            max_parallel_downloads: self.state.max_parallel_downloads.load(Ordering::SeqCst),
        }
    }

    /// Gracefully stop the in-process service.
    /// # Errors
    /// An error when the server task fails or cannot be joined.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        (&mut self.task)
            .await
            .context("mock server task did not shut down cleanly")??;
        Ok(())
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

/// Serve the mock API until Ctrl-C.
/// # Errors
/// An error if the address is not loopback.
///
/// Listener, HTTP server, and ready-file failures are also returned.
pub async fn serve(listen: SocketAddr, ready_file: Option<&Path>) -> Result<()> {
    if !listen.ip().is_loopback() {
        bail!("mock server may listen only on a loopback address");
    }
    let listener = TcpListener::bind(listen).await?;
    let address = listener.local_addr()?;
    let base = loopback_base_url(address)?;
    let router = router(app_state(&base, MockAuthorization::Supported));

    if let Some(path) = ready_file {
        let bytes = serde_json::to_vec(&ReadyFile { base_url: &base })?;
        tokio::fs::write(path, bytes).await?;
    }
    println!("{base}");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn app_state(base: &Url, authorization: MockAuthorization) -> AppState {
    AppState {
        base: base.clone(),
        files: Arc::new(mock_files()),
        authorization,
        active_downloads: Arc::new(AtomicUsize::new(0)),
        max_parallel_downloads: Arc::new(AtomicUsize::new(0)),
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/maps/universal/update", post(update))
        .route("/api/maps/universal/activate", post(activate))
        .route("/downloads/{name}", get(download))
        .route("/__mock/stats", get(stats))
        .with_state(state)
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    validate_client_headers(&headers)?;
    validate_manifest(&request.garmin_device_xml)?;
    if request.options.grouping_mode != "GroupByGeographicArea"
        || request.options.extraction_mode != "ExtractIndependentSmallerRegions"
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    tokio::time::sleep(API_RESPONSE_LATENCY).await;
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        update_response_fixture(&state),
    ))
}

async fn activate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ActivationRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    validate_client_headers(&headers)?;
    validate_manifest(&request.garmin_device_xml)?;
    if request.installed_maps.is_empty()
        || request.installed_maps.iter().all(|identifier| {
            identifier.region_part_number.is_none()
                && identifier.map_image_part_number.is_none()
                && identifier.activation_request_code.is_none()
        })
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    tokio::time::sleep(API_RESPONSE_LATENCY).await;
    let response = match state.authorization {
        MockAuthorization::Supported => include_str!("../fixtures/activate-response.json"),
        MockAuthorization::SignedStorageData => {
            r#"{"signedSdCardBytes":"c2lnbmVk","unlocks":[],"embeddedUnlocks":[]}"#
        }
    };
    Ok(([(header::CONTENT_TYPE, "application/json")], response))
}

async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(name): AxumPath<String>,
) -> Result<Body, StatusCode> {
    validate_user_agent(&headers)?;
    if headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        != Some("*/*")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let bytes = state
        .files
        .get(&name)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    let active = state.active_downloads.fetch_add(1, Ordering::SeqCst) + 1;
    state
        .max_parallel_downloads
        .fetch_max(active, Ordering::SeqCst);
    let active = ActiveDownload(Arc::clone(&state.active_downloads));
    let stream = futures_util::stream::unfold(
        (bytes, 0_usize, active),
        |(bytes, offset, active)| async move {
            if offset >= bytes.len() {
                return None;
            }
            tokio::time::sleep(DOWNLOAD_CHUNK_DELAY).await;
            let end = offset.saturating_add(DOWNLOAD_CHUNK_BYTES).min(bytes.len());
            let chunk = bytes.slice(offset..end);
            Some((Ok::<_, Infallible>(chunk), (bytes, end, active)))
        },
    );
    Ok(Body::from_stream(stream))
}

async fn stats(State(state): State<AppState>) -> Json<MockStats> {
    Json(MockStats {
        active_downloads: state.active_downloads.load(Ordering::SeqCst),
        max_parallel_downloads: state.max_parallel_downloads.load(Ordering::SeqCst),
    })
}

fn validate_client_headers(headers: &HeaderMap) -> Result<(), StatusCode> {
    validate_user_agent(headers)?;
    let client_name = headers
        .get("Garmin-Client-Name")
        .and_then(|value| value.to_str().ok());
    if client_name != Some("EXPRESS") {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

fn validate_user_agent(headers: &HeaderMap) -> Result<(), StatusCode> {
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok());
    if user_agent != Some(EXPECTED_USER_AGENT) {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

fn validate_manifest(xml: &str) -> Result<(), StatusCode> {
    let mut reader = Reader::from_str(xml);
    let mut root_seen = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) if !root_seen => {
                if start.local_name().as_ref() != "Device" {
                    return Err(StatusCode::BAD_REQUEST);
                }
                root_seen = true;
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(StatusCode::BAD_REQUEST),
            _ => {}
        }
    }
    root_seen.then_some(()).ok_or(StatusCode::BAD_REQUEST)
}

fn update_response_fixture(state: &AppState) -> String {
    let file_hash = |name| md5_hex(state.files.get(name).expect("known mock payload"));
    include_str!("../fixtures/update-response.json")
        .replace("{{BASE_URL}}", state.base.as_str())
        .replace("{{EUROPE_MD5}}", &file_hash("europe.img"))
        .replace("{{EUROPE_ROUTING_MD5}}", &file_hash("europe-routing.img"))
        .replace("{{TRAILS_MD5}}", &file_hash("trails.img"))
}

fn mock_files() -> HashMap<String, Bytes> {
    [
        ("europe.img", mock_bytes(0x31, 12_000_000)),
        ("europe-routing.img", mock_bytes(0x52, 10_000_000)),
        ("trails.img", mock_bytes(0x73, 8_000_000)),
    ]
    .into_iter()
    .map(|(name, bytes)| (name.to_owned(), bytes))
    .collect()
}

fn mock_bytes(seed: u8, length: usize) -> Bytes {
    Bytes::from(
        (0..length)
            .map(|index| seed.wrapping_add(u8::try_from(index % 251).expect("modulo fits u8")))
            .collect::<Vec<_>>(),
    )
}

fn md5_hex(bytes: &[u8]) -> String {
    hex::encode(Md5::digest(bytes))
}

fn loopback_base_url(address: SocketAddr) -> Result<Url> {
    let host = match address.ip() {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => format!("[{address}]"),
    };
    Ok(Url::parse(&format!("http://{host}:{}/", address.port()))?)
}

/// Create a marked synthetic Garmin mass-storage device.
/// # Errors
/// An error if the target already exists.
///
/// Serialization and file-system failures are also returned.
pub async fn create_fixture(root: &Path) -> Result<()> {
    if tokio::fs::symlink_metadata(root).await.is_ok() {
        bail!(
            "refusing to overwrite existing fixture path {}",
            root.display()
        );
    }
    let garmin = root.join("Garmin");
    let mock = garmin.join("Mock");
    tokio::fs::create_dir_all(&mock).await?;
    tokio::fs::write(
        garmin.join("GarminDevice.xml"),
        include_bytes!("../fixtures/GarminDevice.xml"),
    )
    .await?;
    tokio::fs::write(root.join(MOCK_DEVICE_MARKER), MOCK_DEVICE_MARKER_CONTENT).await?;
    for old in ["europe-old.img", "trails-old.img", "global-old.img"] {
        tokio::fs::write(mock.join(old), b"old mock content\n").await?;
    }
    Ok(())
}

/// Seed verified synthetic payloads into a download cache.
/// # Errors
/// An error for an unknown plan source, mismatched metadata, or file I/O.
pub async fn seed_download_cache(root: &Path, plan: &garmin_update::UpdatePlan) -> Result<()> {
    tokio::fs::create_dir_all(root).await?;
    let files = mock_files();
    for download in &plan.downloads {
        let name = download
            .source
            .path_segments()
            .and_then(Iterator::last)
            .context("mock plan source has no file name")?;
        let bytes = files
            .get(name)
            .with_context(|| format!("mock plan names an unknown payload: {name}"))?;
        if download.size != bytes.len() as u64 || download.md5 != md5_hex(bytes) {
            bail!("mock plan metadata differs for {name}");
        }
        tokio::fs::write(root.join(&download.cache_name), bytes).await?;
    }
    Ok(())
}

#[must_use]
pub const fn device_timing() -> DeviceTiming {
    DeviceTiming::limited(DEVICE_BYTES_PER_SECOND, DEVICE_OPERATION_LATENCY)
}

/// Verify that the complete mock update was committed to a marked fixture.
/// # Errors
/// An error for a missing marker, missing payload, or byte mismatch.
/// It also rejects stale files, transaction artifacts, and unreadable fixtures.
pub async fn verify_fixture(root: &Path) -> Result<FixtureVerification> {
    require_mock_device_root(root)?;
    verify_fixture_payloads(root).await
}

/// Verify synthetic payloads in a retained virtual-device tree.
///
/// # Errors
/// Missing, stale, or changed payloads and transaction debris.
pub async fn verify_fixture_payloads(root: &Path) -> Result<FixtureVerification> {
    let mock = root.join("Garmin/Mock");
    for (name, expected) in mock_files() {
        let path = mock.join(&name);
        let actual = tokio::fs::read(&path)
            .await
            .with_context(|| format!("cannot read committed mock payload {}", path.display()))?;
        if actual.as_slice() != expected.as_ref() {
            bail!("committed mock payload differs: {}", path.display());
        }
    }
    for old in ["europe-old.img", "trails-old.img", "global-old.img"] {
        if mock.join(old).exists() {
            bail!("obsolete mock payload was not removed: {old}");
        }
    }
    reject_transaction_artifacts(root).await?;
    Ok(FixtureVerification {
        fixture: root.to_owned(),
        committed_update_verified: true,
        files_verified: 3,
        obsolete_files_removed: 3,
        transaction_artifacts_removed: true,
    })
}

async fn reject_transaction_artifacts(root: &Path) -> Result<()> {
    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if entry.file_name() != MOCK_DEVICE_MARKER
                && entry.file_name().to_string_lossy().contains(".garmin-cli-")
            {
                bail!(
                    "transaction artifact remained after commit: {}",
                    entry.path().display()
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_payloads_have_distinct_integrity_values() {
        let files = mock_files();
        assert_eq!(files.len(), 3);
        let hashes = files
            .values()
            .map(|bytes| md5_hex(bytes))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(hashes.len(), files.len());
    }

    #[test]
    fn base_urls_support_both_loopback_address_families() {
        let v4 = loopback_base_url("127.0.0.1:1234".parse().unwrap()).unwrap();
        let v6 = loopback_base_url("[::1]:1234".parse().unwrap()).unwrap();
        assert_eq!(v4.as_str(), "http://127.0.0.1:1234/");
        assert_eq!(v6.as_str(), "http://[::1]:1234/");
    }
}
