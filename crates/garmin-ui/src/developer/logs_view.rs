//! Responsive log filters and records within the panel's single scroll area.
use egui::Ui;
use garmin_model::logging::{Filter, Level, Record};

use super::{Request, State, panel};
use crate::{Size, button, input, select};

pub(super) fn show(ui: &mut Ui, state: &mut State) {
    panel::section(ui, "logs", "Logs", true, |ui| {
        let before = state.filter.clone();
        let was_paused = state.paused;
        filters(ui, &mut state.filter);
        panel::actions(ui, 360.0, |ui| {
            if panel::action(
                ui,
                if state.paused {
                    "Resume stream"
                } else {
                    "Pause stream"
                },
                button::Kind::Secondary,
            )
            .clicked()
            {
                state.paused = !state.paused;
            }
            if panel::action(ui, "Export logs", button::Kind::Secondary).clicked() {
                state.requests.push(Request::Export(state.filter.clone()));
            }
            if state.native
                && panel::action(ui, "Open log folder", button::Kind::Secondary).clicked()
            {
                state.requests.push(Request::OpenFolder);
            }
        });
        if before != state.filter || was_paused != state.paused {
            state.generation += 1;
            if before != state.filter {
                state.cursor = None;
                state.logs.clear();
                state.gap = false;
            }
            if !state.paused {
                state.requests.push(Request::Subscribe {
                    generation: state.generation,
                    filter: state.filter.clone(),
                    cursor: state.cursor,
                });
            }
        }
        panel::note(
            ui,
            &format!(
                "{} · {} records",
                if state.paused {
                    "Stream paused"
                } else if state.connected {
                    "Live · Connected"
                } else {
                    "Disconnected"
                },
                state.logs.len()
            ),
        );
        if state.gap {
            panel::note(
                ui,
                "Log history restarted or some earlier records are no longer available.",
            );
        }
        if let Some(message) = &state.log_error {
            panel::error(ui, message);
        }
        if let Some(message) = &state.export_error {
            panel::error(ui, message);
        }
        if state.logs.is_empty() {
            panel::note(ui, "No log records match these filters.");
        }
        for record in &state.logs {
            record_row(ui, record);
        }
    });
}

fn filters(ui: &mut Ui, filter: &mut Filter) {
    let columns = if ui.available_width() >= 400.0 { 2 } else { 1 };
    ui.columns(columns, |uis| {
        let levels = [
            Level::Trace,
            Level::Debug,
            Level::Info,
            Level::Warn,
            Level::Error,
        ];
        let choices = ["Trace", "Debug", "Info", "Warn", "Error"].map(select::Choice::new);
        let mut selected = levels
            .iter()
            .position(|level| *level == filter.minimum)
            .unwrap_or(2);
        let id = uis[0].make_persistent_id("log-level");
        select::show(
            &mut uis[0],
            id,
            &mut selected,
            &choices,
            select::Props::new("Minimum level").size(Size::Small),
        );
        filter.minimum = levels[selected];
        input::show(
            &mut uis[columns - 1],
            &mut filter.text,
            input::Props::new("Search")
                .placeholder("Message or field value")
                .size(Size::Small),
        );
    });
    let columns = if ui.available_width() >= 500.0 { 3 } else { 1 };
    ui.columns(columns, |uis| {
        for (index, (label, value)) in [
            ("Component", &mut filter.component),
            ("Source", &mut filter.source),
            ("Session", &mut filter.session),
        ]
        .into_iter()
        .enumerate()
        {
            input::show(
                &mut uis[index % columns],
                value,
                input::Props::new(label)
                    .placeholder("All")
                    .size(Size::Small),
            );
        }
    });
    panel::section(ui, "log-time", "Time range", false, |ui| {
        let columns = if ui.available_width() >= 400.0 { 2 } else { 1 };
        ui.columns(columns, |uis| {
            time_filter(&mut uis[0], "Since (epoch ms)", &mut filter.since_ms);
            time_filter(
                &mut uis[columns - 1],
                "Until (epoch ms)",
                &mut filter.until_ms,
            );
        });
    });
}

fn time_filter(ui: &mut Ui, label: &str, value: &mut Option<u64>) {
    ui.push_id(label, |ui| {
        let mut enabled = value.is_some();
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            if ui
                .checkbox(&mut enabled, label)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .changed()
            {
                *value = enabled.then_some(0);
            }
        });
        if let Some(value) = value {
            input::show_number(
                ui,
                value,
                input::NumberProps::new("Milliseconds since Unix epoch")
                    .size(Size::Small)
                    .steppers(false),
            );
        }
    });
}

fn record_row(ui: &mut Ui, record: &Record) {
    let title = format!(
        "{:?} · {} · {}",
        record.level, record.component, record.message
    );
    panel::section(ui, ("record", record.sequence), &title, false, |ui| {
        panel::code(ui, &record.message);
        panel::note(
            ui,
            &format!(
                "{} · {} · {} ms",
                record.source, record.session, record.timestamp_ms
            ),
        );
        for (key, value) in &record.fields {
            panel::code(ui, &format!("{key}: {value}"));
        }
        if panel::action(ui, "Copy record as JSON", button::Kind::Secondary).clicked() {
            ui.ctx()
                .copy_text(serde_json::to_string_pretty(record).unwrap_or_default());
        }
    });
}
