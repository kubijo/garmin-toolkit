//! Host filesystem guarantees for capture artifacts.

use std::fs;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod implementation {
    use super::{Path, fs, io};
    #[cfg(test)]
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    pub fn private_directory_builder(recursive: bool) -> fs::DirBuilder {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(recursive).mode(0o700);
        builder
    }

    pub fn private_file_options() -> fs::OpenOptions {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).read(true).write(true).mode(0o600);
        options
    }

    pub fn private_async_file_options() -> tokio::fs::OpenOptions {
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).read(true).write(true).mode(0o600);
        options
    }

    pub async fn sync_directory(path: &Path) -> io::Result<()> {
        tokio::fs::File::open(path).await?.sync_all().await
    }

    #[cfg(test)]
    pub fn directory_is_private(path: &Path) -> io::Result<bool> {
        Ok(fs::metadata(path)?.permissions().mode() & 0o777 == 0o700)
    }
}

#[cfg(not(unix))]
mod implementation {
    use super::{Path, fs, io};

    pub fn private_directory_builder(recursive: bool) -> fs::DirBuilder {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(recursive);
        builder
    }

    pub fn private_file_options() -> fs::OpenOptions {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).read(true).write(true);
        options
    }

    pub fn private_async_file_options() -> tokio::fs::OpenOptions {
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).read(true).write(true);
        options
    }

    pub async fn sync_directory(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    pub fn directory_is_private(_path: &Path) -> io::Result<bool> {
        Ok(true)
    }
}

pub(crate) use implementation::{
    private_async_file_options, private_directory_builder, private_file_options, sync_directory,
};

#[cfg(test)]
pub(crate) use implementation::directory_is_private;

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    private_directory_builder(false).create(path)
}
