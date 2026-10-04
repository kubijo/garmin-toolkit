//! Portable, consent-gated profile association in the device-state directory.

use std::str::FromStr as _;

use garmin_device::storage::{DeviceIoError, DeviceRead, DeviceWrite};
use garmin_device::{DevicePathState, DevicePathStatus, SafeRelativePath};
use garmin_model::identity::UserId;
use thiserror::Error;
use toml_edit::{DocumentMut, Item, value};
use uuid::Uuid;

use crate::device_state::{DEVICE_STATE_MAGIC, DEVICE_STATE_VERSION, NAMESPACE};

pub const PROFILE_MARKER_PATH: &str = "GARMIN-TOOLKIT/pairing.toml";
const PROFILE_MARKER_LIMIT: u64 = 16 * 1024;
const MAX_REVISIONS: u32 = 64;
const KIND: &str = "pairing";

/// The parsed document is retained so updates preserve comments and unknown fields.
#[derive(Debug, Clone)]
pub struct ProfileMarker {
    document: DocumentMut,
    device_id: Uuid,
    user_id: UserId,
    profile_name: String,
    revision: u32,
}

impl ProfileMarker {
    /// Prepare a new marker. The caller must obtain consent before writing it.
    ///
    /// # Errors
    /// The profile name must be nonblank and the document must fit the marker limit.
    pub fn new(user_id: UserId, profile_name: &str) -> Result<Self, ProfileMarkerError> {
        let profile_name = profile_name.trim();
        if profile_name.is_empty() {
            return Err(ProfileMarkerError::Invalid);
        }
        let mut document = DocumentMut::new();
        let device_id = Uuid::new_v4();
        document["format"]["magic"] = value(DEVICE_STATE_MAGIC);
        document["format"]["kind"] = value(KIND);
        document["format"]["version"] = value(i64::from(DEVICE_STATE_VERSION));
        document["pairing"]["device_id"] = value(device_id.to_string());
        document["pairing"]["user_id"] = value(user_id.to_string());
        document["pairing"]["profile_name"] = value(profile_name);
        document["pairing"]["revision"] = value(0);
        let marker = Self {
            document,
            device_id,
            user_id,
            profile_name: profile_name.to_owned(),
            revision: 0,
        };
        marker.bytes()?;
        Ok(marker)
    }

    /// Parse a bounded marker without discarding unrecognized TOML content.
    ///
    /// # Errors
    /// Malformed, oversized, or unsupported documents are rejected.
    pub fn parse(bytes: &[u8]) -> Result<Self, ProfileMarkerError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > PROFILE_MARKER_LIMIT {
            return Err(ProfileMarkerError::TooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| ProfileMarkerError::Invalid)?;
        let document = DocumentMut::from_str(text).map_err(|_| ProfileMarkerError::Invalid)?;
        if field(&document, "format", "magic") != Some(DEVICE_STATE_MAGIC)
            || field(&document, "format", "kind") != Some(KIND)
        {
            return Err(ProfileMarkerError::Invalid);
        }
        let version = document
            .get("format")
            .and_then(|format| format.get("version"))
            .and_then(Item::as_integer)
            .ok_or(ProfileMarkerError::Invalid)?;
        if version != i64::from(DEVICE_STATE_VERSION) {
            return Err(ProfileMarkerError::UnsupportedVersion(version));
        }
        let device_id = field(&document, "pairing", "device_id")
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(ProfileMarkerError::Invalid)?;
        let user_id = field(&document, "pairing", "user_id")
            .and_then(|value| value.parse::<UserId>().ok())
            .ok_or(ProfileMarkerError::Invalid)?;
        let profile_name = field(&document, "pairing", "profile_name")
            .filter(|name| !name.trim().is_empty())
            .ok_or(ProfileMarkerError::Invalid)?
            .to_owned();
        let revision = document
            .get("pairing")
            .and_then(|pairing| pairing.get("revision"))
            .and_then(Item::as_integer)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value <= MAX_REVISIONS)
            .ok_or(ProfileMarkerError::Invalid)?;
        Ok(Self {
            document,
            device_id,
            user_id,
            profile_name,
            revision,
        })
    }

    #[must_use]
    pub const fn device_id(&self) -> Uuid {
        self.device_id
    }

    #[must_use]
    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    #[must_use]
    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    #[must_use]
    pub const fn revision(&self) -> u32 {
        self.revision
    }

    /// Preserve the original document and device UUID in a new, create-only revision.
    /// # Errors
    /// The revision limit and size limit must be respected.
    pub fn reassign(
        &self,
        user_id: UserId,
        profile_name: &str,
    ) -> Result<Self, ProfileMarkerError> {
        let profile_name = profile_name.trim();
        if profile_name.is_empty() {
            return Err(ProfileMarkerError::Invalid);
        }
        let revision = self
            .revision
            .checked_add(1)
            .filter(|value| *value <= MAX_REVISIONS)
            .ok_or(ProfileMarkerError::TooManyRevisions)?;
        let mut document = self.document.clone();
        document["pairing"]["user_id"] = value(user_id.to_string());
        document["pairing"]["profile_name"] = value(profile_name);
        document["pairing"]["revision"] = value(i64::from(revision));
        let marker = Self {
            document,
            device_id: self.device_id,
            user_id,
            profile_name: profile_name.to_owned(),
            revision,
        };
        marker.bytes()?;
        Ok(marker)
    }

    /// Serialize without reconstructing the document from its known fields.
    ///
    /// # Errors
    /// The document must fit the marker limit.
    pub fn bytes(&self) -> Result<Vec<u8>, ProfileMarkerError> {
        let bytes = self.document.to_string().into_bytes();
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > PROFILE_MARKER_LIMIT {
            return Err(ProfileMarkerError::TooLarge);
        }
        Ok(bytes)
    }
}

