//! Lossless, fail-closed session evidence capture.

use har::v1_2 as har12;
use har::{Har, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::AsyncWriteExt;

mod host;

#[derive(Debug)]
struct CaptureInner {
    root: PathBuf,
    events: Mutex<File>,
    failure: Mutex<Option<String>>,
    har_entries: Mutex<BTreeMap<u64, CaptureHarEntry>>,
    har_writer: tokio::sync::Mutex<()>,
    har_revision: AtomicU64,
    sequence: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct SessionCapture(Arc<CaptureInner>);

#[derive(Debug, Clone)]
pub struct HttpExchange {
    directory: PathBuf,
    sequence: u64,
    started: Instant,
    capture: SessionCapture,
}

#[derive(Debug, Serialize)]
pub struct HttpRequestHead<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub version: &'a str,
    pub headers: Vec<HttpHeader>,
}

#[derive(Debug, Serialize)]
pub struct HttpResponseHead<'a> {
    pub status: u16,
    pub status_text: &'a str,
    pub version: &'a str,
    pub headers: Vec<HttpHeader>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HttpHeader {
    pub name: String,
    pub value_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct CaptureHarEntry {
    entry: har12::Entries,
    evidence: HttpEvidenceEntry,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct HttpEvidenceEntry {
    sequence: u64,
    state: String,
    request_head: String,
    request_body: String,
    response_head: String,
    response_body: String,
}

impl SessionCapture {
    /// Create a private capture directory without replacing existing data.
    /// Missing parent directories are created with private permissions.
    /// # Errors
    /// Existing target or file-creation failure.
    pub fn create(root: &Path) -> Result<Self, CaptureError> {
        if let Some(parent) = root
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let parent_builder = host::private_directory_builder(true);
            parent_builder
                .create(parent)
                .map_err(|source| CaptureError::Create {
                    path: parent.to_owned(),
                    source,
                })?;
        }
        let builder = host::private_directory_builder(false);
        builder
            .create(root)
            .map_err(|source| CaptureError::Create {
                path: root.to_owned(),
                source,
            })?;
        let events_path = root.join("events.jsonl");
        let events = private_file(&events_path)?;
        let http_builder = host::private_directory_builder(false);
        http_builder.create(root.join("http"))?;
        let capture = Self(Arc::new(CaptureInner {
            root: root.to_owned(),
            events: Mutex::new(events),
            failure: Mutex::new(None),
            har_entries: Mutex::new(BTreeMap::new()),
            har_writer: tokio::sync::Mutex::new(()),
            har_revision: AtomicU64::new(1),
            sequence: AtomicU64::new(1),
        }));
        let archive = har_archive(Vec::new());
        let mut har = private_file(&root.join("http.har"))?;
        har.write_all(har::to_json(&archive)?.as_bytes())?;
        har.write_all(b"\n")?;
        har.flush()?;
        har.sync_all()?;
        let mut evidence = private_file(&root.join("http/exchanges.json"))?;
        serde_json::to_writer_pretty(&mut evidence, &Vec::<HttpEvidenceEntry>::new())?;
        evidence.write_all(b"\n")?;
        evidence.flush()?;
        evidence.sync_all()?;
        Ok(capture)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.0.root
    }

    /// Open the session log without replacing an existing file.
    /// # Errors
    /// Log-creation failure.
    pub fn create_log(&self) -> Result<File, CaptureError> {
        self.ensure_healthy()?;
        private_file(&self.0.root.join("garmin-cli.log"))
    }

    /// Resolve a safe capture-relative artifact path.
    /// # Errors
    /// Prior capture failure or unsafe path.
    pub fn artifact_path(&self, relative: &Path) -> Result<PathBuf, CaptureError> {
        self.ensure_healthy()?;
        self.resolve(relative)
    }

    /// Create a private capture-relative directory.
    /// # Errors
    /// Prior capture failure, unsafe path, occupied target, or I/O failure.
    pub async fn create_directory(&self, relative: &Path) -> Result<PathBuf, CaptureError> {
        let path = self.artifact_path(relative)?;
        create_private_parents(&self.0.root, &path).await?;
        host::create_private_directory(&path)?;
        sync_parent_directory(&path).await?;
        Ok(path)
    }

    /// Create a private capture-relative file without replacing an artifact.
    /// # Errors
    /// Prior capture failure, unsafe path, occupied target, or I/O failure.
    pub async fn create_file(&self, relative: &Path) -> Result<tokio::fs::File, CaptureError> {
        let path = self.artifact_path(relative)?;
        create_private_parents(&self.0.root, &path).await?;
        Ok(host::private_async_file_options().open(path).await?)
    }

    /// Persist one named artifact without overwriting an earlier artifact.
    /// # Errors
    /// Unsafe path or unsynced write.
    pub async fn write_bytes(&self, relative: &Path, bytes: &[u8]) -> Result<(), CaptureError> {
        let mut file = self.create_file(relative).await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }

    /// Persist a pretty-printed structured artifact.
    /// # Errors
    /// Serialization or write failure.
    pub async fn write_json(
        &self,
        relative: &Path,
        value: &impl Serialize,
    ) -> Result<(), CaptureError> {
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        self.write_bytes(relative, &bytes).await
    }

    /// Persist a new structured artifact through an atomic same-directory rename.
    ///
    /// State markers use this method and imply complete, synced JSON.
    /// Existing targets are never deliberately replaced.
    /// # Errors
    /// Unsafe and occupied paths return errors. Serialization failures do too.
    /// Writing, syncing, and renaming failures are also returned.
    pub async fn write_json_atomic(
        &self,
        relative: &Path,
        value: &impl Serialize,
    ) -> Result<(), CaptureError> {
        self.ensure_healthy()?;
        let target = self.resolve(relative)?;
        create_private_parents(&self.0.root, &target).await?;
        if tokio::fs::try_exists(&target).await? {
            return Err(CaptureError::Create {
                path: target,
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "capture artifact already exists",
                ),
            });
        }
        let sequence = self.0.sequence.fetch_add(1, Ordering::Relaxed);
        let temporary = TemporaryCaptureIndex(target.with_extension(format!("next-{sequence}")));
        let mut bytes = serde_json::to_vec_pretty(value)?;
        bytes.push(b'\n');
        write_bytes_at(&temporary.0, &bytes).await?;
        tokio::fs::rename(&temporary.0, &target).await?;
        sync_parent_directory(&target).await?;
        Ok(())
    }

    /// Copy a file into the capture and sync the independent copy.
    /// # Errors
    /// Unsafe or existing target, copy failure, or sync failure.
    pub async fn copy_file(&self, relative: &Path, source: &Path) -> Result<(), CaptureError> {
        let target = self.artifact_path(relative)?;
        create_private_parents(&self.0.root, &target).await?;
        copy_file_at(&target, source).await
    }

    /// Retain an immutable file without allocating duplicate blocks when possible.
    ///
    /// Same-filesystem sources are hard-linked. Other filesystems use an
    /// independent, synced copy.
    /// # Errors
    /// Unsafe sources or targets, occupied targets, and I/O failures are rejected.
    pub async fn retain_file(&self, relative: &Path, source: &Path) -> Result<(), CaptureError> {
        let target = self.artifact_path(relative)?;
        create_private_parents(&self.0.root, &target).await?;
        retain_file_at(&target, source).await
    }

    async fn retain_http_body(&self, target: &Path, source: &Path) -> Result<(), CaptureError> {
        self.ensure_healthy()?;
        retain_file_at(target, source).await
    }

    async fn copy_http_body(&self, target: &Path, source: &Path) -> Result<(), CaptureError> {
        self.ensure_healthy()?;
        copy_file_at(target, source).await
    }

    /// Append an ordered structured event to `events.jsonl`.
    /// # Errors
    /// Serialization or write failure.
    pub fn append_event(&self, value: &impl Serialize) -> Result<(), CaptureError> {
        self.ensure_healthy()?;
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        let mut file = self.0.events.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.flush()) {
            *self
                .0
                .failure
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(error.to_string());
            return Err(error.into());
        }
        Ok(())
    }

    /// Check whether an earlier streaming capture write failed.
    /// # Errors
    /// The first durable-capture failure recorded by an event sink.
    pub fn ensure_healthy(&self) -> Result<(), CaptureError> {
        match self
            .0
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            Some(error) => Err(CaptureError::PriorFailure(error.clone())),
            None => Ok(()),
        }
    }

    /// Begin a numbered HTTP exchange and preserve its exact request body.
    /// # Errors
    /// Request-artifact persistence failure.
    pub async fn begin_http(
        &self,
        label: &str,
        head: &HttpRequestHead<'_>,
        body: &[u8],
    ) -> Result<HttpExchange, CaptureError> {
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(CaptureError::UnsafeLabel(label.to_owned()));
        }
        let sequence = self.0.sequence.fetch_add(1, Ordering::Relaxed);
        let relative = PathBuf::from(format!("http/{sequence:06}-{label}"));
        self.write_json(&relative.join("request-head.json"), head)
            .await?;
        self.write_bytes(&relative.join("request-body.bin"), body)
            .await?;
        let started_date_time = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let request_body = format!("{}/request-body.bin", relative.display());
        let request_head = format!("{}/request-head.json", relative.display());
        let response_body = format!("{}/response-body.bin", relative.display());
        let response_head = format!("{}/response-head.json", relative.display());
        let entry = CaptureHarEntry {
            entry: har12::Entries {
                pageref: None,
                started_date_time,
                time: 0.0,
                request: har12::Request {
                    method: head.method.to_owned(),
                    url: head.url.to_owned(),
                    http_version: head.version.to_owned(),
                    cookies: Vec::new(),
                    headers: har_headers(&head.headers),
                    query_string: har_query_string(head.url),
                    post_data: (!body.is_empty()).then(|| har12::PostData {
                        mime_type: header_value(&head.headers, "content-type"),
                        text: String::from_utf8(body.to_vec()).ok(),
                        params: None,
                        comment: Some(format!("Exact bytes: {request_body}")),
                    }),
                    headers_size: -1,
                    body_size: har_size(body.len()),
                    comment: Some(format!(
                        "Exact body: {request_body}; exact header bytes: {request_head}"
                    )),
                },
                response: har12::Response {
                    status: 0,
                    status_text: String::new(),
                    http_version: String::new(),
                    cookies: Vec::new(),
                    headers: Vec::new(),
                    content: har12::Content {
                        size: -1,
                        compression: None,
                        mime_type: None,
                        text: None,
                        encoding: None,
                        comment: Some(format!("Exact bytes: {response_body}")),
                    },
                    redirect_url: None,
                    headers_size: -1,
                    body_size: -1,
                    comment: Some(format!(
                        "Exact body: {response_body}; exact header bytes: {response_head}"
                    )),
                },
                cache: har12::Cache::default(),
                timings: har12::Timings {
                    blocked: None,
                    dns: None,
                    connect: None,
                    send: 0.0,
                    wait: -1.0,
                    receive: -1.0,
                    ssl: None,
                    comment: None,
                },
                server_ip_address: None,
                connection: None,
                comment: Some(format!("garmin-cli exchange {sequence:06}")),
            },
            evidence: HttpEvidenceEntry {
                sequence,
                state: "request-captured".to_owned(),
                request_head,
                request_body,
                response_head,
                response_body,
            },
        };
        self.0
            .har_entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(sequence, entry);
        self.persist_http_indexes().await?;
        Ok(HttpExchange {
            directory: self.0.root.join(relative),
            sequence,
            started: Instant::now(),
            capture: self.clone(),
        })
    }

    fn resolve(&self, relative: &Path) -> Result<PathBuf, CaptureError> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(CaptureError::UnsafePath(relative.to_owned()));
        }
        Ok(self.0.root.join(relative))
    }

    async fn persist_http_indexes(&self) -> Result<(), CaptureError> {
        self.ensure_healthy()?;
        let _writer = self.0.har_writer.lock().await;
        let (entries, evidence) = {
            let entries = self
                .0
                .har_entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            (
                entries
                    .values()
                    .map(|entry| entry.entry.clone())
                    .collect::<Vec<_>>(),
                entries
                    .values()
                    .map(|entry| entry.evidence.clone())
                    .collect::<Vec<_>>(),
            )
        };
        let mut har_bytes = har::to_json(&har_archive(entries))?.into_bytes();
        har_bytes.push(b'\n');
        let mut evidence_bytes = serde_json::to_vec_pretty(&evidence)?;
        evidence_bytes.push(b'\n');
        let revision = self.0.har_revision.fetch_add(1, Ordering::Relaxed);
        replace_capture_index(
            &self.0.root,
            &self.0.root.join("http.har"),
            &format!("http.har.next-{revision}"),
            &har_bytes,
        )
        .await?;
        replace_capture_index(
            &self.0.root,
            &self.0.root.join("http/exchanges.json"),
            &format!("http/exchanges.json.next-{revision}"),
            &evidence_bytes,
        )
        .await?;
        tokio::fs::File::open(&self.0.root)
            .await?
            .sync_all()
            .await?;
        Ok(())
    }
}

