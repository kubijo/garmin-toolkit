//! Bounded snapshot operations. Hosts bind this service to an authenticated actor.

#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC requests"
)]

use remoc::rtc;
use uuid::Uuid;

pub const MAX_SNAPSHOT_CHUNK: u32 = 64 * 1024;

#[garmin_macros::portable(copy, eq)]
pub enum SnapshotState {
    Uploading,
    Verifying,
    AwaitingApproval,
    BackingUp,
    DownloadReady,
    Restoring,
    Completed,
    Cancelled,
    Failed,
}

#[garmin_macros::portable(eq)]
pub struct SnapshotPreview {
    pub format_version: u32,
    pub app_version: String,
    pub created_at: i64,
    pub database_bytes: u64,
    pub database_sha256: String,
    /// Single-use approval, bound to this verified upload and current client session.
    pub approval: Uuid,
}

#[garmin_macros::portable(eq)]
pub struct SnapshotStatus {
    pub operation: Uuid,
    pub epoch: Uuid,
    pub state: SnapshotState,
    pub transferred: u64,
    pub total: Option<u64>,
    pub preview: Option<SnapshotPreview>,
    pub error: Option<String>,
}

/// The original operation, retained by the host across browser reloads.
#[garmin_macros::portable(eq)]
pub enum SnapshotSource {
    Download,
    Upload,
    ServerSave(String),
    ServerRestore(String),
}

#[garmin_macros::portable(eq)]
pub struct SnapshotOperation {
    pub status: SnapshotStatus,
    pub source: SnapshotSource,
    /// Another session or HTTP download still owns this operation.
    pub active: bool,
}

#[garmin_macros::portable(eq)]
pub enum SnapshotRequest {
    List,
    /// Client-generated identifier makes a lost start response resumable.
    BeginBackup {
        operation: Uuid,
    },
    BeginRestore {
        operation: Uuid,
        bytes: u64,
    },
    Upload {
        operation: Uuid,
        offset: u64,
        bytes: Vec<u8>,
    },
    Verify {
        operation: Uuid,
    },
    Approve {
        operation: Uuid,
        approval: Uuid,
    },
    Cancel {
        operation: Uuid,
    },
    Status {
        operation: Uuid,
    },
    /// Claims an existing operation for a reconnected session and revokes prior approval.
    Resume {
        operation: Uuid,
    },
    /// Claims an abandoned operation only; never revokes a live connection or download.
    Recover {
        operation: Uuid,
    },
    Read {
        operation: Uuid,
        offset: u64,
        max_bytes: u32,
    },
    Release {
        operation: Uuid,
    },
}

#[garmin_macros::portable(eq)]
pub enum SnapshotReply {
    Operations(Vec<SnapshotOperation>),
    Status(SnapshotStatus),
    Chunk {
        offset: u64,
        bytes: Vec<u8>,
        end: bool,
    },
    Released,
}

#[garmin_macros::portable(copy, eq)]
pub enum SnapshotFailureKind {
    Unauthorized,
    NotFound,
    Stale,
    Limit,
    InvalidState,
    InvalidApproval,
    Io,
}

#[garmin_macros::portable(eq)]
pub struct SnapshotFailure {
    pub kind: SnapshotFailureKind,
    pub message: String,
}

#[rtc::remote]
pub trait SnapshotService {
    async fn execute(
        &self,
        request: SnapshotRequest,
    ) -> Result<Result<SnapshotReply, SnapshotFailure>, rtc::CallError>;
}
