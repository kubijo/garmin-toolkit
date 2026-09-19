//! Home Assistant native host target entry point.

#![expect(
    clippy::multiple_crate_versions,
    reason = "FIT fixtures and SQLx currently require incompatible transitive releases"
)]

use clap::Parser as _;
use std::io::{self, IsTerminal as _};
use tracing_subscriber::EnvFilter;

#[derive(clap::Parser)]
#[command(version, about)]
struct Args {
    /// Disable browser map upload lifecycle telemetry for controlled profiling comparisons.
    #[arg(long)]
    no_map_upload_telemetry: bool,
}

#[tokio::main]
async fn main() -> Result<(), garmin_hass::Error> {
    let args = Args::parse();
    tracing_logfmt::builder()
        .with_ansi_color(io::stderr().is_terminal())
        .subscriber_builder()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("garmin_hass=info")),
        )
        .init();
    garmin_hass::run(!args.no_map_upload_telemetry).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_telemetry_is_enabled_unless_explicitly_disabled() {
        assert!(
            !Args::try_parse_from(["garmin-hass"])
                .unwrap()
                .no_map_upload_telemetry
        );
        assert!(
            Args::try_parse_from(["garmin-hass", "--no-map-upload-telemetry"])
                .unwrap()
                .no_map_upload_telemetry
        );
        assert!(Args::try_parse_from(["garmin-hass", "--no-map-upload-telemetery"]).is_err());
    }
}
