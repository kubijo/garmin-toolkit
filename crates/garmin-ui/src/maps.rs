//! Map workflow presentation. All choices, approvals, and progress belong to the host.

use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_service_api::maps::{
    Action, Choice, Command, Component, Failure, FailureKind, Phase, Progress, ProgressStatus,
    State,
};

use crate::{Size, button, icons, radio, text::format_bytes, typography};

/// Retain the service failure until rendering so language changes also update visible errors.
pub enum ClientError {
    Request(Failure),
    Connection(String),
}

impl ClientError {
    /// Render localized guidance, with the original host diagnostic available separately.
    pub fn show(&self, ui: &mut Ui, intl: &Intl) {
        let (message, diagnostic) = match self {
            Self::Request(failure) => (
                failure_message(intl, failure.kind),
                failure.message.as_str(),
            ),
            Self::Connection(diagnostic) => (
                format_message!(intl, default_message: "Could not connect to the map service."),
                diagnostic.as_str(),
            ),
        };
        typography::body(ui, &message);
        crate::accordion::show(
            ui,
            &crate::accordion::Props {
                id: "maps.client-error.details",
                label: &format_message!(intl, default_message: "Details"),
                default_open: false,
                inline_padding: 16,
            },
            |ui| {
                typography::body(ui, diagnostic);
            },
        );
    }
}

fn failure_message(intl: &Intl, kind: FailureKind) -> String {
    match kind {
        FailureKind::StaleRevision => {
            format_message!(intl, default_message: "The map workflow changed. Review the current state and try again.")
        }
        FailureKind::ReusedRequest => {
            format_message!(intl, default_message: "This request was already used for another choice. Review the current state and try again.")
        }
        FailureKind::InvalidChoice => {
            format_message!(intl, default_message: "This map selection is not supported. Review your choices.")
        }
        FailureKind::Unavailable => {
            format_message!(intl, default_message: "This action is unavailable in the current map state.")
        }
        FailureKind::Busy => {
            format_message!(intl, default_message: "The device is busy. Wait for its current operation to finish.")
        }
        FailureKind::Operation => {
            format_message!(intl, default_message: "The map operation failed. Review the details before trying again.")
        }
    }
}

/// Render one authoritative snapshot and return at most one user choice.
pub fn show(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Command> {
    let mut command = None;
    egui::ScrollArea::vertical().id_salt("map-management").auto_shrink([false, false]).show(ui, |ui| {
        let ribbon_extent = ui.available_rect_before_wrap().x_range();
        let inline_padding = if ui.available_width() < 480.0 { 16 } else { 32 };
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(inline_padding, 24)).show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.set_max_width(ui.available_width().min(880.0));
            ui.heading(format_message!(intl, default_message: "Manage maps"));
            ui.add_space(8.0);
            typography::body(ui, &format_message!(intl, default_message: "This device's map operation is shared across profiles. You can leave this page and return while it runs."));
            ui.add_space(16.0);
            if let Some(error) = &state.error {
                ui.label(egui::RichText::new(error).color(ui.visuals().error_fg_color));
                ui.add_space(16.0);
            }
            command = match state.phase {
                Phase::Consent => consent(ui, intl, state),
                Phase::Catalog => catalog(ui, intl, state),
                Phase::Review => review(ui, intl, state),
                Phase::Recovery => recovery(ui, intl, state, ribbon_extent, inline_padding),
                Phase::Loading | Phase::Running => running(ui, intl, state),
                Phase::Completed | Phase::Failed | Phase::Cancelled => outcome(ui, intl, state),
            };
            if !state.storages.is_empty() || state.storage_error.is_some() {
                ui.add_space(if state.phase == Phase::Recovery { 16.0 } else { 24.0 });
                ui.label(typography::semibold(format_message!(intl, default_message: "Device storage")));
                if let Some(reason) = &state.storage_error {
                    typography::body(ui, &format_message!(intl, default_message: "Could not read storage usage: {reason}", values: { reason: reason.as_str() }));
                }
                for storage in &state.storages {
                    let detail = storage.capacity.bytes().map_or_else(
                        || format_message!(intl, default_message: "Capacity unavailable"),
                        |(_, free)| format_message!(intl, default_message: "{free} available", values: { free: format_bytes(free) }),
                    );
                    crate::capacity::show(ui, &crate::capacity::Props { label: &storage.label, detail: &detail,
                        bytes: storage.capacity.bytes().map(|(total, free)| (total.saturating_sub(free), total)) });
                }
            }
            history(ui, intl, state);
        });
    });
    command
}

