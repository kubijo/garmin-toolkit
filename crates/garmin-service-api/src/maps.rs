//! Host-owned map workflows.
//! Clients render snapshots and submit revision-bound choices.

#![expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates serialized RPC requests"
)]

use std::time::Duration;

use garmin_model::artifact::{ByteCount, ByteRate};
use remoc::rtc;
use uuid::Uuid;

#[garmin_macros::portable(copy, eq)]
pub enum Action {
    ContactService,
    Choose,
    Review,
    Approve,
    Back,
    Cancel,
    Recover,
    ClearRecovery,
    DiscardPreparation,
    Refresh,
    ReviewRecovery,
    ApproveRecovery,
}

#[garmin_macros::portable(copy, eq)]
pub enum Phase {
    Consent,
    Loading,
    Catalog,
    Review,
    Running,
    Recovery,
    Completed,
    Failed,
    Cancelled,
}

#[garmin_macros::portable(copy, default, eq)]
pub enum Choice {
    #[default]
    Keep,
    Install,
    Remove,
}

#[garmin_macros::portable(copy, eq)]
pub enum CatalogService {
    Garmin,
    Simulation,
}

#[garmin_macros::portable(eq)]
pub struct Component {
    pub index: u32,
    pub name: String,
    pub installed: Option<String>,
    pub available: Option<String>,
    pub operation: garmin_model::map::MapOperation,
    pub can_install: bool,
    pub can_remove: bool,
    pub choice: Choice,
    pub download_bytes: ByteCount,
    pub cached_files: u32,
    pub total_files: u32,
}

#[garmin_macros::portable(eq)]
pub struct Plan {
    pub approval: Uuid,
    pub digest: String,
    pub removal: bool,
    pub components: Vec<String>,
    pub download_bytes: ByteCount,
    pub write_count: u32,
    pub remove_count: u32,
    pub remove_bytes: ByteCount,
    pub paths: Vec<String>,
    pub storage_requirements: Vec<StorageRequirement>,
}

#[garmin_macros::portable(eq)]
pub struct StorageRequirement {
    pub storage: String,
    pub required_free_bytes: ByteCount,
}

#[garmin_macros::portable(eq)]
pub struct Recovery {
    pub plan: String,
    pub removal: bool,
    pub simulated: bool,
    pub actions: Vec<Action>,
    pub review: Option<RecoveryReview>,
}

#[garmin_macros::portable(eq)]
pub struct RecoveryReview {
    pub approval: Uuid,
    pub files: Vec<RecoveryFile>,
}

#[garmin_macros::portable(eq)]
pub struct RecoveryFile {
    pub storage: String,
    pub path: String,
}

#[garmin_macros::portable(eq)]
pub struct Progress {
    pub stage: String,
    pub label: String,
    pub completed: u64,
    pub total: Option<u64>,
    pub bytes: bool,
    #[serde(rename = "elapsed_ms", with = "elapsed_milliseconds")]
    pub elapsed: Duration,
    pub bytes_per_second: Option<ByteRate>,
    #[serde(rename = "remaining_seconds", with = "optional_seconds")]
    pub remaining: Option<Duration>,
    pub stalled: bool,
    pub path: Option<String>,
    pub status: ProgressStatus,
}

mod elapsed_milliseconds {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub(super) fn serialize<S: Serializer>(
        value: &Duration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Duration, D::Error> {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

mod optional_seconds {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    #[expect(clippy::ref_option, reason = "Serde requires the field by reference")]
    pub(super) fn serialize<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(|elapsed| elapsed.as_secs()).serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Option::<u64>::deserialize(deserializer).map(|value| value.map(Duration::from_secs))
    }
}

#[garmin_macros::portable(copy, eq)]
pub enum ProgressStatus {
    Waiting,
    Running,
    Completed,
    Failed,
}

#[garmin_macros::portable(eq)]
pub struct Outcome {
    pub id: Uuid,
    pub phase: Phase,
    pub message: String,
    #[serde(default)]
    pub recovered: bool,
}

#[garmin_macros::portable(eq)]
pub struct State {
    pub id: Uuid,
    pub revision: Uuid,
    pub device: String,
    pub service: CatalogService,
    pub phase: Phase,
    pub components: Vec<Component>,
    pub verified_backup: bool,
    pub dry_run: bool,
    pub actions: Vec<Action>,
    pub storages: Vec<garmin_model::device::DeviceStorageState>,
    pub storage_error: Option<String>,
    pub plan: Option<Plan>,
    pub recovery: Option<Recovery>,
    pub progress: Vec<Progress>,
    pub active: Vec<Progress>,
    pub events: Vec<Progress>,
    pub history: Vec<Outcome>,
    pub error: Option<String>,
}

#[garmin_macros::portable(eq)]
pub enum Command {
    ContactService,
    Choose { component: u32, choice: Choice },
    VerifiedBackup(bool),
    DryRun(bool),
    Review,
    Approve { approval: Uuid },
    Back,
    Cancel,
    Recover,
    ClearRecovery,
    DiscardPreparation,
    Refresh,
    ReviewRecovery,
    ApproveRecovery { approval: Uuid },
}

#[garmin_macros::portable(eq)]
pub enum Request {
    State,
    Change {
        request: Uuid,
        revision: Uuid,
        command: Command,
    },
}

#[garmin_macros::portable(copy, eq)]
pub enum FailureKind {
    StaleRevision,
    ReusedRequest,
    InvalidChoice,
    Unavailable,
    Busy,
    Operation,
}

#[garmin_macros::portable(eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    pub revision: Uuid,
}

#[rtc::remote]
pub trait MapService {
    async fn request(&self, request: Request) -> Result<Result<State, Failure>, rtc::CallError>;
    async fn watch(&self) -> Result<remoc::rch::watch::Receiver<State>, rtc::CallError>;
}
