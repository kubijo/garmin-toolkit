//! Host-owned discovery and bounded caching for `OpenFreeMap` vector tiles.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::StreamExt as _;
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use url::Url;
use walkdir::WalkDir;

const TILE_JSON_URL: &str = "https://tiles.openfreemap.org/planet/latest";
const PROVIDER_HOST: &str = "tiles.openfreemap.org";
const MAX_ZOOM: u8 = 14;
const TILE_JSON_LIMIT: usize = 1024 * 1024;
const TILE_LIMIT: usize = 2 * 1024 * 1024;
const CACHE_LIMIT: u64 = 256 * 1024 * 1024;
const FRESH_FOR: Duration = Duration::from_hours(168);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = concat!("garmin-toolkit/", env!("CARGO_PKG_VERSION"));

/// One validated XYZ vector-tile coordinate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TileId {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    /// Validate an XYZ coordinate supported by the configured provider.
    ///
    /// # Errors
    /// [`Error::InvalidTile`] when zoom or grid coordinates are out of range.
    pub fn validate(self) -> Result<Self, Error> {
        if self.zoom > MAX_ZOOM {
            return Err(Error::InvalidTile);
        }
        let edge = 1_u32
            .checked_shl(u32::from(self.zoom))
            .ok_or(Error::InvalidTile)?;
        if self.x >= edge || self.y >= edge {
            return Err(Error::InvalidTile);
        }
        Ok(self)
    }
}

/// Cached payload and a stable response validator.
#[derive(Debug)]
pub struct Tile {
    pub bytes: Vec<u8>,
    pub etag: String,
    pub max_age_seconds: u64,
}

/// Cloneable, concurrency-deduplicating tile service.
#[derive(Clone)]
pub struct Service {
    client: reqwest::Client,
    cache_root: Arc<Path>,
    tile_json_url: Arc<Url>,
    provider_host: Arc<str>,
    https_only: bool,
    template: Arc<Mutex<Option<String>>>,
    in_flight: Arc<Mutex<HashMap<TileId, Arc<Mutex<()>>>>>,
}

impl Service {
    /// Create the service without performing network I/O.
    ///
    /// # Errors
    /// An error if the HTTP client cannot be constructed.
    pub fn new(cache_root: impl Into<PathBuf>) -> Result<Self, Error> {
        Self::with_provider(
            cache_root.into(),
            Url::parse(TILE_JSON_URL)?,
            PROVIDER_HOST.into(),
            true,
        )
    }