fn consent(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Command> {
    let service = match state.service {
        garmin_service_api::maps::CatalogService::Garmin => {
            format_message!(intl, default_message: "Garmin map service")
        }
        garmin_service_api::maps::CatalogService::Simulation => {
            format_message!(intl, default_message: "Local simulated map service — no data is sent to Garmin")
        }
    };
    ui.label(typography::semibold(service));
    ui.add_space(8.0);
    typography::body(
        ui,
        &format_message!(intl, default_message: "Checking maps sends the device description and installed map versions to the map service. No device files change until you approve a plan."),
    );
    ui.add_space(16.0);
    control(
        ui,
        state,
        Action::ContactService,
        &format_message!(intl, default_message: "Check available maps"),
        icons::MAP_TRIFOLD,
        button::Kind::Primary,
    )
    .then_some(Command::ContactService)
}

fn catalog(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Command> {
    let mut command = None;
    if state.components.is_empty() {
        typography::body(
            ui,
            &format_message!(intl, default_message: "The map service returned no components for this device."),
        );
    }
    for component in &state.components {
        ui.push_id(component.index, |ui| {
            ui.set_max_width(ui.available_width().min(480.0));
            egui::Frame::group(ui.style())
                .fill(crate::theme::color32(
                    crate::theme::palette(ui)
                        .surfaces()
                        .layer(garmin_color::theme::Level::One),
                ))
                .inner_margin(0)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 0.0;
                    egui::Frame::NONE.inner_margin(16).show(ui, |ui| {
                        component_details(ui, intl, component);
                    });
                    if let Some(choice) = component_actions(
                        ui,
                        intl,
                        component,
                        state.actions.contains(&Action::Choose),
                        state.dry_run,
                    ) {
                        command = Some(Command::Choose {
                            component: component.index,
                            choice,
                        });
                    }
                });
            ui.add_space(8.0);
        });
    }
    ui.add_space(8.0);
    command = preferences(ui, intl, state).or(command);
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(
            state
                .components
                .iter()
                .any(|item| item.choice != Choice::Keep),
            |ui| {
                if control(
                    ui,
                    state,
                    Action::Review,
                    &format_message!(intl, default_message: "Review changes"),
                    icons::MAP_TRIFOLD,
                    button::Kind::Primary,
                ) {
                    command = Some(Command::Review);
                }
            },
        );
        if control(
            ui,
            state,
            Action::ContactService,
            &format_message!(intl, default_message: "Refresh catalog"),
            icons::ARROWS_CLOCKWISE,
            button::Kind::Tertiary,
        ) {
            command = Some(Command::ContactService);
        }
    });
    command
}

