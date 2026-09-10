use anyhow::Result;
use byte_unit::{Byte, UnitType};
use crossterm::event;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use garmin_device::{DeviceSummary, TransportKind};
use garmin_progress::{
    CancellationToken, DeviceStateUpdate, OperationStage, ProgressReceiver, ProgressState,
    ProgressUnit,
};
use indoc::{formatdoc, indoc};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Padding, Paragraph, Row, Table,
    TableState, Wrap,
};
use ratatui::{CompletedFrame, Frame, Terminal};
use ratatui_interact::components::{
    CheckBox, CheckBoxState, CheckBoxStyle, DialogConfig, DialogFocusTarget, DialogState,
    PopupDialog,
};
use ratatui_interact::traits::{ContainerAction, EventResult};
use std::future::Future;
use std::io;
use std::io::stdout;
use std::time::{Duration, Instant, SystemTime};

mod device_state;
#[cfg(feature = "gallery")]
pub mod preview;
mod progress_dashboard;
mod progress_state;
mod system;

use progress_dashboard::{DashboardScroll, DashboardViewport, render_progress_dashboard};
use progress_state::{OperationView, ProgressModel, StageView};

use system::close_signal;

type CrosstermTerminal = Terminal<CrosstermBackend<io::Stdout>>;
const DEVICE_RESCAN_INTERVAL: Duration = Duration::from_secs(2);
const STALE_BYTE_PROGRESS_AFTER: Duration = Duration::from_secs(10);
const MIN_RATE_SAMPLE_DURATION: Duration = Duration::from_millis(250);
pub const LOADING_SPINNER_INTERVAL: Duration = Duration::from_millis(80);
const LOADING_SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const PIPELINE_PROBE_WRITE_WARNING: &str = indoc! {"
    A verified download is uploaded under a temporary name and checked on the device.
    The temporary copy is then deleted; existing Garmin files remain untouched."};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceEnvironment {
    Garmin,
    Mock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceChanges {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunProfile {
    pub service: ServiceEnvironment,
    pub device_changes: DeviceChanges,
}

impl RunProfile {
    pub const PRODUCTION: Self = Self {
        service: ServiceEnvironment::Garmin,
        device_changes: DeviceChanges::Enabled,
    };

    pub const REAL_DRY_RUN: Self = Self {
        service: ServiceEnvironment::Garmin,
        device_changes: DeviceChanges::Disabled,
    };

    pub const MOCK: Self = Self {
        service: ServiceEnvironment::Mock,
        device_changes: DeviceChanges::Enabled,
    };
}

struct AppTerminal {
    inner: CrosstermTerminal,
    profile: RunProfile,
}

impl AppTerminal {
    fn draw<F>(&mut self, render: F) -> io::Result<CompletedFrame<'_>>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        let profile = self.profile;
        self.inner.draw(|frame| {
            render(frame);
            draw_profile_banner(frame, profile);
        })
    }

    fn size(&self) -> io::Result<ratatui::layout::Size> {
        self.inner.size()
    }

    fn content_area(&self) -> io::Result<Rect> {
        Ok(profile_content_area(Rect::from(self.size()?), self.profile))
    }
}

pub const UPDATE_PLAN_INTRODUCTION: &str = indoc! {"
    Review this exact plan.
    Its ID fingerprints the device and changes."};
const UPDATE_PROGRESS_TITLE: &str = "garmin-cli — Updating maps";
const UPDATE_COMPLETE: &str = "Transaction complete. Evidence retained; close when ready.";
const COMPLETION_INSTRUCTIONS: &str = indoc! {"
    Transaction committed; recovery files removed.
    Eject or unmount before unplugging."};
const UPDATE_STAGES: &[OperationStage] = &[
    OperationStage::Backup,
    OperationStage::Download,
    OperationStage::Verify,
    OperationStage::Authorize,
    OperationStage::Stage,
    OperationStage::Commit,
    OperationStage::DeviceFinalize,
    OperationStage::DeviceVerify,
    OperationStage::Cleanup,
];
const PIPELINE_PROBE_STAGES: &[OperationStage] = &[
    OperationStage::Inspect,
    OperationStage::Query,
    OperationStage::Plan,
    OperationStage::Download,
    OperationStage::Verify,
    OperationStage::Upload,
    OperationStage::DeviceFinalize,
    OperationStage::DeviceVerify,
    OperationStage::Delete,
];
const PIPELINE_PROBE_COMPLETE: &str =
    "Pipeline probe complete. Download verified; temporary files removed.";
const LINK_BENCHMARK_STAGES: &[OperationStage] = &[
    OperationStage::Upload,
    OperationStage::DeviceFinalize,
    OperationStage::DeviceVerify,
    OperationStage::Delete,
];
const LINK_BENCHMARK_COMPLETE: &str = "Link benchmark complete. Disposable device file removed.";
const REMOVAL_STAGES: &[OperationStage] = &[
    OperationStage::Backup,
    OperationStage::Commit,
    OperationStage::Cleanup,
];
const UPDATE_RECOVERY_STAGES: &[OperationStage] = &[
    OperationStage::Verify,
    OperationStage::Commit,
    OperationStage::DeviceFinalize,
    OperationStage::DeviceVerify,
    OperationStage::Cleanup,
];
const UPDATE_RECOVERY_COMPLETE: &str = "Update recovered. Device state reconciled.";
const REMOVAL_COMPLETE: &str =
    "Removal complete. Files removed; verified backups retained in the capture.";
const REMOVAL_RECOVERY_STAGES: &[OperationStage] = &[OperationStage::Cleanup];
const REMOVAL_RECOVERY_COMPLETE: &str = "Recovery complete. Every target path and size is present.";

pub struct Session {
    terminal: AppTerminal,
    device_state: Option<DeviceStateUpdate>,
}

#[derive(Debug)]
pub struct Cancelled {
    step: &'static str,
}

impl Cancelled {
    #[must_use]
    pub const fn new(step: &'static str) -> Self {
        Self { step }
    }

    #[must_use]
    pub const fn step(&self) -> &'static str {
        self.step
    }
}

impl std::fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} cancelled", self.step)
    }
}

impl std::error::Error for Cancelled {}

impl Session {
    /// Opens an interactive terminal session with mouse input enabled.
    /// # Errors
    /// Terminal setup failure.
    pub fn open() -> Result<Self> {
        Self::open_with_profile(RunProfile::PRODUCTION)
    }

    /// Opens a terminal session and displays its runtime profile.
    /// # Errors
    /// Terminal setup failure.
    pub fn open_with_profile(profile: RunProfile) -> Result<Self> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let inner = match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut output = stdout();
                let _ = execute!(output, LeaveAlternateScreen, DisableMouseCapture);
                let _ = disable_raw_mode();
                return Err(error.into());
            }
        };
        Ok(Self {
            terminal: AppTerminal { inner, profile },
            device_state: None,
        })
    }

    fn terminal(&mut self) -> &mut AppTerminal {
        &mut self.terminal
    }

    pub fn set_device_state(&mut self, state: garmin_device::DeviceStateSnapshot) {
        self.device_state = Some(Ok(state));
    }

    pub fn set_device_state_failure(&mut self, error: impl std::fmt::Display) {
        self.device_state = Some(Err(error.to_string()));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.inner.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.inner.show_cursor();
    }
}

impl Session {
    /// Asks the user to choose one of the discovered devices.
    /// # Errors
    /// Rendering or input failure.
    pub fn select_device(&mut self, devices: &[DeviceSummary]) -> Result<Option<usize>> {
        let rows = update_device_rows(devices);
        match device_selection_loop(
            self.terminal(),
            &rows,
            "Select an update device",
            !devices.is_empty(),
            0,
            None,
        )? {
            DeviceSelection::Selected(selected) => Ok(Some(selected)),
            DeviceSelection::Cancelled => Ok(None),
            DeviceSelection::Rescan(_) => unreachable!("static device picker requested a rescan"),
        }
    }

    /// Chooses a device from a periodically refreshed snapshot.
    /// # Errors
    /// Rendering or input failure.
    pub fn select_device_refreshable(
        &mut self,
        devices: &[DeviceSummary],
        selected: usize,
    ) -> Result<DeviceSelection> {
        let rows = update_device_rows(devices);
        device_selection_loop(
            self.terminal(),
            &rows,
            "Select an update device",
            !devices.is_empty(),
            selected,
            Some(DEVICE_RESCAN_INTERVAL),
        )
    }
}

