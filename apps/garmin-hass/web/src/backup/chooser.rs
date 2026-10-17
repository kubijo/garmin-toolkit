//! The parent tab owns filesystem requests and accepts a selection from its popup.
use super::{
    Controller, Operation, Rc, RefCell, Runner, SelectedFile, Selection, SnapshotSource, State,
    UserId, Uuid, ViewState, spawn_local,
};
use crate::window::BrowserWindow;
use eframe::egui::Context;
use garmin_i18n::{Intl, format_message};
use garmin_service_api::{ApplicationService as _, files::Directory};
use garmin_ui::{
    device_browser::chooser::{self as view, Action},
    window::{Event, Spec, WindowHost as _},
};
use protocol::{Command, Identity, Request, Snapshot};

pub mod popup;
mod protocol;

pub(super) struct Host {
    window: BrowserWindow<Command, Snapshot>,
    actor: UserId,
    token: String,
    operation: Operation,
    request: Uuid,
    directory: Option<Result<Directory, String>>,
    busy: bool,
    inline: Option<view::Chooser>,
    applied_revision: Option<Uuid>,
    view: Option<view::ViewState>,
}

pub(super) fn open(
    shared: Rc<RefCell<State>>,
    context: Context,
    actor: UserId,
    operation: Operation,
    intl: &Intl,
) {
    {
        let mut state = shared.borrow_mut();
        if state.backup.state.busy() || state.client.is_none() || state.backup.recovery.loading {
            return;
        }
        if state
            .backup
            .chooser
            .as_ref()
            .is_some_and(|host| host.actor != actor || host.operation != operation)
        {
            close(&mut state.backup, &context);
        }
        let inline = state
            .profiles
            .iter()
            .find(|profile| profile.user.id() == actor)
            .is_some_and(|profile| profile.user.profile().preferences().inline_file_windows());
        let host = state.backup.chooser.get_or_insert_with(|| Host {
            window: BrowserWindow::default(),
            actor,
            token: Uuid::new_v4().to_string(),
            operation,
            request: Uuid::new_v4(),
            directory: None,
            busy: false,
            inline: inline.then(|| {
                view::Chooser::new(
                    operation,
                    if operation == Operation::Save {
                        "garmin-backup.tar.zst".into()
                    } else {
                        String::new()
                    },
                )
            }),
            applied_revision: None,
            view: None,
        });
        // Launch from the initiating click, before any filesystem await.
        // BrowserWindow retains popup-blocking errors and offers retry/open-in-tab.
        let spec = Spec {
            id: format!("backup-chooser-{}", host.token),
            kind: "backup-chooser".into(),
            title: view::title(intl, operation),
            size: [880.0, 560.0],
        };
        if host.inline.is_some() {
            if !host.window.is_open() {
                host.window.open_inline(&context, spec);
            }
        } else {
            let _ = host.window.open(&context, spec);
        }
        if host.directory.is_some() || host.busy {
            return;
        }
    }
    load_directory(shared, context, "/".into());
}

pub(super) fn close(controller: &mut Controller, context: &Context) {
    if let Some(mut host) = controller.chooser.take() {
        host.window.close(context);
    }
}

fn snapshot(state: &State, context: &Context) -> Option<Snapshot> {
    let host = state.backup.chooser.as_ref()?;
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.user.id() == host.actor);
    Some(Snapshot {
        identity: Identity {
            profile: host.actor,
            generation: state.connection_generation,
            chooser: host.token.clone(),
        },
        operation: host.operation,
        revision: host.request.to_string(),
        directory: host.directory.clone(),
        available: state.client.is_some()
            && profile
                .is_some_and(|profile| profile.user.role() == garmin_model::identity::Role::Owner),
        busy: host.busy || state.preferences_saving || state.backup.state.busy(),
        preferences: profile
            .map(|profile| profile.user.profile().preferences())
            .unwrap_or_default(),
        dark: context.global_style().visuals.dark_mode,
        view: host.view.clone(),
    })
}

