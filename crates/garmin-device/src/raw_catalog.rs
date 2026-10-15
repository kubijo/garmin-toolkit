//! Direct USB MTP attachment discovery and bounded browser snapshots.

use std::collections::BTreeSet;

use garmin_progress::ProgressReporter;
use mtp_rs::{ObjectHandle, Storage};
use sha2::{Digest as _, Sha256};

use crate::{
    DeviceCatalog, DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogStorage,
    SafeRelativePath, attachments,
};

const MAX_DEPTH: usize = 32;
const MAX_ENTRIES: usize = 4_096;

#[derive(Clone)]
pub struct RawMtpCandidate {
    pub location_id: u64,
    key: String,
    name: String,
}

impl RawMtpCandidate {
    fn new(candidate: crate::MtpCandidate, salt: &[u8; 16]) -> Self {
        let mut digest = Sha256::new();
        digest.update(salt);
        digest.update(candidate.location_id.to_le_bytes());
        Self {
            location_id: candidate.location_id,
            key: format!("mtp:{}", hex::encode(&digest.finalize()[..16])),
            name: candidate
                .product
                .unwrap_or_else(|| "Garmin MTP device".to_owned()),
        }
    }

    /// Inspect the selected USB attachment without a desktop mount.
    /// # Errors
    /// The manifest or MTP storage cannot be read.
    pub async fn inspect_async(&self) -> Result<attachments::Metadata, String> {
        let manifest = crate::open_mtp(self.location_id)
            .await
            .map_err(|error| error.to_string())?;
        let parsed = manifest.capabilities();
        let capabilities = parsed
            .capabilities()
            .iter()
            .map(|capability| {
                attachments::Capability::new(capability.data_type(), capability.direction())
            })
            .collect();
        Ok(attachments::Metadata {
            id: Some(parsed.id()),
            device_digest: Some(manifest.identity_digest()),
            report: None,
            name: parsed.model().description().to_owned(),
            software_version: Some(parsed.model().software_version()),
            capabilities,
            storage: crate::DeviceStateSnapshot::default(),
        })
    }

    async fn browse_async(&self, progress: &ProgressReporter) -> Result<DeviceCatalog, String> {
        let device = crate::mtp::open_raw_mtp(self.location_id)
            .await
            .map_err(|error| error.to_string())?;
        let result = async {
            let mut remaining = MAX_ENTRIES;
            let mut result = Vec::new();
            for (index, storage) in device
                .storages()
                .await
                .map_err(|error| error.to_string())?
                .into_iter()
                .enumerate()
            {
                ensure_active(progress)?;
                let info = storage.info();
                let entries = browse_storage(&storage, &mut remaining, progress).await?;
                result.push(DeviceCatalogStorage {
                    id: format!("{:016x}", storage.id().0),
                    label: if info.description.is_empty() {
                        format!("MTP storage {}", index + 1)
                    } else {
                        info.description.clone()
                    },
                    entries,
                });
            }
            if result.is_empty() {
                return Err("device exposed no browsable MTP storage".to_owned());
            }
            Ok(DeviceCatalog { storages: result })
        }
        .await;
        let close = device.close().await;
        match (result, close) {
            (Ok(catalog), Ok(())) => Ok(catalog),
            (Ok(_), Err(error)) => Err(error.to_string()),
            (Err(error), _) => Err(error),
        }
    }
}

impl attachments::Candidate for RawMtpCandidate {
    fn key(&self) -> &str {
        &self.key
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn inspect(&self) -> Result<attachments::Metadata, String> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?
            .block_on(self.inspect_async())
    }

    fn browse_with_progress(&self, progress: &ProgressReporter) -> Result<DeviceCatalog, String> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?
            .block_on(self.browse_async(progress))
    }
}

