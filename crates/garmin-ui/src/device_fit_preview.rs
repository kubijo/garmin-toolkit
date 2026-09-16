//! FIT activity preview opened from a connected-device browser.

use egui::{Id, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_model::identity::UnitSystem;
use garmin_service_api::{DeviceBrowserTarget, DeviceFitPreview};

use crate::{activity, icons, modal};

pub struct Preview {
    target: DeviceBrowserTarget,
    data: DeviceFitPreview,
    selected: usize,
    workspace: activity::Workspace,
}

pub enum Action {
    Close,
    Import(DeviceBrowserTarget),
}

impl Preview {
    #[must_use]
    pub fn new(
        target: DeviceBrowserTarget,
        data: DeviceFitPreview,
        runtime: &activity::map_runtime::MapRuntimeHandle,
    ) -> Self {
        Self {
            target,
            data,
            selected: 0,
            workspace: activity::Workspace::new(runtime),
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        busy: bool,
        units: UnitSystem,
    ) -> Option<Action> {
        self.selected = self
            .selected
            .min(self.data.activities.len().saturating_sub(1));
        let output = self.render(ui, intl, busy, units);
        if let Some(activity::Action::Select(index)) = output.inner {
            self.selected = index;
        }
        match output.action {
            Some(modal::Action::Cancel) if !busy => Some(Action::Close),
            Some(modal::Action::Primary) if !busy => Some(Action::Import(self.target.clone())),
            Some(_) | None => None,
        }
    }

    fn render(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        busy: bool,
        units: UnitSystem,
    ) -> modal::Output<Option<activity::Action>> {
        let presentations = self
            .data
            .activities
            .iter()
            .map(|activity| {
                activity::Presentation::from_summary(
                    activity.summary,
                    &activity.source,
                    intl,
                    units,
                )
            })
            .collect::<Vec<_>>();
        let items = presentations
            .iter()
            .map(activity::Presentation::item_props)
            .collect::<Vec<_>>();
        let selected = self.data.activities.get(self.selected);
        let recording_key = selected.map(|_| format!("{}:{}", self.data.file_name, self.selected));
        let no_route = format_message!(intl, default_message: "No recorded route");
        let description = format_message!(intl, default_message: "FIT activity preview");
        let close = format_message!(intl, default_message: "Close");
        let import = if busy {
            format_message!(intl, default_message: "Importing…")
        } else {
            format_message!(intl, default_message: "Import FIT")
        };
        let empty = format_message!(intl, default_message: "No activities in this FIT file");
        let select = format_message!(intl, default_message: "Select an activity");
        modal::show(
            ui,
            Id::new("device-fit-preview"),
            &modal::Props {
                title: &self.data.file_name,
                description: Some(&description),
                size: modal::Size::Large,
                presentation: modal::Presentation::Modal,
                cancel_label: &close,
                backdrop_closes: Some(!busy),
                primary: modal::Primary {
                    label: &import,
                    icon: Some(icons::UPLOAD_SIMPLE),
                    kind: modal::PrimaryKind::Confirm,
                    enabled: !busy,
                },
            },
            |ui| {
                self.workspace.show(
                    ui,
                    intl,
                    &activity::WorkspaceProps {
                        items: &items,
                        presentations: &presentations,
                        selected: (!items.is_empty()).then_some(self.selected),
                        recording: selected.map(|activity| &activity.recording),
                        recording_key: recording_key.as_deref(),
                        units,
                        empty_list: &empty,
                        empty_detail: &select,
                        no_route: &no_route,
                    },
                )
            },
        )
    }
}