fn preferences(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Command> {
    let mut command = None;
    let updates = state
        .components
        .iter()
        .any(|item| item.choice == Choice::Install);
    let verified = format_message!(intl, default_message: "Verified backup");
    let skip = format_message!(intl, default_message: "Skip backup");
    let choices = [
        radio::Choice::new(&verified, true, "maps.backup.verified"),
        radio::Choice::new(&skip, false, "maps.backup.skip"),
    ];
    if let Some(value) = radio::show(
        ui,
        !updates || state.verified_backup,
        &choices,
        radio::Props {
            label: &format_message!(intl, default_message: "Recovery backup"),
            helper: None,
            enabled: updates,
        },
    ) {
        command = Some(Command::VerifiedBackup(value));
    }
    ui.add_space(8.0);
    backup_warning(ui, intl);
    if updates && !state.verified_backup {
        typography::body(
            ui,
            &format_message!(intl, default_message: "Without a verified backup, automatic rollback is unavailable."),
        );
    }
    ui.add_space(16.0);
    let apply = format_message!(intl, default_message: "Update this device");
    let dry_run = format_message!(intl, default_message: "Simulate on a retained copy");
    let mode_help = if state.dry_run {
        format_message!(intl, default_message: "Test the update on an isolated copy kept for inspection. Your device stays unchanged.")
    } else {
        format_message!(intl, default_message: "Apply the reviewed changes to the connected device after you confirm the plan.")
    };
    let choices = [
        radio::Choice::new(&apply, false, "maps.mode.apply"),
        radio::Choice::new(&dry_run, true, "maps.mode.simulate"),
    ];
    if let Some(value) = radio::show(
        ui,
        state.dry_run,
        &choices,
        radio::Props {
            label: &format_message!(intl, default_message: "Execution mode"),
            helper: Some(&mode_help),
            enabled: !state
                .components
                .iter()
                .any(|item| item.choice == Choice::Remove),
        },
    ) {
        command = Some(Command::DryRun(value));
    }
    ui.add_space(16.0);
    command
}

fn backup_warning(ui: &mut Ui, intl: &Intl) {
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(480.0));
        crate::notification::show(ui, &crate::notification::Props {
            kind: crate::notification::Kind::Warning,
            title: &format_message!(intl, default_message: "MTP backups are extremely slow"),
            detail: Some(&format_message!(intl, default_message: "Reading large map files from Garmin devices over MTP takes a very long time. Plan for a long wait and keep the device connected until the operation finishes.")),
        });
    });
}

fn component_details(ui: &mut Ui, intl: &Intl, component: &Component) {
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.label(typography::semibold(&component.name));
    ui.add_space(8.0);
    let installed = component
        .installed
        .clone()
        .unwrap_or_else(|| format_message!(intl, default_message: "Not reported"));
    let available = component
        .available
        .clone()
        .unwrap_or_else(|| format_message!(intl, default_message: "Not reported"));
    let installed_label = format_message!(intl, default_message: "Installed version");
    let available_label = format_message!(intl, default_message: "Available version");
    let download_label = format_message!(intl, default_message: "Download size");
    let download = format_bytes(component.download_bytes);
    let rows = [
        (installed_label, installed),
        (available_label, available),
        (download_label, download),
    ];
    crate::facts::show(ui, "component-facts", &rows);
    if component.total_files > 0 {
        ui.label(format_message!(intl, default_message: "{cached} of {total} files already cached", values: { cached: u64::from(component.cached_files), total: u64::from(component.total_files) }));
    }
}

fn component_actions(
    ui: &mut Ui,
    intl: &Intl,
    component: &Component,
    enabled: bool,
    dry_run: bool,
) -> Option<Choice> {
    let keep = format_message!(intl, default_message: "Keep");
    let install = operation_label(intl, component.operation);
    let remove = format_message!(intl, default_message: "Remove");
    let support = crate::theme::palette(ui).support();
    let keep_target = format!("maps.component.{}.keep", component.index);
    let install_target = format!("maps.component.{}.install", component.index);
    let remove_target = format!("maps.component.{}.remove", component.index);
    let mut choices = vec![
        button::GroupChoice::new(&keep, icons::MINUS, Choice::Keep)
            .tint(support.information())
            .target(&keep_target),
    ];
    if component.can_install {
        choices.push(
            button::GroupChoice::new(&install, icons::ARROWS_CLOCKWISE, Choice::Install)
                .tint(support.success())
                .target(&install_target),
        );
    }
    if component.can_remove {
        choices.push(
            button::GroupChoice::new(&remove, icons::TRASH, Choice::Remove)
                .tint(support.error())
                .target(&remove_target)
                .enabled(!dry_run),
        );
    }
    button::group(
        ui,
        component.choice,
        &choices,
        button::GroupProps {
            size: Size::Small,
            width: button::Width::Fill,
            enabled,
            style: button::GroupStyle::Tiles,
        },
    )
}

