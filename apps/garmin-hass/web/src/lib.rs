#![cfg(target_arch = "wasm32")]

mod automation;
mod browser_timing;
mod control;
mod developer;
mod files;
mod map_composition;
mod map_worker;
mod window;
mod window_channel;
mod window_client;

use bytes::Bytes;
use camino::Utf8PathBuf;
use eframe::egui::{ColorImage, Id, TextureHandle, TextureOptions, Ui, load::SizedTexture};
use futures_util::{SinkExt as _, StreamExt as _, future};
use garmin_i18n::{Intl, Language, Translations, format_message};
use garmin_model::identity::{LanguagePreference, ProfilePreferences, ThemePreference, UserId};
use garmin_service_api::{
    ActivityDetailSnapshot, ApplicationService, ApplicationServiceClient, AvatarCrop, AvatarUpload,
    DeploymentMode, DeviceBrowserDownloadTicket, DeviceBrowserRequest, DeviceBrowserTarget,
    DeviceBrowserUpload, DeviceCatalogSnapshot, DeviceFitImportOutcome, DeviceFitPreview,
    DeviceSnapshot, ProfileSnapshot,
};
use garmin_ui::{
    activity, device, device_browser, device_fit_preview, icons, image_crop, modal, notification,
    offline, profile, profile_settings, progress, shell,
    workspace::{self, Page},
};
use remoc::prelude::*;
use std::{cell::RefCell, fmt, io, io::Cursor, rc::Rc, time::Duration};
use tokio_wasm_io::io::AsyncWriteExt as _;
use wasm_bindgen::{JsCast as _, prelude::*};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use websocket_web::{Msg, WebSocket};

const CANVAS_ID: &str = "garmin-toolkit";
const HEARTBEAT_INTERVAL_MILLISECONDS: i32 = 3_000;
const HEARTBEAT_TIMEOUT_MILLISECONDS: i32 = 5_000;
const MAX_DEVICE_BROWSER_TRANSFER_BYTES: f64 = 512.0 * 1024.0 * 1024.0;
const MAX_AVATAR_UPLOAD_BYTES: f64 = 10.0 * 1024.0 * 1024.0;

use map_worker::BrowserMapBackend;

struct BrowserMapMetrics {
    upload_events: bool,
}

impl activity::map_runtime::MapMetricsSink for BrowserMapMetrics {
    fn record(&self, _sample: activity::map_runtime::MapPerformanceSample) {}

    fn upload_events_enabled(&self) -> bool {
        self.upload_events
    }

    fn record_upload(&self, milliseconds: f64) {
        browser_timing::measure_duration("garmin.map.tile-upload-latency", milliseconds);
    }

    fn record_upload_event(&self, event: activity::map_runtime::MapUploadEvent) {
        if let Ok(detail) = serde_json::to_string(&event) {
            browser_timing::mark_detail("garmin.map.upload", &detail);
        }
    }

    fn record_render(&self, sample: activity::map_runtime::MapRenderPerformanceSample) {
        let name = match sample.phase {
            activity::map_runtime::MapRenderPhase::Prepare => "garmin.map.wgpu-prepare",
            activity::map_runtime::MapRenderPhase::Draw => "garmin.map.wgpu-draw",
        };
        browser_timing::measure_duration(name, f64::from(sample.milliseconds));
    }
}

