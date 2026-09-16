use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use camino::Utf8Path;
use eframe::egui::{
    ColorImage, Context, Id, Key, Modifiers, TextureHandle, TextureOptions, Ui, ViewportCommand,
    load::SizedTexture,
};
use garmin_color::{Color, swatch};
use garmin_device::attachments as devices;
use garmin_i18n::{Intl, Language, Translations, format_message};
use garmin_model::identity::{LanguagePreference, ThemePreference, UnitSystem, User, UserId};
use garmin_service_api::{
    ActivityDetailSnapshot, DeviceCapability, DeviceCatalogEntry, DeviceCatalogEntryKind,
    DeviceCatalogSnapshot, DeviceCatalogStorage, DeviceDataType, DeviceSnapshot, InspectionState,
    TransferDirection,
};
use garmin_services::{ActivityPreview, Application, UserContext};
use garmin_ui::{
    activity, device, device_browser, device_fit_preview, file_import, icons, image_crop, modal,
    notification, profile, profile_settings, progress, shell,
    workspace::{self, Page},
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
    activity_detail: Option<ActivityDetailSnapshot>,
    activity_workspace: activity::Workspace,
    navigation: shell::Navigation,
    loaded: bool,
    create_profile: Option<profile::CreateState>,
    avatar_editor: Option<AvatarEditor>,
    device_browser: Option<device_browser::Browser>,
    device_fit_preview: Option<device_fit_preview::Preview>,
    device_browser_loading: Option<String>,
    device_browser_status: DeviceBrowserStatus,
    quit: QuitState,
    import: ImportStatus,
    notice: Option<Notice>,
    toasts: notification::Toasts,
    devices: devices::Manager<device_backend::Platform>,
    device_toasts: HashMap<notification::ToastId, String>,
    worker: Worker,
    map_runtime: activity::map_runtime::MapRuntimeHandle,
}

