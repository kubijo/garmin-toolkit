//! Host filesystem behavior needed by update transactions.

use std::io;
use std::path::Path;

pub(crate) trait HostFilesystem {
    fn sync_directory(&self, path: &Path) -> io::Result<()>;
}

pub(crate) struct SystemFilesystem;

#[cfg(unix)]
impl HostFilesystem for SystemFilesystem {
    fn sync_directory(&self, path: &Path) -> io::Result<()> {
        std::fs::File::open(path)?.sync_all()
    }
}

#[cfg(not(unix))]
impl HostFilesystem for SystemFilesystem {
    fn sync_directory(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}