fn field<'a>(document: &'a DocumentMut, table: &str, key: &str) -> Option<&'a str> {
    document.get(table)?.get(key)?.as_str()
}

/// Read the pairing document without changing device contents.
///
/// # Errors
/// The marker must be a bounded, supported regular file.
pub async fn read_profile_marker<D: DeviceRead + ?Sized>(
    device: &D,
    storage: &str,
) -> Result<Option<ProfileMarker>, ProfileMarkerError> {
    let path = SafeRelativePath::parse(PROFILE_MARKER_PATH)?;
    let initial = device
        .read_bounded_file(storage, &path, PROFILE_MARKER_LIMIT)
        .await
        .map_err(|error| match error {
            DeviceIoError::LimitExceeded(_) => ProfileMarkerError::TooLarge,
            error => ProfileMarkerError::Device(error),
        })?
        .map(|bytes| ProfileMarker::parse(&bytes))
        .transpose()?;
    if initial.as_ref().is_some_and(|marker| marker.revision != 0) {
        return Err(ProfileMarkerError::Invalid);
    }
    let namespace = SafeRelativePath::parse(NAMESPACE)?;
    let entries = match device.inspect(storage, &namespace).await? {
        DevicePathStatus::Missing if initial.is_none() => return Ok(None),
        DevicePathStatus::Directory => device.list_directory(storage, &namespace).await?,
        _ => return Err(ProfileMarkerError::Invalid),
    };
    let mut revisions = Vec::new();
    let mut base_count = 0;
    for entry in entries {
        let folded = entry.name.to_ascii_lowercase();
        if folded == "pairing.toml" && entry.name != "pairing.toml" {
            return Err(ProfileMarkerError::Invalid);
        }
        if folded == "pairing.toml" {
            base_count += 1;
            if base_count > 1
                || entry.state != DevicePathState::RegularFile
                || entry.size.is_none_or(|size| size > PROFILE_MARKER_LIMIT)
            {
                return Err(ProfileMarkerError::Invalid);
            }
            continue;
        }
        if !folded.starts_with("pairing-") {
            continue;
        }
        if entry.name != folded {
            return Err(ProfileMarkerError::Invalid);
        }
        let Some(number) = entry
            .name
            .strip_prefix("pairing-")
            .and_then(|name| name.strip_suffix(".toml"))
            .filter(|name| name.len() == 6 && name.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|name| name.parse::<u32>().ok())
            .filter(|number| (1..=MAX_REVISIONS).contains(number))
        else {
            return Err(ProfileMarkerError::Invalid);
        };
        if entry.state != DevicePathState::RegularFile
            || entry.size.is_none_or(|size| size > PROFILE_MARKER_LIMIT)
        {
            return Err(ProfileMarkerError::Invalid);
        }
        revisions.push(number);
    }
    if (base_count == 1) != initial.is_some() {
        return Err(ProfileMarkerError::Invalid);
    }
    revisions.sort_unstable();
    if revisions.is_empty() {
        return Ok(initial);
    }
    let Some(mut current) = initial else {
        return Err(ProfileMarkerError::Invalid);
    };
    for (index, number) in revisions.into_iter().enumerate() {
        if number != u32::try_from(index + 1).unwrap_or(u32::MAX) {
            return Err(ProfileMarkerError::Invalid);
        }
        let bytes = device
            .read_bounded_file(storage, &revision_path(number)?, PROFILE_MARKER_LIMIT)
            .await?
            .ok_or(ProfileMarkerError::Invalid)?;
        let next = ProfileMarker::parse(&bytes)?;
        if next.revision != number || next.device_id != current.device_id {
            return Err(ProfileMarkerError::Invalid);
        }
        current = next;
    }
    Ok(Some(current))
}

