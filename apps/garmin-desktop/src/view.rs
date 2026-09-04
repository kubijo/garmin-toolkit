use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use eframe::egui::{
    ColorImage, Context, Id, Key, Modifiers, TextureHandle, TextureOptions, Ui, ViewportCommand,
    load::SizedTexture,
};
use garmin_color::{Color, swatch};
use garmin_device::attachments as devices;
use garmin_device::capabilities::{DataType, TransferDirection};
use garmin_i18n::{Intl, Language, Translations, format_message};
use garmin_model::{
    activity::{ActivityDuration, ActivitySport, ActivitySummary, Distance},
    identity::{
        DisplayName, LanguagePreference, ProfilePreferences, ThemePreference, UnitSystem, User,
        UserId,
    },
};
use garmin_services::{ActivityPreview, Application, UserContext};
use garmin_ui::{
    activity, device, file_import, icons, image_crop, input, modal, notification, path, profile,
    profile_settings, progress, shell,
};

use crate::{
    Error, device_backend,
    worker::{self, ImportOutcome, Worker},
};

pub struct Desktop {
    context: Context,
    translations: Translations,
    intl: Intl,
    profiles: Vec<ProfileView>,
    selected_profile: Option<usize>,
    profile_menu_expanded: bool,
    page: Page,
    preferences_saving: bool,
    selected_activity: usize,
    activity_path: Option<ActivityPath>,
    navigation: shell::Navigation,
    loaded: bool,
    create_profile: Option<CreateProfile>,
    avatar_editor: Option<AvatarEditor>,
    quit: QuitState,
    import: ImportStatus,
    notice: Option<Notice>,
    toasts: notification::Toasts,
    devices: devices::Manager<device_backend::Platform>,
    device_toasts: HashMap<notification::ToastId, String>,
    worker: Worker,
}

impl Desktop {
    pub fn new(
        application: Application,
        translations: Translations,
        context: eframe::egui::Context,
    ) -> Result<Self, Error> {
        let intl = translations.formatter(Language::English)?;
        let worker = Worker::spawn(application, context.clone())?;
        Ok(Self {
            context,
            translations,
            intl,
            profiles: Vec::new(),
            selected_profile: None,
            profile_menu_expanded: false,
            page: Page::Activities,
            preferences_saving: false,
            selected_activity: 0,
            activity_path: None,
            navigation: shell::Navigation::Expanded,
            loaded: false,
            create_profile: None,
            avatar_editor: None,
            quit: QuitState::Idle,
            import: ImportStatus::default(),
            notice: None,
            toasts: notification::Toasts::default(),
            devices: devices::Manager::new(device_backend::Platform::new()),
            device_toasts: HashMap::new(),
            worker,
        })
    }

    fn show_chooser(&mut self, ui: &mut Ui) {
        let window_copy = WindowCopy::new(&self.intl);
        let window_controls = window_copy.props(ui.ctx());
        let output = shell::show(
            ui,
            &shell::Props {
                product_name: "Garmin Toolkit",
                navigation_groups: &[],
                active: None,
                navigation: shell::Navigation::Rail,
                profile_selector: None,
                toggle_label: "",
                profile_label: "",
                window_controls: Some(&window_controls),
            },
            |ui| self.show_chooser_content(ui),
        );
        self.handle_shell_action(ui.ctx(), output.action, &[]);
    }

    fn show_chooser_content(&mut self, ui: &mut Ui) {
        if let Some(notice) = &self.notice {
            notification::show(ui, &notice.props());
            ui.add_space(12.0);
        }
        if !self.loaded {
            ui.vertical_centered(|ui| {
                ui.set_max_width(360.0);
                progress::show(
                    ui,
                    &progress::Props {
                        label: &format_message!(
                            &self.intl,
                            default_message: "Loading profiles",
                        ),
                        detail: None,
                        value: progress::Value::Indeterminate,
                    },
                );
            });
            return;
        }
        let profiles = self.profile_props();
        match profile::chooser(
            ui,
            &profile::ChooserProps {
                intl: &self.intl,
                profiles: &profiles,
            },
        ) {
            Some(profile::Action::Select(index)) => {
                self.select_profile(index);
            }
            Some(profile::Action::Create) => {
                self.create_profile = Some(CreateProfile::default());
            }
            Some(profile::Action::Toggle | profile::Action::Settings | profile::Action::Logout)
            | None => {}
        }
    }

    fn show_application(&mut self, ui: &mut Ui, profile_index: usize, drop_active: bool) {
        let profile_props = self.profile_props();
        let device_views = self.device_views();
        let device_destinations = device_views
            .iter()
            .map(|device| shell::Destination {
                label: &device.name,
                icon: device.icon,
            })
            .collect::<Vec<_>>();
        let activities = format_message!(
            &self.intl,
            default_message: "Activities",
        );
        let settings = format_message!(
            &self.intl,
            default_message: "Profile settings",
        );
        let primary_destinations = [
            shell::Destination {
                label: &activities,
                icon: icons::ACTIVITY,
            },
            shell::Destination {
                label: &settings,
                icon: icons::GEAR,
            },
        ];
        let devices_label = format_message!(
            &self.intl,
            default_message: "Attached devices",
        );
        let navigation_groups = [
            shell::NavigationGroup {
                label: None,
                destinations: &primary_destinations,
            },
            shell::NavigationGroup {
                label: (!device_destinations.is_empty()).then_some(devices_label.as_str()),
                destinations: &device_destinations,
            },
        ];
        let toggle_navigation = format_message!(
            &self.intl,
            default_message: "Toggle navigation",
        );
        let choose_profile = format_message!(
            &self.intl,
            default_message: "Choose a profile",
        );
        let window_copy = WindowCopy::new(&self.intl);
        let window_controls = window_copy.props(ui.ctx());
        let profile_selector = profile::SelectorProps {
            intl: &self.intl,
            profiles: &profile_props,
            selected: Some(profile_index),
            expanded: self.profile_menu_expanded,
        };
        let props = shell::Props {
            product_name: "Garmin Toolkit",
            navigation_groups: &navigation_groups,
            active: self.page.index(&device_views),
            navigation: self.navigation,
            profile_selector: Some(&profile_selector),
            toggle_label: &toggle_navigation,
            profile_label: &choose_profile,
            window_controls: Some(&window_controls),
        };
        let output = shell::show(ui, &props, |ui| {
            if let Some(notice) = &self.notice {
                notification::show(ui, &notice.props());
                ui.add_space(12.0);
            }
            match &self.page {
                Page::Activities => PageOutput::Activities(self.show_activities_page(
                    ui,
                    profile_index,
                    drop_active,
                )),
                Page::ProfileSettings => PageOutput::Settings(profile_settings::show(
                    ui,
                    &self.profile_settings_props(profile_index),
                )),
                Page::Device(key) => PageOutput::Device {
                    key: key.clone(),
                    action: device_views
                        .iter()
                        .find(|device| &device.key == key)
                        .and_then(|device| device.show(ui, &self.intl)),
                },
            }
        });

        self.handle_page_output(profile_index, output.inner);
        let device_keys = device_views
            .iter()
            .map(|device| device.key.clone())
            .collect::<Vec<_>>();
        self.handle_shell_action(ui.ctx(), output.action, &device_keys);
    }