fn update_device_rows(devices: &[DeviceSummary]) -> Vec<String> {
    let mut rows = devices
        .iter()
        .map(|device| {
            let part_number = device
                .part_number
                .as_deref()
                .map(|part_number| format!(" ({part_number})"))
                .unwrap_or_default();
            let transport = match device.transport {
                TransportKind::MassStorage => "mass storage",
                TransportKind::Mtp => "MTP",
                TransportKind::MountedMtp => "desktop-mounted MTP",
            };
            format!(
                "{}{part_number}  [{transport}]  {}",
                device.model, device.location
            )
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        rows.push("No Garmin device is currently visible".to_owned());
    }
    rows
}

/// Displays a confirmation prompt in a temporary terminal session.
/// # Errors
/// Terminal setup, rendering, or input failure.
pub fn confirm(title: &str, message: &str, confirm_label: &str) -> Result<bool> {
    confirm_body(title, ConfirmationBody::message(message), confirm_label)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationValueTone {
    Neutral,
    Muted,
    Warning,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationField {
    pub label: String,
    pub value: String,
    pub tone: ConfirmationValueTone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedMapAction {
    pub name: String,
    pub action: String,
}

impl SelectedMapAction {
    pub fn new(name: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            action: action.into(),
        }
    }
}

impl ConfirmationField {
    pub fn new(
        label: impl Into<String>,
        value: impl Into<String>,
        tone: ConfirmationValueTone,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationBody {
    pub introduction: Text<'static>,
    pub fields: Vec<ConfirmationField>,
    pub note: Option<Text<'static>>,
    pub confirm_action: String,
}

#[derive(Debug, Clone)]
pub struct UpdateConfirmationBody {
    details: ConfirmationBody,
    backup: CheckBoxState,
    backup_area: Option<Rect>,
    verified_plan_id: Option<String>,
    skipped_plan_id: Option<String>,
}

impl UpdateConfirmationBody {
    #[must_use]
    pub fn with_backup_enabled(mut self, enabled: bool) -> Self {
        self.backup.set_checked(enabled);
        self
    }

    #[must_use]
    pub fn with_backup_plan_ids(
        mut self,
        verified: impl Into<String>,
        skipped: impl Into<String>,
    ) -> Self {
        self.verified_plan_id = Some(verified.into());
        self.skipped_plan_id = Some(skipped.into());
        self.sync_plan_id();
        self
    }

    fn backup_enabled(&self) -> bool {
        self.backup.checked
    }

    fn confirm_label(&self) -> &'static str {
        if self.backup_enabled() {
            "Continue"
        } else {
            "Continue without backup"
        }
    }

    fn sync_plan_id(&mut self) {
        let plan_id = if self.backup_enabled() {
            self.verified_plan_id.as_ref()
        } else {
            self.skipped_plan_id.as_ref()
        };
        if let Some(plan_id) = plan_id
            && let Some(field) = self
                .details
                .fields
                .iter_mut()
                .find(|field| field.label == "Plan ID")
        {
            field.value.clone_from(plan_id);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateConfirmationDecision {
    pub confirmed: bool,
    pub backup_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingRecoveryDecision {
    Recover,
    ClearState,
    Discard,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingRecoveryActions {
    RecoverOnly,
    RecoverOrClear,
    DiscardOnly,
}

impl PendingRecoveryActions {
    const fn allows_clear(self) -> bool {
        matches!(self, Self::RecoverOrClear)
    }

    const fn allows_recover(self) -> bool {
        !matches!(self, Self::DiscardOnly)
    }

    const fn allows_discard(self) -> bool {
        matches!(self, Self::DiscardOnly)
    }
}

impl ConfirmationBody {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            introduction: Text::raw(message.into()),
            fields: Vec::new(),
            note: None,
            confirm_action: "confirm".to_owned(),
        }
    }
}

#[must_use]
pub fn garmin_contact_confirmation_body(
    endpoint: impl Into<String>,
    may_download: bool,
) -> ConfirmationBody {
    let mut introduction = vec![
        Line::from(vec![
            Span::raw("Send "),
            Span::styled("GarminDevice.xml", path_style()),
            Span::raw(" to Garmin:"),
        ]),
        Line::from(Span::styled(endpoint.into(), link_style())),
        Line::raw(""),
        Line::raw("It may contain the device ID, hardware, software, and installed maps."),
    ];
    if may_download {
        introduction.push(Line::raw("Garmin download hosts may also be contacted."));
    }
    introduction.push(Line::raw("No account credentials are sent."));
    ConfirmationBody {
        introduction: Text::from(introduction),
        fields: Vec::new(),
        note: None,
        confirm_action: "confirm".to_owned(),
    }
}

#[must_use]
pub fn update_confirmation_body(
    device: impl Into<String>,
    components: &[SelectedMapAction],
    plan_id: impl Into<String>,
    files: impl Into<String>,
    transfer_size: impl Into<String>,
    removals: impl Into<String>,
    capture: impl Into<String>,
) -> UpdateConfirmationBody {
    UpdateConfirmationBody {
        details: ConfirmationBody {
            introduction: Text::raw(UPDATE_PLAN_INTRODUCTION),
            fields: vec![
                ConfirmationField::new("Device", device, ConfirmationValueTone::Neutral),
                ConfirmationField::new(
                    "Components",
                    selected_map_actions(components),
                    ConfirmationValueTone::Warning,
                ),
                ConfirmationField::new("Plan ID", plan_id, ConfirmationValueTone::Muted),
                ConfirmationField::new("Files", files, ConfirmationValueTone::Neutral),
                ConfirmationField::new(
                    "Transfer size",
                    transfer_size,
                    ConfirmationValueTone::Neutral,
                ),
                ConfirmationField::new("Remove", removals, ConfirmationValueTone::Warning),
                ConfirmationField::new("Capture", capture, ConfirmationValueTone::Path),
            ],
            note: Some(Text::from(Line::from(vec![
                Span::styled(
                    "Map authorization — ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("protected maps need device-bound authorization."),
            ]))),
            confirm_action: "continue".to_owned(),
        },
        backup: CheckBoxState::new(true),
        backup_area: None,
        verified_plan_id: None,
        skipped_plan_id: None,
    }
}

fn selected_map_actions(components: &[SelectedMapAction]) -> String {
    let action_width = components
        .iter()
        .map(|component| component.action.chars().count())
        .max()
        .unwrap_or_default();
    components
        .iter()
        .map(|component| format!("{:<action_width$}  {}", component.action, component.name))
        .collect::<Vec<_>>()
        .join("\n")
}

#[must_use]
pub fn removal_confirmation_body(
    device: impl Into<String>,
    plan_id: impl Into<String>,
    components: impl Into<String>,
    files: impl Into<String>,
    reclaimed: impl Into<String>,
    capture: impl Into<String>,
) -> ConfirmationBody {
    ConfirmationBody {
        introduction: Text::raw(indoc! {"
            Review this exact removal.
            Its ID fingerprints the device and targets."}),
        fields: vec![
            ConfirmationField::new("Device", device, ConfirmationValueTone::Neutral),
            ConfirmationField::new("Plan ID", plan_id, ConfirmationValueTone::Muted),
            ConfirmationField::new("Components", components, ConfirmationValueTone::Warning),
            ConfirmationField::new("Files", files, ConfirmationValueTone::Warning),
            ConfirmationField::new("Reclaim", reclaimed, ConfirmationValueTone::Neutral),
            ConfirmationField::new("Recovery", capture, ConfirmationValueTone::Path),
        ],
        note: Some(Text::from(vec![
            Line::from(Span::styled(
                "Guarded removal",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::raw("Every target is backed up and verified before deletion."),
            Line::raw("An interrupted transaction can be restored from this capture."),
        ])),
        confirm_action: "remove files".to_owned(),
    }
}

#[must_use]
pub fn verification_completion_message(
    bytes: impl std::fmt::Display,
    files: usize,
    capture: impl std::fmt::Display,
) -> String {
    let noun = if files == 1 { "file" } else { "files" };
    formatdoc! {"
        Verified {bytes} across {files} selected {noun}; captured Garmin's authorization response.

        Device unchanged. Evidence: {capture}"}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureValueTone {
    Neutral,
    Error,
    Path,
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureField {
    label: String,
    value: String,
    tone: FailureValueTone,
}

impl FailureField {
    #[must_use]
    pub fn new(label: impl Into<String>, value: impl Into<String>, tone: FailureValueTone) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureBody {
    summary: String,
    fields: Vec<FailureField>,
    outcome: String,
    diagnostics: Option<String>,
}

impl FailureBody {
    #[must_use]
    pub fn new(
        summary: impl Into<String>,
        fields: Vec<FailureField>,
        capture: impl Into<String>,
    ) -> Self {
        Self {
            summary: summary.into(),
            fields,
            outcome: "Update incomplete.".to_owned(),
            diagnostics: Some(capture.into()),
        }
    }

    #[must_use]
    pub fn with_outcome(mut self, outcome: impl Into<String>) -> Self {
        self.outcome = outcome.into();
        self
    }

    #[must_use]
    pub fn without_diagnostics(mut self) -> Self {
        self.diagnostics = None;
        self
    }
}

/// Displays a structured confirmation prompt in a temporary terminal session.
/// # Errors
/// Terminal setup, rendering, or input failure.
pub fn confirm_details(title: &str, body: ConfirmationBody, confirm_label: &str) -> Result<bool> {
    confirm_body(title, body, confirm_label)
}

fn confirm_body(title: &str, body: ConfirmationBody, confirm_label: &str) -> Result<bool> {
    let mut session = Session::open()?;
    session.confirm_details(title, body, confirm_label)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapChoice {
    pub name: String,
    pub version_transition: String,
    pub version_tone: MapVersionTone,
    pub description: String,
    pub install_label: &'static str,
    pub can_remove: bool,
    pub cache: Option<MapCacheAvailability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapCacheAvailability {
    pub cached_files: usize,
    pub total_files: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapVersionTone {
    Outdated,
    UpToDate,
    Unknown,
}

const MAP_ACTION_COLUMN_WIDTH: u16 = 12;
const MAP_TABLE_COLUMN_SPACING: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapActionMode {
    ChangeOnly,
    Combined,
}

impl MapActionMode {
    const fn allows_removal(self) -> bool {
        matches!(self, Self::Combined)
    }
}

pub struct MapSelector {
    actions: Vec<MapChoiceAction>,
    table: TableState,
}

impl MapSelector {
    #[must_use]
    pub fn new(choice_count: usize) -> Self {
        Self {
            actions: vec![MapChoiceAction::Keep; choice_count],
            table: TableState::default().with_selected(Some(0)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum MapChoiceAction {
    #[default]
    Keep,
    Change,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedMapChoices {
    pub changes: Vec<usize>,
    pub removals: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSelection {
    Selected(SelectedMapChoices),
    Cancelled,
    Refresh,
}

/// Ask which map components should be kept or installed.
///
/// Every row starts on `Keep`; the returned indexes are the rows explicitly
/// switched to their install action. `None` means that the user cancelled.
/// # Errors
/// Terminal setup, rendering, or input failure.
pub fn select_maps(choices: &[MapChoice]) -> Result<Option<Vec<usize>>> {
    let mut session = Session::open()?;
    let mut selector = MapSelector::new(choices.len());
    match session.select_maps_step(choices, &mut selector, false)? {
        MapSelection::Selected(selected) => Ok(Some(selected.changes)),
        MapSelection::Cancelled => Ok(None),
        MapSelection::Refresh => unreachable!("refresh is disabled"),
    }
}

impl Session {
    /// Displays a confirmation prompt within this terminal session.
    /// # Errors
    /// Rendering or input failure.
    pub fn confirm(&mut self, title: &str, message: &str, confirm_label: &str) -> Result<bool> {
        self.confirm_details(title, ConfirmationBody::message(message), confirm_label)
    }

    /// Displays a structured confirmation prompt within this terminal session.
    /// # Errors
    /// Rendering or input failure.
    pub fn confirm_details(
        &mut self,
        title: &str,
        body: ConfirmationBody,
        confirm_label: &str,
    ) -> Result<bool> {
        confirmation_loop(self.terminal(), title, body, confirm_label)
    }

    /// Choose how to resolve pending recovery.
    /// # Errors
    /// Rendering or input failure.
    pub fn pending_recovery(
        &mut self,
        body: ConfirmationBody,
        actions: PendingRecoveryActions,
    ) -> Result<PendingRecoveryDecision> {
        pending_recovery_loop(self.terminal(), body, actions)
    }

    /// Displays the update confirmation with its recovery-backup choice.
    /// # Errors
    /// Rendering or input failure.
    pub fn confirm_update(
        &mut self,
        title: &str,
        body: UpdateConfirmationBody,
    ) -> Result<UpdateConfirmationDecision> {
        update_confirmation_loop(self.terminal(), title, body)
    }

    /// Asks which map components should be kept or installed.
    /// # Errors
    /// Rendering or input failure.
    pub fn select_maps(&mut self, choices: &[MapChoice]) -> Result<Option<Vec<usize>>> {
        let mut selector = MapSelector::new(choices.len());
        match self.select_maps_step(choices, &mut selector, false)? {
            MapSelection::Selected(selected) => Ok(Some(selected.changes)),
            MapSelection::Cancelled => Ok(None),
            MapSelection::Refresh => unreachable!("refresh is disabled"),
        }
    }

    /// Selects maps while preserving choices across capacity refreshes.
    /// # Errors
    /// Rendering or input failure.
    pub fn select_maps_step(
        &mut self,
        choices: &[MapChoice],
        selector: &mut MapSelector,
        can_refresh: bool,
    ) -> Result<MapSelection> {
        let snapshot = self.device_state.clone();
        map_selection_loop(
            self.terminal(),
            choices,
            selector,
            snapshot.as_ref(),
            can_refresh,
            MapActionMode::ChangeOnly,
        )
    }

    /// Selects changes and guarded removals in one component table.
    /// # Errors
    /// Rendering or input failure.
    pub fn select_map_actions_step(
        &mut self,
        choices: &[MapChoice],
        selector: &mut MapSelector,
        can_refresh: bool,
    ) -> Result<MapSelection> {
        let snapshot = self.device_state.clone();
        map_selection_loop(
            self.terminal(),
            choices,
            selector,
            snapshot.as_ref(),
            can_refresh,
            MapActionMode::Combined,
        )
    }
}

fn map_selection_loop(
    terminal: &mut AppTerminal,
    choices: &[MapChoice],
    selector: &mut MapSelector,
    snapshot: Option<&DeviceStateUpdate>,
    can_refresh: bool,
    mode: MapActionMode,
) -> Result<MapSelection> {
    loop {
        let area = terminal.content_area()?;
        let mut table_area = area;
        terminal.draw(|frame| {
            let area = device_state::render_update(frame, area, snapshot);
            let areas = Layout::vertical([
                Constraint::Min(5),
                Constraint::Length(map_selection_footer_height(can_refresh)),
            ])
            .split(area);
            table_area = areas[0];
            draw_map_selection(
                frame,
                (table_area, areas[1]),
                choices,
                &selector.actions,
                &mut selector.table,
                can_refresh,
                mode,
            );
        })?;

        match event::read()? {
            Event::Key(key) if is_cancel_key(&key) => return Ok(MapSelection::Cancelled),
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up => select_table_previous(&mut selector.table),
                KeyCode::Down => select_table_next(&mut selector.table, choices.len()),
                KeyCode::Left => {
                    cycle_current_action(
                        &selector.table,
                        &mut selector.actions,
                        choices,
                        mode,
                        CycleDirection::Previous,
                    );
                }
                KeyCode::Right | KeyCode::Char(' ') => {
                    cycle_current_action(
                        &selector.table,
                        &mut selector.actions,
                        choices,
                        mode,
                        CycleDirection::Next,
                    );
                }
                KeyCode::Char('a' | 'A') => {
                    selector.actions.fill(MapChoiceAction::Change);
                }
                KeyCode::Char('k' | 'K') => selector.actions.fill(MapChoiceAction::Keep),
                KeyCode::Char('r' | 'R') if can_refresh => return Ok(MapSelection::Refresh),
                KeyCode::Enter
                    if selector
                        .actions
                        .iter()
                        .any(|action| *action != MapChoiceAction::Keep) =>
                {
                    return Ok(MapSelection::Selected(selected_map_choices(
                        &selector.actions,
                    )));
                }
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => select_table_previous(&mut selector.table),
                MouseEventKind::ScrollDown => {
                    select_table_next(&mut selector.table, choices.len());
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    select_map_cell(
                        table_area,
                        mouse.column,
                        mouse.row,
                        &mut selector.table,
                        &mut selector.actions,
                        choices,
                        mode,
                    );
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn draw_map_selection(
    frame: &mut ratatui::Frame<'_>,
    areas: (Rect, Rect),
    choices: &[MapChoice],
    actions: &[MapChoiceAction],
    state: &mut TableState,
    can_refresh: bool,
    mode: MapActionMode,
) {
    let (table_area, help_area) = areas;
    let header = if mode.allows_removal() {
        Row::new(["Map component", "Keep", "Change", "Remove"])
    } else {
        Row::new(["Map component", "Keep", "Change"])
    }
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1);
    let selected = state.selected();
    let rows = choices
        .iter()
        .zip(actions)
        .enumerate()
        .map(|(index, (choice, action))| {
            map_choice_row(choice, *action, selected == Some(index), mode)
        });
    let widths = if mode.allows_removal() {
        vec![
            Constraint::Min(32),
            Constraint::Length(MAP_ACTION_COLUMN_WIDTH),
            Constraint::Length(MAP_ACTION_COLUMN_WIDTH),
            Constraint::Length(MAP_ACTION_COLUMN_WIDTH),
        ]
    } else {
        vec![
            Constraint::Min(32),
            Constraint::Length(MAP_ACTION_COLUMN_WIDTH),
            Constraint::Length(MAP_ACTION_COLUMN_WIDTH),
        ]
    };
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(MAP_TABLE_COLUMN_SPACING)
        .block(
            Block::default()
                .title(" Garmin map components ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .row_highlight_style(selection_style())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, table_area, state);

    let selected = selected_map_choices(actions);
    let mut bulk_actions = vec![("A", "change all"), ("K", "keep all")];
    if can_refresh {
        bulk_actions.push(("R", "refresh space"));
    }
    let mut hints = vec![
        key_hints(&[("↑/↓", "row"), ("←/→", "action"), ("Mouse", "select")]),
        key_hints(&bulk_actions),
    ];
    if can_refresh {
        hints.push(key_hints(&[("Enter", "continue"), ("Esc", "cancel")]));
    } else {
        hints[1] = key_hints(&[
            ("A", "change all"),
            ("K", "keep all"),
            ("Enter", "continue"),
            ("Esc", "cancel"),
        ]);
    }
    hints.push(Line::from(Span::styled(
        selected_map_choice_count(&selected, mode),
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(
        Paragraph::new(Text::from(hints)).alignment(Alignment::Center),
        help_area,
    );
}

const fn map_selection_footer_height(can_refresh: bool) -> u16 {
    if can_refresh { 4 } else { 3 }
}

fn map_choice_row(
    choice: &MapChoice,
    action: MapChoiceAction,
    highlighted: bool,
    mode: MapActionMode,
) -> Row<'_> {
    let metadata_style = Style::default().fg(if highlighted {
        Color::Black
    } else {
        Color::Gray
    });
    let mut metadata = vec![Span::styled(
        choice.version_transition.as_str(),
        Style::default().fg(if highlighted {
            Color::Black
        } else {
            match choice.version_tone {
                MapVersionTone::Outdated => Color::Yellow,
                MapVersionTone::UpToDate => Color::Green,
                MapVersionTone::Unknown => Color::Gray,
            }
        }),
    )];
    if let Some(cache) = choice.cache {
        metadata.push(Span::styled(" · ", metadata_style));
        let complete = cache.cached_files == cache.total_files;
        let label = if complete {
            "cached".to_owned()
        } else {
            format!("cached {}/{}", cache.cached_files, cache.total_files)
        };
        metadata.push(Span::styled(
            label,
            Style::default().fg(if highlighted {
                Color::Black
            } else if complete {
                Color::Green
            } else {
                Color::Yellow
            }),
        ));
    }
    if !choice.description.is_empty() {
        metadata.push(Span::styled(" · ", metadata_style));
        metadata.push(Span::styled(choice.description.as_str(), metadata_style));
    }
    let details = Text::from(vec![
        Line::from(Span::styled(
            choice.name.as_str(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(metadata),
    ]);
    let keep = choice_marker(action == MapChoiceAction::Keep, "Keep");
    let change = choice_marker(action == MapChoiceAction::Change, choice.install_label);
    let remove = choice
        .can_remove
        .then(|| choice_marker(action == MapChoiceAction::Remove, "Remove"));
    let inactive = if highlighted {
        Style::default().fg(Color::Black)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let active = |color| {
        Style::default()
            .fg(if highlighted { Color::Black } else { color })
            .add_modifier(Modifier::BOLD)
    };
    let mut cells = vec![
        Cell::from(details),
        Cell::from(keep).style(if action == MapChoiceAction::Keep {
            active(Color::Green)
        } else {
            inactive
        }),
        Cell::from(change).style(if action == MapChoiceAction::Change {
            active(Color::Yellow)
        } else {
            inactive
        }),
    ];
    if mode.allows_removal() {
        cells.push(Cell::from(remove.unwrap_or_default()).style(
            if action == MapChoiceAction::Remove {
                active(Color::Red)
            } else {
                inactive
            },
        ));
    }
    Row::new(cells).height(2)
}

fn choice_marker(selected: bool, label: &str) -> String {
    if selected {
        format!("● {label}")
    } else {
        format!("○ {label}")
    }
}

#[derive(Debug, Clone, Copy)]
enum CycleDirection {
    Previous,
    Next,
}

fn cycle_current_action(
    state: &TableState,
    actions: &mut [MapChoiceAction],
    choices: &[MapChoice],
    mode: MapActionMode,
    direction: CycleDirection,
) {
    if let Some(selected) = state
        .selected()
        .filter(|selected| *selected < actions.len() && *selected < choices.len())
    {
        actions[selected] = next_map_action(actions[selected], &choices[selected], mode, direction);
    }
}

const fn next_map_action(
    current: MapChoiceAction,
    choice: &MapChoice,
    mode: MapActionMode,
    direction: CycleDirection,
) -> MapChoiceAction {
    let removal_available = mode.allows_removal() && choice.can_remove;
    match direction {
        CycleDirection::Next => match current {
            MapChoiceAction::Keep => MapChoiceAction::Change,
            MapChoiceAction::Change if removal_available => MapChoiceAction::Remove,
            MapChoiceAction::Change | MapChoiceAction::Remove => MapChoiceAction::Keep,
        },
        CycleDirection::Previous => match current {
            MapChoiceAction::Keep if removal_available => MapChoiceAction::Remove,
            MapChoiceAction::Keep | MapChoiceAction::Remove => MapChoiceAction::Change,
            MapChoiceAction::Change => MapChoiceAction::Keep,
        },
    }
}

fn selected_map_choices(actions: &[MapChoiceAction]) -> SelectedMapChoices {
    let mut changes = Vec::new();
    let mut removals = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        match action {
            MapChoiceAction::Keep => {}
            MapChoiceAction::Change => changes.push(index),
            MapChoiceAction::Remove => removals.push(index),
        }
    }
    SelectedMapChoices { changes, removals }
}

fn selected_map_choice_count(selected: &SelectedMapChoices, mode: MapActionMode) -> String {
    let change_count = selected.changes.len();
    let changes = format!(
        "{change_count} change{}",
        if change_count == 1 { "" } else { "s" }
    );
    if !mode.allows_removal() {
        return format!("{changes} selected");
    }
    let removal_count = selected.removals.len();
    format!(
        "{changes} · {removal_count} removal{} selected",
        if removal_count == 1 { "" } else { "s" }
    )
}

fn select_table_previous(state: &mut TableState) {
    let selected = state.selected().unwrap_or(0);
    state.select(Some(selected.saturating_sub(1)));
}

fn select_table_next(state: &mut TableState, len: usize) {
    let selected = state.selected().unwrap_or(0);
    state.select(Some((selected + 1).min(len.saturating_sub(1))));
}

fn select_map_cell(
    area: Rect,
    column: u16,
    row: u16,
    state: &mut TableState,
    actions: &mut [MapChoiceAction],
    choices: &[MapChoice],
    mode: MapActionMode,
) {
    let body_start = area.y.saturating_add(3);
    if row < body_start || row >= area.bottom().saturating_sub(1) {
        return;
    }
    let visible_row = usize::from((row - body_start) / 2);
    let selected = state.offset().saturating_add(visible_row);
    if selected >= actions.len() || selected >= choices.len() {
        return;
    }
    state.select(Some(selected));

    let right = area.right();
    let action_column_span = MAP_ACTION_COLUMN_WIDTH.saturating_add(MAP_TABLE_COLUMN_SPACING);
    let remove_start = right.saturating_sub(action_column_span);
    let change_start = if mode.allows_removal() {
        remove_start.saturating_sub(action_column_span)
    } else {
        right.saturating_sub(action_column_span)
    };
    let keep_start = change_start.saturating_sub(action_column_span);
    if mode.allows_removal() && column >= remove_start {
        if choices[selected].can_remove {
            actions[selected] = MapChoiceAction::Remove;
        }
    } else if column >= change_start {
        actions[selected] = MapChoiceAction::Change;
    } else if column >= keep_start {
        actions[selected] = MapChoiceAction::Keep;
    }
}

fn confirmation_loop(
    terminal: &mut AppTerminal,
    title: &str,
    body: ConfirmationBody,
    confirm_label: &str,
) -> Result<bool> {
    let config = confirmation_dialog_config(title, confirm_label);
    let mut state = DialogState::new(body);
    state.register_button(0);
    state.register_button(1);
    state.show();

    loop {
        terminal.draw(|frame| {
            render_dialog_backdrop(frame);
            {
                let mut dialog = PopupDialog::new(&config, &mut state, draw_confirmation_body);
                dialog.render(frame);
            }
            render_confirmation_footer(frame);
        })?;

        match event::read()? {
            Event::Key(key) if is_cancel_key(&key) => return Ok(false),
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('y' | 'Y') => return Ok(true),
                KeyCode::Char('n' | 'N') => return Ok(false),
                KeyCode::Left => state.focus.prev(),
                KeyCode::Right => state.focus.next(),
                _ => {
                    let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                    if let Some(confirmed) = confirmation_result(&dialog.handle_key(key)) {
                        return Ok(confirmed);
                    }
                }
            },
            Event::Mouse(mouse) => {
                let screen = Rect::from(terminal.size()?);
                let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                if let Some(confirmed) =
                    confirmation_result(&dialog.handle_mouse_with_screen(mouse, screen))
                {
                    return Ok(confirmed);
                }
            }
            _ => {}
        }
    }
}

fn pending_recovery_loop(
    terminal: &mut AppTerminal,
    body: ConfirmationBody,
    actions: PendingRecoveryActions,
) -> Result<PendingRecoveryDecision> {
    let config = pending_recovery_dialog_config(actions);
    let mut state = DialogState::new(body);
    for index in 0..config.buttons.len() {
        state.register_button(index);
    }
    state.show();

    loop {
        terminal.draw(|frame| {
            render_dialog_backdrop(frame);
            {
                let mut dialog = PopupDialog::new(&config, &mut state, draw_confirmation_body);
                dialog.render(frame);
            }
            render_pending_recovery_footer(frame, actions);
        })?;

        match event::read()? {
            Event::Key(key) if is_cancel_key(&key) => {
                return Ok(PendingRecoveryDecision::Cancel);
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let direct = match key.code {
                    KeyCode::Char('r' | 'R') if actions.allows_recover() => {
                        Some(PendingRecoveryDecision::Recover)
                    }
                    KeyCode::Char('c' | 'C') if actions.allows_clear() => {
                        Some(PendingRecoveryDecision::ClearState)
                    }
                    KeyCode::Char('d' | 'D') if actions.allows_discard() => {
                        Some(PendingRecoveryDecision::Discard)
                    }
                    KeyCode::Left | KeyCode::Up => {
                        state.focus.prev();
                        None
                    }
                    KeyCode::Right | KeyCode::Down => {
                        state.focus.next();
                        None
                    }
                    _ => {
                        let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                        pending_recovery_result(&dialog.handle_key(key))
                    }
                };
                if let Some(decision) = direct {
                    return Ok(decision);
                }
            }
            Event::Mouse(mouse) => {
                let screen = Rect::from(terminal.size()?);
                let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                if let Some(decision) =
                    pending_recovery_result(&dialog.handle_mouse_with_screen(mouse, screen))
                {
                    return Ok(decision);
                }
            }
            _ => {}
        }
    }
}

fn update_confirmation_loop(
    terminal: &mut AppTerminal,
    title: &str,
    body: UpdateConfirmationBody,
) -> Result<UpdateConfirmationDecision> {
    let mut state = DialogState::new(body);
    state.register_button(0);
    state.register_button(1);
    state.register_child(0);
    state.show();

    loop {
        state.children.backup.set_focused(state.is_child_focused(0));
        let config = confirmation_dialog_config(title, state.children.confirm_label());
        terminal.draw(|frame| {
            render_dialog_backdrop(frame);
            {
                let mut dialog =
                    PopupDialog::new(&config, &mut state, draw_update_confirmation_body);
                dialog.render(frame);
            }
            render_update_confirmation_footer(frame);
            if let Some(area) = state.children.backup_area {
                state
                    .click_regions
                    .register(area, DialogFocusTarget::Child(0));
            }
        })?;

        match event::read()? {
            Event::Key(key) if is_cancel_key(&key) => {
                return Ok(UpdateConfirmationDecision {
                    confirmed: false,
                    backup_enabled: state.children.backup_enabled(),
                });
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if state.is_child_focused(0)
                    && matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
                {
                    state.children.backup.toggle();
                    continue;
                }
                match key.code {
                    KeyCode::Char('y' | 'Y') => {
                        return Ok(UpdateConfirmationDecision {
                            confirmed: true,
                            backup_enabled: state.children.backup_enabled(),
                        });
                    }
                    KeyCode::Up => {
                        state.focus.prev();
                    }
                    KeyCode::Down => {
                        state.focus.next();
                    }
                    KeyCode::Left => state.focus.prev(),
                    KeyCode::Right => state.focus.next(),
                    _ => {
                        let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                        if let Some(confirmed) = confirmation_result(&dialog.handle_key(key)) {
                            return Ok(UpdateConfirmationDecision {
                                confirmed,
                                backup_enabled: state.children.backup_enabled(),
                            });
                        }
                    }
                }
            }
            Event::Mouse(mouse) => {
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                    && state
                        .children
                        .backup_area
                        .is_some_and(|area| area.contains((mouse.column, mouse.row).into()))
                {
                    state.focus.set(DialogFocusTarget::Child(0));
                    state.children.backup.toggle();
                    continue;
                }
                let screen = Rect::from(terminal.size()?);
                let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                if let Some(confirmed) =
                    confirmation_result(&dialog.handle_mouse_with_screen(mouse, screen))
                {
                    return Ok(UpdateConfirmationDecision {
                        confirmed,
                        backup_enabled: state.children.backup_enabled(),
                    });
                }
            }
            _ => {}
        }
    }
}

fn render_dialog_backdrop(frame: &mut ratatui::Frame<'_>) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::Rgb(5, 8, 13)),
        ),
        frame.area(),
    );
}

fn draw_profile_banner(frame: &mut ratatui::Frame<'_>, profile: RunProfile) {
    let (message, foreground, background) = match profile {
        RunProfile {
            service: ServiceEnvironment::Garmin,
            device_changes: DeviceChanges::Enabled,
        } => return,
        RunProfile {
            service: ServiceEnvironment::Garmin,
            device_changes: DeviceChanges::Disabled,
        } => (
            "DRY RUN — Garmin service; device writes disabled",
            Color::Black,
            Color::Yellow,
        ),
        RunProfile {
            service: ServiceEnvironment::Mock,
            device_changes: DeviceChanges::Enabled,
        } => (
            "MOCK — Local service; synthetic device",
            Color::White,
            Color::Magenta,
        ),
        RunProfile {
            service: ServiceEnvironment::Mock,
            device_changes: DeviceChanges::Disabled,
        } => (
            "MOCK DRY RUN — Local service; device writes disabled",
            Color::Black,
            Color::Yellow,
        ),
    };
    let area = Rect::new(frame.area().x, frame.area().y, frame.area().width, 1);
    let style = Style::default()
        .fg(foreground)
        .bg(background)
        .add_modifier(Modifier::BOLD);
    frame.render_widget(Block::default().style(style), area);
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
}

fn profile_content_area(area: Rect, profile: RunProfile) -> Rect {
    if profile == RunProfile::PRODUCTION {
        area
    } else {
        Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(1),
        )
    }
}

fn confirmation_dialog_config(title: &str, confirm_label: &str) -> DialogConfig {
    DialogConfig::new(title)
        .width_percent(88)
        .height_percent(96)
        .min_size(44, 20)
        .max_size(104, 26)
        .border_color(Color::Yellow)
        .focused_border_color(Color::Yellow)
        .close_on_outside_click(false)
        .buttons(vec![
            ("Cancel".to_owned(), ContainerAction::Close),
            (confirm_label.to_owned(), ContainerAction::Submit),
        ])
}

fn pending_recovery_dialog_config(actions: PendingRecoveryActions) -> DialogConfig {
    let (title, buttons) = if actions.allows_clear() {
        (
            "Pending device recovery detected",
            vec![
                (
                    "Clear state".to_owned(),
                    ContainerAction::custom("clear-state"),
                ),
                ("Recover now".to_owned(), ContainerAction::Submit),
            ],
        )
    } else if actions.allows_discard() {
        (
            "Interrupted preparation detected",
            vec![(
                "Discard attempt".to_owned(),
                ContainerAction::custom("discard"),
            )],
        )
    } else {
        (
            "Pending device recovery detected",
            vec![("Recover now".to_owned(), ContainerAction::Submit)],
        )
    };
    DialogConfig::new(title)
        .width_percent(88)
        .height_percent(96)
        .min_size(58, 22)
        .max_size(108, 28)
        .border_color(Color::Yellow)
        .focused_border_color(Color::Yellow)
        .close_on_outside_click(false)
        .buttons(buttons)
}

fn confirmation_result(result: &EventResult) -> Option<bool> {
    match result {
        EventResult::Action(ContainerAction::Submit) => Some(true),
        EventResult::Action(ContainerAction::Close) => Some(false),
        _ => None,
    }
}

fn pending_recovery_result(result: &EventResult) -> Option<PendingRecoveryDecision> {
    match result {
        EventResult::Action(ContainerAction::Submit) => Some(PendingRecoveryDecision::Recover),
        EventResult::Action(ContainerAction::Custom(action)) if action == "clear-state" => {
            Some(PendingRecoveryDecision::ClearState)
        }
        EventResult::Action(ContainerAction::Custom(action)) if action == "discard" => {
            Some(PendingRecoveryDecision::Discard)
        }
        EventResult::Action(ContainerAction::Close) => Some(PendingRecoveryDecision::Cancel),
        _ => None,
    }
}

fn draw_confirmation_body(frame: &mut ratatui::Frame<'_>, area: Rect, body: &mut ConfirmationBody) {
    draw_confirmation_content(frame, area, body);
}

fn draw_update_confirmation_body(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    body: &mut UpdateConfirmationBody,
) {
    body.backup_area = None;
    body.sync_plan_id();
    let panel = Block::default()
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(Color::Black));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let field_height = confirmation_fields_height(&body.details);
    let sections = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(field_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(4),
    ])
    .split(inner);
    render_confirmation_details(frame, &body.details, sections[0], sections[2], sections[4]);
    body.backup_area = Some(render_backup_option(frame, sections[6], &body.backup));
}

fn draw_confirmation_content(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    body: &mut ConfirmationBody,
) {
    let panel = Block::default()
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(Color::Black));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let field_height = confirmation_fields_height(body);
    let constraints = if body.fields.is_empty() {
        [
            Constraint::Min(3),
            Constraint::Length(0),
            Constraint::Length(0),
            Constraint::Length(0),
            Constraint::Length(0),
            Constraint::Length(0),
        ]
    } else {
        [
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(field_height),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(0),
        ]
    };
    let sections = Layout::vertical(constraints).split(inner);
    render_confirmation_details(frame, body, sections[0], sections[2], sections[4]);
}

fn confirmation_fields_height(body: &ConfirmationBody) -> u16 {
    body.fields.iter().fold(0_u16, |height, field| {
        height.saturating_add(field_value_height(&field.value))
    })
}

fn render_confirmation_details(
    frame: &mut ratatui::Frame<'_>,
    body: &ConfirmationBody,
    introduction_area: Rect,
    fields_area: Rect,
    note_area: Rect,
) {
    frame.render_widget(
        Paragraph::new(body.introduction.clone()).wrap(Wrap { trim: true }),
        introduction_area,
    );
    let rows = body.fields.iter().map(|field| {
        Row::new([
            Cell::from(format!("{}:", field.label)).style(Style::default().fg(Color::Gray)),
            Cell::from(field.value.as_str()).style(confirmation_value_style(field.tone)),
        ])
        .height(field_value_height(&field.value))
    });
    frame.render_widget(
        Table::new(rows, [Constraint::Length(16), Constraint::Min(1)]),
        fields_area,
    );
    if let Some(note) = &body.note {
        let note_area = Rect::new(
            note_area.x,
            note_area.y,
            note_area.width.min(76),
            note_area.height,
        );
        frame.render_widget(
            Paragraph::new(note.clone()).wrap(Wrap { trim: true }),
            note_area,
        );
    }
}

fn render_backup_option(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    backup: &CheckBoxState,
) -> Rect {
    frame.render_widget(
        Paragraph::new("Recovery:").style(Style::default().fg(Color::Gray)),
        Rect::new(area.x, area.y, area.width, 1),
    );

    let option_area = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(1).min(2),
    );
    let focus_style = selection_style();
    frame.render_widget(
        Block::default().style(if backup.focused {
            focus_style
        } else {
            Style::default().bg(Color::Black)
        }),
        option_area,
    );

    let columns =
        Layout::horizontal([Constraint::Length(6), Constraint::Min(1)]).split(option_area);
    let control_area = Rect::new(columns[0].x + 1, columns[0].y, 4, 1);
    CheckBox::new("", backup)
        .style(
            CheckBoxStyle::custom("[X]", "[ ]")
                .focused_fg(Color::Black)
                .unfocused_fg(Color::White)
                .checked_fg(Color::Green),
        )
        .render_stateful(control_area, frame.buffer_mut());

    let label_style = if backup.focused {
        focus_style.add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };
    let secondary_style = if backup.focused {
        focus_style
    } else if backup.checked {
        Style::default().fg(Color::Gray)
    } else {
        Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD)
    };
    let explanation = if backup.checked {
        "Recommended; enables automatic rollback if the update fails."
    } else {
        "No automatic rollback; old device files can be identified only by their authorized path and size, and reinstall may be required after a failed update."
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(Span::styled(
                "Create verified recovery backups",
                label_style,
            )),
            Line::from(Span::styled(explanation, secondary_style)),
        ]))
        .wrap(Wrap { trim: true }),
        columns[1],
    );

    option_area
}

#[derive(Debug, Clone, Copy)]
struct FooterHint<'a> {
    keys: &'a str,
    action: &'a str,
}

impl<'a> FooterHint<'a> {
    const fn new(keys: &'a str, action: &'a str) -> Self {
        Self { keys, action }
    }
}

fn render_hint_footer(frame: &mut ratatui::Frame<'_>, area: Rect, hints: &[FooterHint<'_>]) {
    if area.is_empty() || hints.is_empty() {
        return;
    }
    let area = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(18, 22, 29))),
        area,
    );
    let column_count = u32::try_from(hints.len()).unwrap_or(u32::MAX);
    let constraints =
        std::iter::repeat_n(Constraint::Ratio(1, column_count), hints.len()).collect::<Vec<_>>();
    let columns = Layout::horizontal(constraints).split(area);
    for (hint, column) in hints.iter().zip(columns.iter()) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    hint.keys,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(": ", Style::default().fg(Color::DarkGray)),
                Span::styled(hint.action, Style::default().fg(Color::White)),
            ]))
            .alignment(Alignment::Center),
            *column,
        );
    }
}

fn screen_footer_area(screen_area: Rect) -> Rect {
    Rect::new(
        screen_area.x,
        screen_area.bottom().saturating_sub(1),
        screen_area.width,
        u16::from(!screen_area.is_empty()),
    )
}

fn render_confirmation_footer(frame: &mut ratatui::Frame<'_>) {
    let footer_area = screen_footer_area(frame.area());
    render_hint_footer(
        frame,
        footer_area,
        &[
            FooterHint::new("Tab", "focus"),
            FooterHint::new("Enter", "confirm"),
            FooterHint::new("Esc", "cancel"),
        ],
    );
}

fn render_update_confirmation_footer(frame: &mut ratatui::Frame<'_>) {
    let footer_area = screen_footer_area(frame.area());
    render_hint_footer(
        frame,
        footer_area,
        &[
            FooterHint::new("Tab", "focus"),
            FooterHint::new("Space", "toggle"),
            FooterHint::new("Enter", "continue"),
            FooterHint::new("Esc", "cancel"),
        ],
    );
}

fn render_pending_recovery_footer(frame: &mut ratatui::Frame<'_>, actions: PendingRecoveryActions) {
    let footer_area = screen_footer_area(frame.area());
    let hints = if actions.allows_clear() {
        vec![
            FooterHint::new("←/→/Tab", "action"),
            FooterHint::new("Enter", "choose"),
            FooterHint::new("R/C", "recover/clear"),
            FooterHint::new("Esc", "cancel"),
        ]
    } else if actions.allows_discard() {
        vec![
            FooterHint::new("Enter or D", "discard attempt"),
            FooterHint::new("Esc", "cancel"),
        ]
    } else {
        vec![
            FooterHint::new("Enter or R", "recover"),
            FooterHint::new("Esc", "cancel"),
        ]
    };
    render_hint_footer(frame, footer_area, &hints);
}

fn render_abort_footer(frame: &mut ratatui::Frame<'_>) {
    let footer_area = screen_footer_area(frame.area());
    render_hint_footer(
        frame,
        footer_area,
        &[
            FooterHint::new("Tab", "focus"),
            FooterHint::new("Enter", "choose"),
            FooterHint::new("A", "abort"),
            FooterHint::new("Esc", "keep running"),
        ],
    );
}

fn field_value_height(value: &str) -> u16 {
    u16::try_from(value.lines().count().max(1)).unwrap_or(u16::MAX)
}

fn confirmation_value_style(tone: ConfirmationValueTone) -> Style {
    match tone {
        ConfirmationValueTone::Neutral => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        ConfirmationValueTone::Muted => Style::default()
            .fg(Color::Gray)
            .add_modifier(Modifier::BOLD),
        ConfirmationValueTone::Warning => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        ConfirmationValueTone::Path => path_style(),
    }
}

/// Runs a task while displaying its pipeline progress.
/// # Errors
/// Task, terminal setup, rendering, or input failure.
pub async fn run_progress<T, F>(
    receiver: ProgressReceiver,
    task: F,
    cancellation: CancellationToken,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let mut session = Session::open()?;
    session
        .run_pipeline_progress(receiver, task, cancellation)
        .await
}

impl Session {
    /// Discovers attached devices without leaving an empty alternate screen.
    /// # Errors
    /// Discovery, rendering, or cancellation failure.
    pub async fn discover_devices<T, F>(&mut self, task: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        loading_loop(self.terminal(), task, None, LoadingActivity::Discovery).await
    }

    /// Reads the selected device manifest without blocking terminal input.
    /// # Errors
    /// Inspection, rendering, or cancellation failure.
    pub async fn inspect_device<T, F>(&mut self, task: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        loading_loop(self.terminal(), task, None, LoadingActivity::DeviceMetadata).await
    }

    /// Recovers an unresponsive raw device link without hiding the transport reset.
    /// # Errors
    /// Recovery, rendering, or cancellation failure.
    pub async fn recover_device_link<T, F>(&mut self, task: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        loading_loop(self.terminal(), task, None, LoadingActivity::DeviceRecovery).await
    }

    /// Queries device metadata without blocking input.
    /// # Errors
    /// Query, rendering, or cancellation failure.
    pub async fn load_device_state<T, F>(&mut self, task: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        loading_loop(self.terminal(), task, None, LoadingActivity::DeviceStorage).await
    }

    /// Runs the map-component query while showing a cancellable loading screen.
    /// # Errors
    /// A task, cancellation, rendering, or input error.
    pub async fn load_available_components<T, F>(&mut self, task: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        let snapshot = self.device_state.clone();
        loading_loop(
            self.terminal(),
            task,
            snapshot.as_ref().and_then(|update| update.as_ref().ok()),
            LoadingActivity::MapComponents,
        )
        .await
    }

    /// Runs the shared update pipeline using the session's runtime profile.
    /// # Errors
    /// A task, rendering, or input error.
    pub async fn run_update_progress<T, F>(
        &mut self,
        receiver: ProgressReceiver,
        task: F,
        cancellation: CancellationToken,
        backup_policy: garmin_update::BackupPolicy,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        self.run_operation_progress(
            receiver,
            task,
            cancellation,
            ProgressConfig {
                title: UPDATE_PROGRESS_TITLE,
                initial: "Preparing update…",
                stages: UPDATE_STAGES,
                completion: Some(UPDATE_COMPLETE),
                initial_completion: (backup_policy == garmin_update::BackupPolicy::Skip).then_some(
                    InitialStageCompletion {
                        stage: OperationStage::Backup,
                        label: "Skipped by user; automatic rollback is unavailable",
                    },
                ),
            },
        )
        .await
    }

    /// Runs a real download-and-device pipeline probe in this session.
    /// # Errors
    /// A task, rendering, or input error.
    pub async fn run_pipeline_progress<T, F>(
        &mut self,
        receiver: ProgressReceiver,
        task: F,
        cancellation: CancellationToken,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        self.run_operation_progress(
            receiver,
            task,
            cancellation,
            ProgressConfig {
                title: "garmin-cli — Real update pipeline probe",
                initial: "Preparing pipeline benchmark…",
                stages: PIPELINE_PROBE_STAGES,
                completion: Some(PIPELINE_PROBE_COMPLETE),
                initial_completion: None,
            },
        )
        .await
    }

    /// Runs a disposable device-link benchmark in this session.
    /// # Errors
    /// A task, rendering, or input error.
    pub async fn run_link_benchmark_progress<T, F>(
        &mut self,
        receiver: ProgressReceiver,
        task: F,
        cancellation: CancellationToken,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        self.run_operation_progress(
            receiver,
            task,
            cancellation,
            ProgressConfig {
                title: "garmin-cli — Device link benchmark",
                initial: "Preparing disposable transfer…",
                stages: LINK_BENCHMARK_STAGES,
                completion: Some(LINK_BENCHMARK_COMPLETE),
                initial_completion: None,
            },
        )
        .await
    }

    /// Runs a guarded component removal and retains its final history.
    /// # Errors
    /// A task, rendering, cancellation, or input error.
    pub async fn run_removal_progress<T, F>(
        &mut self,
        receiver: ProgressReceiver,
        task: F,
        cancellation: CancellationToken,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        self.run_operation_progress(
            receiver,
            task,
            cancellation,
            ProgressConfig {
                title: "garmin-cli — Removing map components",
                initial: "Preparing guarded removal…",
                stages: REMOVAL_STAGES,
                completion: Some(REMOVAL_COMPLETE),
                initial_completion: None,
            },
        )
        .await
    }

    /// Recovers an update and retains its final history.
    /// # Errors
    /// A task, rendering, cancellation, or input error.
    pub async fn run_update_recovery_progress<T, F>(
        &mut self,
        receiver: ProgressReceiver,
        task: F,
        cancellation: CancellationToken,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        self.run_operation_progress(
            receiver,
            task,
            cancellation,
            ProgressConfig {
                title: "garmin-cli — Recovering map update",
                initial: "Inspecting update journal…",
                stages: UPDATE_RECOVERY_STAGES,
                completion: Some(UPDATE_RECOVERY_COMPLETE),
                initial_completion: None,
            },
        )
        .await
    }

    /// Recovers an interrupted component removal and retains its final history.
    /// # Errors
    /// A task, rendering, cancellation, or input error.
    pub async fn run_removal_recovery_progress<T, F>(
        &mut self,
        receiver: ProgressReceiver,
        task: F,
        cancellation: CancellationToken,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        self.run_operation_progress(
            receiver,
            task,
            cancellation,
            ProgressConfig {
                title: "garmin-cli — Recovering component removal",
                initial: "Inspecting removal journal…",
                stages: REMOVAL_RECOVERY_STAGES,
                completion: Some(REMOVAL_RECOVERY_COMPLETE),
                initial_completion: None,
            },
        )
        .await
    }

    async fn run_operation_progress<T, F>(
        &mut self,
        receiver: ProgressReceiver,
        task: F,
        cancellation: CancellationToken,
        config: ProgressConfig<'_>,
    ) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        let initial = self.device_state.clone();
        let result = progress_future_loop(
            self.terminal(),
            &receiver,
            task,
            cancellation,
            config,
            initial,
        )
        .await;
        if let Some(update) = receiver.device_state() {
            self.device_state = Some(update);
        }
        result
    }

    /// Displays a one-button informational dialog in the current session.
    /// # Errors
    /// Rendering or input failure.
    pub fn notice(&mut self, title: &str, message: &str) -> Result<()> {
        message_dialog_loop(
            self.terminal(),
            &notice_dialog_config(title),
            Text::raw(message.to_owned()),
        )
    }

    /// Displays a one-button failure dialog in the current session.
    /// # Errors
    /// Rendering or input failure.
    pub fn failure(&mut self, title: &str, body: FailureBody) -> Result<()> {
        failure_dialog_loop(self.terminal(), &failure_dialog_config(title), body)
    }

    /// Displays the successful installation summary in this session.
    /// # Errors
    /// Rendering or input failure.
    pub fn installation_complete(
        &mut self,
        device: &DeviceSummary,
        report: &garmin_update::ApplyReport,
        elapsed: Duration,
    ) -> Result<()> {
        completion_loop(self.terminal(), device, report, elapsed)
    }
}

#[derive(Debug, Clone, Copy)]
enum LoadingActivity {
    Discovery,
    DeviceMetadata,
    DeviceRecovery,
    DeviceStorage,
    MapComponents,
}

impl LoadingActivity {
    const fn title(self) -> &'static str {
        match self {
            Self::Discovery => " Device discovery ",
            Self::DeviceMetadata => " Device inspection ",
            Self::DeviceRecovery => " Device recovery ",
            Self::DeviceStorage => " Device storage ",
            Self::MapComponents => " Map components ",
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Discovery => "  Looking for attached Garmin devices…",
            Self::DeviceMetadata => "  Reading selected device metadata…",
            Self::DeviceRecovery => "  Resetting an unresponsive Garmin link…",
            Self::DeviceStorage => "  Reading device storage…",
            Self::MapComponents => "  Loading map components…",
        }
    }

    fn detail(self) -> Line<'static> {
        match self {
            Self::Discovery => Line::raw("Checking host-visible attachment candidates."),
            Self::DeviceMetadata => Line::from(vec![
                Span::raw("Reading and validating "),
                Span::styled("GarminDevice.xml", path_style()),
                Span::raw("."),
            ]),
            Self::DeviceRecovery => Line::raw("Clearing stalled MTP state, then waiting quietly."),
            Self::DeviceStorage => Line::raw("Reading storage totals without scanning files."),
            Self::MapComponents => Line::raw("Contacting the map-update service."),
        }
    }

    const fn cancellation_step(self) -> &'static str {
        match self {
            Self::Discovery => "device discovery",
            Self::DeviceMetadata => "device inspection",
            Self::DeviceRecovery => "device-link recovery",
            Self::DeviceStorage => "device storage inspection",
            Self::MapComponents => "map component loading",
        }
    }
}

async fn loading_loop<T, F>(
    terminal: &mut AppTerminal,
    task: F,
    snapshot: Option<&garmin_device::DeviceStateSnapshot>,
    activity: LoadingActivity,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::pin!(task);
    let close = close_signal();
    tokio::pin!(close);
    let mut frame_index = 0_usize;
    let started = Instant::now();
    loop {
        let area = terminal.content_area()?;
        terminal.draw(|frame| {
            let area = device_state::render(frame, area, snapshot);
            LoadingScreen::new(activity, frame_index, started.elapsed()).render(frame, area);
        })?;
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(key) if is_cancel_key(&key) => {
                    return Err(Cancelled::new(activity.cancellation_step()).into());
                }
                _ => {}
            }
        }
        tokio::select! {
            result = &mut task => return result,
            signal = &mut close => {
                signal?;
                return Err(anyhow::anyhow!(
                    "{} interrupted",
                    activity.cancellation_step()
                ));
            }
            () = tokio::time::sleep(LOADING_SPINNER_INTERVAL) => {
                frame_index = frame_index.wrapping_add(1);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LoadingScreen {
    activity: LoadingActivity,
    frame_index: usize,
    elapsed: Duration,
}

impl LoadingScreen {
    const fn new(activity: LoadingActivity, frame_index: usize, elapsed: Duration) -> Self {
        Self {
            activity,
            frame_index,
            elapsed,
        }
    }

    fn render(self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let outer = Block::default()
            .title(self.activity.title())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);
        let sections = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(vec![
                    Span::styled(
                        LOADING_SPINNER_FRAMES[self.frame_index % LOADING_SPINNER_FRAMES.len()],
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(self.activity.message()),
                ]),
                self.activity.detail(),
                Line::from(vec![
                    Span::styled("Elapsed ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        elapsed_clock(self.elapsed),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ]))
            .alignment(Alignment::Center),
            sections[1],
        );
        render_hint_footer(
            frame,
            sections[3],
            &[FooterHint::new("Q/Esc/Ctrl-C", "cancel")],
        );
    }
}

fn elapsed_clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn message_dialog_loop(
    terminal: &mut AppTerminal,
    config: &DialogConfig,
    message: Text<'static>,
) -> Result<()> {
    let mut state = DialogState::new(NoticeBody { message });
    state.register_button(0);
    state.show();
    loop {
        terminal.draw(|frame| {
            render_dialog_backdrop(frame);
            PopupDialog::new(config, &mut state, draw_notice_body).render(frame);
        })?;
        match event::read()? {
            Event::Key(key) if is_cancel_key(&key) => return Ok(()),
            Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Enter => {
                return Ok(());
            }
            Event::Mouse(mouse) => {
                let screen = Rect::from(terminal.size()?);
                let mut dialog = PopupDialog::new(config, &mut state, |_, _, _| {});
                if matches!(
                    dialog.handle_mouse_with_screen(mouse, screen),
                    EventResult::Action(ContainerAction::Close)
                ) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

fn failure_dialog_loop(
    terminal: &mut AppTerminal,
    config: &DialogConfig,
    body: FailureBody,
) -> Result<()> {
    let mut state = DialogState::new(body);
    state.register_button(0);
    state.show();
    loop {
        terminal.draw(|frame| {
            render_dialog_backdrop(frame);
            PopupDialog::new(config, &mut state, draw_failure_body).render(frame);
        })?;
        match event::read()? {
            Event::Key(key) if is_cancel_key(&key) => return Ok(()),
            Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Enter => {
                return Ok(());
            }
            Event::Mouse(mouse) => {
                let screen = Rect::from(terminal.size()?);
                let mut dialog = PopupDialog::new(config, &mut state, |_, _, _| {});
                if matches!(
                    dialog.handle_mouse_with_screen(mouse, screen),
                    EventResult::Action(ContainerAction::Close)
                ) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

struct NoticeBody {
    message: Text<'static>,
}

fn notice_dialog_config(title: &str) -> DialogConfig {
    DialogConfig::new(title)
        .width_percent(84)
        .height_percent(70)
        .min_size(48, 14)
        .max_size(96, 18)
        .border_color(Color::Green)
        .focused_border_color(Color::Green)
        .close_on_outside_click(false)
        .buttons(vec![("Close".to_owned(), ContainerAction::Close)])
}

fn failure_dialog_config(title: &str) -> DialogConfig {
    DialogConfig::new(title)
        .width_percent(84)
        .height_percent(70)
        .min_size(48, 14)
        .max_size(96, 18)
        .border_color(Color::Red)
        .focused_border_color(Color::Red)
        .close_on_outside_click(false)
        .buttons(vec![("Close".to_owned(), ContainerAction::Close)])
}

fn draw_notice_body(frame: &mut ratatui::Frame<'_>, area: Rect, body: &mut NoticeBody) {
    frame.render_widget(
        Paragraph::new(body.message.clone())
            .wrap(Wrap { trim: true })
            .block(Block::default().padding(Padding::new(2, 2, 1, 1))),
        area,
    );
}

fn draw_failure_body(frame: &mut ratatui::Frame<'_>, area: Rect, body: &mut FailureBody) {
    let mut lines = vec![Line::raw(body.summary.clone()), Line::raw("")];
    for field in &body.fields {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", field.label),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(field.value.clone(), failure_value_style(field.tone)),
        ]));
    }
    if !body.fields.is_empty() {
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(body.outcome.clone()));
    if let Some(diagnostics) = &body.diagnostics {
        lines.push(Line::raw("Diagnostics:"));
        lines.push(Line::from(Span::styled(diagnostics.clone(), path_style())));
    } else {
        lines.push(Line::raw("No diagnostic capture was requested."));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: true })
            .block(Block::default().padding(Padding::new(2, 2, 1, 1))),
        area,
    );
}

fn failure_value_style(tone: FailureValueTone) -> Style {
    match tone {
        FailureValueTone::Neutral => Style::default().fg(Color::White),
        FailureValueTone::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        FailureValueTone::Path => path_style(),
        FailureValueTone::Url => link_style(),
    }
}

#[derive(Debug, Clone)]
struct CompletionBody {
    device: DeviceSummary,
    report: garmin_update::ApplyReport,
    workflow_elapsed: Duration,
}

fn completion_loop(
    terminal: &mut AppTerminal,
    device: &DeviceSummary,
    report: &garmin_update::ApplyReport,
    workflow_elapsed: Duration,
) -> Result<()> {
    let config = completion_dialog_config();
    let body = CompletionBody {
        device: device.clone(),
        report: report.clone(),
        workflow_elapsed,
    };
    let mut state = DialogState::new(body);
    state.register_button(0);
    state.show();

    loop {
        terminal.draw(|frame| {
            render_dialog_backdrop(frame);
            PopupDialog::new(&config, &mut state, draw_completion_body).render(frame);
        })?;

        match event::read()? {
            Event::Key(key) if is_cancel_key(&key) => return Ok(()),
            Event::Key(key) if key.kind == KeyEventKind::Press && key.code == KeyCode::Enter => {
                return Ok(());
            }
            Event::Mouse(mouse) => {
                let screen = Rect::from(terminal.size()?);
                let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                if matches!(
                    dialog.handle_mouse_with_screen(mouse, screen),
                    EventResult::Action(ContainerAction::Close)
                ) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

fn completion_dialog_config() -> DialogConfig {
    DialogConfig::new("Update complete")
        .width_percent(84)
        .height_percent(85)
        .min_size(48, 18)
        .max_size(96, 20)
        .border_color(Color::Green)
        .focused_border_color(Color::Green)
        .close_on_outside_click(false)
        .buttons(vec![("Close".to_owned(), ContainerAction::Close)])
}

fn draw_completion_body(frame: &mut ratatui::Frame<'_>, area: Rect, body: &mut CompletionBody) {
    let panel = Block::default()
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(Color::Black));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let rate = display_rate(body.report.bytes_per_second());
    let fields = [
        ("Device", completion_device_line(&body.device)),
        (
            "Installed",
            completion_value_line(format!("{} files", body.report.files_written)),
        ),
        (
            "Removed",
            completion_value_line(format!("{} files", body.report.files_removed)),
        ),
        (
            "Device transfer",
            completion_value_line(format!(
                "{} in {:.2}s · {rate}",
                decimal_bytes(body.report.bytes_written),
                body.report.elapsed.as_secs_f64()
            )),
        ),
        (
            "Transaction",
            completion_value_line(format!(
                "Committed and cleaned up in {:.2}s",
                body.workflow_elapsed.as_secs_f64()
            )),
        ),
    ];
    let label_width = 18;
    let value_width = inner.width.saturating_sub(label_width);
    let field_heights: [u16; 5] = std::array::from_fn(|index| {
        let value = &fields[index].1;
        u16::try_from(
            Paragraph::new(value.clone())
                .wrap(Wrap { trim: true })
                .line_count(value_width),
        )
        .unwrap_or(u16::MAX)
        .max(1)
    });
    let details_height = field_heights
        .iter()
        .copied()
        .fold(0_u16, u16::saturating_add);
    let sections = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(details_height),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new("Map update installed.").style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        sections[0],
    );
    let mut row_y = sections[1].y;
    for ((label, value), height) in fields.into_iter().zip(field_heights) {
        let row = Rect::new(sections[1].x, row_y, sections[1].width, height);
        let columns =
            Layout::horizontal([Constraint::Length(label_width), Constraint::Min(1)]).split(row);
        frame.render_widget(
            Paragraph::new(format!("{label}:")).style(Style::default().fg(Color::Gray)),
            columns[0],
        );
        frame.render_widget(Paragraph::new(value).wrap(Wrap { trim: true }), columns[1]);
        row_y = row_y.saturating_add(height);
    }
    frame.render_widget(
        Paragraph::new(COMPLETION_INSTRUCTIONS).wrap(Wrap { trim: true }),
        sections[3],
    );
    frame.render_widget(
        Paragraph::new(key_hints(&[("Enter, Esc, q, or mouse", "close")]))
            .alignment(Alignment::Center),
        sections[4],
    );
}

fn completion_device_line(device: &DeviceSummary) -> Line<'static> {
    let part_number = device
        .part_number
        .as_deref()
        .map(|part_number| format!(" ({part_number})"))
        .unwrap_or_default();
    let transport = match device.transport {
        TransportKind::MassStorage => "mass storage",
        TransportKind::Mtp => "MTP",
        TransportKind::MountedMtp => "desktop-mounted MTP",
    };
    let value_style = Style::default().add_modifier(Modifier::BOLD);
    let location_style = if device.transport == TransportKind::MassStorage {
        path_style()
    } else {
        value_style
    };
    Line::from(vec![
        Span::styled(
            format!("{}{part_number} — {transport} at ", device.model),
            value_style,
        ),
        Span::styled(device.location.clone(), location_style),
    ])
}

fn completion_value_line(value: String) -> Line<'static> {
    Line::from(Span::styled(
        value,
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

fn display_rate(bytes_per_second: f64) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the non-negative rounded rate is saturated for display as bytes"
    )]
    let bytes_per_second = bytes_per_second.max(0.0).round() as u64;
    format!("{}/s", decimal_bytes(bytes_per_second))
}

#[derive(Debug, Clone, Copy)]
struct ProgressConfig<'a> {
    title: &'a str,
    initial: &'a str,
    stages: &'a [OperationStage],
    completion: Option<&'a str>,
    initial_completion: Option<InitialStageCompletion<'a>>,
}

#[derive(Debug, Clone, Copy)]
struct InitialStageCompletion<'a> {
    stage: OperationStage,
    label: &'a str,
}

async fn progress_future_loop<T, F>(
    terminal: &mut AppTerminal,
    receiver: &ProgressReceiver,
    task: F,
    cancellation: CancellationToken,
    config: ProgressConfig<'_>,
    mut device_state: Option<DeviceStateUpdate>,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::pin!(task);
    let mut model = ProgressModel::new(config.stages, config.initial);
    if let Some(initial) = config.initial_completion {
        model.set_initial_completion(initial.stage, initial.label);
    }
    let mut scroll = DashboardScroll::default();

    let mut abort_requested = false;
    let close = close_signal();
    tokio::pin!(close);

    loop {
        drain_progress_events(receiver, &mut model);
        if let Some(update) = receiver.device_state() {
            device_state = Some(update);
        }
        let viewport = draw_progress_dashboard(
            terminal,
            config.title,
            &model,
            &mut scroll,
            if abort_requested {
                ProgressPhase::Cancelling
            } else {
                ProgressPhase::Running
            },
            device_state.as_ref(),
        )?;
        let phase = if abort_requested {
            ProgressPhase::Cancelling
        } else {
            ProgressPhase::Running
        };
        match handle_progress_input(&mut scroll, &viewport, phase)? {
            ProgressInput::Abort if confirm_abort(terminal, &model.current).await? => {
                cancellation.cancel();
                abort_requested = true;
            }
            ProgressInput::Continue | ProgressInput::Abort | ProgressInput::Close => {}
        }

        tokio::select! {
            result = &mut task => match result {
                Ok(value) => {
                    drain_progress_events(receiver, &mut model);
                    if let Some(completion) = config.completion {
                        model.current = OperationView::completed(completion);
                        await_progress_close(
                            terminal,
                            config.title,
                            &model,
                            &mut scroll,
                            receiver,
                            &mut device_state,
                        )
                        .await?;
                    }

                    return Ok(value);
                }
                Err(error) => return Err(error),
            },
            signal = &mut close, if !abort_requested => {
                signal?;
                if confirm_abort(terminal, &model.current).await? {
                    cancellation.cancel();
                    abort_requested = true;
                } else {
                    close.set(close_signal());
                }
            }
            () = tokio::time::sleep(Duration::from_millis(40)) => {}
        }
    }
}

fn drain_progress_events(receiver: &ProgressReceiver, model: &mut ProgressModel) {
    for event in receiver.try_iter() {
        model.apply(&event);
    }
}

async fn await_progress_close(
    terminal: &mut AppTerminal,
    title: &str,
    model: &ProgressModel,
    scroll: &mut DashboardScroll,
    receiver: &ProgressReceiver,
    device_state: &mut Option<DeviceStateUpdate>,
) -> Result<()> {
    let close = close_signal();
    tokio::pin!(close);
    loop {
        if let Some(update) = receiver.device_state() {
            *device_state = Some(update);
        }
        let viewport = draw_progress_dashboard(
            terminal,
            title,
            model,
            scroll,
            ProgressPhase::Complete,
            device_state.as_ref(),
        )?;
        if handle_progress_input(scroll, &viewport, ProgressPhase::Complete)?
            == ProgressInput::Close
        {
            return Ok(());
        }
        tokio::select! {
            signal = &mut close => return signal.map_err(Into::into),
            () = tokio::time::sleep(Duration::from_millis(40)) => {}
        }
    }
}

fn draw_progress_dashboard(
    terminal: &mut AppTerminal,
    title: &str,
    model: &ProgressModel,
    scroll: &mut DashboardScroll,
    phase: ProgressPhase,
    device_state: Option<&DeviceStateUpdate>,
) -> Result<DashboardViewport> {
    let area = terminal.content_area()?;
    let mut viewport = DashboardViewport::default();
    terminal.draw(|frame| {
        let area = device_state::render_update(frame, area, device_state);
        viewport = render_progress_dashboard(
            frame,
            area,
            ProgressPresentation { title, phase },
            model,
            scroll,
        );
    })?;
    Ok(viewport)
}

#[derive(Debug, Clone, Copy)]
struct ProgressPresentation<'a> {
    title: &'a str,
    phase: ProgressPhase,
}

fn render_progress_footer(frame: &mut ratatui::Frame<'_>, area: Rect, phase: ProgressPhase) {
    match phase {
        ProgressPhase::Running => render_hint_footer(
            frame,
            area,
            &[
                FooterHint::new("Tab", "panel"),
                FooterHint::new("↑↓/PgUp/PgDn/wheel", "scroll"),
                FooterHint::new("Q/Esc/Ctrl-C", "abort operation"),
            ],
        ),
        ProgressPhase::Cancelling => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Abort requested — waiting for the current operation to stop safely.",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )))
                .alignment(Alignment::Center),
                Rect::new(area.x, area.y, area.width, 1),
            );
        }
        ProgressPhase::Complete => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "Operation complete — review the history, then close when ready.",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )))
                .alignment(Alignment::Center),
                Rect::new(area.x, area.y, area.width, 1),
            );
            render_hint_footer(
                frame,
                area,
                &[
                    FooterHint::new("↑↓/wheel", "history"),
                    FooterHint::new("Enter/Esc/Q/Ctrl-C", "close"),
                ],
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressInput {
    Continue,
    Abort,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressPhase {
    Running,
    Cancelling,
    Complete,
}

fn handle_progress_input(
    scroll: &mut DashboardScroll,
    viewport: &DashboardViewport,
    phase: ProgressPhase,
) -> Result<ProgressInput> {
    while event::poll(Duration::ZERO)? {
        let input = scroll.input(&event::read()?, viewport, phase);
        if input != ProgressInput::Continue {
            return Ok(input);
        }
    }
    Ok(ProgressInput::Continue)
}

#[derive(Debug, Clone)]
struct AbortBody {
    operation: OperationView,
}

async fn confirm_abort(terminal: &mut AppTerminal, operation: &OperationView) -> Result<bool> {
    let config = abort_dialog_config();
    let mut state = DialogState::new(AbortBody {
        operation: operation.clone(),
    });
    state.register_button(0);
    state.register_button(1);
    state.show();
    let close = close_signal();
    tokio::pin!(close);

    loop {
        terminal.draw(|frame| {
            render_dialog_backdrop(frame);
            {
                let mut dialog = PopupDialog::new(&config, &mut state, draw_abort_body);
                dialog.render(frame);
            }
            render_abort_footer(frame);
        })?;

        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(key) if is_ctrl_c_key(&key) => return Ok(true),
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('a' | 'A') => return Ok(true),
                    KeyCode::Esc | KeyCode::Char('n' | 'N') => return Ok(false),
                    KeyCode::Left => state.focus.prev(),
                    KeyCode::Right => state.focus.next(),
                    _ => {
                        let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                        if let Some(abort) = confirmation_result(&dialog.handle_key(key)) {
                            return Ok(abort);
                        }
                    }
                },
                Event::Mouse(mouse) => {
                    let screen = Rect::from(terminal.size()?);
                    let mut dialog = PopupDialog::new(&config, &mut state, |_, _, _| {});
                    if let Some(abort) =
                        confirmation_result(&dialog.handle_mouse_with_screen(mouse, screen))
                    {
                        return Ok(abort);
                    }
                }
                _ => {}
            }
        }

        tokio::select! {
            signal = &mut close => {
                signal?;
                return Ok(true);
            }
            () = tokio::time::sleep(Duration::from_millis(40)) => {}
        }
    }
}

fn abort_dialog_config() -> DialogConfig {
    DialogConfig::new("Abort operation?")
        .width_percent(82)
        .height_percent(70)
        .min_size(48, 14)
        .max_size(92, 17)
        .border_color(Color::Yellow)
        .focused_border_color(Color::Yellow)
        .close_on_outside_click(false)
        .buttons(vec![
            ("Keep running".to_owned(), ContainerAction::Close),
            ("Request abort".to_owned(), ContainerAction::Submit),
        ])
}

fn draw_abort_body(frame: &mut ratatui::Frame<'_>, area: Rect, body: &mut AbortBody) {
    let panel = Block::default()
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(Color::Black));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);
    let sections = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new("Request cancellation at the next safe checkpoint?")
            .wrap(Wrap { trim: true }),
        sections[0],
    );
    frame.render_widget(
        Table::new(
            [Row::new([
                Cell::from("Active operation:").style(Style::default().fg(Color::Gray)),
                Cell::from(operation_line(&body.operation, false)),
            ])],
            [Constraint::Length(19), Constraint::Min(1)],
        ),
        sections[1],
    );
    frame.render_widget(
        Paragraph::new(
            "Cancellation may require rollback or later recovery. Do not disconnect until this screen closes.",
        )
        .wrap(Wrap { trim: true }),
        sections[2],
    );
}

fn stage_name(stage: OperationStage) -> &'static str {
    match stage {
        OperationStage::Inspect => "Inspect",
        OperationStage::Query => "Query",
        OperationStage::Plan => "Plan",
        OperationStage::Backup => "Backup",
        OperationStage::Download => "Download",
        OperationStage::Verify => "Verify",
        OperationStage::Authorize => "Authorize",
        OperationStage::Stage => "Stage",
        OperationStage::Commit => "Commit",
        OperationStage::Cleanup => "Cleanup",
        OperationStage::Upload => "Upload",
        OperationStage::DeviceFinalize => "Device finalize",
        OperationStage::DeviceVerify => "Device verify",
        OperationStage::Delete => "Delete",
    }
}

fn stage_symbol(state: ProgressState) -> &'static str {
    match state {
        ProgressState::Started => "▶",
        ProgressState::Advanced => "●",
        ProgressState::Completed => "✓",
        ProgressState::Failed => "✗",
    }
}

fn path_style() -> Style {
    Style::default().fg(Color::Rgb(135, 175, 205))
}

fn link_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED)
}