fn revision_path(revision: u32) -> Result<SafeRelativePath, ProfileMarkerError> {
    Ok(SafeRelativePath::parse(format!(
        "{NAMESPACE}/pairing-{revision:06}.toml"
    ))?)
}

/// Create only an absent marker, then read and compare its exact content.
/// The caller owns mutation consent and the device-wide write lock.
///
/// # Errors
/// Existing, changed, unreadable, and unverified markers are never replaced.
pub async fn create_profile_marker<D: DeviceWrite + ?Sized>(
    device: &D,
    storage: &str,
    marker: &ProfileMarker,
) -> Result<(), ProfileMarkerError> {
    let path = SafeRelativePath::parse(PROFILE_MARKER_PATH)?;
    let bytes = marker.bytes()?;
    if marker.revision != 0 {
        return Err(ProfileMarkerError::Invalid);
    }
    if let Some(existing) = read_profile_marker(device, storage).await? {
        return if existing.revision == 0 && existing.bytes()? == bytes {
            Ok(())
        } else {
            Err(ProfileMarkerError::Conflict)
        };
    }
    device
        .ensure_directory(storage, &SafeRelativePath::parse(NAMESPACE)?)
        .await?;
    device.create_verified_file(storage, &path, &bytes).await?;
    let observed = device
        .read_bounded_file(storage, &path, PROFILE_MARKER_LIMIT)
        .await?
        .ok_or(ProfileMarkerError::MissingAfterWrite)?;
    if observed != bytes {
        return Err(ProfileMarkerError::Conflict);
    }
    ProfileMarker::parse(&observed)?;
    Ok(())
}

/// Reassociate by creating and verifying the next revision; prior markers remain intact.
/// The caller must hold the device mutation gate and obtain explicit consent.
/// # Errors
/// The observed marker must still match the reviewed device UUID and revision.
pub async fn reassign_profile_marker<D: DeviceWrite + ?Sized>(
    device: &D,
    storage: &str,
    expected_device_id: Uuid,
    expected_revision: u32,
    user_id: UserId,
    profile_name: &str,
) -> Result<ProfileMarker, ProfileMarkerError> {
    let current = read_profile_marker(device, storage)
        .await?
        .ok_or(ProfileMarkerError::Conflict)?;
    if current.device_id != expected_device_id || current.revision != expected_revision {
        return Err(ProfileMarkerError::Conflict);
    }
    if current.user_id == user_id {
        return Ok(current);
    }
    let next = current.reassign(user_id, profile_name)?;
    let path = revision_path(next.revision)?;
    let bytes = next.bytes()?;
    device.create_verified_file(storage, &path, &bytes).await?;
    let observed = read_profile_marker(device, storage)
        .await?
        .ok_or(ProfileMarkerError::MissingAfterWrite)?;
    if observed.bytes()? != bytes {
        return Err(ProfileMarkerError::Conflict);
    }
    Ok(observed)
}