    fn handle_page_output(&mut self, profile_index: usize, output: PageOutput) {
        match output {
            PageOutput::Activities((import, selected)) => {
                if let Some(activity::Action::Select(index)) = selected {
                    self.select_activity(index);
                }
                if let Some(action) = import {
                    self.open_import_dialog(action);
                }
            }
            PageOutput::Settings(Some(action)) => {
                self.handle_profile_settings_action(profile_index, action);
            }
            PageOutput::Device {
                key,
                action: Some(device::Action::Inspect),
            } => {
                if let Err(reason) = self.devices.inspect(&key) {
                    self.device_inspection_error(reason);
                }
            }
            PageOutput::Settings(None) | PageOutput::Device { action: None, .. } => {}
        }
    }

    fn show_activities_page(
        &self,
        ui: &mut Ui,
        profile_index: usize,
        drop_active: bool,
    ) -> (Option<file_import::Action>, Option<activity::Action>) {
        let current_profile = &self.profiles[profile_index];
        let items = current_profile.activity_props();
        let metric_props = current_profile.metric_props(self.selected_activity);
        let segments = self
            .activity_path
            .as_ref()
            .filter(|path| {
                current_profile
                    .previews
                    .get(self.selected_activity)
                    .is_some_and(|preview| preview.observation_id() == path.observation_id)
            })
            .map(ActivityPath::segment_props)
            .unwrap_or_default();
        let no_path = format_message!(&self.intl, default_message: "No recorded path");
        let path = path::Props {
            segments: &segments,
            empty: &no_path,
            height: None,
        };
        let detail = current_profile.detail_props(self.selected_activity, &metric_props, path);
        let no_activities = format_message!(&self.intl, default_message: "No activities yet");
        let select_activity = format_message!(&self.intl, default_message: "Select an activity");
        let import_copy = ImportCopy::new(&self.intl, drop_active);
        let import = file_import::show(
            ui,
            &file_import::Props {
                title: &import_copy.title,
                description: &import_copy.description,
                files_label: &import_copy.files,
                folder_label: &import_copy.folder,
                drop_active,
                enabled: !self.import.busy(),
            },
        );
        ui.add_space(12.0);
        show_import_status(ui, &self.intl, &self.import);
        if self.import.visible() {
            ui.add_space(12.0);
        }
        let selected = activity::browser(
            ui,
            &activity::BrowserProps {
                list: activity::ListProps {
                    items: &items,
                    selected: (!items.is_empty()).then_some(self.selected_activity),
                    empty: &no_activities,
                },
                detail: detail.as_ref(),
                empty_detail: &select_activity,
            },
        );
        (import, selected)
    }