async fn retain_file_at(target: &Path, source: &Path) -> Result<(), CaptureError> {
    let metadata = tokio::fs::symlink_metadata(source).await?;
    if !metadata.file_type().is_file() {
        return Err(CaptureError::UnsafePath(source.to_owned()));
    }
    match tokio::fs::hard_link(source, target).await {
        Ok(()) => {
            sync_parent_directory(target).await?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            copy_file_at(target, source).await
        }
        Err(error) => Err(error.into()),
    }
}

async fn copy_file_at(target: &Path, source: &Path) -> Result<(), CaptureError> {
    let mut input = tokio::fs::File::open(source).await?;
    if !input.metadata().await?.is_file() {
        return Err(CaptureError::UnsafePath(source.to_owned()));
    }
    let mut output = host::private_async_file_options().open(target).await?;
    tokio::io::copy(&mut input, &mut output).await?;
    output.flush().await?;
    output.sync_all().await?;
    Ok(())
}

impl HttpExchange {
    /// Persist response metadata before consuming the response stream.
    /// # Errors
    /// Serialization or write failure.
    pub async fn response_head(&self, head: &HttpResponseHead<'_>) -> Result<(), CaptureError> {
        write_json_at(&self.directory.join("response-head.json"), head).await?;
        {
            let mut entries = self
                .capture
                .0
                .har_entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let entry = entries
                .get_mut(&self.sequence)
                .ok_or(CaptureError::MissingHttpExchange(self.sequence))?;
            entry.entry.response.status = i64::from(head.status);
            head.status_text
                .clone_into(&mut entry.entry.response.status_text);
            head.version
                .clone_into(&mut entry.entry.response.http_version);
            entry.entry.response.headers = har_headers(&head.headers);
            let mime_type = header_value(&head.headers, "content-type");
            entry.entry.response.content.mime_type = (!mime_type.is_empty()).then_some(mime_type);
            let redirect = header_value(&head.headers, "location");
            entry.entry.response.redirect_url = (!redirect.is_empty()).then_some(redirect);
            "response-started".clone_into(&mut entry.evidence.state);
            entry.entry.time = self.started.elapsed().as_secs_f64() * 1_000.0;
            entry.entry.timings.wait = entry.entry.time;
        }
        self.capture.persist_http_indexes().await
    }

