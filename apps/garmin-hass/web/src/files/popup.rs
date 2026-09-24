//! Independent file view with window-local pickers and the existing RPC controllers.
use super::{Command, Request, Snapshot};
use crate::window_client::Client;
use eframe::egui::{self, Context};
use garmin_i18n::{Intl, Language, Translations};
use garmin_model::identity::LanguagePreference;
use garmin_service_api::ApplicationService as _;
use garmin_ui::{device_browser, notification};
use std::{cell::RefCell, rc::Rc, time::Duration};

pub fn create(context: Context, session: &str) -> Result<Box<dyn eframe::App>, String> {
    let translations = Translations::bundled().map_err(|error| error.to_string())?;
    let intl = translations
        .formatter(Language::English)
        .map_err(|error| error.to_string())?;
    let shared = Rc::new(RefCell::new(crate::State::default()));
    let connection = Client::new(context.clone(), session)?;
    connect(shared.clone(), context.clone());
    crate::developer::forward(shared.clone(), context);
    Ok(Box::new(Files {
        connection,
        shared,
        translations,
        intl,
        snapshot: None,
        browser: None,
        refresh_pending: false,
        first_frame: true,
    }))
}

struct Files {
    connection: Client<Command, Snapshot>,
    shared: Rc<RefCell<crate::State>>,
    translations: Translations,
    intl: Intl,
    snapshot: Option<Snapshot>,
    browser: Option<device_browser::Browser>,
    refresh_pending: bool,
    first_frame: bool,
}

impl Files {
    fn receive(&mut self, context: &Context, now: f64) {
        for snapshot in self.connection.receive(now) {
            self.apply_snapshot(context, snapshot);
        }
        if !self.connection.link.connected(now)
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.available)
        {
            let mut state = self.shared.borrow_mut();
            if state.interrupt_device_browser_operation() {
                state.connection_generation += 1;
            }
        }
        let refresh = self.shared.borrow_mut().device_browser_refresh.take();
        if let Some((key, result)) = refresh
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.identity.device_key == key)
        {
            match result {
                Ok(catalog) => {
                    if let Some(browser) = &mut self.browser
                        && let Err(error) = browser.refresh(catalog)
                    {
                        self.connection.link.error = Some(error);
                    }
                    self.refresh_pending = true;
                }
                Err(error) => self.connection.link.error = Some(error),
            }
        }
        if self.refresh_pending
            && self.connection.link.connected(now)
            && !self.connection.link.pending()
            && let Some(snapshot) = &self.snapshot
            && snapshot.available
            && !snapshot.busy
        {
            self.connection.send(
                Command {
                    identity: snapshot.identity.clone(),
                    action: Request::Refresh,
                },
                now,
            );
            self.refresh_pending = false;
        }
    }

    fn apply_snapshot(&mut self, context: &Context, snapshot: Snapshot) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|previous| previous.identity != snapshot.identity)
        {
            let mut state = self.shared.borrow_mut();
            state.connection_generation += 1;
            state.interrupt_device_browser_operation();
            state.device_browser_refresh = None;
            self.refresh_pending = false;
        }
        if self
            .snapshot
            .as_ref()
            .is_none_or(|previous| previous.language != snapshot.language)
        {
            let language = match snapshot.language {
                LanguagePreference::English => Language::English,
                LanguagePreference::Czech => Language::Czech,
            };
            if let Ok(intl) = self.translations.formatter(language) {
                self.intl = intl;
            }
        }
        if context.global_style().visuals.dark_mode != snapshot.dark {
            context.set_visuals(if snapshot.dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
        }
        if self
            .snapshot
            .as_ref()
            .is_none_or(|previous| previous.catalog != snapshot.catalog)
        {
            if let Some(catalog) = &snapshot.catalog {
                let result = if let Some(browser) = &mut self.browser {
                    browser.refresh(catalog.clone())
                } else {
                    device_browser::Browser::open(catalog.clone(), &self.intl, &snapshot.name)
                        .map(|browser| self.browser = Some(browser))
                };
                if let Err(error) = result {
                    self.connection.link.error = Some(error);
                }
            } else {
                self.browser = None;
            }
        }
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_title(&format!("Device files · {}", snapshot.name));
        }
        self.snapshot = Some(snapshot);
    }

    fn action(&mut self, context: &Context, action: device_browser::Action, now: f64) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if matches!(action, device_browser::Action::Close) {
            if let Some(window) = web_sys::window() {
                let _ = window.close();
            }
            return;
        }
        if matches!(action, device_browser::Action::Refresh) {
            self.connection.send(
                Command {
                    identity: snapshot.identity.clone(),
                    action: Request::Refresh,
                },
                now,
            );
            return;
        }
        let Some(action) = crate::dispatch_file_action(
            &self.shared,
            context,
            &self.intl,
            action,
            snapshot.identity.device_key.clone(),
        ) else {
            return;
        };
        self.connection.send(
            Command {
                identity: snapshot.identity.clone(),
                action: Request::Action(action),
            },
            now,
        );
    }
}

impl eframe::App for Files {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            crate::hide_loading_overlay();
        }
        let context = ui.ctx().clone();
        let now = ui.input(|input| input.time);
        self.receive(&context, now);
        let parent_connected = self.connection.link.connected(now);
        let (connected, busy, notice) = {
            let mut state = self.shared.borrow_mut();
            state.show_device_browser_interruption(&self.intl);
            (
                state.client.is_some(),
                crate::device_browser_busy(&state),
                state.notice.clone(),
            )
        };
        if !parent_connected {
            ui.label(
                "App tab is not responding. Return to it, or reopen device files from the app.",
            );
        }
        if !connected {
            ui.label("Connecting to the device service…");
        }
        if let Some(error) = &self.connection.link.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(notice) = notice {
            notification::show(ui, &notice.props());
        }
        let available = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.available && !snapshot.busy);
        if let Some(snapshot) = &self.snapshot {
            if !snapshot.available {
                ui.label("Device or profile unavailable. Reopen device files from the app.");
            }
            if let Some(notice) = &snapshot.notice {
                ui.label(notice);
            }
        }
        let action = if let Some(browser) = &mut self.browser {
            ui.add_enabled_ui(
                parent_connected
                    && connected
                    && available
                    && !busy
                    && !self.connection.link.pending()
                    && !self.refresh_pending,
                |ui| browser.show(ui, &self.intl),
            )
            .inner
        } else {
            ui.label(if self.snapshot.as_ref().is_none_or(|snapshot| snapshot.busy) { "Waiting for the device catalogue…" } else { "Device catalogue unavailable. Close this window and choose Browse files to retry." });
            None
        };
        if let Some(action) = action {
            self.action(&context, action, now);
        }
        context.request_repaint_after(Duration::from_millis(250));
    }
}

fn connect(shared: Rc<RefCell<crate::State>>, context: Context) {
    wasm_bindgen_futures::spawn_local(async move {
        loop {
            let result: Result<(), String> = async {
                let client = crate::connect_client()
                    .await
                    .map_err(|error| error.to_string())?;
                let logs = client.logs().await.map_err(|error| error.to_string())?;
                {
                    let mut state = shared.borrow_mut();
                    state.client = Some(client.clone());
                    state.logs = Some(logs);
                }
                context.request_repaint();
                loop {
                    crate::wait_milliseconds(1_000).await;
                    crate::heartbeat(&client)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            .await;
            if let Err(error) = result {
                crate::set_connection_error(&shared, error);
            }
            context.request_repaint();
            crate::wait_milliseconds(1_000).await;
        }
    });
}
