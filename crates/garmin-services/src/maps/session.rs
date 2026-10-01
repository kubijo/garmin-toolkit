//! Host-owned jobs with revision-bound choices and retry-safe commands.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use garmin_progress::{CancellationToken, ProgressReceiver, ProgressReporter};
use garmin_service_api::maps::{
    Action, Choice, Command, Failure, FailureKind, MapService, Outcome, Phase, Request, State,
};
use remoc::{rch, rtc};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use super::{MutationLocks, Settings, workflow::Workflow};

mod observation;

const RETAINED_REQUESTS: usize = 1_024;
use super::outcomes::MAX_OUTCOMES as RETAINED_OUTCOMES;

struct Receipt {
    id: Uuid,
    revision: Uuid,
    command: Command,
    reply: State,
}

struct Control {
    state: State,
    receipts: VecDeque<Receipt>,
    cancellation: Option<CancellationToken>,
}

struct Inner {
    control: Mutex<Control>,
    workflow: AsyncMutex<Workflow>,
    snapshots: rch::watch::Sender<State>,
    mutations: Arc<MutationLocks>,
    deployment: Option<(Arc<crate::deployment::Deployment>, Uuid)>,
}

/// A local service connection and an RPC implementation of the same commands.
/// The host registry retains it independently of client connections or profiles.
#[derive(Clone)]
pub struct Session(Arc<Inner>);

impl Session {
    /// Create a session in a running host runtime
    /// and reconcile recovery evidence.
    #[must_use]
    pub(super) fn start(
        device: String,
        settings: Settings,
        mutations: Arc<MutationLocks>,
        deployment: Option<(Arc<crate::deployment::Deployment>, Uuid)>,
    ) -> Self {
        let state = State {
            id: Uuid::new_v4(),
            revision: Uuid::new_v4(),
            device,
            service: match &settings.source {
                super::CatalogSource::Garmin => garmin_service_api::maps::CatalogService::Garmin,
                super::CatalogSource::Loopback(_) => {
                    garmin_service_api::maps::CatalogService::Simulation
                }
            },
            phase: Phase::Loading,
            components: Vec::new(),
            verified_backup: true,
            dry_run: false,
            actions: Vec::new(),
            storages: Vec::new(),
            storage_error: None,
            plan: None,
            recovery: None,
            progress: Vec::new(),
            active: Vec::new(),
            events: Vec::new(),
            history: Vec::new(),
            error: None,
        };
        let (snapshots, _) = rch::watch::channel(state.clone());
        let session = Self(Arc::new(Inner {
            control: Mutex::new(Control {
                state: state.clone(),
                receipts: VecDeque::new(),
                cancellation: None,
            }),
            workflow: AsyncMutex::new(Workflow::new(settings)),
            snapshots,
            mutations,
            deployment,
        }));
        session.launch(Command::Refresh, state, ProgressReporter::default(), None);
        session
    }

