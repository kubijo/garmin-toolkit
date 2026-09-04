use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version, about = "Loopback Garmin OMT simulator and fixture helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(ServeArgs),
    Fixture(DeviceArgs),
    Verify(DeviceArgs),
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Loopback address to listen on; port zero chooses a free port.
    #[arg(long, default_value = "127.0.0.1:39765")]
    listen: SocketAddr,
    /// Write the selected base URL as JSON once the listener is ready.
    #[arg(long)]
    ready_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct DeviceArgs {
    #[arg(long)]
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Serve(args) => {
            garmin_simulator::serve(args.listen, args.ready_file.as_deref()).await
        }
        Command::Fixture(args) => {
            garmin_simulator::create_fixture(&args.path).await?;
            println!("{}", args.path.display());
            Ok(())
        }
        Command::Verify(args) => {
            let report = garmin_simulator::verify_fixture(&args.path).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}