#[derive(Debug, Error)]
pub enum ProfileMarkerError {
    #[error(transparent)]
    Device(#[from] DeviceIoError),
    #[error(transparent)]
    Path(#[from] garmin_device::PathSafetyError),
    #[error("invalid profile marker")]
    Invalid,
    #[error("unsupported profile marker version {0}")]
    UnsupportedVersion(i64),
    #[error("profile marker exceeds 16 KiB")]
    TooLarge,
    #[error("profile marker conflicts with the reviewed content")]
    Conflict,
    #[error("profile marker is missing after creation")]
    MissingAfterWrite,
    #[error("profile marker has too many revisions")]
    TooManyRevisions,
}

#[cfg(test)]
mod tests {
    use super::*;
    use garmin_device::storage::DirectoryDevice;

    #[test]
    fn parsing_preserves_comments_and_unknown_fields() {
        let first = UserId::new_v4();
        let text = format!(
            "# Keep this note\n[format]\nmagic = \"{DEVICE_STATE_MAGIC}\"\nkind = \"{KIND}\"\nversion = 1\n\n[pairing]\ndevice_id = \"{}\"\nuser_id = \"{first}\"\nprofile_name = \"First\"\nrevision = 0\nextra = \"keep\"\n",
            Uuid::new_v4()
        );
        let marker = ProfileMarker::parse(text.as_bytes()).unwrap();
        let bytes = marker.bytes().unwrap();
        let output = String::from_utf8(bytes.clone()).unwrap();
        assert!(output.contains("# Keep this note"));
        assert!(output.contains("extra = \"keep\""));
        assert_eq!(ProfileMarker::parse(&bytes).unwrap().user_id(), first);
        let revised = marker.reassign(UserId::new_v4(), "Second").unwrap();
        let updated = String::from_utf8(revised.bytes().unwrap()).unwrap();
        assert!(updated.contains("# Keep this note"));
        assert!(updated.contains("extra = \"keep\""));
        assert_eq!(revised.device_id(), marker.device_id());
        assert_eq!(revised.revision(), 1);
    }

    #[test]
    fn missing_fields_and_future_versions_fail_closed() {
        assert!(matches!(
            ProfileMarker::parse(b"format = 1"),
            Err(ProfileMarkerError::Invalid)
        ));
        let marker = ProfileMarker::new(UserId::new_v4(), "First").unwrap();
        let future = String::from_utf8(marker.bytes().unwrap())
            .unwrap()
            .replace("version = 1", "version = 2");
        assert!(matches!(
            ProfileMarker::parse(future.as_bytes()),
            Err(ProfileMarkerError::UnsupportedVersion(2))
        ));
    }

    #[tokio::test]
    async fn creation_is_verified_and_conflicting_marker_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let device = DirectoryDevice::new(root.path().to_owned());
        let first = ProfileMarker::new(UserId::new_v4(), "First").unwrap();
        create_profile_marker(&device, "primary", &first)
            .await
            .unwrap();
        create_profile_marker(&device, "primary", &first)
            .await
            .unwrap();
        let original = first.bytes().unwrap();
        let second = ProfileMarker::new(UserId::new_v4(), "Second").unwrap();
        assert!(matches!(
            create_profile_marker(&device, "primary", &second).await,
            Err(ProfileMarkerError::Conflict)
        ));
        assert_eq!(
            std::fs::read(root.path().join(PROFILE_MARKER_PATH)).unwrap(),
            original
        );
        assert_eq!(
            read_profile_marker(&device, "primary")
                .await
                .unwrap()
                .unwrap()
                .device_id(),
            first.device_id()
        );
    }

    #[tokio::test]
    async fn reassociation_preserves_the_original_and_rejects_stale_or_broken_history() {
        let root = tempfile::tempdir().unwrap();
        let device = DirectoryDevice::new(root.path().to_owned());
        let first = ProfileMarker::new(UserId::new_v4(), "First").unwrap();
        create_profile_marker(&device, "primary", &first)
            .await
            .unwrap();
        let original = first.bytes().unwrap();
        let second_user = UserId::new_v4();
        let second = reassign_profile_marker(
            &device,
            "primary",
            first.device_id(),
            0,
            second_user,
            "Second",
        )
        .await
        .unwrap();
        assert_eq!(second.device_id(), first.device_id());
        assert_eq!(second.user_id(), second_user);
        assert_eq!(second.revision(), 1);
        assert_eq!(
            std::fs::read(root.path().join(PROFILE_MARKER_PATH)).unwrap(),
            original
        );
        assert!(matches!(
            reassign_profile_marker(
                &device,
                "primary",
                first.device_id(),
                0,
                first.user_id(),
                "First"
            )
            .await,
            Err(ProfileMarkerError::Conflict)
        ));
        assert_eq!(
            read_profile_marker(&device, "primary")
                .await
                .unwrap()
                .unwrap()
                .user_id(),
            second_user
        );
        let revision_path = root.path().join("GARMIN-TOOLKIT/pairing-000001.toml");
        let valid_revision = std::fs::read(&revision_path).unwrap();
        let changed_device = String::from_utf8(valid_revision.clone())
            .unwrap()
            .replace(&first.device_id().to_string(), &Uuid::new_v4().to_string());
        std::fs::write(&revision_path, changed_device).unwrap();
        assert!(matches!(
            read_profile_marker(&device, "primary").await,
            Err(ProfileMarkerError::Invalid)
        ));
        std::fs::write(&revision_path, valid_revision).unwrap();
        std::fs::remove_file(root.path().join("GARMIN-TOOLKIT/pairing-000001.toml")).unwrap();
        std::fs::write(
            root.path().join("GARMIN-TOOLKIT/pairing-000002.toml"),
            second.bytes().unwrap(),
        )
        .unwrap();
        assert!(matches!(
            read_profile_marker(&device, "primary").await,
            Err(ProfileMarkerError::Invalid)
        ));
    }
}
