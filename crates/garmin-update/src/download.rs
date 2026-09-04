use crate::DownloadSpec;
use futures_util::StreamExt;
use garmin_capture::{
    CaptureError, HttpExchange, HttpHeader, HttpRequestHead, HttpResponseHead, SessionCapture,
};
use garmin_progress::{OperationStage, ProgressEvent, ProgressReporter, ProgressState};
use md5::{Digest, Md5};
use reqwest::header::{ACCEPT, CONTENT_RANGE, RANGE};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const MAX_ERROR_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
/// Supplies authorization for a protected download URL.
///
/// Applications inject service policy without replacing download, verification,
/// capture, or device execution.
pub trait DownloadUrlAuthorizer: Debug + Send + Sync {
    /// Return a request-ready URL for a protected deliverable.
    /// # Errors
    /// [`DownloadError`] when authorization is unavailable or fails.
    fn authorize(&self, source: &url::Url) -> Result<url::Url, DownloadError>;
}

#[derive(Debug)]
struct RejectProtectedDownloads;

impl DownloadUrlAuthorizer for RejectProtectedDownloads {
    fn authorize(&self, _source: &url::Url) -> Result<url::Url, DownloadError> {
        Err(DownloadError::ProtectedDownloadAuthorizationUnavailable)
    }
}

#[derive(Debug, Clone)]
pub struct Downloader {
    http: reqwest::Client,
    concurrency: usize,
    capture: Option<SessionCapture>,
    authorizer: Arc<dyn DownloadUrlAuthorizer>,
    cache_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

/// Size-checked availability for one set of content-addressed artifacts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactCacheSummary {
    pub cached_files: usize,
    pub total_files: usize,
    pub cached_bytes: u64,
    pub total_bytes: u64,
}

impl ArtifactCacheSummary {
    #[must_use]
    pub const fn is_complete(self) -> bool {
        self.cached_files == self.total_files
    }
}

