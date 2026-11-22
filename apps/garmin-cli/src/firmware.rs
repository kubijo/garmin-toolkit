use anyhow::{Context as _, Result, bail, ensure};
use clap::{Args, Subcommand};
use garmin_capture::SessionCapture;
use garmin_cli_tui::{
    Cancelled, ConfirmationBody, DeviceChanges, DeviceOperation, Session as Terminal,
    firmware_component_summary,
};
use garmin_device::ParsedManifest;
use garmin_map_service::FIRMWARE_UPDATE_ENDPOINT;
use garmin_model::firmware::Component;
use garmin_progress::ProgressReporter;
use garmin_services::firmware::{self, TransferStatus};
use garmin_services::{
    maps::{
        Operations,
        device::{Connector, DirectoryConnector, MountedConnector, RawMtpConnector},
        pending_recovery::PendingRecoveryStore,
    },
    paths::host_state_directory,
};
use garmin_update::firmware::Confirmation;
use std::path::Path;
use std::sync::Arc;
use url::Url;

use super::{
    DeviceWriteConsent, MapService, TargetArgs, UpdateCheckArgs, authorize_device_write,
    authorize_device_write_details, authorize_garmin_contact_at, cache_dir,
    default_session_capture_path, emit_json, load_target,
};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Query available software for a selected device.
    Check(UpdateCheckArgs),
    /// Download, verify and stage selected parts for installation after ejection.
    Stage(StageArgs),
    /// Inspect a prior firmware transfer and compare its versions with the connected device.
    Status(StatusArgs),
    /// Recover an interrupted firmware transfer.
    Recover(RecoverArgs),
}

#[derive(Debug, Args)]
pub(super) struct StageArgs {
    #[command(flatten)]
    check: UpdateCheckArgs,
    #[command(flatten)]
    consent: DeviceWriteConsent,
    /// Part number from firmware check. Repeat to include another component.
    #[arg(long = "part")]
    parts: Vec<String>,
    /// Storage name for a directory mount when its name differs from the device inventory.
    #[arg(long, requires = "path")]
    storage_label: Option<String>,
}

impl StageArgs {
    fn connector(&self, manifest: &ParsedManifest) -> Result<Arc<dyn Connector>> {
        let target = &self.check.target;
        let label = self.storage_label.as_deref().or_else(|| {
            let mount_name = target.path.as_ref()?.file_name()?.to_str()?;
            manifest
                .declares_update_storage(mount_name)
                .then_some(mount_name)
        });
        connector(target, label)
    }
}

#[derive(Debug, Args)]
pub(super) struct StatusArgs {
    #[command(flatten)]
    target: TargetArgs,
    /// Dismiss the completed transfer after inspecting the installed versions.
    #[arg(long)]
    acknowledge: bool,
    /// Inspect or dismiss only this receipt ID from firmware status.
    #[arg(long)]
    receipt: Option<uuid::Uuid>,
}

#[derive(Debug, Args)]
pub(super) struct RecoverArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    consent: DeviceWriteConsent,
    /// Select the receipt to recover when several are registered for this device.
    #[arg(long)]
    receipt: Option<uuid::Uuid>,
}

impl Command {
    pub(super) async fn run(
        self,
        json: bool,
        mock: Option<&Url>,
        capture: Option<SessionCapture>,
    ) -> Result<()> {
        let source = MapService::from_mock_base(mock);
        let open_terminal = || {
            super::interactive_confirmation_available(json)
                .then(|| Terminal::open_with_profile(source.profile(DeviceChanges::Enabled)))
                .transpose()
        };
        let args = match self {
            Self::Check(args) => args,
            Self::Stage(args) => {
                return stage(
                    args,
                    source,
                    json,
                    capture,
                    None,
                    Destination::Selected,
                    open_terminal()?,
                )
                .await;
            }
            Self::Status(args) => {
                return status(args, json, source, Destination::Selected, None).await;
            }
            Self::Recover(args) => {
                return recover(args, json, source, Destination::Selected, open_terminal()?).await;
            }
        };
        let mut terminal =
            if !source.is_mock() && args.contact_consent.confirm_contact_garmin.is_none() {
                open_terminal()?
            } else {
                None
            };
        if !source.is_mock() {
            authorize_garmin_contact_at(
                &args.contact_consent,
                json,
                false,
                terminal.as_mut(),
                FIRMWARE_UPDATE_ENDPOINT,
            )?;
        }
        let query =
            async { firmware::check(&source.shared(), load_target(&args.target).await?).await };
        let components = match terminal.as_mut() {
            Some(tui) => tui.load_firmware_components(query).await?,
            None => query.await?,
        };
        drop(terminal);
        if json {
            return emit_json(&components);
        }
        if components.is_empty() {
            println!("No device updates available.");
        }
        for item in components {
            println!("{}", firmware_component_summary(&item));
            if let Some(reason) = item.staging_issue {
                println!("  {reason}");
            }
        }
        Ok(())
    }
}