pub(super) fn update(
    intl: &Intl,
    shared: &Rc<RefCell<State>>,
    context: &Context,
    owner: Option<UserId>,
) {
    let should_detach = {
        let state = shared.borrow();
        state.backup.chooser.as_ref().is_some_and(|host| {
            host.inline.is_some()
                && Some(host.actor) == owner
                && state
                    .profiles
                    .iter()
                    .find(|profile| profile.user.id() == host.actor)
                    .is_some_and(|profile| {
                        !profile.user.profile().preferences().inline_file_windows()
                    })
        })
    };
    if should_detach {
        detach(shared, context, intl);
    }
    let mut window = {
        let mut state = shared.borrow_mut();
        let Some(host) = &mut state.backup.chooser else {
            return;
        };
        if Some(host.actor) != owner {
            close(&mut state.backup, context);
            return;
        }
        std::mem::take(&mut host.window)
    };
    let events = window.present(
        context,
        intl,
        || snapshot(&shared.borrow(), context).expect("open chooser"),
        |ui| {
            let snapshot = snapshot(&shared.borrow(), context)?;
            let mut state = shared.borrow_mut();
            let host = state.backup.chooser.as_mut()?;
            let chooser = host.inline.as_mut()?;
            chooser.set_show_hidden_files(snapshot.preferences.show_hidden_files());
            if !host.busy
                && host.applied_revision != Some(host.request)
                && let Some(directory) = &host.directory
            {
                match directory {
                    Ok(directory) => chooser.loaded(directory.clone()),
                    Err(error) => chooser.failed(error.clone()),
                }
                host.applied_revision = Some(host.request);
            }
            ui.add_enabled_ui(snapshot.available, |ui| {
                chooser.contents(ui, intl, snapshot.busy)
            })
            .inner
            .map(|action| Command {
                identity: snapshot.identity,
                revision: snapshot.revision,
                action: Request::Action(action),
            })
        },
    );
    if !window.is_open() {
        close(&mut shared.borrow_mut().backup, context);
        return;
    }
    if let Some(host) = &mut shared.borrow_mut().backup.chooser {
        host.window = window;
    }
    for event in events {
        match event {
            Event::Closed => close(&mut shared.borrow_mut().backup, context),
            Event::Command { id, command } => {
                let result = snapshot(&shared.borrow(), context)
                    .ok_or_else(|| "The chooser has closed.".into())
                    .and_then(|snapshot| command.validate(&snapshot));
                if let Some(host) = &mut shared.borrow_mut().backup.chooser {
                    host.window.reply(id, result.as_ref().err().cloned());
                }
                if result.is_ok() {
                    match command.action {
                        Request::Action(request) => {
                            action(intl, Rc::clone(shared), context.clone(), request);
                        }
                        Request::Embed(view) => embed(shared, context, intl, *view),
                    }
                }
            }
        }
    }
    if shared.borrow().backup.chooser.is_some() {
        context.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

fn spec(host: &Host, intl: &Intl) -> Spec {
    Spec {
        id: format!("backup-chooser-{}", host.token),
        kind: "backup-chooser".into(),
        title: view::title(intl, host.operation),
        size: [880.0, 560.0],
    }
}

fn embed(shared: &Rc<RefCell<State>>, context: &Context, intl: &Intl, view: view::ViewState) {
    let Some(snapshot) = snapshot(&shared.borrow(), context) else {
        return;
    };
    if !snapshot.preferences.inline_file_windows() {
        return;
    }
    let mut state = shared.borrow_mut();
    let Some(host) = &mut state.backup.chooser else {
        return;
    };
    let mut chooser = view::Chooser::new(host.operation, String::new());
    if let Some(Ok(directory)) = &host.directory {
        chooser.loaded(directory.clone());
    }
    chooser.restore_view(view);
    host.inline = Some(chooser);
    host.applied_revision = (!host.busy).then_some(host.request);
    host.window.open_inline(context, spec(host, intl));
}

pub(crate) fn detach(shared: &Rc<RefCell<State>>, context: &Context, intl: &Intl) {
    let mut state = shared.borrow_mut();
    let Some(host) = &mut state.backup.chooser else {
        return;
    };
    if let Some(chooser) = host.inline.take() {
        host.view = Some(chooser.view_state());
        let spec = spec(host, intl);
        let _ = host.window.open(context, spec);
    }
}

fn load_directory(shared: Rc<RefCell<State>>, context: Context, path: String) {
    let (client, actor, request, epoch, generation) = {
        let mut state = shared.borrow_mut();
        let Some(client) = state.client.clone() else {
            return;
        };
        let epoch = state.epoch.clone();
        let generation = state.connection_generation;
        let Some(host) = &mut state.backup.chooser else {
            return;
        };
        host.busy = true;
        host.request = Uuid::new_v4();
        (client, host.actor, host.request, epoch, generation)
    };
    spawn_local(async move {
        let result = client
            .server_directory(actor, path)
            .await
            .map_err(|error| error.to_string())
            .and_then(std::convert::identity);
        let mut state = shared.borrow_mut();
        if state.epoch != epoch {
            return;
        }
        let disconnected = state.connection_generation != generation;
        if let Some(host) = &mut state.backup.chooser
            && host.request == request
        {
            host.busy = false;
            host.directory = Some(if disconnected {
                Err("Connection changed. Refresh the directory.".into())
            } else {
                result
            });
        }
        context.request_repaint();
    });
}

fn action(intl: &Intl, shared: Rc<RefCell<State>>, context: Context, action: Action) {
    match action {
        Action::ShowHiddenFiles(show) => {
            let profile = {
                let state = shared.borrow();
                state.backup.chooser.as_ref().and_then(|host| {
                    state
                        .profiles
                        .iter()
                        .find(|profile| profile.user.id() == host.actor)
                        .map(|profile| (profile.user.id(), profile.user.profile().preferences()))
                })
            };
            if let Some((actor, previous)) = profile {
                crate::spawn_update_preferences(
                    shared,
                    context,
                    actor,
                    previous,
                    previous.with_show_hidden_files(show),
                    format_message!(intl, default_message: "Could not save profile settings"),
                );
            }
        }
        Action::Cancel => close(&mut shared.borrow_mut().backup, &context),
        Action::Navigate(path) => load_directory(shared, context, path),
        Action::Select { path, replace } => {
            let (actor, selection) = {
                let mut state = shared.borrow_mut();
                let Some(host) = state.backup.chooser.as_ref() else {
                    return;
                };
                let actor = host.actor;
                let selection = Selection {
                    path,
                    replace,
                    operation: host.operation,
                };
                let label = format_message!(intl, default_message: "Server: {path}", values: { path: selection.path.as_str() });
                state.backup.file = Some(match selection.operation {
                    Operation::Open => SelectedFile::RestoreSource(label),
                    Operation::Save => SelectedFile::SaveDestination(label),
                });
                close(&mut state.backup, &context);
                state.backup.state = ViewState::Starting;
                state.backup.command = None;
                (actor, selection)
            };
            let origin = match selection.operation {
                Operation::Save => SnapshotSource::ServerSave(selection.path.clone()),
                Operation::Open => SnapshotSource::ServerRestore(selection.path.clone()),
            };
            spawn_local(
                Runner {
                    shared,
                    context,
                    actor,
                    source: None,
                    server: Some(selection),
                    operation: None,
                    origin,
                    recover: false,
                    restore_facts: None,
                    restore_started: None,
                }
                .run(),
            );
        }
    }
}
