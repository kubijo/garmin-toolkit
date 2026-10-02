use std::fs;
use std::io;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

use super::Error;

const SELECTED: &str = "storage-selection.json";
const PENDING: &str = "storage-restore.json";
const GENERATIONS: &str = "storage-generations";
const MAX_RECORD: u64 = 4096;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(super) enum Location {
    #[default]
    Original,
    Generation(Uuid),
}

impl Location {
    pub(super) fn path(self, root: &Path) -> PathBuf {
        match self {
            Self::Original => root.join("storage.sqlite3"),
            Self::Generation(id) => root
                .join(GENERATIONS)
                .join(id.to_string())
                .join("storage.sqlite3"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub active: Location,
    pub rollback: Option<Location>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub previous: Selection,
    pub next: Location,
}

pub(super) fn read_selection(root: &Path) -> Result<Selection, Error> {
    if !has_selection(root)? && root.join(GENERATIONS).try_exists()? {
        return Err(Error::Recovery("storage selection is missing"));
    }
    Ok(read_record(&root.join(SELECTED))?.unwrap_or_default())
}

pub(super) fn has_selection(root: &Path) -> Result<bool, Error> {
    Ok(root.join(SELECTED).try_exists()?)
}

pub(super) fn housekeeping(root: &Path, selection: Selection) -> Result<(), Error> {
    if !has_selection(root)? {
        select(root, selection)?;
    }
    prune(root, selection)?;
    match fs::remove_dir_all(root.join(".tmp/snapshots")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn recover(root: &Path) -> Result<bool, Error> {
    let Some(pending) = read_record::<Pending>(&root.join(PENDING))? else {
        return Ok(false);
    };
    // Never open/create an empty replacement when the rollback database is missing.
    if !pending.previous.active.path(root).is_file() {
        return Err(Error::Recovery("previous database is missing"));
    }
    select(root, pending.previous)?;
    finish(root)?;
    Ok(true)
}

pub(super) fn prepare(root: &Path, previous: Selection, next: Location) -> Result<(), Error> {
    if root.join(PENDING).try_exists()? {
        return Err(Error::Recovery("an unfinished restore requires recovery"));
    }
    write_record(root, PENDING, &Pending { previous, next })
}

pub(super) fn select(root: &Path, selection: Selection) -> Result<(), Error> {
    write_record(root, SELECTED, &selection)
}

pub(super) fn finish(root: &Path) -> Result<(), Error> {
    fs::remove_file(root.join(PENDING))?;
    sync_directory(root)?;
    Ok(())
}

pub(super) fn generation(root: &Path) -> Result<Location, Error> {
    let generations = root.join(GENERATIONS);
    fs::create_dir_all(&generations)?;
    let id = Uuid::new_v4();
    fs::create_dir(generations.join(id.to_string()))?;
    sync_directory(&generations)?;
    sync_directory(root)?;
    Ok(Location::Generation(id))
}

pub(super) fn prune(root: &Path, selection: Selection) -> Result<(), Error> {
    let directory = root.join(GENERATIONS);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<Uuid>().ok())
        else {
            continue;
        };
        let location = Location::Generation(id);
        if location != selection.active
            && Some(location) != selection.rollback
            && entry.file_type()?.is_dir()
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    sync_directory(&directory)?;
    Ok(())
}

fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Error> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD {
        return Err(Error::Recovery("restore journal exceeds its limit"));
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn write_record(root: &Path, name: &str, record: &impl Serialize) -> Result<(), Error> {
    let mut temporary = NamedTempFile::new_in(root)?;
    serde_json::to_writer(&mut temporary, record)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(root.join(name))
        .map_err(|error| error.error)?;
    sync_directory(root)?;
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
