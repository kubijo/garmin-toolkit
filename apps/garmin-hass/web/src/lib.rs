#![cfg(target_arch = "wasm32")]

use bytes::Bytes;
use eframe::egui::{Id, Ui};
use futures_util::{SinkExt as _, StreamExt as _, future};
use garmin_i18n::{Intl, Language, Translations, format_message};
use garmin_model::identity::{LanguagePreference, ProfilePreferences, ThemePreference, UserId};
use garmin_service_api::{
    ActivityDetailSnapshot, ApplicationService, ApplicationServiceClient, DeploymentMode,
    DeviceSnapshot, ProfileSnapshot,
};
use garmin_ui::{
    activity, device, notification, offline, path, profile, profile_settings, progress, shell,
    workspace::{self, Page},
};
use remoc::prelude::*;
use std::{cell::RefCell, fmt, io, rc::Rc, time::Duration};
use wasm_bindgen::{JsCast as _, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use websocket_web::{Msg, WebSocket};

const CANVAS_ID: &str = "garmin-toolkit";
const HEARTBEAT_INTERVAL_MILLISECONDS: i32 = 3_000;
const HEARTBEAT_TIMEOUT_MILLISECONDS: i32 = 5_000;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    let canvas = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(CANVAS_ID))
        .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| js_error("the application canvas is missing"))?;

    spawn_local(async move {
        let result = eframe::WebRunner::new()
            .start(
                canvas.clone(),
                eframe::WebOptions::default(),
                Box::new(|creation| {
                    garmin_ui::install(&creation.egui_ctx);
                    Ok(Box::new(App::new(creation.egui_ctx.clone())?))
                }),
            )
            .await;
        match result {
            Ok(()) => identify_text_agent(&canvas),
            Err(error) => show_startup_error(&format!(
                "Could not start Garmin Toolkit: {}",
                js_reason(&error)
            )),
        }
    });
    Ok(())
}

fn identify_text_agent(canvas: &web_sys::HtmlCanvasElement) {
    let Some(input) = canvas
        .next_element_sibling()
        .filter(|element| element.tag_name() == "INPUT")
    else {
        return;
    };
    let _ignored = input.set_attribute("id", "garmin-toolkit-text-agent");
}

struct App {
    context: eframe::egui::Context,
    translations: Translations,
    intl: Intl,
    shared: Rc<RefCell<State>>,
    first_frame: bool,
    navigation: shell::Navigation,
    page: Page,
    selected_profile: Option<usize>,
    profile_menu_expanded: bool,
    selected_activity: usize,
    create_profile: Option<profile::CreateState>,
    profiles: Rc<Vec<ProfileSnapshot>>,
    profile_presentations: Vec<profile::Presentation>,
    activity_presentations: Vec<activity::Presentation>,
    activity_detail: Option<Rc<ActivityDetailSnapshot>>,
    activity_segments: Vec<Vec<path::Point>>,
    applied_preferences: Option<(UserId, ProfilePreferences)>,
}

impl App {
    fn new(context: eframe::egui::Context) -> Result<Self, garmin_i18n::Error> {
        let translations = Translations::bundled()?;
        let intl = translations.formatter(Language::English)?;
        let shared = Rc::new(RefCell::new(State::default()));
        spawn_connection(Rc::clone(&shared), context.clone());
        Ok(Self {
            context,
            translations,
            intl,
            shared,
            first_frame: true,
            navigation: shell::Navigation::Expanded,
            page: Page::Activities,
            selected_profile: None,
            profile_menu_expanded: false,
            selected_activity: 0,
            create_profile: None,
            profiles: Rc::new(Vec::new()),
            profile_presentations: Vec::new(),
            activity_presentations: Vec::new(),
            activity_detail: None,
            activity_segments: Vec::new(),
            applied_preferences: None,
        })
    }

