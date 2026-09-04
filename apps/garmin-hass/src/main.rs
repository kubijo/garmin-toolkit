//! Home Assistant native host target entry point.

#![expect(
    clippy::multiple_crate_versions,
    reason = "FIT fixtures and SQLx currently require incompatible transitive releases"
)]

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), garmin_hass::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("garmin_hass=info")),
        )
        .init();
    garmin_hass::run().await
}
