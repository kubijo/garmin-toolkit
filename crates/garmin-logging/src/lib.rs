//! Bounded, backend-owned structured log history and typed subscriptions.

use garmin_model::logging::{Cursor, Filter, IngestReport, Level, MAX_RECORD_BYTES, Record};
use garmin_service_api::logging::{Batch, LogService, LogServiceClient, LogServiceServerShared};
use remoc::{rch, rtc, rtc::ServerShared as _};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    fs::{File, OpenOptions},
    io,
    io::{BufRead as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tracing_subscriber::{Layer, layer::Context};

mod directory;
mod history;
mod query;

#[cfg(test)]
mod tests;

const SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
const BATCH_LIMIT: usize = 256;
static GLOBAL: OnceLock<Store> = OnceLock::new();

#[derive(Clone)]
pub struct Store(Arc<Inner>);
struct Inner {
    directory: PathBuf,
    state: Arc<Mutex<State>>,
    session: String,
    epoch: [u8; 16],
    source: String,
    pending: std::sync::mpsc::SyncSender<Write>,
    dropped: AtomicU64,
    writer: Option<std::thread::JoinHandle<()>>,
}
enum Write {
    Records(Vec<Record>),
    Flush(std::sync::mpsc::SyncSender<()>),
    Shutdown,
}

struct State {
    history: history::History,
    segments: VecDeque<u64>,
    next: u64,
    bytes: u64,
    file: File,
    error: Option<String>,
    disk_available: bool,
    // Held by the writer state until all queued records have been handled.
    _lease: File,
}

impl Store {
    /// Open an instance-owned log directory, recovering retained history.
    /// Concurrent stores use locked `writer-N` subdirectories with separate histories.
    /// The actual location is available through [`Self::directory`].
    /// # Errors
    /// Returns filesystem or malformed retained-record errors.
    pub fn open(directory: impl AsRef<Path>, source: &str) -> io::Result<Self> {
        let (directory, lease) = directory::claim(directory.as_ref())?;
        let mut history = history::History::default();
        let mut segments = VecDeque::new();
        let mut next = 1;
        let mut recovery_error = None;
        for index in (0..4).rev() {
            if let Ok(file) = File::open(directory.join(format!("log-{index}.jsonl"))) {
                let mut first = true;
                for line in io::BufReader::new(file).lines() {
                    let line = line?;
                    let record: Record = match serde_json::from_str(&line) {
                        Ok(record) => record,
                        Err(error) => {
                            recovery_error =
                                Some(format!("Skipped malformed retained log record: {error}"));
                            continue;
                        }
                    };
                    if first {
                        segments.push_back(record.sequence);
                        first = false;
                    }
                    next = next.max(record.sequence + 1);
                    history.push(record, line.len() + 1);
                }
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("log-0.jsonl"))?;
        let bytes = file.metadata()?.len();
        if bytes > 0 {
            use std::io::{Read as _, Seek as _};
            let mut tail = File::open(directory.join("log-0.jsonl"))?;
            tail.seek(io::SeekFrom::End(-1))?;
            let mut last = [0];
            tail.read_exact(&mut last)?;
            if last[0] != b'\n' {
                file.write_all(b"\n")?;
            }
        }
        let bytes = file.metadata()?.len();
        if segments.is_empty() {
            segments.push_back(next);
        }
        let state = Arc::new(Mutex::new(State {
            history,
            segments,
            next,
            bytes,
            file,
            error: recovery_error,
            disk_available: true,
            _lease: lease,
        }));
        let (pending, receiver) = std::sync::mpsc::sync_channel(512);
        let writer_state = state.clone();
        let writer_directory = directory.clone();
        let writer = std::thread::Builder::new()
            .name("application-log-writer".into())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        Write::Records(records) => {
                            if let Err(error) =
                                append_records(&writer_directory, &writer_state, records)
                            {
                                writer_state
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .error = Some(error);
                            }
                        }
                        Write::Flush(done) => {
                            let _ = writer_state
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .file
                                .flush();
                            let _ = done.send(());
                        }
                        Write::Shutdown => break,
                    }
                }
            })?;
        Ok(Self(Arc::new(Inner {
            directory,
            session: uuid::Uuid::new_v4().to_string(),
            epoch: *uuid::Uuid::new_v4().as_bytes(),
            source: source.into(),
            pending,
            dropped: AtomicU64::new(0),
            state,
            writer: Some(writer),
        })))
    }

    /// Publish the host's shared logging handle.
    pub fn install_global(&self) {
        let _ = GLOBAL.set(self.clone());
    }
    #[must_use]
    pub fn global() -> Option<Self> {
        GLOBAL.get().cloned()
    }
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.0.directory
    }
    #[must_use]
    pub fn session(&self) -> &str {
        &self.0.session
    }

    /// Append a bounded structured batch. Client identities are scoped by the RPC adapter.
    /// # Errors
    /// Returns validation, storage, or synchronization errors.
    pub fn append(&self, records: Vec<Record>) -> Result<(), String> {
        append_records(&self.0.directory, &self.0.state, records)
    }

    /// Drain the bounded event queue before orderly host shutdown.
    pub fn flush(&self) {
        let (done, completion) = std::sync::mpsc::sync_channel(0);
        if self.0.pending.send(Write::Flush(done)).is_ok() {
            let _ = completion.recv();
        }
    }

    #[must_use]
    pub fn history(&self, filter: &Filter, after: Option<Cursor>) -> Batch {
        let state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = state
            .history
            .records
            .front()
            .map_or(state.next, |record| record.sequence);
        let mut batch = Batch {
            cursor: after.unwrap_or(Cursor {
                epoch: self.0.epoch,
                sequence: first.saturating_sub(1),
            }),
            gap: after.is_some_and(|cursor| {
                cursor.epoch != self.0.epoch
                    || cursor.sequence.saturating_add(1) < first
                    || cursor.sequence >= state.next
            }),
            error: state.error.clone(),
            records: Vec::new(),
        };
        if batch.gap {
            batch.cursor = Cursor {
                epoch: self.0.epoch,
                sequence: first.saturating_sub(1),
            };
        }
        for record in &state.history.records {
            if record.sequence <= batch.cursor.sequence {
                continue;
            }
            batch.cursor.sequence = record.sequence;
            if filter.matches(record) {
                batch.records.push(record.clone());
            }
            if batch.records.len() == BATCH_LIMIT {
                break;
            }
        }
        batch
    }

    /// Create a local or remotely transferable RPC handle on the current Tokio runtime.
    #[must_use]
    pub fn client(&self) -> LogServiceClient {
        let (server, client) = LogServiceServerShared::new(Arc::new(self.clone()));
        tokio::spawn(server.serve());
        client
    }

    fn stream(
        &self,
        filter: Filter,
        after: Option<Cursor>,
        live: bool,
    ) -> rch::mpsc::Receiver<Batch> {
        let (sender, receiver) = rch::mpsc::with_local_buffer(4);
        let store = self.clone();
        tokio::spawn(async move {
            let (end, first) = {
                let state = store
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (
                    state.next - 1,
                    state
                        .history
                        .records
                        .front()
                        .map_or(state.next, |record| record.sequence),
                )
            };
            let mut cursor = if live {
                after
            } else {
                Some(Cursor {
                    epoch: store.0.epoch,
                    sequence: first.saturating_sub(1),
                })
            };
            loop {
                let mut batch = store.history(&filter, cursor);
                if !live {
                    batch.records.retain(|record| record.sequence <= end);
                    batch.cursor.sequence = batch.cursor.sequence.min(end);
                }
                cursor = Some(batch.cursor);
                if sender.send(batch).await.is_err() {
                    break;
                }
                if !live && cursor.is_some_and(|cursor| cursor.sequence >= end) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        });
        receiver
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Drain the queue and release the directory lease before reopening is possible.
        let _ = self.pending.send(Write::Shutdown);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

impl LogService for Store {
    fn history(
        &self,
        filter: Filter,
        after: Option<Cursor>,
    ) -> impl Future<Output = Result<Batch, rtc::CallError>> {
        std::future::ready(Ok(Self::history(self, &filter, after)))
    }
    fn subscribe(
        &self,
        filter: Filter,
        after: Option<Cursor>,
    ) -> impl Future<Output = Result<rch::mpsc::Receiver<Batch>, rtc::CallError>> {
        std::future::ready(Ok(self.stream(filter, after, true)))
    }
    fn export(
        &self,
        filter: Filter,
    ) -> impl Future<Output = Result<rch::mpsc::Receiver<Batch>, rtc::CallError>> {
        std::future::ready(Ok(self.stream(filter, None, false)))
    }
    async fn ingest(
        &self,
        records: Vec<Record>,
    ) -> Result<Result<IngestReport, String>, rtc::CallError> {
        let mut report = IngestReport::default();
        if records.len() > BATCH_LIMIT {
            report.rejected = u64::try_from(records.len()).unwrap_or(u64::MAX);
            return Ok(Ok(report));
        }
        let mut admitted = Vec::new();
        for mut record in records {
            record.source = format!("browser/{}", record.source.trim_start_matches("browser/"));
            // Reserve the largest backend sequence before admission so append cannot
            // reject a record after accepting part of the batch.
            record.sequence = u64::MAX;
            if record.source_sequence == 0
                || !matches!(serde_json::to_vec(&record), Ok(encoded) if encoded.len() <= MAX_RECORD_BYTES)
            {
                report.rejected += 1;
            } else {
                admitted.push(record);
            }
        }
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.append(admitted).map(|()| report))
            .await
            .map_err(|_| rtc::CallError::Dropped)
    }
}

impl<S: tracing::Subscriber> Layer<S> for Store {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let level = match *event.metadata().level() {
            tracing::Level::TRACE => Level::Trace,
            tracing::Level::DEBUG => Level::Debug,
            tracing::Level::INFO => Level::Info,
            tracing::Level::WARN => Level::Warn,
            tracing::Level::ERROR => Level::Error,
        };
        let record = Record {
            sequence: 0,
            timestamp_ms: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            level,
            component: event.metadata().target().into(),
            source: self.0.source.clone(),
            session: self.0.session.clone(),
            source_sequence: 0,
            message: fields.0.remove("message").unwrap_or_default(),
            fields: fields.0,
        };
        let dropped = self.0.dropped.swap(0, Ordering::Relaxed);
        let mut record = record;
        if dropped > 0 {
            record
                .fields
                .insert("dropped_records".into(), dropped.to_string());
        }
        if self
            .0
            .pending
            .try_send(Write::Records(vec![record]))
            .is_err()
        {
            self.0.dropped.fetch_add(dropped + 1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct Fields(BTreeMap<String, String>);
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
}

fn append_records(
    directory: &Path,
    state: &Mutex<State>,
    records: Vec<Record>,
) -> Result<(), String> {
    if records.len() > BATCH_LIMIT {
        return Err("log batch exceeds 256 records".into());
    }
    let mut state = state.lock().map_err(|error| error.to_string())?;
    for mut record in records {
        let mut encoded = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_RECORD_BYTES {
            return Err("log record exceeds 16 KiB".into());
        }
        if record.source_sequence == 0 {
            record.source_sequence = state.next;
        }
        if state.history.contains(&record) {
            continue;
        }
        record.sequence = state.next;
        encoded = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
        let result = (|| -> io::Result<()> {
            if !state.disk_available {
                return Ok(());
            }
            if state.bytes + encoded.len() as u64 > SEGMENT_BYTES {
                state.file.flush()?;
                let oldest = directory.join("log-3.jsonl");
                if oldest.exists() {
                    fs::remove_file(oldest)?;
                }
                for index in (0..3).rev() {
                    let path = directory.join(format!("log-{index}.jsonl"));
                    if path.exists() {
                        fs::rename(path, directory.join(format!("log-{}.jsonl", index + 1)))?;
                    }
                }
                state.file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(directory.join("log-0.jsonl"))?;
                state.bytes = 0;
                state.segments.push_back(record.sequence);
                if state.segments.len() > 4 {
                    state.segments.pop_front();
                }
                if let Some(first) = state.segments.front().copied() {
                    state.history.expire_before(first);
                }
            }
            state.file.write_all(&encoded)?;
            state.bytes += encoded.len() as u64;
            Ok(())
        })();
        if let Err(error) = result {
            state.error = Some(format!("Log persistence stopped until reopen: {error}"));
            // A failed rotation or partial write may leave the handle pointing at an
            // old segment or an incomplete line. Do not append through it again.
            state.disk_available = false;
        }
        state.next += 1;
        state.history.push(record, encoded.len());
    }
    Ok(())
}