/// Start the browser application, or leave initialization to the map worker.
///
/// # Errors
/// Returns an error when the application's canvas is missing or has the wrong element type.
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    developer::install();
    let Some(window) = web_sys::window() else {
        // The same generated module is loaded by the dedicated map worker.
        return Ok(());
    };
    install_browser_panic_surface();
    let canvas = window
        .document()
        .and_then(|document| document.get_element_by_id(CANVAS_ID))
        .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| js_error("the application canvas is missing"))?;
    if window::start_if_requested(&canvas)? {
        return Ok(());
    }
    let map_upload_telemetry: bool = serde_json::from_str(
        &canvas
            .get_attribute("data-map-upload-telemetry")
            .ok_or_else(|| js_error("the map upload telemetry configuration is missing"))?,
    )
    .map_err(|_| js_error("the map upload telemetry configuration is invalid"))?;
    let renderer_mode = map_composition::mode(&canvas)?;
    let ui_automation: bool = serde_json::from_str(
        &canvas
            .get_attribute("data-ui-automation")
            .ok_or_else(|| js_error("UI automation configuration is missing"))?,
    )
    .map_err(|_| js_error("UI automation configuration is invalid"))?;

    spawn_local(async move {
        let fixture_canvas = canvas.clone();
        let result = eframe::WebRunner::new()
            .start(
                canvas.clone(),
                map_composition::options(&renderer_mode),
                Box::new(move |creation| {
                    garmin_ui::install(&creation.egui_ctx);
                    let render_state = creation.wgpu_render_state.as_ref().ok_or_else(|| {
                        io::Error::other("eframe did not provide the required WGPU render state")
                    })?;
                    let adapter = render_state.adapter.get_info();
                    map_composition::report_startup(
                        &renderer_mode,
                        &format!("{:?}", adapter.backend),
                        &adapter.name,
                        map_upload_telemetry,
                    );
                    if renderer_mode.starts_with("worker-") && map_composition::is_fixture() {
                        return Ok(Box::new(map_composition::Fixture::new(creation, &fixture_canvas, &renderer_mode)?));
                    }
                    if renderer_mode.starts_with("worker-") {
                        map_composition::install_map(creation, &fixture_canvas, &renderer_mode)?;
                    }
                    browser_timing::mark_detail(
                        "garmin.map.upload-telemetry",
                        &serde_json::json!({
                            "version": 1,
                            "enabled": map_upload_telemetry,
                            "backend": format!("{:?}", render_state.adapter.get_info().backend),
                            "viewport": [window.inner_width().ok().and_then(|v| v.as_f64()), window.inner_height().ok().and_then(|v| v.as_f64())],
                            "dpr": window.device_pixel_ratio(),
                        }).to_string(),
                    );
                    let map_renderer = activity::install_wgpu_map(render_state, 1);
                    let app = App::new(creation.egui_ctx.clone(), map_renderer, map_upload_telemetry)?;
                    if ui_automation { automation::install(&creation.egui_ctx); }
                    Ok(Box::new(app))
                }),
            )
            .await;
        match result {
            Ok(()) => identify_text_agent(&canvas),
            Err(error) => show_browser_failure(&js_reason(&error)),
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
    developer: developer::Host,
    files: files::Host,
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
    activity_workspace: activity::Workspace,
    map_runtime: activity::map_runtime::MapRuntimeHandle,
    applied_preferences: Option<(UserId, ProfilePreferences)>,
    avatar_editor: Option<AvatarEditor>,
    device_browser: Option<device_browser::Browser>,
    device_fit_preview: Option<device_fit_preview::Preview>,
}

impl App {
    fn new(
        context: eframe::egui::Context,
        map_renderer: activity::WgpuMapHandle,
        map_upload_telemetry: bool,
    ) -> Result<Self, garmin_i18n::Error> {
        let translations = Translations::bundled()?;
        let intl = translations.formatter(Language::English)?;
        let shared = Rc::new(RefCell::new(State::default()));
        spawn_connection(Rc::clone(&shared), context.clone());
        let map_runtime = activity::map_runtime::MapRuntimeHandle::new(
            if context
                .plugin_opt::<activity::map_remote::RemoteMapPlugin>()
                .is_some()
            {
                BrowserMapBackend::Remote
            } else {
                BrowserMapBackend::new()
            },
            activity::map_runtime::Renderer::wgpu(map_renderer),
        )
        .with_metrics(BrowserMapMetrics {
            upload_events: map_upload_telemetry,
        });
        let activity_workspace = activity::Workspace::new(&map_runtime);
        Ok(Self {
            developer: developer::Host::new(context.clone()),
            files: files::Host::default(),
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
            activity_workspace,
            map_runtime,
            applied_preferences: None,
            avatar_editor: None,
            device_browser: None,
            device_fit_preview: None,
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
        self.activity_detail = detail;
    }

    fn show_chooser(
        &mut self,
        ui: &mut Ui,
        product_name: &str,
        loaded: bool,
        connected: bool,
        error: Option<&str>,
        notice: Option<&Notice>,
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
                    let title = format_message!(
                        &self.intl,
                        default_message: "Could not connect to the device host",
                    );
                    let detail = format_message!(
                        &self.intl,
                        default_message: "{reason} Retrying…",
                        values: { reason: error },
                    );
                    notification::show(
                        ui,
                        &notification::Props {
                            kind: notification::Kind::Error,
                            title: &title,
                            detail: Some(&detail),
                        },
                    );
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
                    notification::show(ui, &notice.props());
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
            Some(profile::Action::Select(index)) => {
                let profiles = Rc::clone(&self.profiles);
                self.select_profile(index, &profiles);
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
        product_name: &str,
        devices: &[DeviceSnapshot],
        preferences_saving: bool,
        device_catalog_loading: Option<&str>,
        notice: Option<&Notice>,
    ) {
        let profiles = Rc::clone(&self.profiles);
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
        let output =
            workspace::show(
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
                        notification::show(ui, &notice.props());
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
                            &mut self.activity_workspace,
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
                            let action = devices.iter().find(|device| &device.key == key).and_then(
                                |snapshot| {
                                    device::show_snapshot(
                                        ui,
                                        &self.intl,
                                        snapshot,
                                        device_catalog_loading == Some(key.as_str()),
                                    )
                                },
                            );
                            PageAction::Device {
                                key: key.clone(),
                                action,
                            }
                        }
                    }
                },
            );
        self.handle_page_action(output.inner, &profiles);
        self.handle_shell_action(output.action, &profiles, devices);
    }

    fn show_offline(&self, ui: &Ui, since_milliseconds: f64) {
        let seconds = Duration::try_from_secs_f64(
            (js_sys::Date::now() - since_milliseconds).max(0.0) / 1_000.0,
        )
        .unwrap_or_default()
        .as_secs();
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
            PageAction::Settings(Some(profile_settings::Action::ChoosePicture)) => {
                self.choose_profile_picture(profiles);
            }
            PageAction::Settings(Some(profile_settings::Action::UpdatePreferences(
                preferences,
            ))) => {
                self.update_preferences(preferences, profiles);
            }
            PageAction::Device {
                key,
                action: Some(device::Action::BrowseFiles),
            } => self.open_file_window(key),
            PageAction::Activities(None)
            | PageAction::Settings(None)
            | PageAction::Device { action: None, .. } => {}
        }
    }

    fn handle_device_browser_action(&mut self, action: device_browser::Action) {
        if matches!(action, device_browser::Action::Close) {
            if !device_browser_busy(&self.shared.borrow()) {
                self.device_browser = None;
            }
            return;
        }
        if device_browser_busy(&self.shared.borrow()) {
            return;
        }
        let Some(device_key) = self
            .device_browser
            .as_ref()
            .map(|browser| browser.device_key().to_owned())
        else {
            return;
        };
        self.dispatch_device_browser_action(action, device_key);
    }

    fn dispatch_device_browser_action(
        &mut self,
        action: device_browser::Action,
        device_key: String,
    ) {
        let Some(action) = dispatch_file_action(
            &self.shared,
            &self.context,
            &self.intl,
            action,
            device_key.clone(),
        ) else {
            return;
        };
        match action {
            device_browser::Action::Refresh => {
                spawn_device_catalog(self.shared.clone(), self.context.clone(), device_key);
            }
            device_browser::Action::Open(selection) => spawn_device_fit_preview(
                Rc::clone(&self.shared),
                self.context.clone(),
                device_key,
                browser_target(selection),
                &format_message!(&self.intl, default_message: "Opening the FIT file…"),
            ),
            device_browser::Action::ImportFit(selection) => {
                let Some(user_id) = self
                    .selected_profile
                    .and_then(|index| self.profiles.get(index))
                    .map(|profile| profile.user.id())
                else {
                    return;
                };
                spawn_device_fit_import(
                    Rc::clone(&self.shared),
                    self.context.clone(),
                    user_id,
                    device_key,
                    browser_target(selection),
                    &format_message!(&self.intl, default_message: "Importing the FIT file…"),
                );
            }
            _ => unreachable!("filesystem actions and close were handled above"),
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
            format_message!(
                &self.intl,
                default_message: "Could not save profile settings",
            ),
        );
    }

    fn choose_profile_picture(&self, profiles: &[ProfileSnapshot]) {
        let Some(user_id) = self
            .selected_profile
            .and_then(|index| profiles.get(index))
            .map(|profile| profile.user.id())
        else {
            return;
        };
        spawn_choose_avatar(
            Rc::clone(&self.shared),
            self.context.clone(),
            user_id,
            format_message!(&self.intl, default_message: "Could not open profile picture"),
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
            format_message!(&self.intl, default_message: "Could not load activity"),
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
                editor.submitting = true;
                spawn_import_avatar(
                    Rc::clone(&self.shared),
                    self.context.clone(),
                    editor.user_id,
                    AvatarUpload {
                        file_name: editor.file_name.clone(),
                        bytes: editor.bytes.clone(),
                        crop: AvatarCrop {
                            left: output.inner.left(),
                            top: output.inner.top(),
                            edge: output.inner.edge(),
                        },
                    },
                    format_message!(
                        &self.intl,
                        default_message: "Could not save profile picture",
                    ),
                );
            }
            Some(modal::Action::Cancel) | None => {}
        }
    }

    fn show_device_fit_preview(&mut self, ui: &mut Ui) {
        let units = self
            .selected_profile
            .and_then(|index| self.profiles.get(index))
            .map_or(garmin_model::identity::UnitSystem::Metric, |profile| {
                profile.user.profile().preferences().unit_system()
            });
        let busy = device_browser_busy(&self.shared.borrow());
        let Some(preview) = self.device_fit_preview.as_mut() else {
            return;
        };
        match preview.show(ui, &self.intl, busy, units) {
            Some(device_fit_preview::Action::Close) => self.device_fit_preview = None,
            Some(device_fit_preview::Action::Import(target)) => {
                let Some(user_id) = self
                    .selected_profile
                    .and_then(|index| self.profiles.get(index))
                    .map(|profile| profile.user.id())
                else {
                    return;
                };
                let Some(device_key) = self
                    .device_browser
                    .as_ref()
                    .map(|browser| browser.device_key().to_owned())
                else {
                    self.device_fit_preview = None;
                    return;
                };
                spawn_device_fit_import(
                    Rc::clone(&self.shared),
                    self.context.clone(),
                    user_id,
                    device_key,
                    target,
                    &format_message!(&self.intl, default_message: "Importing the FIT file…"),
                );
            }
            None => {}
        }
    }
}

