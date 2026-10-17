//! Bounded last outcomes, independent of connection and process lifetime.

use anyhow::{Result, bail};
use garmin_service_api::maps::Outcome;
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::Read as _;
use std::io::Write as _;
use std::path::Path;

use super::pending_recovery::{
    set_private_directory_permissions, set_private_file_permissions, sync_directory,
};

const MAX_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_OUTCOMES: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    device: String,
    outcomes: Vec<Outcome>,
}

pub(super) fn read(root: &Path, device: &str) -> Result<Vec<Outcome>> {
    validate_device(device)?;
    let path = root.join(format!("{device}.json"));
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        bail!("invalid map outcome record");
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len())? > MAX_BYTES {
        bail!("map outcome record is too large");
    }
    let record: Record = serde_json::from_slice(&bytes)?;
    if record.version != 1 || record.device != device || record.outcomes.len() > MAX_OUTCOMES {
        bail!("unsupported map outcome record");
    }
    Ok(record.outcomes)
}

pub(super) fn write(root: &Path, device: &str, outcomes: Vec<Outcome>) -> Result<()> {
    validate_device(device)?;
    let record = Record {
        version: 1,
        device: device.to_owned(),
        outcomes,
    };
    let bytes = serde_json::to_vec(&record)?;
    if u64::try_from(bytes.len())? > MAX_BYTES {
        bail!("map outcome record is too large");
    }
    fs::create_dir_all(root)?;
    if !fs::symlink_metadata(root)?.is_dir() {
        bail!("unsafe map outcome directory");
    }
    set_private_directory_permissions(root)?;
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    set_private_file_permissions(temporary.path())?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(root.join(format!("{device}.json")))?;
    sync_directory(root)
}

fn validate_device(device: &str) -> Result<()> {
    if device.len() != 32 || !device.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid device digest for map outcomes");
    }
    Ok(())
}
