//! Home Assistant native host target entry point.

#![expect(
    clippy::multiple_crate_versions,
    reason = "FIT fixtures and SQLx currently require incompatible transitive releases"
)]

use std::io::{self, IsTerminal as _};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), garmin_hass::Error> {
    tracing_logfmt::builder()
        .with_ansi_color(io::stderr().is_terminal())
        .subscriber_builder()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("garmin_hass=info")),
        )
        .init();
    garmin_hass::run().await
}