fn operation_line(operation: &OperationView, include_state: bool) -> Line<'static> {
    let mut spans = Vec::new();
    if include_state {
        spans.extend(history_prefix(operation));
    }
    if let Some(stage) = operation.stage {
        spans.push(Span::styled(
            stage_name(stage),
            Style::default().fg(Color::Gray),
        ));
        spans.push(Span::styled(" — ", Style::default().fg(Color::DarkGray)));
    }
    spans.push(Span::raw(operation.label.clone()));
    if let Some(path) = &operation.path {
        spans.push(Span::styled(" — ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(path.clone(), path_style()));
    }
    Line::from(spans)
}

fn history_prefix(operation: &OperationView) -> Vec<Span<'static>> {
    let state = operation.state.unwrap_or(ProgressState::Started);
    vec![
        Span::styled(
            stage_symbol(state),
            Style::default().fg(state_color(operation.state)),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            history_timestamp(operation.recorded_at),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            history_duration(operation.duration),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
    ]
}

fn history_timestamp(recorded_at: Option<SystemTime>) -> String {
    recorded_at
        .and_then(|timestamp| jiff::Zoned::try_from(timestamp).ok())
        .map_or_else(
            || "--:--:--".to_owned(),
            |timestamp| timestamp.strftime("%H:%M:%S").to_string(),
        )
}

fn history_duration(duration: Option<Duration>) -> String {
    let Some(duration) = duration else {
        return "--:--:--".to_owned();
    };
    if duration < Duration::from_secs(1) {
        return format!("00:{:05.2}", duration.as_secs_f64());
    }
    elapsed_clock(duration)
}

fn operation_text(operation: &OperationView, include_state: bool) -> Text<'static> {
    Text::from(operation_line(operation, include_state))
}