    fn sync_profiles(&mut self, profiles: Rc<Vec<ProfileSnapshot>>) {
        if Rc::ptr_eq(&self.profiles, &profiles) {
            return;
        }
        let selected = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
            .map(|profile| profile.user.id());
        self.profile_presentations = profiles
            .iter()
            .map(profile::Presentation::from_snapshot)
            .collect();
        self.profiles = profiles;
        self.selected_profile = selected.and_then(|user_id| {
            self.profiles
                .iter()
                .position(|profile| profile.user.id() == user_id)
        });
        self.sync_selected_preferences();
        self.rebuild_activities();
    }

    fn sync_selected_preferences(&mut self) {
        let Some((user_id, preferences)) = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
            .map(|profile| (profile.user.id(), profile.user.profile().preferences()))
        else {
            return;
        };
        if self.applied_preferences != Some((user_id, preferences)) {
            self.apply_preferences(user_id, preferences);
        }
    }

    fn rebuild_activities(&mut self) {
        self.activity_presentations.clear();
        let Some(profile) = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
        else {
            return;
        };
        let units = profile.user.profile().preferences().unit_system();
        self.activity_presentations.extend(
            profile
                .activities
                .iter()
                .map(|activity| activity::Presentation::from_snapshot(activity, &self.intl, units)),
        );
    }

