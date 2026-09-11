mod diagnostic;
mod pending_recovery;
mod pipeline;

use anyhow::{Context, Result, bail};
use byte_unit::{Byte, UnitType};
use clap::{Args, Parser, Subcommand, ValueEnum};
use crossterm::style::{Color as AnsiColor, Stylize as _};
use garmin_capture::SessionCapture;
use garmin_cli_tui as tui;
use garmin_device::storage::{DeviceRead as _, DirectoryDevice};
use garmin_device::{
    DeviceInventory, DeviceManifest, DeviceProbeReport, DeviceSummary, MountedMtpDevice,
    MountedMtpProbeFailure, RawMtpSession, SafeRelativePath, TransportKind,
    discover_garmin_usb_sysfs, discover_mass_storage, discover_mounted_mtp,
    discover_mtp_candidates, inventory_mass_storage, inventory_mtp,
    mtp_usb_reset_known_ineffective, open_mass_storage, open_mtp, reset_mtp_transport,
};
use garmin_i18n::Language;
use garmin_map_service::{ClientIdentity, OmtClient};
use garmin_model::map::{MapCatalog, MapComponent, MapVersionStatus};
use garmin_progress::ProgressReporter;
use garmin_services::maps::{
    GarminDownloadAuthorizer, PhysicalTarget, RecoveryExecution, SimulatedTarget, UpdateExecution,
    UpdateOutcome, UpdateTarget, execute_update_plan, recover_update,
};
use garmin_simulator::require_mock_device_root;
use garmin_update::{
    BackupPolicy, DeviceTransactionKind, DeviceTransactionStore, RecoveryOutcome,
    RemovalApplyReport, RemovalPlan, UpdatePlan, execute_removal, recover_mass_storage,
    recover_removal,
};
use indoc::{formatdoc, indoc};
use pending_recovery::{PendingRecovery, PendingRecoveryKind, PendingRecoveryStore};
use serde::Serialize;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;
use termcolor::{ColorChoice as TerminalColorChoice, StandardStream, WriteColor};
use tracing_subscriber::EnvFilter;

const LINK_BENCHMARK_WARNING: &str =
    "Creates, syncs, and deletes one disposable device file. Existing Garmin files are untouched.";
static OUTPUT_COLOR: OnceLock<ColorChoice> = OnceLock::new();

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
enum ColorChoice {
    #[default]
    Auto,
    Off,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LanguageChoice {
    Auto,
    #[value(name = "en", alias = "english")]
    English,
    #[value(name = "cs", alias = "czech")]
    Czech,
}

#[derive(Debug, Parser)]
#[command(version, about = "Unofficial Garmin-compatible map maintenance client")]
struct Cli {
    /// Emit stable, scriptable JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Control color in command output.
    #[arg(long, global = true, value_enum, default_value = "auto")]
    color: ColorChoice,
    /// Set the interface language; defaults to `GARMIN_LANGUAGE` or the system locale.
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "LANGUAGE",
        env = "GARMIN_LANGUAGE",
        default_value = "auto"
    )]
    language: LanguageChoice,
    /// Use a loopback map service.
    #[arg(long, global = true, value_name = "URL")]
    mock_server: Option<url::Url>,
    /// Use a synthetic device in the interactive flow.
    #[arg(
        long,
        value_name = "PATH",
        requires_all = ["mock_server", "capture"]
    )]
    mock_device: Option<PathBuf>,
    /// Write evidence and backups to a new directory.
    #[arg(long, global = true, value_name = "NEW_DIRECTORY")]
    capture: Option<PathBuf>,
    /// Store reusable application artifacts in this directory.
    #[arg(long, global = true, value_name = "DIRECTORY")]
    cache_dir: Option<PathBuf>,
    /// Verify a real update without committing it.
    #[arg(long, requires = "capture", conflicts_with = "json")]
    dry: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Discover and inspect devices.
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Query, plan, install, or remove maps.
    Updates {
        #[command(subcommand)]
        command: UpdateCommand,
    },
    /// Measure network and device transfers.
    Benchmark {
        #[command(subcommand)]
        command: BenchmarkCommand,
    },
    /// Report device access and services.
    Doctor,
    /// Run synthetic workflows.
    Mock {
        #[command(subcommand)]
        command: MockCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    List,
    Inspect(TargetArgs),
    /// Recover an interrupted mass-storage update.
    Recover(RecoverArgs),
    /// Recover a mounted-MTP update from its capture.
    RecoverUpdate(MountedUpdateRecoveryArgs),
    /// Restore a removal from captured backups.
    RecoverRemoval(RemovalRecoveryArgs),
}

#[derive(Debug, Subcommand)]
enum UpdateCommand {
    Check(UpdateCheckArgs),
    Plan(UpdatePlanArgs),
    /// Preview component removal.
    RemovalPlan(RemovalPlanArgs),
    /// Execute a reviewed removal plan.
    Remove(RemoveArgs),
    Apply(ApplyArgs),
}

#[derive(Debug, Subcommand)]
enum BenchmarkCommand {
    /// Measure the device link with disposable bytes.
    Link(BenchmarkLinkArgs),
    /// Probe the full download and device path.
    Pipeline(PipelineBenchmarkArgs),
}

#[derive(Debug, Subcommand)]
enum MockCommand {
    /// Run the interactive flow on a disposable device.
    Demo,
    /// Test updates on a disposable device.
    Test,
}

#[derive(Debug, Args, Clone)]
#[group(required = true, multiple = false)]
pub(crate) struct TargetArgs {
    /// Mounted Garmin root.
    #[arg(long)]
    pub(crate) path: Option<PathBuf>,
    /// MTP ID from `device list`.
    #[arg(long)]
    pub(crate) mtp_location: Option<u64>,
    /// Desktop MTP ID from `device list`.
    #[arg(long)]
    pub(crate) mounted_mtp: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum GarminContactConfirmation {
    #[value(name = "CONTACT-GARMIN")]
    Confirmed,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DeviceWriteConfirmation {
    #[value(name = "WRITE-DEVICE")]
    Confirmed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
enum BackupChoice {
    #[default]
    Verified,
    Skip,
}

impl From<BackupChoice> for BackupPolicy {
    fn from(value: BackupChoice) -> Self {
        match value {
            BackupChoice::Verified => Self::Verified,
            BackupChoice::Skip => Self::Skip,
        }
    }
}

#[derive(Debug, Args)]
struct GarminContactConsent {
    /// Allow Garmin contact and returned download hosts.
    #[arg(long = "confirm-contact-garmin", value_name = "CONTACT-GARMIN")]
    confirm_contact_garmin: Option<GarminContactConfirmation>,
}

#[derive(Debug, Args)]
struct DeviceWriteConsent {
    /// Allow selected-device file changes.
    #[arg(long = "confirm-device-write", value_name = "WRITE-DEVICE")]
    confirm_device_write: Option<DeviceWriteConfirmation>,
}

#[derive(Debug, Args)]
struct RecoverArgs {
    /// Mounted Garmin root.
    #[arg(long)]
    path: PathBuf,
    #[command(flatten)]
    write_consent: DeviceWriteConsent,
}

#[derive(Debug, Args)]
struct RemovalRecoveryArgs {
    #[command(flatten)]
    target: OptionalTargetArgs,
    /// Interrupted removal capture.
    #[arg(long, value_name = "CAPTURE_DIRECTORY")]
    transaction: PathBuf,
    #[command(flatten)]
    write_consent: DeviceWriteConsent,
}

#[derive(Debug, Args)]
struct MountedUpdateRecoveryArgs {
    #[command(flatten)]
    target: OptionalTargetArgs,
    /// Interrupted update capture.
    #[arg(long, value_name = "CAPTURE_DIRECTORY")]
    transaction: PathBuf,
    #[command(flatten)]
    write_consent: DeviceWriteConsent,
}

#[derive(Debug, Args)]
struct UpdateCheckArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    contact_consent: GarminContactConsent,
}

#[derive(Debug, Args)]
struct UpdatePlanArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    contact_consent: GarminContactConsent,
    #[command(flatten)]
    selection: MapSelectionArgs,
    /// Recovery-backup policy included in the plan identity.
    #[arg(long, value_enum, default_value = "verified")]
    backup: BackupChoice,
}

#[derive(Debug, Args)]
struct RemovalPlanArgs {
    #[command(flatten)]
    target: OptionalTargetArgs,
    #[command(flatten)]
    contact_consent: GarminContactConsent,
    #[command(flatten)]
    selection: RemovalSelectionArgs,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    #[command(flatten)]
    plan: RemovalPlanArgs,
    #[command(flatten)]
    write_consent: DeviceWriteConsent,
    /// Digest from `updates removal-plan`.
    #[arg(long)]
    confirm_plan: String,
}

#[derive(Debug, Args)]
#[group(id = "removal-selection", required = true, multiple = false)]
struct RemovalSelectionArgs {
    /// Exact component name; repeatable.
    #[arg(long = "remove", value_name = "NAME", action = clap::ArgAction::Append, group = "removal-selection")]
    components: Vec<String>,
    /// Remove all removable components.
    #[arg(long, group = "removal-selection")]
    all_removable: bool,
}

#[derive(Debug, Args)]
#[group(id = "map-selection", multiple = false)]
struct MapSelectionArgs {
    /// Exact map name; repeatable.
    #[arg(long = "map", value_name = "NAME", action = clap::ArgAction::Append, group = "map-selection")]
    maps: Vec<String>,
    /// Select all returned maps.
    #[arg(long, group = "map-selection")]
    all_maps: bool,
}

#[derive(Debug, Args)]
struct ApplyArgs {
    #[command(flatten)]
    target: TargetArgs,
    #[command(flatten)]
    contact_consent: GarminContactConsent,
    #[command(flatten)]
    write_consent: DeviceWriteConsent,
    #[command(flatten)]
    selection: MapSelectionArgs,
    /// Recovery-backup policy; the interactive confirmation can change it.
    #[arg(long, value_enum, default_value = "verified")]
    backup: BackupChoice,
    /// Digest from `updates plan`.
    #[arg(long)]
    confirm_plan: Option<String>,
    /// Concurrent download limit.
    #[arg(
        long,
        default_value_t = 4,
        value_parser = parse_concurrency
    )]
    concurrency: usize,
}

#[derive(Debug, Args)]
struct BenchmarkLinkArgs {
    #[command(flatten)]
    target: OptionalTargetArgs,
    /// Payload size, such as 500MB or 2GB.
    #[arg(long, default_value = "256MB", value_parser = parse_byte_size)]
    size: Byte,
    #[command(flatten)]
    write_consent: DeviceWriteConsent,
}

#[derive(Debug, Args)]
struct PipelineBenchmarkArgs {
    #[command(flatten)]
    target: OptionalTargetArgs,
    /// Largest download matching a map or destination.
    #[arg(long)]
    map: Option<String>,
    #[command(flatten)]
    contact_consent: GarminContactConsent,
    #[command(flatten)]
    write_consent: DeviceWriteConsent,
}

#[derive(Debug, Args)]
#[group(multiple = false)]
struct OptionalTargetArgs {
    /// Mounted Garmin root.
    #[arg(long)]
    path: Option<PathBuf>,
    /// MTP ID from `device list`.
    #[arg(long)]
    mtp_location: Option<u64>,
    /// Desktop MTP ID from `device list`.
    #[arg(long)]
    mounted_mtp: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    OUTPUT_COLOR.get_or_init(|| cli.color);
    if let Err(error) = diagnostic::install(error_color_enabled()) {
        eprintln!("Could not install the terminal error reporter: {error}");
        return ExitCode::FAILURE;
    }
    match Box::pin(run_main(cli)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{:?}", diagnostic::report(into_diagnostic_error(error)));
            ExitCode::FAILURE
        }
    }
}

async fn run_main(cli: Cli) -> Result<()> {
    tui::configure_language(resolve_language(cli.language))?;
    configure_cache_dir(cli.cache_dir.as_deref())?;
    let terminal_ui = !cli.json && std::io::stdout().is_terminal();
    if cli.dry && (cli.command.is_some() || cli.mock_device.is_some()) {
        bail!("--dry cannot be combined with commands or --mock-device");
    }
    if cli.dry && !interactive_confirmation_available(cli.json) {
        bail!("--dry requires an interactive terminal");
    }
    let capture_path = if let Some(path) = cli.capture.clone() {
        Some(path)
    } else if cli.command.is_none() && cli.mock_device.is_none() && !cli.dry && !cli.json {
        Some(default_session_capture_path()?)
    } else {
        None
    };
    let capture = capture_path
        .as_deref()
        .map(SessionCapture::create)
        .transpose()
        .context("cannot create the requested session capture")?;
    initialize_tracing(capture.as_ref(), terminal_ui)?;
    if let Some(capture) = &capture {
        tracing::info!(directory = %capture.root().display(), "session capture started");
        capture
            .write_json(
                Path::new("session.json"),
                &serde_json::json!({
                    "garmin_cli_version": env!("CARGO_PKG_VERSION"),
                    "cache_directory": cache_dir()?,
                    "arguments": std::env::args_os()
                        .map(|argument| argument.to_string_lossy().into_owned())
                        .collect::<Vec<_>>(),
                    "working_directory": std::env::current_dir()?,
                    "started_unix_seconds": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs(),
                }),
            )
            .await?;
    }
    let result = Box::pin(run_cli(cli, capture.clone())).await;
    let cancelled = result
        .as_ref()
        .err()
        .and_then(|error| cancellation_step(error));
    match (&result, cancelled) {
        (_, Some(step)) => tracing::info!(%step, "garmin-cli session cancelled"),
        (Ok(()), None) => tracing::info!("garmin-cli session completed"),
        (Err(error), None) if capture.is_some() => {
            let error = diagnostic_error(error);
            tracing::error!(error = ?error, "garmin-cli session failed");
        }
        (Err(_), None) => {}
    }
    if let Some(capture) = &capture {
        let terminal = match (&result, cancelled) {
            (_, Some(step)) => serde_json::json!({ "status": "cancelled", "step": step }),
            (Ok(()), None) => serde_json::json!({ "status": "complete" }),
            (Err(error), None) => serde_json::json!({
                "status": "error",
                "error_chain": diagnostic_error(error)
                    .chain()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            }),
        };
        if let Err(capture_error) = capture
            .write_json(Path::new("terminal.json"), &terminal)
            .await
        {
            return match result {
                Ok(()) => Err(capture_error.into()),
                Err(error) => Err(error.context(format!(
                    "the terminal outcome could not be captured: {capture_error}"
                ))),
            };
        }
    }
    if cancelled.is_some() { Ok(()) } else { result }
}

fn resolve_language(choice: LanguageChoice) -> Language {
    match choice {
        LanguageChoice::English => Language::English,
        LanguageChoice::Czech => Language::Czech,
        LanguageChoice::Auto => sys_locale::get_locales()
            .find_map(|locale| Language::from_locale(&locale))
            .unwrap_or_default(),
    }
}

fn default_session_capture_path() -> Result<PathBuf> {
    let state = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("no user state directory is available for the automatic session capture")?;
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    Ok(state
        .join("garmin-cli/sessions")
        .join(format!("{started}-{}", uuid::Uuid::new_v4().simple())))
}

#[derive(Debug)]
struct PresentedError(anyhow::Error);

impl std::fmt::Display for PresentedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for PresentedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

fn diagnostic_error(error: &anyhow::Error) -> &anyhow::Error {
    presented_error(error).map_or(error, |presented| &presented.0)
}

fn into_diagnostic_error(error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<PresentedError>() {
        Ok(presented) => presented.0,
        Err(error) => error,
    }
}

fn presented_error(error: &anyhow::Error) -> Option<&PresentedError> {
    let outer: &(dyn std::error::Error + 'static) = error.as_ref();
    outer.downcast_ref()
}

fn cancellation_step(error: &anyhow::Error) -> Option<&'static str> {
    if let Some(cancelled) = error.downcast_ref::<tui::Cancelled>() {
        return Some(cancelled.step());
    }
    if matches!(
        error.downcast_ref::<garmin_update::DownloadError>(),
        Some(garmin_update::DownloadError::Cancelled)
    ) {
        return Some("map download");
    }
    if matches!(
        error.downcast_ref::<garmin_update::BackupError>(),
        Some(garmin_update::BackupError::Cancelled)
    ) {
        return Some("device backup");
    }
    if matches!(
        error.downcast_ref::<garmin_update::RemovalExecutionError>(),
        Some(garmin_update::RemovalExecutionError::Cancelled)
    ) {
        return Some("component removal");
    }
    if matches!(
        error.downcast_ref::<garmin_update::RemovalExecutionError>(),
        Some(garmin_update::RemovalExecutionError::Device(source))
            if source.is_cancelled()
    ) {
        return Some("component removal");
    }
    if matches!(
        error.downcast_ref::<garmin_update::MountedInstallError>(),
        Some(garmin_update::MountedInstallError::Cancelled)
    ) {
        return Some("device update");
    }
    if matches!(
        error.downcast_ref::<garmin_device::MountedMtpError>(),
        Some(source) if source.is_cancelled()
    ) {
        return Some("device upload");
    }
    matches!(
        error.downcast_ref::<garmin_update::ApplyError>(),
        Some(garmin_update::ApplyError::Cancelled)
    )
    .then_some("device update")
}

async fn run_cli(cli: Cli, capture: Option<SessionCapture>) -> Result<()> {
    if cli.dry {
        return interactive_guided_update(GuidedUpdate {
            source: GuidedDeviceSource::Discover,
            service: MapService::Garmin,
            device: GuidedDeviceAdapter::Production(CommitPolicy::Skip),
            cache: None,
            capture: capture.context("--dry requires --capture NEW_DIRECTORY")?,
        })
        .await;
    }
    if cli.mock_device.is_some() && cli.command.is_some() {
        bail!(
            "--mock-device opens the complete interactive flow and cannot be used with a command"
        );
    }
    match cli.command {
        None => match (cli.mock_device, cli.mock_server.as_ref()) {
            (Some(path), Some(mock_server)) => {
                interactive_mock_demo(path, mock_server, None, capture).await
            }
            (None, _) if cli.json => bail!("the interactive production app does not emit JSON"),
            (None, _) => {
                interactive_guided_update(GuidedUpdate {
                    source: GuidedDeviceSource::Discover,
                    service: MapService::Garmin,
                    device: GuidedDeviceAdapter::Production(CommitPolicy::Apply),
                    cache: None,
                    capture: capture.context("interactive production updates require a capture")?,
                })
                .await
            }
            (Some(_), None) => unreachable!("clap requires --mock-server with --mock-device"),
        },
        Some(Command::Device { command }) => Box::pin(device(command, cli.json)).await,
        Some(Command::Updates { command }) => {
            let runtime = UpdateRuntime {
                json: cli.json,
                service: MapService::from_mock_base(cli.mock_server.as_ref()),
                cache_override: None,
                capture,
            };
            updates(command, runtime, None).await
        }
        Some(Command::Benchmark { command }) => {
            Box::pin(benchmark(
                command,
                cli.json,
                cli.mock_server.as_ref(),
                capture,
            ))
            .await
        }
        Some(Command::Doctor) => doctor(cli.json).await,
        Some(Command::Mock { command }) => mock_command(command, cli.json, capture).await,
    }
}

