use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_service_api::course_transfer::CourseTransferPhase;
use garmin_service_api::routes::{CourseVersion, RouteRequest};

use super::{Action, State, Workspace, action_button, action_row, sport_label, widgets};
use crate::{button, icons, modal, typography};

impl Workspace {
    pub(super) fn detail(&mut self, ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
        let route = state.detail.as_ref()?;
        let enabled = !state.busy && state.pending.is_none();
        let mut action = header(ui, intl, state);
        if state.transfer.is_some() {
            ui.add_space(16.0);
            let panel = ui.scope(|ui| transfer_panel(ui, intl, state));
            action = panel.inner.or(action);
            if self.scroll_to_transfer {
                ui.scroll_to_rect(panel.response.rect, Some(egui::Align::Min));
                self.scroll_to_transfer = false;
            }
        }
        ui.add_space(16.0);
        self.preview(ui, intl, state, route.geometry, &route.revision.to_string());
        ui.add_space(24.0);
        ui.heading(format_message!(intl, default_message: "FIT Course"));
        let current = state
            .versions
            .iter()
            .find(|version| version.current_encoder);
        if let Some(version) = current {
            action = version_row(ui, intl, version, enabled, &mut self.deleting).or(action);
        } else if enabled {
            action = empty_courses(ui, intl, state).or(action);
        }
        let history = state
            .versions
            .iter()
            .filter(|version| current.is_none_or(|current| current.id != version.id));
        if history.clone().next().is_some() || state.next_versions.is_some() {
            ui.add_space(16.0);
            let response = ui.collapsing(
                format_message!(intl, default_message: "Previous FIT files"),
                |ui| {
                    for version in history {
                        ui.push_id(version.id, |ui| {
                            action = version_row(ui, intl, version, enabled, &mut self.deleting)
                                .or(action.take());
                        });
                    }
                    action = more_versions(ui, intl, state, enabled).or(action.take());
                },
            );
            crate::semantics::target(ui, &response.header_response, "routes.history");
        }
        action
    }

    pub(super) fn delete_confirmation(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        state: &State,
    ) -> Option<Action> {
        let version = self.deleting.as_ref()?;
        if state
            .detail
            .as_ref()
            .is_none_or(|route| route.revision != version.revision)
            || !state.versions.iter().any(|item| item.id == version.id)
        {
            self.deleting = None;
            return None;
        }
        let title = format_message!(intl, default_message: "Delete Course version {version}?", values: { version: u64::from(version.version.get()) });
        let description = format_message!(intl, default_message: "This removes the generated FIT file. The route and original GPX are kept.");
        let delete = format_message!(intl, default_message: "Delete FIT Course");
        let cancel = format_message!(intl, default_message: "Cancel");
        let output = modal::show(
            ui,
            egui::Id::new(("routes.delete", version.id)),
            &modal::Props {
                title: &title,
                description: Some(&description),
                size: modal::Size::Medium,
                presentation: modal::Presentation::Modal,
                cancel_label: Some(&cancel),
                backdrop_closes: Some(true),
                primary: modal::Primary {
                    label: &delete,
                    icon: Some(icons::TRASH),
                    kind: modal::PrimaryKind::Danger,
                    enabled: !state.busy && state.pending.is_none(),
                },
            },
            |_| {},
        );
        crate::semantics::target(ui, &output.primary, "routes.delete.confirm");
        if let Some(cancel) = &output.cancel {
            crate::semantics::target(ui, cancel, "routes.delete.cancel");
        }
        match output.action {
            Some(modal::Action::Primary) => {
                let action = Action::Request(RouteRequest::DeleteCourse {
                    revision: version.revision,
                    generation: version.id,
                });
                self.deleting = None;
                Some(action)
            }
            Some(modal::Action::Cancel) => {
                self.deleting = None;
                None
            }
            None => None,
        }
    }
}

fn header(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    state.detail.as_ref()?;
    let enabled = !state.busy && state.pending.is_none();
    let original = format_message!(intl, default_message: "Download GPX");
    let mut source_button = widgets::button(&original, button::Kind::Tertiary, enabled);
    source_button.icon = Some(icons::DOWNLOAD_SIMPLE);
    let actions_width = state
        .source
        .as_ref()
        .map_or(0.0, |_| source_button.natural_width(ui));
    let actions = |ui: &mut Ui| {
        if let Some(source) = &state.source {
            action_row(
                ui,
                [(
                    "routes.source",
                    Action::Download(source.artifact),
                    source_button,
                )],
            )
        } else {
            None
        }
    };
    let mut action = None;
    if ui.available_width() >= actions_width + 344.0 {
        let title_width = ui.available_width() - actions_width - 8.0;
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(title_width, 40.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width(title_width);
                    action = title_row(ui, intl, state);
                },
            );
            action = actions(ui).or(action.take());
        });
    } else {
        action = title_row(ui, intl, state);
        action = actions(ui).or(action);
    }
    action
}