impl App {
    fn apply_avatar_results(&mut self) {
        let (avatar_draft, avatar_finished) = {
            let mut state = self.shared.borrow_mut();
            (state.avatar_draft.take(), state.avatar_finished.take())
        };
        if let Some(draft) = avatar_draft {
            self.avatar_editor = Some(AvatarEditor::from_draft(&self.context, draft));
        }
        if let Some(saved) = avatar_finished {
            if saved {
                self.avatar_editor = None;
            } else if let Some(editor) = self.avatar_editor.as_mut() {
                editor.submitting = false;
            }
        }
    }

    fn apply_device_catalogs(&mut self, devices: &[DeviceSnapshot]) {
        let (device_catalog, device_browser_refresh) = {
            let mut state = self.shared.borrow_mut();
            (
                state.device_catalog.take(),
                state.device_browser_refresh.take(),
            )
        };
        if let Some((requested_key, result)) = device_catalog
            && self.files.device_key.as_deref() == Some(requested_key.as_str())
        {
            let failure_title =
                format_message!(&self.intl, default_message: "Could not browse device files");
            match result {
                Ok(catalog) => {
                    let device_name = devices
                        .iter()
                        .find(|device| device.key == catalog.device_key)
                        .map(|device| device.name.clone());
                    if let Some(device_name) = device_name {
                        match device_browser::Browser::open(catalog, &self.intl, &device_name) {
                            Ok(browser) => self.device_browser = Some(browser),
                            Err(error) => {
                                self.shared.borrow_mut().notice =
                                    Some(Notice::error(failure_title, error));
                            }
                        }
                    }
                }
                Err(error) => {
                    self.shared.borrow_mut().notice = Some(Notice::error(failure_title, error));
                }
            }
            self.context.request_repaint();
        }
        if let Some((requested_key, result)) = device_browser_refresh
            && self
                .device_browser
                .as_ref()
                .is_some_and(|browser| browser.device_key() == requested_key)
        {
            let failure_title =
                format_message!(&self.intl, default_message: "Could not browse device files");
            match result {
                Ok(catalog) => {
                    if let Err(error) = self
                        .device_browser
                        .as_mut()
                        .expect("the active browser was checked above")
                        .refresh(catalog)
                    {
                        self.shared.borrow_mut().notice = Some(Notice::error(failure_title, error));
                    }
                }
                Err(error) => {
                    self.shared.borrow_mut().notice = Some(Notice::error(failure_title, error));
                }
            }
        }
    }