fn initialize_tracing(capture: Option<&SessionCapture>, terminal_ui: bool) -> Result<()> {
    if let Some(capture) = capture {
        let log = capture.create_log()?;
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("trace"))
            .with_ansi(false)
            .with_writer(log)
            .try_init()
            .map_err(|error| anyhow::anyhow!("cannot initialize logging: {error}"))?;
    } else if terminal_ui {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into());
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::sink)
            .try_init()
            .map_err(|error| anyhow::anyhow!("cannot initialize logging: {error}"))?;
    } else {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into());
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init()
            .map_err(|error| anyhow::anyhow!("cannot initialize logging: {error}"))?;
    }
    Ok(())
}

async fn interactive_mock_demo(
    path: PathBuf,
    mock_server: &url::Url,
    cache_override: Option<&Path>,
    capture: Option<SessionCapture>,
) -> Result<()> {
    require_mock_device_root(&path).context("mock updates require a marked synthetic device")?;
    interactive_guided_update(GuidedUpdate {
        source: GuidedDeviceSource::Fixed(path.clone()),
        service: MapService::Loopback(mock_server),
        device: GuidedDeviceAdapter::FixedMassStorage { device: path },
        cache: cache_override,
        capture: capture.context("interactive updates require --capture NEW_DIRECTORY")?,
    })
    .await
}

#[derive(Debug, Serialize)]
struct MockTestReport {
    mock_test: &'static str,
    plan: UpdatePlan,
    apply: garmin_update::ApplyReport,
    server: garmin_simulator::MockStats,
    fixture: garmin_simulator::FixtureVerification,
}

#[derive(Debug, Clone)]
struct UpdateRuntime<'a> {
    json: bool,
    service: MapService<'a>,
    cache_override: Option<&'a Path>,
    capture: Option<SessionCapture>,
}

impl UpdateRuntime<'_> {
    async fn query(
        &self,
        manifest: &DeviceManifest,
        session: Option<&mut tui::Session>,
    ) -> Result<(OmtClient, MapCatalog)> {
        query_device_updates_with_feedback(manifest, self.service, self.capture.clone(), session)
            .await
    }
}

#[derive(Debug, Clone, Copy)]
enum MapService<'a> {
    Garmin,
    Loopback(&'a url::Url),
}

impl<'a> MapService<'a> {
    fn from_mock_base(base: Option<&'a url::Url>) -> Self {
        base.map_or(Self::Garmin, Self::Loopback)
    }

    const fn is_mock(self) -> bool {
        matches!(self, Self::Loopback(_))
    }

    const fn profile(self, device_changes: tui::DeviceChanges) -> tui::RunProfile {
        tui::RunProfile {
            service: match self {
                Self::Garmin => tui::ServiceEnvironment::Garmin,
                Self::Loopback(_) => tui::ServiceEnvironment::Mock,
            },
            device_changes,
        }
    }
}

type DeviceUpdateAdapter = Box<dyn UpdateTarget>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitPolicy {
    Skip,
    Apply,
}

struct SelectedUpdateResult {
    plan: UpdatePlan,
    execution: UpdateOutcome,
}

struct SelectedUpdatePlan {
    plan: UpdatePlan,
    components: Vec<tui::SelectedMapAction>,
}

#[derive(Serialize)]
struct RemovalPlanReport {
    service_plan: RemovalPlan,
    execution_plan: garmin_update::RemovalExecutionPlan,
}

enum UpdateCommandOutput {
    RemovalPlan(RemovalPlanReport),
    RemovalApplied(RemovalApplyReport),
}

struct SelectedUpdateContext<'a> {
    response: &'a MapCatalog,
    manifest: DeviceManifest,
    client: OmtClient,
    runtime: UpdateRuntime<'a>,
    session: Option<&'a mut tui::Session>,
}

struct SelectedUpdateRequest {
    selection: MapSelectionArgs,
    preselected: Option<Vec<usize>>,
    concurrency: usize,
    backup_policy: BackupPolicy,
    device: DeviceUpdateAdapter,
    approval: UpdateApproval,
}

enum UpdateApproval {
    User {
        write_consent: DeviceWriteConsent,
        confirm_plan: Option<String>,
    },
    ScriptedMock,
}

enum GuidedDeviceSource {
    Discover,
    Fixed(PathBuf),
}

#[derive(Debug, Clone)]
struct SelectableDevice {
    summary: DeviceSummary,
    target: TargetArgs,
}

enum GuidedDeviceAdapter {
    Production(CommitPolicy),
    FixedMassStorage { device: PathBuf },
}

impl GuidedDeviceAdapter {
    const fn device_changes(&self) -> tui::DeviceChanges {
        match self {
            Self::Production(CommitPolicy::Skip) => tui::DeviceChanges::Disabled,
            Self::Production(CommitPolicy::Apply) | Self::FixedMassStorage { .. } => {
                tui::DeviceChanges::Enabled
            }
        }
    }

    const fn supports_guided_removal(&self, manifest: &DeviceManifest) -> bool {
        matches!(self, Self::Production(CommitPolicy::Apply))
            && matches!(manifest.summary.transport, TransportKind::MountedMtp)
    }

    const fn supports_pending_recovery(&self, manifest: &DeviceManifest) -> bool {
        matches!(self, Self::Production(CommitPolicy::Apply))
            && matches!(manifest.summary.transport, TransportKind::MountedMtp)
    }

    fn resolve(&self, manifest: &DeviceManifest) -> Result<DeviceUpdateAdapter> {
        if let Self::FixedMassStorage { device } = self {
            return Ok(Box::new(SimulatedTarget {
                source: Box::new(DirectoryDevice::new(device.clone())),
                fixture: Some(device.clone()),
            }));
        }
        let Self::Production(commit) = self else {
            unreachable!()
        };
        let device: Box<dyn garmin_device::storage::DeviceWrite> = match manifest.summary.transport
        {
            TransportKind::MassStorage => Box::new(DirectoryDevice::new(PathBuf::from(
                &manifest.summary.location,
            ))),
            TransportKind::MountedMtp => Box::new(MountedMtpDevice::new(
                manifest
                    .summary
                    .location
                    .strip_prefix("mounted-mtp:")
                    .context("discovered mounted-MTP device has no usable location")?,
            )),
            TransportKind::Mtp => bail!(
                "map installation through {} is not supported",
                transport_label(manifest.summary.transport)
            ),
        };
        Ok(match commit {
            CommitPolicy::Apply => Box::new(PhysicalTarget(device)),
            CommitPolicy::Skip => Box::new(SimulatedTarget {
                source: device,
                fixture: None,
            }),
        })
    }
}

struct GuidedUpdate<'a> {
    source: GuidedDeviceSource,
    service: MapService<'a>,
    device: GuidedDeviceAdapter,
    cache: Option<&'a Path>,
    capture: SessionCapture,
}

async fn interactive_guided_update(config: GuidedUpdate<'_>) -> Result<()> {
    let profile = config.service.profile(config.device.device_changes());
    let mut session = tui::Session::open_with_profile(profile)?;
    let result = interactive_guided_update_flow(&config, &mut session).await;
    match result {
        Ok(()) => Ok(()),
        Err(error) if cancellation_step(&error).is_some() => Err(error),
        Err(error) if presented_error(&error).is_some() => Err(error),
        Err(error) => {
            if retain_and_present_update_failure(&mut session, &error, &config.capture).await {
                Err(PresentedError(error).into())
            } else {
                Err(error)
            }
        }
    }
}

async fn interactive_guided_update_flow(
    config: &GuidedUpdate<'_>,
    session: &mut tui::Session,
) -> Result<()> {
    let manifest = select_guided_device(&config.source, session).await?;
    let device = config.device.resolve(&manifest)?;
    if config.device.supports_pending_recovery(&manifest) {
        let writable = device
            .writable_device()
            .context("physical update target does not expose its device adapter")?;
        Box::pin(offer_pending_recovery(&manifest, writable, session)).await?;
    }
    let state = session.load_device_state(device.state()).await?;
    config
        .capture
        .write_json(Path::new("device-state-selected.json"), &state)
        .await?;
    session.set_device_state(state);
    if !config.service.is_mock() {
        authorize_garmin_contact(
            &GarminContactConsent {
                confirm_contact_garmin: None,
            },
            false,
            true,
            Some(session),
        )?;
    }
    let (client, response) = query_device_updates_with_feedback(
        &manifest,
        config.service,
        Some(config.capture.clone()),
        Some(session),
    )
    .await
    .context("Garmin's map-update response could not be read")?;
    let runtime = UpdateRuntime {
        json: false,
        service: config.service,
        cache_override: config.cache,
        capture: Some(config.capture.clone()),
    };
    if config.device.supports_guided_removal(&manifest) {
        return run_guided_combined_map_actions(
            &response, manifest, client, runtime, session, device,
        )
        .await;
    }
    run_guided_selected_update(None, &response, manifest, client, runtime, session, device).await
}

async fn offer_pending_recovery(
    manifest: &DeviceManifest,
    device: &dyn garmin_device::storage::DeviceWrite,
    session: &mut tui::Session,
) -> Result<()> {
    let device_store = DeviceTransactionStore::open(device).await?;
    let device_digest = manifest.identity_digest();
    let transaction = device_store.active(&device_digest).await?;
    let receipt_store = PendingRecoveryStore::application()?;
    let active_identity = transaction.as_ref().map(|transaction| {
        (
            pending_recovery_kind(transaction.kind()),
            transaction.plan_digest(),
        )
    });
    let recovery = receipt_store.for_selected_device(&device_digest, active_identity)?;
    let (kind, plan_digest, actions) = match (transaction.as_ref(), recovery.as_ref()) {
        (Some(transaction), _) => (
            pending_recovery_kind(transaction.kind()),
            transaction.plan_digest(),
            tui::PendingRecoveryActions::RecoverOrClear,
        ),
        (None, Some(recovery)) if recovery.is_prepared()? => (
            recovery.kind,
            recovery.plan_digest.as_str(),
            tui::PendingRecoveryActions::RecoverOnly,
        ),
        (None, Some(recovery)) => (
            recovery.kind,
            recovery.plan_digest.as_str(),
            tui::PendingRecoveryActions::DiscardOnly,
        ),
        (None, None) => return Ok(()),
    };
    let operation = match kind {
        PendingRecoveryKind::Update => tui::RecoveryOperation::Update,
        PendingRecoveryKind::Removal => tui::RecoveryOperation::Removal,
    };
    let body = tui::pending_recovery_confirmation_body(
        update_confirmation_device(manifest),
        operation,
        plan_digest,
        recovery
            .as_ref()
            .map(|recovery| recovery.transaction.display().to_string()),
        actions,
    );
    match session.pending_recovery(body, actions)? {
        tui::PendingRecoveryDecision::Recover => {
            let recovery = recovery
                .context("this host has no retained payload for the device's active transaction")?;
            Box::pin(run_detected_recovery(manifest, session, &recovery)).await?;
            clear_recovery_notice(&receipt_store, &recovery);
        }
        tui::PendingRecoveryDecision::ClearState => {
            let transaction = transaction
                .as_ref()
                .context("host-only recovery receipts cannot clear device state")?;
            device_store.prove_and_clear(transaction).await?;
            if let Some(recovery) = recovery {
                receipt_store.clear(&recovery)?;
            }
        }
        tui::PendingRecoveryDecision::Discard => {
            let recovery = recovery.context("no interrupted preparation is retained")?;
            receipt_store.discard_unprepared(&recovery)?;
        }
        tui::PendingRecoveryDecision::Cancel => {
            return Err(tui::Cancelled::new("pending recovery decision").into());
        }
    }
    Ok(())
}

const fn pending_recovery_kind(kind: DeviceTransactionKind) -> PendingRecoveryKind {
    match kind {
        DeviceTransactionKind::Update => PendingRecoveryKind::Update,
        DeviceTransactionKind::Removal => PendingRecoveryKind::Removal,
    }
}

fn clear_recovery_notice(store: &PendingRecoveryStore, recovery: &PendingRecovery) {
    if let Err(error) = store.clear(recovery) {
        tracing::warn!(
            %error,
            capture = %recovery.transaction.display(),
            "completed recovery notice could not be cleared"
        );
    }
}

async fn run_detected_recovery(
    manifest: &DeviceManifest,
    session: &mut tui::Session,
    recovery: &PendingRecovery,
) -> Result<()> {
    let mount_id = manifest
        .summary
        .location
        .strip_prefix("mounted-mtp:")
        .context("pending recovery requires a desktop-mounted MTP device")?;
    let result = match recovery.kind {
        PendingRecoveryKind::Update => Box::pin(run_update_recovery_in_session(
            session,
            manifest,
            mount_id,
            recovery.transaction.clone(),
        ))
        .await
        .map(|_| ()),
        PendingRecoveryKind::Removal => run_removal_recovery_in_session(
            session,
            manifest,
            mount_id,
            recovery.transaction.clone(),
        )
        .await
        .map(|_| ()),
    };
    if let Err(error) = &result
        && cancellation_step(error).is_none()
    {
        let failure = match recovery.kind {
            PendingRecoveryKind::Update => {
                update_failure_presentation(error, &recovery.transaction)
            }
            PendingRecoveryKind::Removal => {
                removal_recovery_failure_presentation(error, &recovery.transaction)
            }
        };
        if present_failure(session, failure) {
            return Err(PresentedError(result.expect_err("result is known to be an error")).into());
        }
    }
    result
}

async fn run_guided_combined_map_actions(
    response: &MapCatalog,
    manifest: DeviceManifest,
    client: OmtClient,
    runtime: UpdateRuntime<'_>,
    session: &mut tui::Session,
    device: DeviceUpdateAdapter,
) -> Result<()> {
    let cache = runtime
        .cache_override
        .map_or_else(cache_dir, |path| Ok(path.to_owned()))?;
    let selected = select_guided_map_actions(
        response,
        &manifest,
        runtime.service,
        session,
        InteractiveSelectionRuntime {
            device: device.as_ref(),
            cache: &cache,
        },
    )
    .await?;
    if !selected.removals.is_empty() {
        let target = target_for_device(&manifest.summary)?;
        run_guided_removal(
            selected.removals,
            &target,
            response,
            &manifest,
            &runtime,
            session,
        )
        .await?;
    }
    if selected.changes.is_empty() {
        return Ok(());
    }
    run_guided_selected_update(
        Some(selected.changes),
        response,
        manifest,
        client,
        runtime,
        session,
        device,
    )
    .await
}

async fn run_guided_selected_update(
    preselected: Option<Vec<usize>>,
    response: &MapCatalog,
    manifest: DeviceManifest,
    client: OmtClient,
    runtime: UpdateRuntime<'_>,
    session: &mut tui::Session,
    device: DeviceUpdateAdapter,
) -> Result<()> {
    Box::pin(run_selected_update(
        SelectedUpdateRequest {
            selection: MapSelectionArgs {
                maps: Vec::new(),
                all_maps: false,
            },
            preselected,
            concurrency: 4,
            backup_policy: BackupPolicy::Verified,
            device,
            approval: UpdateApproval::User {
                write_consent: DeviceWriteConsent {
                    confirm_device_write: None,
                },
                confirm_plan: None,
            },
        },
        SelectedUpdateContext {
            response,
            manifest,
            client,
            runtime,
            session: Some(session),
        },
    ))
    .await?;
    Ok(())
}

async fn select_guided_device(
    source: &GuidedDeviceSource,
    session: &mut tui::Session,
) -> Result<DeviceManifest> {
    if let GuidedDeviceSource::Fixed(path) = source {
        let manifest = session.inspect_device(load_manifest(path)).await?;
        if session
            .select_device(std::slice::from_ref(&manifest.summary))?
            .is_none()
        {
            return Err(tui::Cancelled::new("device selection").into());
        }
        return Ok(manifest);
    }
    let target = select_guided_target(session).await?;
    inspect_target(&target, Some(session)).await
}

async fn select_guided_target(session: &mut tui::Session) -> Result<TargetArgs> {
    let mut selected_location = None;
    let mut first_scan = true;
    loop {
        let devices = if first_scan {
            first_scan = false;
            session
                .discover_devices(discover_selectable_devices())
                .await?
        } else {
            discover_selectable_devices().await?
        };
        let summaries = devices
            .iter()
            .map(|device| device.summary.clone())
            .collect::<Vec<_>>();
        let selected = selected_location
            .as_ref()
            .and_then(|location| {
                devices
                    .iter()
                    .position(|device| device.summary.location == *location)
            })
            .unwrap_or(0);
        match session.select_device_refreshable(&summaries, selected)? {
            tui::DeviceSelection::Selected(selected) => {
                return devices
                    .into_iter()
                    .nth(selected)
                    .map(|device| device.target)
                    .context("selected device disappeared from the discovered set");
            }
            tui::DeviceSelection::Rescan(selected) => {
                selected_location = devices
                    .get(selected)
                    .map(|device| device.summary.location.clone());
            }
            tui::DeviceSelection::Cancelled => {
                return Err(tui::Cancelled::new("device selection").into());
            }
        }
    }
}