    fn profile_settings_props(&self, profile_index: usize) -> profile_settings::Props<'_> {
        let profile = self.profiles[profile_index].user.profile();
        let preferences = profile.preferences();
        profile_settings::Props {
            intl: &self.intl,
            values: profile_settings::Values {
                unit_system: match preferences.unit_system() {
                    UnitSystem::Metric => profile_settings::UnitSystem::Metric,
                    UnitSystem::Imperial => profile_settings::UnitSystem::Imperial,
                },
                language: match preferences.language() {
                    LanguagePreference::English => profile_settings::Language::English,
                    LanguagePreference::Czech => profile_settings::Language::Czech,
                },
                theme: match preferences.theme() {
                    ThemePreference::Auto => profile_settings::Theme::Auto,
                    ThemePreference::Dark => profile_settings::Theme::Dark,
                    ThemePreference::Light => profile_settings::Theme::Light,
                },
            },
            profile: self.profiles[profile_index].profile_props(),
            disabled: self.preferences_saving,
        }
    }

    fn device_views(&self) -> Vec<DeviceView> {
        self.devices
            .presentations()
            .into_iter()
            .map(|presentation| DeviceView::new(presentation, &self.intl))
            .collect()
    }

    fn handle_profile_settings_action(
        &mut self,
        profile_index: usize,
        action: profile_settings::Action,
    ) {
        if action == profile_settings::Action::ChoosePicture {
            self.choose_profile_picture(profile_index);
        } else {
            self.update_preferences(profile_index, action);
        }
    }

    fn choose_profile_picture(&mut self, profile_index: usize) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Profile picture", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        else {
            return;
        };
        let user = UserContext::new(self.profiles[profile_index].user.id());
        match AvatarEditor::load(&self.context, user, path) {
            Ok(editor) => self.avatar_editor = Some(editor),
            Err(reason) => self.notice = Some(Notice::error(reason)),
        }
    }

    fn update_preferences(&mut self, profile_index: usize, action: profile_settings::Action) {
        let user = &self.profiles[profile_index].user;
        let current = user.profile().preferences();
        let preferences = match action {
            profile_settings::Action::ChoosePicture => return,
            profile_settings::Action::UnitSystem(unit_system) => ProfilePreferences::from_parts(
                match unit_system {
                    profile_settings::UnitSystem::Metric => UnitSystem::Metric,
                    profile_settings::UnitSystem::Imperial => UnitSystem::Imperial,
                },
                current.language(),
                current.theme(),
            ),
            profile_settings::Action::Language(language) => ProfilePreferences::from_parts(
                current.unit_system(),
                match language {
                    profile_settings::Language::English => LanguagePreference::English,
                    profile_settings::Language::Czech => LanguagePreference::Czech,
                },
                current.theme(),
            ),
            profile_settings::Action::Theme(theme) => ProfilePreferences::from_parts(
                current.unit_system(),
                current.language(),
                match theme {
                    profile_settings::Theme::Auto => ThemePreference::Auto,
                    profile_settings::Theme::Dark => ThemePreference::Dark,
                    profile_settings::Theme::Light => ThemePreference::Light,
                },
            ),
        };
        self.preferences_saving = true;
        self.worker
            .update_preferences(UserContext::new(user.id()), preferences);
    }

    fn handle_shell_action(
        &mut self,
        context: &Context,
        action: Option<shell::Action>,
        device_keys: &[String],
    ) {
        match action {
            Some(shell::Action::ToggleNavigation) => {
                self.navigation = match self.navigation {
                    shell::Navigation::Expanded => shell::Navigation::Rail,
                    shell::Navigation::Rail => shell::Navigation::Expanded,
                };
            }
            Some(shell::Action::Profile(profile::Action::Toggle)) => {
                self.profile_menu_expanded = !self.profile_menu_expanded;
            }
            Some(shell::Action::Profile(profile::Action::Select(index))) => {
                self.select_profile(index);
                self.profile_menu_expanded = false;
            }
            Some(shell::Action::Profile(profile::Action::Create)) => {
                self.create_profile = Some(CreateProfile::default());
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
            Some(shell::Action::Window(shell::WindowAction::Drag)) => {
                context.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            Some(shell::Action::Window(shell::WindowAction::Minimize)) => {
                context.send_viewport_cmd(ViewportCommand::Minimized(true));
            }
            Some(shell::Action::Window(shell::WindowAction::ToggleMaximize)) => {
                let maximized =
                    context.input(|input| input.viewport().maximized.unwrap_or_default());
                context.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }
            Some(shell::Action::Window(shell::WindowAction::Close)) => self.request_quit(context),
            Some(shell::Action::Navigate(index)) => {
                self.page = Page::from_index(index, device_keys).unwrap_or(Page::Activities);
                self.profile_menu_expanded = false;
            }
            None => {}
        }
    }

    fn work_in_progress(&self) -> bool {
        !self.loaded
            || self.import.busy()
            || self.preferences_saving
            || self.create_profile.is_some()
            || self.avatar_editor.is_some()
            || self.devices.inspecting_name().is_some()
    }

    fn active_operations(&self) -> Vec<ActiveOperation> {
        let mut operations = Vec::new();
        if !self.loaded {
            operations.push(ActiveOperation::LoadingProfiles);
        }
        if let Some(operation) = self.import.operation() {
            operations.push(operation);
        }
        if self.preferences_saving {
            operations.push(ActiveOperation::SavingPreferences);
        }
        if let Some(profile) = &self.create_profile {
            operations.push(if profile.submitting {
                ActiveOperation::CreatingProfile
            } else {
                ActiveOperation::ProfileDraft
            });
        }
        if let Some(editor) = &self.avatar_editor {
            operations.push(if editor.submitting {
                ActiveOperation::SavingProfilePicture
            } else {
                ActiveOperation::ProfilePictureDraft
            });
        }
        if let Some(name) = self.devices.inspecting_name() {
            operations.push(ActiveOperation::InspectingDevice(name.to_owned()));
        }
        operations
    }

    fn request_quit(&mut self, context: &Context) {
        if self.work_in_progress() {
            self.quit = QuitState::Confirming;
        } else {
            self.quit = QuitState::Closing;
            context.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn handle_quit_input(&mut self, context: &Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested && self.quit != QuitState::Closing {
            if self.work_in_progress() {
                context.send_viewport_cmd(ViewportCommand::CancelClose);
                self.quit = QuitState::Confirming;
            } else {
                self.quit = QuitState::Closing;
            }
        }
        let quit = context.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Q));
        if quit && self.quit != QuitState::Closing {
            self.request_quit(context);
        }
        let _ignored = context.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::W));
    }

    fn select_profile(&mut self, index: usize) {
        self.selected_profile = Some(index);
        self.profile_menu_expanded = false;
        self.page = Page::Activities;
        self.selected_activity = 0;
        self.activity_path = None;
        self.import = ImportStatus::default();
        self.apply_selected_preferences();
        self.load_selected_activity();
    }

    fn select_activity(&mut self, index: usize) {
        if self.selected_activity == index && self.activity_path.is_some() {
            return;
        }
        self.selected_activity = index;
        self.load_selected_activity();
    }

    fn load_selected_activity(&mut self) {
        self.activity_path = None;
        let Some((user, observation_id)) = self.selected_profile.and_then(|profile_index| {
            let profile = self.profiles.get(profile_index)?;
            let preview = profile.previews.get(self.selected_activity)?;
            Some((
                UserContext::new(profile.user.id()),
                preview.observation_id(),
            ))
        }) else {
            return;
        };
        self.worker.load_activity(user, observation_id);
    }

    fn profile_props(&self) -> Vec<profile::ProfileProps<'_>> {
        self.profiles
            .iter()
            .map(|profile| profile.profile_props())
            .collect()
    }

    fn open_import_dialog(&mut self, action: file_import::Action) {
        let dialog = rfd::FileDialog::new().add_filter("FIT activity", &["fit"]);
        let paths = match action {
            file_import::Action::Files => dialog.pick_files(),
            file_import::Action::Folder => dialog.pick_folder().map(|path| vec![path]),
        };
        if let Some(paths) = paths {
            self.start_import(paths);
        }
    }

    fn start_import(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() || self.import.busy() {
            return;
        }
        let Some(index) = self.selected_profile else {
            self.notice = Some(Notice::information(format_message!(
                &self.intl,
                default_message: "Choose a profile before importing files",
            )));
            return;
        };
        let user = UserContext::new(self.profiles[index].user.id());
        self.import.start();
        self.worker.import(user, paths);
    }

    fn process_devices(&mut self, context: &Context) {
        context.request_repaint_after(Duration::from_secs(1));
        for event in self.devices.poll() {
            match event {
                devices::Event::Attached { key, name } => {
                    let title = format_message!(
                        &self.intl,
                        default_message: "{device} connected",
                        description: "Notification title for a newly attached device",
                        values: { device: name.as_str() },
                    );
                    let detail = format_message!(
                        &self.intl,
                        default_message: "Open its device page to inspect or manage it.",
                    );
                    let action = format_message!(
                        &self.intl,
                        default_message: "View device",
                    );
                    let id = self.toasts.push(
                        notification::Toast::new(notification::Kind::Information, title)
                            .detail(detail)
                            .action(action),
                    );
                    self.device_toasts.insert(id, key);
                }
                devices::Event::Detached { key } => {
                    let stale = self
                        .device_toasts
                        .iter()
                        .filter_map(|(id, pending)| (pending == &key).then_some(*id))
                        .collect::<Vec<_>>();
                    for id in stale {
                        self.device_toasts.remove(&id);
                        self.toasts.dismiss(id);
                    }
                    if matches!(&self.page, Page::Device(active) if active == &key) {
                        self.page = Page::Activities;
                    }
                }
                devices::Event::Inspected { name } => {
                    let title = format_message!(
                        &self.intl,
                        default_message: "{device} inspected",
                        description: "Notification title after device metadata was read",
                        values: { device: name.as_str() },
                    );
                    self.toasts
                        .push(notification::Toast::new(notification::Kind::Success, title));
                }
                devices::Event::InspectionFailed { name, reason } => {
                    let title = format_message!(
                        &self.intl,
                        default_message: "Could not inspect {device}",
                        description: "Notification title after device inspection failed",
                        values: { device: name.as_str() },
                    );
                    self.toasts.push(
                        notification::Toast::new(notification::Kind::Error, title)
                            .detail(reason)
                            .persistent(),
                    );
                }
            }
        }
    }

    fn show_toasts(&mut self, context: &Context) {
        let events = self.toasts.show(context, Id::new("desktop-notifications"));
        for event in events {
            match event {
                notification::ToastEvent::Invoked(id) => {
                    let Some(key) = self.device_toasts.remove(&id) else {
                        continue;
                    };
                    self.page = Page::Device(key);
                }
                notification::ToastEvent::Dismissed(id) => {
                    self.device_toasts.remove(&id);
                }
            }
        }
    }

    fn device_inspection_error(&mut self, reason: String) {
        self.toasts.push(
            notification::Toast::new(
                notification::Kind::Error,
                format_message!(
                    &self.intl,
                    default_message: "Could not inspect the device",
                ),
            )
            .detail(reason)
            .persistent(),
        );
    }

    fn process_events(&mut self) {
        let events = self.worker.drain().collect::<Vec<_>>();
        for event in events {
            match event {
                worker::Event::Reloaded(result) => match result {
                    Ok(profiles) => {
                        self.replace_profiles(profiles, None);
                        self.loaded = true;
                    }
                    Err(reason) => {
                        self.loaded = true;
                        self.notice = Some(Notice::error(reason));
                    }
                },
                worker::Event::ProfileCreated(result) => match result {
                    Ok((user_id, profiles)) => {
                        self.replace_profiles(profiles, Some(user_id));
                        self.create_profile = None;
                    }
                    Err(reason) => {
                        if let Some(dialog) = self.create_profile.as_mut() {
                            dialog.submitting = false;
                        }
                        self.notice = Some(Notice::error(reason));
                    }
                },
                worker::Event::ProfileUpdated(result) => {
                    self.preferences_saving = false;
                    match result {
                        Ok(profiles) => {
                            self.replace_profiles(profiles, None);
                        }
                        Err(reason) => self.notice = Some(Notice::error(reason)),
                    }
                }
                worker::Event::AvatarUpdated(result) => match result {
                    Ok(profiles) => {
                        self.replace_profiles(profiles, None);
                        self.avatar_editor = None;
                    }
                    Err(reason) => {
                        if let Some(editor) = self.avatar_editor.as_mut() {
                            editor.submitting = false;
                        }
                        self.notice = Some(Notice::error(reason));
                    }
                },
                worker::Event::ImportStarted { total } => self.import.begin(total),
                worker::Event::ImportItem(item) => self.import.record(item),
                worker::Event::ImportFinished(result) => {
                    if let Ok(profiles) = result {
                        self.replace_profiles(profiles, None);
                    } else if let Err(reason) = result {
                        self.import.failed += 1;
                        self.import.last_problem = Some(reason);
                    }
                    self.import.finish();
                }
                worker::Event::ActivityLoaded {
                    observation_id,
                    result,
                } => {
                    let selected = self.selected_profile.and_then(|profile_index| {
                        self.profiles
                            .get(profile_index)?
                            .previews
                            .get(self.selected_activity)
                            .map(ActivityPreview::observation_id)
                    });
                    if selected != Some(observation_id) {
                        continue;
                    }
                    match result {
                        Ok(Some(details)) => {
                            self.activity_path = Some(ActivityPath::from_details(&details));
                        }
                        Ok(None) => {
                            self.notice = Some(Notice::error(format_message!(
                                &self.intl,
                                default_message: "The selected activity no longer exists",
                            )));
                        }
                        Err(reason) => self.notice = Some(Notice::error(reason)),
                    }
                }
            }
        }
    }

    fn replace_profiles(&mut self, profiles: Vec<worker::ProfileData>, select: Option<UserId>) {
        let selected = select.or_else(|| {
            self.selected_profile
                .and_then(|index| self.profiles.get(index))
                .map(|profile| profile.user.id())
        });
        self.profiles = profiles
            .into_iter()
            .map(|profile| ProfileView::new(profile, &self.intl, UnitSystem::Metric))
            .collect();
        self.selected_profile = selected.and_then(|user_id| {
            self.profiles
                .iter()
                .position(|profile| profile.user.id() == user_id)
        });
        let count = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
            .map_or(0, |profile| profile.activities.len());
        self.selected_activity = self.selected_activity.min(count.saturating_sub(1));
        self.apply_selected_preferences();
        self.load_selected_activity();
    }

    fn apply_selected_preferences(&mut self) {
        let Some(preferences) = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
            .map(|profile| profile.user.profile().preferences())
        else {
            return;
        };
        let language = match preferences.language() {
            LanguagePreference::English => Language::English,
            LanguagePreference::Czech => Language::Czech,
        };
        match self.translations.formatter(language) {
            Ok(intl) => {
                for profile in &mut self.profiles {
                    profile.reformat(&intl, preferences.unit_system());
                }
                self.intl = intl;
            }
            Err(error) => self.notice = Some(Notice::error(error.to_string())),
        }
        let theme = match preferences.theme() {
            ThemePreference::Auto => eframe::egui::ThemePreference::System,
            ThemePreference::Dark => eframe::egui::ThemePreference::Dark,
            ThemePreference::Light => eframe::egui::ThemePreference::Light,
        };
        self.context.set_theme(theme);
    }

    fn show_create_profile(&mut self, ui: &mut Ui) {
        let Some(dialog) = self.create_profile.as_mut() else {
            return;
        };
        let title = format_message!(&self.intl, default_message: "Create a profile");
        let description = format_message!(
            &self.intl,
            default_message: "Profiles keep each person's activities and devices separate.",
        );
        let label = format_message!(&self.intl, default_message: "Profile name");
        let placeholder = format_message!(&self.intl, default_message: "Enter a name");
        let required = format_message!(&self.intl, default_message: "Enter a profile name");
        let cancel = format_message!(&self.intl, default_message: "Cancel");
        let create = if dialog.submitting {
            format_message!(&self.intl, default_message: "Creating…")
        } else {
            format_message!(&self.intl, default_message: "Create")
        };
        let parsed = DisplayName::from_string(dialog.name.clone());
        let message = (!dialog.name.is_empty() && parsed.is_err())
            .then_some(input::Message::Error(required.as_str()));
        let output = modal::show(
            ui,
            Id::new("create-profile"),
            &modal::Props {
                title: &title,
                description: Some(&description),
                size: modal::Size::Medium,
                presentation: modal::Presentation::Modal,
                cancel_label: &cancel,
                backdrop_closes: Some(!dialog.submitting),
                primary: modal::Primary {
                    label: &create,
                    icon: Some(icons::PLUS),
                    kind: modal::PrimaryKind::Confirm,
                    enabled: parsed.is_ok() && !dialog.submitting,
                },
            },
            |ui| {
                input::show(
                    ui,
                    &mut dialog.name,
                    input::Props::new(&label)
                        .placeholder(&placeholder)
                        .message(message)
                        .disabled(dialog.submitting),
                )
            },
        );
        match output.action {
            Some(modal::Action::Cancel) if !dialog.submitting => self.create_profile = None,
            Some(modal::Action::Primary) => {
                if let Ok(display_name) = parsed {
                    dialog.submitting = true;
                    self.worker.create_profile(display_name);
                }
            }
            Some(modal::Action::Cancel) | None => {}
        }
    }

    fn show_avatar_editor(&mut self, ui: &mut Ui) {
        let Some(editor) = self.avatar_editor.as_mut() else {
            return;
        };
        let title = format_message!(&self.intl, default_message: "Adjust profile picture");
        let description = format_message!(
            &self.intl,
            default_message: "Drag to reposition. Scroll or use the slider to zoom.",
        );
        let zoom = format_message!(&self.intl, default_message: "Zoom");
        let cancel = format_message!(&self.intl, default_message: "Cancel");
        let save = if editor.submitting {
            format_message!(&self.intl, default_message: "Saving…")
        } else {
            format_message!(&self.intl, default_message: "Save picture")
        };
        let output = modal::show(
            ui,
            Id::new("profile-picture"),
            &modal::Props {
                title: &title,
                description: Some(&description),
                size: modal::Size::Medium,
                presentation: modal::Presentation::Modal,
                cancel_label: &cancel,
                backdrop_closes: Some(!editor.submitting),
                primary: modal::Primary {
                    label: &save,
                    icon: Some(icons::CHECK),
                    kind: modal::PrimaryKind::Confirm,
                    enabled: !editor.submitting,
                },
            },
            |ui| {
                ui.vertical_centered(|ui| {
                    image_crop::show(
                        ui,
                        &mut editor.crop,
                        image_crop::Props {
                            texture: SizedTexture::from_handle(&editor.texture),
                            source_size: editor.source_size,
                            zoom_label: &zoom,
                            disabled: editor.submitting,
                        },
                    )
                    .crop
                })
                .inner
            },
        );
        match output.action {
            Some(modal::Action::Cancel) if !editor.submitting => self.avatar_editor = None,
            Some(modal::Action::Primary) => {
                match garmin_importer::AvatarCrop::from_pixels(
                    output.inner.left(),
                    output.inner.top(),
                    output.inner.edge(),
                ) {
                    Ok(crop) => {
                        editor.submitting = true;
                        self.worker.import_avatar(
                            editor.user,
                            editor.path.clone(),
                            editor.bytes.clone(),
                            crop,
                        );
                    }
                    Err(error) => self.notice = Some(Notice::error(error.to_string())),
                }
            }
            Some(modal::Action::Cancel) | None => {}
        }
    }

    fn show_quit_confirmation(&mut self, ui: &mut Ui) {
        if self.quit != QuitState::Confirming {
            return;
        }
        let operations = self.active_operations();
        if operations.is_empty() {
            self.quit = QuitState::Idle;
            return;
        }
        let title = format_message!(
            &self.intl,
            default_message: "Abort current work and quit?",
        );
        let description = format_message!(
            &self.intl,
            default_message: "The following work is still in progress:",
        );
        let discarded = format_message!(
            &self.intl,
            default_message: "Unfinished work will be discarded.",
        );
        let cancel = format_message!(&self.intl, default_message: "Keep working");
        let quit = format_message!(&self.intl, default_message: "Abort and quit");
        let operations = operations
            .into_iter()
            .map(|operation| operation.label(&self.intl))
            .collect::<Vec<_>>();
        let output = modal::show(
            ui,
            Id::new("confirm-quit"),
            &modal::Props {
                title: &title,
                description: Some(&description),
                size: modal::Size::Small,
                presentation: modal::Presentation::Modal,
                cancel_label: &cancel,
                backdrop_closes: Some(false),
                primary: modal::Primary {
                    label: &quit,
                    icon: Some(icons::POWER),
                    kind: modal::PrimaryKind::Danger,
                    enabled: true,
                },
            },
            |ui| {
                for operation in &operations {
                    ui.label(format!("• {operation}"));
                }
                ui.add_space(12.0);
                ui.label(&discarded);
            },
        );
        match output.action {
            Some(modal::Action::Cancel) => self.quit = QuitState::Idle,
            Some(modal::Action::Primary) => {
                self.worker.abort();
                self.quit = QuitState::Closing;
                ui.ctx().send_viewport_cmd(ViewportCommand::Close);
            }
            None => {}
        }
    }
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.process_events();
        self.process_devices(ui.ctx());
        self.handle_quit_input(ui.ctx());
        let (drop_active, dropped) = ui.ctx().input(|input| {
            (
                !input.raw.hovered_files.is_empty(),
                input
                    .raw
                    .dropped_files
                    .iter()
                    .map(|file| file.path().to_owned())
                    .collect::<Vec<_>>(),
            )
        });
        if !dropped.is_empty() {
            self.start_import(dropped);
        }
        match self.selected_profile {
            Some(index) => self.show_application(ui, index, drop_active),
            None => self.show_chooser(ui),
        }
        self.show_create_profile(ui);
        self.show_avatar_editor(ui);
        self.show_quit_confirmation(ui);
        self.show_toasts(ui.ctx());
        crate::window::resize(ui);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum QuitState {
    #[default]
    Idle,
    Confirming,
    Closing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ActiveOperation {
    LoadingProfiles,
    ScanningFiles,
    ImportingActivities,
    SavingPreferences,
    CreatingProfile,
    ProfileDraft,
    SavingProfilePicture,
    ProfilePictureDraft,
    InspectingDevice(String),
}

impl ActiveOperation {
    fn label(&self, intl: &Intl) -> String {
        match self {
            Self::LoadingProfiles => {
                format_message!(intl, default_message: "Loading profiles and activities")
            }
            Self::ScanningFiles => {
                format_message!(intl, default_message: "Scanning selected files")
            }
            Self::ImportingActivities => {
                format_message!(intl, default_message: "Importing FIT activities")
            }
            Self::SavingPreferences => {
                format_message!(intl, default_message: "Saving profile settings")
            }
            Self::CreatingProfile => {
                format_message!(intl, default_message: "Creating a profile")
            }
            Self::ProfileDraft => format_message!(intl, default_message: "New profile draft"),
            Self::SavingProfilePicture => {
                format_message!(intl, default_message: "Saving profile picture")
            }
            Self::ProfilePictureDraft => {
                format_message!(intl, default_message: "Profile picture crop")
            }
            Self::InspectingDevice(device) => format_message!(
                intl,
                default_message: "Inspecting {device}",
                description: "Active operation shown while reading a connected device manifest",
                values: { device: device.as_str() },
            ),
        }
    }
}

struct AvatarEditor {
    user: UserContext,
    path: PathBuf,
    bytes: Vec<u8>,
    texture: TextureHandle,
    source_size: [u32; 2],
    crop: image_crop::State,
    submitting: bool,
}

impl AvatarEditor {
    fn load(context: &Context, user: UserContext, path: PathBuf) -> Result<Self, String> {
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let preview = garmin_importer::avatar_preview(&bytes).map_err(|error| error.to_string())?;
        let dimensions = preview.dimensions();
        let width = usize::try_from(dimensions.width()).map_err(|error| error.to_string())?;
        let height = usize::try_from(dimensions.height()).map_err(|error| error.to_string())?;
        let image = ColorImage::from_rgba_unmultiplied([width, height], preview.rgba());
        let texture = context.load_texture(
            format!("profile-picture-preview/{}", path.display()),
            image,
            TextureOptions::LINEAR,
        );
        Ok(Self {
            user,
            path,
            bytes,
            texture,
            source_size: [dimensions.width(), dimensions.height()],
            crop: image_crop::State::default(),
            submitting: false,
        })
    }
}

struct DeviceView {
    key: String,
    name: String,
    identifier: Option<String>,
    firmware: Option<String>,
    status: String,
    transfers: Vec<DeviceTransfer>,
    storages: Vec<DeviceStorageView>,
    icon: icons::Icon,
    inspect_enabled: bool,
}

impl DeviceView {
    fn new(presentation: devices::Presentation, intl: &Intl) -> Self {
        let inspect_enabled = matches!(
            presentation.state,
            devices::InspectionState::Available | devices::InspectionState::Failed
        );
        let status = match presentation.state {
            devices::InspectionState::Available => {
                format_message!(intl, default_message: "Not inspected")
            }
            devices::InspectionState::Running => {
                format_message!(intl, default_message: "Inspecting…")
            }
            devices::InspectionState::Ready => format_message!(intl, default_message: "Ready"),
            devices::InspectionState::Failed => {
                format_message!(intl, default_message: "Inspection failed")
            }
        };
        let firmware = presentation.software_version.map(|version| {
            format_message!(
                intl,
                default_message: "Software {version}",
                values: { version: version.to_string() },
            )
        });
        let icon = device_icon(&presentation.name);
        let transfers = device_transfers(&presentation.capabilities, intl);
        Self {
            key: presentation.key,
            name: presentation.name,
            identifier: presentation.identifier.map(|id| id.to_string()),
            firmware,
            status,
            transfers,
            storages: presentation
                .storage
                .into_iter()
                .flat_map(|state| state.storages)
                .map(|storage| DeviceStorageView::new(storage, intl))
                .collect(),
            icon,
            inspect_enabled,
        }
    }

    fn show(&self, ui: &mut Ui, intl: &Intl) -> Option<device::Action> {
        let inspect_label = if self.inspect_enabled {
            format_message!(intl, default_message: "Read device details")
        } else {
            format_message!(intl, default_message: "Device details read")
        };
        let status_label = format_message!(intl, default_message: "Status");
        let identifier_label = format_message!(intl, default_message: "Device ID");
        let software_label = format_message!(intl, default_message: "Software");
        let transfers_label = format_message!(intl, default_message: "Supported transfers");
        let transfers = self
            .transfers
            .iter()
            .map(|transfer| device::Transfer {
                data: &transfer.data,
                directions: &transfer.directions,
            })
            .collect::<Vec<_>>();
        device::show(
            ui,
            &device::Props {
                name: &self.name,
                connection: "USB/MTP",
                identifier: self.identifier.as_deref(),
                software: self.firmware.as_deref(),
                status: &self.status,
                status_label: &status_label,
                identifier_label: &identifier_label,
                software_label: &software_label,
                transfers_label: &transfers_label,
                transfers: &transfers,
                storages: &self
                    .storages
                    .iter()
                    .map(|storage| garmin_ui::capacity::Props {
                        label: &storage.label,
                        detail: &storage.detail,
                        bytes: storage.bytes,
                    })
                    .collect::<Vec<_>>(),
                icon: self.icon,
                inspect_label: &inspect_label,
                inspect_enabled: self.inspect_enabled,
            },
        )
    }
}

struct DeviceTransfer {
    data: String,
    directions: String,
}

struct DeviceStorageView {
    label: String,
    detail: String,
    bytes: Option<(u64, u64)>,
}

impl DeviceStorageView {
    fn new(storage: garmin_device::DeviceStorageState, intl: &Intl) -> Self {
        let label = if storage.writable == Some(false) {
            format_message!(intl, default_message: "{storage} · read-only", values: { storage: storage.label })
        } else {
            storage.label
        };
        let measured = storage.capacity.bytes();
        let bytes = measured.map(|(total, free)| (total - free, total));
        let detail = if let Some((total, free)) = measured {
            format_message!(intl,
            default_message: "{used} used · {free} free · {total} total",
            values: {
                used: capacity_bytes(total - free),
                free: capacity_bytes(free),
                total: capacity_bytes(total)
            })
        } else {
            let reason = match storage.capacity {
                garmin_device::StorageCapacity::Unavailable { reason } => reason,
                garmin_device::StorageCapacity::Available { .. } => {
                    "The device reported invalid storage totals".to_owned()
                }
            };
            format_message!(intl,
                default_message: "Could not read storage usage: {reason}",
                values: { reason: reason })
        };
        Self {
            label,
            detail,
            bytes,
        }
    }
}

fn capacity_bytes(bytes: u64) -> String {
    format!(
        "{:.2}",
        byte_unit::Byte::from_u64(bytes).get_appropriate_unit(byte_unit::UnitType::Decimal)
    )
}

fn device_transfers(capabilities: &[devices::Capability], intl: &Intl) -> Vec<DeviceTransfer> {
    let data_types = [
        (
            DataType::Activity,
            format_message!(intl, default_message: "Activities"),
        ),
        (
            DataType::Workout,
            format_message!(intl, default_message: "Workouts"),
        ),
        (
            DataType::Course,
            format_message!(intl, default_message: "Courses"),
        ),
    ];
    data_types
        .into_iter()
        .filter_map(|(data_type, data)| {
            let readable = capabilities.iter().any(|capability| {
                capability.data_type() == data_type
                    && matches!(
                        capability.direction(),
                        TransferDirection::OutputFromUnit | TransferDirection::InputOutput
                    )
            });
            let writable = capabilities.iter().any(|capability| {
                capability.data_type() == data_type
                    && matches!(
                        capability.direction(),
                        TransferDirection::InputToUnit | TransferDirection::InputOutput
                    )
            });
            let directions = match (readable, writable) {
                (true, true) => format_message!(intl, default_message: "Read and write"),
                (true, false) => format_message!(intl, default_message: "Read"),
                (false, true) => format_message!(intl, default_message: "Write"),
                (false, false) => return None,
            };
            Some(DeviceTransfer { data, directions })
        })
        .collect()
}

fn device_icon(name: &str) -> icons::Icon {
    let name = name.to_lowercase();
    if name.contains("edge") {
        icons::BICYCLE
    } else if name.contains("fenix") || name.contains("fēnix") || name.contains("venu") {
        icons::WATCH
    } else {
        icons::HARD_DRIVE
    }
}

struct WindowCopy {
    minimize: String,
    maximize: String,
    restore: String,
    close: String,
}

impl WindowCopy {
    fn new(intl: &Intl) -> Self {
        Self {
            minimize: format_message!(intl, default_message: "Minimize window"),
            maximize: format_message!(intl, default_message: "Maximize window"),
            restore: format_message!(intl, default_message: "Restore window"),
            close: format_message!(intl, default_message: "Close window"),
        }
    }

    fn props<'a>(&'a self, context: &Context) -> shell::WindowControls<'a> {
        shell::WindowControls {
            maximized: context.input(|input| input.viewport().maximized.unwrap_or_default()),
            minimize_label: &self.minimize,
            maximize_label: &self.maximize,
            restore_label: &self.restore,
            close_label: &self.close,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum Page {
    #[default]
    Activities,
    ProfileSettings,
    Device(String),
}

impl Page {
    fn index(&self, devices: &[DeviceView]) -> Option<usize> {
        match self {
            Self::Activities => Some(0),
            Self::ProfileSettings => Some(1),
            Self::Device(key) => devices
                .iter()
                .position(|device| &device.key == key)
                .map(|index| index + 2),
        }
    }

    fn from_index(index: usize, device_keys: &[String]) -> Option<Self> {
        match index {
            0 => Some(Self::Activities),
            1 => Some(Self::ProfileSettings),
            _ => device_keys.get(index - 2).cloned().map(Self::Device),
        }
    }
}

enum PageOutput {
    Activities((Option<file_import::Action>, Option<activity::Action>)),
    Settings(Option<profile_settings::Action>),
    Device {
        key: String,
        action: Option<device::Action>,
    },
}

#[derive(Default)]
struct CreateProfile {
    name: String,
    submitting: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ImportPhase {
    #[default]
    Idle,
    Scanning,
    Running,
    Finished,
}

#[derive(Default)]
struct ImportStatus {
    phase: ImportPhase,
    completed: usize,
    total: usize,
    imported_files: usize,
    imported_activities: usize,
    duplicates: usize,
    rejected: usize,
    failed: usize,
    last_problem: Option<String>,
}

impl ImportStatus {
    const fn busy(&self) -> bool {
        matches!(self.phase, ImportPhase::Scanning | ImportPhase::Running)
    }

    const fn operation(&self) -> Option<ActiveOperation> {
        match self.phase {
            ImportPhase::Scanning => Some(ActiveOperation::ScanningFiles),
            ImportPhase::Running => Some(ActiveOperation::ImportingActivities),
            ImportPhase::Idle | ImportPhase::Finished => None,
        }
    }

    const fn visible(&self) -> bool {
        !matches!(self.phase, ImportPhase::Idle)
    }

    fn start(&mut self) {
        *self = Self {
            phase: ImportPhase::Scanning,
            ..Self::default()
        };
    }

    const fn begin(&mut self, total: usize) {
        self.phase = ImportPhase::Running;
        self.total = total;
    }

    fn record(&mut self, item: worker::ImportItem) {
        self.completed += 1;
        let name = item.path.file_name().map_or_else(
            || item.path.display().to_string(),
            |name| name.to_string_lossy().into(),
        );
        match item.outcome {
            ImportOutcome::Imported(activities) => {
                self.imported_files += 1;
                self.imported_activities += activities;
            }
            ImportOutcome::Duplicate => {
                self.duplicates += 1;
            }
            ImportOutcome::Rejected(reason) => {
                self.rejected += 1;
                self.last_problem = Some(format!("{name}: {reason}"));
            }
            ImportOutcome::Failed(reason) => {
                self.failed += 1;
                self.last_problem = Some(format!("{name}: {reason}"));
            }
        }
    }

    const fn finish(&mut self) {
        self.phase = ImportPhase::Finished;
    }
}

struct ImportCopy {
    title: String,
    description: String,
    files: String,
    folder: String,
}

impl ImportCopy {
    fn new(intl: &Intl, drop_active: bool) -> Self {
        let description = if drop_active {
            format_message!(intl, default_message: "Release to import these files")
        } else {
            format_message!(intl, default_message: "Drop FIT files or folders here")
        };
        Self {
            title: format_message!(intl, default_message: "Import FIT activities"),
            description,
            files: format_message!(intl, default_message: "Choose files"),
            folder: format_message!(intl, default_message: "Choose a folder"),
        }
    }
}

struct Notice {
    kind: notification::Kind,
    title: String,
}

impl Notice {
    const fn information(title: String) -> Self {
        Self {
            kind: notification::Kind::Information,
            title,
        }
    }

    const fn error(title: String) -> Self {
        Self {
            kind: notification::Kind::Error,
            title,
        }
    }

    fn props(&self) -> notification::Props<'_> {
        notification::Props {
            kind: self.kind,
            title: &self.title,
            detail: None,
        }
    }
}

fn show_import_status(ui: &mut Ui, intl: &Intl, status: &ImportStatus) {
    match status.phase {
        ImportPhase::Idle => {}
        ImportPhase::Scanning => progress::show(
            ui,
            &progress::Props {
                label: &format_message!(intl, default_message: "Scanning selected files"),
                detail: None,
                value: progress::Value::Indeterminate,
            },
        ),
        ImportPhase::Running => {
            let detail = format_message!(
                intl,
                default_message: "{completed} of {total} files",
                values: {
                    completed: count(status.completed),
                    total: count(status.total),
                },
            );
            progress::show(
                ui,
                &progress::Props {
                    label: &format_message!(intl, default_message: "Importing FIT activities"),
                    detail: Some(&detail),
                    value: progress::Value::Determinate {
                        completed: status.completed,
                        total: status.total,
                    },
                },
            );
        }
        ImportPhase::Finished => {
            let title = if status.imported_activities == 0
                && status.duplicates > 0
                && status.rejected == 0
                && status.failed == 0
            {
                format_message!(
                    intl,
                    default_message: "{count, plural, one {# file was already imported} other {# files were already imported}}",
                    values: { count: count(status.duplicates) },
                )
            } else {
                format_message!(
                    intl,
                    default_message: "{count, plural, one {# activity imported} other {# activities imported}}",
                    values: { count: count(status.imported_activities) },
                )
            };
            let summary = format_message!(
                intl,
                default_message: "{files} accepted · {duplicates} already imported · {rejected} rejected · {failed} failed",
                values: {
                    files: count(status.imported_files),
                    duplicates: count(status.duplicates),
                    rejected: count(status.rejected),
                    failed: count(status.failed),
                },
            );
            let detail = status.last_problem.as_ref().map_or_else(
                || summary.clone(),
                |problem| {
                    format_message!(
                        intl,
                        default_message: "{summary} · {problem}",
                        values: {
                            summary: summary.as_str(),
                            problem: problem.as_str(),
                        },
                    )
                },
            );
            let kind = if status.failed > 0 && status.imported_files == 0 {
                notification::Kind::Error
            } else if status.failed > 0 || status.rejected > 0 {
                notification::Kind::Warning
            } else {
                notification::Kind::Success
            };
            notification::show(
                ui,
                &notification::Props {
                    kind,
                    title: &title,
                    detail: Some(detail.as_str()),
                },
            );
        }
    }
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

struct ProfileView {
    user: User,
    accent: Color,
    avatar: Option<profile::AvatarImage>,
    previews: Vec<ActivityPreview>,
    activities: Vec<ActivityView>,
}

impl ProfileView {
    fn new(profile: worker::ProfileData, intl: &Intl, units: UnitSystem) -> Self {
        let avatar = profile.avatar.map(|avatar| {
            let uri = format!(
                "bytes://garmin-toolkit/profile-avatar/{}.png",
                avatar.artifact_id()
            );
            profile::AvatarImage::encoded(uri, avatar.into_thumbnail())
        });
        let activities = profile
            .activities
            .iter()
            .map(|preview| ActivityView::new(preview, intl, units))
            .collect();
        let accent = profile.user.profile().accent().unwrap_or(swatch::ACTION);
        Self {
            user: profile.user,
            accent,
            avatar,
            previews: profile.activities,
            activities,
        }
    }

    fn reformat(&mut self, intl: &Intl, units: UnitSystem) {
        self.activities = self
            .previews
            .iter()
            .map(|preview| ActivityView::new(preview, intl, units))
            .collect();
    }

    fn profile_props(&self) -> profile::ProfileProps<'_> {
        profile::ProfileProps {
            display_name: self.user.profile().display_name().as_str(),
            accent: self.accent,
            avatar: self.avatar.as_ref(),
        }
    }

    fn activity_props(&self) -> Vec<activity::ItemProps<'_>> {
        self.activities
            .iter()
            .map(ActivityView::item_props)
            .collect()
    }

    fn metric_props(&self, selected: usize) -> Vec<activity::MetricProps<'_>> {
        self.activities
            .get(selected)
            .map_or_else(Vec::new, ActivityView::metric_props)
    }

    fn detail_props<'a>(
        &'a self,
        selected: usize,
        metrics: &'a [activity::MetricProps<'a>],
        path: path::Props<'a>,
    ) -> Option<activity::DetailProps<'a>> {
        self.activities
            .get(selected)
            .map(|activity| activity.detail_props(metrics, path))
    }
}

