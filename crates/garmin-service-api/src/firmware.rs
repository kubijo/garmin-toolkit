//! Host-owned device software lookup and selection.
#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC requests"
)]

use garmin_model::artifact::ByteCount;
use remoc::{rch, rtc};
use uuid::Uuid;

pub use garmin_model::firmware::{Component, ReleaseNote};
pub use garmin_progress::{
    OperationStage, ProgressAmount, ProgressState,
    model::{StageLine, Transcript},
};

pub const WRITE_STAGES: &[OperationStage] = &[
    OperationStage::Download,
    OperationStage::Verify,
    OperationStage::Stage,
    OperationStage::Backup,
    OperationStage::Commit,
    OperationStage::DeviceFinalize,
    OperationStage::DeviceVerify,
    OperationStage::Cleanup,
];

#[garmin_macros::portable(eq)]
pub struct AvailableUpdate {
    pub attachment: Uuid,
    pub version: String,
}

#[garmin_macros::portable(eq)]
pub struct Review {
    pub approval: Uuid,
    pub selected: Vec<u32>,
    pub download_bytes: ByteCount,
}

#[garmin_macros::portable(eq)]
pub enum Phase {
    Consent,
    Checking,
    Available(Vec<Component>),
    Failed(String),
    Staging(TransferProgress),
    ReadyToEject { needs_restart: bool },
    RecoveryRequired,
    Transfers(Vec<RetainedTransfer>),
    ReviewRecovery { approval: Uuid, files: Vec<String> },
    Reconnected(Vec<Observation>),
    VersionsUnverified,
    Recovering,
    NoTransfer,
    Disconnected,
    PreviousTransferPending,
    WriteFailed(String),
}

#[garmin_macros::portable(eq)]
pub struct RetainedTransfer {
    pub receipt: Uuid,
    pub registered_at: garmin_model::value::Timestamp,
    pub status: ReceiptStatus,
}

#[garmin_macros::portable(eq)]
pub enum ReceiptStatus {
    VersionsUnverified,
    RecoveryRequired,
    Unavailable,
    Confirmed,
    AwaitingConfirmation,
}

#[garmin_macros::portable(eq)]
pub struct TransferProgress {
    pub label: String,
    pub completed: Option<ByteCount>,
    pub total: Option<ByteCount>,
}

#[garmin_macros::portable(eq)]
pub struct Observation {
    pub part_number: String,
    pub name: String,
    pub offered: String,
    pub status: InstalledStatus,
}

#[garmin_macros::portable(eq)]
pub enum InstalledStatus {
    Confirmed,
    Unchanged,
    OtherVersion(String),
    Unreported,
}

#[garmin_macros::portable(eq)]
pub enum Action {
    Check,
    Review(Vec<u32>),
    Stage(Uuid),
    Inspect,
    InspectReceipt(Uuid),
    Recover,
    ApproveRecovery(Uuid),
    Acknowledge,
    CloseInspection,
    EditSelection,
}

#[garmin_macros::portable(eq)]
pub struct Inspection {
    pub background: Phase,
    pub from_list: bool,
}

#[garmin_macros::portable(eq)]
pub struct State {
    pub simulated: bool,
    pub revision: Uuid,
    pub phase: Phase,
    pub review: Option<Review>,
    pub inspection: Option<Inspection>,
    pub catalog_id: Uuid,
    #[serde(default)]
    pub has_retained_transfer: bool,
    #[serde(default)]
    pub transcript: Transcript,
}

#[rtc::remote]
pub trait FirmwareService {
    async fn watch(&self) -> Result<rch::watch::Receiver<State>, rtc::CallError>;
    async fn check(&self, revision: Uuid) -> Result<Result<State, String>, rtc::CallError>;
    async fn review(
        &self,
        revision: Uuid,
        selected: Vec<u32>,
    ) -> Result<Result<State, String>, rtc::CallError>;
    async fn act(
        &self,
        revision: Uuid,
        action: Action,
    ) -> Result<Result<State, String>, rtc::CallError>;
}