pub(super) async fn guided(
    operation: DeviceOperation,
    target: TargetArgs,
    source: MapService<'_>,
    capture: SessionCapture,
    cache: Option<&Path>,
    terminal: Terminal,
) -> Result<()> {
    let destination = if source.is_mock() {
        Destination::Virtual
    } else {
        Destination::Selected
    };
    let consent = DeviceWriteConsent {
        confirm_device_write: None,
    };
    match operation {
        DeviceOperation::Firmware => {
            stage(
                StageArgs {
                    check: UpdateCheckArgs {
                        target,
                        contact_consent: super::GarminContactConsent {
                            confirm_contact_garmin: None,
                        },
                    },
                    consent,
                    parts: Vec::new(),
                    storage_label: None,
                },
                source,
                false,
                Some(capture),
                cache,
                destination,
                Some(terminal),
            )
            .await
        }
        DeviceOperation::FirmwareStatus => {
            status(
                StatusArgs {
                    target,
                    acknowledge: false,
                    receipt: None,
                },
                false,
                source,
                destination,
                Some(terminal),
            )
            .await
        }
        DeviceOperation::FirmwareRecovery => {
            recover(
                RecoverArgs {
                    target,
                    consent,
                    receipt: None,
                },
                false,
                source,
                destination,
                Some(terminal),
            )
            .await
        }
        DeviceOperation::Maps => unreachable!("handled by the maps workflow"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Destination {
    Selected,
    Virtual,
}

fn connector(target: &TargetArgs, label: Option<&str>) -> Result<Arc<dyn Connector>> {
    match (&target.path, target.mtp_location, &target.mounted_mtp) {
        (Some(root), None, None) => Ok(Arc::new(DirectoryConnector {
            root: root.clone(),
            storage_id: "primary".into(),
            storage_label: label.unwrap_or("Device storage").into(),
        })),
        (None, Some(location), None) => Ok(Arc::new(RawMtpConnector(location))),
        (None, None, Some(mount)) => Ok(Arc::new(MountedConnector(mount.clone()))),
        _ => bail!("select exactly one device transport"),
    }
}

fn registry(service: MapService<'_>, destination: Destination) -> Result<PendingRecoveryStore> {
    if !service.is_mock() {
        return PendingRecoveryStore::physical_firmware();
    }
    let directory = if destination == Destination::Virtual {
        "virtual"
    } else {
        "simulator"
    };
    Ok(PendingRecoveryStore::new(
        host_state_directory()?
            .join("garmin-cli/firmware")
            .join(directory),
    ))
}

async fn retained_connection(
    target: &TargetArgs,
    registry: &PendingRecoveryStore,
    destination: Destination,
) -> Result<garmin_services::maps::device::Connection> {
    let mut device = connector(target, None)?.connect().await?;
    if destination == Destination::Virtual {
        let digest = device.manifest.identity_digest();
        let entries = registry.for_device(&digest)?;
        match entries.as_slice() {
            [] => {}
            [pending] => {
                let shadow = garmin_simulator::shadow::Shadow::reopen(&pending.transaction)
                    .await
                    .with_context(|| {
                        format!(
                            "restore the virtual device capture at {}",
                            pending.transaction.display()
                        )
                    })?;
                ensure!(
                    shadow.matches_source(&digest, &pending.plan_digest),
                    "virtual firmware target changed"
                );
                device.device = Box::new(shadow);
            }
            _ => bail!("expected one virtual firmware transaction"),
        }
    }
    Ok(device)
}

fn require_mock_target(service: MapService<'_>, target: &TargetArgs) -> Result<()> {
    if service.is_mock() {
        garmin_simulator::require_mock_device_root(
            target
                .path
                .as_deref()
                .context("simulated firmware requires a marked directory device")?,
        )?;
    }
    Ok(())
}

fn selected_parts(
    components: &[Component],
    parts: &[String],
    terminal: Option<&mut Terminal>,
) -> Result<Vec<u32>> {
    if parts.is_empty() {
        let terminal = terminal.context("supply --part for a noninteractive firmware selection")?;
        return terminal
            .select_firmware(components)?
            .ok_or_else(|| Cancelled::new("firmware selection"))?
            .into_iter()
            .map(|row| u32::try_from(row).map_err(Into::into))
            .collect();
    }
    parts
        .iter()
        .map(|part| {
            let mut matches = components
                .iter()
                .enumerate()
                .filter(|(_, item)| &item.part_number == part);
            let (index, _) = matches
                .next()
                .with_context(|| format!("part {part} is unavailable"))?;
            ensure!(matches.next().is_none(), "part {part} has multiple offers");
            Ok(u32::try_from(index)?)
        })
        .collect()
}

async fn stage(
    args: StageArgs,
    source: MapService<'_>,
    json: bool,
    capture: Option<SessionCapture>,
    cache: Option<&Path>,
    destination: Destination,
    mut terminal: Option<Terminal>,
) -> Result<()> {
    require_mock_target(source, &args.check.target)?;
    if !source.is_mock() {
        authorize_garmin_contact_at(
            &args.check.contact_consent,
            json,
            true,
            terminal.as_mut(),
            FIRMWARE_UPDATE_ENDPOINT,
        )?;
    }
    let query = async {
        let device = load_target(&args.check.target).await?;
        let target = args.connector(device.capabilities())?;
        let session = Operations::default().connect_firmware(
            device.identity_digest(),
            target,
            source.shared(),
        );
        let (revision, components) = session.check_components().await?;
        Ok((device, session, revision, components))
    };
    let (device, session, revision, components) = match terminal.as_mut() {
        Some(tui) => tui.load_firmware_components(query).await?,
        None => query.await?,
    };
    let selected = selected_parts(&components, &args.parts, terminal.as_mut())?;
    if selected.is_empty() {
        return Ok(());
    }
    let reviewed = session.review_selection(revision, selected)?;
    let approval = reviewed.review.context("missing firmware approval")?;
    let body = ConfirmationBody::firmware_write(
        approval
            .selected
            .iter()
            .map(|&index| &components[index as usize]),
        approval.download_bytes,
        &device.summary.model,
    );
    authorize_device_write_details(
        &args.consent,
        json,
        "Write to device?",
        body,
        "Write to device",
        terminal.as_mut(),
    )?;
    let approved = session
        .claim_for_staging(reviewed.revision, approval.approval)
        .await?;
    ensure!(
        approved.connection().manifest.identity_digest() == device.identity_digest(),
        "selected device changed"
    );
    let needs_restart = approved
        .offers()
        .iter()
        .any(|offer| offer.installation.is_restart_required);
    let capture = match capture {
        Some(capture) => capture,
        None => SessionCapture::create(&default_session_capture_path()?)?,
    };
    let cache = match cache {
        Some(dir) => dir.to_owned(),
        None => cache_dir()?,
    }
    .join("firmware");
    let receipts = registry(source, destination)?;
    let approved = if destination == Destination::Virtual {
        approved.on_virtual_device()
    } else {
        approved
    };
    let report = if let Some(tui) = terminal.as_mut() {
        let (progress, receiver) = ProgressReporter::channel();
        let cancellation = progress.cancellation_token();
        Box::pin(tui.run_firmware_progress(
            receiver,
            Box::pin(approved.stage(&cache, &capture, &receipts, progress)),
            cancellation,
        ))
        .await?
    } else {
        approved
            .stage(&cache, &capture, &receipts, ProgressReporter::default())
            .await?
    };
    drop(terminal);
    if json {
        return emit_json(
            &serde_json::json!({"state": "ready_to_eject", "needs_restart": needs_restart, "transfer": report}),
        );
    }
    print_staging_result(destination, &capture, needs_restart);
    Ok(())
}

fn print_staging_result(destination: Destination, capture: &SessionCapture, needs_restart: bool) {
    if destination == Destination::Virtual {
        println!(
            "Staged on the virtual device in {}",
            capture.root().join("simulation").display()
        );
        return;
    }
    println!("Ready to eject. Disconnect the device so it can apply the staged updates.");
    if needs_restart {
        println!("The update requires a device restart.");
    }
}

async fn status(
    args: StatusArgs,
    json: bool,
    source: MapService<'_>,
    destination: Destination,
    mut terminal: Option<Terminal>,
) -> Result<()> {
    let registry = registry(source, destination)?;
    let device = retained_connection(&args.target, &registry, destination).await?;
    let reports = firmware::review_status(&device, &registry, args.receipt, |report, count| {
        let Some(tui) = terminal.as_mut() else {
            ensure!(
                !args.acknowledge || count == 1,
                "select --receipt ID from firmware status"
            );
            return Ok(args.acknowledge);
        };
        let mut details = vec![format!("{} · {}", report.registered_at, report.receipt)];
        match &report.status {
            TransferStatus::VersionsUnverified => details.push(
                "Transfer completed. This older receipt cannot verify installed versions."
                    .to_owned(),
            ),
            TransferStatus::RecoveryRequired | TransferStatus::Unavailable { .. } => {
                return Ok(false);
            }
            TransferStatus::Confirmed { components }
            | TransferStatus::AwaitingConfirmation { components } => {
                details.extend(components.iter().map(component_result));
            }
        }
        tui.confirm(
            "Last firmware transfer",
            &details.join("\n"),
            "Dismiss transfer receipt",
        )
    })
    .await?;
    drop(terminal);
    if json {
        return emit_json(&reports);
    }
    if reports.is_empty() {
        println!("No pending firmware transfer.");
    }
    for report in reports {
        println!("{}  {}", report.receipt, report.registered_at);
        let components = match report.status {
            TransferStatus::VersionsUnverified => {
                println!(
                    "Transfer completed. This older receipt cannot verify installed versions."
                );
                continue;
            }
            TransferStatus::Unavailable { reason } => {
                println!("  {reason}");
                continue;
            }
            TransferStatus::RecoveryRequired => {
                println!("Recovery required. Run firmware recover.");
                continue;
            }
            TransferStatus::Confirmed { components } => {
                println!("The device reports all selected versions.");
                components
            }
            TransferStatus::AwaitingConfirmation { components } => {
                println!("Awaiting confirmation from the device.");
                components
            }
        };
        for component in components {
            println!("  {}", component_result(&component));
        }
    }
    Ok(())
}

fn component_result(component: &firmware::ComponentStatus) -> String {
    let part = &component.part_number;
    match &component.status {
        Confirmation::Confirmed => format!("{part}  confirmed"),
        Confirmation::Unchanged => format!("{part}  unchanged"),
        Confirmation::OtherVersion(version) => format!("{part}  device reports {version}"),
        Confirmation::Unreported => format!("{part}  absent from the device inventory"),
    }
}

async fn recover(
    args: RecoverArgs,
    json: bool,
    source: MapService<'_>,
    destination: Destination,
    mut terminal: Option<Terminal>,
) -> Result<()> {
    require_mock_target(source, &args.target)?;
    let registry = registry(source, destination)?;
    let device = retained_connection(&args.target, &registry, destination).await?;
    let identity = device.manifest.identity_digest();
    let receipt = firmware::recovery_receipt_id(&device, &registry, args.receipt).await?;
    authorize_device_write(
        &args.consent,
        json,
        &format!("Recover firmware transfer {receipt} on the selected device?"),
        terminal.as_mut(),
    )?;
    let device = retained_connection(&args.target, &registry, destination).await?;
    ensure!(
        device.manifest.identity_digest() == identity,
        "selected device changed"
    );
    ensure!(
        destination != Destination::Virtual || device.device.execution_target().is_some(),
        "no virtual firmware transfer to recover"
    );
    let progress = ProgressReporter::default();
    let result = match firmware::recover_registered(&device, &registry, Some(receipt), &progress)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let Some(review) =
                firmware::review_empty_uploads(&device, &registry, Some(receipt), &progress)
                    .await?
            else {
                return Err(error);
            };
            let Some(tui) = terminal.as_mut() else {
                if json {
                    return emit_json(
                        &serde_json::json!({"state": "review_required", "files": review.files()}),
                    );
                }
                bail!("use interactive firmware recovery to review the empty uploads");
            };
            let files = review
                .files()
                .iter()
                .map(|f| format!("{} · {}", f.storage_label, f.path))
                .collect::<Vec<_>>()
                .join("\n");
            ensure!(
                tui.confirm(
                    "Preserve empty uploads and restore the originals?",
                    &files,
                    "Preserve and restore files"
                )?,
                "firmware recovery cancelled"
            );
            let fresh = retained_connection(&args.target, &registry, destination).await?;
            Some(
                firmware::approve_empty_uploads(
                    &fresh,
                    &registry,
                    Some(receipt),
                    &review,
                    &progress,
                )
                .await?,
            )
        }
    };
    drop(terminal);
    if let Some(recovered) = result {
        if json {
            return emit_json(&recovered);
        }
        println!("Firmware recovery completed: {:?}", recovered.outcome);
        return Ok(());
    }
    if json {
        emit_json(&serde_json::json!({"state":"unprepared"}))
    } else {
        println!("Cleared an interrupted preparation.");
        Ok(())
    }
}
