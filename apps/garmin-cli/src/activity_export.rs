//! Offline activity package creation and inspection.

use std::{fs::File, path::PathBuf};

use anyhow::{Context as _, Result};
use clap::{Args, Subcommand};
use garmin_model::{identity::UserId, observation::ObservationId};
use garmin_services::{UserContext, activity_export, deployment::Deployment};
use serde::Serialize;

#[derive(Debug, Args)]
pub struct Options {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Export an activity from a stopped deployment into a new Data Package ZIP.
    Export {
        #[arg(long)]
        deployment: PathBuf,
        #[arg(long)]
        profile: UserId,
        #[arg(long)]
        observation: ObservationId,
        #[arg(long)]
        output: PathBuf,
    },
    /// Read and verify a portable activity package without importing it.
    Inspect { package: PathBuf },
}

#[derive(Serialize)]
struct Summary {
    selected_observation: ObservationId,
    observations: usize,
    original_fit_files: usize,
}

impl Options {
    pub async fn run(self, json: bool) -> Result<()> {
        let summary = match self.command {
            Command::Export {
                deployment,
                profile,
                observation,
                output,
            } => {
                anyhow::ensure!(deployment.is_dir(), "deployment directory does not exist");
                let deployment = Deployment::open(deployment).await?;
                let result = {
                    let application = deployment.application(deployment.epoch()).await?;
                    application
                        .export_activity(UserContext::new(profile), observation, &output)
                        .await
                };
                deployment.close().await;
                let report = result?.context("activity does not exist in the selected profile")?;
                Summary {
                    selected_observation: observation,
                    observations: report.observations,
                    original_fit_files: report.originals,
                }
            }
            Command::Inspect { package } => {
                let package = activity_export::read(File::open(package)?)?;
                Summary {
                    selected_observation: package.selected,
                    observations: package.activities.len(),
                    original_fit_files: package.originals.len(),
                }
            }
        };
        if json {
            crate::emit_json(&summary)?;
        } else {
            println!(
                "Activity {}: {} observation(s), {} original FIT file(s)",
                summary.selected_observation, summary.observations, summary.original_fit_files
            );
        }
        Ok(())
    }
}