impl Desktop {
    pub fn new(
        application: Application,
        translations: Translations,
        context: eframe::egui::Context,
        device_platform: device_backend::Platform,
        data_root: &Path,
        map_renderer: activity::WgpuMapHandle,
    ) -> Result<Self, Error> {
        let intl = translations.formatter(Language::English)?;
        let worker = Worker::spawn(application, context.clone())?;
        let map_worker = crate::map_worker::Worker::spawn(&data_root.join("cache/activity-map"))?;
        let map_runtime = activity::map_runtime::MapRuntimeHandle::new(
            map_worker,
            activity::map_runtime::Renderer::wgpu(map_renderer),
        );
        let activity_workspace = activity::Workspace::new(&map_runtime);
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
            activity_detail: None,
            activity_workspace,
            navigation: shell::Navigation::Expanded,
            loaded: false,
            create_profile: None,
            avatar_editor: None,
            device_browser: None,
            device_fit_preview: None,
            device_browser_loading: None,
            device_browser_status: DeviceBrowserStatus::Idle,
            quit: QuitState::Idle,
            import: ImportStatus::default(),
            notice: None,
            toasts: notification::Toasts::default(),
            devices: devices::Manager::new(device_platform),
            device_toasts: HashMap::new(),
            worker,
            map_runtime,
        })
    }

    fn show_chooser(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        let window_copy = WindowCopy::new(&self.intl);
        let window_controls = window_copy.props(ui.ctx());
        let output = shell::show(
            ui,
            &shell::Props {
                product_name: crate::mode::WINDOW_TITLE,
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
        self.handle_shell_action(ui.ctx(), frame, output.action, &[]);
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
                self.create_profile = Some(profile::CreateState::default());
            }
            Some(profile::Action::Toggle | profile::Action::Settings | profile::Action::Logout)
            | None => {}
        }
    }

    fn show_application(
        &mut self,
        ui: &mut Ui,
        frame: &eframe::Frame,
        profile_index: usize,
        drop_active: bool,
    ) {
        let mut device_browser = self.device_browser.take();
        let profile_props = self
            .profiles
            .iter()
            .map(ProfileView::profile_props)
            .collect::<Vec<_>>();
        let device_snapshots = self.device_snapshots();
        let activities_page = ActivitiesPage::new(
            &self.intl,
            &self.profiles[profile_index],
            self.selected_activity,
            self.activity_detail.as_ref(),
            &self.import,
            drop_active,
        );
        let profile = self.profiles[profile_index].user.profile();
        let settings_props = profile_settings::Props {
            intl: &self.intl,
            preferences: profile.preferences(),
            profile: self.profiles[profile_index].profile_props(),
            picture_enabled: true,
            disabled: self.preferences_saving,
        };
        let notice = self.notice.as_ref();
        let page = &self.page;
        let intl = &self.intl;
        let device_browser_loading = self.device_browser_loading.as_deref();
        let activity_workspace = &mut self.activity_workspace;
        let window_copy = WindowCopy::new(&self.intl);
        let window_controls = window_copy.props(ui.ctx());
        let props = workspace::Props {
            product_name: crate::mode::WINDOW_TITLE,
            intl: &self.intl,
            profiles: &profile_props,
            selected_profile: profile_index,
            profile_menu_expanded: self.profile_menu_expanded,
            page: &self.page,
            navigation: self.navigation,
            devices: &device_snapshots,
            window_controls: Some(&window_controls),
        };
        let output = workspace::show(ui, &props, |ui| {
            if let Some(notice) = notice {
                notification::show(ui, &notice.props());
                ui.add_space(12.0);
            }
            match page {
                Page::Activities => {
                    PageOutput::Activities(activities_page.show(ui, activity_workspace))
                }
                Page::ProfileSettings => {
                    PageOutput::Settings(profile_settings::show(ui, &settings_props))
                }
                Page::Device(key) => {
                    let action = device_snapshots
                        .iter()
                        .find(|snapshot| &snapshot.key == key)
                        .and_then(|snapshot| {
                            device::show_snapshot(
                                ui,
                                intl,
                                snapshot,
                                device_browser_loading == Some(key.as_str()),
                            )
                        });
                    PageOutput::Device {
                        key: key.clone(),
                        action,
                    }
                }
            }
        });
        let browser_action = match (&self.page, device_browser.as_mut()) {
            (Page::Device(key), Some(browser)) if browser.device_key() == key => {
                browser.show_window(ui, &self.intl)
            }
            _ => None,
        };

        self.device_browser = device_browser;
        self.handle_page_output(profile_index, output.inner);
        self.handle_shell_action(ui.ctx(), frame, output.action, &device_snapshots);
        if let Some(action) = browser_action {
            self.handle_device_browser_action(&action);
        }
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
                action: Some(device::Action::BrowseFiles),
            } => self.browse_device(key),
            PageOutput::Settings(None) | PageOutput::Device { action: None, .. } => {}
        }
    }

    fn handle_device_browser_action(&mut self, action: &device_browser::Action) {
        if matches!(action, device_browser::Action::Close) {
            if !self.device_browser_status.is_busy() {
                self.device_browser = None;
            }
            return;
        }
        if self.device_browser_status.is_busy() {
            return;
        }
        let Some(key) = self
            .device_browser
            .as_ref()
            .map(|browser| browser.device_key().to_owned())
        else {
            return;
        };
        let Some(candidate) = self.devices.candidate(&key) else {
            self.notice = Some(Notice::error(format_message!(
                &self.intl,
                default_message: "The selected device is no longer connected",
            )));
            return;
        };
        let operation = match action {
            device_browser::Action::Download(selection) => {
                let file_name = browser_download_name(selection);
                let mut dialog = rfd::FileDialog::new().set_file_name(&file_name);
                if selection.kind == DeviceCatalogEntryKind::Directory {
                    dialog = dialog.add_filter("ZIP archive", &["zip"]);
                }
                let Some(destination) = dialog.save_file() else {
                    return;
                };
                device_browser_download_operation(selection, destination)
            }
            device_browser::Action::Upload {
                storage_id,
                directory,
            } => {
                let Some(source) = rfd::FileDialog::new().pick_file() else {
                    return;
                };
                device_browser_upload_operation(storage_id, directory, source)
            }
            device_browser::Action::CreateDirectory {
                storage_id,
                parent,
                name,
            } => worker::DeviceBrowserOperation::CreateDirectory {
                storage_id: storage_id.clone(),
                parent: parent.clone(),
                name: name.clone(),
            },
            device_browser::Action::Remove(selection) => {
                worker::DeviceBrowserOperation::Remove(device_browser_target(selection))
            }
            device_browser::Action::Open(selection) => {
                worker::DeviceBrowserOperation::OpenFit(device_browser_target(selection))
            }
            device_browser::Action::ImportFit(selection) => {
                let Some(user) = self
                    .selected_profile
                    .and_then(|index| self.profiles.get(index))
                    .map(|profile| UserContext::new(profile.user.id()))
                else {
                    return;
                };
                worker::DeviceBrowserOperation::ImportFit {
                    target: device_browser_target(selection),
                    user,
                }
            }
            device_browser::Action::Close => unreachable!("close was handled above"),
        };
        self.device_browser_status = DeviceBrowserStatus::Busy;
        self.notice = Some(Notice::information(device_browser_progress(
            &self.intl,
            operation.kind(),
        )));
        self.worker.operate_device(candidate, operation);
    }

    fn browse_device(&mut self, key: String) {
        if self.device_browser_loading.is_some() {
            return;
        }
        let Some(candidate) = self.devices.candidate(&key) else {
            self.notice = Some(Notice::error(format_message!(
                &self.intl,
                default_message: "The selected device is no longer connected",
            )));
            return;
        };
        self.device_browser_loading = Some(key);
        self.worker.browse_device(candidate);
    }

    fn device_snapshots(&self) -> Vec<DeviceSnapshot> {
        self.devices
            .presentations()
            .into_iter()
            .map(device_snapshot)
            .collect()
    }

    fn handle_profile_settings_action(
        &mut self,
        profile_index: usize,
        action: profile_settings::Action,
    ) {
        match action {
            profile_settings::Action::ChoosePicture => self.choose_profile_picture(profile_index),
            profile_settings::Action::UpdatePreferences(preferences) => {
                let user = &self.profiles[profile_index].user;
                self.preferences_saving = true;
                self.worker
                    .update_preferences(UserContext::new(user.id()), preferences);
            }
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

    fn handle_shell_action(
        &mut self,
        context: &Context,
        frame: &eframe::Frame,
        action: Option<shell::Action>,
        devices: &[DeviceSnapshot],
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
            Some(shell::Action::Window(shell::WindowAction::Drag)) => {
                context.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            Some(shell::Action::Window(shell::WindowAction::ShowMenu(position))) => {
                crate::window::show_menu(context, frame, position);
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
                self.page = Page::from_index(index, devices).unwrap_or(Page::Activities);
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
            || self.device_browser_status.is_busy()
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
            operations.push(if profile.is_submitting() {
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
        if self.device_browser_status.is_busy() {
            operations.push(ActiveOperation::AccessingDeviceFiles);
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
        self.activity_detail = None;
        self.import = ImportStatus::default();
        self.apply_selected_preferences();
        self.load_selected_activity();
    }

    fn select_activity(&mut self, index: usize) {
        if self.selected_activity == index && self.activity_detail.is_some() {
            return;
        }
        self.selected_activity = index;
        self.load_selected_activity();
    }

    fn load_selected_activity(&mut self) {
        self.activity_detail = None;
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
                    let detail = format_message!(&self.intl, default_message: "Inspecting…");
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
                    self.dismiss_device_toasts(&key);
                    if self.device_browser_loading.as_deref() == Some(key.as_str()) {
                        self.device_browser_loading = None;
                    }
                    if self
                        .device_browser
                        .as_ref()
                        .is_some_and(|browser| browser.device_key() == key)
                    {
                        self.device_browser = None;
                        self.device_fit_preview = None;
                        self.device_browser_status = DeviceBrowserStatus::Idle;
                    }
                    if matches!(&self.page, Page::Device(active) if active == &key) {
                        self.page = Page::Activities;
                    }
                }
                devices::Event::Inspected { key, name } => {
                    self.dismiss_device_toasts(&key);
                    let title = format_message!(
                        &self.intl,
                        default_message: "{device} inspected",
                        description: "Notification title after device metadata was read",
                        values: { device: name.as_str() },
                    );
                    self.toasts
                        .push(notification::Toast::new(notification::Kind::Success, title));
                }
                devices::Event::InspectionFailed { key, name, reason } => {
                    self.dismiss_device_toasts(&key);
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

    fn dismiss_device_toasts(&mut self, key: &str) {
        let stale = self
            .device_toasts
            .iter()
            .filter_map(|(id, pending)| (pending == key).then_some(*id))
            .collect::<Vec<_>>();
        for id in stale {
            self.device_toasts.remove(&id);
            self.toasts.dismiss(id);
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
                            dialog.set_problem(reason);
                        } else {
                            self.notice = Some(Notice::error(reason));
                        }
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
                worker::Event::DeviceCatalog { key, result } => {
                    self.handle_device_catalog(key, result);
                }
                worker::Event::DeviceBrowser { key, kind, result } => {
                    self.handle_device_browser_result(key, kind, result);
                }
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
                            self.activity_detail = Some(ActivityDetailSnapshot {
                                id: details.observation_id(),
                                recording: garmin_service_api::ActivityRecordingSnapshot::from(
                                    details.normalized().activity(),
                                ),
                            });
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

    fn handle_device_catalog(
        &mut self,
        key: String,
        result: Result<garmin_device::DeviceCatalog, String>,
    ) {
        if self.device_browser_loading.as_deref() != Some(key.as_str()) {
            return;
        }
        self.device_browser_loading = None;
        if !matches!(&self.page, Page::Device(active) if active == &key) {
            return;
        }
        let catalog = match result {
            Ok(catalog) => catalog,
            Err(reason) => {
                self.notice = Some(Notice::error(reason));
                return;
            }
        };
        let Some(device_name) = self
            .devices
            .presentations()
            .into_iter()
            .find(|device| device.key == key)
            .map(|device| device.name)
        else {
            return;
        };
        match device_browser::Browser::open(
            device_catalog_snapshot(key, catalog),
            &self.intl,
            &device_name,
        ) {
            Ok(browser) => self.device_browser = Some(browser),
            Err(reason) => self.notice = Some(Notice::error(reason)),
        }
    }

    fn handle_device_browser_result(
        &mut self,
        key: String,
        kind: worker::DeviceBrowserOperationKind,
        result: Result<worker::DeviceBrowserOutcome, String>,
    ) {
        self.device_browser_status = DeviceBrowserStatus::Idle;
        if self
            .device_browser
            .as_ref()
            .is_none_or(|browser| browser.device_key() != key)
        {
            return;
        }
        match result {
            Ok(worker::DeviceBrowserOutcome::Downloaded(path)) => {
                self.notice = Some(Notice::success(format_message!(
                    &self.intl,
                    default_message: "Saved {file}",
                    values: { file: path.display().to_string() },
                )));
            }
            Ok(worker::DeviceBrowserOutcome::FitPreview { target, preview }) => {
                self.notice = None;
                let fit_preview =
                    device_fit_preview::Preview::new(target, preview, &self.map_runtime);
                self.device_fit_preview = Some(fit_preview);
            }
            Ok(worker::DeviceBrowserOutcome::FitImported { item, profiles }) => {
                self.import = ImportStatus::default();
                self.import.begin(1);
                self.import.record(item);
                self.import.finish();
                self.replace_profiles(profiles, None);
                self.device_fit_preview = None;
                self.page = Page::Activities;
                self.notice = None;
            }
            Ok(worker::DeviceBrowserOutcome::Refreshed(catalog)) => {
                let snapshot = device_catalog_snapshot(key, catalog);
                match self
                    .device_browser
                    .as_mut()
                    .expect("the active browser was checked above")
                    .refresh(snapshot)
                {
                    Ok(()) => {
                        self.notice =
                            Some(Notice::success(device_browser_complete(&self.intl, kind)));
                    }
                    Err(reason) => self.notice = Some(Notice::error(reason)),
                }
            }
            Err(reason) => self.notice = Some(Notice::error(reason)),
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
        match profile::create_dialog(ui, &self.intl, dialog) {
            Some(profile::CreateAction::Cancel) => self.create_profile = None,
            Some(profile::CreateAction::Submit(display_name)) => {
                dialog.set_submitting(true);
                self.worker.create_profile(display_name);
            }
            None => {}
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

    fn show_device_fit_preview(&mut self, ui: &mut Ui) {
        let units = self.selected_unit_system();
        let Some(preview) = self.device_fit_preview.as_mut() else {
            return;
        };
        match preview.show(ui, &self.intl, self.device_browser_status.is_busy(), units) {
            Some(device_fit_preview::Action::Close) => self.device_fit_preview = None,
            Some(device_fit_preview::Action::Import(target)) => {
                self.device_fit_preview = None;
                self.handle_device_browser_action(&device_browser::Action::ImportFit(
                    device_browser_selection(target),
                ));
            }
            None => {}
        }
    }

    fn selected_unit_system(&self) -> UnitSystem {
        self.selected_profile
            .and_then(|index| self.profiles.get(index))
            .map_or(UnitSystem::Metric, |profile| {
                profile.user.profile().preferences().unit_system()
            })
    }
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
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
            Some(index) => self.show_application(ui, frame, index, drop_active),
            None => self.show_chooser(ui, frame),
        }
        self.show_create_profile(ui);
        self.show_avatar_editor(ui);
        self.show_device_fit_preview(ui);
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DeviceBrowserStatus {
    #[default]
    Idle,
    Busy,
}

impl DeviceBrowserStatus {
    const fn is_busy(self) -> bool {
        matches!(self, Self::Busy)
    }
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
    AccessingDeviceFiles,
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
            Self::AccessingDeviceFiles => {
                format_message!(intl, default_message: "Accessing device files")
            }
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

fn device_snapshot(presentation: devices::Presentation) -> DeviceSnapshot {
    DeviceSnapshot {
        key: presentation.key,
        name: presentation.name,
        identifier: presentation
            .identifier
            .map(garmin_device::DeviceId::into_u32),
        software_version: presentation
            .software_version
            .map(garmin_device::SoftwareVersion::into_hundredths),
        inspection: match presentation.state {
            devices::InspectionState::Running => InspectionState::Running,
            devices::InspectionState::Ready => InspectionState::Ready,
            devices::InspectionState::Failed => InspectionState::Failed,
        },
        inspection_error: presentation.inspection_error,
        capabilities: presentation
            .capabilities
            .into_iter()
            .filter_map(|capability| {
                let data_type = match capability.data_type() {
                    garmin_device::DataType::Activity => DeviceDataType::Activity,
                    garmin_device::DataType::Workout => DeviceDataType::Workout,
                    garmin_device::DataType::Course => DeviceDataType::Course,
                    _ => return None,
                };
                let direction = match capability.direction() {
                    garmin_device::TransferDirection::OutputFromUnit => {
                        TransferDirection::OutputFromUnit
                    }
                    garmin_device::TransferDirection::InputToUnit => TransferDirection::InputToUnit,
                    garmin_device::TransferDirection::InputOutput => TransferDirection::InputOutput,
                };
                Some(DeviceCapability {
                    data_type,
                    direction,
                })
            })
            .collect(),
        storages: presentation
            .storage
            .map(|storage| storage.storages)
            .unwrap_or_default(),
    }
}

fn device_catalog_snapshot(
    device_key: String,
    catalog: garmin_device::DeviceCatalog,
) -> DeviceCatalogSnapshot {
    DeviceCatalogSnapshot {
        device_key,
        storages: catalog
            .storages
            .into_iter()
            .map(|storage| DeviceCatalogStorage {
                id: storage.id,
                label: storage.label,
                entries: storage
                    .entries
                    .into_iter()
                    .map(|entry| DeviceCatalogEntry {
                        path: entry.path,
                        kind: match entry.kind {
                            garmin_device::DeviceCatalogEntryKind::Directory => {
                                DeviceCatalogEntryKind::Directory
                            }
                            garmin_device::DeviceCatalogEntryKind::File => {
                                DeviceCatalogEntryKind::File
                            }
                        },
                        size: entry.size,
                    })
                    .collect(),
            })
            .collect(),
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

enum PageOutput {
    Activities((Option<file_import::Action>, Option<activity::Action>)),
    Settings(Option<profile_settings::Action>),
    Device {
        key: String,
        action: Option<device::Action>,
    },
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

struct ActivitiesPage<'a> {
    intl: &'a Intl,
    profile: &'a ProfileView,
    selected_activity: usize,
    detail: Option<&'a ActivityDetailSnapshot>,
    import: &'a ImportStatus,
    drop_active: bool,
}

impl<'a> ActivitiesPage<'a> {
    fn new(
        intl: &'a Intl,
        profile: &'a ProfileView,
        selected_activity: usize,
        detail: Option<&'a ActivityDetailSnapshot>,
        import: &'a ImportStatus,
        drop_active: bool,
    ) -> Self {
        let detail = detail.filter(|detail| {
            profile
                .previews
                .get(selected_activity)
                .is_some_and(|preview| preview.observation_id() == detail.id)
        });
        Self {
            intl,
            profile,
            selected_activity,
            detail,
            import,
            drop_active,
        }
    }

    fn show(
        &self,
        ui: &mut Ui,
        workspace: &mut activity::Workspace,
    ) -> (Option<file_import::Action>, Option<activity::Action>) {
        let drop_rect = ui.available_rect_before_wrap();
        let items = self.profile.activity_props();
        let recording_key = self.detail.map(|detail| detail.id.to_string());
        let no_route = format_message!(self.intl, default_message: "No recorded route");
        let no_activities = format_message!(self.intl, default_message: "No activities yet");
        let select_activity = format_message!(self.intl, default_message: "Select an activity");
        let import_copy = ImportCopy::new(self.intl, self.drop_active);
        let import = file_import::show(
            ui,
            &file_import::Props {
                title: &import_copy.title,
                description: &import_copy.description,
                files_label: &import_copy.files,
                folder_label: &import_copy.folder,
                drop_active: self.drop_active,
                enabled: !self.import.busy(),
            },
        );
        ui.add_space(12.0);
        show_import_status(ui, self.intl, self.import);
        if self.import.visible() {
            ui.add_space(12.0);
        }
        let selected = workspace.show(
            ui,
            self.intl,
            &activity::WorkspaceProps {
                items: &items,
                presentations: &self.profile.activities,
                selected: (!items.is_empty()).then_some(self.selected_activity),
                recording: self.detail.map(|detail| &detail.recording),
                recording_key: recording_key.as_deref(),
                units: self.profile.user.profile().preferences().unit_system(),
                empty_list: &no_activities,
                empty_detail: &select_activity,
                no_route: &no_route,
            },
        );
        if self.drop_active {
            file_import::drop_overlay(ui, drop_rect, &import_copy.description);
        }
        (import, selected)
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

    const fn success(title: String) -> Self {
        Self {
            kind: notification::Kind::Success,
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

fn device_browser_target(
    selection: &device_browser::Selection,
) -> garmin_device::DeviceBrowserTarget {
    garmin_device::DeviceBrowserTarget {
        storage_id: selection.storage_id.clone(),
        path: selection.path.clone(),
        kind: match selection.kind {
            DeviceCatalogEntryKind::Directory => garmin_device::DeviceCatalogEntryKind::Directory,
            DeviceCatalogEntryKind::File => garmin_device::DeviceCatalogEntryKind::File,
        },
    }
}

fn device_browser_download_operation(
    selection: &device_browser::Selection,
    destination: PathBuf,
) -> worker::DeviceBrowserOperation {
    worker::DeviceBrowserOperation::Download {
        target: device_browser_target(selection),
        destination,
    }
}

fn device_browser_upload_operation(
    storage_id: &str,
    directory: &Utf8Path,
    source: PathBuf,
) -> worker::DeviceBrowserOperation {
    worker::DeviceBrowserOperation::Upload {
        storage_id: storage_id.to_owned(),
        directory: directory.to_owned(),
        source,
    }
}

fn device_browser_selection(
    target: garmin_service_api::DeviceBrowserTarget,
) -> device_browser::Selection {
    device_browser::Selection {
        storage_label: target.storage_id.clone(),
        storage_id: target.storage_id,
        path: target.path,
        kind: target.kind,
        size: None,
    }
}

fn browser_download_name(selection: &device_browser::Selection) -> String {
    let name = selection
        .path
        .file_name()
        .unwrap_or(&selection.storage_label);
    if selection.kind == DeviceCatalogEntryKind::Directory {
        format!("{name}.zip")
    } else {
        name.to_owned()
    }
}

fn device_browser_progress(intl: &Intl, kind: worker::DeviceBrowserOperationKind) -> String {
    match kind {
        worker::DeviceBrowserOperationKind::OpenFit => {
            format_message!(intl, default_message: "Opening the FIT file…")
        }
        worker::DeviceBrowserOperationKind::ImportFit => {
            format_message!(intl, default_message: "Importing the FIT file…")
        }
        worker::DeviceBrowserOperationKind::Download => {
            format_message!(intl, default_message: "Downloading from the device…")
        }
        worker::DeviceBrowserOperationKind::Upload => {
            format_message!(intl, default_message: "Uploading to the device…")
        }
        worker::DeviceBrowserOperationKind::CreateDirectory => {
            format_message!(intl, default_message: "Creating the folder…")
        }
        worker::DeviceBrowserOperationKind::Remove => {
            format_message!(intl, default_message: "Removing the selected item…")
        }
    }
}

fn device_browser_complete(intl: &Intl, kind: worker::DeviceBrowserOperationKind) -> String {
    match kind {
        worker::DeviceBrowserOperationKind::OpenFit => {
            format_message!(intl, default_message: "FIT file opened")
        }
        worker::DeviceBrowserOperationKind::ImportFit => {
            format_message!(intl, default_message: "FIT import complete")
        }
        worker::DeviceBrowserOperationKind::Download => {
            format_message!(intl, default_message: "Download complete")
        }
        worker::DeviceBrowserOperationKind::Upload => {
            format_message!(intl, default_message: "Upload complete")
        }
        worker::DeviceBrowserOperationKind::CreateDirectory => {
            format_message!(intl, default_message: "Folder created")
        }
        worker::DeviceBrowserOperationKind::Remove => {
            format_message!(intl, default_message: "Item removed")
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
    activities: Vec<activity::Presentation>,
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
            .map(|preview| activity_presentation(preview, intl, units))
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
            .map(|preview| activity_presentation(preview, intl, units))
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
            .map(activity::Presentation::item_props)
            .collect()
    }
}

fn activity_presentation(
    preview: &ActivityPreview,
    intl: &Intl,
    units: UnitSystem,
) -> activity::Presentation {
    let source = preview
        .creator()
        .product_name()
        .map_or("FIT", |value| value.as_str());
    activity::Presentation::from_summary(preview.summary(), source, intl, units)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(path: &str, kind: DeviceCatalogEntryKind) -> device_browser::Selection {
        device_browser::Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: path.into(),
            kind,
            size: None,
        }
    }

    #[test]
    fn native_upload_selection_preserves_the_chosen_source_and_target_directory() {
        let source = PathBuf::from("/tmp/selected/ride.fit");

        let operation = device_browser_upload_operation(
            "internal",
            Utf8Path::new("Garmin/Activity"),
            source.clone(),
        );

        let worker::DeviceBrowserOperation::Upload {
            storage_id,
            directory,
            source: selected_source,
        } = operation
        else {
            panic!("the selected native file must route to upload");
        };
        assert_eq!(storage_id, "internal");
        assert_eq!(directory, "Garmin/Activity");
        assert_eq!(selected_source, source);
    }

    #[test]
    fn native_download_selection_preserves_destination_and_suggested_name() {
        let directory = selection("Garmin/Activity", DeviceCatalogEntryKind::Directory);
        let destination = PathBuf::from("/tmp/selected/Activity.zip");

        assert_eq!(browser_download_name(&directory), "Activity.zip");
        let operation = device_browser_download_operation(&directory, destination.clone());

        let worker::DeviceBrowserOperation::Download {
            target,
            destination: selected_destination,
        } = operation
        else {
            panic!("the selected native destination must route to download");
        };
        assert_eq!(target.storage_id, "internal");
        assert_eq!(target.path, "Garmin/Activity");
        assert_eq!(selected_destination, destination);
    }
}
