//! Typed contracts shared by native hosts and isolated clients.

use camino::Utf8PathBuf;
use garmin_model::{
    activity::ActivitySummary, device::DeviceStorageState, identity::User,
    observation::ObservationId, route::Coordinate,
};
#[garmin_macros::portable(eq)]
pub struct DeviceSnapshot {
    pub key: String,
    pub name: String,
    pub identifier: Option<u32>,
    pub software_version: Option<u16>,
    pub inspection: InspectionState,
    pub inspection_error: Option<String>,
    pub capabilities: Vec<DeviceCapability>,
    pub storages: Vec<DeviceStorageState>,
}

#[garmin_macros::portable(eq)]
pub struct DeviceCatalogSnapshot {
    pub device_key: String,
    pub storages: Vec<DeviceCatalogStorage>,
}

#[garmin_macros::portable(eq)]
pub struct DeviceCatalogStorage {
    pub id: String,
    pub label: String,
    pub entries: Vec<DeviceCatalogEntry>,
}

#[garmin_macros::portable(eq)]
pub struct DeviceCatalogEntry {
    pub path: Utf8PathBuf,
    pub kind: DeviceCatalogEntryKind,
    pub size: Option<u64>,
}

#[garmin_macros::portable(copy, eq)]
pub enum DeviceCatalogEntryKind {
    Directory,
    File,
}

#[garmin_macros::portable(eq)]
pub struct DeviceBrowserTarget {
    pub storage_id: String,
    pub path: Utf8PathBuf,
    pub kind: DeviceCatalogEntryKind,
}

#[garmin_macros::portable(eq)]
pub enum DeviceBrowserRequest {
    CreateDirectory {
        device_key: String,
        storage_id: String,
        parent: Utf8PathBuf,
        name: String,
    },
    Remove {
        device_key: String,
        target: DeviceBrowserTarget,
    },
}

#[garmin_macros::portable(eq)]
pub struct DeviceBrowserUpload {
    pub device_key: String,
    pub storage_id: String,
    pub directory: Utf8PathBuf,
    pub file_name: String,
    pub size: u64,
}

#[garmin_macros::portable(eq)]
pub struct DeviceBrowserDownloadTicket {
    pub token: String,
    pub file_name: String,
}

#[garmin_macros::portable]
pub struct DeviceFitPreview {
    pub file_name: String,
    pub activities: Vec<DeviceFitPreviewActivity>,
}

#[garmin_macros::portable]
pub struct DeviceFitPreviewActivity {
    pub source: String,
    pub summary: ActivitySummary,
    pub segments: Vec<Vec<Coordinate>>,
}

#[garmin_macros::portable(eq)]
pub enum DeviceFitImportOutcome {
    Imported { activities: u64 },
    Duplicate,
    Rejected { reason: String },
}

#[garmin_macros::portable(copy, eq)]
pub struct DeviceCapability {
    pub data_type: DeviceDataType,
    pub direction: TransferDirection,
}

#[garmin_macros::portable(copy, eq)]
pub enum DeviceDataType {
    Activity,
    Workout,
    Course,
}

#[garmin_macros::portable(copy, eq)]
pub enum TransferDirection {
    OutputFromUnit,
    InputToUnit,
    InputOutput,
}

#[garmin_macros::portable(copy, default, eq)]
pub enum DeploymentMode {
    #[default]
    Production,
    Demo,
}

#[garmin_macros::portable]
pub struct ProfileSnapshot {
    pub user: User,
    pub avatar: Option<ProfileAvatarSnapshot>,
    pub activities: Vec<ActivitySnapshot>,
}

#[garmin_macros::portable(eq)]
pub struct ProfileAvatarSnapshot {
    pub key: String,
    pub thumbnail: Vec<u8>,
}

#[garmin_macros::portable(eq)]
pub struct AvatarUpload {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub crop: AvatarCrop,
}

#[garmin_macros::portable(copy, eq)]
pub struct AvatarCrop {
    pub left: u32,
    pub top: u32,
    pub edge: u32,
}

#[garmin_macros::portable]
pub struct ActivitySnapshot {
    pub id: ObservationId,
    pub source: String,
    pub summary: ActivitySummary,
}

