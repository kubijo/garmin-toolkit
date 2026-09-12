//! Typed contracts shared by native hosts and isolated clients.

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
    pub capabilities: Vec<DeviceCapability>,
    pub storages: Vec<DeviceStorageState>,
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
    use super::{ActivityDetailSnapshot, DeploymentMode, DeviceSnapshot, ProfileSnapshot};
    use garmin_model::{
        identity::{ProfilePreferences, User, UserId},
        observation::ObservationId,
    };
    use remoc::{prelude::*, rtc};

    #[rtc::remote]
    pub trait ApplicationService {
        async fn deployment_mode(&self) -> Result<DeploymentMode, rtc::CallError>;
        async fn heartbeat(&self) -> Result<(), rtc::CallError>;
        async fn watch_devices(
            &self,
        ) -> Result<rch::watch::Receiver<Vec<DeviceSnapshot>>, rtc::CallError>;
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
        async fn activity(
            &self,
            user_id: UserId,
            observation_id: ObservationId,
        ) -> Result<Result<Option<ActivityDetailSnapshot>, String>, rtc::CallError>;
    }
}

pub use rpc::{ApplicationService, ApplicationServiceClient, ApplicationServiceServerShared};