fn progress_gauge(view: &StageView, width: u16) -> Gauge<'static> {
    Gauge::default()
        .gauge_style(
            Style::default()
                .fg(progress_fill_color(view.state))
                .bg(Color::Black),
        )
        .ratio(progress_ratio(view))
        .label(" ".repeat(usize::from(width)))
}

fn render_progress_gauge(
    frame: &mut ratatui::Frame<'_>,
    stage: OperationStage,
    view: &StageView,
    area: Rect,
    model: &ProgressModel,
) {
    frame.render_widget(progress_gauge(view, area.width), area);
    frame.render_widget(
        Paragraph::new(left_aligned_gauge_label(stage, view, model, area.width)),
        area,
    );
}

fn left_aligned_gauge_label(
    stage: OperationStage,
    view: &StageView,
    model: &ProgressModel,
    width: u16,
) -> Line<'static> {
    // Gauge reverses its foreground inside the fill, so the overlay must not inherit that black.
    let mut label = Line::from(Span::styled(
        format!("  {}", dashboard_gauge_label(stage, view, model)),
        Style::default().fg(Color::Gray),
    ));
    if let Some(path) = &view.path {
        label.push_span(Span::styled(" — ", Style::default().fg(Color::DarkGray)));
        label.push_span(Span::styled(path.clone(), path_style()));
    }
    let label_width = label.width();
    label.push_span(Span::raw(
        " ".repeat(usize::from(width).saturating_sub(label_width)),
    ));
    label
}

