//! Shared route library, exact GPX candidate review, and immutable Course versions.

mod detail;
mod review;
mod state;
#[cfg(test)]
mod tests;
mod widgets;
pub use state::{Action, State};

use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_model::route::RouteSport;
use garmin_service_api::routes::RouteRequest;

use crate::{
    activity::{
        map::{ActivityMap, Props, samples::Samples},
        map_runtime::MapRuntimeHandle,
    },
    button, icons, typography,
};
use widgets::{Row, action_button, action_row};

pub struct Workspace {
    map: ActivityMap,
    viewport_height: f32,
    deleting: Option<garmin_service_api::routes::CourseVersion>,
    scroll_to_transfer: bool,
}

impl Workspace {
    #[must_use]
    pub fn new(runtime: &MapRuntimeHandle) -> Self {
        Self {
            map: ActivityMap::new(runtime),
            viewport_height: 640.0,
            deleting: None,
            scroll_to_transfer: false,
        }
    }

    pub fn show(&mut self, ui: &mut Ui, intl: &Intl, state: &mut State) -> Option<Action> {
        self.viewport_height = ui.available_height();
        let padding = if ui.available_width() < 480.0 { 16 } else { 24 };
        let action = egui::ScrollArea::vertical()
            .id_salt("routes")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::same(padding))
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(1120.0));
                        ui.set_min_width(ui.available_width());
                        let retry = error(ui, intl, state);
                        let action = if state.upload.is_some() {
                            self.review(ui, intl, state)
                        } else if state.detail.is_some() {
                            self.detail(ui, intl, state)
                        } else {
                            library(ui, intl, state)
                        };
                        action.or(retry)
                    })
                    .inner
            })
            .inner;
        if action.is_some() {
            self.deleting = None;
        }
        let action = action.or_else(|| self.delete_confirmation(ui, intl, state));
        match action {
            Some(Action::BeginTransfer(generation)) => {
                state.transfer = Some(state::TransferState::new(generation));
                self.scroll_to_transfer = true;
                None
            }
            Some(Action::DismissTransfer) => {
                state.transfer = None;
                self.scroll_to_transfer = false;
                None
            }
            action => action,
        }
    }

    fn preview(&mut self, ui: &mut Ui, intl: &Intl, state: &State, geometry: bool, key: &str) {
        let height = preview_height(ui.available_width(), self.viewport_height);
        if !geometry {
            egui::Frame::NONE.inner_margin(16).show(ui, |ui| {
                typography::body(ui, &format_message!(intl, default_message: "This route contains unresolved control points. Save it for reference; Course generation requires resolved geometry."));
            });
        } else if state.points_ready {
            self.map.show(ui, &Props {
                label: &format_message!(intl, default_message: "Route preview"),
                samples: Samples::Planned(&state.points),
                selected_coordinate: None,
                sample_range: 0..=state.points.len().saturating_sub(1),
                highlighted_range: None,
                fit_key: key,
                empty: &format_message!(intl, default_message: "No route geometry"),
                loading_background: &format_message!(intl, default_message: "Loading map…"),
                background_unavailable: &format_message!(intl, default_message: "Map background unavailable"),
                height,
            });
        } else {
            ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                });
            });
        }
    }
}

fn preview_height(width: f32, viewport_height: f32) -> f32 {
    ((width * 0.625)
        .min(viewport_height - 224.0)
        .clamp(256.0, 640.0)
        / 8.0)
        .floor()
        * 8.0
}

fn error(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    let error = state.error.as_ref()?;
    typography::body(
        ui,
        &format_message!(intl, default_message: "The route operation could not be completed."),
    );
    crate::accordion::show(
        ui,
        &crate::accordion::Props {
            id: "routes.error",
            label: &format_message!(intl, default_message: "Details"),
            default_open: false,
            inline_padding: 0,
        },
        |ui| {
            typography::body(ui, error);
        },
    );
    let retry = state
        .retry
        .as_ref()
        .filter(|_| {
            action_button(
                ui,
                "routes.retry",
                &format_message!(intl, default_message: "Retry"),
                button::Kind::Secondary,
                !state.busy,
            )
        })
        .cloned()
        .map(Action::Request);
    ui.add_space(16.0);
    retry
}

fn library(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    let enabled = !state.busy && state.pending.is_none();
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading(format_message!(intl, default_message: "Routes"));
        widgets::busy(ui, state);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = format_message!(intl, default_message: "Import GPX");
            let mut import = widgets::button(&label, button::Kind::Primary, enabled);
            import.icon = Some(icons::UPLOAD_SIMPLE);
            let response = import.show(ui);
            crate::semantics::target(ui, &response, "routes.import");
            if response.clicked() {
                action = Some(Action::Import);
            }
        });
    });
    typography::body(
        ui,
        &format_message!(intl, default_message: "Import a GPX file to preview a route and create a FIT Course."),
    );
    ui.add_space(16.0);
    if state.routes.is_empty() && enabled {
        typography::body(
            ui,
            &format_message!(intl, default_message: "No saved routes yet."),
        );
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for route in &state.routes {
            let response = Row {
                title: route.name.as_str(),
                subtitle: &sport_label(intl, route.sport),
                icon: sport_icon(route.sport),
                selected: false,
                enabled,
                target: &format!("routes.open.{}", route.id),
            }
            .show(ui);
            if response.clicked() {
                action = Some(Action::Request(RouteRequest::Detail { plan: route.id }));
            }
        }
    });
    if let Some(offset) = state.next
        && action_button(
            ui,
            "routes.next",
            &format_message!(intl, default_message: "More routes"),
            button::Kind::Secondary,
            enabled,
        )
    {
        action = Some(Action::Request(RouteRequest::List { offset }));
    }
    action
}

fn sport_label(intl: &Intl, sport: RouteSport) -> String {
    match sport {
        RouteSport::Walking => format_message!(intl, default_message: "Walking"),
        RouteSport::Hiking => format_message!(intl, default_message: "Hiking"),
        RouteSport::Running => format_message!(intl, default_message: "Running"),
        RouteSport::Cycling => format_message!(intl, default_message: "Cycling"),
    }
}

const fn sport_icon(sport: RouteSport) -> icons::Icon {
    match sport {
        RouteSport::Walking => icons::PERSON_SIMPLE_WALK,
        RouteSport::Hiking => icons::PERSON_SIMPLE_HIKE,
        RouteSport::Running => icons::PERSON_SIMPLE_RUN,
        RouteSport::Cycling => icons::BICYCLE,
    }
}