fn title_row(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    let route = state.detail.as_ref()?;
    let mut action = None;
    ui.horizontal(|ui| {
        if widgets::icon_button(
            ui,
            "routes.back",
            &format_message!(intl, default_message: "Back to routes"),
            icons::CARET_LEFT,
            !state.busy && state.pending.is_none(),
        ) {
            action = Some(Action::Request(RouteRequest::List { offset: 0 }));
        }
        let sport = sport_label(intl, route.sport);
        let sport_width = ui
            .fonts_mut(|fonts| {
                fonts.layout_no_wrap(
                    sport.clone(),
                    egui::TextStyle::Body.resolve(ui.style()),
                    ui.visuals().text_color(),
                )
            })
            .size()
            .x;
        let title_width = ui
            .painter()
            .layout_no_wrap(
                route.name.to_string(),
                egui::TextStyle::Heading.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .x
            .min((ui.available_width() - sport_width - 40.0).max(40.0));
        let title = ui
            .add_sized(
                egui::vec2(title_width, 40.0),
                egui::Label::new(egui::RichText::new(route.name.as_str()).heading())
                    .truncate()
                    .halign(egui::Align::Min)
                    .selectable(false),
            )
            .on_hover_text(route.name.as_str());
        crate::semantics::target(ui, &title, "routes.title");
        let sport = ui.label(egui::RichText::new(sport).color(crate::theme::color32(
            crate::theme::palette(ui).content().text_secondary(),
        )));
        crate::semantics::target(ui, &sport, "routes.sport");
        widgets::busy(ui, state);
    });
    action
}

fn more_versions(ui: &mut Ui, intl: &Intl, state: &State, enabled: bool) -> Option<Action> {
    let offset = state.next_versions?;
    let revision = state.detail.as_ref()?.revision;
    action_button(
        ui,
        "routes.versions.next",
        &format_message!(intl, default_message: "More Course versions"),
        button::Kind::Tertiary,
        enabled,
    )
    .then_some(Action::Request(RouteRequest::Versions { revision, offset }))
}

fn empty_courses(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    let route = state.detail.as_ref()?;
    let mut action = None;
    let palette = crate::theme::palette(ui);
    let muted = crate::theme::color32(palette.content().text_secondary());
    egui::Frame::NONE
        .fill(crate::theme::color32(palette.surfaces().layer(garmin_color::theme::Level::One)))
        .stroke(egui::Stroke::new(1.0, crate::theme::color32(palette.borders().subtle())))
        .inner_margin(24)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.add(icons::FOLDER_DASHED.mask().tint(muted).fit_to_exact_size(egui::Vec2::splat(32.0)));
                ui.label(typography::semibold(format_message!(intl, default_message: "No current FIT file")));
                ui.add(egui::Label::new(typography::body_text(format_message!(intl, default_message: "Generate a FIT file to download this route for your device.")).color(muted)).wrap());
                let label = format_message!(intl, default_message: "Generate FIT");
                let mut props = widgets::button(&label, button::Kind::Primary, route.geometry && state.points_ready);
                props.icon = Some(icons::PLUS);
                let response = props.show(ui);
                crate::semantics::target(ui, &response, "routes.generate");
                if response.clicked() {
                    action = Some(Action::Request(RouteRequest::PrepareGeneration { revision: route.revision }));
                }
            });
        });
    action
}