    fn apply_device_fit_results(&mut self) {
        let (device_fit_preview, device_fit_import) = {
            let mut state = self.shared.borrow_mut();
            (
                state.device_fit_preview.take(),
                state.device_fit_import.take(),
            )
        };
        if let Some((requested_key, target, result)) = device_fit_preview
            && self
                .device_browser
                .as_ref()
                .is_some_and(|browser| browser.device_key() == requested_key)
        {
            match result {
                Ok(preview) => {
                    let fit_preview =
                        device_fit_preview::Preview::new(target, preview, &self.map_runtime);
                    self.device_fit_preview = Some(fit_preview);
                }
                Err(error) => {
                    let title =
                        format_message!(&self.intl, default_message: "Could not open FIT file");
                    self.shared.borrow_mut().notice = Some(Notice::error(title, error));
                }
            }
        }
        if let Some(result) = device_fit_import {
            self.device_fit_preview = None;
            self.page = Page::Activities;
            self.shared.borrow_mut().notice = Some(match result {
                Ok(DeviceFitImportOutcome::Imported { activities }) => {
                    Notice::success(format_message!(
                        &self.intl,
                        default_message: "Imported {count} activities",
                        values: { count: activities },
                    ))
                }
                Ok(DeviceFitImportOutcome::Duplicate) => Notice::information(format_message!(
                    &self.intl,
                    default_message: "This FIT file was already imported",
                )),
                Ok(DeviceFitImportOutcome::Rejected { reason }) | Err(reason) => Notice::error(
                    format_message!(&self.intl, default_message: "Could not import FIT file"),
                    reason,
                ),
            });
            self.context.request_repaint();
        }
    }

    fn apply_profile_creation(&mut self, profiles: &[ProfileSnapshot]) {
        let (created, creating, create_problem) = {
            let state = self.shared.borrow();
            (
                state.created_profile,
                state.profile_creating,
                state.profile_create_problem.clone(),
            )
        };
        if let Some(user_id) = created {
            if let Some(index) = profiles
                .iter()
                .position(|profile| profile.user.id() == user_id)
            {
                self.select_profile(index, profiles);
                self.create_profile = None;
                self.shared.borrow_mut().created_profile = None;
            }
        } else if let Some(problem) = create_problem {
            if let Some(dialog) = self.create_profile.as_mut() {
                dialog.set_problem(problem);
            }
            self.shared.borrow_mut().profile_create_problem = None;
        } else if !creating
            && let Some(dialog) = self.create_profile.as_mut()
            && dialog.is_submitting()
        {
            dialog.set_submitting(false);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        map_composition::update_diagnostics(ui.ctx());
        garmin_ui::automation::show_status(ui.ctx());
        if self.first_frame {
            self.first_frame = false;
            hide_loading_overlay();
        }
        let (
            devices,
            profiles,
            loaded,
            connected,
            error,
            notice,
            activity_detail,
            preferences_saving,
            device_catalog_loading,
            disconnected_since,
            deployment_mode,
        ) = {
            let mut state = self.shared.borrow_mut();
            state.show_device_browser_interruption(&self.intl);
            (
                Rc::clone(&state.snapshots),
                Rc::clone(&state.profiles),
                state.profiles_loaded,
                state.client.is_some(),
                state.error.clone(),
                state.notice.clone(),
                state.activity_detail.as_ref().map(Rc::clone),
                state.preferences_saving || state.avatar_saving,
                state.device_catalog_request.loading().map(str::to_owned),
                state.disconnected_since_milliseconds,
                state.deployment_mode,
            )
        };
        let product_name = match deployment_mode {
            DeploymentMode::Production => "Garmin Toolkit",
            DeploymentMode::Demo => "Garmin Toolkit Demo",
        };
        self.sync_profiles(profiles);
        self.sync_file_window_owner();
        self.apply_avatar_results();
        self.apply_device_catalogs(&devices);
        self.apply_device_fit_results();
        self.sync_activity_detail(activity_detail);
        let profiles = Rc::clone(&self.profiles);
        if matches!(&self.page, Page::Device(key) if !devices.iter().any(|device| &device.key == key))
        {
            self.device_browser = None;
            self.device_fit_preview = None;
            self.page = Page::Activities;
        }
        if let Some(profile) = self.selected_profile.and_then(|index| profiles.get(index)) {
            self.selected_activity = self
                .selected_activity
                .min(profile.activities.len().saturating_sub(1));
        }
        self.apply_profile_creation(&profiles);
        self.sync_selected_preferences();
        let offline = loaded && disconnected_since.is_some();
        if offline {
            ui.disable();
        }
        match self.selected_profile {
            Some(_) => self.show_application(
                ui,
                product_name,
                devices.as_slice(),
                preferences_saving,
                device_catalog_loading.as_deref(),
                notice.as_ref(),
            ),
            None => self.show_chooser(
                ui,
                product_name,
                loaded,
                connected,
                error.as_deref(),
                notice.as_ref(),
            ),
        }
        self.sync_file_window_owner();
        if !offline {
            self.show_create_profile(ui);
            self.show_avatar_editor(ui);
            self.show_device_fit_preview(ui);
        }
        if let Some(since) = disconnected_since.filter(|_| loaded) {
            self.show_offline(ui, since);
        }
        automation::dispatch_menu(ui.ctx());
        self.developer.update(&self.intl);
        self.update_file_window();
        developer::update_logs(ui.ctx(), self.shared.borrow().logs.clone());
    }
}

enum PageAction {
    Activities(Option<activity::Action>),
    Settings(Option<profile_settings::Action>),
    Device {
        key: String,
        action: Option<device::Action>,
    },
}

fn show_activities(
    ui: &mut Ui,
    intl: &Intl,
    profile: &ProfileSnapshot,
    presentations: &[activity::Presentation],
    selected: usize,
    detail: Option<&ActivityDetailSnapshot>,
    workspace: &mut activity::Workspace,
) -> Option<activity::Action> {
    let items = presentations
        .iter()
        .map(activity::Presentation::item_props)
        .collect::<Vec<_>>();
    let detail = detail.filter(|detail| {
        profile
            .activities
            .get(selected)
            .is_some_and(|activity| activity.id == detail.id)
    });
    let recording_key = detail.map(|detail| detail.id.to_string());
    let no_route = format_message!(intl, default_message: "No recorded route");
    let no_activities = format_message!(intl, default_message: "No activities yet");
    let select_activity = format_message!(intl, default_message: "Select an activity");
    workspace.show(
        ui,
        intl,
        &activity::WorkspaceProps {
            items: &items,
            presentations,
            selected: (!items.is_empty()).then_some(selected),
            recording: detail.map(|detail| &detail.recording),
            recording_key: recording_key.as_deref(),
            units: profile.user.profile().preferences().unit_system(),
            empty_list: &no_activities,
            empty_detail: &select_activity,
            no_route: &no_route,
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
        picture_enabled: true,
        disabled,
    }
}

fn set_profile_preferences(snapshot: &mut ProfileSnapshot, preferences: ProfilePreferences) {
    let mut profile = snapshot.user.profile().clone();
    profile.replace_preferences(preferences);
    snapshot.user.replace_profile(profile);
}

#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "profile readiness and three independent asynchronous operations may overlap"
)]
struct State {
    client: Option<ApplicationServiceClient>,
    logs: Option<garmin_service_api::logging::LogServiceClient>,
    deployment_mode: DeploymentMode,
    snapshots: Rc<Vec<DeviceSnapshot>>,
    profiles: Rc<Vec<ProfileSnapshot>>,
    profiles_loaded: bool,
    activity_detail: Option<Rc<ActivityDetailSnapshot>>,
    requested_activity: Option<garmin_model::observation::ObservationId>,
    preferences_saving: bool,
    avatar_saving: bool,
    avatar_draft: Option<AvatarDraft>,
    avatar_finished: Option<bool>,
    device_catalog: Option<(String, Result<DeviceCatalogSnapshot, String>)>,
    device_catalog_request: files::catalog::Request,
    active_device_browser_operation: Option<DeviceBrowserOperationToken>,
    device_browser_interrupted: bool,
    next_device_browser_operation: u64,
    connection_generation: u64,
    device_browser_refresh: Option<(String, Result<DeviceCatalogSnapshot, String>)>,
    device_fit_preview: Option<(
        String,
        DeviceBrowserTarget,
        Result<DeviceFitPreview, String>,
    )>,
    device_fit_import: Option<Result<DeviceFitImportOutcome, String>>,
    profile_creating: bool,
    profile_create_problem: Option<String>,
    created_profile: Option<UserId>,
    notice: Option<Notice>,
    error: Option<String>,
    disconnected_since_milliseconds: Option<f64>,
}

