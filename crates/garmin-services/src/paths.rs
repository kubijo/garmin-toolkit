//! Host state location, with an application override independent of the platform.

use anyhow::{Context as _, Result, bail};
use std::path::PathBuf;

/// Resolve the parent of CLI logs, captures, and the legacy recovery registry.
/// `GARMIN_TOOLKIT_STATE_DIR` overrides the platform state/data directory without
/// changing the established relative paths beneath it.
///
/// # Errors
/// An empty override or an unavailable platform state directory.
pub fn host_state_directory() -> Result<PathBuf> {
    if let Some(directory) = std::env::var_os("GARMIN_TOOLKIT_STATE_DIR") {
        if directory.is_empty() {
            bail!("GARMIN_TOOLKIT_STATE_DIR cannot be empty");
        }
        return Ok(PathBuf::from(directory));
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("no user state directory is available")
}