async fn discover_selectable_devices() -> Result<Vec<SelectableDevice>> {
    let mounted_mtp = discover_mounted_mtp();
    let raw_mtp = mounted_mtp
        .is_empty()
        .then(discover_mtp_candidates)
        .unwrap_or_default();
    let mass_storage = discover_mass_storage().await;
    let mut devices = Vec::with_capacity(mass_storage.len() + mounted_mtp.len() + raw_mtp.len());
    for manifest in mass_storage {
        let target = target_for_device(&manifest.summary)?;
        devices.push(SelectableDevice {
            summary: manifest.summary,
            target,
        });
    }
    devices.extend(mounted_mtp.into_iter().map(|candidate| SelectableDevice {
        summary: DeviceSummary {
            transport: TransportKind::MountedMtp,
            model: candidate.name,
            part_number: None,
            software_version: None,
            location: format!("mounted-mtp:{}", candidate.mount_id),
        },
        target: TargetArgs {
            path: None,
            mtp_location: None,
            mounted_mtp: Some(candidate.mount_id),
        },
    }));
    devices.extend(raw_mtp.into_iter().map(|candidate| {
        SelectableDevice {
            summary: DeviceSummary {
                transport: TransportKind::Mtp,
                model: candidate
                    .product
                    .unwrap_or_else(|| "Garmin MTP device".to_owned()),
                part_number: None,
                software_version: None,
                location: format!("mtp:{:#x}", candidate.location_id),
            },
            target: TargetArgs {
                path: None,
                mtp_location: Some(candidate.location_id),
                mounted_mtp: None,
            },
        }
    }));
    Ok(devices)
}

struct FailurePresentation {
    title: &'static str,
    body: tui::FailureBody,
}

async fn retain_and_present_update_failure(
    session: &mut tui::Session,
    error: &anyhow::Error,
    capture: &SessionCapture,
) -> bool {
    retain_failure_report(error, capture, "Update failed.").await;
    let failure = update_failure_presentation(error, capture.root());
    present_failure(session, failure)
}

async fn retain_failure_report(error: &anyhow::Error, capture: &SessionCapture, heading: &str) {
    let details = error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" — ");
    let message = formatdoc! {"
        {heading}

        {details}

        Diagnostics: {}", capture.root().display()};
    if let Err(capture_error) = capture
        .write_bytes(Path::new("failure.txt"), message.as_bytes())
        .await
    {
        tracing::error!(%capture_error, "could not retain the update failure report");
    }
}

fn present_failure(session: &mut tui::Session, failure: FailurePresentation) -> bool {
    if let Err(dialog_error) = session.failure(failure.title, failure.body) {
        tracing::error!(%dialog_error, "could not display the update failure dialog");
        false
    } else {
        true
    }
}

async fn retain_and_present_pipeline_failure(
    session: &mut tui::Session,
    error: &anyhow::Error,
    capture: Option<&SessionCapture>,
) -> bool {
    if let Some(capture) = capture {
        retain_failure_report(error, capture, "Pipeline probe failed.").await;
    }
    let capture_path = capture
        .map(|capture| capture.root().display().to_string())
        .unwrap_or_default();
    let mut failure = pipeline_failure_presentation(error, &capture_path);
    if capture.is_none() {
        failure.body = failure.body.without_diagnostics();
    }
    present_failure(session, failure)
}

fn update_failure_presentation(error: &anyhow::Error, capture: &Path) -> FailurePresentation {
    let capture = capture.display().to_string();
    if let Some(space) = error.downcast_ref::<garmin_update::space::SpaceError>() {
        return space_failure_presentation(space, &capture);
    }
    if let Some(removal) = error.downcast_ref::<garmin_update::RemovalExecutionError>() {
        return removal_failure_presentation(removal, error, &capture);
    }
    if let Some(mounted) = error.downcast_ref::<garmin_update::MountedInstallError>() {
        return mounted_install_failure_presentation(mounted, error, &capture);
    }
    if let Some(download) = error.downcast_ref::<garmin_update::DownloadError>() {
        return download_failure_presentation(download, error, &capture);
    }
    if let Some(service) = error.downcast_ref::<garmin_map_service::OmtError>() {
        return service_failure_presentation(service, &capture);
    }
    FailurePresentation {
        title: "Update failed",
        body: tui::FailureBody::new(
            "The update failed.",
            vec![tui::FailureField::new(
                "Reason",
                error.root_cause().to_string(),
                tui::FailureValueTone::Error,
            )],
            capture,
        ),
    }
}

fn space_failure_presentation(
    space: &garmin_update::space::SpaceError,
    capture: &str,
) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};
    let fields = if let garmin_update::space::SpaceError::Insufficient {
        label,
        required,
        available,
    } = space
    {
        vec![
            FailureField::new("Storage", label, FailureValueTone::Neutral),
            FailureField::new(
                "Required",
                format_bytes(*required),
                FailureValueTone::Neutral,
            ),
            FailureField::new("Free", format_bytes(*available), FailureValueTone::Error),
        ]
    } else {
        vec![FailureField::new(
            "Reason",
            space.to_string(),
            FailureValueTone::Error,
        )]
    };
    FailurePresentation {
        title: "Storage check failed",
        body: FailureBody::new("Storage cannot satisfy this operation.", fields, capture)
            .with_outcome("Writing blocked. Check storage; keep this capture."),
    }
}

fn mounted_install_failure_presentation(
    mounted: &garmin_update::MountedInstallError,
    error: &anyhow::Error,
    capture: &str,
) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};

    match mounted {
        garmin_update::MountedInstallError::Space(space) => {
            space_failure_presentation(space, capture)
        }
        garmin_update::MountedInstallError::RecoveryEvidence(path) => FailurePresentation {
            title: "Recovery evidence mismatch",
            body: FailureBody::new(
                "Recovery cannot safely identify this file.",
                vec![FailureField::new(
                    "File",
                    path.to_string(),
                    FailureValueTone::Path,
                )],
                capture,
            )
            .with_outcome("Files and backups preserved. Inspect before retrying."),
        },
        garmin_update::MountedInstallError::Rollback {
            operation,
            rollback,
        } => FailurePresentation {
            title: "Update recovery required",
            body: FailureBody::new(
                "Automatic rollback could not restore the complete pre-update state.",
                vec![
                    FailureField::new("Update", operation.to_string(), FailureValueTone::Error),
                    FailureField::new("Rollback", rollback.to_string(), FailureValueTone::Error),
                ],
                capture,
            )
            .with_outcome("Keep this capture and run `device recover-update` before retrying."),
        },
        garmin_update::MountedInstallError::UnprotectedMutation {
            operation,
            evidence,
        } => unprotected_mutation_failure_presentation(operation, evidence, capture),
        garmin_update::MountedInstallError::UnexpectedDeviceState { path, state, size } => {
            FailurePresentation {
                title: "Device contents changed",
                body: FailureBody::new(
                    "A device object no longer matches the prepared transaction.",
                    vec![
                        FailureField::new("File", path.to_string(), FailureValueTone::Path),
                        FailureField::new(
                            "State",
                            format!("{state:?}, size {size:?}"),
                            FailureValueTone::Error,
                        ),
                    ],
                    capture,
                )
                .with_outcome("The update stopped before touching an unverified object."),
            }
        }
        garmin_update::MountedInstallError::RecoveryDeviceMismatch => FailurePresentation {
            title: "Wrong recovery device",
            body: FailureBody::new(
                "The recovery capture belongs to a different Garmin device.",
                vec![FailureField::new(
                    "Reason",
                    mounted.to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            )
            .with_outcome("No device file was changed."),
        },
        garmin_update::MountedInstallError::UnsafeJournal(_)
        | garmin_update::MountedInstallError::JournalVersion(_)
        | garmin_update::MountedInstallError::InvalidJournal
        | garmin_update::MountedInstallError::RecoveryPlanMismatch
        | garmin_update::MountedInstallError::JournalJson(_) => {
            rejected_recovery_evidence_presentation(error, capture)
        }
        _ => FailurePresentation {
            title: "Update stopped safely",
            body: FailureBody::new(
                "The mounted-MTP update could not be completed.",
                vec![FailureField::new(
                    "Reason",
                    error.root_cause().to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            )
            .with_outcome("Device targets were left intact or restored from verified backups."),
        },
    }
}

fn rejected_recovery_evidence_presentation(
    error: &anyhow::Error,
    capture: &str,
) -> FailurePresentation {
    FailurePresentation {
        title: "Recovery evidence rejected",
        body: tui::FailureBody::new(
            "The retained mounted-update transaction cannot be used safely.",
            vec![tui::FailureField::new(
                "Reason",
                error.root_cause().to_string(),
                tui::FailureValueTone::Error,
            )],
            capture,
        )
        .with_outcome(
            "This attempt performed no device operation. The interrupted state remains unresolved.",
        ),
    }
}

fn unprotected_mutation_failure_presentation(
    operation: &garmin_update::MountedInstallError,
    evidence: &garmin_update::UnprotectedMutationEvidence,
    capture: &str,
) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};

    let mutation_fields =
        |applied_operations: usize, total_operations: usize, failed_target: &Option<String>| {
            let failed_operations = usize::from(failed_target.is_some());
            let unstarted = total_operations
                .saturating_sub(applied_operations)
                .saturating_sub(failed_operations);
            let mut fields = vec![FailureField::new(
                "Applied",
                format!("{applied_operations} of {total_operations} device operations"),
                FailureValueTone::Error,
            )];
            if let Some(path) = failed_target {
                fields.push(FailureField::new("Failed at", path, FailureValueTone::Path));
            }
            fields.push(FailureField::new(
                "Unstarted",
                format!("{unstarted} device operations"),
                FailureValueTone::Neutral,
            ));
            fields
        };
    let (title, mut fields) = match evidence {
        garmin_update::UnprotectedMutationEvidence::IntentRecorded {
            total_operations,
            failed_target,
        } => (
            "Update incomplete — mutation uncertain",
            mutation_fields(0, *total_operations, failed_target),
        ),
        garmin_update::UnprotectedMutationEvidence::Applied {
            applied_operations,
            total_operations,
            failed_target,
        } => (
            "Update incomplete — device changed",
            mutation_fields(*applied_operations, *total_operations, failed_target),
        ),
        garmin_update::UnprotectedMutationEvidence::Unavailable {
            total_operations,
            reason,
        } => (
            "Update incomplete — evidence unreadable",
            vec![
                FailureField::new(
                    "Planned",
                    format!("{total_operations} device operations"),
                    FailureValueTone::Neutral,
                ),
                FailureField::new("Evidence", reason, FailureValueTone::Error),
            ],
        ),
    };
    fields.push(FailureField::new(
        "Reason",
        operation.to_string(),
        FailureValueTone::Error,
    ));
    FailurePresentation {
        title,
        body: FailureBody::new(
            match evidence {
                garmin_update::UnprotectedMutationEvidence::Applied { .. } => "The update stopped after changing the device. Recovery backups were skipped, so automatic rollback is unavailable.",
                _ => "The update stopped after device mutation became possible. Recovery backups were skipped, so automatic rollback is unavailable.",
            },
            fields,
            capture,
        )
        .with_outcome(
            "Do not start another update. Keep this capture and run `device recover-update` to inspect and resume it.",
        ),
    }
}

fn removal_failure_presentation(
    removal: &garmin_update::RemovalExecutionError,
    error: &anyhow::Error,
    capture: &str,
) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};

    match removal {
        garmin_update::RemovalExecutionError::Space(space) => {
            space_failure_presentation(space, capture)
        }
        garmin_update::RemovalExecutionError::Rollback {
            operation,
            rollback,
        } => FailurePresentation {
            title: "Removal recovery required",
            body: FailureBody::new(
                "Automatic rollback could not restore every selected file.",
                vec![
                    FailureField::new("Removal", operation, FailureValueTone::Error),
                    FailureField::new("Rollback", rollback, FailureValueTone::Error),
                ],
                capture,
            )
            .with_outcome("Keep this capture and run `device recover-removal` before retrying."),
        },
        garmin_update::RemovalExecutionError::UnexpectedDeviceState { path, state, size } => {
            FailurePresentation {
                title: "Device contents changed",
                body: FailureBody::new(
                    "A removal target no longer matches the reviewed plan.",
                    vec![
                        FailureField::new("File", path.to_string(), FailureValueTone::Path),
                        FailureField::new(
                            "State",
                            format!("{state:?}, size {size:?}"),
                            FailureValueTone::Error,
                        ),
                    ],
                    capture,
                )
                .with_outcome("Removal stopped before an unverified object could be deleted."),
            }
        }
        garmin_update::RemovalExecutionError::RecoveryDeviceMismatch => FailurePresentation {
            title: "Wrong recovery device",
            body: FailureBody::new(
                "The recovery capture belongs to a different Garmin device.",
                vec![FailureField::new(
                    "Reason",
                    removal.to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            )
            .with_outcome("No device file was changed."),
        },
        _ => FailurePresentation {
            title: "Removal stopped safely",
            body: FailureBody::new(
                "The selected component files were not removed.",
                vec![FailureField::new(
                    "Reason",
                    error.root_cause().to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            )
            .with_outcome("Device targets were left intact or restored from verified backups."),
        },
    }
}

fn removal_recovery_failure_presentation(
    error: &anyhow::Error,
    transaction: &Path,
) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};

    let removal = error.downcast_ref::<garmin_update::RemovalExecutionError>();
    if let Some(garmin_update::RemovalExecutionError::Space(space)) = removal {
        return space_failure_presentation(space, &transaction.display().to_string());
    }
    let wrong_device = matches!(
        removal,
        Some(garmin_update::RemovalExecutionError::RecoveryDeviceMismatch)
    );
    FailurePresentation {
        title: if wrong_device {
            "Wrong recovery device"
        } else {
            "Removal recovery failed"
        },
        body: FailureBody::new(
            if wrong_device {
                "The recovery capture belongs to a different Garmin device."
            } else {
                "The interrupted removal could not be recovered."
            },
            vec![FailureField::new(
                "Reason",
                error.root_cause().to_string(),
                FailureValueTone::Error,
            )],
            transaction.display().to_string(),
        )
        .with_outcome("No unverified file was written to the device."),
    }
}

fn pipeline_failure_presentation(error: &anyhow::Error, capture: &str) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};

    if let Some(device) = error.downcast_ref::<garmin_device::MountedMtpError>() {
        let (title, summary, fields) = match device.probe_failure() {
            Some(MountedMtpProbeFailure::Upload { reason }) => (
                "Device upload failed",
                "Could not copy the verified file through the desktop MTP mount.",
                vec![FailureField::new("Reason", reason, FailureValueTone::Error)],
            ),
            Some(MountedMtpProbeFailure::UploadCleanup { upload, cleanup }) => (
                "Device cleanup failed",
                "A failed upload left a disposable object that could not be removed.",
                vec![
                    FailureField::new("Upload", upload, FailureValueTone::Error),
                    FailureField::new("Cleanup", cleanup, FailureValueTone::Error),
                ],
            ),
            Some(MountedMtpProbeFailure::ReadBack { reason }) => (
                "Device read-back failed",
                "Could not read the disposable file back through the desktop MTP mount.",
                vec![FailureField::new("Reason", reason, FailureValueTone::Error)],
            ),
            Some(MountedMtpProbeFailure::Size { expected, actual }) => (
                "Device verification failed",
                "The disposable device copy has an unexpected size.",
                vec![
                    FailureField::new(
                        "Expected",
                        format!("{expected} bytes"),
                        FailureValueTone::Neutral,
                    ),
                    FailureField::new(
                        "Received",
                        format!("{actual} bytes"),
                        FailureValueTone::Error,
                    ),
                ],
            ),
            Some(MountedMtpProbeFailure::Checksum) => (
                "Device verification failed",
                "The disposable device copy did not match the uploaded file.",
                vec![FailureField::new(
                    "Reason",
                    device.to_string(),
                    FailureValueTone::Error,
                )],
            ),
            Some(MountedMtpProbeFailure::NotRemoved) => (
                "Device cleanup failed",
                "The disposable device copy could not be removed.",
                vec![FailureField::new(
                    "Reason",
                    device.to_string(),
                    FailureValueTone::Error,
                )],
            ),
            None => (
                "Mounted device operation failed",
                "Could not complete the probe through the desktop MTP mount.",
                vec![FailureField::new(
                    "Reason",
                    error.root_cause().to_string(),
                    FailureValueTone::Error,
                )],
            ),
        };
        return FailurePresentation {
            title,
            body: FailureBody::new(summary, fields, capture)
                .with_outcome("Pipeline probe incomplete; existing Garmin files were not changed."),
        };
    }

    let mut failure = update_failure_presentation(error, Path::new(capture));
    failure.body = failure
        .body
        .with_outcome("Pipeline probe incomplete; existing Garmin files were not changed.");
    failure
}

