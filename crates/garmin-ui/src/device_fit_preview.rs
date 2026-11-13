//! Shared read-only FIT previews with explicit activity import confirmation.

use egui::{Id, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_model::identity::UnitSystem;
use garmin_service_api::{DeviceBrowserTarget, DeviceFitPreview, DeviceFitPreviewContent};

use crate::{activity, icons, modal};

pub struct Preview<T = DeviceBrowserTarget> {
    target: T,
    data: DeviceFitPreview,
    selected: usize,
    workspace: activity::Workspace,
    course_map: activity::map::ActivityMap,
    presentation: modal::Presentation,
    cloud: Option<(garmin_model::garmin_connect::ActivityDetails, bool)>,
}

pub enum Action<T = DeviceBrowserTarget> {
    Close,
    Import(T),
}

impl<T: Clone> Preview<T> {
    #[must_use]
    pub const fn target(&self) -> &T {
        &self.target
    }

    #[must_use]
    pub fn new(
        target: T,
        data: DeviceFitPreview,
        runtime: &activity::map_runtime::MapRuntimeHandle,
    ) -> Self {
        Self {
            target,
            data,
            selected: 0,
            workspace: activity::Workspace::new(runtime),
            course_map: activity::map::ActivityMap::new(runtime),
            presentation: modal::Presentation::Modal,
            cloud: None,
        }
    }

    #[must_use]
    pub const fn with_presentation(mut self, presentation: modal::Presentation) -> Self {
        self.presentation = presentation;
        self
    }

    #[must_use]
    pub fn with_cloud_details(
        mut self,
        details: garmin_model::garmin_connect::ActivityDetails,
        imported: bool,
    ) -> Self {
        self.cloud = Some((details, imported));
        self
    }

    fn already_imported(&self) -> bool {
        self.cloud.as_ref().is_some_and(|(_, imported)| *imported)
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        busy: bool,
        units: UnitSystem,
        today: jiff::civil::Date,
    ) -> Option<Action<T>> {
        if matches!(&self.data.content, DeviceFitPreviewContent::Course(_)) {
            return self.show_course(ui, intl, busy);
        }
        self.selected = self
            .selected
            .min(self.data.activities().len().saturating_sub(1));
        let output = self.render(ui, intl, busy, units, today);
        if let Some(activity::Action::Select(index)) = output.inner {
            self.selected = index;
        }
        match output.action {
            Some(modal::Action::Cancel) if !busy => Some(Action::Close),
            Some(modal::Action::Primary)
                if !busy && !self.already_imported() && self.data.supports_activity_import() =>
            {
                Some(Action::Import(self.target.clone()))
            }
            Some(_) | None => None,
        }
    }

    fn render(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        busy: bool,
        units: UnitSystem,
        today: jiff::civil::Date,
    ) -> modal::Output<Option<activity::Action>> {
        let presentations = self
            .data
            .activities()
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
        let selected = self.data.activities().get(self.selected);
        let recording_key = selected.map(|_| format!("{}:{}", self.data.file_name, self.selected));
        let no_route = format_message!(intl, default_message: "No recorded route");
        let description = if self.cloud.is_some() {
            format_message!(intl, default_message: "FIT activity preview · {file}", values: { file: self.data.file_name.as_str() })
        } else {
            format_message!(intl, default_message: "FIT activity preview")
        };
        let close = format_message!(intl, default_message: "Close");
        let import = if self.already_imported() {
            format_message!(intl, default_message: "Already imported")
        } else if busy {
            format_message!(intl, default_message: "Importing…")
        } else {
            format_message!(intl, default_message: "Import FIT")
        };
        let empty = format_message!(intl, default_message: "No activities in this FIT file");
        let select = format_message!(intl, default_message: "Select an activity");
        modal::show_compact(
            ui,
            Id::new("device-fit-preview"),
            &modal::Props {
                title: self
                    .cloud
                    .as_ref()
                    .and_then(|(details, _)| details.name.as_deref())
                    .unwrap_or(&self.data.file_name),
                description: Some(&description),
                size: modal::Size::Large,
                presentation: self.presentation,
                cancel_label: Some(&close),
                cancel_disabled: Some(busy),
                backdrop_closes: Some(!busy),
                primary: Some(modal::Primary {
                    label: &import,
                    icon: Some(if self.already_imported() {
                        icons::CHECK
                    } else {
                        icons::UPLOAD_SIMPLE
                    }),
                    kind: modal::PrimaryKind::Confirm,
                    enabled: !busy
                        && !self.already_imported()
                        && self.data.supports_activity_import(),
                }),
            },
            |ui| {
                show_notes(
                    ui,
                    self.cloud
                        .as_ref()
                        .and_then(|(details, _)| details.description.as_deref()),
                );
                self.workspace.show(
                    ui,
                    intl,
                    &activity::WorkspaceProps {
                        today,
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

    fn show_course(&mut self, ui: &mut Ui, intl: &Intl, busy: bool) -> Option<Action<T>> {
        let DeviceFitPreviewContent::Course(course) = &self.data.content else {
            return None;
        };
        let description = format_message!(intl, default_message: "Route preview");
        let close = format_message!(intl, default_message: "Close");
        let height = (ui.available_height() - 192.0).clamp(256.0, 640.0);
        let output = modal::show_compact(
            ui,
            Id::new("device-fit-course-preview"),
            &modal::Props {
                title: &self.data.file_name,
                description: Some(&description),
                size: modal::Size::Large,
                presentation: self.presentation,
                cancel_label: Some(&close),
                cancel_disabled: Some(busy),
                backdrop_closes: Some(!busy),
                primary: None,
            },
            |ui| {
                if let Some(name) = &course.name {
                    ui.label(crate::typography::semibold(name));
                    ui.add_space(8.0);
                }
                self.course_map.show(ui, &activity::map::Props {
                    label: &description,
                    samples: activity::map::samples::Samples::Planned(&course.points),
                    selected_coordinate: None,
                    sample_range: 0..=course.points.len().saturating_sub(1),
                    highlighted_range: None,
                    fit_key: &self.data.file_name,
                    empty: &format_message!(intl, default_message: "No route geometry"),
                    loading_background: &format_message!(intl, default_message: "Loading map…"),
                    background_unavailable: &format_message!(intl, default_message: "Map background unavailable"),
                    height,
                });
            },
        );
        (output.action == Some(modal::Action::Cancel) && !busy).then_some(Action::Close)
    }
}

fn show_notes(ui: &mut Ui, notes: Option<&str>) {
    let Some(notes) = notes else {
        return;
    };
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(16, 8))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("cloud-activity-notes")
                .max_height(96.0)
                .show(ui, |ui| {
                    ui.label(notes);
                });
        });
}
