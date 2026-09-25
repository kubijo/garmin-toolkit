//! Cursor-based history and live log transport. Cursors advance over filtered records too.

#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC request types"
)]

use garmin_model::logging::{Cursor, Filter, IngestReport, Record};
use remoc::{rch, rtc};

#[garmin_macros::portable(default)]
pub struct Batch {
    pub records: Vec<Record>,
    pub cursor: Cursor,
    pub gap: bool,
    pub error: Option<String>,
}

#[rtc::remote]
pub trait LogService {
    async fn history(&self, filter: Filter, after: Option<Cursor>)
    -> Result<Batch, rtc::CallError>;
    async fn subscribe(
        &self,
        filter: Filter,
        after: Option<Cursor>,
    ) -> Result<rch::mpsc::Receiver<Batch>, rtc::CallError>;
    async fn ingest(
        &self,
        records: Vec<Record>,
    ) -> Result<Result<IngestReport, String>, rtc::CallError>;
    async fn export(&self, filter: Filter) -> Result<rch::mpsc::Receiver<Batch>, rtc::CallError>;
}
