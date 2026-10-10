use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_model::route::{RouteCandidateSource, RouteSport};
use garmin_service_api::routes::{GpxUpload, GpxUploadPhase, RouteRequest};

use super::{Action, Row, State, Workspace, error, route_header, widgets};
use crate::{Size, button, icons, input, typography};

impl Workspace {
    pub(super) fn review(&mut self, ui: &mut Ui, intl: &Intl, state: &mut State) -> Option<Action> {
        let upload = state.upload.as_ref()?.clone();
        let enabled = !state.busy && state.pending.is_none();
        let importing = route_header(ui, intl, state, enabled);
        let retry = error(ui, intl, state);
        ui.add_space(16.0);
        let cancelled = header(
            ui,
            intl,
            state,
            &upload,
            enabled || matches!(upload.phase, GpxUploadPhase::Parsing),
        );
        ui.add_space(16.0);
        if matches!(upload.phase, GpxUploadPhase::Review { .. }) {
            if state.candidates.is_empty() && enabled {
                typography::body(
                    ui,
                    &format_message!(intl, default_message: "No usable routes found. This file has no importable tracks or routes."),
                );
            } else if !state.candidates.is_empty() {
                ui.label(typography::semibold(
                    format_message!(intl, default_message: "Choose a path"),
                ));
                typography::body(
                    ui,
                    &format_message!(intl, default_message: "A GPX file can contain multiple tracks, segments, or routes. Choose one path to preview and save."),
                );
                if let GpxUploadPhase::Review { candidates, .. } = &upload.phase {
                    ui.label(format_message!(intl, default_message: "Available paths: {count}", values: { count: u64::from(*candidates) }));
                }
                ui.add_space(8.0);
            }
            candidates(ui, intl, state, enabled);
            if let Some(candidate) = state.candidate {
                ui.add_space(16.0);
                fields(ui, intl, state, enabled);
                ui.add_space(16.0);
                let geometry = state
                    .candidates
                    .iter()
                    .find(|value| value.source == candidate)
                    .is_some_and(|value| value.geometry);
                self.preview(
                    ui,
                    intl,
                    state,
                    geometry,
                    &format!("{}:{candidate:?}", upload.operation),
                );
                ui.add_space(16.0);
            }
        }
        if cancelled {
            return Some(Action::Request(RouteRequest::Cancel {
                operation: upload.operation,
            }));
        }
        if state.candidate.is_some() {
            let confirm = state.confirm();
            let save = format_message!(intl, default_message: "Save route");
            let mut props =
                widgets::button(&save, button::Kind::Primary, enabled && confirm.is_some());
            props.icon = Some(icons::CHECK);
            let response = props.show(ui);
            crate::semantics::target(ui, &response, "routes.save");
            if response.clicked() {
                return confirm.map(Action::Request);
            }
        }
        importing.or(retry)
    }
}

fn header(ui: &mut Ui, intl: &Intl, state: &State, upload: &GpxUpload, can_cancel: bool) -> bool {
    let mut cancelled = false;
    let palette = crate::theme::palette(ui);
    egui::Frame::NONE
        .fill(crate::theme::color32(
            palette.surfaces().layer(garmin_color::theme::Level::One),
        ))
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let text_width = (ui.available_width() - 80.0).max(0.0);
                let has_progress = !matches!(upload.phase, GpxUploadPhase::Review { .. });
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, 48.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(text_width);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&upload.file_name).strong().size(16.0),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&upload.file_name);
                        if has_progress {
                            typography::body(
                                ui,
                                &format_message!(intl, default_message: "{received} of {total}", values: { received: crate::text::format_bytes(upload.received.as_u64()), total: crate::text::format_bytes(upload.total.as_u64()) }),
                            );
                        } else {
                            typography::body(ui, &format_message!(intl, default_message: "File size: {size}", values: { size: crate::text::format_bytes(upload.total.as_u64()) }));
                        }
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(24.0, 48.0),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        if (state.busy || state.pending.is_some())
                            && matches!(
                                upload.phase,
                                GpxUploadPhase::Uploading | GpxUploadPhase::Parsing
                            )
                        {
                            ui.add(egui::Spinner::new().size(16.0));
                        }
                    },
                );
                cancelled = widgets::icon_button(
                    ui,
                    "routes.cancel",
                    &format_message!(intl, default_message: "Cancel import"),
                    icons::X,
                    button::Kind::Tertiary,
                    can_cancel,
                );
            });
        });
    cancelled
}

