use egui::{Align, Ui};
use garmin_i18n::{Intl, format_message};
use garmin_service_api::firmware::{
    OperationStage, ProgressAmount, ProgressState, StageLine, Transcript,
};

use crate::summary_table;
use crate::summary_table::{Cell, Column};
use crate::{accordion, typography};

pub(super) fn show(ui: &mut Ui, intl: &Intl, transcript: &Transcript, finished: bool) {
    if transcript.stages.is_empty() {
        return;
    }
    ui.add_space(8.0);
    let operation = format_message!(intl, default_message: "Operation");
    let status = format_message!(intl, default_message: "Status");
    let rows = transcript
        .stages
        .iter()
        .map(|line| stage_cells(intl, line, finished));
    if ui.available_width() >= 640.0 {
        let detail = format_message!(intl, default_message: "Details");
        summary_table::show(
            ui,
            &operation,
            [
                Column::new(&operation, 2, Align::Min),
                Column::new(&detail, 6, Align::Min),
                Column::new(&status, 3, Align::Max),
            ],
            rows.map(|(mut stage, value)| {
                let detail = Cell::new(stage.detail.take().unwrap_or_default());
                [stage, detail, value]
            }),
            None,
        );
    } else {
        summary_table::show(
            ui,
            &operation,
            [
                Column::new(&operation, 3, Align::Min),
                Column::new(&status, 2, Align::Max),
            ],
            rows.map(|(description, value)| [description, value]),
            None,
        );
    }
    if transcript.history.is_empty() {
        return;
    }
    ui.add_space(16.0);
    accordion::show_with_body_margin(
        ui,
        &accordion::Props {
            id: "firmware.transfer.history",
            label: &format_message!(intl, default_message: "Operation history"),
            default_open: true,
            inline_padding: 8,
        },
        egui::Margin {
            top: 0,
            ..egui::Margin::same(8)
        },
        |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for event in &transcript.history {
                let mut text = event.label.clone();
                if let Some(path) = &event.path {
                    text.push_str(" · ");
                    text.push_str(path);
                }
                ui.horizontal_top(|ui| {
                    if let Some(time) = event
                        .recorded_at
                        .and_then(|time| jiff::Zoned::try_from(time).ok())
                    {
                        ui.weak(time.strftime("%H:%M:%S").to_string());
                    }
                    ui.add(egui::Label::new(text).wrap().selectable(true));
                });
            }
        },
    );
}

fn stage_cells(intl: &Intl, line: &StageLine, finished: bool) -> (Cell, Cell) {
    let running = matches!(
        line.state,
        Some(ProgressState::Started | ProgressState::Advanced)
    );
    let status = match line.state {
        None if finished => format_message!(intl, default_message: "Not run"),
        None => format_message!(intl, default_message: "Waiting"),
        Some(ProgressState::Completed) => format_message!(intl, default_message: "Complete"),
        Some(ProgressState::Failed) => format_message!(intl, default_message: "Failed"),
        Some(_) if finished => format_message!(intl, default_message: "Stopped"),
        Some(_) => format_message!(intl, default_message: "In progress"),
    };
    let mut description = Cell::new(typography::semibold(stage_name(intl, line.stage)));
    if line.state.is_none() {
        return (description, Cell::new(status));
    }
    description.detail = Some(match &line.path {
        Some(path) => format!("{} · {path}", line.label),
        None => line.label.clone(),
    });
    let (amount, fraction) = match line.amount {
        ProgressAmount::Bytes {
            completed,
            total: Some(total),
        } if running && !total.is_zero() => (
            format!("{} / {}", completed.format_binary(), total.format_binary()),
            Some(f32::from(completed.ratio_scaled(total, 1000)) / 1000.0),
        ),
        ProgressAmount::Operations {
            completed,
            total: Some(total),
        } if running && total > 0 => {
            use egui::emath::Numeric as _;
            (
                format!("{completed} / {total}"),
                Some(f32::from_f64(
                    completed.min(total).to_f64() / total.to_f64(),
                )),
            )
        }
        ProgressAmount::Bytes { completed, .. } if !completed.is_zero() => {
            (completed.format_binary(), None)
        }
        _ => return (description, Cell::new(status)),
    };
    let mut value = Cell::new(format!("{status} · {amount}"));
    if !finished {
        value.fraction = fraction;
    }
    (description, value)
}

fn stage_name(intl: &Intl, stage: OperationStage) -> String {
    match stage {
        OperationStage::Inspect => format_message!(intl, default_message: "Inspect"),
        OperationStage::Query => format_message!(intl, default_message: "Query"),
        OperationStage::Plan => format_message!(intl, default_message: "Plan"),
        OperationStage::Backup => format_message!(intl, default_message: "Backup"),
        OperationStage::Download => format_message!(intl, default_message: "Download"),
        OperationStage::Verify => format_message!(intl, default_message: "Verify"),
        OperationStage::Authorize => format_message!(intl, default_message: "Authorize"),
        OperationStage::Stage => format_message!(intl, default_message: "Stage"),
        OperationStage::Commit => format_message!(intl, default_message: "Write"),
        OperationStage::Cleanup => format_message!(intl, default_message: "Cleanup"),
        OperationStage::Upload => format_message!(intl, default_message: "Upload"),
        OperationStage::DeviceFinalize => format_message!(intl, default_message: "Device finalize"),
        OperationStage::DeviceVerify => format_message!(intl, default_message: "Device verify"),
        OperationStage::Delete => format_message!(intl, default_message: "Delete"),
    }
}