    fn sync_activity_detail(&mut self, detail: Option<Rc<ActivityDetailSnapshot>>) {
        if matches!((&self.activity_detail, &detail), (Some(current), Some(next)) if Rc::ptr_eq(current, next))
            || self.activity_detail.is_none() && detail.is_none()
        {
            return;
        }
        self.activity_segments = detail
            .as_deref()
            .map(|detail| {
                detail
                    .segments
                    .iter()
                    .map(|segment| {
                        segment
                            .iter()
                            .map(|point| path::Point {
                                latitude: point.latitude().as_degrees(),
                                longitude: point.longitude().as_degrees(),
                            })
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.activity_detail = detail;
    }

    fn show_chooser(
        &mut self,
        ui: &mut Ui,
        product_name: &str,
        profiles: &[ProfileSnapshot],
        loaded: bool,
        connected: bool,
        error: Option<&str>,
        notice: Option<&str>,
    ) {
        let profile_props = self
            .profile_presentations
            .iter()
            .map(profile::Presentation::props)
            .collect::<Vec<_>>();
        let output = shell::show(
            ui,
            &shell::Props {
                product_name,
                navigation_groups: &[],
                active: None,
                navigation: shell::Navigation::Rail,
                profile_selector: None,
                toggle_label: "",
                profile_label: "",
                window_controls: None,
            },
            |ui| {
                if let Some(error) = error {
                    device::show_collection_state(ui, device::CollectionState::Error(error));
                    return None;
                }
                if !connected || !loaded {
                    let label = if connected {
                        format_message!(&self.intl, default_message: "Loading profiles")
                    } else {
                        format_message!(
                            &self.intl,
                            default_message: "Connecting to the device host…"
                        )
                    };
                    progress::show(
                        ui,
                        &progress::Props {
                            label: &label,
                            detail: None,
                            value: progress::Value::Indeterminate,
                        },
                    );
                    return None;
                }
                if let Some(notice) = notice {
                    notification::show(
                        ui,
                        &notification::Props {
                            kind: notification::Kind::Error,
                            title: notice,
                            detail: None,
                        },
                    );
                    ui.add_space(12.0);
                }
                profile::chooser(
                    ui,
                    &profile::ChooserProps {
                        intl: &self.intl,
                        profiles: &profile_props,
                    },
                )
            },
        );
        match output.inner {
            Some(profile::Action::Select(index)) => self.select_profile(index, profiles),
            Some(profile::Action::Create) => {
                self.create_profile = Some(profile::CreateState::default());
            }
            Some(profile::Action::Toggle | profile::Action::Settings | profile::Action::Logout)
            | None => {}
        }
    }

    fn show_application(
        &mut self,
        ui: &mut Ui,
        product_name: &str,
        profiles: &[ProfileSnapshot],
        devices: &[DeviceSnapshot],
        preferences_saving: bool,
        notice: Option<&str>,
    ) {
        let Some(profile_index) = self
            .selected_profile
            .filter(|index| *index < profiles.len())
        else {
            self.selected_profile = None;
            return;
        };
        let profile_props = self
            .profile_presentations
            .iter()
            .map(profile::Presentation::props)
            .collect::<Vec<_>>();
        let current_profile = &profiles[profile_index];
        let output = workspace::show(
            ui,
            &workspace::Props {
                product_name,
                intl: &self.intl,
                profiles: &profile_props,
                selected_profile: profile_index,
                profile_menu_expanded: self.profile_menu_expanded,
                page: &self.page,
                navigation: self.navigation,
                devices,
                window_controls: None,
            },
            |ui| {
                if let Some(notice) = notice {
                    notification::show(
                        ui,
                        &notification::Props {
                            kind: notification::Kind::Error,
                            title: notice,
                            detail: None,
                        },
                    );
                    ui.add_space(12.0);
                }
                match &self.page {
                    Page::Activities => PageAction::Activities(show_activities(
                        ui,
                        &self.intl,
                        current_profile,
                        &self.activity_presentations,
                        self.selected_activity,
                        self.activity_detail.as_deref(),
                        &self.activity_segments,
                    )),
                    Page::ProfileSettings => PageAction::Settings(profile_settings::show(
                        ui,
                        &settings_props(
                            &self.intl,
                            current_profile,
                            self.profile_presentations[profile_index].props(),
                            preferences_saving,
                        ),
                    )),
                    Page::Device(key) => {
                        if let Some(snapshot) = devices.iter().find(|device| &device.key == key) {
                            device::show_snapshot(ui, &self.intl, snapshot);
                        }
                        PageAction::Device
                    }
                }
            },
        );
        self.handle_page_action(output.inner, profiles);
        self.handle_shell_action(output.action, profiles, devices);
    }

    fn show_offline(&self, ui: &Ui, since_milliseconds: f64) {
        let seconds = ((js_sys::Date::now() - since_milliseconds).max(0.0) / 1_000.0) as u64;
        let duration = offline::format_duration(seconds);
        let title = format_message!(&self.intl, default_message: "Connection lost");
        let message = format_message!(
            &self.intl,
            default_message: "Reconnecting automatically…",
        );
        let elapsed = format_message!(
            &self.intl,
            default_message: "Offline for {duration}",
            values: { duration: duration.as_str() },
        );
        offline::show(
            ui,
            Id::new("device-host-offline"),
            &offline::Props {
                title: &title,
                message: &message,
                elapsed: &elapsed,
            },
        );
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }

    fn handle_page_action(&mut self, action: PageAction, profiles: &[ProfileSnapshot]) {
        match action {
            PageAction::Activities(Some(activity::Action::Select(index))) => {
                self.selected_activity = index;
                self.load_selected_activity(profiles);
            }
            PageAction::Settings(Some(profile_settings::Action::ChoosePicture)) => {}
            PageAction::Settings(Some(profile_settings::Action::UpdatePreferences(
                preferences,
            ))) => {
                self.update_preferences(preferences, profiles);
            }
            PageAction::Activities(None) | PageAction::Settings(None) | PageAction::Device => {}
        }
    }

    fn handle_shell_action(
        &mut self,
        action: Option<shell::Action>,
        profiles: &[ProfileSnapshot],
        devices: &[DeviceSnapshot],
    ) {
        match action {
            Some(shell::Action::ToggleNavigation) => {
                self.navigation = match self.navigation {
                    shell::Navigation::Expanded => shell::Navigation::Rail,
                    shell::Navigation::Rail => shell::Navigation::Expanded,
                };
            }
            Some(shell::Action::Navigate(index)) => {
                self.page = Page::from_index(index, devices).unwrap_or(Page::Activities);
                self.profile_menu_expanded = false;
            }
            Some(shell::Action::Profile(profile::Action::Toggle)) => {
                self.profile_menu_expanded = !self.profile_menu_expanded;
            }
            Some(shell::Action::Profile(profile::Action::Select(index))) => {
                self.select_profile(index, profiles);
            }
            Some(shell::Action::Profile(profile::Action::Create)) => {
                self.create_profile = Some(profile::CreateState::default());
                self.profile_menu_expanded = false;
            }
            Some(shell::Action::Profile(profile::Action::Settings)) => {
                self.page = Page::ProfileSettings;
                self.profile_menu_expanded = false;
            }
            Some(shell::Action::Profile(profile::Action::Logout)) => {
                self.selected_profile = None;
                self.page = Page::Activities;
                self.profile_menu_expanded = false;
            }
            Some(shell::Action::Window(_)) | None => {}
        }
    }

    fn select_profile(&mut self, index: usize, profiles: &[ProfileSnapshot]) {
        let Some(profile) = profiles.get(index) else {
            return;
        };
        self.selected_profile = Some(index);
        self.profile_menu_expanded = false;
        self.page = Page::Activities;
        self.selected_activity = 0;
        self.apply_preferences(profile.user.id(), profile.user.profile().preferences());
        self.rebuild_activities();
        self.load_selected_activity(profiles);
    }

    fn apply_preferences(&mut self, user_id: UserId, preferences: ProfilePreferences) {
        let language = match preferences.language() {
            LanguagePreference::English => Language::English,
            LanguagePreference::Czech => Language::Czech,
        };
        if let Ok(intl) = self.translations.formatter(language) {
            self.intl = intl;
        }
        let theme = match preferences.theme() {
            ThemePreference::Auto => eframe::egui::ThemePreference::System,
            ThemePreference::Dark => eframe::egui::ThemePreference::Dark,
            ThemePreference::Light => eframe::egui::ThemePreference::Light,
        };
        self.context.set_theme(theme);
        self.applied_preferences = Some((user_id, preferences));
    }

    fn update_preferences(
        &mut self,
        preferences: ProfilePreferences,
        profiles: &[ProfileSnapshot],
    ) {
        let Some(profile) = self.selected_profile.and_then(|index| profiles.get(index)) else {
            return;
        };
        let current = profile.user.profile().preferences();
        self.apply_preferences(profile.user.id(), preferences);
        spawn_update_preferences(
            Rc::clone(&self.shared),
            self.context.clone(),
            profile.user.id(),
            current,
            preferences,
        );
    }

    fn load_selected_activity(&self, profiles: &[ProfileSnapshot]) {
        let Some((profile, activity)) = self
            .selected_profile
            .and_then(|index| profiles.get(index))
            .and_then(|profile| {
                profile
                    .activities
                    .get(self.selected_activity)
                    .map(|activity| (profile, activity))
            })
        else {
            let mut state = self.shared.borrow_mut();
            state.requested_activity = None;
            state.activity_detail = None;
            return;
        };
        spawn_activity(
            Rc::clone(&self.shared),
            self.context.clone(),
            profile.user.id(),
            activity.id,
            format_message!(
                &self.intl,
                default_message: "The selected activity no longer exists",
            ),
        );
    }

    fn show_create_profile(&mut self, ui: &mut Ui) {
        let Some(dialog) = self.create_profile.as_mut() else {
            return;
        };
        match profile::create_dialog(ui, &self.intl, dialog) {
            Some(profile::CreateAction::Cancel) => self.create_profile = None,
            Some(profile::CreateAction::Submit(name)) => {
                dialog.set_submitting(true);
                spawn_create_profile(
                    Rc::clone(&self.shared),
                    self.context.clone(),
                    name.into_string(),
                );
            }
            None => {}
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            remove_loading_overlay();
        }
        let (
            devices,
            profiles,
            loaded,
            connected,
            error,
            notice,
            activity_detail,
            saving,
            created,
            creating,
            create_problem,
            disconnected_since,
            deployment_mode,
        ) = {
            let state = self.shared.borrow();
            (
                Rc::clone(&state.snapshots),
                Rc::clone(&state.profiles),
                state.profiles_loaded,
                state.client.is_some(),
                state.error.clone(),
                state.notice.clone(),
                state.activity_detail.as_ref().map(Rc::clone),
                state.preferences_saving,
                state.created_profile,
                state.profile_creating,
                state.profile_create_problem.clone(),
                state.disconnected_since_milliseconds,
                state.deployment_mode,
            )
        };
        let product_name = match deployment_mode {
            DeploymentMode::Production => "Garmin Toolkit",
            DeploymentMode::Demo => "Garmin Toolkit Demo",
        };
        self.sync_profiles(profiles);
        self.sync_activity_detail(activity_detail);
        let profiles = Rc::clone(&self.profiles);
        if matches!(&self.page, Page::Device(key) if !devices.iter().any(|device| &device.key == key))
        {
            self.page = Page::Activities;
        }
        if let Some(profile) = self.selected_profile.and_then(|index| profiles.get(index)) {
            self.selected_activity = self
                .selected_activity
                .min(profile.activities.len().saturating_sub(1));
        }
        if let Some(user_id) = created {
            if let Some(index) = profiles
                .iter()
                .position(|profile| profile.user.id() == user_id)
            {
                self.select_profile(index, profiles.as_slice());
                self.create_profile = None;
                self.shared.borrow_mut().created_profile = None;
            }
        } else if let Some(problem) = create_problem {
            if let Some(dialog) = self.create_profile.as_mut() {
                dialog.set_problem(problem);
            }
            self.shared.borrow_mut().profile_create_problem = None;
        } else if !creating
            && self
                .create_profile
                .as_ref()
                .is_some_and(profile::CreateState::is_submitting)
        {
            if let Some(dialog) = self.create_profile.as_mut() {
                dialog.set_submitting(false);
            }
        }
        self.sync_selected_preferences();
        let offline = loaded && disconnected_since.is_some();
        if offline {
            ui.disable();
        }
        match self.selected_profile {
            Some(_) => self.show_application(
                ui,
                product_name,
                profiles.as_slice(),
                devices.as_slice(),
                saving,
                notice.as_deref(),
            ),
            None => self.show_chooser(
                ui,
                product_name,
                profiles.as_slice(),
                loaded,
                connected,
                error.as_deref(),
                notice.as_deref(),
            ),
        }
        if !offline {
            self.show_create_profile(ui);
        }
        if let Some(since) = disconnected_since.filter(|_| loaded) {
            self.show_offline(ui, since);
        }
    }
}

enum PageAction {
    Activities(Option<activity::Action>),
    Settings(Option<profile_settings::Action>),
    Device,
}

fn show_activities(
    ui: &mut Ui,
    intl: &Intl,
    profile: &ProfileSnapshot,
    presentations: &[activity::Presentation],
    selected: usize,
    detail: Option<&ActivityDetailSnapshot>,
    points: &[Vec<path::Point>],
) -> Option<activity::Action> {
    let items = presentations
        .iter()
        .map(activity::Presentation::item_props)
        .collect::<Vec<_>>();
    let metrics = presentations
        .get(selected)
        .map_or_else(Vec::new, activity::Presentation::metric_props);
    let points = detail
        .filter(|detail| {
            profile
                .activities
                .get(selected)
                .is_some_and(|activity| activity.id == detail.id)
        })
        .map_or(&[][..], |_| points);
    let segments = points
        .iter()
        .map(|points| path::Segment { points })
        .collect::<Vec<_>>();
    let no_path = format_message!(intl, default_message: "No recorded path");
    let path = path::Props {
        segments: &segments,
        empty: &no_path,
        height: None,
    };
    let detail = presentations
        .get(selected)
        .map(|presentation| presentation.detail_props(&metrics, path));
    let no_activities = format_message!(intl, default_message: "No activities yet");
    let select_activity = format_message!(intl, default_message: "Select an activity");
    activity::browser(
        ui,
        &activity::BrowserProps {
            list: activity::ListProps {
                items: &items,
                selected: (!items.is_empty()).then_some(selected),
                empty: &no_activities,
            },
            detail: detail.as_ref(),
            empty_detail: &select_activity,
        },
    )
}

fn settings_props<'a>(
    intl: &'a Intl,
    snapshot: &ProfileSnapshot,
    profile: profile::ProfileProps<'a>,
    disabled: bool,
) -> profile_settings::Props<'a> {
    profile_settings::Props {
        intl,
        preferences: snapshot.user.profile().preferences(),
        profile,
        picture_enabled: false,
        disabled,
    }
}

fn set_profile_preferences(snapshot: &mut ProfileSnapshot, preferences: ProfilePreferences) {
    let mut profile = snapshot.user.profile().clone();
    profile.replace_preferences(preferences);
    snapshot.user.replace_profile(profile);
}

#[derive(Default)]
struct State {
    client: Option<ApplicationServiceClient>,
    deployment_mode: DeploymentMode,
    snapshots: Rc<Vec<DeviceSnapshot>>,
    profiles: Rc<Vec<ProfileSnapshot>>,
    profiles_loaded: bool,
    activity_detail: Option<Rc<ActivityDetailSnapshot>>,
    requested_activity: Option<garmin_model::observation::ObservationId>,
    preferences_saving: bool,
    profile_creating: bool,
    profile_create_problem: Option<String>,
    created_profile: Option<UserId>,
    notice: Option<String>,
    error: Option<String>,
    disconnected_since_milliseconds: Option<f64>,
}

fn spawn_connection(shared: Rc<RefCell<State>>, context: eframe::egui::Context) {
    spawn_local(async move {
        let mut retry_milliseconds = 1_000;
        loop {
            match connect().await {
                Ok((client, mut snapshots, profiles, deployment_mode)) => {
                    retry_milliseconds = 1_000;
                    match snapshots.borrow_and_update() {
                        Ok(initial) => {
                            let mut state = shared.borrow_mut();
                            state.client = Some(client.clone());
                            state.deployment_mode = deployment_mode;
                            state.snapshots = Rc::new(initial.clone());
                            state.profiles = Rc::new(profiles);
                            state.profiles_loaded = true;
                            state.error = None;
                            state.disconnected_since_milliseconds = None;
                        }
                        Err(error) => {
                            set_connection_error(&shared, error);
                            context.request_repaint();
                            wait_milliseconds(retry_milliseconds).await;
                            continue;
                        }
                    }
                    context.request_repaint();
                    let reason = loop {
                        match next_connection_event(&mut snapshots).await {
                            ConnectionEvent::Snapshot(Ok(())) => {
                                match snapshots.borrow_and_update() {
                                    Ok(value) => {
                                        shared.borrow_mut().snapshots = Rc::new(value.clone());
                                        context.request_repaint();
                                    }
                                    Err(error) => break error.to_string(),
                                }
                            }
                            ConnectionEvent::Snapshot(Err(reason)) => break reason,
                            ConnectionEvent::Heartbeat => {
                                if let Err(error) = heartbeat(&client).await {
                                    break error.to_string();
                                }
                            }
                        }
                    };
                    set_connection_error(&shared, reason);
                }
                Err(error) => set_connection_error(&shared, error),
            }
            context.request_repaint();
            wait_milliseconds(retry_milliseconds).await;
            retry_milliseconds = (retry_milliseconds * 2).min(10_000);
        }
    });
}

enum ConnectionEvent {
    Snapshot(Result<(), String>),
    Heartbeat,
}

async fn next_connection_event(
    snapshots: &mut rch::watch::Receiver<Vec<DeviceSnapshot>>,
) -> ConnectionEvent {
    let changed = snapshots.changed();
    let heartbeat_due = wait_milliseconds(HEARTBEAT_INTERVAL_MILLISECONDS);
    futures_util::pin_mut!(changed, heartbeat_due);
    match future::select(changed, heartbeat_due).await {
        future::Either::Left((result, _)) => {
            ConnectionEvent::Snapshot(result.map_err(|error| error.to_string()))
        }
        future::Either::Right(((), _)) => ConnectionEvent::Heartbeat,
    }
}

fn set_connection_error(shared: &RefCell<State>, error: impl fmt::Display) {
    let mut state = shared.borrow_mut();
    state.client = None;
    if state.profiles_loaded && state.disconnected_since_milliseconds.is_none() {
        state.disconnected_since_milliseconds = Some(js_sys::Date::now());
    }
    state.error = Some(format!("{error} Retrying…"));
}

async fn heartbeat(client: &ApplicationServiceClient) -> Result<(), ConnectError> {
    let call = client.heartbeat();
    let timeout = wait_milliseconds(HEARTBEAT_TIMEOUT_MILLISECONDS);
    futures_util::pin_mut!(call, timeout);
    match future::select(call, timeout).await {
        future::Either::Left((result, _)) => result.map_err(ConnectError::call),
        future::Either::Right(((), _)) => Err(ConnectError::transport("the heartbeat timed out")),
    }
}

async fn wait_milliseconds(milliseconds: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let scheduled = web_sys::window().is_some_and(|window| {
            window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds)
                .is_ok()
        });
        if !scheduled {
            let _ = resolve.call0(&JsValue::UNDEFINED);
        }
    });
    let _ = JsFuture::from(promise).await;
}