struct ActivityView {
    icon: icons::Icon,
    title: String,
    subtitle: String,
    distance: Option<String>,
    duration: String,
    metrics: Vec<MetricView>,
}

impl ActivityView {
    fn new(preview: &ActivityPreview, intl: &Intl, units: UnitSystem) -> Self {
        let summary = preview.summary();
        let title = sport_title(summary.sport(), intl);
        let icon = sport_icon(summary.sport());
        let source = preview
            .creator()
            .product_name()
            .map_or_else(|| "FIT".to_owned(), ToString::to_string);
        let subtitle = format_message!(
            intl,
            default_message: "{start} · {source}",
            values: {
                start: summary.time().start().to_string(),
                source: source,
            },
        );
        let totals = summary.totals();
        let distance = totals.distance().map(|value| format_distance(value, units));
        let duration = duration(totals.timer());
        let metrics = metrics(summary, intl, units);
        Self {
            icon,
            title,
            subtitle,
            distance,
            duration,
            metrics,
        }
    }

    fn item_props(&self) -> activity::ItemProps<'_> {
        activity::ItemProps {
            icon: self.icon,
            title: &self.title,
            subtitle: &self.subtitle,
            distance: self.distance.as_deref(),
            duration: &self.duration,
        }
    }

    fn metric_props(&self) -> Vec<activity::MetricProps<'_>> {
        self.metrics.iter().map(MetricView::props).collect()
    }

    fn detail_props<'a>(
        &'a self,
        metrics: &'a [activity::MetricProps<'a>],
        path: path::Props<'a>,
    ) -> activity::DetailProps<'a> {
        activity::DetailProps {
            icon: self.icon,
            title: &self.title,
            subtitle: &self.subtitle,
            metrics,
            path: Some(path),
            footer: None,
        }
    }
}