fn state_icon(view: &StageView) -> &'static str {
    view.state.map_or("·", stage_symbol)
}

fn state_color(state: Option<ProgressState>) -> Color {
    match state {
        Some(ProgressState::Started | ProgressState::Advanced) => Color::Cyan,
        Some(ProgressState::Completed) => Color::Green,
        Some(ProgressState::Failed) => Color::Red,
        None => Color::DarkGray,
    }
}

fn progress_fill_color(state: Option<ProgressState>) -> Color {
    match state {
        Some(ProgressState::Started | ProgressState::Advanced) => Color::Rgb(18, 61, 73),
        Some(ProgressState::Completed) => Color::Rgb(24, 61, 36),
        Some(ProgressState::Failed) => Color::Rgb(87, 31, 44),
        None => Color::Black,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "the gauge API requires an approximate f64 ratio"
)]
fn progress_ratio(view: &StageView) -> f64 {
    match (view.state, view.total) {
        (Some(ProgressState::Completed), _) => 1.0,
        (_, Some(total)) if total > 0 => (view.completed as f64 / total as f64).clamp(0.0, 1.0),
        (Some(ProgressState::Started | ProgressState::Advanced), _) => 0.03,
        _ => 0.0,
    }
}

fn gauge_label(stage: OperationStage, view: &StageView) -> String {
    gauge_label_with_metrics(stage, view, &progress_metrics(stage, view))
}