fn spawn_create_profile(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    display_name: String,
) {
    let Some(client) = shared.borrow().client.clone() else {
        return;
    };
    {
        let mut state = shared.borrow_mut();
        state.profile_creating = true;
        state.profile_create_problem = None;
        state.notice = None;
    }
    spawn_local(async move {
        let result = client
            .create_profile(display_name)
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain));
        let mut state = shared.borrow_mut();
        state.profile_creating = false;
        match result {
            Ok(user) => {
                let user_id = user.id();
                let profiles = Rc::make_mut(&mut state.profiles);
                profiles.push(ProfileSnapshot {
                    user,
                    avatar: None,
                    activities: Vec::new(),
                });
                profiles.sort_by_cached_key(|profile| {
                    (
                        profile
                            .user
                            .profile()
                            .display_name()
                            .as_str()
                            .to_lowercase(),
                        profile.user.id().to_string(),
                    )
                });
                state.created_profile = Some(user_id);
            }
            Err(error) => state.profile_create_problem = Some(error.to_string()),
        }
        drop(state);
        context.request_repaint();
    });
}

fn spawn_update_preferences(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    user_id: UserId,
    previous: ProfilePreferences,
    preferences: ProfilePreferences,
) {
    let Some(client) = shared.borrow().client.clone() else {
        return;
    };
    {
        let mut state = shared.borrow_mut();
        state.preferences_saving = true;
        state.notice = None;
        if let Some(profile) = Rc::make_mut(&mut state.profiles)
            .iter_mut()
            .find(|profile| profile.user.id() == user_id)
        {
            set_profile_preferences(profile, preferences);
        }
    }
    spawn_local(async move {
        let result = client
            .update_preferences(user_id, preferences)
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain));
        let mut state = shared.borrow_mut();
        state.preferences_saving = false;
        match result {
            Ok(user) => {
                if let Some(profile) = Rc::make_mut(&mut state.profiles)
                    .iter_mut()
                    .find(|profile| profile.user.id() == user_id)
                {
                    profile.user = user;
                }
            }
            Err(error) => {
                if let Some(profile) = Rc::make_mut(&mut state.profiles)
                    .iter_mut()
                    .find(|profile| profile.user.id() == user_id)
                {
                    set_profile_preferences(profile, previous);
                }
                state.notice = Some(error.to_string());
            }
        }
        drop(state);
        context.request_repaint();
    });
}