struct ActivityPath {
    observation_id: garmin_model::observation::ObservationId,
    segments: Vec<Vec<path::Point>>,
}

impl ActivityPath {
    fn from_details(details: &garmin_services::ActivityDetails) -> Self {
        let mut segments = Vec::new();
        let mut current = Vec::new();
        for sample in details.normalized().activity().track() {
            if let Some(coordinate) = sample.coordinate() {
                current.push(path::Point {
                    latitude: coordinate.latitude().as_degrees(),
                    longitude: coordinate.longitude().as_degrees(),
                });
            } else if current.len() >= 2 {
                segments.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
        if current.len() >= 2 {
            segments.push(current);
        }
        Self {
            observation_id: details.observation_id(),
            segments,
        }
    }

    fn segment_props(&self) -> Vec<path::Segment<'_>> {
        self.segments
            .iter()
            .map(|points| path::Segment { points })
            .collect()
    }
}

struct MetricView {
    label: String,
    value: String,
}

impl MetricView {
    fn props(&self) -> activity::MetricProps<'_> {
        activity::MetricProps {
            label: &self.label,
            value: &self.value,
        }
    }
}

fn metrics(summary: ActivitySummary, intl: &Intl, units: UnitSystem) -> Vec<MetricView> {
    let totals = summary.totals();
    let mut metrics = Vec::with_capacity(4);
    if let Some(distance) = totals.distance() {
        metrics.push(MetricView {
            label: format_message!(intl, default_message: "Distance"),
            value: format_distance(distance, units),
        });
    }
    metrics.push(MetricView {
        label: format_message!(intl, default_message: "Active time"),
        value: duration(totals.timer()),
    });
    if let Some(heart_rate) = summary.metrics().average_heart_rate() {
        metrics.push(MetricView {
            label: format_message!(intl, default_message: "Average heart rate"),
            value: heart_rate.to_string(),
        });
    }
    if let Some(ascent) = totals.ascent() {
        metrics.push(MetricView {
            label: format_message!(intl, default_message: "Ascent"),
            value: format_distance(ascent, units),
        });
    }
    metrics
}