fn operation_label(intl: &Intl, operation: garmin_model::map::MapOperation) -> String {
    use garmin_model::map::MapOperation;
    match operation {
        MapOperation::Install => format_message!(intl, default_message: "Install"),
        MapOperation::Update => format_message!(intl, default_message: "Update"),
        MapOperation::Reinstall => format_message!(intl, default_message: "Reinstall"),
        MapOperation::Repair => format_message!(intl, default_message: "Repair"),
        MapOperation::Downgrade => format_message!(intl, default_message: "Downgrade"),
    }
}

fn review(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Command> {
    let plan = state.plan.as_ref()?;
    ui.heading(if plan.removal {
        format_message!(intl, default_message: "Review removal")
    } else {
        format_message!(intl, default_message: "Review map update")
    });
    ui.add_space(8.0);
    typography::body(ui, &plan.components.join(", "));
    if plan.removal {
        typography::body(
            ui,
            &format_message!(intl, default_message: "Selected removals run first with verified backups. Any selected updates require a separate approval afterwards."),
        );
    }
    if state.dry_run {
        typography::body(
            ui,
            &format_message!(intl, default_message: "This plan runs on an isolated copy. The source device is unchanged."),
        );
    }
    let backup = if state.verified_backup || plan.removal {
        format_message!(intl, default_message: "Verified")
    } else {
        format_message!(intl, default_message: "Skipped — no automatic rollback")
    };
    let download_label = format_message!(intl, default_message: "Download size");
    let write_label = format_message!(intl, default_message: "Files to write");
    let remove_label = format_message!(intl, default_message: "Files to remove");
    let backup_label = format_message!(intl, default_message: "Recovery backup");
    let mut rows = vec![
        (download_label, format_bytes(plan.download_bytes)),
        (write_label, plan.write_count.to_string()),
        (remove_label, plan.remove_count.to_string()),
        (backup_label, backup),
    ];
    for requirement in &plan.storage_requirements {
        let label = state
            .storages
            .iter()
            .find(|storage| storage.id == requirement.storage)
            .map_or(requirement.storage.as_str(), |storage| {
                storage.label.as_str()
            });
        rows.push((
            label.to_owned(),
            format_message!(intl, default_message: "{space} free space required", values: {
                space: format_bytes(requirement.required_free_bytes)
            }),
        ));
    }
    ui.add_space(16.0);
    crate::facts::show(ui, "map-plan-facts", &rows);
    ui.add_space(16.0);
    crate::accordion::show(
        ui,
        &crate::accordion::Props {
            id: "maps.plan.paths",
            label: &format_message!(intl, default_message: "Affected paths"),
            default_open: false,
            inline_padding: 16,
        },
        |ui| {
            for path in &plan.paths {
                ui.label(path);
            }
            ui.label(format_message!(intl, default_message: "Plan: {digest}", values: { digest: plan.digest.as_str() }));
        },
    );
    ui.add_space(16.0);
    if state.verified_backup || plan.removal {
        backup_warning(ui, intl);
        ui.add_space(16.0);
    }
    approval_controls(ui, intl, state, plan)
}

fn approval_controls(
    ui: &mut Ui,
    intl: &Intl,
    state: &State,
    plan: &garmin_service_api::maps::Plan,
) -> Option<Command> {
    let mut command = None;
    ui.horizontal_wrapped(|ui| {
        let label = if state.dry_run {
            format_message!(intl, default_message: "Run simulation")
        } else if plan.removal {
            format_message!(intl, default_message: "Remove selected maps")
        } else {
            format_message!(intl, default_message: "Apply map update")
        };
        if control(
            ui,
            state,
            Action::Approve,
            &label,
            icons::MAP_TRIFOLD,
            if plan.removal {
                button::Kind::Danger
            } else {
                button::Kind::Primary
            },
        ) {
            command = Some(Command::Approve {
                approval: plan.approval,
            });
        }
        if control(
            ui,
            state,
            Action::Back,
            &format_message!(intl, default_message: "Change selection"),
            icons::ARROWS_CLOCKWISE,
            button::Kind::Tertiary,
        ) {
            command = Some(Command::Back);
        }
    });
    command
}

fn running(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Command> {
    ui.heading(if state.phase == Phase::Loading {
        format_message!(intl, default_message: "Inspecting device and preparing map information…")
    } else {
        format_message!(intl, default_message: "Map operation in progress")
    });
    if state.progress.is_empty() && state.active.is_empty() {
        ui.spinner();
    }
    ui.add_space(16.0);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        for progress in state.progress.iter().chain(&state.active) {
            progress_panel(ui, intl, progress);
        }
    });
    ui.add_space(16.0);
    typography::body(
        ui,
        &format_message!(intl, default_message: "Cancellation waits for required transaction cleanup. Keep the device connected."),
    );
    ui.add_space(8.0);
    control(
        ui,
        state,
        Action::Cancel,
        &format_message!(intl, default_message: "Cancel operation"),
        icons::X,
        button::Kind::Tertiary,
    )
    .then_some(Command::Cancel)
}