    /// Create the exact response-body artifact for streaming capture.
    /// # Errors
    /// File-creation failure.
    pub async fn response_body_file(&self) -> Result<tokio::fs::File, CaptureError> {
        self.capture.ensure_healthy()?;
        let path = self.directory.join("response-body.bin");
        let options = host::private_async_file_options();
        Ok(options.open(path).await?)
    }

    /// Persist a complete response body in one operation.
    /// # Errors
    /// Body write or sync failure.
    pub async fn response_body(&self, bytes: &[u8]) -> Result<(), CaptureError> {
        write_bytes_at(&self.directory.join("response-body.bin"), bytes).await?;
        self.complete_response_body(bytes.len()).await
    }

    /// Retain a complete immutable response body from an existing file.
    /// # Errors
    /// File retention or HAR sync failure.
    pub async fn retain_response_body(
        &self,
        source: &Path,
        bytes: usize,
    ) -> Result<(), CaptureError> {
        self.capture
            .retain_http_body(&self.directory.join("response-body.bin"), source)
            .await?;
        self.complete_response_body(bytes).await
    }

    /// Copy an incomplete response body before its source may be resumed.
    /// # Errors
    /// File copying or HAR sync failure.
    pub async fn copy_response_body(
        &self,
        source: &Path,
        bytes: usize,
    ) -> Result<(), CaptureError> {
        self.capture
            .copy_http_body(&self.directory.join("response-body.bin"), source)
            .await?;
        self.complete_response_body(bytes).await
    }