    fn with_provider(
        cache_root: PathBuf,
        tile_json_url: Url,
        provider_host: Arc<str>,
        https_only: bool,
    ) -> Result<Self, Error> {
        if (https_only && tile_json_url.scheme() != "https")
            || (!https_only && !matches!(tile_json_url.scheme(), "http" | "https"))
            || tile_json_url.host_str() != Some(provider_host.as_ref())
        {
            return Err(Error::InvalidTemplate);
        }
        let redirect_host = Arc::clone(&provider_host);
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                    let url = attempt.url();
                    let scheme_allowed = if https_only {
                        url.scheme() == "https"
                    } else {
                        matches!(url.scheme(), "http" | "https")
                    };
                    if scheme_allowed && url.host_str() == Some(redirect_host.as_ref()) {
                        attempt.follow()
                    } else {
                        attempt.stop()
                    }
                }))
                .build()?,
            cache_root: Arc::from(cache_root),
            tile_json_url: Arc::new(tile_json_url),
            provider_host,
            https_only,
            template: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Read a fresh cached tile or retrieve and atomically cache it.
    ///
    /// Concurrent misses for the same tile are deduplicated at the service boundary. Hosts also
    /// bound total network concurrency independently.
    ///
    /// # Errors
    /// A validation, provider, size-limit, or cache I/O error.
    pub async fn tile(&self, tile_id: TileId) -> Result<Tile, Error> {
        let tile_id = tile_id.validate()?;
        if let Some(tile) = self.read_fresh(tile_id).await? {
            return Ok(tile);
        }
        let tile_gate = {
            let mut in_flight = self.in_flight.lock().await;
            Arc::clone(
                in_flight
                    .entry(tile_id)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let guard = tile_gate.lock().await;
        let result = self.fetch_and_cache(tile_id).await;
        drop(guard);
        self.release_gate(tile_id, &tile_gate).await;
        result
    }

    async fn fetch_and_cache(&self, tile_id: TileId) -> Result<Tile, Error> {
        if let Some(tile) = self.read_fresh(tile_id).await? {
            return Ok(tile);
        }
        let cached = self.read_cached(tile_id).await?;
        let template = match self.template().await {
            Ok(template) => template,
            Err(error) => return self.cached_or_error(tile_id, cached, error).await,
        };
        let url = tile_url(&template, tile_id, &self.provider_host, self.https_only)?;
        let mut request = self.client.get(url);
        if let Some(etag) = cached
            .as_ref()
            .and_then(|cached| cached.metadata.etag.as_deref())
        {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                return self
                    .cached_or_error(tile_id, cached, Error::Http(error))
                    .await;
            }
        };
        if response.status() == StatusCode::NOT_MODIFIED {
            let Some(mut cached) = cached else {
                return Err(Error::ProviderStatus(StatusCode::NOT_MODIFIED));
            };
            cached.metadata.fetched_at = now_seconds()?;
            cached.metadata.accessed_at = cached.metadata.fetched_at;
            if let Some(fresh_for_seconds) = response_max_age(response.headers()) {
                cached.metadata.fresh_for_seconds = fresh_for_seconds.min(FRESH_FOR.as_secs());
            }
            self.write_metadata(tile_id, &cached.metadata).await?;
            let max_age_seconds = cached.metadata.fresh_for_seconds;
            return Ok(cached_tile(cached, max_age_seconds));
        }
        if !response.status().is_success() {
            let error = Error::ProviderStatus(response.status());
            return self.cached_or_error(tile_id, cached, error).await;
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if content_type.starts_with("text/") || content_type.starts_with("application/json") {
            return self
                .cached_or_error(tile_id, cached, Error::MalformedTile)
                .await;
        }
        let fresh_for_seconds = response_max_age(response.headers())
            .unwrap_or(FRESH_FOR.as_secs())
            .min(FRESH_FOR.as_secs());
        let upstream_etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let bytes = match read_limited(response, TILE_LIMIT).await {
            Ok(bytes) => bytes,
            Err(error) => return self.cached_or_error(tile_id, cached, error).await,
        };
        if !valid_tile_payload(&bytes) {
            return self
                .cached_or_error(tile_id, cached, Error::MalformedTile)
                .await;
        }
        let now = now_seconds()?;
        let metadata = Metadata {
            fetched_at: now,
            accessed_at: now,
            fresh_for_seconds,
            etag: upstream_etag,
        };
        self.write(tile_id, &bytes, &metadata).await?;
        self.prune().await?;
        Ok(Tile {
            etag: response_etag(&bytes),
            bytes,
            max_age_seconds: fresh_for_seconds,
        })
    }

    async fn cached_or_error(
        &self,
        tile_id: TileId,
        cached: Option<Cached>,
        error: Error,
    ) -> Result<Tile, Error> {
        let Some(mut cached) = cached else {
            return Err(error);
        };
        if let Ok(accessed_at) = now_seconds() {
            cached.metadata.accessed_at = accessed_at;
            let _ignored = self.write_metadata(tile_id, &cached.metadata).await;
        }
        Ok(cached_tile(cached, 0))
    }

    async fn release_gate(&self, tile_id: TileId, tile_gate: &Arc<Mutex<()>>) {
        let mut in_flight = self.in_flight.lock().await;
        if in_flight
            .get(&tile_id)
            .is_some_and(|current| Arc::ptr_eq(current, tile_gate))
            && Arc::strong_count(tile_gate) == 2
        {
            in_flight.remove(&tile_id);
        }
    }

    async fn template(&self) -> Result<String, Error> {
        let mut template = self.template.lock().await;
        if let Some(value) = template.as_ref() {
            return Ok(value.clone());
        }
        let response = self
            .client
            .get(Url::clone(self.tile_json_url.as_ref()))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::ProviderStatus(response.status()));
        }
        let bytes = read_limited(response, TILE_JSON_LIMIT).await?;
        let tile_json = serde_json::from_slice::<TileJson>(&bytes)?;
        let value = tile_json
            .tiles
            .into_iter()
            .next()
            .ok_or(Error::MissingTemplate)?;
        validate_template(&value, &self.provider_host, self.https_only)?;
        *template = Some(value.clone());
        Ok(value)
    }

    async fn read_fresh(&self, tile_id: TileId) -> Result<Option<Tile>, Error> {
        let Some(mut cached) = self.read_cached(tile_id).await? else {
            return Ok(None);
        };
        let age = now_seconds()?.saturating_sub(cached.metadata.fetched_at);
        if cached.metadata.fresh_for_seconds == 0 || age >= cached.metadata.fresh_for_seconds {
            return Ok(None);
        }
        cached.metadata.accessed_at = now_seconds()?;
        let _ignored = self.write_metadata(tile_id, &cached.metadata).await;
        let max_age_seconds = cached.metadata.fresh_for_seconds.saturating_sub(age);
        Ok(Some(cached_tile(cached, max_age_seconds)))
    }

    async fn read_cached(&self, tile_id: TileId) -> Result<Option<Cached>, Error> {
        let paths = CachePaths::new(self.cache_root.as_ref(), tile_id);
        let bytes = match tokio::fs::read(&paths.tile).await {
            Ok(bytes) if bytes.len() <= TILE_LIMIT && valid_tile_payload(&bytes) => bytes,
            Ok(_) => {
                remove_cache_entry(&paths).await?;
                return Ok(None);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = match tokio::fs::read(&paths.metadata).await {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Metadata::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(Cached { bytes, metadata }))
    }

    async fn write(&self, tile_id: TileId, bytes: &[u8], metadata: &Metadata) -> Result<(), Error> {
        let paths = CachePaths::new(self.cache_root.as_ref(), tile_id);
        let parent = paths.tile.parent().ok_or(Error::InvalidCachePath)?;
        tokio::fs::create_dir_all(parent).await?;
        atomic_write(&paths.tile, bytes).await?;
        self.write_metadata(tile_id, metadata).await
    }

    async fn write_metadata(&self, tile_id: TileId, metadata: &Metadata) -> Result<(), Error> {
        let path = CachePaths::new(self.cache_root.as_ref(), tile_id).metadata;
        let parent = path.parent().ok_or(Error::InvalidCachePath)?;
        tokio::fs::create_dir_all(parent).await?;
        atomic_write(&path, &serde_json::to_vec(metadata)?).await
    }

    async fn prune(&self) -> Result<(), Error> {
        let cache_root = Arc::clone(&self.cache_root);
        tokio::task::spawn_blocking(move || prune_cache(cache_root.as_ref(), CACHE_LIMIT))
            .await??;
        Ok(())
    }

    #[cfg(test)]
    fn prune_to(&self, limit: u64) -> Result<(), Error> {
        prune_cache(self.cache_root.as_ref(), limit)
    }
}

fn prune_cache(cache_root: &Path, limit: u64) -> Result<(), Error> {
    let mut entries = WalkDir::new(cache_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let path = entry.into_path();
            (metadata.is_file() && path.extension().is_some_and(|value| value == "pbf")).then(
                || {
                    let metadata_path = path.with_extension("json");
                    let metadata_size = fs::metadata(&metadata_path).map_or(0, |value| value.len());
                    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
                    let accessed_at = fs::read(&metadata_path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Metadata>(&bytes).ok())
                        .map_or(modified, |value| {
                            UNIX_EPOCH
                                + Duration::from_secs(value.accessed_at.max(value.fetched_at))
                        });
                    (
                        path,
                        metadata_path,
                        metadata.len() + metadata_size,
                        accessed_at,
                    )
                },
            )
        })
        .collect::<Vec<_>>();
    let mut total = entries.iter().map(|(_, _, size, _)| size).sum::<u64>();
    if total <= limit {
        return Ok(());
    }
    entries.sort_by_key(|(_, _, _, accessed_at)| *accessed_at);
    for (path, metadata_path, size, _) in entries {
        if total <= limit {
            break;
        }
        match fs::remove_file(path) {
            Ok(()) => total = total.saturating_sub(size),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        match fs::remove_file(metadata_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct TileJson {
    tiles: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    fetched_at: u64,
    #[serde(default)]
    accessed_at: u64,
    #[serde(default = "default_fresh_for_seconds")]
    fresh_for_seconds: u64,
    etag: Option<String>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            fetched_at: 0,
            accessed_at: 0,
            fresh_for_seconds: default_fresh_for_seconds(),
            etag: None,
        }
    }
}

struct Cached {
    bytes: Vec<u8>,
    metadata: Metadata,
}

fn cached_tile(cached: Cached, max_age_seconds: u64) -> Tile {
    Tile {
        etag: response_etag(&cached.bytes),
        bytes: cached.bytes,
        max_age_seconds,
    }
}

const fn default_fresh_for_seconds() -> u64 {
    FRESH_FOR.as_secs()
}

fn response_max_age(headers: &header::HeaderMap) -> Option<u64> {
    headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .find_map(|directive| {
            let (name, value) = directive.split_once('=')?;
            if !name.trim().eq_ignore_ascii_case("max-age") {
                return None;
            }
            value.trim().trim_matches('"').parse::<u64>().ok()
        })
}

fn valid_tile_payload(bytes: &[u8]) -> bool {
    bytes.is_empty() || valid_mvt(bytes)
}

fn valid_mvt(bytes: &[u8]) -> bool {
    let mut input = bytes;
    let mut has_layer = false;
    while !input.is_empty() {
        let Some(key) = take_varint(&mut input) else {
            return false;
        };
        let field = key >> 3;
        let wire = key & 0b111;
        if field == 0 {
            return false;
        }
        if field == 3 && wire == 2 {
            let Some(layer) = take_length_delimited(&mut input) else {
                return false;
            };
            if !valid_mvt_layer(layer) {
                return false;
            }
            has_layer = true;
        } else if !skip_protobuf_value(&mut input, wire) {
            return false;
        }
    }
    has_layer
}

fn valid_mvt_layer(mut input: &[u8]) -> bool {
    let mut has_name = false;
    let mut has_version = false;
    while !input.is_empty() {
        let Some(key) = take_varint(&mut input) else {
            return false;
        };
        let field = key >> 3;
        let wire = key & 0b111;
        if field == 0 {
            return false;
        }
        match (field, wire) {
            (1, 2) => {
                let Some(name) = take_length_delimited(&mut input) else {
                    return false;
                };
                has_name = !name.is_empty() && std::str::from_utf8(name).is_ok();
            }
            (15, 0) => {
                let Some(version) = take_varint(&mut input) else {
                    return false;
                };
                has_version = matches!(version, 1 | 2);
            }
            _ if !skip_protobuf_value(&mut input, wire) => return false,
            _ => {}
        }
    }
    has_name && has_version
}

fn take_varint(input: &mut &[u8]) -> Option<u64> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let (&byte, rest) = input.split_first()?;
        *input = rest;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn take_length_delimited<'a>(input: &mut &'a [u8]) -> Option<&'a [u8]> {
    let length = usize::try_from(take_varint(input)?).ok()?;
    let (value, rest) = input.split_at_checked(length)?;
    *input = rest;
    Some(value)
}