fn format_distance(value: Distance, units: UnitSystem) -> String {
    match units {
        UnitSystem::Metric => metric_distance(value),
        UnitSystem::Imperial => imperial_distance(value),
    }
}

fn metric_distance(value: Distance) -> String {
    let millimeters = u128::from(value.as_millimeters());
    if millimeters >= 1_000_000 {
        let hundredths = (millimeters + 5_000) / 10_000;
        format!("{}.{:02} km", hundredths / 100, hundredths % 100)
    } else {
        format!("{} m", (millimeters + 500) / 1_000)
    }
}

fn imperial_distance(value: Distance) -> String {
    const MILLIMETERS_PER_MILE: u128 = 1_609_344;
    let millimeters = u128::from(value.as_millimeters());
    if millimeters >= MILLIMETERS_PER_MILE {
        let hundredths = (millimeters * 100 + MILLIMETERS_PER_MILE / 2) / MILLIMETERS_PER_MILE;
        format!("{}.{:02} mi", hundredths / 100, hundredths % 100)
    } else {
        let feet = (millimeters * 10 + 1_524) / 3_048;
        format!("{feet} ft")
    }
}

fn duration(value: ActivityDuration) -> String {
    let seconds = (value.as_milliseconds() + 500) / 1_000;
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    if hours > 0 {
        format!("{hours} h {minutes} min")
    } else if minutes > 0 {
        format!("{minutes} min")
    } else {
        format!("{seconds} s")
    }
}