impl State {
    fn interrupt_device_browser_operation(&mut self) -> bool {
        let interrupted = self.active_device_browser_operation.take().is_some();
        self.device_browser_interrupted |= interrupted;
        interrupted
    }

    fn show_device_browser_interruption(&mut self, intl: &Intl) {
        if std::mem::take(&mut self.device_browser_interrupted) {
            let explanation = format_message!(
                intl,
                default_message: "The connection was lost, so the result is unknown. The operation wasn’t retried.",
            );
            let instruction = format_message!(
                intl,
                default_message: "Refresh the files view and check the result before trying again.",
            );
            self.notice = Some(Notice::error(
                format_message!(intl, default_message: "Device operation interrupted"),
                format!("{explanation}\n{instruction}"),
            ));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeviceBrowserOperationToken {
    connection_generation: u64,
    operation: u64,
}

#[derive(Clone)]
struct Notice {
    kind: notification::Kind,
    title: String,
    detail: Option<String>,
}

impl Notice {
    const fn information(title: String) -> Self {
        Self {
            kind: notification::Kind::Information,
            title,
            detail: None,
        }
    }

    const fn success(title: String) -> Self {
        Self {
            kind: notification::Kind::Success,
            title,
            detail: None,
        }
    }

    fn error(title: String, detail: impl fmt::Display) -> Self {
        Self {
            kind: notification::Kind::Error,
            title,
            detail: Some(notification::normalize_error_detail(&detail.to_string())),
        }
    }

    fn props(&self) -> notification::Props<'_> {
        notification::Props {
            kind: self.kind,
            title: &self.title,
            detail: self.detail.as_deref(),
        }
    }
}

struct AvatarDraft {
    user_id: UserId,
    file_name: String,
    bytes: Vec<u8>,
    image: ColorImage,
    source_size: [u32; 2],
}

struct AvatarEditor {
    user_id: UserId,
    file_name: String,
    bytes: Vec<u8>,
    texture: TextureHandle,
    source_size: [u32; 2],
    crop: image_crop::State,
    submitting: bool,
}

impl AvatarEditor {
    fn from_draft(context: &eframe::egui::Context, draft: AvatarDraft) -> Self {
        let texture = context.load_texture(
            format!("profile-picture-preview/{}", draft.file_name),
            draft.image,
            TextureOptions::LINEAR,
        );
        Self {
            user_id: draft.user_id,
            file_name: draft.file_name,
            bytes: draft.bytes,
            texture,
            source_size: draft.source_size,
            crop: image_crop::State::default(),
            submitting: false,
        }
    }
}

fn spawn_choose_avatar(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    user_id: UserId,
    failure: String,
) {
    spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Profile picture", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
            .await
        else {
            return;
        };
        if file.inner().size() > MAX_AVATAR_UPLOAD_BYTES {
            shared.borrow_mut().notice = Some(Notice::error(
                failure,
                "profile picture exceeds the 10 MiB limit".to_owned(),
            ));
            context.request_repaint();
            return;
        }
        let file_name = file.file_name();
        let bytes = file.read().await;
        let result = avatar_draft(user_id, file_name, bytes);
        let mut state = shared.borrow_mut();
        match result {
            Ok(draft) => state.avatar_draft = Some(draft),
            Err(error) => state.notice = Some(Notice::error(failure, error)),
        }
        drop(state);
        context.request_repaint();
    });
}