    #[must_use]
    pub fn snapshot(&self) -> State {
        self.0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .state
            .clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> rch::watch::Receiver<State> {
        self.0.snapshots.subscribe()
    }

    /// Apply a choice exactly once.
    /// Observation updates never invalidate a control revision.
    ///
    /// # Errors
    /// Stale revision, reused request ID, unavailable command, invalid choice, or busy device.
    pub fn submit(&self, request: Request) -> Result<State, Failure> {
        let Request::Change {
            request,
            revision,
            command,
        } = request
        else {
            return Ok(self.snapshot());
        };
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(receipt) = control
            .receipts
            .iter()
            .find(|receipt| receipt.id == request)
        {
            return if receipt.revision == revision && receipt.command == command {
                Ok(receipt.reply.clone())
            } else {
                Err(failure(
                    &control.state,
                    FailureKind::ReusedRequest,
                    "request ID was already used for another choice",
                ))
            };
        }
        if control.state.revision != revision {
            return Err(failure(
                &control.state,
                FailureKind::StaleRevision,
                "the workflow changed; review the current state",
            ));
        }
        let action = action(&command);
        if !control.state.actions.contains(&action) {
            return Err(failure(
                &control.state,
                FailureKind::Unavailable,
                "this action is not available in the current state",
            ));
        }
        let pending = self.apply(&mut control, &command)?;
        control.state.revision = Uuid::new_v4();
        control.state.actions = actions(&control.state);
        let reply = control.state.clone();
        control.receipts.push_back(Receipt {
            id: request,
            revision,
            command: command.clone(),
            reply: reply.clone(),
        });
        if control.receipts.len() > RETAINED_REQUESTS {
            control.receipts.pop_front();
        }
        self.0.snapshots.send_replace(reply.clone());
        if let Some((reporter, receiver, guard)) = pending {
            self.launch(command, reply.clone(), reporter, Some((receiver, guard)));
        }
        Ok(reply)
    }

    fn apply(&self, control: &mut Control, command: &Command) -> Result<Option<Pending>, Failure> {
        let state = &mut control.state;
        match command {
            Command::Choose { component, choice } => {
                let position = state
                    .components
                    .iter()
                    .position(|item| item.index == *component)
                    .ok_or_else(|| {
                        failure(state, FailureKind::InvalidChoice, "unknown component")
                    })?;
                let item = &mut state.components[position];
                if (*choice == Choice::Install && !item.can_install)
                    || (*choice == Choice::Remove && (!item.can_remove || state.dry_run))
                {
                    return Err(failure(
                        state,
                        FailureKind::InvalidChoice,
                        "component does not support this action",
                    ));
                }
                item.choice = *choice;
            }
            Command::VerifiedBackup(value) => state.verified_backup = *value,
            Command::DryRun(true)
                if state
                    .components
                    .iter()
                    .any(|item| item.choice == Choice::Remove) =>
            {
                return Err(failure(
                    state,
                    FailureKind::InvalidChoice,
                    "removal simulation is unavailable; remove the removal selections first",
                ));
            }
            Command::DryRun(value) => state.dry_run = *value,
            Command::Back => {
                state.phase = Phase::Catalog;
                state.plan = None;
            }
            Command::Cancel => {
                control
                    .cancellation
                    .as_ref()
                    .ok_or_else(|| failure(state, FailureKind::Unavailable, "no job is running"))?
                    .cancel();
            }
            Command::Approve { approval }
                if state
                    .plan
                    .as_ref()
                    .is_none_or(|plan| plan.approval != *approval) =>
            {
                return Err(failure(
                    state,
                    FailureKind::InvalidChoice,
                    "approval does not match the displayed plan",
                ));
            }
            _ => {
                // Reads also hold the gate so a browser write cannot invalidate their inventory mid-plan.
                let guard = self
                    .0
                    .mutations
                    .acquire(&state.device)
                    .map_err(|message| failure(state, FailureKind::Busy, message))?;
                let (reporter, receiver) = ProgressReporter::channel();
                control.cancellation = Some(reporter.cancellation_token());
                state.phase = if matches!(
                    command,
                    Command::Approve { .. } | Command::Recover | Command::ClearRecovery
                ) {
                    Phase::Running
                } else {
                    Phase::Loading
                };
                state.error = None;
                state.progress.clear();
                state.active.clear();
                state.events.clear();
                return Ok(Some((reporter, receiver, guard)));
            }
        }
        Ok(None)
    }

    fn launch(
        &self,
        command: Command,
        mut state: State,
        reporter: ProgressReporter,
        pending: Option<(ProgressReceiver, tokio::sync::OwnedMutexGuard<()>)>,
    ) {
        let session = self.clone();
        tokio::spawn(async move {
            let _lease = if let Some((deployment, epoch)) = &session.0.deployment {
                match deployment.application(*epoch).await {
                    Ok(lease) => Some(lease),
                    Err(error) => {
                        session.finish(state, Err(error.into()), false, None).await;
                        return;
                    }
                }
            } else {
                None
            };
            let _initial_guard = if pending.is_none() {
                match session.0.mutations.acquire(&state.device) {
                    Ok(guard) => Some(guard),
                    Err(message) => {
                        session
                            .finish(state, Err(anyhow::anyhow!(message)), false, None)
                            .await;
                        return;
                    }
                }
            } else {
                None
            };
            let mut workflow = session.0.workflow.lock().await;
            let mut observation = observation::Observation::new();
            let result = {
                let task = workflow.run(command, &mut state, &reporter);
                tokio::pin!(task);
                loop {
                    tokio::select! {
                        result = &mut task => break result,
                        () = tokio::time::sleep(Duration::from_millis(100)) => {
                            if let Some((receiver, _)) = &pending {
                                session.observe(&mut observation, receiver);
                            }
                        }
                    }
                }
            };
            if let Some((receiver, _)) = &pending {
                session.observe(&mut observation, receiver);
            }
            let result = match result {
                Err(error) => {
                    if let Err(inspection) = workflow.inspect(&mut state).await {
                        Err(error.context(format!(
                            "could not reconcile recovery state: {inspection:#}"
                        )))
                    } else {
                        Err(error)
                    }
                }
                result => result,
            };
            session
                .finish(state, result, reporter.is_cancelled(), Some(&workflow))
                .await;
        });
    }

    fn observe(&self, observation: &mut observation::Observation, receiver: &ProgressReceiver) {
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        observation.update(&mut control.state, receiver);
        self.0.snapshots.send_replace(control.state.clone());
    }

    async fn finish(
        &self,
        mut state: State,
        result: anyhow::Result<()>,
        cancelled: bool,
        workflow: Option<&Workflow>,
    ) {
        {
            let control = self
                .0
                .control
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.progress.clone_from(&control.state.progress);
            state.active.clone_from(&control.state.active);
            state.events.clone_from(&control.state.events);
            if !control.state.progress.is_empty() {
                state.storages.clone_from(&control.state.storages);
                state.storage_error.clone_from(&control.state.storage_error);
            }
        }
        if let Err(error) = result {
            state.phase = if cancelled {
                Phase::Cancelled
            } else {
                Phase::Failed
            };
            state.error = Some(format!("{error:#}"));
            state.plan = None;
        }
        if matches!(
            state.phase,
            Phase::Completed | Phase::Cancelled | Phase::Failed
        ) {
            state.history.push(Outcome {
                id: Uuid::new_v4(),
                phase: state.phase,
                message: state
                    .error
                    .clone()
                    .unwrap_or_else(|| "Completed".to_owned()),
            });
            if state.history.len() > RETAINED_OUTCOMES {
                state.history.remove(0);
            }
        }
        if state.recovery.is_some() {
            state.phase = Phase::Recovery;
        }
        state.revision = Uuid::new_v4();
        state.actions = actions(&state);
        if let Some(workflow) = workflow
            && let Err(error) = workflow.retain_outcomes(&state).await
        {
            let message = format!("Could not retain the operation outcome: {error:#}");
            state.error = Some(
                state
                    .error
                    .map_or(message.clone(), |original| format!("{original}; {message}")),
            );
        }
        let mut control = self
            .0
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        control.cancellation = None;
        control.state = state.clone();
        self.0.snapshots.send_replace(state);
    }
}

type Pending = (
    ProgressReporter,
    ProgressReceiver,
    tokio::sync::OwnedMutexGuard<()>,
);

fn failure(state: &State, kind: FailureKind, message: &str) -> Failure {
    Failure {
        kind,
        message: message.to_owned(),
        revision: state.revision,
    }
}

fn action(command: &Command) -> Action {
    match command {
        Command::ContactService => Action::ContactService,
        Command::Choose { .. } | Command::VerifiedBackup(_) | Command::DryRun(_) => Action::Choose,
        Command::Review => Action::Review,
        Command::Approve { .. } => Action::Approve,
        Command::Back => Action::Back,
        Command::Cancel => Action::Cancel,
        Command::Recover => Action::Recover,
        Command::ClearRecovery => Action::ClearRecovery,
        Command::DiscardPreparation => Action::DiscardPreparation,
        Command::Refresh => Action::Refresh,
    }
}

fn actions(state: &State) -> Vec<Action> {
    match state.phase {
        Phase::Consent => vec![Action::ContactService, Action::Refresh],
        Phase::Loading | Phase::Running => vec![Action::Cancel],
        Phase::Catalog => vec![
            Action::Choose,
            Action::Review,
            Action::ContactService,
            Action::Refresh,
        ],
        Phase::Review => vec![Action::Approve, Action::Back, Action::Refresh],
        Phase::Recovery => state.recovery.as_ref().map_or_else(Vec::new, |recovery| {
            let mut actions = recovery.actions.clone();
            actions.push(Action::Refresh);
            actions
        }),
        Phase::Completed | Phase::Failed | Phase::Cancelled => vec![Action::Refresh],
    }
}

impl MapService for Session {
    fn request(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<Result<State, Failure>, rtc::CallError>> {
        std::future::ready(Ok(self.submit(request)))
    }
    fn watch(&self) -> impl Future<Output = Result<rch::watch::Receiver<State>, rtc::CallError>> {
        std::future::ready(Ok(self.subscribe()))
    }
}