fn version_row(
    ui: &mut Ui,
    intl: &Intl,
    version: &CourseVersion,
    enabled: bool,
    deleting: &mut Option<CourseVersion>,
) -> Option<Action> {
    let title = format_message!(intl, default_message: "Course version {version} · {size}", values: { version: u64::from(version.version.get()), size: crate::text::format_bytes(version.byte_count.as_u64()) });
    let date = intl.dates().datetime(
        version
            .generated_at
            .as_jiff()
            .to_zoned(jiff::tz::TimeZone::UTC)
            .datetime(),
    );
    let date = format_message!(intl, default_message: "Generated (UTC): {date}", values: { date: date.as_str() });
    let download = format_message!(intl, default_message: "Download FIT");
    let send = format_message!(intl, default_message: "Send FIT to device");
    let delete = format_message!(intl, default_message: "Delete FIT Course");
    let fill = crate::theme::color32(
        crate::theme::palette(ui)
            .surfaces()
            .layer(garmin_color::theme::Level::One),
    );
    let mut action = None;
    egui::Frame::NONE
        .fill(fill)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let text_width = ui.available_width() - 144.0;
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, 40.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(text_width);
                        ui.label(typography::semibold(&title));
                        typography::body(ui, &date);
                    },
                );
                if widgets::icon_button(
                    ui,
                    &format!("routes.send.{}", version.id),
                    &send,
                    icons::UPLOAD_SIMPLE,
                    enabled,
                ) {
                    action = Some(Action::BeginTransfer(version.operation));
                }
                if widgets::icon_button(
                    ui,
                    &format!("routes.download.{}", version.id),
                    &download,
                    icons::DOWNLOAD_SIMPLE,
                    enabled,
                ) {
                    action = Some(Action::Download(version.artifact));
                }
                let response = button::IconProps {
                    label: &delete,
                    icon: icons::TRASH,
                    kind: button::Kind::Danger,
                    size: crate::Size::Medium,
                    enabled,
                }
                .show(ui);
                crate::semantics::target(ui, &response, format!("routes.delete.{}", version.id));
                if response.clicked() {
                    *deleting = Some(version.clone());
                }
            });
        });
    action
}

fn transfer_panel(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    let transfer = state.transfer.as_ref()?;
    let mut action = None;
    egui::Frame::NONE
        .fill(crate::theme::color32(
            crate::theme::palette(ui)
                .surfaces()
                .layer(garmin_color::theme::Level::One),
        ))
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.heading(format_message!(intl, default_message: "Send FIT Course"));
                let close = ui.button(format_message!(intl, default_message: "Close"));
                crate::semantics::target(ui, &close, "routes.transfer.close");
                if close.clicked() {
                    action = Some(Action::DismissTransfer);
                }
            });
            ui.add_space(8.0);
            let enabled = !state.busy;
            if transfer.device_key.is_none() {
                action = transfer_devices(ui, intl, state, transfer.generation);
                return;
            }
            let Some(device_key) = &transfer.device_key else {
                return;
            };
            if let Some(review) = &transfer.review {
                action = transfer_review(ui, intl, review, device_key, enabled);
                return;
            }
            if let Some(cleanup) = &transfer.cleanup {
                action = transfer_cleanup(ui, intl, cleanup, device_key, enabled);
                return;
            }
            if let Some(status) = &transfer.status {
                action = transfer_status(ui, intl, status, device_key, enabled);
            } else {
                action = transfer_storage(ui, intl, transfer, device_key, enabled);
            }
            for receipt in &transfer.receipts {
                if transfer
                    .status
                    .as_ref()
                    .is_some_and(|current| current.transfer == receipt.transfer)
                {
                    continue;
                }
                if transfer_button(
                    ui,
                    &format_message!(intl,
                    default_message: "Check previous transfer"),
                    &format!("routes.transfer.previous.{}", receipt.transfer),
                    enabled,
                ) {
                    action = Some(Action::PollTransfer {
                        device_key: device_key.clone(),
                        transfer: receipt.transfer,
                    });
                }
            }
        });
    action
}

fn transfer_devices(
    ui: &mut Ui,
    intl: &Intl,
    state: &State,
    generation: garmin_model::route::CourseGenerationOperationId,
) -> Option<Action> {
    ui.label(
        format_message!(intl, default_message: "Choose a connected device for this transfer."),
    );
    if state.devices.is_empty() {
        ui.label(format_message!(intl, default_message: "Connect a device to send this Course."));
    }
    for device in &state.devices {
        let response = ui.add_enabled(!state.busy, egui::Button::new(&device.name));
        crate::semantics::target(
            ui,
            &response,
            format!("routes.transfer.device.{}", device.key),
        );
        if response.clicked() {
            return Some(Action::ChooseTransferDevice {
                generation,
                device_key: device.key.clone(),
            });
        }
    }
    None
}

fn transfer_review(
    ui: &mut Ui,
    intl: &Intl,
    review: &garmin_service_api::course_transfer::CourseTransferReview,
    device_key: &str,
    enabled: bool,
) -> Option<Action> {
    ui.label(format_message!(intl, default_message: "Review sending Course version {version}", values: { version: u64::from(review.version) }));
    ui.label(format!(
        "{} · {} · {} · {} bytes",
        review.target.device_name,
        review.target.storage_label,
        review.file_name,
        review.byte_count.as_u64()
    ));
    ui.label(format!(
        "{} / {}",
        review.target.directory, review.file_name
    ));
    ui.label(review.digest.to_string());
    ui.label(format_message!(intl,
        default_message: "This transfer does not pair the device or change its existing pairing."));
    ui.add_space(8.0);
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(format_message!(intl,
        default_message: "Confirm transfer")),
    );
    crate::semantics::target(ui, &response, "routes.transfer.approve");
    response.clicked().then(|| Action::ApproveTransfer {
        device_key: device_key.to_owned(),
        approval: review.approval,
    })
}