fn spawn_activity(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    user_id: UserId,
    observation_id: garmin_model::observation::ObservationId,
    missing: String,
) {
    let Some(client) = shared.borrow().client.clone() else {
        return;
    };
    {
        let mut state = shared.borrow_mut();
        state.requested_activity = Some(observation_id);
        state.activity_detail = None;
        state.notice = None;
    }
    spawn_local(async move {
        let result = client
            .activity(user_id, observation_id)
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain));
        let mut state = shared.borrow_mut();
        if state.requested_activity == Some(observation_id) {
            match result {
                Ok(Some(detail)) => state.activity_detail = Some(Rc::new(detail)),
                Ok(None) => {
                    state.activity_detail = None;
                    state.notice = Some(missing);
                }
                Err(error) => state.notice = Some(error.to_string()),
            }
            state.requested_activity = None;
        }
        drop(state);
        context.request_repaint();
    });
}

async fn connect() -> Result<
    (
        ApplicationServiceClient,
        remoc::rch::watch::Receiver<Vec<DeviceSnapshot>>,
        Vec<ProfileSnapshot>,
        DeploymentMode,
    ),
    ConnectError,
> {
    let websocket = WebSocket::connect(&websocket_url()?)
        .await
        .map_err(ConnectError::transport)?;
    let (websocket_tx, websocket_rx) = websocket.into_split();
    let transport_tx = websocket_tx
        .with(|packet: Bytes| future::ready(Ok::<_, io::Error>(Msg::Binary(packet.into()))));
    let transport_rx = websocket_rx.filter_map(|message| {
        future::ready(match message {
            Ok(Msg::Binary(packet)) => Some(Ok(Bytes::from(packet))),
            Ok(Msg::Text(_)) => None,
            Err(error) => Some(Err(error)),
        })
    });
    let client: ApplicationServiceClient =
        remoc::Connect::framed(remoc::Cfg::default(), transport_tx, transport_rx)
            .consume()
            .await
            .map_err(ConnectError::remoc)?;
    let deployment_mode = client.deployment_mode().await.map_err(ConnectError::call)?;
    let snapshots = client.watch_devices().await.map_err(ConnectError::call)?;
    let profiles = client
        .profiles()
        .await
        .map_err(ConnectError::call)?
        .map_err(ConnectError::domain)?;
    Ok((client, snapshots, profiles, deployment_mode))
}