fn fields(ui: &mut Ui, intl: &Intl, state: &mut State, enabled: bool) {
    let name = |ui: &mut Ui, value: &mut String| {
        field_label(
            ui,
            &format_message!(intl, default_message: "Route name"),
            "routes.name.label",
        );
        let response = input::show(ui, value, input::Props::new("").disabled(!enabled));
        crate::semantics::target(ui, &response, "routes.name");
    };
    let sport = |ui: &mut Ui, selected: &mut Option<RouteSport>| {
        field_label(
            ui,
            &format_message!(intl, default_message: "Route type"),
            "routes.type.label",
        );
        sports(ui, intl, selected, enabled);
    };
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(16.0, 8.0);
        if ui.available_width() >= 960.0 {
            ui.columns(2, |columns| {
                name(&mut columns[0], &mut state.name);
                sport(&mut columns[1], &mut state.sport);
            });
        } else {
            name(ui, &mut state.name);
            ui.add_space(8.0);
            sport(ui, &mut state.sport);
        }
    });
}

fn field_label(ui: &mut Ui, label: &str, target: &str) {
    let response =
        ui.add(egui::Label::new(egui::RichText::new(label).size(12.0)).selectable(false));
    crate::semantics::target(ui, &response, target);
}

fn candidates(ui: &mut Ui, intl: &Intl, state: &mut State, enabled: bool) {
    let mut selected = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for candidate in &state.candidates {
            let label = candidate.suggested_name.as_ref().map_or_else(
                || format_message!(intl, default_message: "Unnamed path"),
                ToString::to_string,
            );
            let detail = if candidate.geometry {
                format_message!(intl, default_message: "{count, plural, one {# point} other {# points}} · FIT Course available", values: { count: u64::from(candidate.point_count) })
            } else {
                format_message!(intl, default_message: "{count, plural, one {# control point} other {# control points}} · FIT Course unavailable", values: { count: u64::from(candidate.point_count) })
            };
            if (Row {
                title: &label,
                subtitle: &candidate_label(intl, candidate.source),
                detail: Some(&detail),
                outline: Some((&candidate.outline, candidate.geometry)),
                icon: match candidate.source {
                    RouteCandidateSource::TrackSegment { .. } => icons::PATH,
                    RouteCandidateSource::Route { .. } => icons::ROUTE,
                },
                selected: state.candidate == Some(candidate.source),
                enabled,
                target: &format!("routes.candidate.{:?}", candidate.source),
            })
            .show(ui)
            .clicked()
            {
                selected = Some(candidate.source);
            }
        }
    });
    if let Some(selected) = selected {
        state.select_candidate(selected);
    }
    for rejected in &state.rejected {
        typography::body(ui, &candidate_label(intl, rejected.source));
        typography::body(
            ui,
            &format_message!(intl, default_message: "A GPX candidate could not be used: {reason}", values: { reason: rejected.reason.to_string() }),
        );
    }
}

fn candidate_label(intl: &Intl, source: RouteCandidateSource) -> String {
    match source {
        RouteCandidateSource::TrackSegment { track, segment } => {
            format_message!(intl, default_message: "Track {track}, segment {segment}", values: { track: (track + 1) as u64, segment: (segment + 1) as u64 })
        }
        RouteCandidateSource::Route { route } => {
            format_message!(intl, default_message: "Route {route}", values: { route: (route + 1) as u64 })
        }
    }
}

fn sports(ui: &mut Ui, intl: &Intl, selected: &mut Option<RouteSport>, enabled: bool) {
    let walking = format_message!(intl, default_message: "Walking");
    let hiking = format_message!(intl, default_message: "Hiking");
    let running = format_message!(intl, default_message: "Running");
    let cycling = format_message!(intl, default_message: "Cycling");
    let choices = [
        button::GroupChoice::new(
            &walking,
            icons::PERSON_SIMPLE_WALK,
            Some(RouteSport::Walking),
        )
        .target("routes.sport.walking"),
        button::GroupChoice::new(&hiking, icons::PERSON_SIMPLE_HIKE, Some(RouteSport::Hiking))
            .target("routes.sport.hiking"),
        button::GroupChoice::new(
            &running,
            icons::PERSON_SIMPLE_RUN,
            Some(RouteSport::Running),
        )
        .target("routes.sport.running"),
        button::GroupChoice::new(&cycling, icons::BICYCLE, Some(RouteSport::Cycling))
            .target("routes.sport.cycling"),
    ];
    if let Some(sport) = button::group(
        ui,
        *selected,
        &choices,
        button::GroupProps {
            size: Size::Medium,
            width: button::Width::Fit,
            enabled,
            style: button::GroupStyle::Subtle,
        },
    ) {
        *selected = sport;
    }
}