    /// Mark a separately streamed response body as complete in the HAR index.
    /// # Errors
    /// Missing exchange or HAR sync failure.
    pub async fn complete_response_body(&self, bytes: usize) -> Result<(), CaptureError> {
        {
            let mut entries = self
                .capture
                .0
                .har_entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let entry = entries
                .get_mut(&self.sequence)
                .ok_or(CaptureError::MissingHttpExchange(self.sequence))?;
            let bytes = har_size(bytes);
            entry.entry.response.body_size = bytes;
            entry.entry.response.content.size = bytes;
            "complete".clone_into(&mut entry.evidence.state);
            entry.entry.time = self.started.elapsed().as_secs_f64() * 1_000.0;
            entry.entry.timings.receive =
                (entry.entry.time - entry.entry.timings.wait.max(0.0)).max(0.0);
        }
        self.capture.persist_http_indexes().await
    }

    /// Record a transport failure associated with this exchange.
    /// # Errors
    /// Failure-report write error.
    pub async fn error(&self, error: &str) -> Result<(), CaptureError> {
        write_bytes_at(&self.directory.join("error.txt"), error.as_bytes()).await?;
        {
            let mut entries = self
                .capture
                .0
                .har_entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let entry = entries
                .get_mut(&self.sequence)
                .ok_or(CaptureError::MissingHttpExchange(self.sequence))?;
            "error".clone_into(&mut entry.evidence.state);
            entry.entry.time = self.started.elapsed().as_secs_f64() * 1_000.0;
        }
        self.capture.persist_http_indexes().await
    }
}