fn progress_panel(ui: &mut Ui, intl: &Intl, progress: &Progress) {
    let palette = crate::theme::palette(ui);
    egui::Frame::group(ui.style())
        .fill(crate::theme::color32(palette.surfaces().layer(garmin_color::theme::Level::One)))
        .inner_margin(16)
        .show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().interact_size.y = 16.0;
        ui.visuals_mut().extreme_bg_color = crate::theme::color32(
            palette.surfaces().layer(garmin_color::theme::Level::Two),
        );
        let running = progress.status == ProgressStatus::Running;
        let detail = if progress.bytes {
            match progress.total {
                Some(total) => format!(
                    "{} / {}",
                    format_bytes(progress.completed),
                    format_bytes(total)
                ),
                None => format_bytes(progress.completed),
            }
        } else if let Some(total) = progress.total {
            format!("{} / {total}", progress.completed)
        } else {
            String::new()
        };
        let value = progress.total.filter(|total| *total > 0).map_or(
            crate::progress::Value::Indeterminate,
            |total| {
                let fraction = u16::try_from(
                    u128::from(progress.completed.min(total)) * 10_000 / u128::from(total),
                )
                .unwrap_or(10_000);
                crate::progress::Value::Determinate {
                    completed: usize::from(fraction),
                    total: 10_000,
                }
            },
        );
        let detail = if detail.is_empty() {
            elapsed(progress.elapsed_ms)
        } else {
            format!("{detail} · {}", elapsed(progress.elapsed_ms))
        };
        if running {
            crate::progress::show(
            ui,
            &crate::progress::Props {
                label: &progress.label,
                detail: Some(&detail),
                value,
                height: None,
            },
            );
        } else {
            ui.horizontal(|ui| {
                let (icon, color) = match progress.status {
                    ProgressStatus::Completed => (icons::CHECK, palette.support().success()),
                    ProgressStatus::Failed => (icons::WARNING, palette.support().error()),
                    _ => (icons::PAUSE, palette.content().text_secondary()),
                };
                ui.add(icon.image(color).fit_to_exact_size(egui::vec2(16.0, 16.0)));
                ui.label(typography::semibold(&progress.label));
            });
            ui.label(egui::RichText::new(detail).size(12.0).weak());
        }
        if let Some(path) = &progress.path {
            ui.label(egui::RichText::new(path).size(12.0).weak());
        }
        if running && progress.stalled {
            typography::body(
                ui,
                &format_message!(intl, default_message: "No recent progress; the device may be finalizing or stalled."),
            );
        } else if running && let Some(rate) = progress.bytes_per_second {
            ui.label(format_message!(intl, default_message: "Average {rate}/s", values: { rate: format_bytes(rate) }));
        }
        if running && let Some(remaining) = progress.remaining_seconds {
            ui.label(format_message!(intl, default_message: "About {seconds} seconds remaining", values: { seconds: remaining }));
        }
    });
}

