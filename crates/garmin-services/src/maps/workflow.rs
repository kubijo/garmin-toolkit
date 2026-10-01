//! Map decisions and execution independent of terminal, native, or RPC presentation.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result, bail};
use garmin_capture::SessionCapture;
use garmin_map_service::OmtClient;
use garmin_model::map::MapCatalog;
use garmin_progress::ProgressReporter;
use garmin_service_api::maps::{Action, Choice, Command, Phase, Plan, Recovery, State};
use garmin_update::{BackupPolicy, RemovalPlan, UpdatePlan};
use uuid::Uuid;

use super::{
    CatalogSource, GarminDownloadAuthorizer, PhysicalTarget, RemovalPlanReport, SimulatedTarget,
    UpdateExecution, UpdateTarget,
    device::{Connection, Connector},
    pending_recovery::{PendingRecoveryKind, PendingRecoveryStore},
    recovery::PendingState,
};

pub struct Settings {
    pub connector: Arc<dyn Connector>,
    pub source: CatalogSource,
    pub cache: PathBuf,
    pub captures: PathBuf,
    pub receipts: PendingRecoveryStore,
    pub concurrency: usize,
    pub simulation_write_bytes_per_second: Option<std::num::NonZeroU64>,
}

enum PreparedPlan {
    Update(Box<ReadyUpdate>),
    Removal(Box<RemovalPlanReport>),
}

struct ReadyUpdate {
    plan: UpdatePlan,
    payloads: super::PreparedUpdatePayloads,
    preflight: garmin_update::MountedMtpPreflight,
}

struct Prepared {
    approval: Uuid,
    plan: PreparedPlan,
    capture: SessionCapture,
    dry_run: bool,
}

pub(super) struct Workflow {
    settings: Settings,
    identity: Option<String>,
    catalog: Option<MapCatalog>,
    prepared: Option<Prepared>,
    history_loaded: bool,
}