fn har_archive(entries: Vec<har12::Entries>) -> Har {
    Har {
        log: Spec::V1_2(har12::Log {
            creator: har12::Creator {
                name: "garmin-cli".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                comment: None,
            },
            browser: None,
            pages: None,
            entries,
            comment: Some(
                "Byte-exact bodies and header values are indexed by http/exchanges.json".to_owned(),
            ),
        }),
    }
}

fn har_headers(headers: &[HttpHeader]) -> Vec<har12::Headers> {
    headers
        .iter()
        .map(|header| har12::Headers {
            name: header.name.clone(),
            value: String::from_utf8_lossy(&header.value_bytes).into_owned(),
            comment: None,
        })
        .collect()
}

fn har_query_string(url: &str) -> Vec<har12::QueryString> {
    url::Url::parse(url).map_or_else(
        |_| Vec::new(),
        |url| {
            url.query_pairs()
                .map(|(name, value)| har12::QueryString {
                    name: name.into_owned(),
                    value: value.into_owned(),
                    comment: None,
                })
                .collect()
        },
    )
}

fn header_value(headers: &[HttpHeader], name: &str) -> String {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map_or_else(String::new, |header| {
            String::from_utf8_lossy(&header.value_bytes).into_owned()
        })
}

fn har_size(bytes: usize) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

async fn write_json_at(path: &Path, value: &impl Serialize) -> Result<(), CaptureError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_bytes_at(path, &bytes).await
}

async fn write_bytes_at(path: &Path, bytes: &[u8]) -> Result<(), CaptureError> {
    let options = host::private_async_file_options();
    let mut file = options.open(path).await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    Ok(())
}

async fn replace_capture_index(
    root: &Path,
    target: &Path,
    temporary_relative: &str,
    bytes: &[u8],
) -> Result<(), CaptureError> {
    let temporary = TemporaryCaptureIndex(root.join(temporary_relative));
    write_bytes_at(&temporary.0, bytes).await?;
    tokio::fs::rename(&temporary.0, target).await?;
    sync_parent_directory(target).await?;
    Ok(())
}

struct TemporaryCaptureIndex(PathBuf);

impl Drop for TemporaryCaptureIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

