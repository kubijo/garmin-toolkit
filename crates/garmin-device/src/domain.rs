//! Device identity, inventory, transport, and path-safety types.

use crate::capabilities::Manifest as DeviceCapabilities;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// How a Garmin device is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    MassStorage,
    Mtp,
    MountedMtp,
}

/// Read-only metadata for one declared device path in one storage location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePathInspection {
    pub storage_id: String,
    pub storage_label: String,
    pub path: SafeRelativePath,
    pub state: DevicePathState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePathState {
    Missing,
    RegularFile,
    Directory,
    Other,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePathStatus {
    Missing,
    RegularFile { size: u64 },
    Directory,
    Other,
    Ambiguous,
}

impl DevicePathStatus {
    #[must_use]
    pub const fn from_parts(state: DevicePathState, size: Option<u64>) -> Option<Self> {
        match (state, size) {
            (DevicePathState::Missing, None) => Some(Self::Missing),
            (DevicePathState::RegularFile, Some(size)) => Some(Self::RegularFile { size }),
            (DevicePathState::Directory, None) => Some(Self::Directory),
            (DevicePathState::Other, None) => Some(Self::Other),
            (DevicePathState::Ambiguous, None) => Some(Self::Ambiguous),
            _ => None,
        }
    }

    #[must_use]
    pub const fn state(self) -> DevicePathState {
        match self {
            Self::Missing => DevicePathState::Missing,
            Self::RegularFile { .. } => DevicePathState::RegularFile,
            Self::Directory => DevicePathState::Directory,
            Self::Other => DevicePathState::Other,
            Self::Ambiguous => DevicePathState::Ambiguous,
        }
    }

    #[must_use]
    pub const fn size(self) -> Option<u64> {
        match self {
            Self::RegularFile { size } => Some(size),
            Self::Missing | Self::Directory | Self::Other | Self::Ambiguous => None,
        }
    }

    #[must_use]
    pub const fn into_parts(self) -> (DevicePathState, Option<u64>) {
        (self.state(), self.size())
    }
}

/// Read-only inventory returned by a physical-device adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInventory {
    pub transport: TransportKind,
    pub paths: Vec<DevicePathInspection>,
}

/// Public, redacted summary of a detected device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub transport: TransportKind,
    pub model: String,
    pub part_number: Option<String>,
    pub software_version: Option<String>,
    pub location: String,
}

/// Device manifest retained exactly for Garmin service requests.
#[derive(Debug, Clone)]
pub struct DeviceManifest {
    pub summary: DeviceSummary,
    capabilities: DeviceCapabilities,
}

impl DeviceManifest {
    #[must_use]
    pub fn new(summary: DeviceSummary, capabilities: DeviceCapabilities) -> Self {
        Self {
            summary,
            capabilities,
        }
    }

    #[must_use]
    pub fn raw_xml(&self) -> &str {
        self.capabilities.raw_xml()
    }

    #[must_use]
    pub const fn capabilities(&self) -> &DeviceCapabilities {
        &self.capabilities
    }

    #[must_use]
    pub fn identity_digest(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.capabilities.id().to_string());
        hex_digest(&hash.finalize())[..32].to_owned()
    }
}
/// A relative destination proven not to escape a selected device root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeRelativePath(PathBuf);

impl Serialize for SafeRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(
            self.0
                .to_str()
                .expect("safe relative paths are validated as Unicode"),
        )
    }
}

impl<'de> Deserialize<'de> for SafeRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for SafeRelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(formatter)
    }
}

impl SafeRelativePath {
    /// Validate an untrusted device-relative path.
    /// # Errors
    /// [`PathSafetyError`] for empty, absolute, or non-normal paths.
    pub fn parse(value: impl AsRef<Path>) -> Result<Self, PathSafetyError> {
        let value = value.as_ref();
        if value.as_os_str().is_empty()
            || value.is_absolute()
            || value.to_str().is_none_or(|value| value.contains('\0'))
        {
            return Err(PathSafetyError::Unsafe(value.to_path_buf()));
        }
        if value
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(PathSafetyError::Unsafe(value.to_path_buf()));
        }
        Ok(Self(value.to_path_buf()))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    #[must_use]
    pub fn under(&self, root: &Path) -> PathBuf {
        root.join(&self.0)
    }
}

#[derive(Debug, Error)]
pub enum PathSafetyError {
    #[error("unsafe device-relative path: {0}")]
    Unsafe(PathBuf),
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_that_escape_device() {
        for unsafe_path in [
            "../secret",
            "/etc/passwd",
            "Garmin/../secret",
            "Garmin/bad\0name.img",
            "",
        ] {
            assert!(
                SafeRelativePath::parse(unsafe_path).is_err(),
                "{unsafe_path}"
            );
        }
    }

    #[test]
    fn accepts_normal_device_path() {
        let path = SafeRelativePath::parse("Garmin/gmapprom.img").unwrap();
        assert_eq!(path.as_path(), Path::new("Garmin/gmapprom.img"));
    }

    #[test]
    fn deserialization_preserves_the_safe_path_invariant() {
        assert!(serde_json::from_str::<SafeRelativePath>(r#""../outside""#).is_err());
        let path = serde_json::from_str::<SafeRelativePath>(r#""Garmin/map.img""#).unwrap();
        assert_eq!(path.as_path(), Path::new("Garmin/map.img"));
    }
}