fn websocket_url() -> Result<String, ConnectError> {
    let href = web_sys::window()
        .ok_or_else(|| ConnectError::location("browser window is unavailable"))?
        .location()
        .href()
        .map_err(|_| ConnectError::location("browser location is unavailable"))?;
    let (base, _) = href
        .rsplit_once('/')
        .ok_or_else(|| ConnectError::location("browser URL has no directory"))?;
    let base = base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            base.strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
        .ok_or_else(|| ConnectError::location("browser URL is not HTTP or HTTPS"))?;
    Ok(format!("{base}/remoc"))
}

#[derive(Debug)]
struct ConnectError(String);

impl ConnectError {
    fn transport(error: impl fmt::Display) -> Self {
        Self(format!("WebSocket connection failed: {error}"))
    }

    fn remoc(error: impl fmt::Display) -> Self {
        Self(format!("Application service negotiation failed: {error}"))
    }

    fn call(error: impl fmt::Display) -> Self {
        Self(format!("Application service call failed: {error}"))
    }

    fn domain(error: impl fmt::Display) -> Self {
        Self(error.to_string())
    }

    fn location(reason: &str) -> Self {
        Self(reason.to_owned())
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn remove_loading_overlay() {
    if let Some(overlay) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("loading"))
    {
        overlay.remove();
    }
}

fn show_startup_error(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
    if let Some(overlay) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("loading-message"))
    {
        overlay.set_text_content(Some(message));
    }
}

fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

fn js_reason(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}
