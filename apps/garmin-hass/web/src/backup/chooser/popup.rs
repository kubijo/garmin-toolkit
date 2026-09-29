//! Window-local chooser state; the parent tab performs all filesystem and backup work.
use super::protocol::{Command, Request, Snapshot};
use crate::window_client::Client;
use eframe::egui::{self, Context};
use garmin_i18n::{Intl, Language, Translations, format_message};
use garmin_model::identity::LanguagePreference;
use garmin_service_api::files::Operation;
use garmin_ui::device_browser::chooser::{self, Chooser};
use std::time::Duration;

pub fn create(context: Context, session: &str) -> Result<Box<dyn eframe::App>, String> {
    let translations = Translations::bundled().map_err(|error| error.to_string())?;
    let intl = translations
        .formatter_for_client(Language::English)
        .map_err(|error| error.to_string())?;
    Ok(Box::new(Popup {
        connection: Client::new(context, session)?,
        translations,
        intl,
        snapshot: None,
        chooser: None,
        applied_revision: None,
        restore: None,
        first_frame: true,
    }))
}

struct Popup {
    connection: Client<Command, Snapshot>,
    translations: Translations,
    intl: Intl,
    snapshot: Option<Snapshot>,
    chooser: Option<Chooser>,
    applied_revision: Option<String>,
    restore: Option<chooser::ViewState>,
    first_frame: bool,
}

impl Popup {
    fn apply(&mut self, context: &Context, snapshot: Snapshot) {
        let initial = self
            .snapshot
            .as_ref()
            .is_none_or(|previous| previous.identity != snapshot.identity);
        if initial {
            self.restore.clone_from(&snapshot.view);
            self.chooser = Some(Chooser::new(
                snapshot.operation,
                if snapshot.operation == Operation::Save {
                    "garmin-backup.tar.zst".into()
                } else {
                    String::new()
                },
            ));
            self.applied_revision = None;
        }
        if self.snapshot.as_ref().is_none_or(|previous| {
            previous.preferences.language() != snapshot.preferences.language()
        }) {
            let language = match snapshot.preferences.language() {
                LanguagePreference::English => Language::English,
                LanguagePreference::Czech => Language::Czech,
            };
            if let Ok(intl) = self.translations.formatter_for_client(language) {
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
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_title(&chooser::title(&self.intl, snapshot.operation));
        }
        if let Some(chooser) = &mut self.chooser {
            chooser.set_show_hidden_files(snapshot.preferences.show_hidden_files());
            if !snapshot.busy
                && self.applied_revision.as_ref() != Some(&snapshot.revision)
                && let Some(directory) = &snapshot.directory
            {
                match directory {
                    Ok(directory) => chooser.loaded(directory.clone()),
                    Err(error) => chooser.failed(error.clone()),
                }
                self.applied_revision = Some(snapshot.revision.clone());
                if let Some(view) = self.restore.take() {
                    chooser.restore_view(view);
                }
            }
        }
        self.snapshot = Some(snapshot);
    }
}

impl eframe::App for Popup {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            crate::hide_loading_overlay();
        }
        let context = ui.ctx().clone();
        let now = ui.input(|input| input.time);
        for snapshot in self.connection.receive(now) {
            self.apply(&context, snapshot);
        }
        let connected = self.connection.link.connected(now);
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !connected || !snapshot.available)
        {
            garmin_ui::typography::body(
                ui,
                &format_message!(&self.intl,
                default_message: "Return to the app tab to reconnect, or reopen the file chooser."),
            );
        }
        if let Some(error) = &self.connection.link.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let (Some(chooser), Some(snapshot)) = (&mut self.chooser, &self.snapshot) {
            if connected
                && snapshot.preferences.inline_file_windows()
                && !self.connection.link.pending()
            {
                self.connection.send(
                    Command {
                        identity: snapshot.identity.clone(),
                        revision: snapshot.revision.clone(),
                        action: Request::Embed(Box::new(chooser.view_state())),
                    },
                    now,
                );
                context.request_repaint_after(Duration::from_millis(100));
                return;
            }
            let busy = snapshot.busy || self.connection.link.pending();
            let action = ui
                .add_enabled_ui(connected && snapshot.available, |ui| {
                    chooser.contents(ui, &self.intl, busy)
                })
                .inner;
            if let Some(action) = action {
                if matches!(action, chooser::Action::Cancel) {
                    if let Some(window) = web_sys::window() {
                        let _ = window.close();
                    }
                } else {
                    self.connection.send(
                        Command {
                            identity: snapshot.identity.clone(),
                            revision: snapshot.revision.clone(),
                            action: Request::Action(action),
                        },
                        now,
                    );
                }
            }
        } else {
            ui.spinner();
            ui.label(format_message!(&self.intl, default_message: "Loading files…"));
        }
        context.request_repaint_after(Duration::from_millis(250));
    }
}
