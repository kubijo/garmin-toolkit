//! Typed contracts shared by native hosts and isolated clients.

use garmin_model::device::DeviceStorageState;
use remoc::rtc;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

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
    Available,
    Running,
    Ready,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum InspectionError {
    Rejected(String),
    Call(rtc::CallError),
}

impl From<rtc::CallError> for InspectionError {
    fn from(error: rtc::CallError) -> Self {
        Self::Call(error)
    }
}

impl fmt::Display for InspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => reason.fmt(formatter),
            Self::Call(error) => error.fmt(formatter),
        }
    }
}

impl Error for InspectionError {}

#[expect(
    clippy::unsafe_derive_deserialize,
    reason = "Remoc generates the serialized RPC request type"
)]
mod rpc {
    use super::{DeviceSnapshot, InspectionError};
    use remoc::{prelude::*, rtc};

    #[rtc::remote]
    pub trait DeviceService {
        async fn watch(&self) -> Result<rch::watch::Receiver<Vec<DeviceSnapshot>>, rtc::CallError>;

        async fn inspect(&self, key: String) -> Result<(), InspectionError>;
    }
}

pub use rpc::{DeviceService, DeviceServiceClient, DeviceServiceServerShared};
