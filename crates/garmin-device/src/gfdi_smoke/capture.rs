use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::Instant;

use super::SmokeError;

pub(super) struct Capture {
    file: File,
    start: Instant,
    count: usize,
    bytes: usize,
    max_count: usize,
    max_bytes: usize,
}

impl Capture {
    pub(super) fn create(directory: &Path) -> Result<Self, SmokeError> {
        Self::with_limits(directory, 4096, 2 * 1024 * 1024)
    }

    pub(super) fn create_download(directory: &Path) -> Result<Self, SmokeError> {
        Self::with_limits(directory, 2_000_000, 64 * 1024 * 1024)
    }

    fn with_limits(
        directory: &Path,
        max_count: usize,
        max_bytes: usize,
    ) -> Result<Self, SmokeError> {
        DirBuilder::new().mode(0o700).create(directory)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join("events.jsonl"))?;
        Ok(Self {
            file,
            start: Instant::now(),
            count: 0,
            bytes: 0,
            max_count,
            max_bytes,
        })
    }

    pub(super) fn record(&mut self, kind: &str, bytes: Option<&[u8]>) -> Result<(), SmokeError> {
        self.record_detail(kind, bytes, &serde_json::Value::Null)
    }

    pub(super) fn metadata(
        &mut self,
        kind: &str,
        detail: &serde_json::Value,
    ) -> Result<(), SmokeError> {
        self.record_detail(kind, None, detail)
    }

    fn record_detail(
        &mut self,
        kind: &str,
        bytes: Option<&[u8]>,
        detail: &serde_json::Value,
    ) -> Result<(), SmokeError> {
        let size = bytes.map_or(0, <[u8]>::len);
        if self.count >= self.max_count || self.bytes.saturating_add(size) > self.max_bytes {
            return Err(SmokeError::TraceLimit);
        }
        serde_json::to_writer(
            &mut self.file,
            &serde_json::json!({
                "elapsed_ms": self.start.elapsed().as_millis(),
                "event": kind,
                "bytes": bytes.map(hex::encode),
                "detail": detail,
            }),
        )?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        self.count += 1;
        self.bytes += size;
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<(), SmokeError> {
        self.file.sync_all()?;
        Ok(())
    }
}