async fn browse_storage(
    storage: &Storage,
    remaining: &mut usize,
    progress: &ProgressReporter,
) -> Result<Vec<DeviceCatalogEntry>, String> {
    let mut pending: Vec<(Option<ObjectHandle>, String, usize)> = vec![(None, String::new(), 0)];
    let mut entries = Vec::new();
    while let Some((parent, prefix, depth)) = pending.pop() {
        ensure_active(progress)?;
        let mut names = BTreeSet::new();
        for object in storage
            .list_objects(parent)
            .await
            .map_err(|error| error.to_string())?
        {
            ensure_active(progress)?;
            if *remaining == 0 {
                return Err(format!(
                    "device browser exceeded its {MAX_ENTRIES}-entry limit"
                ));
            }
            *remaining -= 1;
            if !names.insert(object.filename.to_lowercase()) {
                return Err(format!("device directory {prefix} is ambiguous"));
            }
            let path = if prefix.is_empty() {
                object.filename.clone()
            } else {
                format!("{prefix}/{}", object.filename)
            };
            let path = SafeRelativePath::parse(&path)
                .map_err(|error| error.to_string())?
                .into_utf8_path_buf();
            let (kind, size) = if object.is_folder() {
                if depth >= MAX_DEPTH {
                    return Err(format!(
                        "device browser exceeded its {MAX_DEPTH}-level depth limit"
                    ));
                }
                pending.push((Some(object.handle), path.to_string(), depth + 1));
                (DeviceCatalogEntryKind::Directory, None)
            } else if object.is_file() {
                (DeviceCatalogEntryKind::File, Some(object.size))
            } else {
                continue;
            };
            entries.push(DeviceCatalogEntry { path, kind, size });
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn ensure_active(progress: &ProgressReporter) -> Result<(), String> {
    if progress.is_cancelled() {
        Err("device browsing was cancelled".to_owned())
    } else {
        Ok(())
    }
}

pub struct RawMtpMonitor {
    salt: [u8; 16],
}

impl RawMtpMonitor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            salt: *uuid::Uuid::new_v4().as_bytes(),
        }
    }
}

impl Default for RawMtpMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl attachments::Backend for RawMtpMonitor {
    type Candidate = RawMtpCandidate;

    fn poll_changed(&self) -> bool {
        false
    }

    fn candidates(&self) -> Vec<Self::Candidate> {
        crate::discover_mtp_candidates()
            .into_iter()
            .map(|candidate| RawMtpCandidate::new(candidate, &self.salt))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, time::Duration};

    use mtp_rs::{MtpDevice, VirtualDeviceConfig, VirtualStorageConfig};

    use super::*;

    #[test]
    fn attachment_key_does_not_expose_usb_location() {
        let candidate = crate::MtpCandidate {
            location_id: 0x1234,
            vendor_id: crate::GARMIN_USB_VENDOR_ID,
            product_id: 0x51b4,
            product: None,
            speed: None,
        };
        let first = RawMtpCandidate::new(candidate.clone(), &[1; 16]);
        let same = RawMtpCandidate::new(candidate.clone(), &[1; 16]);
        let other_session = RawMtpCandidate::new(candidate, &[2; 16]);
        assert_eq!(first.key, same.key);
        assert_ne!(first.key, other_session.key);
        assert!(!first.key.contains("1234"));
    }

    #[tokio::test]
    async fn browser_reads_root_and_nested_entries_from_virtual_mtp() -> Result<(), Box<dyn Error>>
    {
        let backing = tempfile::tempdir()?;
        fs::create_dir(backing.path().join("Garmin"))?;
        fs::write(backing.path().join("Garmin/course.fit"), b"fit bytes")?;
        let device = MtpDevice::builder()
            .open_virtual(VirtualDeviceConfig {
                storages: vec![VirtualStorageConfig {
                    description: "Internal Storage".to_owned(),
                    capacity: 1_048_576,
                    backing_dir: backing.path().to_path_buf(),
                    read_only: true,
                }],
                event_poll_interval: Duration::ZERO,
                watch_backing_dirs: false,
                ..VirtualDeviceConfig::default()
            })
            .await?;
        let storage = device.storages().await?.remove(0);
        let mut remaining = MAX_ENTRIES;
        let entries =
            browse_storage(&storage, &mut remaining, &ProgressReporter::default()).await?;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path.as_str(), "Garmin");
        assert_eq!(entries[0].kind, DeviceCatalogEntryKind::Directory);
        assert_eq!(entries[1].path.as_str(), "Garmin/course.fit");
        assert_eq!(entries[1].kind, DeviceCatalogEntryKind::File);
        assert_eq!(entries[1].size, Some(9));
        device.close().await?;
        Ok(())
    }
}
