//! Explicit experimental GFDI identity and activity-download commands.

#[cfg(target_os = "linux")]
mod cli {
    use std::path::PathBuf;
    use std::time::Duration;

    use clap::{Parser, Subcommand};
    use garmin_device::gfdi_smoke::{
        LiveOptions, download, live, pair, simulate, simulate_download,
    };
    use garmin_gfdi::LinkMode;

    #[derive(Parser)]
    #[command(about = "Bounded native Garmin protocol commands; see ADR 0011 before live use")]
    struct Args {
        #[command(subcommand)]
        mode: Mode,
    }

    #[derive(Subcommand)]
    enum Mode {
        /// Establish an OS bond using Ubuntu's confirmation dialog, then complete Garmin setup.
        Pair {
            #[arg(long)]
            adapter: String,
            #[arg(long)]
            device: bluer::Address,
            #[arg(long)]
            capture_dir: PathBuf,
            #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..=600))]
            timeout_secs: u64,
            #[arg(long)]
            plain: bool,
        },
        /// Exercise an activity download and FIT retention with a synthetic peer.
        SimulateDownload {
            #[arg(long)]
            capture_dir: PathBuf,
        },
        /// Download one activity through Garmin's Protobuf service and transfer pipe.
        Download {
            #[arg(long)]
            adapter: String,
            #[arg(long)]
            device: bluer::Address,
            #[arg(long)]
            capture_dir: PathBuf,
            #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=600))]
            timeout_secs: u64,
            #[arg(long)]
            plain: bool,
        },
        /// Exercise the complete driver against a synthetic peer,
        /// without a system bus or radio.
        Simulate {
            /// New, private capture directory;
            /// existing paths are never reused.
            #[arg(long)]
            capture_dir: PathBuf,
        },
        /// Observe an explicitly selected, already bonded
        /// disposable device; never pairs or scans.
        Live {
            #[arg(long)]
            adapter: String,
            #[arg(long)]
            device: bluer::Address,
            /// New, private capture directory;
            /// includes raw protocol bytes and device identity.
            #[arg(long)]
            capture_dir: PathBuf,
            #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=60))]
            timeout_secs: u64,
            /// Explicitly request plain `MultiLink` instead
            /// of reliable mode; no automatic fallback.
            #[arg(long)]
            plain: bool,
        },
    }

    pub(super) async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let identity = match Args::parse().mode {
            Mode::Pair {
                adapter,
                device,
                capture_dir,
                timeout_secs,
                plain,
            } => {
                pair(LiveOptions {
                    adapter,
                    device,
                    capture_dir,
                    timeout: Duration::from_secs(timeout_secs),
                    mode: if plain {
                        LinkMode::Plain
                    } else {
                        LinkMode::Reliable
                    },
                })
                .await?
            }
            Mode::SimulateDownload { capture_dir } => {
                let report = simulate_download(capture_dir).await?;
                println!(
                    "{}",
                    serde_json::json!({"file": report.path, "bytes": report.bytes, "sha256": report.sha256, "model": report.identity.model})
                );
                return Ok(());
            }
            Mode::Download {
                adapter,
                device,
                capture_dir,
                timeout_secs,
                plain,
            } => {
                let report = download(LiveOptions {
                    adapter,
                    device,
                    capture_dir,
                    timeout: Duration::from_secs(timeout_secs),
                    mode: if plain {
                        LinkMode::Plain
                    } else {
                        LinkMode::Reliable
                    },
                })
                .await?;
                println!(
                    "{}",
                    serde_json::json!({"file": report.path, "bytes": report.bytes, "sha256": report.sha256, "model": report.identity.model})
                );
                return Ok(());
            }
            Mode::Simulate { capture_dir } => simulate(capture_dir).await?,
            Mode::Live {
                adapter,
                device,
                capture_dir,
                timeout_secs,
                plain,
            } => {
                live(LiveOptions {
                    adapter,
                    device,
                    capture_dir,
                    timeout: Duration::from_secs(timeout_secs),
                    mode: if plain {
                        LinkMode::Plain
                    } else {
                        LinkMode::Reliable
                    },
                })
                .await?
            }
        };
        println!(
            "{}",
            serde_json::json!({
                "model": identity.model,
                "product_number": identity.product_number,
                "firmware": format!("{}.{:02}", identity.software_version / 100, identity.software_version % 100),
                "protocol_version": identity.protocol_version,
            })
        );
        Ok(())
    }
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    cli::run().await
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The GFDI smoke harness currently supports Linux only.");
    std::process::exit(2);
}
