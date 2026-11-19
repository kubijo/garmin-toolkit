//! File-manager requests shared with the process that owns the deployment.

use std::fs;
use std::io::{self, Write as _};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

pub(crate) struct Inbox(PathBuf);

impl Inbox {
    pub(crate) fn new(data_root: &Path) -> Self {
        Self(data_root.join("open-files"))
    }

    pub(crate) fn enqueue(&self, files: Vec<PathBuf>) -> io::Result<()> {
        let cwd = std::env::current_dir()?;
        let paths = files
            .into_iter()
            .map(|file| cwd.join(file).into_os_string().into_vec())
            .collect::<Vec<_>>();
        let message = serde_json::to_vec(&paths)?;
        if message.len() > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "too many paths in the open request",
            ));
        }
        fs::create_dir_all(&self.0)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.0)?;
        temporary.write_all(&message)?;
        temporary
            .persist_noclobber(self.0.join(format!("{}.json", uuid::Uuid::new_v4())))
            .map_err(|error| error.error)?;
        Ok(())
    }

    /// Called only by the process that holds the deployment lock.
    pub(crate) fn take(&self) -> io::Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.0) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file()
                || path
                    .extension()
                    .is_none_or(|value| value.as_bytes() != b"json")
            {
                continue;
            }
            let bytes = (entry.metadata()?.len() <= MAX_REQUEST_BYTES as u64)
                .then(|| fs::read(&path))
                .transpose()?;
            fs::remove_file(path)?;
            if let Some(message) = bytes
                && let Ok(files) = serde_json::from_slice::<Vec<Vec<u8>>>(&message)
            {
                paths.extend(
                    files
                        .into_iter()
                        .map(|file| PathBuf::from(std::ffi::OsString::from_vec(file))),
                );
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_paths_are_consumed_once_and_preserve_non_utf8() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let inbox = Inbox::new(directory.path());
        let file = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"ride \xff.fit".to_vec()));
        inbox.enqueue(vec![file.clone()])?;
        assert_eq!(inbox.take()?, [file]);
        assert_eq!(inbox.take()?, [] as [PathBuf; 0]);
        Ok(())
    }
}
