//! Bounded, single-use browser downloads. Sources retain ownership until delivery ends.

use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, header},
    response::Response,
};
use futures_util::{StreamExt as _, future::BoxFuture, stream::BoxStream};
use garmin_service_api::DownloadTicket;
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(60);
const MAX_DOWNLOADS: usize = 8;
const CHUNK_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests;

pub(super) struct Prepared {
    file_name: String,
    size: u64,
    media_type: &'static str,
    stream: BoxStream<'static, io::Result<Bytes>>,
    operation: Option<Uuid>,
    staged_bytes: u64,
    cleanup: Option<BoxFuture<'static, ()>>,
}

impl Prepared {
    pub fn from_route(
        download: garmin_services::routes::operations::delivery::RouteDownload,
    ) -> Self {
        let file_name = download.file_name().to_owned();
        let size = download.size();
        let media_type = download.media_type();
        let stream = futures_util::stream::once(async move {
            download
                .begin()
                .await
                .map_err(|error| io::Error::other(error.message))
        })
        .map(|result| match result {
            Ok(delivery) => futures_util::stream::try_unfold(delivery, |mut delivery| async move {
                Ok::<_, io::Error>(
                    delivery
                        .next_chunk()
                        .map(Bytes::copy_from_slice)
                        .map(|bytes| (bytes, delivery)),
                )
            })
            .boxed(),
            Err(error) => futures_util::stream::once(async move { Err(error) }).boxed(),
        })
        .flatten()
        .boxed();
        Self {
            staged_bytes: size,
            ..Self::new(file_name, size, media_type, stream)
        }
    }

    pub fn new(
        file_name: String,
        size: u64,
        media_type: &'static str,
        stream: BoxStream<'static, io::Result<Bytes>>,
    ) -> Self {
        Self {
            file_name,
            size,
            media_type,
            stream,
            operation: None,
            staged_bytes: 0,
            cleanup: None,
        }
    }

    pub fn from_file(
        file_name: String,
        size: u64,
        file: tokio::fs::File,
        owner: impl Send + 'static,
    ) -> Self {
        let stream =
            futures_util::stream::try_unfold((file, owner), |(mut file, owner)| async move {
                let mut bytes = vec![0; CHUNK_BYTES];
                let count = file.read(&mut bytes).await?;
                if count == 0 {
                    return Ok::<_, io::Error>(None);
                }
                bytes.truncate(count);
                Ok(Some((Bytes::from(bytes), (file, owner))))
            })
            .boxed();
        Self {
            staged_bytes: size,
            ..Self::new(file_name, size, "application/octet-stream", stream)
        }
    }

    pub fn with_operation(mut self, operation: Uuid, cleanup: BoxFuture<'static, ()>) -> Self {
        self.operation = Some(operation);
        self.cleanup = Some(cleanup);
        self
    }
}

#[derive(Clone, Default)]
pub(super) struct Downloads {
    pending: Arc<Mutex<HashMap<Uuid, Pending>>>,
    reservations: Arc<Mutex<HashMap<Uuid, Reservation>>>,
}

struct Pending {
    created: Instant,
    download: Download,
}
struct Reservation {
    operation: Option<Uuid>,
    staged_bytes: u64,
}

impl Downloads {
    pub fn insert(&self, prepared: Prepared) -> Result<DownloadTicket, String> {
        self.expire();
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let mut reservations = self
            .reservations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if reservations.len() >= MAX_DOWNLOADS {
            return Err("too many browser downloads are pending or active".into());
        }
        if prepared.operation.is_some_and(|operation| {
            reservations
                .values()
                .any(|value| value.operation == Some(operation))
        }) {
            return Err("a download for this operation is already pending or active".into());
        }
        let bytes = reservations
            .values()
            .try_fold(prepared.staged_bytes, |sum, value| {
                sum.checked_add(value.staged_bytes)
            });
        if bytes.is_none_or(|bytes| bytes > garmin_device::MAX_BROWSER_TRANSFER_BYTES) {
            return Err("staged browser downloads exceed the 512 MiB limit".into());
        }
        let token = loop {
            let token = Uuid::new_v4();
            if !reservations.contains_key(&token) {
                break token;
            }
        };
        let ticket = DownloadTicket {
            token,
            file_name: prepared.file_name.clone(),
        };
        reservations.insert(
            token,
            Reservation {
                operation: prepared.operation,
                staged_bytes: prepared.staged_bytes,
            },
        );
        let remaining = prepared.size;
        pending.insert(
            token,
            Pending {
                created: Instant::now(),
                download: Download {
                    prepared,
                    remaining,
                    lease: Some(Lease {
                        token,
                        reservations: Arc::clone(&self.reservations),
                    }),
                },
            },
        );
        drop(reservations);
        drop(pending);
        let pending = Arc::downgrade(&self.pending);
        tokio::spawn(async move {
            tokio::time::sleep(TTL).await;
            if let Some(pending) = pending.upgrade() {
                pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&token);
            }
        });
        Ok(ticket)
    }

    fn expire(&self) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, pending| pending.created.elapsed() < TTL);
    }

    pub fn take(&self, token: Uuid) -> Option<Download> {
        self.expire();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&token)
            .map(|pending| pending.download)
    }
}

struct Lease {
    token: Uuid,
    reservations: Arc<Mutex<HashMap<Uuid, Reservation>>>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.reservations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
    }
}

pub(super) struct Download {
    prepared: Prepared,
    remaining: u64,
    lease: Option<Lease>,
}

impl Download {
    async fn next(&mut self) -> io::Result<Option<Bytes>> {
        let Some(bytes) = self.prepared.stream.next().await.transpose()? else {
            if self.remaining != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "download ended before its declared length",
                ));
            }
            return Ok(None);
        };
        if bytes.is_empty() || bytes.len() > CHUNK_BYTES {
            return Err(io::Error::other("invalid download chunk length"));
        }
        self.remaining = self
            .remaining
            .checked_sub(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("download exceeded its declared length"))?;
        Ok(Some(bytes))
    }

    pub fn response(self) -> Response {
        let size = self.prepared.size;
        let media_type = self.prepared.media_type;
        let stream = futures_util::stream::try_unfold(self, |mut download| async move {
            Ok::<_, io::Error>(download.next().await?.map(|bytes| (bytes, download)))
        });
        let mut response = Response::new(Body::from_stream(stream));
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        response
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        if let Some(cleanup) = self.prepared.cleanup.take() {
            let lease = self.lease.take();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    cleanup.await;
                    drop(lease);
                });
            }
        }
    }
}