fn skip_protobuf_value(input: &mut &[u8], wire: u64) -> bool {
    match wire {
        0 => take_varint(input).is_some(),
        1 => take_bytes(input, 8),
        2 => take_length_delimited(input).is_some(),
        5 => take_bytes(input, 4),
        _ => false,
    }
}

fn take_bytes(input: &mut &[u8], length: usize) -> bool {
    let Some((_, rest)) = input.split_at_checked(length) else {
        return false;
    };
    *input = rest;
    true
}

struct CachePaths {
    tile: PathBuf,
    metadata: PathBuf,
}

impl CachePaths {
    fn new(root: &Path, tile_id: TileId) -> Self {
        let directory = root
            .join(tile_id.zoom.to_string())
            .join(tile_id.x.to_string());
        let stem = tile_id.y.to_string();
        Self {
            tile: directory.join(format!("{stem}.pbf")),
            metadata: directory.join(format!("{stem}.json")),
        }
    }
}

async fn read_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, Error> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(limit).unwrap_or(u64::MAX))
    {
        return Err(Error::ResponseTooLarge(limit));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(Error::ResponseTooLarge(limit));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn validate_template(template: &str, provider_host: &str, https_only: bool) -> Result<(), Error> {
    let parsed = Url::parse(
        &template
            .replace("{z}", "0")
            .replace("{x}", "0")
            .replace("{y}", "0"),
    )?;
    if (https_only && parsed.scheme() != "https")
        || (!https_only && !matches!(parsed.scheme(), "http" | "https"))
        || parsed.host_str() != Some(provider_host)
        || !template.contains("{z}")
        || !template.contains("{x}")
        || !template.contains("{y}")
    {
        return Err(Error::InvalidTemplate);
    }
    Ok(())
}

fn tile_url(
    template: &str,
    tile_id: TileId,
    provider_host: &str,
    https_only: bool,
) -> Result<Url, Error> {
    let value = template
        .replace("{z}", &tile_id.zoom.to_string())
        .replace("{x}", &tile_id.x.to_string())
        .replace("{y}", &tile_id.y.to_string());
    let url = Url::parse(&value)?;
    if (https_only && url.scheme() != "https")
        || (!https_only && !matches!(url.scheme(), "http" | "https"))
        || url.host_str() != Some(provider_host)
    {
        return Err(Error::InvalidTemplate);
    }
    Ok(url)
}

async fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::InvalidCachePath)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    tokio::fs::write(temporary.path(), bytes).await?;
    temporary
        .persist(path)
        .map_err(|error| Error::Io(error.error))?;
    Ok(())
}