impl Workflow {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            identity: None,
            catalog: None,
            prepared: None,
            history_loaded: false,
        }
    }

    async fn connect(&mut self) -> Result<Connection> {
        let connection = self.settings.connector.connect().await?;
        let identity = connection.manifest.identity_digest();
        if self
            .identity
            .as_ref()
            .is_some_and(|expected| *expected != identity)
        {
            bail!("the selected device changed; reopen map management for the new device");
        }
        self.identity = Some(identity);
        Ok(connection)
    }

    fn capture(&self) -> Result<SessionCapture> {
        Ok(SessionCapture::create(
            &self.settings.captures.join(Uuid::new_v4().to_string()),
        )?)
    }

    fn outcomes_root(&self) -> PathBuf {
        self.settings.captures.with_extension("outcomes")
    }

    pub async fn retain_outcomes(&self, state: &State) -> Result<()> {
        let Some(identity) = self.identity.clone() else {
            return Ok(());
        };
        if state.history.is_empty() {
            return Ok(());
        }
        let root = self.outcomes_root();
        let outcomes = state.history.clone();
        tokio::task::spawn_blocking(move || super::outcomes::write(&root, &identity, outcomes))
            .await?
    }

    /// Recovery must be reconciled before any further choice that can cause a mutation.
    pub async fn inspect(&mut self, state: &mut State) -> Result<()> {
        let connection = self.connect().await?;
        if !self.history_loaded {
            let root = self.outcomes_root();
            let identity = connection.manifest.identity_digest();
            state.history =
                tokio::task::spawn_blocking(move || super::outcomes::read(&root, &identity))
                    .await??;
            self.history_loaded = true;
        }
        state.storages = connection.device.state().await?.storages;
        state.storage_error = None;
        let pending = PendingState::inspect(
            connection.device.as_ref(),
            &connection.manifest.identity_digest(),
            &self.settings.receipts,
        )
        .await?;
        state.recovery = pending.as_ref().map(recovery_view);
        state.phase = if state.recovery.is_some() {
            Phase::Recovery
        } else if self.catalog.is_some() {
            Phase::Catalog
        } else {
            Phase::Consent
        };
        state.plan = None;
        self.prepared = None;
        Ok(())
    }

    pub async fn run(
        &mut self,
        command: Command,
        state: &mut State,
        progress: &ProgressReporter,
    ) -> Result<()> {
        if progress.is_cancelled() {
            bail!("map operation cancelled");
        }
        let read_only = matches!(
            command,
            Command::Refresh | Command::ContactService | Command::Review
        );
        let result = match command {
            Command::Refresh => self.inspect(state).await,
            Command::ContactService => self.query(state).await,
            Command::Review => self.review(state, progress).await,
            Command::Approve { approval } => self.execute(approval, state, progress).await,
            Command::Recover | Command::ClearRecovery | Command::DiscardPreparation => {
                self.recover(command, state, progress).await
            }
            _ => bail!("command does not require host execution"),
        };
        if read_only && progress.is_cancelled() {
            bail!("map operation cancelled");
        }
        result
    }

    async fn query(&mut self, state: &mut State) -> Result<()> {
        self.inspect(state).await?;
        if state.recovery.is_some() {
            return Ok(());
        }
        let connection = self.connect().await?;
        let (_, catalog) = self
            .settings
            .source
            .query(&connection.manifest, Some(self.capture()?))
            .await?;
        state.components = self
            .settings
            .source
            .components(
                &catalog,
                &connection.manifest.identity_digest(),
                &self.settings.cache,
            )
            .await?;
        self.catalog = Some(catalog);
        state.phase = Phase::Catalog;
        Ok(())
    }

    async fn review(&mut self, state: &mut State, progress: &ProgressReporter) -> Result<()> {
        self.inspect(state).await?;
        if state.recovery.is_some() {
            return Ok(());
        }
        let connection = self.connect().await?;
        let catalog = self
            .catalog
            .as_ref()
            .context("contact the map service before choosing maps")?;
        let removed = selected(state, Choice::Remove);
        let installed = selected(state, Choice::Install);
        if removed.is_empty() && installed.is_empty() {
            bail!("select at least one component");
        }
        let capture = self.capture()?;
        capture
            .write_bytes(
                Path::new("device/GarminDevice.xml"),
                connection.manifest.raw_xml().as_bytes(),
            )
            .await?;
        let plan = if removed.is_empty() {
            let policy = if state.verified_backup {
                BackupPolicy::Verified
            } else {
                BackupPolicy::Skip
            };
            let plan = self
                .settings
                .source
                .plan(catalog, connection.manifest.identity_digest(), installed)?
                .with_backup_policy(policy)?;
            capture.write_json(Path::new("plan.json"), &plan).await?;
            PreparedPlan::Update(Box::new(
                self.prepare_update(connection, plan, &capture, progress)
                    .await?,
            ))
        } else {
            if state.dry_run {
                bail!("removal simulation is not available; deselect removals for a dry run");
            }
            let plan = RemovalPlan::from_response_selection(
                catalog,
                connection.manifest.identity_digest(),
                removed,
            )?;
            let inventory = connection
                .device
                .inventory(&plan.paths_to_inventory())
                .await?;
            PreparedPlan::Removal(Box::new(
                super::prepare_removal(plan, &inventory, Some(&capture)).await?,
            ))
        };
        let prepared = Prepared {
            approval: Uuid::new_v4(),
            plan,
            capture,
            dry_run: state.dry_run,
        };
        state.plan = Some(prepared.preview(state)?);
        state.phase = Phase::Review;
        self.prepared = Some(prepared);
        Ok(())
    }

    async fn prepare_update(
        &self,
        connection: Connection,
        plan: UpdatePlan,
        capture: &SessionCapture,
        progress: &ProgressReporter,
    ) -> Result<ReadyUpdate> {
        let evidence = capture.clone();
        let execution = UpdateExecution {
            client: self
                .settings
                .source
                .client()?
                .with_capture(Some(capture.clone())),
            manifest: connection.manifest,
            plan,
            device: Box::new(PhysicalTarget(connection.device)),
            cache: self.settings.cache.clone(),
            concurrency: self.settings.concurrency,
            download_authorizer: Arc::new(GarminDownloadAuthorizer),
            progress: progress.observe(move |event| {
                let _ = evidence.append_event(event);
            }),
            capture: capture.clone(),
            prepared: None,
        };
        let payloads = execution.prepare_payloads().await?;
        let device = execution
            .device
            .writable_device()
            .context("preflight requires a device transport")?;
        let preflight = garmin_update::preflight_mounted_mtp_update(
            &execution.plan,
            &payloads.downloads,
            &payloads.authorization,
            device,
            capture,
            &execution.progress,
        )
        .await?;
        Ok(ReadyUpdate {
            plan: execution.plan,
            payloads,
            preflight,
        })
    }

    async fn execute(
        &mut self,
        approval: Uuid,
        state: &mut State,
        progress: &ProgressReporter,
    ) -> Result<()> {
        let prepared = self
            .prepared
            .take()
            .context("no plan is awaiting approval")?;
        if approval != prepared.approval {
            bail!("approval does not match the prepared plan");
        }
        let connection = self.connect().await?;
        if PendingState::inspect(
            connection.device.as_ref(),
            &connection.manifest.identity_digest(),
            &self.settings.receipts,
        )
        .await?
        .is_some()
        {
            bail!("pending recovery must be resolved before execution");
        }
        let removal = matches!(prepared.plan, PreparedPlan::Removal(_));
        match prepared.plan {
            PreparedPlan::Update(ready) => {
                let ReadyUpdate { plan, payloads, .. } = *ready;
                let client: OmtClient = self
                    .settings
                    .source
                    .client()?
                    .with_capture(Some(prepared.capture.clone()));
                let device: Box<dyn UpdateTarget> = if prepared.dry_run {
                    Box::new(SimulatedTarget {
                        source: connection.device,
                        fixture: None,
                        write_bytes_per_second: self.settings.simulation_write_bytes_per_second,
                    })
                } else {
                    Box::new(PhysicalTarget(connection.device))
                };
                super::execute_registered_update(
                    UpdateExecution {
                        prepared: Some(payloads),
                        client,
                        manifest: connection.manifest,
                        plan,
                        device,
                        cache: self.settings.cache.clone(),
                        concurrency: self.settings.concurrency,
                        download_authorizer: Arc::new(GarminDownloadAuthorizer),
                        progress: progress.clone(),
                        capture: prepared.capture,
                    },
                    Some(&self.settings.receipts),
                )
                .await?;
            }
            PreparedPlan::Removal(plan) => {
                super::execute_removal(
                    &plan.execution_plan,
                    &prepared.capture,
                    progress,
                    connection.device.as_ref(),
                    &self.settings.receipts,
                )
                .await?;
            }
        }
        state.plan = None;
        for component in &mut state.components {
            if component.choice
                == if removal {
                    Choice::Remove
                } else {
                    Choice::Install
                }
            {
                component.choice = Choice::Keep;
            }
        }
        if removal
            && state
                .components
                .iter()
                .any(|component| component.choice == Choice::Install)
        {
            state.history.push(garmin_service_api::maps::Outcome {
                id: Uuid::new_v4(),
                phase: Phase::Completed,
                message: "Removal completed".to_owned(),
            });
            if state.history.len() > super::outcomes::MAX_OUTCOMES {
                state.history.remove(0);
            }
            self.retain_outcomes(state).await?;
            self.review(state, progress).await?;
        } else {
            state.phase = Phase::Completed;
        }
        Ok(())
    }

    async fn recover(
        &mut self,
        command: Command,
        state: &mut State,
        progress: &ProgressReporter,
    ) -> Result<()> {
        let connection = self.connect().await?;
        let identity = connection.manifest.identity_digest();
        let pending = PendingState::inspect(
            connection.device.as_ref(),
            &identity,
            &self.settings.receipts,
        )
        .await?
        .context("the pending transaction is no longer present; refresh its state")?;
        if state
            .recovery
            .as_ref()
            .is_none_or(|view| view.plan != pending.plan)
        {
            bail!("pending transaction changed; review its state again");
        }
        match command {
            Command::Recover => {
                pending
                    .recover(
                        &identity,
                        connection.device.as_ref(),
                        progress,
                        &self.settings.receipts,
                    )
                    .await?;
            }
            Command::ClearRecovery => {
                pending
                    .prove_and_clear(connection.device.as_ref(), &self.settings.receipts)
                    .await?;
            }
            Command::DiscardPreparation => pending.discard(&self.settings.receipts)?,
            _ => unreachable!("recovery dispatch accepts recovery commands"),
        }
        self.inspect(state).await
    }
}