fn avatar_draft(user_id: UserId, file_name: String, bytes: Vec<u8>) -> Result<AvatarDraft, String> {
    const MAX_AVATAR_BYTES: usize = 10 * 1024 * 1024;
    const MAX_AVATAR_DIMENSION: u32 = 4_096;
    const MAX_DECODE_ALLOCATION: u64 = 128 * 1024 * 1024;
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err("profile picture exceeds the 10 MiB limit".to_owned());
    }
    let format = image::guess_format(&bytes).map_err(|error| error.to_string())?;
    if !matches!(
        format,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
    ) {
        return Err("profile picture must be a PNG, JPEG, or WebP image".to_owned());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_AVATAR_DIMENSION);
    limits.max_image_height = Some(MAX_AVATAR_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation =
        image::ImageDecoder::orientation(&mut decoder).map_err(|error| error.to_string())?;
    let mut oriented =
        image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    oriented.apply_orientation(orientation);
    let width = usize::try_from(oriented.width()).map_err(|error| error.to_string())?;
    let height = usize::try_from(oriented.height()).map_err(|error| error.to_string())?;
    let rgba = oriented.to_rgba8();
    Ok(AvatarDraft {
        user_id,
        file_name,
        bytes,
        image: ColorImage::from_rgba_unmultiplied([width, height], rgba.as_raw()),
        source_size: [oriented.width(), oriented.height()],
    })
}

fn spawn_import_avatar(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    user_id: UserId,
    upload: AvatarUpload,
    failure: String,
) {
    let Some(client) = shared.borrow().client.clone() else {
        return;
    };
    {
        let mut state = shared.borrow_mut();
        state.avatar_saving = true;
        state.notice = None;
    }
    spawn_local(async move {
        let result = client
            .import_avatar(user_id, upload)
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain));
        let mut state = shared.borrow_mut();
        state.avatar_saving = false;
        state.avatar_finished = Some(result.is_ok());
        match result {
            Ok(avatar) => {
                if let Some(profile) = Rc::make_mut(&mut state.profiles)
                    .iter_mut()
                    .find(|profile| profile.user.id() == user_id)
                {
                    profile.avatar = Some(avatar);
                }
            }
            Err(error) => state.notice = Some(Notice::error(failure, error)),
        }
        drop(state);
        context.request_repaint();
    });
}

fn dispatch_file_action(
    shared: &Rc<RefCell<State>>,
    context: &eframe::egui::Context,
    intl: &Intl,
    action: device_browser::Action,
    device_key: String,
) -> Option<device_browser::Action> {
    match action {
        device_browser::Action::Download(selection) => spawn_device_download(
            Rc::clone(shared),
            context.clone(),
            device_key,
            browser_target(selection),
            &format_message!(intl, default_message: "Downloading from the device…"),
            format_message!(intl, default_message: "Download started"),
            format_message!(intl, default_message: "Download failed"),
        ),
        device_browser::Action::Upload {
            storage_id,
            directory,
        } => spawn_device_upload(
            Rc::clone(shared),
            context.clone(),
            device_key,
            storage_id,
            directory,
            DeviceUploadCopy {
                progress: format_message!(
                    intl,
                    default_message: "Uploading to the device…",
                ),
                complete: format_message!(intl, default_message: "Upload complete"),
                failure: format_message!(intl, default_message: "Upload failed"),
                too_large: format_message!(
                    intl,
                    default_message: "The selected file exceeds the 512 MiB device-browser limit",
                ),
            },
        ),
        device_browser::Action::CreateDirectory {
            storage_id,
            parent,
            name,
        } => spawn_device_browser_request(
            Rc::clone(shared),
            context.clone(),
            DeviceBrowserRequest::CreateDirectory {
                device_key,
                storage_id,
                parent,
                name,
            },
            &format_message!(intl, default_message: "Creating the folder…"),
            format_message!(intl, default_message: "Folder created"),
            format_message!(intl, default_message: "Could not create folder"),
        ),
        device_browser::Action::Remove(selection) => spawn_device_browser_request(
            Rc::clone(shared),
            context.clone(),
            DeviceBrowserRequest::Remove {
                device_key,
                target: browser_target(selection),
            },
            &format_message!(intl, default_message: "Removing the selected item…"),
            format_message!(intl, default_message: "Item removed"),
            format_message!(intl, default_message: "Could not remove item"),
        ),
        other => return Some(other),
    }
    None
}

fn browser_target(selection: device_browser::Selection) -> DeviceBrowserTarget {
    DeviceBrowserTarget {
        storage_id: selection.storage_id,
        path: selection.path,
        kind: selection.kind,
    }
}

fn spawn_device_browser_request(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    request: DeviceBrowserRequest,
    progress: &str,
    complete: String,
    failure: String,
) {
    let Some((client, token)) = begin_device_browser_request(&shared, progress) else {
        return;
    };
    spawn_local(execute_device_browser_request(
        shared, context, client, token, request, complete, failure,
    ));
}

fn spawn_device_download(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    device_key: String,
    target: DeviceBrowserTarget,
    progress: &str,
    complete: String,
    failure: String,
) {
    let Some((client, token)) = begin_device_browser_request(&shared, progress) else {
        return;
    };
    spawn_local(async move {
        let result = client
            .prepare_device_browser_download(device_key, target)
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain))
            .map_err(|error| error.to_string());
        let mut state = shared.borrow_mut();
        if !finish_device_browser_request(&mut state, token) {
            return;
        }
        state.notice = Some(
            match result.and_then(|ticket| trigger_browser_download(&ticket)) {
                Ok(()) => Notice::success(complete),
                Err(error) => Notice::error(failure, error),
            },
        );
        drop(state);
        context.request_repaint();
    });
}