fn recovery(
    ui: &mut Ui,
    intl: &Intl,
    state: &State,
    ribbon_extent: egui::emath::Rangef,
    inline_padding: i8,
) -> Option<Command> {
    let mut command = None;
    ui.scope(|ui| {
        // Give the ribbon its actual full width without widening the padded page layout.
        let mut ribbon_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("map-recovery-ribbon")
                .max_rect(egui::Rect::from_x_y_ranges(
                    ribbon_extent,
                    ui.available_rect_before_wrap().y_range(),
                )),
        );
        let ribbon = egui::Frame::NONE
            .fill(crate::theme::color32(
                crate::theme::palette(ui)
                    .surfaces()
                    .layer(garmin_color::theme::Level::One),
            ))
            .inner_margin(egui::Margin::symmetric(0, 16))
            .show(&mut ribbon_ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                command = recovery_content(ui, intl, state, inline_padding);
            });
        ui.allocate_space(egui::vec2(
            ui.available_width(),
            ribbon.response.rect.height(),
        ));
        ui.add_space(8.0);
        if control(
            ui,
            state,
            Action::Refresh,
            &format_message!(intl, default_message: "Refresh"),
            icons::ARROWS_CLOCKWISE,
            button::Kind::Tertiary,
        ) {
            command = Some(Command::Refresh);
        }
    });
    command
}

fn recovery_content(
    ui: &mut Ui,
    intl: &Intl,
    state: &State,
    inline_padding: i8,
) -> Option<Command> {
    let mut command = None;
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(inline_padding, 0)).show(ui, |ui| {
    ui.set_max_width(ui.available_width().min(560.0));
    ui.heading(format_message!(intl, default_message: "Interrupted map operation"));
    ui.add_space(8.0);
    if state
        .recovery
        .as_ref()
        .is_some_and(|recovery| recovery.simulated)
    {
        typography::body(
            ui,
            &format_message!(intl, default_message: "This recovery applies to the retained simulation. The source device remains unchanged."),
        );
    }
    typography::body(
        ui,
        &format_message!(intl, default_message: "Resolve the retained transaction before making further changes. Recovery requires this host's original evidence."),
    );
    ui.add_space(16.0);
    ui.horizontal_wrapped(|ui| {
        for (action, choice, label, icon, kind) in [
            (
                Action::Recover,
                Command::Recover,
                format_message!(intl, default_message: "Recover"),
                icons::CLOCK_COUNTER_CLOCKWISE,
                button::Kind::Primary,
            ),
            (
                Action::ClearRecovery,
                Command::ClearRecovery,
                format_message!(intl, default_message: "Verify and clear"),
                icons::CHECK,
                button::Kind::Tertiary,
            ),
            (
                Action::DiscardPreparation,
                Command::DiscardPreparation,
                format_message!(intl, default_message: "Discard preparation"),
                icons::TRASH,
                button::Kind::Tertiary,
            ),
        ] {
            if state.actions.contains(&action) && control(ui, state, action, &label, icon, kind) {
                command = Some(choice);
            }
        }
    });
    });
    if let Some(recovery) = &state.recovery {
        ui.add_space(16.0);
        crate::accordion::show(
            ui,
            &crate::accordion::Props {
                id: "maps.recovery.details",
                label: &format_message!(intl, default_message: "Details"),
                default_open: false,
                inline_padding,
            },
            |ui| {
                ui.set_max_width(ui.available_width().min(560.0));
                ui.label(
                    egui::RichText::new(format_message!(intl, default_message: "Plan")).weak(),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(&recovery.plan).monospace().size(12.0))
                        .wrap(),
                );
            },
        );
    }
    command
}

