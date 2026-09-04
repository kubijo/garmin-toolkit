//! Host signal adapter.

use std::io;

#[cfg(unix)]
mod implementation {
    use super::io;
    use tokio::signal::unix::{SignalKind, signal};

    pub async fn close_signal() -> io::Result<()> {
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        let mut hangup = signal(SignalKind::hangup())?;
        let mut quit = signal(SignalKind::quit())?;
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
            _ = hangup.recv() => {}
            _ = quit.recv() => {}
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod implementation {
    use super::io;

    pub async fn close_signal() -> io::Result<()> {
        tokio::signal::ctrl_c().await
    }
}

pub(super) use implementation::close_signal;
