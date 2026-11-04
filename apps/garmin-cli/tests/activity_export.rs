//! The CLI produces and reads an activity package from an isolated stopped deployment.

use std::{
    path::Path,
    process::{Command, Output},
};

use garmin_services::{UserContext, deployment::Deployment};

fn cli(root: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_garmin-cli"))
        .env("GARMIN_TOOLKIT_STATE_DIR", root.join("cli-state"))
        .args(arguments)
        .output()
}

#[tokio::test]
async fn cli_exports_and_inspects_without_replacing_files_or_opening_a_live_deployment()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let deployment_path = root.path().join("deployment");
    let deployment = Deployment::open_initialized(&deployment_path, async |storage| {
        garmin_fixtures::seed(storage).await?;
        Ok(())
    })
    .await?;
    let application = deployment.application(deployment.epoch()).await?;
    let profile = application.profiles().await?[0].id();
    let observation = application.activities(UserContext::new(profile)).await?[0].observation_id();
    drop(application);
    let output = root.path().join("activity.zip");
    let path = deployment_path.to_str().ok_or("invalid deployment path")?;
    let destination = output.to_str().ok_or("invalid output path")?;
    let profile = profile.to_string();
    let observation = observation.to_string();
    let arguments = [
        "--json",
        "activity",
        "export",
        "--deployment",
        path,
        "--profile",
        &profile,
        "--observation",
        &observation,
        "--output",
        destination,
    ];
    let busy = cli(root.path(), &arguments)?;
    assert!(!busy.status.success());
    assert!(!output.exists());
    deployment.close().await;
    drop(deployment);
    let exported = cli(root.path(), &arguments)?;
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&exported.stdout)?;
    assert_eq!(summary["selected_observation"], observation);
    let original = std::fs::read(&output)?;
    let inspected = cli(root.path(), &["--json", "activity", "inspect", destination])?;
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&inspected.stdout)?,
        summary
    );
    assert!(!cli(root.path(), &arguments)?.status.success());
    assert_eq!(std::fs::read(output)?, original);
    Ok(())
}