fn dashboard_gauge_label(stage: OperationStage, view: &StageView, model: &ProgressModel) -> String {
    if stage == OperationStage::Verify && model.stage_is_active(OperationStage::Download) {
        let (completed, total) = model.item_completion(OperationStage::Verify);
        let mut metrics = Vec::with_capacity(3);
        if total > 0 {
            metrics.push(format!(
                "{completed}/{total} file{} verified",
                if total == 1 { "" } else { "s" }
            ));
        }
        metrics.push("waiting for active downloads".to_owned());
        if let Some(elapsed) = stage_elapsed(view) {
            metrics.push(format!("{} elapsed", progress_elapsed(elapsed)));
        }
        return gauge_label_with_metrics(stage, view, &metrics);
    }
    if view.is_active() && stale_byte_progress(view).is_some() && model.active().next().is_some() {
        let mut metrics = Vec::with_capacity(2);
        if let Some(elapsed) = stage_elapsed(view) {
            metrics.push(format!("{} elapsed", progress_elapsed(elapsed)));
        }
        metrics.push("current file progress shown below".to_owned());
        return gauge_label_with_metrics(stage, view, &metrics);
    }
    gauge_label(stage, view)
}

fn gauge_label_with_metrics(stage: OperationStage, view: &StageView, metrics: &[String]) -> String {
    let mut label = match view.total {
        Some(total)
            if view.unit == ProgressUnit::Operations
                && (stage == OperationStage::Commit || total > 1) =>
        {
            format!("{} / {total} operations — {}", view.completed, view.label)
        }
        Some(total) if view.unit == ProgressUnit::Bytes && total > 1 => {
            let completed = decimal_bytes(view.completed);
            let total = decimal_bytes(total);
            format!("{completed} / {total} — {}", view.label)
        }
        _ => view.label.clone(),
    };
    if !metrics.is_empty() {
        let (prefix, description) = label
            .split_once(" — ")
            .map_or((None, label.as_str()), |(prefix, description)| {
                (Some(prefix), description)
            });
        label = match prefix {
            Some(prefix) => format!("{prefix} · {} — {description}", metrics.join(" · ")),
            None => format!("{} · {description}", metrics.join(" · ")),
        };
    }
    label
}