#[garmin_macros::portable]
pub struct ActivityDetailSnapshot {
    pub id: ObservationId,
    pub segments: Vec<Vec<Coordinate>>,
}

#[garmin_macros::portable(copy, eq)]
#[serde(rename_all = "snake_case")]
pub enum InspectionState {
    Running,
    Ready,
    Failed,
}

#[expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates the serialized RPC request type"
)]
mod rpc {
    use super::{
        ActivityDetailSnapshot, AvatarUpload, DeploymentMode, DeviceBrowserDownloadTicket,
        DeviceBrowserRequest, DeviceBrowserTarget, DeviceBrowserUpload, DeviceCatalogSnapshot,
        DeviceFitImportOutcome, DeviceFitPreview, DeviceSnapshot, ProfileAvatarSnapshot,
        ProfileSnapshot,
    };
    use garmin_model::{
        identity::{ProfilePreferences, User, UserId},
        observation::ObservationId,
    };
    use remoc::{rch, rtc};

    #[rtc::remote]
    pub trait ApplicationService {
        async fn deployment_mode(&self) -> Result<DeploymentMode, rtc::CallError>;
        async fn heartbeat(&self) -> Result<(), rtc::CallError>;
        async fn watch_devices(
            &self,
        ) -> Result<rch::watch::Receiver<Vec<DeviceSnapshot>>, rtc::CallError>;
        async fn device_catalog(
            &self,
            device_key: String,
        ) -> Result<Result<DeviceCatalogSnapshot, String>, rtc::CallError>;
        async fn device_browser(
            &self,
            request: DeviceBrowserRequest,
        ) -> Result<Result<DeviceCatalogSnapshot, String>, rtc::CallError>;
        async fn upload_device_browser_file(
            &self,
            request: DeviceBrowserUpload,
            contents: rch::io::Receiver,
        ) -> Result<Result<DeviceCatalogSnapshot, String>, rtc::CallError>;
        async fn prepare_device_browser_download(
            &self,
            device_key: String,
            target: DeviceBrowserTarget,
        ) -> Result<Result<DeviceBrowserDownloadTicket, String>, rtc::CallError>;
        async fn device_fit_preview(
            &self,
            device_key: String,
            target: DeviceBrowserTarget,
        ) -> Result<Result<DeviceFitPreview, String>, rtc::CallError>;
        async fn import_device_fit(
            &self,
            user_id: UserId,
            device_key: String,
            target: DeviceBrowserTarget,
        ) -> Result<Result<DeviceFitImportOutcome, String>, rtc::CallError>;
        async fn profiles(&self) -> Result<Result<Vec<ProfileSnapshot>, String>, rtc::CallError>;
        async fn create_profile(
            &self,
            display_name: String,
        ) -> Result<Result<User, String>, rtc::CallError>;
        async fn update_preferences(
            &self,
            user_id: UserId,
            preferences: ProfilePreferences,
        ) -> Result<Result<User, String>, rtc::CallError>;
        async fn import_avatar(
            &self,
            user_id: UserId,
            upload: AvatarUpload,
        ) -> Result<Result<ProfileAvatarSnapshot, String>, rtc::CallError>;
        async fn activity(
            &self,
            user_id: UserId,
            observation_id: ObservationId,
        ) -> Result<Result<Option<ActivityDetailSnapshot>, String>, rtc::CallError>;
    }
}

pub use rpc::{ApplicationService, ApplicationServiceClient, ApplicationServiceServerShared};

#[cfg(test)]
mod tests {
    use super::{DeviceBrowserTarget, DeviceCatalogEntryKind};

    #[test]
    fn browser_path_wire_format_remains_a_string() {
        let target = DeviceBrowserTarget {
            storage_id: "internal".to_owned(),
            path: "Garmin/Activity/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
        };

        let json = serde_json::to_string(&target).unwrap();
        assert!(json.contains(r#""path":"Garmin/Activity/ride.fit""#));
        assert_eq!(
            serde_json::from_str::<DeviceBrowserTarget>(&json).unwrap(),
            target
        );
    }
}