fn spawn_device_upload(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    device_key: String,
    storage_id: String,
    directory: Utf8PathBuf,
    copy: DeviceUploadCopy,
) {
    let Some((client, token)) = begin_device_browser_request(&shared, &copy.progress) else {
        return;
    };
    spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new().pick_file().await else {
            let mut state = shared.borrow_mut();
            if finish_device_browser_request(&mut state, token) {
                state.notice = None;
            }
            context.request_repaint();
            return;
        };
        // A reconnect or changed parent session while the picker was open invalidates it.
        if shared.borrow().active_device_browser_operation != Some(token) {
            return;
        }
        if file.inner().size() > MAX_DEVICE_BROWSER_TRANSFER_BYTES {
            let mut state = shared.borrow_mut();
            if finish_device_browser_request(&mut state, token) {
                state.notice = Some(Notice::error(copy.failure, copy.too_large));
            }
            context.request_repaint();
            return;
        }
        let file_name = file.file_name();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "File.size is a nonnegative integer, checked above against the 512 MiB limit"
        )]
        let size = file.inner().size() as u32;
        let (sender, receiver) = remoc::rch::io::sized::<remoc::codec::Default>(u64::from(size));
        let request = DeviceBrowserUpload {
            device_key: device_key.clone(),
            storage_id,
            directory,
            file_name,
            size: u64::from(size),
        };
        let rpc = async move {
            client
                .upload_device_browser_file(request, receiver)
                .await
                .map_err(ConnectError::call)
                .and_then(|result| result.map_err(ConnectError::domain))
                .map_err(|error| error.to_string())
        };
        let stream = stream_browser_upload(file.inner().clone(), size, sender);
        let result = future::try_join(rpc, stream)
            .await
            .map(|(catalog, ())| catalog);
        let mut state = shared.borrow_mut();
        if !finish_device_browser_request(&mut state, token) {
            return;
        }
        match result {
            Ok(catalog) => {
                state.device_browser_refresh = Some((device_key, Ok(catalog)));
                state.notice = Some(Notice::success(copy.complete));
            }
            Err(error) => state.notice = Some(Notice::error(copy.failure, error)),
        }
        drop(state);
        context.request_repaint();
    });
}

async fn stream_browser_upload(
    file: web_sys::File,
    size: u32,
    mut sender: remoc::rch::io::Sender,
) -> Result<(), String> {
    const CHUNK_BYTES: u32 = 1024 * 1024;
    let mut offset = 0_u32;
    while offset < size {
        let end = offset.saturating_add(CHUNK_BYTES).min(size);
        let blob = file
            .slice_with_f64_and_f64(f64::from(offset), f64::from(end))
            .map_err(|error| format!("could not read the selected file: {}", js_reason(&error)))?;
        let buffer = JsFuture::from(blob.array_buffer())
            .await
            .map_err(|error| format!("could not read the selected file: {}", js_reason(&error)))?;
        let array = js_sys::Uint8Array::new(&buffer);
        let mut bytes = vec![0_u8; array.length() as usize];
        array.copy_to(&mut bytes);
        sender
            .write_all(&bytes)
            .await
            .map_err(|error| format!("could not send the selected file: {error}"))?;
        offset = end;
    }
    sender
        .shutdown()
        .await
        .map_err(|error| format!("could not finish sending the selected file: {error}"))
}

struct DeviceUploadCopy {
    progress: String,
    complete: String,
    failure: String,
    too_large: String,
}

fn spawn_device_fit_preview(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    device_key: String,
    target: DeviceBrowserTarget,
    progress: &str,
) {
    let Some((client, token)) = begin_device_browser_request(&shared, progress) else {
        return;
    };
    spawn_local(async move {
        let result = client
            .device_fit_preview(device_key.clone(), target.clone())
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain))
            .map_err(|error| error.to_string());
        let mut state = shared.borrow_mut();
        if !finish_device_browser_request(&mut state, token) {
            return;
        }
        state.notice = None;
        state.device_fit_preview = Some((device_key, target, result));
        drop(state);
        context.request_repaint();
    });
}

fn spawn_device_fit_import(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    user_id: UserId,
    device_key: String,
    target: DeviceBrowserTarget,
    progress: &str,
) {
    let Some((client, token)) = begin_device_browser_request(&shared, progress) else {
        return;
    };
    spawn_local(async move {
        let result = client
            .import_device_fit(user_id, device_key, target)
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain))
            .map_err(|error| error.to_string());
        let profiles = if result.is_ok() {
            client.profiles().await.ok().and_then(Result::ok)
        } else {
            None
        };
        let mut state = shared.borrow_mut();
        if !finish_device_browser_request(&mut state, token) {
            return;
        }
        if let Some(profiles) = profiles {
            state.profiles = Rc::new(profiles);
        }
        state.device_fit_import = Some(result);
        drop(state);
        context.request_repaint();
    });
}

fn begin_device_browser_request(
    shared: &RefCell<State>,
    progress: &str,
) -> Option<(ApplicationServiceClient, DeviceBrowserOperationToken)> {
    let mut state = shared.borrow_mut();
    if device_browser_busy(&state) {
        return None;
    }
    let client = state.client.clone()?;
    let token = DeviceBrowserOperationToken {
        connection_generation: state.connection_generation,
        operation: state.next_device_browser_operation,
    };
    state.next_device_browser_operation = state.next_device_browser_operation.wrapping_add(1);
    state.active_device_browser_operation = Some(token);
    state.device_browser_interrupted = false;
    state.notice = Some(Notice::information(progress.to_owned()));
    Some((client, token))
}

fn device_browser_busy(state: &State) -> bool {
    state.active_device_browser_operation.is_some()
}