fn stage_elapsed(view: &StageView) -> Option<Duration> {
    view.elapsed
        .or_else(|| {
            view.started_recorded_at
                .and_then(|started| SystemTime::now().duration_since(started).ok())
        })
        .or_else(|| view.started_at.map(|started| started.elapsed()))
}

fn stale_byte_progress(view: &StageView) -> Option<Duration> {
    if view.unit != ProgressUnit::Bytes || !view.is_active() {
        return None;
    }
    view.updated_at
        .map(|updated| updated.elapsed())
        .filter(|idle| *idle >= STALE_BYTE_PROGRESS_AFTER)
}

fn byte_sample_elapsed(view: &StageView) -> Option<Duration> {
    view.started_recorded_at
        .zip(view.updated_recorded_at)
        .and_then(|(started, updated)| updated.duration_since(started).ok())
        .or_else(|| {
            view.started_at
                .zip(view.updated_at)
                .map(|(started, updated)| updated.saturating_duration_since(started))
        })
        .or(view.elapsed)
}

fn progress_metrics(_stage: OperationStage, view: &StageView) -> Vec<String> {
    let Some(elapsed) = stage_elapsed(view) else {
        return Vec::new();
    };
    if let Some(idle) = stale_byte_progress(view) {
        return vec![
            format!("{} elapsed", progress_elapsed(elapsed)),
            format!(
                "no progress for {}; device finalizing or stalled",
                progress_elapsed(idle)
            ),
        ];
    }
    let mut metrics = Vec::with_capacity(3);
    let bytes_per_second = if let Some(sample_elapsed) = byte_sample_elapsed(view)
        && view.unit == ProgressUnit::Bytes
        && view.completed > 0
        && sample_elapsed >= MIN_RATE_SAMPLE_DURATION
    {
        #[expect(
            clippy::cast_precision_loss,
            reason = "an approximate transfer rate is intentionally represented as f64"
        )]
        let rate = view.completed as f64 / sample_elapsed.as_secs_f64();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the non-negative rounded rate is saturated for display as bytes"
        )]
        let rounded = rate.max(0.0).round() as u64;
        metrics.push(format!("avg {}/s", decimal_bytes(rounded)));
        Some(rate)
    } else {
        None
    };
    metrics.push(format!("{} elapsed", progress_elapsed(elapsed)));
    if view.is_active()
        && let (Some(total), Some(rate)) = (view.total, bytes_per_second)
        && total > view.completed
        && rate.is_finite()
        && rate > 0.0
    {
        #[expect(
            clippy::cast_precision_loss,
            reason = "an approximate transfer ETA is intentionally represented as f64"
        )]
        let seconds = (total - view.completed) as f64 / rate;
        if let Ok(eta) = Duration::try_from_secs_f64(seconds.max(0.0)) {
            metrics.push(format!("ETA {}", elapsed_clock(eta)));
        }
    }
    metrics
}

fn progress_elapsed(elapsed: Duration) -> String {
    if elapsed < Duration::from_secs(1) {
        format!("{:.2}s", elapsed.as_secs_f64())
    } else {
        elapsed_clock(elapsed)
    }
}

fn decimal_bytes(bytes: u64) -> String {
    format!(
        "{:.2}",
        Byte::from_u64(bytes).get_appropriate_unit(UnitType::Decimal)
    )
}

fn device_selection_loop(
    terminal: &mut AppTerminal,
    rows: &[String],
    title: &str,
    selectable: bool,
    selected: usize,
    rescan_interval: Option<Duration>,
) -> Result<DeviceSelection> {
    let mut state =
        ListState::default().with_selected(Some(selected.min(rows.len().saturating_sub(1))));
    let rescan_at = rescan_interval.map(|interval| Instant::now() + interval);
    loop {
        let content_area = terminal.content_area()?;
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(5)])
            .split(content_area);
        terminal.draw(|frame| {
            render_device_selection(
                frame,
                content_area,
                rows,
                title,
                rescan_interval.is_some(),
                &mut state,
            );
        })?;

        let next_event = match rescan_at {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if !event::poll(remaining)? {
                    return Ok(DeviceSelection::Rescan(state.selected().unwrap_or(0)));
                }
                event::read()?
            }
            None => event::read()?,
        };
        match next_event {
            Event::Key(key) if is_cancel_key(&key) => {
                return Ok(DeviceSelection::Cancelled);
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('r' | 'R') if rescan_interval.is_some() => {
                    return Ok(DeviceSelection::Rescan(state.selected().unwrap_or(0)));
                }
                KeyCode::Enter if selectable => {
                    return Ok(DeviceSelection::Selected(state.selected().unwrap_or(0)));
                }
                KeyCode::Up => select_previous(&mut state),
                KeyCode::Down => select_next(&mut state, rows.len()),
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => select_previous(&mut state),
                MouseEventKind::ScrollDown => select_next(&mut state, rows.len()),
                MouseEventKind::Down(MouseButton::Left) => {
                    let list_area = areas[1];
                    if selectable
                        && mouse.column > list_area.x
                        && mouse.column < list_area.right().saturating_sub(1)
                        && mouse.row > list_area.y
                        && mouse.row < list_area.bottom().saturating_sub(1)
                    {
                        let index = usize::from(mouse.row.saturating_sub(list_area.y + 1));
                        if index < rows.len() {
                            state.select(Some(index));
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn render_device_selection(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[String],
    title: &str,
    refreshable: bool,
    state: &mut ListState,
) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(area);
    let mut hints = vec![
        ("↑/↓ or Mouse", "select"),
        ("Enter", "continue"),
        ("Esc or Ctrl-C", "cancel"),
    ];
    if refreshable {
        hints.insert(hints.len().saturating_sub(1), ("R", "rescan"));
    }
    frame.render_widget(
        Paragraph::new(key_hints(&hints)).block(
            Block::default()
                .borders(Borders::ALL)
                .padding(Padding::new(1, 1, 0, 0)),
        ),
        areas[0],
    );
    let items = rows.iter().map(|row| ListItem::new(row.as_str()));
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" {title} "))
                .borders(Borders::ALL)
                .padding(Padding::new(1, 1, 0, 0)),
        )
        .highlight_style(selection_style().add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, areas[1], state);
}

fn selection_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Blue)
}

fn key_hints<'a>(hints: &[(&'a str, &'a str)]) -> Line<'a> {
    let mut spans = Vec::with_capacity(hints.len() * 4);
    for (index, (key, action)) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("   ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            *key,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(": ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(*action, Style::default().fg(Color::Gray)));
    }
    Line::from(spans)
}

fn is_cancel_key(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | 'Q')) || is_ctrl_c_key(key))
}