const fn sport_icon(sport: ActivitySport) -> icons::Icon {
    match sport {
        ActivitySport::Running => icons::PERSON_SIMPLE_RUN,
        ActivitySport::Cycling => icons::BICYCLE,
    }
}

fn sport_title(sport: ActivitySport, intl: &Intl) -> String {
    match sport {
        ActivitySport::Running => format_message!(intl, default_message: "Running"),
        ActivitySport::Cycling => format_message!(intl, default_message: "Cycling"),
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn units_change_distance_rendering_without_changing_the_value() {
        let value = Distance::from_millimeters(10_000_000);

        assert_eq!(format_distance(value, UnitSystem::Metric), "10.00 km");
        assert_eq!(format_distance(value, UnitSystem::Imperial), "6.21 mi");
        assert_eq!(value.as_millimeters(), 10_000_000);
    }

    #[test]
    fn activity_duration_is_compact() {
        assert_eq!(
            duration(ActivityDuration::from_milliseconds(42_000)),
            "42 s"
        );
        assert_eq!(
            duration(ActivityDuration::from_milliseconds(3_180_000)),
            "53 min"
        );
        assert_eq!(
            duration(ActivityDuration::from_milliseconds(7_500_000)),
            "2 h 5 min"
        );
    }

    #[test]
    fn device_transfers_group_direction_specific_capabilities() {
        let translations = Translations::bundled().expect("embedded catalogs are valid");
        let intl = translations
            .formatter(Language::English)
            .expect("the source locale is available");
        let capabilities = [
            devices::Capability::new(DataType::Activity, TransferDirection::OutputFromUnit),
            devices::Capability::new(DataType::Workout, TransferDirection::OutputFromUnit),
            devices::Capability::new(DataType::Workout, TransferDirection::InputToUnit),
            devices::Capability::new(DataType::Course, TransferDirection::OutputFromUnit),
            devices::Capability::new(DataType::Course, TransferDirection::InputToUnit),
        ];

        let transfers = device_transfers(&capabilities, &intl)
            .into_iter()
            .map(|transfer| (transfer.data, transfer.directions))
            .collect::<Vec<_>>();

        assert_eq!(
            transfers,
            [
                ("Activities".to_owned(), "Read".to_owned()),
                ("Workouts".to_owned(), "Read and write".to_owned()),
                ("Courses".to_owned(), "Read and write".to_owned()),
            ]
        );
    }
}