async fn create_private_parents(root: &Path, path: &Path) -> Result<(), CaptureError> {
    let parent = path
        .parent()
        .ok_or_else(|| CaptureError::UnsafePath(path.to_owned()))?;
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| CaptureError::UnsafePath(path.to_owned()))?;
    let mut current = root.to_owned();
    for component in relative.components() {
        current.push(component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(CaptureError::UnsafePath(current)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = current
                    .parent()
                    .ok_or_else(|| CaptureError::UnsafePath(current.clone()))?
                    .to_owned();
                host::create_private_directory(&current)?;
                sync_directory(&current).await?;
                sync_directory(&parent).await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn sync_parent_directory(path: &Path) -> Result<(), CaptureError> {
    let parent = path
        .parent()
        .ok_or_else(|| CaptureError::UnsafePath(path.to_owned()))?;
    sync_directory(parent).await
}

async fn sync_directory(path: &Path) -> Result<(), CaptureError> {
    host::sync_directory(path).await?;
    Ok(())
}

fn private_file(path: &Path) -> Result<File, CaptureError> {
    let options = host::private_file_options();
    options.open(path).map_err(|source| CaptureError::Create {
        path: path.to_owned(),
        source,
    })
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("cannot create capture artifact {path}: {source}")]
    Create {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("unsafe capture path: {0}")]
    UnsafePath(PathBuf),
    #[error("unsafe HTTP capture label: {0}")]
    UnsafeLabel(String),
    #[error("HTTP capture {0} is missing from the HAR index")]
    MissingHttpExchange(u64),
    #[error("an earlier session capture write failed: {0}")]
    PriorFailure(String),
    #[error("capture I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("capture serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("capture timestamp formatting failed: {0}")]
    Time(#[from] time::error::Format),
    #[error("HAR serialization or deserialization failed: {0}")]
    Har(#[from] har::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_missing_private_parent_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().join("missing/parents");
        let root = parent.join("capture");

        let capture = SessionCapture::create(&root).unwrap();

        assert_eq!(capture.root(), root);
        assert!(root.join("events.jsonl").is_file());
        assert!(root.join("http").is_dir());
        assert!(host::directory_is_private(&parent).unwrap());
        assert!(host::directory_is_private(&root).unwrap());
    }

    #[tokio::test]
    async fn refuses_to_replace_a_capture_or_artifact() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("capture");
        let capture = SessionCapture::create(&root).unwrap();
        capture
            .write_bytes(Path::new("plan.json"), b"one")
            .await
            .unwrap();
        assert!(
            capture
                .write_bytes(Path::new("plan.json"), b"two")
                .await
                .is_err()
        );
        assert!(SessionCapture::create(&root).is_err());
    }

    #[tokio::test]
    async fn retained_file_survives_source_removal() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.bin");
        tokio::fs::write(&source, b"retained payload")
            .await
            .unwrap();
        let capture = SessionCapture::create(&temporary.path().join("capture")).unwrap();

        capture
            .retain_file(Path::new("downloads/payload.bin"), &source)
            .await
            .unwrap();
        tokio::fs::remove_file(source).await.unwrap();

        assert_eq!(
            tokio::fs::read(capture.root().join("downloads/payload.bin"))
                .await
                .unwrap(),
            b"retained payload"
        );
    }

    #[tokio::test]
    async fn retains_an_http_body_from_an_immutable_artifact() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("content.bin");
        tokio::fs::write(&source, b"response body").await.unwrap();
        let capture = SessionCapture::create(&temporary.path().join("capture")).unwrap();
        let exchange = capture
            .begin_http(
                "download",
                &HttpRequestHead {
                    method: "GET",
                    url: "https://example.test/content.bin",
                    version: "HTTP/2.0",
                    headers: Vec::new(),
                },
                b"",
            )
            .await
            .unwrap();
        exchange.retain_response_body(&source, 13).await.unwrap();
        tokio::fs::remove_file(source).await.unwrap();

        assert_eq!(
            tokio::fs::read(exchange.directory.join("response-body.bin"))
                .await
                .unwrap(),
            b"response body"
        );
    }

    #[tokio::test]
    async fn indexes_exact_external_bodies_in_har() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("capture");
        let capture = SessionCapture::create(&root).unwrap();
        let exchange = capture
            .begin_http(
                "test",
                &HttpRequestHead {
                    method: "POST",
                    url: "https://example.test/update?device=one",
                    version: "HTTP/1.1",
                    headers: vec![HttpHeader {
                        name: "content-type".to_owned(),
                        value_bytes: b"application/octet-stream".to_vec(),
                    }],
                },
                b"request\0body",
            )
            .await
            .unwrap();
        exchange
            .response_head(&HttpResponseHead {
                status: 200,
                status_text: "OK",
                version: "HTTP/2.0",
                headers: vec![HttpHeader {
                    name: "content-type".to_owned(),
                    value_bytes: b"application/octet-stream".to_vec(),
                }],
            })
            .await
            .unwrap();
        exchange.response_body(b"response\0body").await.unwrap();

        let archive =
            har::from_slice(&tokio::fs::read(root.join("http.har")).await.unwrap()).unwrap();
        let Spec::V1_2(log) = archive.log else {
            panic!("capture must use HAR 1.2");
        };
        assert_eq!(log.entries.len(), 1);
        assert_eq!(log.entries[0].request.query_string[0].name, "device");
        assert_eq!(log.entries[0].response.status, 200);
        let evidence: Vec<HttpEvidenceEntry> = serde_json::from_slice(
            &tokio::fs::read(root.join("http/exchanges.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(evidence[0].state, "complete");
        assert_eq!(
            evidence[0].response_body,
            "http/000001-test/response-body.bin"
        );
        assert_eq!(
            tokio::fs::read(root.join("http/000001-test/request-body.bin"))
                .await
                .unwrap(),
            b"request\0body"
        );
        assert_eq!(
            tokio::fs::read(root.join("http/000001-test/response-body.bin"))
                .await
                .unwrap(),
            b"response\0body"
        );
    }
}
