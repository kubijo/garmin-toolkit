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
    /// Use the original main-thread browser map renderer instead of the worker.
    #[arg(long)]
    no_map_render_worker: bool,
    /// Enable built-in semantic UI scenarios (demo builds only).
    #[arg(long)]
    ui_automation: bool,
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
    garmin_hass::run(garmin_hass::BrowserOptions {
        map_upload_telemetry: !args.no_map_upload_telemetry,
        map_render_experiment: !args.no_map_render_worker,
        ui_automation: args.ui_automation,
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_map_is_enabled_unless_explicitly_disabled() {
        assert!(
            !Args::try_parse_from(["garmin-hass"])
                .unwrap()
                .no_map_render_worker
        );
        assert!(
            Args::try_parse_from(["garmin-hass", "--no-map-render-worker"])
                .unwrap()
                .no_map_render_worker
        );
        assert!(Args::try_parse_from(["garmin-hass", "--map-render-experiment"]).is_err());
    }

    #[test]
    fn automation_flag_is_independent() {
        assert!(!Args::try_parse_from(["garmin-hass"]).unwrap().ui_automation);
        let args = Args::try_parse_from(["garmin-hass", "--ui-automation"]).unwrap();
        assert!(args.ui_automation);
        assert!(!args.no_map_render_worker);
    }

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
