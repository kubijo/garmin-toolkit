//! File-manager requests shared with the process that owns the deployment.

use std::fs;
use std::io::{self, Write as _};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone)]
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

#[cfg(target_os = "macos")]
pub(crate) fn forward_from_bundle(inbox: Inbox) -> io::Result<bool> {
    use std::time::{Duration, Instant};
    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
        platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS, set_open_files_handler},
        window::WindowId,
    };

    struct Forwarding {
        deadline: Instant,
        result: Option<io::Result<()>>,
    }

    impl ApplicationHandler<io::Result<()>> for Forwarding {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, result: io::Result<()>) {
            self.result = Some(result);
            event_loop.exit();
        }

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            if Instant::now() >= self.deadline {
                event_loop.exit();
            }
        }
    }

    let executable = std::env::current_exe()?;
    if !executable
        .ancestors()
        .nth(3)
        .is_some_and(|path| path.extension().is_some_and(|extension| extension == "app"))
    {
        return Ok(false);
    }

    // Finder delivers the pending document after AppKit starts, even when this
    // process cannot acquire the deployment lock.
    let event_loop = EventLoop::<io::Result<()>>::with_user_event()
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_activate_ignoring_other_apps(false)
        .build()
        .map_err(io::Error::other)?;
    let proxy = event_loop.create_proxy();
    if !set_open_files_handler(move |files| {
        let result = inbox.enqueue(files);
        let accepted = result.is_ok();
        let _ = proxy.send_event(result);
        accepted
    }) {
        return Err(io::Error::other(
            "Finder file-open handler is already installed",
        ));
    }
    let mut forwarding = Forwarding {
        deadline: Instant::now() + Duration::from_secs(2),
        result: None,
    };
    event_loop
        .run_app(&mut forwarding)
        .map_err(io::Error::other)?;
    forwarding.result.transpose().map(|result| result.is_some())
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