fn transfer_cleanup(
    ui: &mut Ui,
    intl: &Intl,
    cleanup: &garmin_service_api::course_transfer::CourseCleanupReview,
    device_key: &str,
    enabled: bool,
) -> Option<Action> {
    ui.label(format_message!(intl, default_message: "Delete this partial Course upload?"));
    ui.label(format!(
        "{} / {} · {} bytes",
        cleanup.target.directory, cleanup.file_name, cleanup.byte_count
    ));
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(format_message!(intl,
        default_message: "Delete partial upload")),
    );
    crate::semantics::target(ui, &response, "routes.transfer.cleanup.approve");
    response.clicked().then(|| Action::ApproveTransferCleanup {
        device_key: device_key.to_owned(),
        approval: cleanup.approval,
    })
}

fn transfer_status(
    ui: &mut Ui,
    intl: &Intl,
    status: &garmin_service_api::course_transfer::CourseTransferStatus,
    device_key: &str,
    enabled: bool,
) -> Option<Action> {
    let phase = match &status.phase {
        CourseTransferPhase::Running => {
            format_message!(intl, default_message: "Transfer in progress")
        }
        CourseTransferPhase::Verified => {
            format_message!(intl, default_message: "Verified on device")
        }
        CourseTransferPhase::Accepted => {
            format_message!(intl, default_message: "Confirmed on device")
        }
        CourseTransferPhase::Missing => {
            format_message!(intl, default_message: "Course file is missing")
        }
        CourseTransferPhase::NeedsReview(_) => {
            format_message!(intl, default_message: "Transfer needs review")
        }
    };
    let phase_label = ui.label(format!("{}: {}", status.file_name, phase));
    crate::semantics::target(ui, &phase_label, "routes.transfer.status");
    if let CourseTransferPhase::NeedsReview(reason) = &status.phase {
        ui.label(reason);
    }
    let mut action = None;
    ui.horizontal(|ui| {
        if transfer_button(
            ui,
            &format_message!(intl, default_message: "Check device"),
            "routes.transfer.check",
            enabled,
        ) {
            action = Some(Action::PollTransfer {
                device_key: device_key.to_owned(),
                transfer: status.transfer,
            });
        }
        if matches!(status.phase, CourseTransferPhase::Running)
            && transfer_button(
                ui,
                &format_message!(intl, default_message: "Cancel transfer"),
                "routes.transfer.cancel",
                enabled,
            )
        {
            action = Some(Action::CancelTransfer {
                device_key: device_key.to_owned(),
                transfer: status.transfer,
            });
        }
        if matches!(status.phase, CourseTransferPhase::Verified)
            && transfer_button(
                ui,
                &format_message!(intl,
                default_message: "Confirm visible on device"),
                "routes.transfer.accept",
                enabled,
            )
        {
            action = Some(Action::AcceptTransfer {
                device_key: device_key.to_owned(),
                transfer: status.transfer,
            });
        }
        if matches!(status.phase, CourseTransferPhase::NeedsReview(_))
            && transfer_button(
                ui,
                &format_message!(intl,
                default_message: "Review partial cleanup"),
                "routes.transfer.cleanup.review",
                enabled,
            )
        {
            action = Some(Action::PrepareTransferCleanup {
                device_key: device_key.to_owned(),
                transfer: status.transfer,
            });
        }
    });
    action
}

fn transfer_storage(
    ui: &mut Ui,
    intl: &Intl,
    transfer: &super::state::TransferState,
    device_key: &str,
    enabled: bool,
) -> Option<Action> {
    ui.label(format_message!(intl, default_message: "Choose device storage"));
    if transfer.targets.is_empty() {
        ui.label(
            format_message!(intl, default_message: "No writable FIT Course location was found."),
        );
    }
    for target in &transfer.targets {
        if transfer_button(
            ui,
            &format!("{} · {}", target.storage_label, target.directory),
            &format!("routes.transfer.storage.{}", target.storage_id),
            enabled,
        ) {
            return Some(Action::PrepareTransfer {
                generation: transfer.generation,
                device_key: device_key.to_owned(),
                storage_id: target.storage_id.clone(),
            });
        }
    }
    None
}

fn transfer_button(ui: &mut Ui, label: &str, id: &str, enabled: bool) -> bool {
    let response = ui.add_enabled(enabled, egui::Button::new(label));
    crate::semantics::target(ui, &response, id);
    response.clicked()
}
