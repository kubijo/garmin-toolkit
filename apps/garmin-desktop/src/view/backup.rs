use super::*;
use garmin_ui::backup::{Action, State};

impl Desktop {
    pub(super) fn backup_enabled(&self) -> bool {
        self.selected_profile
            .and_then(|index| self.profiles.get(index))
            .is_some_and(|profile| profile.user.role() == garmin_model::identity::Role::Owner)
    }

    pub(super) fn handle_backup_action(&mut self, action: Option<Action>) {
        match action {
            Some(Action::Clear) => self.backup.clear(),
            Some(Action::Cancel) => self.backup.cancel(),
            Some(Action::Approve) if self.backup_enabled() => self.backup.approve(),
            Some(Action::Backup | Action::Restore) if self.backup_enabled() => {
                let actor = UserContext::new(
                    self.profiles[self.selected_profile.expect("selected owner")]
                        .user
                        .id(),
                );
                let dialog = rfd::FileDialog::new().add_filter("Garmin backup", &["zst"]);
                let job = if action == Some(Action::Backup) {
                    dialog
                        .set_file_name("garmin-backup.tar.zst")
                        .save_file()
                        .map(crate::backup::Job::Backup)
                } else {
                    dialog.pick_file().map(crate::backup::Job::Restore)
                };
                if let Some(job) = job {
                    self.backup.start(actor, self.epoch, job);
                }
            }
            _ => {}
        }
    }

    pub(super) fn process_backup(&mut self) {
        self.backup.poll();
        let epoch = self.deployment.epoch();
        if self.epoch == epoch {
            return;
        }
        self.epoch = epoch;
        self.worker.set_epoch(epoch);
        self.profiles.clear();
        self.selected_profile = None;
        self.profile_menu_expanded = false;
        self.page = Page::Activities;
        self.preferences_saving = false;
        self.selected_activity = 0;
        self.activity_detail = None;
        self.activity_workspace = activity::Workspace::new(&self.map_runtime);
        self.create_profile = None;
        self.avatar_editor = None;
        self.device_browser = None;
        self.device_fit_preview = None;
        self.device_browser_loading = None;
        self.device_browser_status = DeviceBrowserStatus::Idle;
        self.file_window.close(&self.context);
        self.file_window_device = None;
        self.file_window_owner = None;
        self.import = ImportStatus::default();
        self.notice = None;
        self.toasts = notification::Toasts::default();
        self.device_toasts.clear();
        self.load = LoadState::Loading;
        self.worker.reload();
    }

    pub(super) fn show_backup_result(&mut self, ui: &mut Ui) {
        if !matches!(self.backup.state, State::Idle) {
            let action = ui
                .scope(|ui| {
                    garmin_ui::backup::show_operation(
                        ui,
                        &garmin_ui::backup::Props {
                            server_files: false,
                            intl: &self.intl,
                            state: &self.backup.state,
                            file: self.backup.file.as_ref(),
                            enabled: false,
                        },
                    )
                })
                .inner;
            self.handle_backup_action(action);
            ui.add_space(16.0);
        }
    }
}
