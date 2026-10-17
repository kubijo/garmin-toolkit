//! Directory-at-a-time access to the filesystem visible inside the HASS host.

use garmin_service_api::{DeviceCatalogEntry, DeviceCatalogEntryKind, files::Directory};
use std::path::Path;

pub(super) fn directory(path: &Path) -> Result<Directory, String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    for entry in path.read_dir().map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let Ok(metadata) = entry.path().metadata() else {
            continue;
        };
        if !metadata.is_file() && !metadata.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "file name is not valid UTF-8")?;
        if metadata.is_file() && !name.ends_with(".tar.zst") {
            continue;
        }
        if entries.len() >= 10_000 {
            return Err("directory exceeds the 10,000-entry browser limit".into());
        }
        entries.push(DeviceCatalogEntry {
            path: name.into(),
            kind: if metadata.is_dir() {
                DeviceCatalogEntryKind::Directory
            } else {
                DeviceCatalogEntryKind::File
            },
            size: metadata.is_file().then_some(metadata.len()),
        });
    }
    Ok(Directory {
        path: path
            .to_str()
            .ok_or("directory path is not valid UTF-8")?
            .into(),
        entries,
    })
}
