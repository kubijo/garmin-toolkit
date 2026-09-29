use super::*;
use garmin_service_api::snapshots::SnapshotOperation;
use garmin_ui::backup::recovery::{self as view, Action as RecoveryAction};

#[derive(Default)]
pub(super) struct Discovery {
    actor: Option<UserId>,
    operations: Vec<SnapshotOperation>,
    pub loading: bool,
    error: Option<String>,
    request: Option<Uuid>,
}

pub(super) fn show(
    ui: &mut eframe::egui::Ui,
    intl: &garmin_i18n::Intl,
    shared: &Rc<RefCell<State>>,
    actor: UserId,
) {
    {
        let state = shared.borrow();
        if state.backup.state.busy()
            || state.backup.file.is_some()
            || state.backup.chooser.is_some()
        {
            return;
        }
    }
    if shared.borrow().backup.recovery.actor != Some(actor) {
        refresh(Rc::clone(shared), ui.ctx().clone(), actor);
    }
    let action = {
        let state = shared.borrow();
        let discovery = &state.backup.recovery;
        if discovery.operations.is_empty() && discovery.error.is_none() && !discovery.loading {
            return;
        }
        let action = view::show(
            ui,
            &view::Props {
                intl,
                operations: &discovery.operations,
                loading: discovery.loading || state.client.is_none(),
                error: discovery.error.as_deref(),
            },
        );
        ui.add_space(24.0);
        action
    };
    match action {
        Some(RecoveryAction::Refresh) => refresh(Rc::clone(shared), ui.ctx().clone(), actor),
        Some(RecoveryAction::Resume(index)) => {
            let entry = shared
                .borrow()
                .backup
                .recovery
                .operations
                .get(index)
                .map(|entry| (entry.status.operation, entry.source.clone()));
            let Some((operation, origin)) = entry else {
                return;
            };
            let label = view::source_label(intl, &origin);
            {
                let mut state = shared.borrow_mut();
                state.backup.file = Some(match &origin {
                    SnapshotSource::Download | SnapshotSource::ServerSave(_) => {
                        SelectedFile::SaveDestination(label)
                    }
                    _ => SelectedFile::RestoreSource(label),
                });
                state.backup.state = ViewState::Starting;
                state.backup.command = None;
            }
            let runner = Runner {
                shared: Rc::clone(shared),
                context: ui.ctx().clone(),
                actor,
                source: None,
                server: None,
                operation: Some(operation),
                origin,
                recover: true,
            };
            spawn_local(runner.run());
        }
        Some(RecoveryAction::Discard(index)) => {
            let operation = shared
                .borrow()
                .backup
                .recovery
                .operations
                .get(index)
                .map(|entry| entry.status.operation);
            if let Some(operation) = operation {
                discard(Rc::clone(shared), ui.ctx().clone(), actor, operation);
            }
        }
        None => {}
    }
}

fn refresh(shared: Rc<RefCell<State>>, context: eframe::egui::Context, actor: UserId) {
    let (client, epoch, request) = {
        let mut state = shared.borrow_mut();
        let Some(client) = state.client.clone() else {
            return;
        };
        let request = Uuid::new_v4();
        state.backup.recovery = Discovery {
            actor: Some(actor),
            loading: true,
            request: Some(request),
            ..Discovery::default()
        };
        (client, state.epoch.clone(), request)
    };
    spawn_local(async move {
        let result = list(&client, actor).await;
        let mut state = shared.borrow_mut();
        if state.epoch != epoch || state.backup.recovery.request != Some(request) {
            return;
        }
        let discovery = &mut state.backup.recovery;
        discovery.loading = false;
        match result {
            Ok(operations) => discovery.operations = operations,
            Err(error) => discovery.error = Some(error),
        }
        context.request_repaint();
    });
}

async fn list(
    app: &ApplicationServiceClient,
    actor: UserId,
) -> Result<Vec<SnapshotOperation>, String> {
    let session = app
        .snapshots(actor, None)
        .await
        .map_err(|error| error.to_string())??;
    match session
        .execute(SnapshotRequest::List)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.message)?
    {
        SnapshotReply::Operations(operations) => Ok(operations),
        _ => Err("snapshot operation list was not returned".into()),
    }
}

fn discard(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    actor: UserId,
    operation: Uuid,
) {
    let (client, epoch, request) = {
        let mut state = shared.borrow_mut();
        let Some(client) = state.client.clone() else {
            return;
        };
        let request = Uuid::new_v4();
        state.backup.recovery.loading = true;
        state.backup.recovery.error = None;
        state.backup.recovery.request = Some(request);
        (client, state.epoch.clone(), request)
    };
    spawn_local(async move {
        let result = discard_operation(&client, actor, operation).await;
        {
            let mut state = shared.borrow_mut();
            if state.epoch != epoch || state.backup.recovery.request != Some(request) {
                return;
            }
            if let Err(error) = result {
                state.backup.recovery.loading = false;
                state.backup.recovery.error = Some(error);
                context.request_repaint();
                return;
            }
        }
        refresh(shared, context, actor);
    });
}

async fn discard_operation(
    app: &ApplicationServiceClient,
    actor: UserId,
    operation: Uuid,
) -> Result<(), String> {
    let session = app
        .snapshots(actor, Some(operation))
        .await
        .map_err(|error| error.to_string())??;
    for request in [
        SnapshotRequest::Recover { operation },
        SnapshotRequest::Cancel { operation },
        SnapshotRequest::Release { operation },
    ] {
        session
            .execute(request)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.message)?;
    }
    Ok(())
}