fn is_ctrl_c_key(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && matches!(key.code, KeyCode::Char('c' | 'C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSelection {
    Selected(usize),
    Rescan(usize),
    Cancelled,
}

fn select_previous(state: &mut ListState) {
    let current = state.selected().unwrap_or(0);
    state.select(Some(current.saturating_sub(1)));
}

fn select_next(state: &mut ListState, len: usize) {
    let current = state.selected().unwrap_or(0);
    state.select(Some((current + 1).min(len.saturating_sub(1))));
}

#[cfg(test)]
mod tests {
    use super::{
        LoadingActivity, MapActionMode, MapChoice, MapChoiceAction, MapVersionTone,
        PendingRecoveryActions, PendingRecoveryDecision, SelectedMapAction, dashboard_gauge_label,
        elapsed_clock, gauge_label, is_cancel_key, path_style, pending_recovery_dialog_config,
        pending_recovery_result, progress_metrics, screen_footer_area, select_map_cell,
        selected_map_actions, selected_map_choice_count, selected_map_choices,
        update_confirmation_body,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use garmin_progress::{OperationStage, ProgressReporter, ProgressState, ProgressUnit};
    use ratatui::layout::Rect;
    use ratatui::widgets::TableState;
    use std::time::{Duration, Instant};

    use crate::progress_state::{ProgressModel, StageView};

    #[test]
    fn pending_recovery_actions_remain_distinct() {
        use ratatui_interact::traits::{ContainerAction, EventResult};

        assert_eq!(
            pending_recovery_result(&EventResult::Action(ContainerAction::Submit)),
            Some(PendingRecoveryDecision::Recover)
        );
        assert_eq!(
            pending_recovery_result(&EventResult::Action(
                ContainerAction::custom("clear-state",)
            )),
            Some(PendingRecoveryDecision::ClearState)
        );
        assert_eq!(
            pending_recovery_result(&EventResult::Action(ContainerAction::Close)),
            Some(PendingRecoveryDecision::Cancel)
        );
        assert_eq!(
            pending_recovery_result(&EventResult::Action(ContainerAction::custom("discard"))),
            Some(PendingRecoveryDecision::Discard)
        );
    }

    #[test]
    fn host_only_recovery_does_not_offer_clear_state() {
        let host_only = pending_recovery_dialog_config(PendingRecoveryActions::RecoverOnly);
        let portable = pending_recovery_dialog_config(PendingRecoveryActions::RecoverOrClear);
        let unprepared = pending_recovery_dialog_config(PendingRecoveryActions::DiscardOnly);

        assert_eq!(host_only.buttons.len(), 1);
        assert_eq!(host_only.buttons[0].0, "Recover now");
        assert_eq!(portable.buttons.len(), 2);
        assert_eq!(portable.buttons[0].0, "Clear state");
        assert_eq!(unprepared.buttons.len(), 1);
        assert_eq!(unprepared.buttons[0].0, "Discard attempt");
    }

    #[test]
    fn mouse_clicks_choose_keep_change_or_remove() {
        let area = Rect::new(0, 0, 100, 12);
        let mut state = TableState::default().with_selected(Some(0));
        let mut actions = [MapChoiceAction::Keep];
        let choices = [MapChoice {
            name: "Map".to_owned(),
            version_transition: "(1.00) → 2.00".to_owned(),
            version_tone: MapVersionTone::Outdated,
            description: String::new(),
            install_label: "Update",
            can_remove: true,
            cache: None,
        }];

        select_map_cell(
            area,
            90,
            3,
            &mut state,
            &mut actions,
            &choices,
            MapActionMode::Combined,
        );
        assert_eq!(actions[0], MapChoiceAction::Remove);
        select_map_cell(
            area,
            75,
            3,
            &mut state,
            &mut actions,
            &choices,
            MapActionMode::Combined,
        );
        assert_eq!(actions[0], MapChoiceAction::Change);
        select_map_cell(
            area,
            65,
            3,
            &mut state,
            &mut actions,
            &choices,
            MapActionMode::Combined,
        );
        assert_eq!(actions[0], MapChoiceAction::Keep);
    }

    #[test]
    fn unavailable_removal_cell_does_not_change_the_action() {
        let area = Rect::new(0, 0, 100, 12);
        let mut state = TableState::default().with_selected(Some(0));
        let mut actions = [MapChoiceAction::Keep];
        let choices = [MapChoice {
            name: "CourseView".to_owned(),
            version_transition: "Installed 26.20".to_owned(),
            version_tone: MapVersionTone::UpToDate,
            description: String::new(),
            install_label: "Reinstall",
            can_remove: false,
            cache: None,
        }];

        select_map_cell(
            area,
            90,
            3,
            &mut state,
            &mut actions,
            &choices,
            MapActionMode::Combined,
        );

        assert_eq!(actions[0], MapChoiceAction::Keep);
    }

    #[test]
    fn mixed_component_actions_partition_changes_and_removals() {
        let selected = selected_map_choices(&[
            MapChoiceAction::Keep,
            MapChoiceAction::Change,
            MapChoiceAction::Remove,
        ]);

        assert_eq!(selected.changes, [1]);
        assert_eq!(selected.removals, [2]);
    }

    #[test]
    fn selection_count_mentions_removals_only_when_the_column_exists() {
        let selected = super::SelectedMapChoices {
            changes: vec![1],
            removals: Vec::new(),
        };

        assert_eq!(
            selected_map_choice_count(&selected, MapActionMode::ChangeOnly),
            "1 change selected"
        );
        assert_eq!(
            selected_map_choice_count(&selected, MapActionMode::Combined),
            "1 change · 0 removals selected"
        );
    }

    #[test]
    fn selected_actions_align_without_trailing_space() {
        let rendered = selected_map_actions(&[
            SelectedMapAction::new("Base maps", "Update"),
            SelectedMapAction::new("Worldwide map", "Install"),
        ]);

        assert_eq!(rendered, "Update   Base maps\nInstall  Worldwide map");
        assert!(rendered.lines().all(|line| !line.ends_with(' ')));
    }

    #[test]
    fn backup_choice_switches_the_plan_identity_shown_for_confirmation() {
        let mut body = update_confirmation_body(
            "Device",
            &[SelectedMapAction::new("Map", "Update")],
            "verified-plan",
            "1 file",
            "1 GB",
            "1 file",
            "capture",
        )
        .with_backup_plan_ids("verified-plan", "skipped-plan");
        let plan_id = |body: &super::UpdateConfirmationBody| {
            body.details
                .fields
                .iter()
                .find(|field| field.label == "Plan ID")
                .unwrap()
                .value
                .clone()
        };

        assert_eq!(plan_id(&body), "verified-plan");
        body.backup.toggle();
        body.sync_plan_id();
        assert_eq!(plan_id(&body), "skipped-plan");
        assert_eq!(body.confirm_label(), "Continue without backup");
    }

    #[test]
    fn control_c_is_a_terminal_cancel_key() {
        assert!(is_cancel_key(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(!is_cancel_key(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn dialog_hints_span_the_bottom_of_the_terminal() {
        assert_eq!(
            screen_footer_area(Rect::new(4, 3, 100, 30)),
            Rect::new(4, 32, 100, 1)
        );
    }

    #[test]
    fn dialog_hints_are_hidden_for_an_empty_terminal() {
        assert_eq!(
            screen_footer_area(Rect::new(0, 0, 0, 0)),
            Rect::new(0, 0, 0, 0)
        );
    }

    #[test]
    fn loading_elapsed_time_keeps_hours_visible() {
        assert_eq!(elapsed_clock(Duration::from_secs(7)), "00:00:07");
        assert_eq!(elapsed_clock(Duration::from_secs(3_723)), "01:02:03");
    }

    #[test]
    fn device_read_back_shows_rate_elapsed_time_and_eta() {
        let view = StageView {
            state: Some(ProgressState::Advanced),
            label: "Reading MTP object back".to_owned(),
            completed: 64_000_000,
            total: Some(256_000_000),
            elapsed: Some(Duration::from_secs(24)),
            ..StageView::default()
        };

        assert_eq!(
            progress_metrics(OperationStage::DeviceVerify, &view),
            ["avg 2.67 MB/s", "00:00:24 elapsed", "ETA 00:01:12"]
        );
        assert_eq!(
            gauge_label(OperationStage::DeviceVerify, &view),
            "64.00 MB / 256.00 MB · avg 2.67 MB/s · 00:00:24 elapsed · ETA 00:01:12 — Reading MTP object back"
        );
    }

    #[test]
    fn completed_read_back_retains_its_rate_and_elapsed_time() {
        let view = StageView {
            state: Some(ProgressState::Completed),
            label: "Read back and SHA-256 verified".to_owned(),
            completed: 256_000_000,
            total: Some(256_000_000),
            elapsed: Some(Duration::from_secs(96)),
            ..StageView::default()
        };

        assert_eq!(
            gauge_label(OperationStage::DeviceVerify, &view),
            "256.00 MB / 256.00 MB · avg 2.67 MB/s · 00:01:36 elapsed — Read back and SHA-256 verified"
        );
    }

    #[test]
    fn subsecond_byte_sample_does_not_claim_an_impossible_rate_or_eta() {
        let view = StageView {
            state: Some(ProgressState::Advanced),
            unit: ProgressUnit::Bytes,
            label: "Verifying existing device file".to_owned(),
            completed: 1_980_000_000,
            total: Some(16_920_000_000),
            elapsed: Some(Duration::from_nanos(237)),
            ..StageView::default()
        };

        let metrics = progress_metrics(OperationStage::DeviceVerify, &view);

        assert_eq!(metrics, ["0.00s elapsed"]);
        assert!(metrics.iter().all(|metric| !metric.contains("/s")));
        assert!(metrics.iter().all(|metric| !metric.contains("ETA")));
    }

    #[test]
    fn queued_byte_events_use_producer_elapsed_time_for_rate() {
        let recorded_now = std::time::SystemTime::now();
        let view = StageView {
            state: Some(ProgressState::Advanced),
            unit: ProgressUnit::Bytes,
            label: "Verifying existing device file".to_owned(),
            completed: 1_980_000_000,
            total: Some(16_920_000_000),
            started_at: Some(Instant::now()),
            started_recorded_at: recorded_now.checked_sub(Duration::from_secs(20)),
            updated_at: Some(Instant::now()),
            updated_recorded_at: Some(recorded_now),
            ..StageView::default()
        };

        let metrics = progress_metrics(OperationStage::DeviceVerify, &view);

        assert!(metrics[0].starts_with("avg "));
        assert!(!metrics[0].contains("PB/s"));
        assert!(
            metrics
                .iter()
                .any(|metric| metric.contains("00:00:20 elapsed"))
        );
    }

    #[test]
    fn transfer_rate_uses_the_last_byte_sample_instead_of_decaying_with_wall_time() {
        let started = std::time::UNIX_EPOCH + Duration::from_secs(1_000);
        let view = StageView {
            state: Some(ProgressState::Advanced),
            unit: ProgressUnit::Bytes,
            label: "Writing verified file to device".to_owned(),
            completed: 20_000_000,
            total: Some(100_000_000),
            started_recorded_at: Some(started),
            updated_at: Some(Instant::now()),
            updated_recorded_at: Some(started + Duration::from_secs(20)),
            elapsed: Some(Duration::from_secs(100)),
            ..StageView::default()
        };

        assert_eq!(
            progress_metrics(OperationStage::Commit, &view),
            ["avg 1.00 MB/s", "00:01:40 elapsed", "ETA 00:01:20"]
        );
    }

    #[test]
    fn stale_byte_progress_suppresses_decaying_rate_and_rising_eta() {
        let now = Instant::now();
        let view = StageView {
            state: Some(ProgressState::Advanced),
            unit: ProgressUnit::Bytes,
            label: "Writing verified file to device".to_owned(),
            completed: 4_510_000_000,
            total: Some(16_920_000_000),
            started_at: Some(now.checked_sub(Duration::from_secs(1_340)).unwrap()),
            updated_at: Some(now.checked_sub(Duration::from_secs(12)).unwrap()),
            ..StageView::default()
        };

        let metrics = progress_metrics(OperationStage::Commit, &view);

        assert_eq!(metrics[0], "00:22:20 elapsed");
        assert!(metrics[1].starts_with("no progress for 00:00:12"));
        assert!(metrics[1].contains("device finalizing or stalled"));
        assert!(metrics.iter().all(|metric| !metric.contains("/s")));
        assert!(metrics.iter().all(|metric| !metric.contains("ETA")));
    }

    #[test]
    fn stale_aggregate_stage_defers_to_the_active_scoped_file() {
        let (progress, receiver) = ProgressReporter::channel();
        let file = progress.for_item("recovery-write-000001");
        progress.started(
            OperationStage::Commit,
            "Reconciling interrupted update files",
            Some(16_920_000_000),
        );
        file.started_with_path(
            OperationStage::DeviceVerify,
            "Verifying existing device file",
            "Garmin/D9486080A.img",
            Some(3_457_482_752),
        );
        file.advanced_with_path(
            OperationStage::DeviceVerify,
            "Verifying existing device file",
            "Garmin/D9486080A.img",
            1_000_000,
            Some(3_457_482_752),
        );
        let mut model = ProgressModel::new(
            &[OperationStage::Commit, OperationStage::DeviceVerify],
            "Preparing",
        );
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        let now = Instant::now();
        model.stages[0].1.started_at = now.checked_sub(Duration::from_secs(30));
        model.stages[0].1.updated_at = now.checked_sub(Duration::from_secs(12));

        let label = dashboard_gauge_label(OperationStage::Commit, &model.stages[0].1, &model);

        assert!(label.contains("current file progress shown below"));
        assert!(!label.contains("stalled"));
        assert!(!label.contains("ETA"));
    }

    #[test]
    fn completed_subsecond_stage_retains_a_useful_duration() {
        let view = StageView {
            state: Some(ProgressState::Completed),
            unit: ProgressUnit::Operations,
            label: "Device finalized the MTP upload".to_owned(),
            completed: 1,
            total: Some(1),
            elapsed: Some(Duration::from_millis(30)),
            ..StageView::default()
        };

        assert_eq!(
            gauge_label(OperationStage::DeviceFinalize, &view),
            "0.03s elapsed · Device finalized the MTP upload"
        );
    }

    #[test]
    fn commit_progress_uses_the_reported_unit() {
        let bytes = StageView {
            state: Some(ProgressState::Advanced),
            unit: ProgressUnit::Bytes,
            label: "Writing verified file to device".to_owned(),
            completed: 370_532_364,
            total: Some(413_298_576),
            ..StageView::default()
        };
        let operations = StageView {
            state: Some(ProgressState::Advanced),
            unit: ProgressUnit::Operations,
            label: "Installed map file".to_owned(),
            completed: 1,
            total: Some(3),
            ..StageView::default()
        };

        assert_eq!(
            gauge_label(OperationStage::Commit, &bytes),
            "370.53 MB / 413.30 MB — Writing verified file to device"
        );
        assert_eq!(
            gauge_label(OperationStage::Commit, &operations),
            "1 / 3 operations — Installed map file"
        );
    }

    #[test]
    fn verification_waiting_on_downloads_reports_files_without_rate_or_eta() {
        let (progress, receiver) = ProgressReporter::channel();
        let first = progress.for_item("download:0");
        let second = progress.for_item("download:1");
        let third = progress.for_item("download:2");
        progress.started(
            OperationStage::Download,
            "Downloading 3 map files",
            Some(413_298_576),
        );
        progress.started(
            OperationStage::Verify,
            "Waiting to verify downloaded files",
            Some(413_298_576),
        );
        for item in [&first, &second, &third] {
            item.started(OperationStage::Download, "Downloading", Some(100));
        }
        first.completed(OperationStage::Download, "Downloaded", 100, Some(100));
        first.started(OperationStage::Verify, "Checking Garmin MD5", Some(100));
        first.completed(
            OperationStage::Verify,
            "Garmin MD5 verified",
            100,
            Some(100),
        );
        progress.advanced(
            OperationStage::Download,
            "Downloading 3 map files",
            51_810_000,
            Some(413_298_576),
        );
        progress.advanced(
            OperationStage::Verify,
            "Verifying 3 map files",
            23_360,
            Some(413_298_576),
        );
        let mut model = ProgressModel::new(
            &[OperationStage::Download, OperationStage::Verify],
            "Preparing",
        );
        for event in receiver.try_iter() {
            model.apply(&event);
        }
        model.stages[1].1.elapsed = Some(Duration::from_secs(2));
        let verify = &model.stages[1].1;
        let label = dashboard_gauge_label(OperationStage::Verify, verify, &model);

        assert!(label.contains("1/3 files verified"));
        assert!(label.contains("waiting for active downloads"));
        assert!(!label.contains("/s"));
        assert!(!label.contains("ETA"));
    }

    #[test]
    fn device_metadata_loading_styles_the_manifest_as_a_path() {
        let detail = LoadingActivity::DeviceMetadata.detail();

        assert_eq!(detail.spans[1].content, "GarminDevice.xml");
        assert_eq!(detail.spans[1].style, path_style());
    }
}