fn selected(state: &State, choice: Choice) -> Vec<usize> {
    state
        .components
        .iter()
        .filter(|item| item.choice == choice)
        .map(|item| item.index as usize)
        .collect()
}

fn recovery_view(pending: &PendingState) -> Recovery {
    let mut actions = Vec::new();
    if pending.receipt.is_some() && pending.prepared {
        actions.push(Action::Recover);
    }
    if pending.transaction.is_some() {
        actions.push(Action::ClearRecovery);
    }
    if pending.receipt.is_some() && !pending.prepared && pending.transaction.is_none() {
        actions.push(Action::DiscardPreparation);
    }
    Recovery {
        plan: pending.plan.clone(),
        removal: pending.kind == PendingRecoveryKind::Removal,
        simulated: pending.simulated,
        actions,
    }
}

impl Prepared {
    fn preview(&self, state: &State) -> Result<Plan> {
        let removal = matches!(self.plan, PreparedPlan::Removal(_));
        let components = state
            .components
            .iter()
            .filter(|item| {
                item.choice
                    == if removal {
                        Choice::Remove
                    } else {
                        Choice::Install
                    }
            })
            .map(|item| item.name.clone())
            .collect();
        let mut preview = Plan {
            approval: self.approval,
            digest: String::new(),
            removal,
            components,
            download_bytes: 0,
            write_count: 0,
            remove_count: 0,
            remove_bytes: 0,
            paths: Vec::new(),
            storage_requirements: Vec::new(),
        };
        match &self.plan {
            PreparedPlan::Update(ready) => {
                let plan = &ready.plan;
                preview.digest.clone_from(&plan.digest);
                preview.download_bytes = plan.total_bytes;
                preview.write_count = u32::try_from(ready.preflight.files_to_write)?;
                preview.remove_count = u32::try_from(ready.preflight.files_to_remove)?;
                preview.storage_requirements = ready
                    .preflight
                    .storage_requirements
                    .iter()
                    .map(|requirement| garmin_service_api::maps::StorageRequirement {
                        storage: requirement.storage_id.clone(),
                        required_free_bytes: requirement.required_free_bytes,
                    })
                    .collect();
                preview.paths = plan
                    .downloads
                    .iter()
                    .map(|file| file.destination.to_string())
                    .chain(plan.files_to_remove.iter().map(ToString::to_string))
                    .collect();
            }
            PreparedPlan::Removal(report) => {
                let plan = &report.execution_plan;
                preview.digest.clone_from(&plan.digest);
                preview.remove_count = u32::try_from(plan.files_to_remove.len())?;
                preview.remove_bytes = plan.bytes_to_remove;
                preview.paths = plan
                    .files_to_remove
                    .iter()
                    .map(|file| file.path.to_string())
                    .collect();
            }
        }
        Ok(preview)
    }
}