async fn remove_cache_entry(paths: &CachePaths) -> Result<(), Error> {
    for path in [&paths.tile, &paths.metadata] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn now_seconds() -> Result<u64, Error> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn response_etag(bytes: &[u8]) -> String {
    format!("\"{}\"", blake3::hash(bytes).to_hex())
}

/// Tile discovery, provider, or cache failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid map tile coordinate")]
    InvalidTile,
    #[error("OpenFreeMap TileJSON did not provide a tile template")]
    MissingTemplate,
    #[error("OpenFreeMap returned an invalid tile template")]
    InvalidTemplate,
    #[error("OpenFreeMap returned a malformed vector tile")]
    MalformedTile,
    #[error("OpenFreeMap returned HTTP {0}")]
    ProviderStatus(StatusCode),
    #[error("map response exceeded its {0}-byte limit")]
    ResponseTooLarge(usize),
    #[error("map cache path has no parent")]
    InvalidCachePath,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Clock(#[from] std::time::SystemTimeError),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use axum::{
        Json, Router,
        body::Body,
        extract::State,
        http::{HeaderMap, HeaderValue, Response, header},
        routing::get,
    };
    use serde_json::json;

    use super::{
        Error, Metadata, Service, TileId, response_max_age, tile_url, valid_mvt, validate_template,
    };

    const VECTOR_CONTENT_TYPE: &str = "application/vnd.mapbox-vector-tile";

    fn vector_tile() -> Vec<u8> {
        vec![0x1a, 0x05, 0x0a, 0x01, b'x', 0x78, 0x02]
    }

    #[derive(Clone)]
    struct Fixture {
        origin: String,
        tile_requests: Arc<AtomicUsize>,
        tile_bytes: Arc<[u8]>,
    }

    struct RunningFixture {
        service: Service,
        tile_requests: Arc<AtomicUsize>,
        server: tokio::task::JoinHandle<()>,
    }

    async fn tile_json(State(fixture): State<Fixture>) -> Json<serde_json::Value> {
        Json(json!({
            "tiles": [format!("{}/tiles/{{z}}/{{x}}/{{y}}.pbf", fixture.origin)]
        }))
    }

    async fn fixture_tile(State(fixture): State<Fixture>) -> Response<Body> {
        fixture.tile_requests.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        Response::builder()
            .header(header::CONTENT_TYPE, VECTOR_CONTENT_TYPE)
            .body(Body::from(fixture.tile_bytes.as_ref().to_vec()))
            .unwrap()
    }

    async fn fixture(
        cache_root: PathBuf,
        tile_bytes: Vec<u8>,
    ) -> Result<RunningFixture, Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let tile_requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/tilejson", get(tile_json))
            .route("/tiles/{zoom}/{x}/{file}", get(fixture_tile))
            .with_state(Fixture {
                origin: origin.clone(),
                tile_requests: Arc::clone(&tile_requests),
                tile_bytes: Arc::from(tile_bytes),
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let url = url::Url::parse(&format!("{origin}/tilejson"))?;
        let host: Arc<str> = url.host_str().unwrap().into();
        let service = Service::with_provider(cache_root, url, host, false)?;
        Ok(RunningFixture {
            service,
            tile_requests,
            server,
        })
    }

    #[test]
    fn rejects_out_of_grid_coordinates() {
        assert!(
            TileId {
                zoom: 2,
                x: 4,
                y: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            TileId {
                zoom: 15,
                x: 0,
                y: 0
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn template_is_https_and_provider_scoped() {
        let template = "https://tiles.openfreemap.org/planet/v1/{z}/{x}/{y}.pbf";
        validate_template(template, super::PROVIDER_HOST, true).unwrap();
        let url = tile_url(
            template,
            TileId {
                zoom: 7,
                x: 64,
                y: 42,
            },
            super::PROVIDER_HOST,
            true,
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://tiles.openfreemap.org/planet/v1/7/64/42.pbf"
        );
        assert!(
            validate_template(
                "http://tiles.openfreemap.org/{z}/{x}/{y}.pbf",
                super::PROVIDER_HOST,
                true,
            )
            .is_err()
        );
        assert!(
            validate_template(
                "https://example.com/{z}/{x}/{y}.pbf",
                super::PROVIDER_HOST,
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_structurally_malformed_vector_tiles() {
        assert!(valid_mvt(&vector_tile()));
        assert!(!valid_mvt(b"not a vector tile"));
        assert!(!valid_mvt(&[0x1a, 0xff]));
    }

    #[test]
    fn reads_case_insensitive_http_freshness() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, Max-Age=120, immutable"),
        );

        assert_eq!(response_max_age(&headers), Some(120));
    }

    #[tokio::test]
    async fn concurrent_requests_are_deduplicated_and_then_hit_disk() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = vector_tile();
        let RunningFixture {
            service,
            tile_requests,
            server,
        } = fixture(directory.path().into(), bytes.clone())
            .await
            .unwrap();
        let tile_id = TileId {
            zoom: 2,
            x: 1,
            y: 1,
        };

        let (first, second, third) = tokio::join!(
            service.tile(tile_id),
            service.tile(tile_id),
            service.tile(tile_id),
        );
        assert_eq!(first.unwrap().bytes, bytes);
        assert_eq!(second.unwrap().bytes, vector_tile());
        assert_eq!(third.unwrap().bytes, vector_tile());
        assert_eq!(tile_requests.load(Ordering::SeqCst), 1);
        assert!(service.in_flight.lock().await.is_empty());

        server.abort();
        assert_eq!(service.tile(tile_id).await.unwrap().bytes, vector_tile());
    }

    #[tokio::test]
    async fn oversized_provider_responses_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let RunningFixture {
            service, server, ..
        } = fixture(directory.path().into(), vec![0; super::TILE_LIMIT + 1])
            .await
            .unwrap();

        let error = service
            .tile(TileId {
                zoom: 0,
                x: 0,
                y: 0,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, Error::ResponseTooLarge(_)));
        server.abort();
    }

    #[tokio::test]
    async fn malformed_provider_tiles_are_rejected_before_caching() {
        let directory = tempfile::tempdir().unwrap();
        let RunningFixture {
            service, server, ..
        } = fixture(directory.path().into(), b"not an mvt".to_vec())
            .await
            .unwrap();
        let tile_id = TileId {
            zoom: 0,
            x: 0,
            y: 0,
        };

        let error = service.tile(tile_id).await.unwrap_err();
        assert!(matches!(error, Error::MalformedTile));
        assert!(
            !super::CachePaths::new(directory.path(), tile_id)
                .tile
                .exists()
        );
        server.abort();
    }

    #[tokio::test]
    async fn empty_provider_tiles_are_cached_as_valid_empty_space() {
        let directory = tempfile::tempdir().unwrap();
        let RunningFixture {
            service,
            tile_requests,
            server,
        } = fixture(directory.path().into(), Vec::new()).await.unwrap();
        let tile_id = TileId {
            zoom: 0,
            x: 0,
            y: 0,
        };

        assert!(service.tile(tile_id).await.unwrap().bytes.is_empty());
        assert!(service.tile(tile_id).await.unwrap().bytes.is_empty());
        assert_eq!(tile_requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn stale_cache_survives_provider_failure_but_an_offline_miss_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let RunningFixture {
            service, server, ..
        } = fixture(directory.path().into(), vector_tile())
            .await
            .unwrap();
        let cached_id = TileId {
            zoom: 1,
            x: 0,
            y: 0,
        };
        let bytes = vector_tile();
        service
            .write(
                cached_id,
                &bytes,
                &Metadata {
                    fetched_at: 0,
                    accessed_at: 0,
                    fresh_for_seconds: 1,
                    etag: None,
                },
            )
            .await
            .unwrap();
        server.abort();
        let _cancelled = server.await;

        let stale = service.tile(cached_id).await.unwrap();
        assert_eq!(stale.bytes, bytes);
        assert_eq!(stale.max_age_seconds, 0);
        assert!(
            service
                .tile(TileId {
                    zoom: 1,
                    x: 1,
                    y: 0,
                })
                .await
                .is_err()
        );
    }

    #[test]
    fn pruning_removes_the_least_recently_used_tile_and_its_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let service = Service::new(directory.path()).unwrap();
        for (tile_id, accessed_at) in [
            (
                TileId {
                    zoom: 1,
                    x: 0,
                    y: 0,
                },
                1,
            ),
            (
                TileId {
                    zoom: 1,
                    x: 0,
                    y: 1,
                },
                2,
            ),
        ] {
            let paths = super::CachePaths::new(directory.path(), tile_id);
            std::fs::create_dir_all(paths.tile.parent().unwrap()).unwrap();
            std::fs::write(&paths.tile, [0; 8]).unwrap();
            std::fs::write(
                &paths.metadata,
                serde_json::to_vec(&Metadata {
                    fetched_at: accessed_at,
                    accessed_at,
                    fresh_for_seconds: super::FRESH_FOR.as_secs(),
                    etag: None,
                })
                .unwrap(),
            )
            .unwrap();
        }

        service.prune_to(128).unwrap();

        let oldest = super::CachePaths::new(
            directory.path(),
            TileId {
                zoom: 1,
                x: 0,
                y: 0,
            },
        );
        let newest = super::CachePaths::new(
            directory.path(),
            TileId {
                zoom: 1,
                x: 0,
                y: 1,
            },
        );
        assert!(!oldest.tile.exists());
        assert!(!oldest.metadata.exists());
        assert!(newest.tile.exists());
        assert!(newest.metadata.exists());
    }

    #[tokio::test]
    async fn fresh_cache_is_available_without_provider_access() {
        let directory = tempfile::tempdir().unwrap();
        let service = Service::new(directory.path()).unwrap();
        let tile_id = TileId {
            zoom: 0,
            x: 0,
            y: 0,
        };
        let paths = super::CachePaths::new(directory.path(), tile_id);
        tokio::fs::create_dir_all(paths.tile.parent().unwrap())
            .await
            .unwrap();
        let bytes = vector_tile();
        tokio::fs::write(&paths.tile, &bytes).await.unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        tokio::fs::write(
            paths.metadata,
            serde_json::to_vec(&Metadata {
                fetched_at: now,
                accessed_at: now,
                fresh_for_seconds: super::FRESH_FOR.as_secs(),
                etag: Some("upstream".to_owned()),
            })
            .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(service.tile(tile_id).await.unwrap().bytes, bytes);
    }

    #[tokio::test]
    async fn malformed_disk_entries_are_removed_before_refetch() {
        let directory = tempfile::tempdir().unwrap();
        let RunningFixture {
            service, server, ..
        } = fixture(directory.path().into(), vector_tile())
            .await
            .unwrap();
        let tile_id = TileId {
            zoom: 0,
            x: 0,
            y: 0,
        };
        let paths = super::CachePaths::new(directory.path(), tile_id);
        tokio::fs::create_dir_all(paths.tile.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&paths.tile, b"truncated").await.unwrap();
        tokio::fs::write(&paths.metadata, b"not metadata")
            .await
            .unwrap();

        assert_eq!(service.tile(tile_id).await.unwrap().bytes, vector_tile());
        assert!(paths.tile.exists());
        assert!(paths.metadata.exists());
        server.abort();
    }
}
