//! Each active store owns its files; overlapping processes reuse separate slots.
use std::{
    fs,
    fs::{File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
};

pub(super) fn claim(root: &Path) -> io::Result<(PathBuf, File)> {
    for slot in 0..u32::MAX {
        let directory = if slot == 0 {
            root.to_path_buf()
        } else {
            root.join(format!("writer-{slot}"))
        };
        fs::create_dir_all(&directory)?;
        // Never remove or replace this inode: doing so would bypass a live lock.
        let lease = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("writer.lock"))?;
        match lease.try_lock() {
            Ok(()) => return Ok((directory, lease)),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Err(io::Error::other("no available log writer directory"))
}
