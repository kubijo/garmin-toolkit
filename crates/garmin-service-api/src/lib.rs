//! Typed contracts shared by native hosts and isolated clients.

use garmin_model::device::DeviceStorageState;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceSnapshot {
    pub key: String,
    pub name: String,
    pub identifier: Option<u32>,
    pub software_version: Option<u16>,
    pub inspection: InspectionState,
    pub storages: Vec<DeviceStorageState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
    use super::DeviceSnapshot;
    use remoc::{prelude::*, rtc};

    #[rtc::remote]
    pub trait DeviceService {
        async fn watch(&self) -> Result<rch::watch::Receiver<Vec<DeviceSnapshot>>, rtc::CallError>;
    }
}

pub use rpc::{DeviceService, DeviceServiceClient, DeviceServiceServerShared};