fn map_panel(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    panel_frame(ui).inner_margin(16).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 4.0;
        content(ui);
    });
}

fn panel_frame(ui: &Ui) -> egui::Frame {
    egui::Frame::group(ui.style()).fill(crate::theme::color32(
        crate::theme::palette(ui)
            .surfaces()
            .layer(garmin_color::theme::Level::One),
    ))
}

fn outcome(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Command> {
    ui.heading(match state.phase {
        Phase::Completed => format_message!(intl, default_message: "Map operation completed"),
        Phase::Cancelled => format_message!(intl, default_message: "Map operation cancelled"),
        _ => format_message!(intl, default_message: "Map operation failed"),
    });
    ui.add_space(16.0);
    control(
        ui,
        state,
        Action::Refresh,
        &format_message!(intl, default_message: "Inspect device again"),
        icons::ARROWS_CLOCKWISE,
        button::Kind::Secondary,
    )
    .then_some(Command::Refresh)
}

fn history(ui: &mut Ui, intl: &Intl, state: &State) {
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(560.0));
        if !state.events.is_empty() {
            ui.add_space(16.0);
            crate::accordion::show(
                ui,
                &crate::accordion::Props {
                    id: "maps.operation.history",
                    label: &format_message!(intl, default_message: "Operation history"),
                    default_open: false,
                    inline_padding: 16,
                },
                |ui| {
                    for event in &state.events {
                        map_panel(ui, |ui| {
                            ui.spacing_mut().interact_size.y = 16.0;
                            ui.horizontal(|ui| {
                                let icon = match event.status {
                                    garmin_service_api::maps::ProgressStatus::Completed => {
                                        icons::CHECK
                                    }
                                    garmin_service_api::maps::ProgressStatus::Failed => {
                                        icons::WARNING
                                    }
                                    _ => icons::ARROWS_CLOCKWISE,
                                };
                                ui.add(
                                    icon.image(crate::theme::palette(ui).content().text_primary())
                                        .fit_to_exact_size(egui::Vec2::splat(16.0)),
                                );
                                ui.label(typography::semibold(&event.label));
                            });
                            ui.label(
                                egui::RichText::new(elapsed(event.elapsed_ms))
                                    .size(12.0)
                                    .weak(),
                            );
                            if let Some(path) = &event.path {
                                ui.label(egui::RichText::new(path).size(12.0).weak());
                            }
                        });
                    }
                },
            );
        }
        if !state.history.is_empty() {
            ui.add_space(24.0);
            ui.label(typography::semibold(
                format_message!(intl, default_message: "Previous outcomes"),
            ));
            ui.add_space(8.0);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for outcome in state.history.iter().rev() {
                    outcome_row(ui, intl, outcome);
                }
            });
        }
    });
}

fn outcome_row(ui: &mut Ui, intl: &Intl, outcome: &garmin_service_api::maps::Outcome) {
    let label = match outcome.phase {
        Phase::Completed => format_message!(intl, default_message: "Map operation completed"),
        Phase::Cancelled => format_message!(intl, default_message: "Map operation cancelled"),
        _ => format_message!(intl, default_message: "Map operation failed"),
    };
    crate::accordion::show(
        ui,
        &crate::accordion::Props {
            id: &format!("maps.outcome.{}", outcome.id),
            label: &label,
            default_open: false,
            inline_padding: 16,
        },
        |ui| {
            typography::body(ui, &outcome.message);
        },
    );
}

fn elapsed(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn control(
    ui: &mut Ui,
    state: &State,
    action: Action,
    label: &str,
    icon: icons::Icon,
    kind: button::Kind,
) -> bool {
    let props = button::Props {
        label,
        icon: Some(icon),
        kind,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled: state.actions.contains(&action),
    };
    let response = props.show(ui);
    crate::semantics::target(ui, &response, format!("maps.{action:?}"));
    response.clicked()
}