fn download_failure_presentation(
    download: &garmin_update::DownloadError,
    error: &anyhow::Error,
    capture: &str,
) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};

    match download {
        garmin_update::DownloadError::Space(space) => space_failure_presentation(space, capture),
        garmin_update::DownloadError::Request(request) => {
            let server = request
                .url()
                .and_then(url::Url::host_str)
                .unwrap_or("unknown download host");
            let reason = if request.is_timeout() {
                "Connection timed out".to_owned()
            } else if error.chain().any(|cause| {
                let cause = cause.to_string().to_ascii_lowercase();
                cause.contains("dns error")
                    || cause.contains("failed to lookup address information")
                    || cause.contains("name or service not known")
            }) {
                "DNS lookup failed".to_owned()
            } else {
                error.root_cause().to_string()
            };
            FailurePresentation {
                title: "Download server unavailable",
                body: FailureBody::new(
                    "Could not reach a Garmin download host.",
                    vec![
                        FailureField::new("Server", server, FailureValueTone::Url),
                        FailureField::new("Reason", reason, FailureValueTone::Error),
                    ],
                    capture,
                ),
            }
        }
        garmin_update::DownloadError::HttpStatus(status) => FailurePresentation {
            title: "Download request rejected",
            body: FailureBody::new(
                "All Garmin download hosts rejected the file.",
                vec![FailureField::new(
                    "Response",
                    status.to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            ),
        },
        garmin_update::DownloadError::Checksum {
            path,
            expected,
            actual,
        } => FailurePresentation {
            title: "Downloaded file is corrupt",
            body: FailureBody::new(
                "Downloaded file does not match Garmin's checksum.",
                vec![
                    FailureField::new("File", path.display().to_string(), FailureValueTone::Path),
                    FailureField::new("Expected", expected, FailureValueTone::Neutral),
                    FailureField::new("Received", actual, FailureValueTone::Error),
                ],
                capture,
            ),
        },
        garmin_update::DownloadError::Size {
            path,
            expected,
            actual,
        } => FailurePresentation {
            title: "Downloaded file is incomplete",
            body: FailureBody::new(
                "Downloaded size does not match the plan.",
                vec![
                    FailureField::new("File", path.display().to_string(), FailureValueTone::Path),
                    FailureField::new("Expected", expected.to_string(), FailureValueTone::Neutral),
                    FailureField::new("Received", actual.to_string(), FailureValueTone::Error),
                ],
                capture,
            ),
        },
        _ => FailurePresentation {
            title: "Download failed",
            body: FailureBody::new(
                "Could not download and verify a selected file.",
                vec![FailureField::new(
                    "Reason",
                    download.to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            ),
        },
    }
}

fn service_failure_presentation(
    service: &garmin_map_service::OmtError,
    capture: &str,
) -> FailurePresentation {
    use tui::{FailureBody, FailureField, FailureValueTone};

    match service {
        garmin_map_service::OmtError::Json(details) => FailurePresentation {
            title: "Invalid Garmin response",
            body: FailureBody::new(
                "Could not parse Garmin's update response.",
                vec![FailureField::new(
                    "Parser",
                    details.to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            ),
        },
        garmin_map_service::OmtError::HttpStatus { status, summary } => FailurePresentation {
            title: "Garmin rejected request",
            body: FailureBody::new(
                "Garmin's map-update service rejected the request.",
                vec![
                    FailureField::new("Response", status.to_string(), FailureValueTone::Error),
                    FailureField::new("Details", summary, FailureValueTone::Neutral),
                ],
                capture,
            ),
        },
        garmin_map_service::OmtError::Request(request) => FailurePresentation {
            title: "Garmin service unavailable",
            body: FailureBody::new(
                "Could not reach Garmin's map-update service.",
                vec![FailureField::new(
                    "Reason",
                    request.to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            ),
        },
        _ => FailurePresentation {
            title: "Garmin request failed",
            body: FailureBody::new(
                "The Garmin request failed.",
                vec![FailureField::new(
                    "Reason",
                    service.to_string(),
                    FailureValueTone::Error,
                )],
                capture,
            ),
        },
    }
}

async fn mock_command(
    command: MockCommand,
    json: bool,
    capture: Option<SessionCapture>,
) -> Result<()> {
    let temporary = tempfile::tempdir().context("cannot create disposable mock workspace")?;
    let device = temporary.path().join("device");
    let disposable_cache = temporary.path().join("cache");
    let capture = match capture {
        Some(capture) => capture,
        None => SessionCapture::create(&default_session_capture_path()?)?,
    };
    garmin_simulator::create_fixture(&device).await?;
    let server = garmin_simulator::MockServer::start().await?;
    let base = server.base_url().clone();

    match command {
        MockCommand::Demo => {
            if json {
                bail!("mock demo is interactive and does not support --json");
            }
            let cache = cache_dir()?;
            interactive_mock_demo(device.clone(), &base, Some(&cache), Some(capture)).await?;
            server.shutdown().await?;
            Ok(())
        }
        MockCommand::Test => {
            let manifest = load_manifest(&device).await?;
            let (client, response) = query_device_updates(
                &manifest,
                MapService::Loopback(&base),
                Some(capture.clone()),
            )
            .await?;
            let runtime = UpdateRuntime {
                json: true,
                service: MapService::Loopback(&base),
                cache_override: Some(&disposable_cache),
                capture: Some(capture),
            };
            let result = Box::pin(run_selected_update(
                SelectedUpdateRequest {
                    selection: MapSelectionArgs {
                        maps: Vec::new(),
                        all_maps: true,
                    },
                    preselected: None,
                    concurrency: 4,
                    backup_policy: BackupPolicy::Verified,
                    device: Box::new(SimulatedTarget {
                        source: Box::new(DirectoryDevice::new(device.clone())),
                        fixture: Some(device.clone()),
                    }),
                    approval: UpdateApproval::ScriptedMock,
                },
                SelectedUpdateContext {
                    response: &response,
                    manifest,
                    client,
                    runtime,
                    session: None,
                },
            ))
            .await?;
            let apply = result.execution.apply;
            let artifact = result
                .execution
                .artifact
                .context("mock update omitted its virtual-device artifact")?;
            let fixture =
                garmin_simulator::verify_fixture_payloads(&artifact.join("device/storage-001"))
                    .await?;
            let stats = server.stats();
            if stats.max_parallel_downloads < 2 || stats.active_downloads != 0 {
                bail!("mock server did not observe completed concurrent downloads");
            }
            server.shutdown().await?;
            let report = MockTestReport {
                mock_test: "passed",
                plan: result.plan,
                apply,
                server: stats,
                fixture,
            };
            if json {
                emit_json(&report)
            } else {
                println!("✓ Automated mock update test passed");
                println!("  Files written       {}", report.apply.files_written);
                println!("  Files removed       {}", report.apply.files_removed);
                println!("  Payload integrity   verified");
                println!("  Journal cleanup     verified");
                println!(
                    "  Parallel downloads  {}",
                    report.server.max_parallel_downloads
                );
                Ok(())
            }
        }
    }
}

async fn device(command: DeviceCommand, json: bool) -> Result<()> {
    match command {
        DeviceCommand::List => {
            let mounted = discover_mass_storage().await;
            let mtp = discover_mtp_candidates();
            let mounted_mtp = discover_mounted_mtp();
            let usb_sysfs = discover_garmin_usb_sysfs();
            if json {
                return emit_json(&serde_json::json!({
                    "mass_storage": mounted.iter().map(|item| &item.summary).collect::<Vec<_>>(),
                    "mounted_mtp": mounted_mtp,
                    "mtp_candidates": mtp,
                    "usb_sysfs": usb_sysfs,
                }));
            }
            for item in mounted {
                println!(
                    "{}\tmass-storage\t{}",
                    item.summary.model, item.summary.location
                );
            }
            for item in mounted_mtp {
                println!("{}\tmounted-mtp\tmount={}", item.name, item.mount_id);
            }
            for item in mtp {
                println!(
                    "{}\tmtp\t{:04x}:{:04x}\tlocation={}",
                    item.product.as_deref().unwrap_or("Garmin device"),
                    item.vendor_id,
                    item.product_id,
                    item.location_id
                );
            }
            for item in usb_sysfs {
                println!(
                    "{}\tusb-visible\t{:04x}:{:04x}\tsysfs={}",
                    item.product.as_deref().unwrap_or("Garmin device"),
                    item.vendor_id,
                    item.product_id,
                    item.sysfs_path.display()
                );
            }
            Ok(())
        }
        DeviceCommand::Inspect(args) => {
            let manifest = load_target(&args).await?;
            if json {
                emit_json(&manifest.summary)
            } else {
                println!("Model: {}", manifest.summary.model);
                println!(
                    "Part number: {}",
                    manifest.summary.part_number.as_deref().unwrap_or("unknown")
                );
                println!(
                    "Software: {}",
                    manifest
                        .summary
                        .software_version
                        .as_deref()
                        .unwrap_or("unknown")
                );
                println!("Transport: {:?}", manifest.summary.transport);
                println!("Location: {}", manifest.summary.location);
                println!("Identity digest: {}", manifest.identity_digest());
                Ok(())
            }
        }
        DeviceCommand::Recover(args) => recover_device(args, json).await,
        DeviceCommand::RecoverUpdate(args) => {
            Box::pin(recover_mounted_update_device(args, json)).await
        }
        DeviceCommand::RecoverRemoval(args) => recover_removal_device(args, json).await,
    }
}

async fn recover_device(args: RecoverArgs, json: bool) -> Result<()> {
    let manifest = load_manifest(&args.path).await?;
    let device = device_identification(&manifest);
    let message = formatdoc! {"
        Device: {device}

        Inspect garmin-cli's transaction journal.
        Roll back an interrupted update or clean up a committed one.
        Garmin is not contacted."};
    authorize_device_write(&args.write_consent, json, &message, None)?;
    let outcome = recover_mass_storage(&args.path).await?;
    if json {
        emit_json(&serde_json::json!({
            "device": manifest.summary,
            "outcome": outcome,
        }))
    } else {
        let message = match outcome {
            RecoveryOutcome::NoTransaction => "No interrupted transaction was found.",
            RecoveryOutcome::RolledBack => {
                "The interrupted transaction was rolled back to the previous device state."
            }
            RecoveryOutcome::Finalized => {
                "The committed transaction was preserved and its temporary files were removed."
            }
        };
        println!("{message}");
        Ok(())
    }
}

async fn run_update_recovery_in_session(
    session: &mut tui::Session,
    manifest: &DeviceManifest,
    mount_id: &str,
    transaction: PathBuf,
) -> Result<garmin_update::MountedUpdateRecoveryReport> {
    let (progress, receiver) = ProgressReporter::channel();
    let cancellation = progress.cancellation_token();
    let recovery = RecoveryExecution {
        transaction,
        device_digest: manifest.identity_digest(),
        device: Box::new(MountedMtpDevice::new(mount_id)),
        progress,
    };
    let task = async { recover_update(recovery).await };
    Box::pin(session.run_update_recovery_progress(receiver, task, cancellation)).await
}

async fn run_removal_recovery_in_session(
    session: &mut tui::Session,
    manifest: &DeviceManifest,
    mount_id: &str,
    transaction: PathBuf,
) -> Result<garmin_update::RemovalRecoveryReport> {
    let adapter = MountedMtpDevice::new(mount_id);
    let (progress, receiver) = ProgressReporter::channel();
    let cancellation = progress.cancellation_token();
    let task = async {
        Ok::<_, anyhow::Error>(
            recover_removal(
                &transaction,
                &manifest.identity_digest(),
                &progress,
                &adapter,
            )
            .await?,
        )
    };
    session
        .run_removal_recovery_progress(receiver, task, cancellation)
        .await
}

async fn recover_mounted_update_device(args: MountedUpdateRecoveryArgs, json: bool) -> Result<()> {
    let interactive = interactive_confirmation_available(json);
    let mut session = interactive.then(tui::Session::open).transpose()?;
    let target = resolve_optional_target(&args.target, json, session.as_mut()).await?;
    let manifest = inspect_target(&target, session.as_mut()).await?;
    let mount_id = mounted_removal_target(&target)?;
    let pending_store = PendingRecoveryStore::application()?;
    let pending = pending_store.register_capture(PendingRecoveryKind::Update, &args.transaction)?;
    let body = tui::ConfirmationBody {
        introduction: ratatui::text::Text::raw(indoc! {"
            Inspect the captured transaction.
            Keep a committed update, resume a backup-free one, or restore from verified backups."}),
        fields: vec![
            tui::ConfirmationField::new(
                "Device",
                update_confirmation_device(&manifest),
                tui::ConfirmationValueTone::Neutral,
            ),
            tui::ConfirmationField::new(
                "Recovery",
                args.transaction.display().to_string(),
                tui::ConfirmationValueTone::Path,
            ),
        ],
        note: None,
    };
    authorize_device_write_details(
        &args.write_consent,
        json,
        "Recover interrupted update?",
        body,
        "Recover update",
        session.as_mut(),
    )?;
    let result: Result<_> = if let Some(session) = session.as_mut() {
        Box::pin(run_update_recovery_in_session(
            session,
            &manifest,
            mount_id,
            args.transaction.clone(),
        ))
        .await
    } else {
        recover_update(RecoveryExecution {
            transaction: args.transaction.clone(),
            device_digest: manifest.identity_digest(),
            device: Box::new(MountedMtpDevice::new(mount_id)),
            progress: ProgressReporter::default(),
        })
        .await
    };
    if let Err(error) = &result
        && cancellation_step(error).is_none()
        && let Some(session) = session.as_mut()
        && present_failure(
            session,
            update_failure_presentation(error, &args.transaction),
        )
    {
        return Err(PresentedError(result.expect_err("result is known to be an error")).into());
    }
    drop(session);
    let result = result?;
    clear_recovery_notice(&pending_store, &pending);
    if json {
        emit_json(&result)
    } else if interactive {
        Ok(())
    } else {
        println!(
            "Recovery {:?}: checked {} transaction objects.",
            result.outcome, result.files_checked
        );
        Ok(())
    }
}

async fn recover_removal_device(args: RemovalRecoveryArgs, json: bool) -> Result<()> {
    let interactive = interactive_confirmation_available(json);
    let mut session = interactive.then(tui::Session::open).transpose()?;
    let target = resolve_optional_target(&args.target, json, session.as_mut()).await?;
    let manifest = inspect_target(&target, session.as_mut()).await?;
    let mount_id = mounted_removal_target(&target)?;
    let pending_store = PendingRecoveryStore::application()?;
    let pending =
        pending_store.register_capture(PendingRecoveryKind::Removal, &args.transaction)?;
    let body = tui::ConfirmationBody {
        introduction: ratatui::text::Text::raw(
            "Restore any files missing from the interrupted removal.",
        ),
        fields: vec![
            tui::ConfirmationField::new(
                "Device",
                update_confirmation_device(&manifest),
                tui::ConfirmationValueTone::Neutral,
            ),
            tui::ConfirmationField::new(
                "Recovery",
                args.transaction.display().to_string(),
                tui::ConfirmationValueTone::Path,
            ),
        ],
        note: None,
    };
    authorize_device_write_details(
        &args.write_consent,
        json,
        "Recover interrupted removal?",
        body,
        "Recover files",
        session.as_mut(),
    )?;
    let result: Result<_> = if let Some(session) = session.as_mut() {
        run_removal_recovery_in_session(session, &manifest, mount_id, args.transaction.clone())
            .await
    } else {
        recover_removal(
            &args.transaction,
            &manifest.identity_digest(),
            &ProgressReporter::default(),
            &MountedMtpDevice::new(mount_id),
        )
        .await
        .map_err(Into::into)
    };
    if let Err(error) = &result
        && cancellation_step(error).is_none()
        && let Some(session) = session.as_mut()
        && present_failure(
            session,
            removal_recovery_failure_presentation(error, &args.transaction),
        )
    {
        return Err(PresentedError(result.expect_err("result is known to be an error")).into());
    }
    drop(session);
    let result = result?;
    clear_recovery_notice(&pending_store, &pending);
    if json {
        emit_json(&result)
    } else if interactive {
        Ok(())
    } else {
        println!(
            "Recovery {:?}: verified {}, restored {}.",
            result.outcome, result.files_verified, result.files_restored
        );
        Ok(())
    }
}

async fn updates(
    command: UpdateCommand,
    runtime: UpdateRuntime<'_>,
    session: Option<tui::Session>,
) -> Result<()> {
    let interactive = interactive_confirmation_available(runtime.json);
    let mut session = match (session, &command, interactive) {
        (Some(session), _, _) => Some(session),
        (None, UpdateCommand::Apply(_) | UpdateCommand::Remove(_), true) => Some(
            tui::Session::open_with_profile(runtime.service.profile(tui::DeviceChanges::Enabled))?,
        ),
        (None, UpdateCommand::RemovalPlan(_), true) => Some(tui::Session::open()?),
        (None, _, _) => None,
    };
    let capture = runtime.capture.clone();
    let json = runtime.json;
    let result = updates_flow(command, runtime, session.as_mut()).await;
    let presented = if let (Err(error), Some(session), Some(capture)) =
        (&result, session.as_mut(), capture.as_ref())
        && cancellation_step(error).is_none()
    {
        retain_and_present_update_failure(session, error, capture).await
    } else {
        false
    };
    drop(session);
    if presented {
        let Err(error) = result else {
            unreachable!("only errors can be presented");
        };
        return Err(PresentedError(error).into());
    }
    match result? {
        Some(UpdateCommandOutput::RemovalPlan(report)) => print_removal_plan(&report, json),
        Some(UpdateCommandOutput::RemovalApplied(report)) => {
            print_removal_apply_report(&report, json, interactive)
        }
        None => Ok(()),
    }
}

async fn updates_flow(
    command: UpdateCommand,
    runtime: UpdateRuntime<'_>,
    mut session: Option<&mut tui::Session>,
) -> Result<Option<UpdateCommandOutput>> {
    require_update_capture(&command, &runtime)?;
    let removal_target = match &command {
        UpdateCommand::RemovalPlan(args) => {
            Some(resolve_optional_target(&args.target, runtime.json, session.as_deref_mut()).await?)
        }
        UpdateCommand::Remove(args) => Some(
            resolve_optional_target(&args.plan.target, runtime.json, session.as_deref_mut())
                .await?,
        ),
        _ => None,
    };
    let (contact_consent, may_download) = update_contact_policy(&command);
    if !runtime.service.is_mock() {
        authorize_garmin_contact(
            contact_consent,
            runtime.json,
            may_download,
            session.as_deref_mut(),
        )?;
    }
    let manifest =
        load_update_command_manifest(&command, removal_target.as_ref(), runtime.service.is_mock())
            .await?;
    let (client, response) = runtime.query(&manifest, session.as_deref_mut()).await?;
    match command {
        UpdateCommand::Check(_) => {
            if runtime.json {
                emit_json(&response)?;
            } else {
                print_updates(&response);
            }
            Ok(None)
        }
        UpdateCommand::Plan(args) => {
            let plan = select_update_plan(
                &response,
                &manifest,
                &args.selection,
                runtime.service,
                runtime.json,
                None,
            )?
            .with_backup_policy(args.backup.into())?;
            if let Some(capture) = &runtime.capture {
                capture.write_json(Path::new("plan.json"), &plan).await?;
            }
            print_plan(&plan, runtime.json)?;
            Ok(None)
        }
        UpdateCommand::RemovalPlan(args) => {
            let report = run_removal_plan(
                args,
                require_removal_target(removal_target.as_ref())?,
                &response,
                &manifest,
                &runtime,
            )
            .await?;
            Ok(Some(UpdateCommandOutput::RemovalPlan(report)))
        }
        UpdateCommand::Remove(args) => {
            let report = run_removal(
                args,
                require_removal_target(removal_target.as_ref())?,
                &response,
                &manifest,
                &runtime,
                session,
            )
            .await?;
            Ok(Some(UpdateCommandOutput::RemovalApplied(report)))
        }
        UpdateCommand::Apply(args) => {
            run_apply_command(args, &response, manifest, client, runtime, session).await?;
            Ok(None)
        }
    }
}

async fn run_apply_command(
    args: ApplyArgs,
    response: &MapCatalog,
    manifest: DeviceManifest,
    client: OmtClient,
    runtime: UpdateRuntime<'_>,
    mut session: Option<&mut tui::Session>,
) -> Result<()> {
    let ApplyArgs {
        target: _,
        write_consent,
        selection,
        backup,
        confirm_plan,
        concurrency,
        ..
    } = args;
    let device = GuidedDeviceAdapter::Production(CommitPolicy::Apply).resolve(&manifest)?;
    if let Some(session) = session.as_deref_mut() {
        let state = session.load_device_state(device.state()).await?;
        session.set_device_state(state);
    }
    let result = Box::pin(run_selected_update(
        SelectedUpdateRequest {
            selection,
            preselected: None,
            concurrency,
            backup_policy: backup.into(),
            device,
            approval: UpdateApproval::User {
                write_consent,
                confirm_plan,
            },
        },
        SelectedUpdateContext {
            response,
            manifest,
            client,
            runtime: runtime.clone(),
            session,
        },
    ))
    .await?;
    print_apply_report(
        &result.execution.apply,
        &runtime,
        interactive_confirmation_available(runtime.json),
    )
}

const fn update_contact_policy(command: &UpdateCommand) -> (&GarminContactConsent, bool) {
    match command {
        UpdateCommand::Check(args) => (&args.contact_consent, false),
        UpdateCommand::Plan(args) => (&args.contact_consent, false),
        UpdateCommand::RemovalPlan(args) => (&args.contact_consent, false),
        UpdateCommand::Remove(args) => (&args.plan.contact_consent, false),
        UpdateCommand::Apply(args) => (&args.contact_consent, true),
    }
}

async fn load_update_command_manifest(
    command: &UpdateCommand,
    removal_target: Option<&TargetArgs>,
    mock_service: bool,
) -> Result<DeviceManifest> {
    match command {
        UpdateCommand::Check(args) => load_target(&args.target).await,
        UpdateCommand::Plan(args) => load_target(&args.target).await,
        UpdateCommand::RemovalPlan(_) | UpdateCommand::Remove(_) => {
            load_target(require_removal_target(removal_target)?).await
        }
        UpdateCommand::Apply(args) => {
            if mock_service {
                let path = args
                    .target
                    .path
                    .as_deref()
                    .context("mock updates require --path to a synthetic device")?;
                require_mock_device_root(path)
                    .context("mock updates require a valid synthetic device")?;
            }
            load_target(&args.target).await
        }
    }
}

fn require_removal_target(target: Option<&TargetArgs>) -> Result<&TargetArgs> {
    target.context("removal command has no resolved device")
}

fn require_update_capture(command: &UpdateCommand, runtime: &UpdateRuntime<'_>) -> Result<()> {
    if matches!(command, UpdateCommand::Remove(_)) && runtime.service.is_mock() {
        bail!("component removal cannot use a mock map service with a physical device");
    }
    if !runtime.service.is_mock() && runtime.capture.is_none() {
        match command {
            UpdateCommand::Apply(_) => {
                bail!(
                    "real installs require --capture NEW_DIRECTORY for transaction and server evidence"
                );
            }
            UpdateCommand::Remove(_) => {
                bail!("component removal requires --capture NEW_DIRECTORY for recovery");
            }
            _ => {}
        }
    }
    Ok(())
}

async fn run_guided_removal(
    selected: Vec<usize>,
    target: &TargetArgs,
    response: &MapCatalog,
    manifest: &DeviceManifest,
    runtime: &UpdateRuntime<'_>,
    session: &mut tui::Session,
) -> Result<()> {
    let report = build_removal_plan(selected, target, response, manifest, runtime).await?;
    execute_removal_report(
        report,
        target,
        manifest,
        runtime,
        Some(session),
        &DeviceWriteConsent {
            confirm_device_write: None,
        },
    )
    .await?;
    Ok(())
}

async fn run_removal_plan(
    args: RemovalPlanArgs,
    target: &TargetArgs,
    response: &MapCatalog,
    manifest: &DeviceManifest,
    runtime: &UpdateRuntime<'_>,
) -> Result<RemovalPlanReport> {
    let selected = select_removal_components(response, &args.selection)?;
    build_removal_plan(selected, target, response, manifest, runtime).await
}

async fn build_removal_plan(
    selected: Vec<usize>,
    target: &TargetArgs,
    response: &MapCatalog,
    manifest: &DeviceManifest,
    runtime: &UpdateRuntime<'_>,
) -> Result<RemovalPlanReport> {
    let plan =
        RemovalPlan::from_response_selection(response, manifest.identity_digest(), selected)?;
    let inventory = inventory_target(target, &plan.paths_to_inventory()).await?;
    let execution = plan.bind_inventory(&inventory)?;
    if let Some(capture) = &runtime.capture {
        capture
            .write_json(Path::new("removal-plan.json"), &plan)
            .await?;
        capture
            .write_json(Path::new("device-inventory.json"), &inventory)
            .await?;
        capture
            .write_json(Path::new("removal-execution-plan.json"), &execution)
            .await?;
    }
    Ok(RemovalPlanReport {
        service_plan: plan,
        execution_plan: execution,
    })
}

async fn run_removal(
    args: RemoveArgs,
    target: &TargetArgs,
    response: &MapCatalog,
    manifest: &DeviceManifest,
    runtime: &UpdateRuntime<'_>,
    session: Option<&mut tui::Session>,
) -> Result<RemovalApplyReport> {
    let report = run_removal_plan(args.plan, target, response, manifest, runtime).await?;
    if args.confirm_plan != report.execution_plan.digest {
        bail!(
            "removal plan changed; review `updates removal-plan` and pass --confirm-plan {}",
            report.execution_plan.digest
        );
    }
    execute_removal_report(
        report,
        target,
        manifest,
        runtime,
        session,
        &args.write_consent,
    )
    .await
}

async fn execute_removal_report(
    report: RemovalPlanReport,
    target: &TargetArgs,
    manifest: &DeviceManifest,
    runtime: &UpdateRuntime<'_>,
    mut session: Option<&mut tui::Session>,
    write_consent: &DeviceWriteConsent,
) -> Result<RemovalApplyReport> {
    let capture = runtime
        .capture
        .clone()
        .context("component removal requires --capture NEW_DIRECTORY")?;
    if report.execution_plan.files_to_remove.is_empty() {
        bail!("no selected component files are present on the device; nothing was changed");
    }
    let mount_id = mounted_removal_target(target)?;
    let adapter = MountedMtpDevice::new(mount_id);
    if let Some(session) = session.as_deref_mut() {
        let state = session
            .load_device_state(async {
                adapter.state_snapshot().await.map_err(anyhow::Error::from)
            })
            .await?;
        session.set_device_state(state);
    }
    let removed_components = report
        .service_plan
        .components
        .iter()
        .filter(|component| component.disposition == garmin_update::ComponentDisposition::Remove)
        .map(|component| component.name.as_str())
        .collect::<Vec<_>>();
    let confirmation_title = if removed_components.len() == 1 {
        "Remove map component?"
    } else {
        "Remove map components?"
    };
    let body = tui::removal_confirmation_body(
        update_confirmation_device(manifest),
        &report.execution_plan.digest,
        removed_components.join(", "),
        report.execution_plan.files_to_remove.len(),
        format_bytes(report.execution_plan.bytes_to_remove),
        capture.root().display().to_string(),
    );
    authorize_device_write_details(
        write_consent,
        runtime.json,
        confirmation_title,
        body,
        "Remove files",
        session.as_deref_mut(),
    )?;
    let current = load_target(target).await?;
    if current.identity_digest() != report.execution_plan.device_digest {
        bail!("the selected device changed after the removal plan was confirmed");
    }
    let pending_store = PendingRecoveryStore::application()?;
    let pending = pending_store.register(
        PendingRecoveryKind::Removal,
        capture.root(),
        &report.execution_plan.device_digest,
        &report.execution_plan.digest,
    )?;
    let result = if let Some(session) = session {
        let (progress, receiver) = ProgressReporter::channel();
        let cancellation = progress.cancellation_token();
        let progress = captured_progress(capture.clone(), &progress);
        let task = async {
            Ok::<_, anyhow::Error>(
                execute_removal(&report.execution_plan, &capture, &progress, &adapter).await?,
            )
        };
        session
            .run_removal_progress(receiver, task, cancellation)
            .await?
    } else {
        let downstream = ProgressReporter::default();
        let progress = captured_progress(capture.clone(), &downstream);
        execute_removal(&report.execution_plan, &capture, &progress, &adapter).await?
    };
    capture
        .write_json(Path::new("removal-report.json"), &result)
        .await?;
    clear_recovery_notice(&pending_store, &pending);
    Ok(result)
}

fn mounted_removal_target(target: &TargetArgs) -> Result<&str> {
    target
        .mounted_mtp
        .as_deref()
        .context("removal requires desktop-mounted MTP; planning supports every transport")
}

fn print_removal_apply_report(
    report: &RemovalApplyReport,
    json: bool,
    interactive: bool,
) -> Result<()> {
    if json {
        return emit_json(report);
    }
    if !interactive {
        let palette = HumanPalette::stdout();
        println!(
            "{} {} totaling {}; verified backups remain in {}.",
            palette.paint("Removed", AnsiColor::Green, true),
            palette.paint(file_count(report.files_removed), AnsiColor::White, true),
            palette.paint(format_bytes(report.bytes_removed), AnsiColor::White, true),
            palette.paint(report.backup_directory.display(), AnsiColor::Cyan, false)
        );
    }
    Ok(())
}

async fn run_selected_update(
    request: SelectedUpdateRequest,
    mut context: SelectedUpdateContext<'_>,
) -> Result<SelectedUpdateResult> {
    let cache = context
        .runtime
        .cache_override
        .map_or_else(cache_dir, |path| Ok(path.to_owned()))?;
    let mut selection = if let Some(selected) = &request.preselected {
        build_selected_update_plan(
            context.response,
            context.manifest.identity_digest(),
            selected.clone(),
            context.runtime.service,
        )?
    } else {
        select_update_plan_for_run(
            context.response,
            &context.manifest,
            &request.selection,
            context.runtime.service,
            context.runtime.json,
            context.session.as_deref_mut(),
            InteractiveSelectionRuntime {
                device: request.device.as_ref(),
                cache: &cache,
            },
        )
        .await?
    };
    if selection.plan.downloads.is_empty() {
        bail!("select at least one map component before continuing");
    }
    selection.plan = selection.plan.with_backup_policy(request.backup_policy)?;
    let capture = context
        .runtime
        .capture
        .clone()
        .context("executing an update requires a session capture")?;
    authorize_selected_update(&request, &mut selection, &capture, &mut context)?;
    capture
        .write_json(Path::new("plan.json"), &selection.plan)
        .await?;
    let plan_for_result = selection.plan.clone();
    let execution = Box::pin(execute_selected_update(
        request,
        selection.plan,
        cache,
        capture,
        context,
    ))
    .await?;
    Ok(SelectedUpdateResult {
        plan: plan_for_result,
        execution,
    })
}

fn authorize_selected_update(
    request: &SelectedUpdateRequest,
    selection: &mut SelectedUpdatePlan,
    capture: &SessionCapture,
    context: &mut SelectedUpdateContext<'_>,
) -> Result<()> {
    match &request.approval {
        UpdateApproval::User {
            write_consent,
            confirm_plan,
        } => {
            if confirm_plan
                .as_deref()
                .is_some_and(|digest| digest != selection.plan.digest)
            {
                bail!(
                    "plan changed; inspect `updates plan` and pass --confirm-plan {}",
                    selection.plan.digest
                );
            }
            let interactive = interactive_confirmation_available(context.runtime.json);
            let write_prompt = request.device.modifies_device()
                && should_prompt(
                    write_consent.confirm_device_write.as_ref(),
                    interactive,
                    "--confirm-device-write WRITE-DEVICE",
                )?;
            let plan_prompt = should_prompt(
                confirm_plan.as_ref(),
                interactive,
                "--confirm-plan <PLAN_DIGEST>",
            )?;
            if write_prompt || plan_prompt {
                let verified_plan = selection
                    .plan
                    .clone()
                    .with_backup_policy(BackupPolicy::Verified)?;
                let skipped_plan = selection
                    .plan
                    .clone()
                    .with_backup_policy(BackupPolicy::Skip)?;
                let body = tui::update_confirmation_body(
                    update_confirmation_device(&context.manifest),
                    &selection.components,
                    &selection.plan.digest,
                    selection.plan.downloads.len(),
                    format_bytes(selection.plan.total_bytes),
                    selection.plan.files_to_remove.len(),
                    capture.root().display().to_string(),
                )
                .with_backup_plan_ids(&verified_plan.digest, &skipped_plan.digest)
                .with_backup_enabled(selection.plan.backup_policy == BackupPolicy::Verified);
                let decision = context
                    .session
                    .as_deref_mut()
                    .context("interactive update session is unavailable")?
                    .confirm_update(body)?;
                if !decision.confirmed {
                    return Err(tui::Cancelled::new("update confirmation").into());
                }
                selection.plan = if decision.backup_enabled {
                    verified_plan
                } else {
                    skipped_plan
                };
                if confirm_plan
                    .as_deref()
                    .is_some_and(|digest| digest != selection.plan.digest)
                {
                    bail!(
                        "backup choice changed the plan; pass --confirm-plan {}",
                        selection.plan.digest
                    );
                }
            }
        }
        UpdateApproval::ScriptedMock => {
            if !context.runtime.service.is_mock() {
                bail!("scripted update approval is restricted to the loopback mock service");
            }
            let device = request
                .device
                .fixture_root()
                .context("scripted updates require a synthetic-device source")?;
            require_mock_device_root(device)
                .context("scripted updates require a marked synthetic device")?;
        }
    }
    Ok(())
}

async fn execute_selected_update(
    request: SelectedUpdateRequest,
    plan: UpdatePlan,
    cache: PathBuf,
    capture: SessionCapture,
    context: SelectedUpdateContext<'_>,
) -> Result<UpdateOutcome> {
    let pending = if request.device.modifies_device()
        && request.device.fixture_root().is_none()
        && context.manifest.summary.transport == TransportKind::MountedMtp
    {
        let store = PendingRecoveryStore::application()?;
        let recovery = store.register(
            PendingRecoveryKind::Update,
            capture.root(),
            &plan.device_digest,
            &plan.digest,
        )?;
        Some((store, recovery))
    } else {
        None
    };
    let SelectedUpdateRequest {
        concurrency,
        device,
        ..
    } = request;
    let mut execution = UpdateExecution {
        client: context.client,
        manifest: context.manifest,
        plan,
        cache,
        concurrency,
        download_authorizer: std::sync::Arc::new(GarminDownloadAuthorizer),
        progress: ProgressReporter::default(),
        capture,
        device,
    };
    let result = if let Some(session) = context.session {
        let (screen_progress, receiver) = ProgressReporter::channel();
        let cancellation = screen_progress.cancellation_token();
        execution.progress = screen_progress;
        let backup_policy = execution.plan.backup_policy;
        let task = execute_update_plan(execution);
        Box::pin(session.run_update_progress(receiver, task, cancellation, backup_policy)).await
    } else {
        eprintln!(
            "Downloading and verifying {} files…",
            execution.plan.downloads.len()
        );
        if !execution.plan.identifiers.is_empty() {
            eprintln!("Obtaining device-bound map authorization data…");
        }
        execute_update_plan(execution).await
    };
    if let Some((store, recovery)) = pending {
        if result.is_ok() {
            clear_recovery_notice(&store, &recovery);
        } else {
            match recovery.is_prepared() {
                Ok(false) => clear_recovery_notice(&store, &recovery),
                Ok(true) => {}
                Err(error) => tracing::warn!(
                    %error,
                    capture = %recovery.transaction.display(),
                    "pending update state could not be inspected"
                ),
            }
        }
    }
    result
}

fn print_apply_report(
    report: &garmin_update::ApplyReport,
    runtime: &UpdateRuntime<'_>,
    interactive: bool,
) -> Result<()> {
    if runtime.json {
        return emit_json(report);
    }
    if interactive {
        return Ok(());
    }
    println!(
        "wrote {} files ({}) at {}; removed {}; wrote {} unlocks",
        report.files_written,
        format_bytes(report.bytes_written),
        format_rate(report.bytes_per_second()),
        report.files_removed,
        report.unlocks_written
    );
    match report.backup_policy {
        BackupPolicy::Verified => println!("Verified recovery backups were retained."),
        BackupPolicy::Skip => println!(
            "Recovery backup was skipped; automatic rollback is unavailable for this update."
        ),
    }
    match report.recovery {
        RecoveryOutcome::NoTransaction => {}
        RecoveryOutcome::RolledBack => {
            println!("Recovered and rolled back an earlier interrupted transaction.");
        }
        RecoveryOutcome::Finalized => {
            println!("Finished cleanup from an earlier committed transaction.");
        }
    }
    Ok(())
}

fn captured_progress(capture: SessionCapture, downstream: &ProgressReporter) -> ProgressReporter {
    downstream.observe(move |event| {
        tracing::trace!(
            scope = ?event.scope,
            stage = ?event.stage,
            state = ?event.state,
            unit = ?event.unit,
            label = %event.label,
            path = ?event.path,
            completed = event.completed,
            total = ?event.total,
            "pipeline progress"
        );
        let _ = capture.append_event(event);
    })
}

async fn benchmark(
    command: BenchmarkCommand,
    json: bool,
    mock_server: Option<&url::Url>,
    capture: Option<SessionCapture>,
) -> Result<()> {
    match command {
        BenchmarkCommand::Link(args) => benchmark_link(args, json, capture.as_ref()).await,
        BenchmarkCommand::Pipeline(args) => {
            Box::pin(benchmark_pipeline(args, json, mock_server, capture)).await
        }
    }
}

async fn benchmark_link(
    args: BenchmarkLinkArgs,
    json: bool,
    capture: Option<&SessionCapture>,
) -> Result<()> {
    let interactive = !json && std::io::stdout().is_terminal();
    let mut session = interactive.then(tui::Session::open).transpose()?;
    let target = resolve_optional_target(&args.target, json, session.as_mut()).await?;
    let (manifest, raw_mtp_session) =
        inspect_link_benchmark_target(&target, session.as_mut()).await?;
    authorize_device_write(
        &args.write_consent,
        json,
        LINK_BENCHMARK_WARNING,
        session.as_mut(),
    )?;
    let bytes = args.size.as_u64();
    let staging = cache_dir()?.join("link-probes");
    tokio::fs::create_dir_all(&staging).await?;
    let source = staging.join(format!("{}.bin", uuid::Uuid::new_v4()));
    let file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&source)
        .await?;
    file.set_len(bytes).await?;
    drop(file);
    let device_identity = manifest.identity_digest();
    let recovery_root = cache_dir()?.join("link-probes").join("pending");
    let result = run_link_benchmark(
        &target,
        &source,
        &recovery_root,
        &device_identity,
        capture,
        raw_mtp_session,
        session.as_mut(),
    )
    .await;
    tokio::fs::remove_file(&source)
        .await
        .context("unable to remove sparse host benchmark source")?;
    let report = result?;
    capture_benchmark_report(capture, &report).await?;
    drop(session);
    if json {
        emit_json(&report)
    } else {
        print_link_benchmark(&report);
        Ok(())
    }
}

async fn run_link_benchmark(
    target: &TargetArgs,
    source: &Path,
    recovery_root: &Path,
    device_identity: &str,
    capture: Option<&SessionCapture>,
    raw_mtp_session: Option<RawMtpSession>,
    session: Option<&mut tui::Session>,
) -> Result<DeviceProbeReport> {
    let (reporter, receiver) = ProgressReporter::channel();
    let cancellation = reporter.cancellation_token();
    let progress = capture.map_or_else(
        || reporter.clone(),
        |capture| captured_progress(capture.clone(), &reporter),
    );
    let task = async move {
        match raw_mtp_session {
            Some(raw_mtp_session) => {
                pipeline::probe_raw_mtp_session(
                    raw_mtp_session,
                    source,
                    recovery_root,
                    device_identity,
                    &progress,
                )
                .await
            }
            None => {
                pipeline::probe_device_link(
                    target,
                    source,
                    recovery_root,
                    device_identity,
                    &progress,
                )
                .await
            }
        }
    };
    match session {
        Some(session) => {
            Box::pin(session.run_link_benchmark_progress(receiver, task, cancellation)).await
        }
        None => task.await,
    }
}

fn print_link_benchmark(report: &DeviceProbeReport) {
    println!("Device link benchmark");
    println!(
        "  Upload       {} ({} in {:.2}s)",
        format_rate(report.upload_bytes_per_second()),
        format_bytes(report.bytes),
        report.upload_seconds
    );
    println!(
        "  Finalize     {:.2}s ({})",
        report.device_finalize_seconds,
        match report.upload_completion {
            garmin_device::UploadCompletion::Acknowledged => "device acknowledged",
            garmin_device::UploadCompletion::ReconciledAfterTimeout => {
                "verified after response timeout"
            }
        }
    );
    println!(
        "  Read back    {} ({} in {:.2}s; SHA-256 verified)",
        format_rate(report.read_back_bytes_per_second()),
        format_bytes(report.bytes),
        report.read_back_seconds
    );
    println!(
        "  Cleanup      {:.2}s; temporary file removed",
        report.cleanup_seconds
    );
}

async fn capture_benchmark_report(
    capture: Option<&SessionCapture>,
    report: &impl Serialize,
) -> Result<()> {
    if let Some(capture) = capture {
        capture
            .write_json(Path::new("benchmark-report.json"), report)
            .await?;
    }
    Ok(())
}

async fn benchmark_pipeline(
    args: PipelineBenchmarkArgs,
    json: bool,
    mock_server: Option<&url::Url>,
    capture: Option<SessionCapture>,
) -> Result<()> {
    let interactive = !json && std::io::stdout().is_terminal();
    let mut session = interactive.then(tui::Session::open).transpose()?;
    let result = Box::pin(benchmark_pipeline_flow(
        args,
        json,
        mock_server,
        capture.clone(),
        session.as_mut(),
    ))
    .await;
    if let (Err(error), Some(session)) = (&result, session.as_mut())
        && cancellation_step(error).is_none()
        && retain_and_present_pipeline_failure(session, error, capture.as_ref()).await
    {
        return Err(PresentedError(result.expect_err("the result is known to be an error")).into());
    }
    drop(session);
    let report = result?;
    if json {
        emit_json(&report)
    } else {
        print_pipeline_report(&report);
        Ok(())
    }
}

async fn benchmark_pipeline_flow(
    args: PipelineBenchmarkArgs,
    json: bool,
    mock_server: Option<&url::Url>,
    capture: Option<SessionCapture>,
    mut session: Option<&mut tui::Session>,
) -> Result<pipeline::PipelineReport> {
    let target = resolve_pipeline_target(&args, json, session.as_deref_mut()).await?;
    if mock_server.is_none() {
        authorize_garmin_contact(&args.contact_consent, json, true, session.as_deref_mut())?;
    }
    authorize_device_write(
        &args.write_consent,
        json,
        tui::PIPELINE_PROBE_WRITE_WARNING,
        session.as_deref_mut(),
    )?;
    if let Some(session) = session {
        let (reporter, receiver) = ProgressReporter::channel();
        let cancellation = reporter.cancellation_token();
        let reporter = capture.as_ref().map_or(reporter.clone(), |capture| {
            captured_progress(capture.clone(), &reporter)
        });
        let task = pipeline::run(target, args.map, reporter, mock_server.cloned(), capture);
        Box::pin(session.run_pipeline_progress(receiver, task, cancellation)).await
    } else {
        let downstream = ProgressReporter::default();
        let progress = capture.as_ref().map_or_else(
            || downstream.clone(),
            |capture| captured_progress(capture.clone(), &downstream),
        );
        pipeline::run(target, args.map, progress, mock_server.cloned(), capture).await
    }
}

fn print_pipeline_report(report: &pipeline::PipelineReport) {
    println!("Map: {}", report.map_name);
    println!("File: {}", report.destination.display());
    println!(
        "Download: {} at {} in {:.2}s (MD5 verified)",
        format_bytes(report.bytes),
        format_rate(report.download_bytes_per_second),
        report.download_seconds
    );
    println!(
        "Device upload: {} at {} in {:.2}s ({:?})",
        format_bytes(report.device_probe.bytes),
        format_rate(report.device_probe.upload_bytes_per_second()),
        report.device_probe.upload_seconds,
        report.device_probe.transport
    );
    println!(
        "Query {:.2}s; device verify {:.2}s; delete {:.2}s; total {:.2}s",
        report.query_seconds,
        report.device_probe.read_back_seconds,
        report.device_probe.cleanup_seconds,
        report.total_seconds
    );
    println!("Disposable host and device files removed.");
}

async fn resolve_pipeline_target(
    args: &PipelineBenchmarkArgs,
    json: bool,
    session: Option<&mut tui::Session>,
) -> Result<TargetArgs> {
    resolve_optional_target(&args.target, json, session).await
}

async fn resolve_optional_target(
    target: &OptionalTargetArgs,
    json: bool,
    session: Option<&mut tui::Session>,
) -> Result<TargetArgs> {
    if target.path.is_some() || target.mtp_location.is_some() {
        return Ok(TargetArgs {
            path: target.path.clone(),
            mtp_location: target.mtp_location,
            mounted_mtp: target.mounted_mtp.clone(),
        });
    }
    if target.mounted_mtp.is_some() {
        return Ok(TargetArgs {
            path: None,
            mtp_location: None,
            mounted_mtp: target.mounted_mtp.clone(),
        });
    }
    if !interactive_confirmation_available(json) {
        bail!(
            "automatic device selection requires an interactive terminal; pass a device selector"
        );
    }
    let session = session.context("interactive device selection has no terminal session")?;
    select_guided_target(session).await
}

async fn inspect_target(
    target: &TargetArgs,
    session: Option<&mut tui::Session>,
) -> Result<DeviceManifest> {
    match session {
        Some(session) => {
            let first_attempt = session.inspect_device(load_target(target)).await;
            let Err(initial_error) = first_attempt else {
                return first_attempt;
            };
            let Some(location) = target.mtp_location.filter(|_| {
                initial_error
                    .downcast_ref::<garmin_device::MtpError>()
                    .is_some_and(garmin_device::MtpError::needs_transport_reset)
            }) else {
                return Err(initial_error);
            };
            if mtp_usb_reset_known_ineffective(location) {
                return Err(initial_error).with_context(|| {
                    format!(
                        "Garmin MTP location {location} is unresponsive; its USB reset is known ineffective, so physically disconnect and reconnect the device"
                    )
                });
            }
            session
                .recover_device_link(async move {
                    reset_mtp_transport(location)
                        .await
                        .with_context(|| format!("could not reset Garmin MTP location {location}"))
                })
                .await?;
            session
                .inspect_device(load_target(target))
                .await
                .with_context(|| {
                    format!(
                        "Garmin MTP location {location} remained unresponsive after transport recovery"
                    )
                })
        }
        None => load_target(target).await,
    }
}

async fn inspect_link_benchmark_target(
    target: &TargetArgs,
    session: Option<&mut tui::Session>,
) -> Result<(DeviceManifest, Option<RawMtpSession>)> {
    let Some(location) = target
        .mtp_location
        .filter(|_| target.path.is_none() && target.mounted_mtp.is_none())
    else {
        return Ok((inspect_target(target, session).await?, None));
    };
    let open = || async move {
        let (raw_mtp_session, manifest) = RawMtpSession::open(location)
            .await
            .with_context(|| format!("cannot open Garmin MTP device at location {location}"))?;
        Ok((manifest, Some(raw_mtp_session)))
    };
    let Some(session) = session else {
        return open().await;
    };
    let first_attempt = session.inspect_device(open()).await;
    let Err(initial_error) = first_attempt else {
        return first_attempt;
    };
    let needs_reset = initial_error
        .downcast_ref::<garmin_device::MtpError>()
        .is_some_and(garmin_device::MtpError::needs_transport_reset);
    if !needs_reset {
        return Err(initial_error);
    }
    if mtp_usb_reset_known_ineffective(location) {
        return Err(initial_error).with_context(|| {
            format!(
                "Garmin MTP location {location} is unresponsive; its USB reset is known ineffective, so physically disconnect and reconnect the device"
            )
        });
    }
    session
        .recover_device_link(async move {
            reset_mtp_transport(location)
                .await
                .with_context(|| format!("could not reset Garmin MTP location {location}"))
        })
        .await?;
    session.inspect_device(open()).await.with_context(|| {
        format!("Garmin MTP location {location} remained unresponsive after transport recovery")
    })
}

fn target_for_device(device: &DeviceSummary) -> Result<TargetArgs> {
    match device.transport {
        TransportKind::MassStorage => Ok(TargetArgs {
            path: Some(PathBuf::from(&device.location)),
            mtp_location: None,
            mounted_mtp: None,
        }),
        TransportKind::Mtp => {
            let location = device
                .location
                .strip_prefix("mtp:0x")
                .context("discovered MTP device has no usable location")?;
            let location = u64::from_str_radix(location, 16)
                .context("discovered MTP device has an invalid location")?;
            Ok(TargetArgs {
                path: None,
                mtp_location: Some(location),
                mounted_mtp: None,
            })
        }
        TransportKind::MountedMtp => {
            let mount_id = device
                .location
                .strip_prefix("mounted-mtp:")
                .context("discovered mounted-MTP device has no usable location")?;
            Ok(TargetArgs {
                path: None,
                mtp_location: None,
                mounted_mtp: Some(mount_id.to_owned()),
            })
        }
    }
}

fn authorize_garmin_contact(
    consent: &GarminContactConsent,
    json: bool,
    may_download: bool,
    session: Option<&mut tui::Session>,
) -> Result<()> {
    let interactive = interactive_confirmation_available(json);
    if !should_prompt(
        consent.confirm_contact_garmin.as_ref(),
        interactive,
        "--confirm-contact-garmin CONTACT-GARMIN",
    )? {
        return Ok(());
    }
    let endpoint = garmin_map_service::omt_update_endpoint().to_string();
    let confirmed = match session {
        Some(session) => session.confirm_garmin_contact(endpoint, may_download),
        None => tui::confirm_garmin_contact(endpoint, may_download),
    }?;
    if confirmed {
        Ok(())
    } else {
        Err(tui::Cancelled::new("Garmin contact confirmation").into())
    }
}

fn authorize_device_write(
    consent: &DeviceWriteConsent,
    json: bool,
    message: &str,
    session: Option<&mut tui::Session>,
) -> Result<()> {
    authorize_device_write_details(
        consent,
        json,
        "Write to device?",
        tui::ConfirmationBody::message(message),
        "Write to device",
        session,
    )
}

fn authorize_device_write_details(
    consent: &DeviceWriteConsent,
    json: bool,
    title: &str,
    body: tui::ConfirmationBody,
    confirm_label: &str,
    session: Option<&mut tui::Session>,
) -> Result<()> {
    let interactive = interactive_confirmation_available(json);
    if !should_prompt(
        consent.confirm_device_write.as_ref(),
        interactive,
        "--confirm-device-write WRITE-DEVICE",
    )? {
        return Ok(());
    }
    let confirmed = match session {
        Some(session) => session.confirm_details(title, body, confirm_label),
        None => tui::confirm_details(title, body, confirm_label),
    }?;
    if confirmed {
        Ok(())
    } else {
        Err(tui::Cancelled::new("device write confirmation").into())
    }
}

fn device_identification(manifest: &DeviceManifest) -> String {
    let (identity, location) = device_identification_parts(manifest);
    format!("{identity} — {location}")
}

fn update_confirmation_device(manifest: &DeviceManifest) -> String {
    let (identity, location) = device_identification_parts(manifest);
    formatdoc! {"
        {identity}
        {location}"}
}

const fn transport_label(transport: TransportKind) -> &'static str {
    match transport {
        TransportKind::MassStorage => "mass storage",
        TransportKind::Mtp => "MTP",
        TransportKind::MountedMtp => "desktop-mounted MTP",
    }
}

fn device_identification_parts(manifest: &DeviceManifest) -> (String, String) {
    let summary = &manifest.summary;
    let part_number = summary
        .part_number
        .as_deref()
        .map(|part_number| format!(" ({part_number})"))
        .unwrap_or_default();
    let transport = transport_label(summary.transport);
    (
        format!("{}{part_number}", summary.model),
        format!("{transport} at {}", summary.location),
    )
}

fn file_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "file" } else { "files" })
}

fn should_prompt<T>(confirmation: Option<&T>, interactive: bool, fallback: &str) -> Result<bool> {
    match (confirmation.is_some(), interactive) {
        (true, _) => Ok(false),
        (false, true) => Ok(true),
        (false, false) => {
            bail!("interactive confirmation is unavailable; explicitly pass {fallback}")
        }
    }
}

fn interactive_confirmation_available(json: bool) -> bool {
    !json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

fn omt_client(service: MapService<'_>) -> Result<OmtClient> {
    let identity = ClientIdentity::default();
    match service {
        MapService::Garmin => OmtClient::anonymous(&identity).map_err(Into::into),
        MapService::Loopback(base) => {
            OmtClient::local_mock(&identity, base.clone()).map_err(Into::into)
        }
    }
}

async fn query_device_updates(
    manifest: &DeviceManifest,
    service: MapService<'_>,
    capture: Option<SessionCapture>,
) -> Result<(OmtClient, MapCatalog)> {
    if let Some(capture) = &capture {
        capture
            .write_bytes(
                Path::new("device/GarminDevice.xml"),
                manifest.raw_xml().as_bytes(),
            )
            .await?;
    }
    let client = omt_client(service)?.with_capture(capture);
    let response = client
        .check_maps(
            manifest.raw_xml(),
            manifest.capabilities().installed_map_files(),
        )
        .await?;
    Ok((client, response))
}

async fn query_device_updates_with_feedback(
    manifest: &DeviceManifest,
    service: MapService<'_>,
    capture: Option<SessionCapture>,
    session: Option<&mut tui::Session>,
) -> Result<(OmtClient, MapCatalog)> {
    let query = query_device_updates(manifest, service, capture);
    match session {
        Some(session) => session.load_available_components(query).await,
        None => query.await,
    }
}

fn build_update_plan(
    response: &MapCatalog,
    device_digest: String,
    selected: Vec<usize>,
    service: MapService<'_>,
) -> Result<UpdatePlan> {
    match service {
        MapService::Loopback(base) => Ok(UpdatePlan::from_mock_response_selection(
            response,
            device_digest,
            selected,
            base,
        )?),
        MapService::Garmin => Ok(UpdatePlan::from_response_selection(
            response,
            device_digest,
            selected,
        )?),
    }
}

fn build_selected_update_plan(
    response: &MapCatalog,
    device_digest: String,
    selected: Vec<usize>,
    service: MapService<'_>,
) -> Result<SelectedUpdatePlan> {
    let components = selected_map_actions(response, &selected)?;
    let plan = build_update_plan(response, device_digest, selected, service)?;
    Ok(SelectedUpdatePlan { plan, components })
}

fn selected_map_actions(
    response: &MapCatalog,
    selected: &[usize],
) -> Result<Vec<tui::SelectedMapAction>> {
    let choices = map_choices(response);
    selected
        .iter()
        .map(|index| {
            let choice = choices
                .get(*index)
                .with_context(|| format!("selected map index {index} is outside the catalog"))?;
            Ok(tui::SelectedMapAction::new(
                choice.name.clone(),
                choice.operation,
            ))
        })
        .collect()
}

fn select_selected_update_plan(
    response: &MapCatalog,
    manifest: &DeviceManifest,
    selection: &MapSelectionArgs,
    service: MapService<'_>,
    json: bool,
    session: Option<&mut tui::Session>,
) -> Result<SelectedUpdatePlan> {
    let selected = select_maps(response, selection, json, session)?;
    build_selected_update_plan(response, manifest.identity_digest(), selected, service)
}

fn select_update_plan(
    response: &MapCatalog,
    manifest: &DeviceManifest,
    selection: &MapSelectionArgs,
    service: MapService<'_>,
    json: bool,
    session: Option<&mut tui::Session>,
) -> Result<UpdatePlan> {
    Ok(select_selected_update_plan(response, manifest, selection, service, json, session)?.plan)
}

async fn select_update_plan_for_run(
    response: &MapCatalog,
    manifest: &DeviceManifest,
    selection: &MapSelectionArgs,
    service: MapService<'_>,
    json: bool,
    session: Option<&mut tui::Session>,
    runtime: InteractiveSelectionRuntime<'_>,
) -> Result<SelectedUpdatePlan> {
    let Some(session) = session else {
        return select_selected_update_plan(response, manifest, selection, service, json, None);
    };
    if selection.all_maps || !selection.maps.is_empty() {
        return select_selected_update_plan(
            response,
            manifest,
            selection,
            service,
            json,
            Some(session),
        );
    }
    let choices = map_choices_with_cache(response, manifest, service, runtime.cache).await?;
    let mut selector = tui::MapSelector::new(choices.len());
    let selected = loop {
        match session.select_maps_step(&choices, &mut selector, true)? {
            tui::MapSelection::Selected(selected) => break selected.changes,
            tui::MapSelection::Cancelled => {
                return Err(tui::Cancelled::new("map selection").into());
            }
            tui::MapSelection::Refresh => {
                match session.load_device_state(runtime.device.state()).await {
                    Ok(state) => session.set_device_state(state),
                    Err(error) if cancellation_step(&error).is_some() => return Err(error),
                    Err(error) => session.set_device_state_failure(error),
                }
            }
        }
    };
    build_selected_update_plan(response, manifest.identity_digest(), selected, service)
}

async fn select_guided_map_actions(
    response: &MapCatalog,
    manifest: &DeviceManifest,
    service: MapService<'_>,
    session: &mut tui::Session,
    runtime: InteractiveSelectionRuntime<'_>,
) -> Result<tui::SelectedMapChoices> {
    let choices = map_choices_with_cache(response, manifest, service, runtime.cache).await?;
    let mut selector = tui::MapSelector::new(choices.len());
    loop {
        match session.select_map_actions_step(&choices, &mut selector, true)? {
            tui::MapSelection::Selected(selected) => return Ok(selected),
            tui::MapSelection::Cancelled => {
                return Err(tui::Cancelled::new("map selection").into());
            }
            tui::MapSelection::Refresh => {
                match session.load_device_state(runtime.device.state()).await {
                    Ok(state) => session.set_device_state(state),
                    Err(error) if cancellation_step(&error).is_some() => return Err(error),
                    Err(error) => session.set_device_state_failure(error),
                }
            }
        }
    }
}

async fn map_choices_with_cache(
    response: &MapCatalog,
    manifest: &DeviceManifest,
    service: MapService<'_>,
    cache: &Path,
) -> Result<Vec<tui::MapChoice>> {
    let mut choices = map_choices(response);
    if choices.is_empty() {
        bail!("Garmin returned no map components for this device");
    }
    for (index, choice) in choices.iter_mut().enumerate() {
        let plan = build_update_plan(response, manifest.identity_digest(), vec![index], service)?;
        let cached = garmin_update::inspect_artifact_cache(&plan.downloads, cache).await?;
        choice.cache = (cached.total_files > 0).then_some(tui::MapCacheAvailability {
            cached_files: cached.cached_files,
            total_files: cached.total_files,
        });
    }
    Ok(choices)
}

struct InteractiveSelectionRuntime<'a> {
    device: &'a dyn UpdateTarget,
    cache: &'a Path,
}

async fn doctor(json: bool) -> Result<()> {
    let mounted = discover_mass_storage().await;
    let mtp = discover_mtp_candidates();
    let mounted_mtp = discover_mounted_mtp();
    let usb_sysfs = discover_garmin_usb_sysfs();
    let cache = cache_dir()?;
    let result = serde_json::json!({
        "mass_storage_devices": mounted.len(),
        "mounted_mtp": mounted_mtp,
        "mtp_candidates": mtp,
        "usb_sysfs": usb_sysfs,
        "cache": cache,
        "omt": "https://omt.garmin.com/",
        "mtp_access": "read-only manifest discovery and disposable throughput benchmark available",
    });
    if json {
        emit_json(&result)
    } else {
        println!("Garmin device diagnostics");
        println!();
        println!("Device access");
        println!("  Mass-storage manifests  {}", mounted.len());
        println!("  Desktop-mounted MTP     {}", mounted_mtp.len());
        println!(
            "  Openable MTP interfaces {}",
            result["mtp_candidates"].as_array().map_or(0, Vec::len)
        );
        println!("  Kernel-visible USB     {}", usb_sysfs.len());
        for device in &usb_sysfs {
            println!(
                "    {} ({:04x}:{:04x}) at {}",
                device.product.as_deref().unwrap_or("Garmin device"),
                device.vendor_id,
                device.product_id,
                device.sysfs_path.display()
            );
        }
        if !usb_sysfs.is_empty() && mtp.is_empty() && mounted_mtp.is_empty() && mounted.is_empty() {
            println!(
                "  Status                  USB device found, but no readable Garmin manifest is available"
            );
        }
        println!();
        println!("Services");
        println!("  Cache                   {}", cache.display());
        println!("  Anonymous map service   https://omt.garmin.com/");
        println!("  MTP capability          Manifest reads and disposable link benchmarks");
        Ok(())
    }
}

async fn load_manifest(path: &Path) -> Result<DeviceManifest> {
    open_mass_storage(path)
        .await
        .with_context(|| format!("cannot open Garmin device at {}", path.display()))
}

pub(crate) async fn load_target(target: &TargetArgs) -> Result<DeviceManifest> {
    if let Some(mount_id) = &target.mounted_mtp {
        if target.path.is_some() || target.mtp_location.is_some() {
            bail!("select exactly one device transport");
        }
        return MountedMtpDevice::new(mount_id)
            .open()
            .await
            .with_context(|| format!("cannot open mounted MTP device {mount_id}"));
    }
    match (&target.path, target.mtp_location) {
        (Some(path), None) => load_manifest(path).await,
        (None, Some(location)) => open_mtp(location)
            .await
            .with_context(|| format!("cannot open Garmin MTP device at location {location}")),
        _ => bail!("select exactly one of --path or --mtp-location"),
    }
}

async fn inventory_target(
    target: &TargetArgs,
    paths: &[SafeRelativePath],
) -> Result<DeviceInventory> {
    if let Some(mount_id) = &target.mounted_mtp {
        return MountedMtpDevice::new(mount_id)
            .inventory(paths)
            .await
            .with_context(|| format!("cannot inventory mounted MTP device {mount_id}"));
    }
    match (&target.path, target.mtp_location) {
        (Some(path), None) => inventory_mass_storage(path, paths)
            .await
            .with_context(|| format!("cannot inventory Garmin device at {}", path.display())),
        (None, Some(location)) => inventory_mtp(location, paths)
            .await
            .with_context(|| format!("cannot inventory Garmin MTP device at location {location}")),
        _ => bail!("select exactly one device transport"),
    }
}

pub(crate) fn cache_dir() -> Result<PathBuf> {
    CACHE_DIRECTORY
        .get()
        .cloned()
        .map_or_else(|| resolve_cache_dir(None), Ok)
}

static CACHE_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
const CACHE_DIRECTORY_ENV: &str = "GARMIN_TOOLKIT_CACHE_DIR";

fn configure_cache_dir(explicit: Option<&Path>) -> Result<()> {
    let directory = resolve_cache_dir(explicit)?;
    CACHE_DIRECTORY
        .set(directory)
        .map_err(|_| anyhow::anyhow!("application cache directory was configured more than once"))
}

fn resolve_cache_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(directory) = explicit {
        return Ok(directory.to_owned());
    }
    if let Some(directory) = std::env::var_os(CACHE_DIRECTORY_ENV) {
        if directory.is_empty() {
            bail!("{CACHE_DIRECTORY_ENV} cannot be empty");
        }
        return Ok(PathBuf::from(directory));
    }
    let root = dirs::cache_dir().context("no user cache directory is available")?;
    Ok(root.join("garmin-toolkit/maps"))
}

fn print_updates(response: &MapCatalog) {
    let maps = response.maps.iter().chain(&response.bundled_maps);
    let mut count = 0;
    for map in maps {
        count += 1;
        println!(
            "{}\t{}\t{}\t{} option(s)",
            map.display_name,
            map_version_transition(map),
            map.operation().label(),
            map.install_options.len()
        );
    }
    if count == 0 {
        println!("No map updates reported.");
    }
}

fn select_removal_components(
    response: &MapCatalog,
    selection: &RemovalSelectionArgs,
) -> Result<Vec<usize>> {
    let maps = response
        .maps
        .iter()
        .chain(&response.bundled_maps)
        .collect::<Vec<_>>();
    if selection.all_removable {
        let selected = maps
            .iter()
            .enumerate()
            .filter(|(_, map)| {
                map.installation_state.is_present()
                    && map.can_uninstall
                    && !map.files_to_remove.is_empty()
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            bail!("Garmin reported no installed removable components");
        }
        return Ok(selected);
    }

    let mut selected = Vec::with_capacity(selection.components.len());
    for requested in &selection.components {
        let matches = maps
            .iter()
            .enumerate()
            .filter(|(_, map)| map.display_name.eq_ignore_ascii_case(requested))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [index] => selected.push(*index),
            [] => bail!("Garmin reported no component named {requested:?}"),
            _ => bail!("multiple Garmin components are named {requested:?}"),
        }
    }
    selected.sort_unstable();
    selected.dedup();
    Ok(selected)
}

fn print_removal_plan(report: &RemovalPlanReport, json: bool) -> Result<()> {
    let plan = &report.service_plan;
    let execution = &report.execution_plan;
    if json {
        return emit_json(&serde_json::json!(report));
    }
    let palette = HumanPalette::stdout();
    println!(
        "{}  {}",
        palette.paint("Removal plan", AnsiColor::Cyan, true),
        palette.paint(&execution.digest, AnsiColor::Cyan, false)
    );
    println!();
    println!("{}", palette.paint("Components", AnsiColor::Cyan, true));
    for component in plan
        .components
        .iter()
        .filter(|component| component.disposition == garmin_update::ComponentDisposition::Remove)
    {
        println!(
            "  {}  {}",
            palette.paint("Remove", AnsiColor::Yellow, true),
            palette.paint(&component.name, AnsiColor::White, true)
        );
    }
    println!();
    println!("{}", palette.paint("Device files", AnsiColor::Cyan, true));
    for file in &execution.files_to_remove {
        println!(
            "  {}  {}  {}  {}",
            palette.paint("Remove", AnsiColor::Yellow, true),
            palette.paint(&file.storage_label, AnsiColor::DarkGrey, false),
            palette.paint(&file.path, AnsiColor::Cyan, false),
            palette.paint(format_bytes(file.size), AnsiColor::White, true)
        );
    }
    for path in &execution.files_already_absent {
        println!(
            "  {}  {}",
            palette.paint("Absent", AnsiColor::DarkGrey, false),
            palette.paint(path, AnsiColor::Cyan, false)
        );
    }
    for file in &plan.files_preserved {
        println!(
            "  {}  {}  {}",
            palette.paint("Keep", AnsiColor::Green, true),
            palette.paint(&file.path, AnsiColor::Cyan, false),
            palette.paint(
                format!("shared with {}", file.preserved_for.join(", ")),
                AnsiColor::DarkGrey,
                false
            )
        );
    }
    println!();
    println!(
        "{} {} from {}.",
        palette.paint("Would remove", AnsiColor::Yellow, true),
        palette.paint(
            format_bytes(execution.bytes_to_remove),
            AnsiColor::White,
            true
        ),
        palette.paint(
            file_count(execution.files_to_remove.len()),
            AnsiColor::White,
            true
        )
    );
    println!(
        "{} {}",
        palette.paint("Read-only plan.", AnsiColor::Green, true),
        palette.paint("The device was not changed.", AnsiColor::DarkGrey, false)
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct HumanPalette {
    enabled: bool,
}

impl HumanPalette {
    fn stdout() -> Self {
        Self {
            enabled: output_color_enabled(),
        }
    }

    fn paint(self, value: impl std::fmt::Display, color: AnsiColor, bold: bool) -> String {
        let value = value.to_string();
        if !self.enabled {
            return value;
        }
        let styled = value.with(color);
        if bold {
            styled.bold().to_string()
        } else {
            styled.to_string()
        }
    }
}

fn select_maps(
    response: &MapCatalog,
    selection: &MapSelectionArgs,
    json: bool,
    session: Option<&mut tui::Session>,
) -> Result<Vec<usize>> {
    let choices = map_choices(response);
    if selection.all_maps {
        return Ok((0..choices.len()).collect());
    }
    if !selection.maps.is_empty() {
        let mut selected = Vec::with_capacity(selection.maps.len());
        for requested in &selection.maps {
            let matches = choices
                .iter()
                .enumerate()
                .filter(|(_, choice)| choice.name.eq_ignore_ascii_case(requested))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [index] => selected.push(*index),
                [] => bail!("Garmin reported no map named {requested:?}"),
                _ => bail!("map name {requested:?} is ambiguous; use --all-maps or the selector"),
            }
        }
        selected.sort_unstable();
        selected.dedup();
        return Ok(selected);
    }
    if choices.is_empty() {
        bail!("Garmin returned no map components for this device");
    }
    if !interactive_confirmation_available(json) {
        bail!("select maps with --map <exact-name> or --all-maps");
    }
    let selection = match session {
        Some(session) => session.select_maps(&choices),
        None => tui::select_maps(&choices),
    }?;
    selection.ok_or_else(|| tui::Cancelled::new("map selection").into())
}

fn map_choices(response: &MapCatalog) -> Vec<tui::MapChoice> {
    response
        .maps
        .iter()
        .map(|map| (map, false))
        .chain(response.bundled_maps.iter().map(|map| (map, true)))
        .map(|(map, bundled)| {
            let preferred = map
                .install_options
                .iter()
                .find(|option| option.is_preferred)
                .or_else(|| map.install_options.first());
            let mut details = Vec::new();
            if let Some(region) = map
                .geographic_region
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                details.push(region.to_owned());
            }
            if let Some(map_type) = map.map_type.as_deref().filter(|value| !value.is_empty()) {
                details.push(map_type.to_owned());
            }
            if bundled {
                details.push("Bundled component".to_owned());
            }
            if let Some(option) = preferred {
                let bytes = option
                    .files
                    .iter()
                    .filter_map(|content| {
                        content
                            .downloads
                            .iter()
                            .find(|delivery| delivery.delivery_type.as_deref() == Some("Full"))
                            .or_else(|| content.downloads.first())
                    })
                    .map(|delivery| delivery.size_in_bytes)
                    .sum();
                details.push(format!(
                    "{} file{} · {}",
                    option.files.len(),
                    if option.files.len() == 1 { "" } else { "s" },
                    format_bytes(bytes)
                ));
                if !option.display_name.is_empty() && option.display_name != map.display_name {
                    details.push(option.display_name.clone());
                }
            }
            tui::MapChoice {
                name: map.display_name.clone(),
                version_transition: map_version_transition(map),
                version_tone: map_version_tone(map),
                description: if details.is_empty() {
                    "No additional description supplied by Garmin".to_owned()
                } else {
                    details.join(" · ")
                },
                operation: map.operation(),
                can_remove: map_removal_available(map),
                cache: None,
            }
        })
        .collect()
}

fn map_removal_available(map: &MapComponent) -> bool {
    map.installation_state.is_present() && map.can_uninstall && !map.files_to_remove.is_empty()
}

fn map_version_transition(map: &MapComponent) -> String {
    let installed = map
        .installed_release()
        .unwrap_or_else(|| "unknown".to_owned());
    let available = map
        .release
        .as_deref()
        .filter(|release| !release.is_empty())
        .unwrap_or("unknown");
    format!("({installed}) → {available}")
}

fn map_version_tone(map: &MapComponent) -> tui::MapVersionTone {
    match map.version_status() {
        MapVersionStatus::Outdated => tui::MapVersionTone::Outdated,
        MapVersionStatus::UpToDate => tui::MapVersionTone::UpToDate,
        MapVersionStatus::Unknown => tui::MapVersionTone::Unknown,
    }
}

fn print_plan(plan: &UpdatePlan, json: bool) -> Result<()> {
    if json {
        return emit_json(plan);
    }
    println!("Plan: {}", plan.digest);
    println!("Device: {}", plan.device_digest);
    println!(
        "Recovery backup: {}",
        match plan.backup_policy {
            BackupPolicy::Verified => "verified",
            BackupPolicy::Skip => "skipped",
        }
    );
    println!(
        "Download: {} in {} files",
        format_bytes(plan.total_bytes),
        plan.downloads.len()
    );
    for item in &plan.downloads {
        println!(
            "  {} -> {}",
            item.map_name,
            item.destination.as_path().display()
        );
    }
    println!(
        "Remove after replacement: {} files",
        plan.files_to_remove.len()
    );
    let backup_argument = match plan.backup_policy {
        BackupPolicy::Verified => "verified",
        BackupPolicy::Skip => "skip",
    };
    println!(
        "Apply interactively with: garmin-cli updates apply <device-selector> --backup {backup_argument}"
    );
    println!(
        "Automation must repeat the same --map selections (or --all-maps) and pass \
         --confirm-contact-garmin CONTACT-GARMIN --confirm-device-write WRITE-DEVICE \
         --backup {backup_argument} --confirm-plan {}",
        plan.digest,
    );
    Ok(())
}

fn emit_json(value: &impl Serialize) -> Result<()> {
    let choice = if output_color_enabled() {
        TerminalColorChoice::Always
    } else {
        TerminalColorChoice::Never
    };
    let output = StandardStream::stdout(choice);
    write_json(&mut output.lock(), value)
}

fn write_json(output: &mut impl WriteColor, value: &impl Serialize) -> Result<()> {
    termcolor_json::to_writer(&mut *output, value)?;
    writeln!(output)?;
    Ok(())
}

fn output_color_enabled() -> bool {
    color_enabled(
        OUTPUT_COLOR.get().copied().unwrap_or_default(),
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var_os("FORCE_COLOR").as_deref(),
    )
}

fn error_color_enabled() -> bool {
    color_enabled(
        OUTPUT_COLOR.get().copied().unwrap_or_default(),
        std::io::stderr().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var_os("FORCE_COLOR").as_deref(),
    )
}

fn color_enabled(
    choice: ColorChoice,
    terminal: bool,
    no_color: bool,
    force_color: Option<&std::ffi::OsStr>,
) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Off => false,
        ColorChoice::Auto => force_color.map_or(!no_color && terminal, |value| value != "0"),
    }
}

fn parse_byte_size(value: &str) -> std::result::Result<Byte, String> {
    Byte::parse_str(value, true).map_err(|error| error.to_string())
}

fn parse_concurrency(value: &str) -> std::result::Result<usize, String> {
    let value = value.parse::<usize>().map_err(|error| error.to_string())?;
    (1..=32)
        .contains(&value)
        .then_some(value)
        .ok_or_else(|| "concurrency must be between 1 and 32".to_owned())
}

fn format_bytes(bytes: u64) -> String {
    let bytes = Byte::from_u64(bytes);
    format!("{:.2}", bytes.get_appropriate_unit(UnitType::Decimal))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the non-negative rounded rate is saturated for display as a byte count"
)]
fn format_rate(bytes_per_second: f64) -> String {
    format!(
        "{}/s",
        format_bytes(bytes_per_second.max(0.0).round() as u64)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyArgs, BackupChoice, Cli, ColorChoice, Command, LanguageChoice, MapSelectionArgs,
        MockCommand, UpdateCommand, UpdatePlanArgs, color_enabled, device_identification,
        format_bytes, map_choices, mock_command, mounted_install_failure_presentation,
        parse_byte_size, resolve_cache_dir, resolve_language, select_maps, selected_map_actions,
        should_prompt, tui, write_json,
    };
    use clap::Parser;
    use garmin_device::{TransportKind, parse_manifest};
    use garmin_model::map::{MapCatalog, MapComponent, MapFile, MapOperation};

    #[tokio::test]
    async fn mock_test_exercises_the_production_update_workflow() {
        let temporary = tempfile::tempdir().unwrap();
        let capture =
            garmin_capture::SessionCapture::create(&temporary.path().join("capture")).unwrap();
        mock_command(MockCommand::Test, true, Some(capture))
            .await
            .unwrap();
    }

    #[test]
    fn backup_free_partial_failure_reports_mutation_and_recovery_work() {
        let mounted = garmin_update::MountedInstallError::UnprotectedMutation {
            operation: Box::new(garmin_update::MountedInstallError::RecoveryUnavailable),
            evidence: garmin_update::UnprotectedMutationEvidence::Applied {
                applied_operations: 12,
                total_operations: 27,
                failed_target: Some("Garmin/DB980010A.img".to_owned()),
            },
        };
        let error = anyhow::Error::msg("outer error");

        let presentation = mounted_install_failure_presentation(&mounted, &error, ".tmp/fenix-t03");
        let body = format!("{:?}", presentation.body);

        assert_eq!(presentation.title, "Update incomplete — device changed");
        assert!(body.contains("12 of 27 device operations"));
        assert!(body.contains("Garmin/DB980010A.img"));
        assert!(body.contains("14 device operations"));
        assert!(body.contains("run `device recover-update`"));
        assert!(!body.contains("left intact"));
    }

    #[test]
    fn backup_free_intent_only_failure_reports_uncertain_mutation() {
        let mounted = garmin_update::MountedInstallError::UnprotectedMutation {
            operation: Box::new(garmin_update::MountedInstallError::Cancelled),
            evidence: garmin_update::UnprotectedMutationEvidence::IntentRecorded {
                total_operations: 4,
                failed_target: Some("Garmin/map.img".to_owned()),
            },
        };
        let error = anyhow::Error::msg("outer error");

        let presentation = mounted_install_failure_presentation(&mounted, &error, ".tmp/capture");
        let body = format!("{:?}", presentation.body);

        assert_eq!(presentation.title, "Update incomplete — mutation uncertain");
        assert!(body.contains("0 of 4 device operations"));
        assert!(body.contains("device mutation became possible"));
        assert!(!body.contains("left intact"));
    }

    #[test]
    fn backup_free_failure_does_not_invent_counts_when_checkpoint_evidence_is_unreadable() {
        let mounted = garmin_update::MountedInstallError::UnprotectedMutation {
            operation: Box::new(garmin_update::MountedInstallError::RecoveryUnavailable),
            evidence: garmin_update::UnprotectedMutationEvidence::Unavailable {
                total_operations: 27,
                reason: "mounted update journal is invalid JSON".to_owned(),
            },
        };
        let error = anyhow::Error::msg("outer error");

        let presentation = mounted_install_failure_presentation(&mounted, &error, ".tmp/capture");
        let body = format!("{:?}", presentation.body);

        assert_eq!(
            presentation.title,
            "Update incomplete — evidence unreadable"
        );
        assert!(body.contains("27 device operations"));
        assert!(body.contains("journal is invalid JSON"));
        assert!(!body.contains("0 of 27"));
    }

    #[test]
    fn rejected_recovery_evidence_does_not_claim_device_restoration() {
        let mounted = garmin_update::MountedInstallError::JournalVersion(3);
        let error = anyhow::Error::msg("mounted update journal version 3 is unsupported");

        let presentation = mounted_install_failure_presentation(&mounted, &error, ".tmp/fenix-t03");
        let body = format!("{:?}", presentation.body);

        assert_eq!(presentation.title, "Recovery evidence rejected");
        assert!(body.contains("performed no device operation"));
        assert!(body.contains("interrupted state remains unresolved"));
        assert!(!body.contains("left intact"));
        assert!(!body.contains("restored"));
    }

    #[test]
    fn parses_sizes_with_library_units() {
        assert_eq!(parse_byte_size("256MB").unwrap().as_u64(), 256_000_000);
        assert_eq!(parse_byte_size("1GiB").unwrap().as_u64(), 1_073_741_824);
    }

    #[test]
    fn explicit_language_resolves_without_system_detection() {
        assert_eq!(
            resolve_language(LanguageChoice::English),
            garmin_i18n::Language::English
        );
        assert_eq!(
            resolve_language(LanguageChoice::Czech),
            garmin_i18n::Language::Czech
        );
    }

    #[test]
    fn language_is_a_global_root_option() {
        let parsed =
            Cli::try_parse_from(["garmin-cli", "device", "list", "--language", "cs"]).unwrap();

        assert_eq!(parsed.language, LanguageChoice::Czech);
    }

    #[test]
    fn link_benchmark_can_select_a_device_interactively() {
        let parsed = Cli::try_parse_from([
            "garmin-cli",
            "--cache-dir",
            "/tmp/garmin-cache",
            "benchmark",
            "link",
        ])
        .unwrap();

        assert_eq!(
            resolve_cache_dir(parsed.cache_dir.as_deref()).unwrap(),
            std::path::Path::new("/tmp/garmin-cache")
        );
    }

    #[test]
    fn color_policy_honors_flags_and_environment() {
        let forced = Some(std::ffi::OsStr::new("1"));
        let disabled = Some(std::ffi::OsStr::new("0"));

        assert!(color_enabled(ColorChoice::Always, false, true, disabled));
        assert!(!color_enabled(ColorChoice::Off, true, false, forced));
        assert!(color_enabled(ColorChoice::Auto, false, true, forced));
        assert!(!color_enabled(ColorChoice::Auto, true, false, disabled));
        assert!(!color_enabled(ColorChoice::Auto, true, true, None));
        assert!(color_enabled(ColorChoice::Auto, true, false, None));
    }

    #[test]
    fn json_formatter_supports_plain_and_colored_pretty_output() {
        let value = serde_json::json!({
            "enabled": true,
            "mount": "line\nbreak",
            "storages": [1, null],
        });
        let mut plain = termcolor::Buffer::no_color();
        let mut colored = termcolor::Buffer::ansi();
        write_json(&mut plain, &value).unwrap();
        write_json(&mut colored, &value).unwrap();
        let plain = String::from_utf8(plain.into_inner()).unwrap();
        let colored = String::from_utf8(colored.into_inner()).unwrap();

        assert_eq!(
            plain,
            format!("{}\n", serde_json::to_string_pretty(&value).unwrap())
        );
        assert!(colored.contains("\u{1b}["));
        assert!(colored.contains("\n  "));
    }

    #[test]
    fn human_readable_output_uses_appropriate_decimal_unit() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500), "1.50 KB");
        assert_eq!(format_bytes(1_500_000_000), "1.50 GB");
    }

    #[test]
    fn mock_device_requires_a_server_and_capture() {
        let missing_requirements =
            Cli::try_parse_from(["garmin-cli", "--mock-device", "/tmp/mock-device"]);
        assert!(missing_requirements.is_err());

        let interactive = Cli::try_parse_from([
            "garmin-cli",
            "--mock-server",
            "http://127.0.0.1:39765/",
            "--capture",
            "/tmp/mock-capture",
            "--mock-device",
            "/tmp/mock-device",
        ])
        .unwrap();
        assert!(interactive.command.is_none());
    }

    #[test]
    fn guided_dry_run_requires_a_capture_and_rejects_json() {
        assert!(Cli::try_parse_from(["garmin-cli", "--dry"]).is_err());
        assert!(
            Cli::try_parse_from([
                "garmin-cli",
                "--dry",
                "--capture",
                "/safe/new/session",
                "--json",
            ])
            .is_err()
        );
        let dry =
            Cli::try_parse_from(["garmin-cli", "--dry", "--capture", "/safe/new/session"]).unwrap();
        assert!(dry.dry);
        assert!(dry.command.is_none());
    }

    #[test]
    fn update_confirmation_identifies_the_resolved_device() {
        let manifest = parse_manifest(
            indoc::indoc! {r#"
                <Device xmlns="http://www.garmin.com/xmlschemas/GarminDevice/v2">
                  <Model>
                    <PartNumber>006-TEST-02</PartNumber>
                    <SoftwareVersion>9902</SoftwareVersion>
                    <Description>Example Cycling Computer</Description>
                  </Model>
                  <Id>42</Id>
                  <MassStorageMode />
                </Device>
            "#},
            TransportKind::MountedMtp,
            "synthetic-mount".to_owned(),
        )
        .unwrap();

        assert_eq!(
            device_identification(&manifest),
            "Example Cycling Computer (006-TEST-02) — desktop-mounted MTP at synthetic-mount"
        );
    }

    #[test]
    fn outbound_commands_require_exact_contact_confirmation() {
        let missing = Cli::try_parse_from(["garmin-cli", "updates", "check", "--path", "/device"]);
        assert!(missing.is_ok());

        let misspelled = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "check",
            "--path",
            "/device",
            "--confirm-contact-garmin",
            "contact-garmin",
        ]);
        assert!(misspelled.is_err());

        let confirmed = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "check",
            "--path",
            "/device",
            "--confirm-contact-garmin",
            "CONTACT-GARMIN",
        ]);
        assert!(confirmed.is_ok());
        assert!(should_prompt::<()>(None, true, "fallback").unwrap());
        assert!(should_prompt::<()>(None, false, "fallback").is_err());
        assert!(!should_prompt(Some(&()), false, "fallback").unwrap());
    }

    #[test]
    fn pipeline_requires_independent_contact_and_write_confirmations() {
        let automatic =
            Cli::try_parse_from(["garmin-cli", "benchmark", "pipeline", "--map", "Base maps"]);
        assert!(automatic.is_ok());

        let contact_only = Cli::try_parse_from([
            "garmin-cli",
            "benchmark",
            "pipeline",
            "--path",
            "/device",
            "--confirm-contact-garmin",
            "CONTACT-GARMIN",
        ]);
        assert!(contact_only.is_ok());

        let write_only = Cli::try_parse_from([
            "garmin-cli",
            "benchmark",
            "pipeline",
            "--path",
            "/device",
            "--confirm-device-write",
            "WRITE-DEVICE",
        ]);
        assert!(write_only.is_ok());

        let both = Cli::try_parse_from([
            "garmin-cli",
            "benchmark",
            "pipeline",
            "--path",
            "/device",
            "--confirm-contact-garmin",
            "CONTACT-GARMIN",
            "--confirm-device-write",
            "WRITE-DEVICE",
        ]);
        assert!(both.is_ok());
    }

    #[test]
    fn every_device_write_requires_write_confirmation() {
        let link_without_confirmation =
            Cli::try_parse_from(["garmin-cli", "benchmark", "link", "--path", "/device"]);
        assert!(link_without_confirmation.is_ok());

        let link_misspelled = Cli::try_parse_from([
            "garmin-cli",
            "benchmark",
            "link",
            "--path",
            "/device",
            "--confirm-device-write",
            "write-device",
        ]);
        assert!(link_misspelled.is_err());

        let link_confirmed = Cli::try_parse_from([
            "garmin-cli",
            "benchmark",
            "link",
            "--path",
            "/device",
            "--confirm-device-write",
            "WRITE-DEVICE",
        ]);
        assert!(link_confirmed.is_ok());

        let apply_confirmed = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "apply",
            "--path",
            "/device",
            "--confirm-contact-garmin",
            "CONTACT-GARMIN",
            "--confirm-device-write",
            "WRITE-DEVICE",
            "--confirm-plan",
            "plan-digest",
            "--all-maps",
        ]);
        assert!(apply_confirmed.is_ok());

        let removal_without_plan = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "remove",
            "--remove",
            "Example Map",
            "--confirm-device-write",
            "WRITE-DEVICE",
        ]);
        assert!(removal_without_plan.is_err());

        let removal_confirmed = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "remove",
            "--remove",
            "Example Map",
            "--confirm-plan",
            "plan-digest",
            "--confirm-device-write",
            "WRITE-DEVICE",
        ]);
        assert!(removal_confirmed.is_ok());

        let recovery_misspelled = Cli::try_parse_from([
            "garmin-cli",
            "device",
            "recover-removal",
            "--transaction",
            "/capture",
            "--confirm-device-write",
            "write-device",
        ]);
        assert!(recovery_misspelled.is_err());

        let recovery_confirmed = Cli::try_parse_from([
            "garmin-cli",
            "device",
            "recover-removal",
            "--transaction",
            "/capture",
            "--confirm-device-write",
            "WRITE-DEVICE",
        ]);
        assert!(recovery_confirmed.is_ok());
    }

    #[test]
    fn update_plan_and_apply_accept_an_explicit_backup_policy() {
        let plan = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "plan",
            "--path",
            "/device",
            "--all-maps",
            "--backup",
            "skip",
        ])
        .unwrap();
        let apply = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "apply",
            "--path",
            "/device",
            "--all-maps",
            "--backup",
            "skip",
        ])
        .unwrap();

        assert!(matches!(
            plan.command,
            Some(Command::Updates {
                command: UpdateCommand::Plan(UpdatePlanArgs {
                    backup: BackupChoice::Skip,
                    ..
                })
            })
        ));
        assert!(matches!(
            apply.command,
            Some(Command::Updates {
                command: UpdateCommand::Apply(ApplyArgs {
                    backup: BackupChoice::Skip,
                    ..
                })
            })
        ));
    }

    #[test]
    fn mounted_update_recovery_requires_exact_write_confirmation() {
        let misspelled = Cli::try_parse_from([
            "garmin-cli",
            "device",
            "recover-update",
            "--transaction",
            "/capture",
            "--confirm-device-write",
            "write-device",
        ]);
        assert!(misspelled.is_err());

        let confirmed = Cli::try_parse_from([
            "garmin-cli",
            "device",
            "recover-update",
            "--transaction",
            "/capture",
            "--confirm-device-write",
            "WRITE-DEVICE",
        ]);
        assert!(confirmed.is_ok());
    }

    #[test]
    fn map_choices_describe_reinstall_update_and_install_state() {
        let response = MapCatalog {
            maps: vec![
                MapComponent {
                    display_name: "Current base map".to_owned(),
                    release: Some("9.00".to_owned()),
                    installed_version: Some(garmin_model::map::MapVersion::new(9, 0)),
                    geographic_region: Some("Europe".to_owned()),
                    is_reinstall: true,
                    installation_state: garmin_model::map::InstallationState::Installed,
                    ..Default::default()
                },
                MapComponent {
                    display_name: "Outdated Europe map".to_owned(),
                    release: Some("2026.20".to_owned()),
                    installed_version: Some(garmin_model::map::MapVersion::new(25, 10)),
                    installation_state: garmin_model::map::InstallationState::Installed,
                    ..Default::default()
                },
                MapComponent {
                    display_name: "Optional Africa map".to_owned(),
                    is_reinstall: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let choices = map_choices(&response);
        assert_eq!(choices[0].operation, MapOperation::Reinstall);
        assert_eq!(choices[0].version_transition, "(9.00) → 9.00");
        assert_eq!(choices[0].version_tone, tui::MapVersionTone::UpToDate);
        assert!(choices[0].description.contains("Europe"));
        assert_eq!(choices[1].operation, MapOperation::Update);
        assert_eq!(choices[1].version_transition, "(2025.10) → 2026.20");
        assert_eq!(choices[1].version_tone, tui::MapVersionTone::Outdated);
        assert_eq!(choices[2].operation, MapOperation::Install);
        assert_eq!(choices[2].version_transition, "(unknown) → unknown");
        assert_eq!(choices[2].version_tone, tui::MapVersionTone::Unknown);
    }

    #[test]
    fn map_choices_only_enable_removal_when_garmin_supplies_a_safe_candidate() {
        let installed_file = || MapFile {
            file_name: "Garmin/map.img".to_owned(),
            part_number: String::new(),
            size_in_bytes: 42_000_000,
            is_shared: false,
        };
        let response = MapCatalog {
            maps: vec![
                MapComponent {
                    display_name: "CourseView".to_owned(),
                    installed_version: Some(garmin_model::map::MapVersion::new(26, 20)),
                    installation_state: garmin_model::map::InstallationState::Installed,
                    can_uninstall: false,
                    files_to_remove: vec![installed_file()],
                    ..Default::default()
                },
                MapComponent {
                    display_name: "Ski Map".to_owned(),
                    installed_version: Some(garmin_model::map::MapVersion::new(26, 10)),
                    installation_state: garmin_model::map::InstallationState::Installed,
                    can_uninstall: true,
                    files_to_remove: vec![installed_file()],
                    ..Default::default()
                },
                MapComponent {
                    display_name: "Not installed".to_owned(),
                    can_uninstall: true,
                    files_to_remove: vec![installed_file()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let choices = map_choices(&response);

        assert_eq!(choices.len(), 3);
        assert!(!choices[0].can_remove);
        assert!(choices[1].can_remove);
        assert!(!choices[2].can_remove);
    }

    #[test]
    fn selected_map_actions_retain_names_and_operations() {
        let response = MapCatalog {
            maps: vec![
                MapComponent {
                    display_name: "Base maps".to_owned(),
                    is_reinstall: true,
                    installation_state: garmin_model::map::InstallationState::Installed,
                    ..Default::default()
                },
                MapComponent {
                    display_name: "TopoActive Central Europe".to_owned(),
                    installation_state: garmin_model::map::InstallationState::Installed,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let actions = selected_map_actions(&response, &[0, 1]).unwrap();

        assert_eq!(actions[0].name, "Base maps");
        assert_eq!(actions[0].operation, MapOperation::Reinstall);
        assert_eq!(actions[1].name, "TopoActive Central Europe");
        assert_eq!(actions[1].operation, MapOperation::Update);
    }

    #[test]
    fn repeated_map_arguments_select_multiple_components() {
        let response = MapCatalog {
            maps: vec![
                MapComponent {
                    display_name: "Europe".to_owned(),
                    ..Default::default()
                },
                MapComponent {
                    display_name: "Africa".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let selection = MapSelectionArgs {
            maps: vec!["africa".to_owned(), "Europe".to_owned()],
            all_maps: false,
        };

        assert_eq!(
            select_maps(&response, &selection, true, None).unwrap(),
            [0, 1]
        );
    }

    #[test]
    fn map_and_all_maps_are_mutually_exclusive() {
        let parsed = Cli::try_parse_from([
            "garmin-cli",
            "updates",
            "plan",
            "--path",
            "/device",
            "--map",
            "Europe",
            "--all-maps",
        ]);
        assert!(parsed.is_err());
    }
}