/// Inspect cached artifacts without reading their payloads.
///
/// Staging still verifies every cached checksum before use.
/// # Errors
/// Unsafe entries and byte-count overflow are rejected.
pub async fn inspect_artifact_cache(
    specs: &[DownloadSpec],
    cache: &Path,
) -> Result<ArtifactCacheSummary, DownloadError> {
    let mut summary = ArtifactCacheSummary::default();
    let mut counted = HashSet::new();
    for spec in specs {
        if !counted.insert(&spec.cache_name) {
            continue;
        }
        summary.total_files += 1;
        summary.total_bytes = summary
            .total_bytes
            .checked_add(spec.size)
            .ok_or(DownloadError::TotalSizeOverflow)?;
        let path = cache_path(cache, spec)?;
        match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.file_type().is_file() && metadata.len() == spec.size => {
                summary.cached_files += 1;
                summary.cached_bytes = summary
                    .cached_bytes
                    .checked_add(spec.size)
                    .ok_or(DownloadError::TotalSizeOverflow)?;
            }
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err(DownloadError::UnsafeCacheEntry(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(summary)
}

impl Downloader {
    /// Create a downloader with bounded parallelism and no HTTP redirects.
    /// # Errors
    /// [`DownloadError`] if the HTTP client cannot be constructed.
    pub fn new(concurrency: usize, user_agent: &str) -> Result<Self, DownloadError> {
        if !(1..=32).contains(&concurrency) {
            return Err(DownloadError::InvalidConcurrency(concurrency));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(user_agent)
            .build()?;
        Ok(Self {
            http,
            concurrency: concurrency.max(1),
            capture: None,
            authorizer: Arc::new(RejectProtectedDownloads),
            cache_locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[must_use]
    pub fn with_capture(mut self, capture: Option<SessionCapture>) -> Self {
        self.capture = capture;
        self
    }

    #[must_use]
    pub fn with_url_authorizer(mut self, authorizer: Arc<dyn DownloadUrlAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    fn cache_lock(&self, cache_name: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .cache_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(
            locks
                .entry(cache_name.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    /// Download and verify every content item with bounded concurrency.
    /// # Errors
    /// [`DownloadError`] for cache, request, integrity, or worker failures.
    pub async fn stage_all(
        &self,
        specs: &[DownloadSpec],
        cache: &Path,
    ) -> Result<Vec<DownloadProgress>, DownloadError> {
        self.stage_all_with_progress(specs, cache, ProgressReporter::default())
            .await
    }

    /// Download and verify every content item while reporting aggregate progress.
    /// # Errors
    /// [`DownloadError`] for cache, request, integrity, or worker failures.
    pub async fn stage_all_with_progress(
        &self,
        specs: &[DownloadSpec],
        cache: &Path,
        progress: ProgressReporter,
    ) -> Result<Vec<DownloadProgress>, DownloadError> {
        tokio::fs::create_dir_all(cache).await?;
        let total = total_download_bytes(specs)?;
        let mut needed = 0_u64;
        let mut counted = HashSet::new();
        for spec in specs {
            let cached = cache_path(cache, spec)?;
            reject_unsafe_cache_entry(&cached).await?;
            if counted.insert(&spec.cache_name)
                && !verify_file(&cached, spec).await.unwrap_or(false)
            {
                needed = needed
                    .checked_add(spec.size)
                    .ok_or(DownloadError::TotalSizeOverflow)?;
            }
        }
        crate::space::check_host_space(cache, needed).await?;
        progress.started(
            OperationStage::Download,
            format!("Downloading {} map files", specs.len()),
            Some(total),
        );
        progress.started(
            OperationStage::Verify,
            "Waiting to verify downloaded files",
            Some(total),
        );
        let downloaded = Arc::new(Mutex::new(vec![0_u64; specs.len()]));
        let verified = Arc::new(Mutex::new(vec![0_u64; specs.len()]));
        let semaphore = Arc::new(Semaphore::new(self.concurrency));
        let mut tasks = JoinSet::new();
        for (index, spec) in specs.iter().cloned().enumerate() {
            let worker = self.clone();
            let cache = cache.to_owned();
            let semaphore = Arc::clone(&semaphore);
            let parent = progress.clone();
            let downloaded = Arc::clone(&downloaded);
            let verified = Arc::clone(&verified);
            let child_progress = ProgressReporter::from_handler(move |event| {
                forward_file_progress(event, index, total, &downloaded, &verified, &parent);
            })
            .with_cancellation(progress.cancellation_token())
            .for_item(format!("download:{index}"));
            tasks.spawn(async move {
                let _permit = semaphore.acquire_owned().await?;
                if child_progress.is_cancelled() {
                    return Err(DownloadError::Cancelled);
                }
                let progress = worker
                    .stage_with_progress(&spec, &cache, &child_progress)
                    .await?;
                Ok::<_, DownloadError>((index, progress))
            });
        }

        let mut completed = Vec::with_capacity(specs.len());
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(progress)) => completed.push(progress),
                Ok(Err(error)) => {
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                    progress.failed(OperationStage::Download, "A map download failed");
                    return Err(error);
                }
                Err(error) => {
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                    progress.failed(OperationStage::Download, "A download task failed");
                    return Err(DownloadError::Task(error));
                }
            }
        }
        completed.sort_by_key(|(index, _)| *index);
        if let Some(capture) = &self.capture {
            capture_downloads(capture, &completed, specs).await?;
        }
        let cached = completed
            .iter()
            .filter(|(_, download)| download.from_cache)
            .count();
        progress.completed(
            OperationStage::Download,
            completed_download_label(specs.len(), cached),
            total,
            Some(total),
        );
        progress.completed(
            OperationStage::Verify,
            completed_verification_label(specs.len(), cached),
            total,
            Some(total),
        );
        Ok(completed
            .into_iter()
            .map(|(_, progress)| progress)
            .collect())
    }

    /// Download and verify one content item, resuming a partial file when safe.
    /// # Errors
    /// [`DownloadError`] for cache I/O, request, size, or checksum failure.
    pub async fn stage(
        &self,
        spec: &DownloadSpec,
        cache: &Path,
    ) -> Result<DownloadProgress, DownloadError> {
        self.stage_with_progress(spec, cache, &ProgressReporter::default())
            .await
    }

    /// Download one content item while emitting typed progress events.
    /// # Errors
    /// [`DownloadError`] for cache I/O, request, size, or checksum failure.
    pub async fn stage_with_progress(
        &self,
        spec: &DownloadSpec,
        cache: &Path,
        progress: &ProgressReporter,
    ) -> Result<DownloadProgress, DownloadError> {
        let label = spec.map_name.clone();
        let display_path = spec.destination.as_path().display().to_string();
        progress.started_with_path(
            OperationStage::Download,
            &label,
            &display_path,
            Some(spec.size),
        );
        let cache_lock = self.cache_lock(&spec.cache_name);
        let _cache_guard = cache_lock.lock().await;
        if progress.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        tokio::fs::create_dir_all(cache).await?;
        require_cache_directory(cache).await?;
        let final_path = cache_path(cache, spec)?;
        reject_unsafe_cache_entry(&final_path).await?;
        if let Some(cached) = cached_download(&final_path, spec, progress).await? {
            return Ok(cached);
        }

        let partial = cache.join(format!("{}.part", spec.cache_name));
        reject_unsafe_cache_entry(&partial).await?;
        let (response, resumed_at, hash, append, exchange) = request_download(
            &self.http,
            self.authorizer.as_ref(),
            spec,
            &partial,
            self.capture.as_ref(),
            progress,
            &display_path,
        )
        .await?;
        let target = TransferTarget {
            spec,
            partial: &partial,
            progress,
            label: &label,
            display_path: &display_path,
            exchange: exchange.clone(),
        };
        let (bytes, hash, elapsed) =
            write_response(response, resumed_at, hash, append, target).await?;

        if bytes != spec.size {
            capture_rejected_response(exchange.as_ref(), resumed_at, &partial, bytes).await?;
            progress.failed(
                OperationStage::Download,
                "Downloaded size did not match the plan",
            );
            return Err(DownloadError::Size {
                expected: spec.size,
                actual: bytes,
                path: partial,
            });
        }
        progress.completed_with_path(
            OperationStage::Download,
            &label,
            &display_path,
            bytes,
            Some(spec.size),
        );
        progress.started_with_path(
            OperationStage::Verify,
            "Checking Garmin MD5",
            &display_path,
            Some(spec.size),
        );
        let actual_md5 = hex::encode(hash.finalize());
        if !actual_md5.eq_ignore_ascii_case(&spec.md5) {
            capture_rejected_response(exchange.as_ref(), resumed_at, &partial, bytes).await?;
            progress.failed(OperationStage::Verify, "Garmin MD5 mismatch");
            return Err(DownloadError::Checksum {
                expected: spec.md5.clone(),
                actual: actual_md5,
                path: partial,
            });
        }
        progress.completed_with_path(
            OperationStage::Verify,
            "Garmin MD5 verified",
            &display_path,
            bytes,
            Some(spec.size),
        );
        publish_cache_artifact(&partial, &final_path, exchange, resumed_at, bytes).await?;
        Ok(DownloadProgress {
            path: final_path,
            bytes,
            elapsed,
            resumed_at,
            from_cache: false,
        })
    }
}

async fn publish_cache_artifact(
    partial: &Path,
    final_path: &Path,
    exchange: Option<HttpExchange>,
    resumed_at: u64,
    bytes: u64,
) -> Result<(), DownloadError> {
    tokio::fs::rename(partial, final_path).await?;
    let mut permissions = tokio::fs::metadata(final_path).await?.permissions();
    permissions.set_readonly(true);
    tokio::fs::set_permissions(final_path, permissions).await?;
    if resumed_at == 0
        && let Some(exchange) = exchange
    {
        exchange
            .retain_response_body(
                final_path,
                usize::try_from(bytes).map_err(|_| DownloadError::SizeOverflow)?,
            )
            .await?;
    }
    Ok(())
}

fn forward_file_progress(
    event: ProgressEvent,
    index: usize,
    total: u64,
    downloaded: &Mutex<Vec<u64>>,
    verified: &Mutex<Vec<u64>>,
    parent: &ProgressReporter,
) {
    let values = match event.stage {
        OperationStage::Download => downloaded,
        OperationStage::Verify => verified,
        _ => return,
    };
    parent.event(event.clone());
    match event.state {
        ProgressState::Advanced | ProgressState::Completed => {
            let mut values = values.lock().unwrap_or_else(PoisonError::into_inner);
            values[index] = event.completed;
            let completed = values.iter().copied().sum();
            let action = match event.stage {
                OperationStage::Verify => "Verifying",
                _ => "Downloading",
            };
            parent.advanced(
                event.stage,
                format!("{action} {} map files", values.len()),
                completed,
                Some(total),
            );
        }
        ProgressState::Failed => parent.failed(event.stage, event.label),
        ProgressState::Started => {}
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    use garmin_progress::ProgressScope;

    fn spec(md5: &str, size: u64) -> DownloadSpec {
        DownloadSpec {
            map_name: "Cached map".to_owned(),
            source: url::Url::parse("https://example.com/map").unwrap(),
            alternate_sources: Vec::new(),
            requires_garmin_token: false,
            destination: garmin_device::SafeRelativePath::parse("Garmin/map.img").unwrap(),
            cache_name: md5.to_owned(),
            size,
            md5: md5.to_owned(),
        }
    }

    #[test]
    fn file_lifecycles_and_aggregate_counters_are_independent() {
        let (parent, receiver) = ProgressReporter::channel();
        let (child, incoming) = ProgressReporter::channel();
        let downloaded = Mutex::new(vec![0; 2]);
        let verified = Mutex::new(vec![0; 2]);
        for index in 0..2 {
            let item = child.for_item(index.to_string());
            item.started_with_path(
                OperationStage::Download,
                "Downloading",
                format!("Garmin/{index}.img"),
                Some(100),
            );
            item.completed(OperationStage::Download, "Downloaded", 100, Some(100));
            item.started(OperationStage::Verify, "Verifying", Some(100));
            item.completed(OperationStage::Verify, "Verified", 100, Some(100));
            for event in incoming.try_iter() {
                forward_file_progress(event, index, 200, &downloaded, &verified, &parent);
            }
        }
        let events = receiver.try_iter().collect::<Vec<_>>();
        let items = events
            .iter()
            .filter(|event| matches!(event.scope, ProgressScope::Item { .. }))
            .collect::<Vec<_>>();
        assert_eq!(items.len(), 8);
        assert_eq!(items[0].state, ProgressState::Started);
        assert_eq!(items[1].state, ProgressState::Completed);
        for stage in [OperationStage::Download, OperationStage::Verify] {
            let totals = events
                .iter()
                .filter(|event| event.scope == ProgressScope::Stage && event.stage == stage)
                .collect::<Vec<_>>();
            assert_eq!(
                totals
                    .iter()
                    .map(|event| event.completed)
                    .collect::<Vec<_>>(),
                [100, 200]
            );
            assert!(
                totals
                    .iter()
                    .all(|event| event.state == ProgressState::Advanced
                        && event.path.is_none()
                        && event.total == Some(200))
            );
        }
    }

    #[tokio::test]
    async fn cache_inventory_reports_size_checked_content_addresses() {
        let cache = tempfile::tempdir().unwrap();
        let first = spec("00000000000000000000000000000000", 4);
        let second = spec("11111111111111111111111111111111", 8);
        tokio::fs::write(cache.path().join(&first.cache_name), b"four")
            .await
            .unwrap();
        tokio::fs::write(cache.path().join(&second.cache_name), b"short")
            .await
            .unwrap();

        let summary = inspect_artifact_cache(&[first, second], cache.path())
            .await
            .unwrap();

        assert_eq!(summary.cached_files, 1);
        assert_eq!(summary.total_files, 2);
        assert_eq!(summary.cached_bytes, 4);
        assert_eq!(summary.total_bytes, 12);
        assert!(!summary.is_complete());
    }

    #[test]
    fn completion_labels_preserve_cache_reuse() {
        assert_eq!(completed_download_label(3, 3), "Reused 3 cached map files");
        assert_eq!(
            completed_download_label(3, 2),
            "Downloaded 1 map file; reused 2 cached map files"
        );
        assert_eq!(
            completed_verification_label(3, 2),
            "Verified 3 map files (2 cached)"
        );
    }
}

fn total_download_bytes(specs: &[DownloadSpec]) -> Result<u64, DownloadError> {
    specs.iter().try_fold(0_u64, |sum, spec| {
        sum.checked_add(spec.size)
            .ok_or(DownloadError::TotalSizeOverflow)
    })
}

async fn capture_downloads(
    capture: &SessionCapture,
    completed: &[(usize, DownloadProgress)],
    specs: &[DownloadSpec],
) -> Result<(), DownloadError> {
    crate::space::check_host_space(capture.root(), total_download_bytes(specs)?).await?;
    let mut captured = HashSet::new();
    for ((_, item), spec) in completed.iter().zip(specs) {
        if !captured.insert(&spec.cache_name) {
            continue;
        }
        capture
            .retain_file(
                &PathBuf::from("downloads").join(format!("{}.bin", spec.cache_name)),
                &item.path,
            )
            .await?;
    }
    Ok(())
}

async fn request_download(
    http: &reqwest::Client,
    authorizer: &dyn DownloadUrlAuthorizer,
    spec: &DownloadSpec,
    partial: &Path,
    capture: Option<&SessionCapture>,
    progress: &ProgressReporter,
    display_path: &str,
) -> Result<(reqwest::Response, u64, Md5, bool, Option<HttpExchange>), DownloadError> {
    let mut resumed_at = tokio::fs::metadata(partial)
        .await
        .map_or(0, |metadata| metadata.len());
    if resumed_at > spec.size {
        tokio::fs::remove_file(partial).await?;
        resumed_at = 0;
    }

    let mut hash = Md5::new();
    if resumed_at > 0 {
        hash_existing(partial, &mut hash).await?;
    }
    let candidates = std::iter::once(&spec.source)
        .chain(&spec.alternate_sources)
        .collect::<Vec<_>>();
    let mut selected = None;
    for (index, source) in candidates.iter().enumerate() {
        let host = download_host(source);
        tracing::info!(
            map = %spec.map_name,
            %host,
            attempt = index + 1,
            candidates = candidates.len(),
            "trying Garmin download host"
        );
        let source = if spec.requires_garmin_token {
            authorizer.authorize(source)?
        } else {
            (*source).clone()
        };
        let mut request = http.get(source).header(ACCEPT, "*/*");
        if resumed_at > 0 {
            request = request.header(RANGE, format!("bytes={resumed_at}-"));
        }
        let request = request.build()?;
        let exchange = begin_download_capture(capture, &request).await?;
        let response = match http.execute(request).await {
            Ok(response) => response,
            Err(error) => {
                if let Some(exchange) = &exchange {
                    exchange.error(&error.to_string()).await?;
                }
                tracing::warn!(
                    map = %spec.map_name,
                    %host,
                    attempt = index + 1,
                    candidates = candidates.len(),
                    %error,
                    "Garmin download host is unavailable"
                );
                if index + 1 < candidates.len() {
                    report_download_pivot(
                        progress,
                        &host,
                        candidates[index + 1],
                        display_path,
                        resumed_at,
                        spec.size,
                    );
                    continue;
                }
                return Err(error.into());
            }
        };
        capture_download_response_head(exchange.as_ref(), &response).await?;
        if !response.status().is_success() {
            let status = capture_error_response(response, exchange.as_ref()).await?;
            tracing::warn!(
                map = %spec.map_name,
                %host,
                attempt = index + 1,
                candidates = candidates.len(),
                %status,
                "Garmin download host rejected the request"
            );
            if index + 1 < candidates.len() {
                report_download_pivot(
                    progress,
                    &host,
                    candidates[index + 1],
                    display_path,
                    resumed_at,
                    spec.size,
                );
                continue;
            }
            return Err(DownloadError::HttpStatus(status));
        }
        if index > 0 {
            tracing::info!(
                map = %spec.map_name,
                %host,
                attempt = index + 1,
                "using fallback Garmin download host"
            );
        }
        selected = Some((response, exchange));
        break;
    }
    let (response, exchange) = selected.expect("download candidates are never empty");
    let append = resumed_at > 0
        && response.status() == reqwest::StatusCode::PARTIAL_CONTENT
        && content_range_starts_at(&response, resumed_at);
    if resumed_at > 0 && !append {
        resumed_at = 0;
        hash = Md5::new();
    }
    if let Some(length) = response.content_length()
        && length > spec.size.saturating_sub(resumed_at)
    {
        let error = DownloadError::Size {
            expected: spec.size,
            actual: resumed_at.saturating_add(length),
            path: partial.to_owned(),
        };
        if let Some(exchange) = &exchange {
            exchange.error(&error.to_string()).await?;
        }
        return Err(error);
    }
    Ok((response, resumed_at, hash, append, exchange))
}

fn report_download_pivot(
    progress: &ProgressReporter,
    failed_host: &str,
    next_source: &url::Url,
    display_path: &str,
    completed: u64,
    total: u64,
) {
    let next_host = download_host(next_source);
    progress.advanced_with_path(
        OperationStage::Download,
        format!("{failed_host} unavailable; trying {next_host}"),
        display_path,
        completed,
        Some(total),
    );
}

fn download_host(source: &url::Url) -> String {
    source
        .host_str()
        .map_or_else(|| source.as_str().to_owned(), str::to_owned)
}

async fn begin_download_capture(
    capture: Option<&SessionCapture>,
    request: &reqwest::Request,
) -> Result<Option<HttpExchange>, DownloadError> {
    let Some(capture) = capture else {
        return Ok(None);
    };
    let version = format!("{:?}", request.version());
    let exchange = capture
        .begin_http(
            "map-download",
            &HttpRequestHead {
                method: request.method().as_str(),
                url: request.url().as_str(),
                version: &version,
                headers: capture_headers(request.headers()),
            },
            request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .unwrap_or_default(),
        )
        .await?;
    Ok(Some(exchange))
}

async fn capture_download_response_head(
    exchange: Option<&HttpExchange>,
    response: &reqwest::Response,
) -> Result<(), DownloadError> {
    let Some(exchange) = exchange else {
        return Ok(());
    };
    let version = format!("{:?}", response.version());
    exchange
        .response_head(&HttpResponseHead {
            status: response.status().as_u16(),
            status_text: response.status().canonical_reason().unwrap_or_default(),
            version: &version,
            headers: capture_headers(response.headers()),
        })
        .await?;
    Ok(())
}

async fn capture_error_response(
    response: reqwest::Response,
    exchange: Option<&HttpExchange>,
) -> Result<reqwest::StatusCode, DownloadError> {
    let result = capture_error_response_body(response, exchange).await;
    if let (Err(error), Some(exchange)) = (&result, exchange) {
        exchange.error(&error.to_string()).await?;
    }
    result
}

async fn capture_error_response_body(
    response: reqwest::Response,
    exchange: Option<&HttpExchange>,
) -> Result<reqwest::StatusCode, DownloadError> {
    let status = response.status();
    let Some(exchange) = exchange else {
        return Ok(status);
    };
    let mut output = exchange.response_body_file().await?;
    let mut received = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        received = received
            .checked_add(u64::try_from(chunk.len()).map_err(|_| DownloadError::SizeOverflow)?)
            .ok_or(DownloadError::SizeOverflow)?;
        if received > MAX_ERROR_RESPONSE_BYTES {
            return Err(DownloadError::ErrorResponseTooLarge);
        }
        output.write_all(&chunk).await?;
    }
    output.flush().await?;
    output.sync_all().await?;
    exchange
        .complete_response_body(usize::try_from(received).map_err(|_| DownloadError::SizeOverflow)?)
        .await?;
    Ok(status)
}

struct TransferTarget<'a> {
    spec: &'a DownloadSpec,
    partial: &'a Path,
    progress: &'a ProgressReporter,
    label: &'a str,
    display_path: &'a str,
    exchange: Option<HttpExchange>,
}

async fn write_response(
    response: reqwest::Response,
    resumed_at: u64,
    hash: Md5,
    append: bool,
    target: TransferTarget<'_>,
) -> Result<(u64, Md5, Duration), DownloadError> {
    let exchange = target.exchange.clone();
    let partial = target.partial.to_owned();
    let result = write_response_body(response, resumed_at, hash, append, target).await;
    if let (Err(error), Some(exchange)) = (&result, exchange) {
        if resumed_at == 0 && !matches!(error, DownloadError::Capture(_)) {
            match tokio::fs::symlink_metadata(&partial).await {
                Ok(metadata) if metadata.file_type().is_file() => {
                    exchange
                        .copy_response_body(
                            &partial,
                            usize::try_from(metadata.len())
                                .map_err(|_| DownloadError::SizeOverflow)?,
                        )
                        .await?;
                }
                Ok(_) | Err(_) => {}
            }
        }
        exchange.error(&error.to_string()).await?;
    }
    result
}

async fn write_response_body(
    response: reqwest::Response,
    resumed_at: u64,
    mut hash: Md5,
    append: bool,
    target: TransferTarget<'_>,
) -> Result<(u64, Md5, Duration), DownloadError> {
    let mut output = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(target.partial)
        .await?;
    let mut captured_body = match (&target.exchange, resumed_at) {
        (Some(exchange), 1..) => Some(exchange.response_body_file().await?),
        (None, _) | (Some(_), 0) => None,
    };
    let started = Instant::now();
    let mut bytes = resumed_at;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if target.progress.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let chunk = chunk?;
        let next = bytes
            .checked_add(u64::try_from(chunk.len()).map_err(|_| DownloadError::SizeOverflow)?)
            .ok_or(DownloadError::SizeOverflow)?;
        if next > target.spec.size {
            target.progress.failed(
                OperationStage::Download,
                "Download exceeded the planned size",
            );
            return Err(DownloadError::Size {
                expected: target.spec.size,
                actual: next,
                path: target.partial.to_owned(),
            });
        }
        output.write_all(&chunk).await?;
        if let Some(captured_body) = &mut captured_body {
            captured_body.write_all(&chunk).await?;
        }
        hash.update(&chunk);
        bytes = next;
        target.progress.advanced_with_path(
            OperationStage::Download,
            target.label,
            target.display_path,
            bytes,
            Some(target.spec.size),
        );
    }
    output.flush().await?;
    output.sync_all().await?;
    if let Some(mut captured_body) = captured_body {
        captured_body.flush().await?;
        captured_body.sync_all().await?;
    }
    if let Some(exchange) = &target.exchange
        && resumed_at > 0
    {
        let response_bytes = usize::try_from(bytes.saturating_sub(resumed_at))
            .map_err(|_| DownloadError::SizeOverflow)?;
        exchange.complete_response_body(response_bytes).await?;
    }
    Ok((bytes, hash, started.elapsed()))
}

async fn capture_rejected_response(
    exchange: Option<&HttpExchange>,
    resumed_at: u64,
    partial: &Path,
    bytes: u64,
) -> Result<(), DownloadError> {
    if resumed_at == 0
        && let Some(exchange) = exchange
    {
        exchange
            .copy_response_body(
                partial,
                usize::try_from(bytes).map_err(|_| DownloadError::SizeOverflow)?,
            )
            .await?;
    }
    Ok(())
}

fn capture_headers(headers: &reqwest::header::HeaderMap) -> Vec<HttpHeader> {
    headers
        .iter()
        .map(|(name, value)| HttpHeader {
            name: name.as_str().to_owned(),
            value_bytes: value.as_bytes().to_vec(),
        })
        .collect()
}

async fn reject_unsafe_cache_entry(path: &Path) -> Result<(), DownloadError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(DownloadError::UnsafeCacheEntry(path.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn require_cache_directory(path: &Path) -> Result<(), DownloadError> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_dir() {
        return Err(DownloadError::UnsafeCacheEntry(path.to_owned()));
    }
    Ok(())
}

fn content_range_starts_at(response: &reqwest::Response, expected: u64) -> bool {
    response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, _)| start.parse::<u64>().ok())
        == Some(expected)
}

async fn cached_download(
    final_path: &Path,
    spec: &DownloadSpec,
    progress: &ProgressReporter,
) -> Result<Option<DownloadProgress>, std::io::Error> {
    if !final_path.is_file() || !verify_file(final_path, spec).await? {
        if tokio::fs::symlink_metadata(final_path)
            .await
            .is_ok_and(|metadata| metadata.file_type().is_file())
        {
            tokio::fs::remove_file(final_path).await?;
        }
        return Ok(None);
    }
    let mut permissions = tokio::fs::metadata(final_path).await?.permissions();
    if !permissions.readonly() {
        permissions.set_readonly(true);
        tokio::fs::set_permissions(final_path, permissions).await?;
    }
    progress.completed_with_path(
        OperationStage::Download,
        "Already cached",
        spec.destination.as_path().display().to_string(),
        spec.size,
        Some(spec.size),
    );
    progress.completed_with_path(
        OperationStage::Verify,
        "Cached MD5 verified",
        spec.destination.as_path().display().to_string(),
        spec.size,
        Some(spec.size),
    );
    Ok(Some(DownloadProgress {
        path: final_path.to_owned(),
        bytes: spec.size,
        elapsed: Duration::ZERO,
        resumed_at: spec.size,
        from_cache: true,
    }))
}

fn completed_download_label(total: usize, cached: usize) -> String {
    match (total.saturating_sub(cached), cached) {
        (0, cached) => format!("Reused {}", map_file_count(cached, true)),
        (downloaded, 0) => format!("Downloaded {}", map_file_count(downloaded, false)),
        (downloaded, cached) => {
            format!(
                "Downloaded {}; reused {}",
                map_file_count(downloaded, false),
                map_file_count(cached, true)
            )
        }
    }
}

fn map_file_count(count: usize, cached: bool) -> String {
    format!(
        "{count} {}map file{}",
        if cached { "cached " } else { "" },
        if count == 1 { "" } else { "s" }
    )
}

fn completed_verification_label(total: usize, cached: usize) -> String {
    if cached == 0 {
        format!("Verified {total} map files")
    } else {
        format!("Verified {total} map files ({cached} cached)")
    }
}

fn cache_path(cache: &Path, spec: &DownloadSpec) -> Result<PathBuf, DownloadError> {
    if spec.cache_name != spec.md5
        || spec.cache_name.len() != 32
        || !spec.cache_name.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DownloadError::UnsafeCacheEntry(
            cache.join(&spec.cache_name),
        ));
    }
    Ok(cache.join(&spec.cache_name))
}

async fn hash_existing(path: &Path, digest: &mut Md5) -> Result<(), std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(())
}

async fn verify_file(path: &Path, spec: &DownloadSpec) -> Result<bool, std::io::Error> {
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.len() != spec.size {
        return Ok(false);
    }
    let mut hash = Md5::new();
    hash_existing(path, &mut hash).await?;
    Ok(hex::encode(hash.finalize()).eq_ignore_ascii_case(&spec.md5))
}

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub path: PathBuf,
    pub bytes: u64,
    pub elapsed: Duration,
    pub resumed_at: u64,
    pub from_cache: bool,
}

impl DownloadProgress {
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "an approximate transfer rate is intentionally represented as f64"
    )]
    pub fn bytes_per_second(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.bytes as f64 / self.elapsed.as_secs_f64()
    }
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error(transparent)]
    Space(#[from] crate::space::SpaceError),
    #[error("map download cancelled")]
    Cancelled,
    #[error("combined download size exceeds the supported range")]
    TotalSizeOverflow,
    #[error("session capture failed: {0}")]
    Capture(#[from] CaptureError),
    #[error("download concurrency must be between 1 and 32, received {0}")]
    InvalidConcurrency(usize),
    #[error("download request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("download server returned HTTP {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("download server error response exceeded 16 MB")]
    ErrorResponseTooLarge,
    #[error("cache I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("downloaded size mismatch for {path}: expected {expected}, received {actual}")]
    Size {
        expected: u64,
        actual: u64,
        path: PathBuf,
    },
    #[error("downloaded checksum mismatch for {path}: expected {expected}, received {actual}")]
    Checksum {
        expected: String,
        actual: String,
        path: PathBuf,
    },
    #[error("download worker failed: {0}")]
    Task(tokio::task::JoinError),
    #[error("download concurrency controller closed unexpectedly: {0}")]
    Semaphore(#[from] tokio::sync::AcquireError),
    #[error("this deliverable requires a protected-download authorizer that is unavailable")]
    ProtectedDownloadAuthorizationUnavailable,
    #[error("protected download authorization failed: {0}")]
    ProtectedDownloadAuthorization(String),
    #[error("download byte count exceeds u64")]
    SizeOverflow,
    #[error("refusing symbolic link in download cache: {0}")]
    UnsafeCacheEntry(PathBuf),
}
