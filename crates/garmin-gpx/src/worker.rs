//! Resource-contained native parsing through the packaged GPX helper.

mod limits;
mod protocol;

#[cfg(all(test, target_os = "linux"))]
mod tests;

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use garmin_model::{artifact::ArtifactDigest, value::ComponentVersion};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    sync::Semaphore,
};

use crate::Document;

/// Bounds parser address space; this is not the host's memory limit.
pub const MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// Bounds parser CPU even when the parent process exits.
pub const CPU_SECONDS: u64 = 5;
/// Bounds IPC output and pending geometry retained by the host.
pub const MAX_REPLY_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CANDIDATES: usize = 128;
pub const MAX_NAME_BYTES: usize = 4096;
const WALL_TIME: Duration = Duration::from_secs(10);
static PARSER_SLOT: Semaphore = Semaphore::const_new(1);

/// A validated parse result bound to original bytes and parser identity.
/// Only the contained worker client constructs this value.
#[derive(Clone, Debug)]
pub struct PreparedGpx {
    bytes: Arc<[u8]>,
    document: Arc<Document>,
    parser: ComponentVersion,
}

impl PreparedGpx {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }
    #[must_use]
    pub const fn parser(&self) -> &ComponentVersion {
        &self.parser
    }
}

/// Uses a host-selected executable; never accept this path from an upload request.
#[derive(Clone, Debug)]
pub struct Parser {
    executable: PathBuf,
    timeout: Duration,
}

impl Parser {
    /// Finds the helper installed alongside the native application.
    /// # Errors
    /// [`Error`] when the native executable location is unavailable.
    pub fn beside_host() -> Result<Self, Error> {
        let host = std::env::current_exe()?;
        let parent = host.parent().ok_or(Error::InvalidExecutable)?;
        Self::new(parent.join("garmin-gpx-worker"))
    }

    /// Selects an absolute helper path under host control.
    /// # Errors
    /// [`Error::InvalidExecutable`] for relative paths.
    pub fn new(executable: impl AsRef<Path>) -> Result<Self, Error> {
        if !cfg!(target_os = "linux") {
            return Err(Error::UnsupportedPlatform);
        }
        if !executable.as_ref().is_absolute() {
            return Err(Error::InvalidExecutable);
        }
        Ok(Self {
            executable: executable.as_ref().to_owned(),
            timeout: WALL_TIME,
        })
    }

    /// Parses with bounded input/output, process memory/CPU limits, and a wall deadline.
    /// Dropping this future kills its child. Only one parser may run per host process.
    /// # Errors
    /// [`Error`] for busy capacity, input rejection, process failure, or protocol violations.
    pub async fn parse(&self, bytes: Arc<[u8]>) -> Result<PreparedGpx, Error> {
        if bytes.len() > crate::MAX_BYTES {
            return Err(Error::InputTooLarge);
        }
        let _slot = PARSER_SLOT.try_acquire().map_err(|_| Error::Busy)?;
        let mut child = Command::new(&self.executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or(Error::Protocol)?;
        let stdout = child.stdout.take().ok_or(Error::Protocol)?;
        let exchange = async {
            let input = &bytes;
            let send = async move {
                stdin.write_all(input).await?;
                drop(stdin);
                Ok::<_, std::io::Error>(())
            };
            let receive = async {
                let mut output = Vec::new();
                stdout
                    .take((MAX_REPLY_BYTES + 1) as u64)
                    .read_to_end(&mut output)
                    .await?;
                Ok::<_, std::io::Error>(output)
            };
            let ((), output) = tokio::try_join!(send, receive)?;
            if output.len() > MAX_REPLY_BYTES {
                return Err(Error::ReplyTooLarge);
            }
            let status = child.wait().await?;
            if !status.success() {
                return Err(Error::WorkerFailed);
            }
            let (document, parser) = protocol::decode(&output, ArtifactDigest::from_bytes(&bytes))?;
            Ok(PreparedGpx {
                bytes,
                document: Arc::new(document),
                parser,
            })
        };
        match tokio::time::timeout(self.timeout, exchange).await {
            Ok(Ok(prepared)) => Ok(prepared),
            Ok(Err(error)) => {
                let _ = child.kill().await;
                Err(error)
            }
            Err(_) => {
                let _ = child.kill().await;
                Err(Error::Timeout)
            }
        }
    }
}

/// Helper entry point. Limits are installed before reading or parsing untrusted input.
/// # Errors
/// An error if limits cannot be installed, IPC fails, or its output exceeds the cap.
pub fn run_worker() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read as _, Write as _};
    limits::apply()?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take((crate::MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let output = protocol::encode(&bytes)?;
    if output.len() > MAX_REPLY_BYTES {
        return Err("GPX worker reply exceeds its limit".into());
    }
    std::io::stdout().lock().write_all(&output)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("contained GPX parsing is not supported on this platform")]
    UnsupportedPlatform,
    #[error("another GPX file is being parsed")]
    Busy,
    #[error("GPX input exceeds its size limit")]
    InputTooLarge,
    #[error("GPX parser exceeded its time limit")]
    Timeout,
    #[error("GPX parser exited before completing; the file may exceed its resource limits")]
    WorkerFailed,
    #[error("GPX parser reply exceeds its size limit")]
    ReplyTooLarge,
    #[error("GPX parser returned invalid or incompatible data")]
    Protocol,
    #[error("GPX worker requires an absolute executable path")]
    InvalidExecutable,
    #[error("GPX could not be parsed: {0}")]
    Rejected(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