fn finish_device_browser_request(state: &mut State, token: DeviceBrowserOperationToken) -> bool {
    if state.active_device_browser_operation != Some(token)
        || state.connection_generation != token.connection_generation
    {
        return false;
    }
    state.active_device_browser_operation = None;
    true
}

async fn execute_device_browser_request(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    client: ApplicationServiceClient,
    token: DeviceBrowserOperationToken,
    request: DeviceBrowserRequest,
    complete: String,
    failure: String,
) {
    let device_key = browser_request_device_key(&request).to_owned();
    let result = client
        .device_browser(request)
        .await
        .map_err(ConnectError::call)
        .and_then(|result| result.map_err(ConnectError::domain))
        .map_err(|error| error.to_string());
    let mut state = shared.borrow_mut();
    if !finish_device_browser_request(&mut state, token) {
        return;
    }
    match result {
        Ok(catalog) => {
            state.device_browser_refresh = Some((device_key, Ok(catalog)));
            state.notice = Some(Notice::success(complete));
        }
        Err(error) => state.notice = Some(Notice::error(failure, error)),
    }
    drop(state);
    context.request_repaint();
}

fn trigger_browser_download(ticket: &DeviceBrowserDownloadTicket) -> Result<(), String> {
    (|| -> Result<(), JsValue> {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| js_error("the browser document is unavailable"))?;
        let anchor = document
            .create_element("a")?
            .dyn_into::<web_sys::HtmlAnchorElement>()?;
        anchor.set_href(&format!("device-download/{}", ticket.token));
        anchor.set_download(&ticket.file_name);
        anchor.set_attribute("hidden", "")?;
        let body = document
            .body()
            .ok_or_else(|| js_error("the browser document body is unavailable"))?;
        body.append_child(&anchor)?;
        anchor.click();
        anchor.remove();
        Ok(())
    })()
    .map_err(|error| format!("could not start the download: {}", js_reason(&error)))
}

fn browser_request_device_key(request: &DeviceBrowserRequest) -> &str {
    match request {
        DeviceBrowserRequest::CreateDirectory { device_key, .. }
        | DeviceBrowserRequest::Remove { device_key, .. } => device_key,
    }
}

fn spawn_device_catalog(
    shared: Rc<RefCell<State>>,
    context: eframe::egui::Context,
    device_key: String,
) {
    let (client, connection_generation, request) = {
        let mut state = shared.borrow_mut();
        let Some(client) = state.client.clone() else {
            return;
        };
        let Some(request) = state.device_catalog_request.begin(&device_key) else {
            return;
        };
        state.device_catalog = None;
        (client, state.connection_generation, request)
    };
    spawn_local(async move {
        let requested_key = device_key.clone();
        let result = client
            .device_catalog(device_key)
            .await
            .map_err(ConnectError::call)
            .and_then(|result| result.map_err(ConnectError::domain))
            .map_err(|error| error.to_string());
        let mut state = shared.borrow_mut();
        if state.connection_generation != connection_generation
            || !state.device_catalog_request.finish(request)
        {
            return;
        }
        state.device_catalog = Some((requested_key, result));
        context.request_repaint();
    });
}

fn spawn_connection(shared: Rc<RefCell<State>>, context: eframe::egui::Context) {
    developer::forward(shared.clone(), context.clone());
    spawn_local(async move {
        let mut retry_milliseconds = 1_000;
        loop {
            match connect().await {
                Ok((client, mut snapshots, profiles, deployment_mode)) => {
                    retry_milliseconds = 1_000;
                    shared.borrow_mut().logs = client.logs().await.ok();
                    garmin_ui::developer::reconnect(&context);
                    garmin_ui::developer::state(&context)
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .debug = format!(
                        "Version: {}\nHost: HASS browser\nMode: {:?}",
                        env!("CARGO_PKG_VERSION"),
                        deployment_mode
                    );
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
                    let _control = control::register(&client, &context).await;
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
    state.logs = None;
    state.interrupt_device_browser_operation();
    state.device_catalog_request.invalidate();
    state.connection_generation = state.connection_generation.wrapping_add(1);
    if state.profiles_loaded && state.disconnected_since_milliseconds.is_none() {
        state.disconnected_since_milliseconds = Some(js_sys::Date::now());
    }
    state.error = Some(notification::normalize_error_detail(&error.to_string()));
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
    failure: String,
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
                state.notice = Some(Notice::error(failure, error));
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
    failure: String,
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
                    state.notice = Some(Notice::error(failure.clone(), missing));
                }
                Err(error) => state.notice = Some(Notice::error(failure, error)),
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
    let client = connect_client().await?;
    let deployment_mode = client.deployment_mode().await.map_err(ConnectError::call)?;
    let snapshots = client.watch_devices().await.map_err(ConnectError::call)?;
    let profiles = client
        .profiles()
        .await
        .map_err(ConnectError::call)?
        .map_err(ConnectError::domain)?;
    Ok((client, snapshots, profiles, deployment_mode))
}

async fn connect_client() -> Result<ApplicationServiceClient, ConnectError> {
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
    Ok(client)
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

fn hide_loading_overlay() {
    dispatch_loader_event("garmin-toolkit-ready", &JsValue::NULL);
}

fn install_browser_panic_surface() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        show_browser_failure(&panic.to_string());
        previous(panic);
    }));
}

/// Report a fatal application error through the loader's terminal failure transition.
#[wasm_bindgen]
pub fn show_browser_failure(message: &str) {
    dispatch_loader_event("garmin-toolkit-failure", &JsValue::from_str(message));
}

fn dispatch_loader_event(name: &str, detail: &JsValue) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let options = web_sys::CustomEventInit::new();
    options.set_detail(detail);
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict(name, &options) {
        let _ignored = window.dispatch_event(&event);
    }
}

fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}

fn js_reason(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}
