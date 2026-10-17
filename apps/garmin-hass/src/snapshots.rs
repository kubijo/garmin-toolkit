//! Snapshot stream adapter for the shared browser download registry.

use axum::body::Bytes;
use futures_util::{FutureExt as _, StreamExt as _};
use garmin_service_api::snapshots::{MAX_SNAPSHOT_CHUNK, SnapshotReply};
use garmin_services::snapshots::{SnapshotDownload, SnapshotSession};
use std::sync::Arc;
use uuid::Uuid;

pub(super) async fn prepare(
    session: Arc<SnapshotSession>,
    operation: Uuid,
) -> Result<crate::downloads::Prepared, String> {
    let download = Arc::new(
        session
            .download(operation)
            .await
            .map_err(|error| error.message)?,
    );
    let size = download.size();
    let source = Download {
        handle: Arc::clone(&download),
        size,
        offset: 0,
    };
    let stream = futures_util::stream::try_unfold(source, |mut source| async move {
        Ok::<_, std::io::Error>(
            source
                .next()
                .await?
                .map(|bytes| (Bytes::from(bytes), source)),
        )
    })
    .boxed();
    let cleanup = async move {
        let _ = download.release();
    }
    .boxed();
    Ok(crate::downloads::Prepared::new(
        "garmin-backup.tar.zst".into(),
        size,
        "application/zstd",
        stream,
    )
    .with_operation(operation, cleanup))
}

struct Download {
    handle: Arc<SnapshotDownload>,
    size: u64,
    offset: u64,
}

impl Download {
    pub async fn next(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.offset == self.size {
            return Ok(None);
        }
        let reply = self
            .handle
            .read(self.offset, MAX_SNAPSHOT_CHUNK)
            .await
            .map_err(|error| std::io::Error::other(error.message))?;
        let SnapshotReply::Chunk { offset, bytes, end } = reply else {
            return Err(std::io::Error::other("snapshot chunk was not returned"));
        };
        if offset != self.offset || bytes.is_empty() || bytes.len() > MAX_SNAPSHOT_CHUNK as usize {
            return Err(std::io::Error::other("invalid snapshot download chunk"));
        }
        self.offset += bytes.len() as u64;
        if self.offset > self.size || end != (self.offset == self.size) {
            return Err(std::io::Error::other("snapshot download length changed"));
        }
        Ok(Some(bytes))
    }
}
