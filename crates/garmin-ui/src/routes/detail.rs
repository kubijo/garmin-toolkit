use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_service_api::course_transfer::CourseTransferPhase;
use garmin_service_api::routes::{CourseVersion, RouteRequest};

use super::{Action, State, Workspace, action_button, action_row, error, sport_label, widgets};
use crate::{button, icons, modal, typography};

impl Workspace {
    pub(super) fn detail(&mut self, ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
        let route = state.detail.as_ref()?;
        let enabled = !state.busy && state.pending.is_none();
        let mut action = header(ui, intl, state);
        let retry = error(ui, intl, state);
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
        if state.transfer.is_some() {
            action = transfer_dialog(ui, intl, state).or(action);
        }
        action.or(retry)
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
                cancel_disabled: None,
                backdrop_closes: Some(true),
                primary: Some(modal::Primary {
                    label: &delete,
                    icon: Some(icons::TRASH),
                    kind: modal::PrimaryKind::Danger,
                    enabled: !state.busy && state.pending.is_none(),
                }),
            },
            |_| {},
        );
        if let Some(primary) = &output.primary {
            crate::semantics::target(ui, primary, "routes.delete.confirm");
        }
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
            button::Kind::Tertiary,
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
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
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
                    icons::SEND_TO_DEVICE,
                    button::Kind::Primary,
                    enabled,
                ) {
                    action = Some(Action::BeginTransfer(version.operation));
                }
                if widgets::icon_button(
                    ui,
                    &format!("routes.download.{}", version.id),
                    &download,
                    icons::DOWNLOAD_SIMPLE,
                    button::Kind::Secondary,
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

fn transfer_dialog(ui: &mut Ui, intl: &Intl, state: &State) -> Option<Action> {
    let transfer = state.transfer.as_ref()?;
    let title = format_message!(intl, default_message: "Send FIT Course");
    let running = transfer
        .status
        .as_ref()
        .is_some_and(|status| matches!(status.phase, CourseTransferPhase::Running));
    let status_open =
        transfer.status.is_some() && transfer.review.is_none() && transfer.cleanup.is_none();
    let cancel_label = if status_open {
        format_message!(intl, default_message: "Cancel")
    } else {
        format_message!(intl, default_message: "Close")
    };
    let description = transfer_description(intl, transfer);
    let primary = transfer_primary(intl, transfer);
    let back_label = format_message!(intl, default_message: "Back");
    // Each step has different intrinsic height. A fresh area ID prevents egui from
    // retaining a taller dialog's size after the transfer changes state.
    let phase = if transfer.review.is_some() {
        "review"
    } else if transfer.cleanup.is_some() {
        "cleanup"
    } else if let Some(status) = &transfer.status {
        match status.phase {
            CourseTransferPhase::Running => "running",
            CourseTransferPhase::Verified => "verified",
            CourseTransferPhase::Accepted => "accepted",
            CourseTransferPhase::Missing => "missing",
            CourseTransferPhase::NeedsReview(_) => "needs-review",
        }
    } else if transfer.device_key.is_some() {
        "storage"
    } else {
        "device"
    };
    let output = modal::show_with_footer(
        ui,
        egui::Id::new(("routes-transfer", phase)),
        &modal::Props {
            title: &title,
            description: Some(&description),
            size: modal::Size::Medium,
            presentation: modal::Presentation::Modal,
            cancel_label: Some(&cancel_label),
            cancel_disabled: status_open.then_some(!running || state.busy),
            backdrop_closes: Some(false),
            primary: primary
                .as_ref()
                .map(|(label, icon, kind, _, _)| modal::Primary {
                    label,
                    icon: Some(*icon),
                    kind: *kind,
                    enabled: !(state.busy || status_open && running),
                }),
        },
        modal::FooterOptions {
            back: transfer.can_go_back().then_some(modal::Back {
                label: &back_label,
                icon: icons::CARET_LEFT,
                enabled: !state.busy,
            }),
            cancel_icon: Some(icons::X),
        },
        |ui| transfer_body(ui, intl, state, transfer),
    );
    if let (Some(response), Some((_, _, _, _, id))) = (&output.primary, &primary) {
        crate::semantics::target(ui, response, *id);
    }
    if let Some(response) = &output.back {
        crate::semantics::target(ui, response, "routes.transfer.back");
    }
    if let Some(response) = &output.cancel {
        crate::semantics::target(
            ui,
            response,
            if status_open {
                "routes.transfer.cancel"
            } else {
                "routes.transfer.close"
            },
        );
    }
    if output.back.as_ref().is_some_and(egui::Response::clicked) {
        return Some(Action::BackTransfer);
    }
    match output.action {
        Some(modal::Action::Cancel)
            if status_open && output.cancel.as_ref().is_some_and(egui::Response::clicked) =>
        {
            Some(Action::CancelTransfer {
                device_key: transfer.device_key.clone()?,
                transfer: transfer.status.as_ref()?.transfer,
            })
        }
        Some(modal::Action::Cancel) if status_open => None,
        Some(modal::Action::Cancel) => Some(Action::DismissTransfer),
        Some(modal::Action::Primary) => primary.map(|(_, _, _, action, _)| action),
        None => output.inner,
    }
}

fn transfer_description(intl: &Intl, transfer: &super::state::TransferState) -> String {
    if transfer.device_key.is_none() {
        format_message!(intl, default_message: "Choose a connected device for this transfer.")
    } else if let Some(review) = &transfer.review {
        format_message!(intl, default_message: "Review sending Course version {version}", values: { version: u64::from(review.version) })
    } else if transfer.cleanup.is_some() {
        format_message!(intl, default_message: "Review this partial Course upload before deleting it.")
    } else if let Some(status) = &transfer.status {
        match status.phase {
            CourseTransferPhase::Running => {
                format_message!(intl, default_message: "Sending the FIT file to your device.")
            }
            CourseTransferPhase::Verified | CourseTransferPhase::Accepted => {
                format_message!(
                    intl,
                    id: "routes.transfer.verified_description",
                    default_message: "Disconnect the device to check its Courses menu."
                )
            }
            CourseTransferPhase::Missing => {
                format_message!(
                    intl,
                    id: "routes.transfer.file_missing_description",
                    default_message: "We checked the device's upload folder."
                )
            }
            CourseTransferPhase::NeedsReview(_) => {
                format_message!(intl, default_message: "The transfer could not be verified. Review the status below.")
            }
        }
    } else {
        format_message!(intl, default_message: "Choose device storage for this Course.")
    }
}

fn transfer_primary(
    intl: &Intl,
    transfer: &super::state::TransferState,
) -> Option<(
    String,
    icons::Icon,
    modal::PrimaryKind,
    Action,
    &'static str,
)> {
    let device_key = transfer.device_key.as_ref()?.clone();
    if let Some(review) = &transfer.review {
        return Some((
            format_message!(intl, default_message: "Confirm transfer"),
            icons::SEND_TO_DEVICE,
            modal::PrimaryKind::Confirm,
            Action::ApproveTransfer {
                device_key,
                approval: review.approval,
            },
            "routes.transfer.approve",
        ));
    }
    if let Some(cleanup) = &transfer.cleanup {
        return Some((
            format_message!(intl, default_message: "Delete partial upload"),
            icons::TRASH,
            modal::PrimaryKind::Danger,
            Action::ApproveTransferCleanup {
                device_key,
                approval: cleanup.approval,
            },
            "routes.transfer.cleanup.approve",
        ));
    }
    transfer.status.as_ref()?;
    Some((
        format_message!(intl, default_message: "Close"),
        icons::X,
        modal::PrimaryKind::Confirm,
        Action::DismissTransfer,
        "routes.transfer.close",
    ))
}

fn transfer_body(
    ui: &mut Ui,
    intl: &Intl,
    state: &State,
    transfer: &super::state::TransferState,
) -> Option<Action> {
    ui.spacing_mut().item_spacing.y = 8.0;
    let enabled = !state.busy;
    if transfer.device_key.is_none() {
        return transfer_devices(ui, intl, state, transfer.generation);
    }
    let device_key = transfer.device_key.as_ref()?;
    if let Some(review) = &transfer.review {
        transfer_review(ui, intl, review);
        return None;
    }
    if let Some(cleanup) = &transfer.cleanup {
        transfer_cleanup(ui, cleanup);
        return None;
    }
    if let Some(status) = &transfer.status {
        transfer_status(ui, intl, status, device_key, enabled)
    } else {
        transfer_storage(ui, intl, transfer, device_key, enabled)
    }
}

fn transfer_devices(
    ui: &mut Ui,
    intl: &Intl,
    state: &State,
    generation: garmin_model::route::CourseGenerationOperationId,
) -> Option<Action> {
    if state.devices.is_empty() {
        ui.label(format_message!(intl, default_message: "Connect a device to send this Course."));
    }
    for device in &state.devices {
        let details = transfer_device_details(intl, device);
        let enabled = !state.busy && ui.is_enabled();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 56.0),
            if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                enabled,
                format!("{} · {details}", device.name),
            )
        });
        let palette = crate::theme::palette(ui);
        let states = palette.buttons().secondary();
        let colors = if !enabled {
            states.disabled()
        } else if response.is_pointer_button_down_on() {
            states.active()
        } else if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            states.hover()
        } else {
            states.rest()
        };
        ui.painter().rect_filled(
            rect,
            crate::theme::CONTROL_RADIUS,
            crate::theme::color32(colors.background()),
        );
        let content_rect = rect.shrink2(egui::vec2(16.0, 8.0));
        let mut content = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        content.set_clip_rect(content_rect);
        content.spacing_mut().item_spacing.y = 4.0;
        let foreground = crate::theme::color32(colors.foreground());
        let detail_color = if enabled {
            crate::theme::color32(colors.foreground().with_alpha(0xd0))
        } else {
            foreground
        };
        content.add(
            egui::Label::new(
                egui::RichText::new(device.name.as_str())
                    .color(foreground)
                    .strong(),
            )
            .truncate()
            .selectable(false),
        );
        content.add(
            egui::Label::new(
                egui::RichText::new(details.as_str())
                    .size(12.0)
                    .color(detail_color),
            )
            .truncate()
            .selectable(false),
        );
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

fn transfer_device_details(intl: &Intl, device: &garmin_service_api::DeviceSnapshot) -> String {
    let identifier_label = format_message!(intl, default_message: "Device ID");
    let software_label = format_message!(intl, default_message: "Software");
    let identifier = device
        .identifier
        .map(|value| format!("{identifier_label}: {value}"));
    let software = device
        .software_version
        .map(|value| format!("{software_label}: {}.{:02}", value / 100, value % 100));
    let missing_detail = identifier.is_none() || software.is_none();
    let mut details = [identifier, software]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if missing_detail {
        details.push(match device.inspection {
            garmin_service_api::InspectionState::Running if device.report.is_some() => {
                format_message!(intl, default_message: "Refreshing…")
            }
            garmin_service_api::InspectionState::Running => {
                format_message!(intl, default_message: "Inspecting…")
            }
            garmin_service_api::InspectionState::Ready => {
                format_message!(intl, default_message: "Ready")
            }
            garmin_service_api::InspectionState::Failed => {
                format_message!(intl, default_message: "Some device information is unavailable")
            }
        });
    }
    details.join(" · ")
}

fn transfer_review(
    ui: &mut Ui,
    intl: &Intl,
    review: &garmin_service_api::course_transfer::CourseTransferReview,
) {
    let palette = crate::theme::palette(ui);
    let fill = crate::theme::color32(palette.surfaces().layer(garmin_color::theme::Level::Two));
    let muted = crate::theme::color32(palette.content().text_secondary());
    egui::Frame::NONE
        .fill(fill)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(
                egui::RichText::new(format_message!(intl, default_message: "To device"))
                    .size(12.0)
                    .color(muted),
            );
            ui.label(typography::semibold(&review.target.device_name).size(16.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{} · {}",
                        review.target.storage_label, review.target.directory
                    ))
                    .color(muted),
                )
                .wrap(),
            );
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(16.0);
            ui.label(
                egui::RichText::new(format_message!(intl, default_message: "FIT Course file"))
                    .size(12.0)
                    .color(muted),
            );
            ui.label(
                typography::semibold(crate::text::format_bytes(review.byte_count.as_u64()))
                    .size(16.0),
            );
            ui.add(egui::Label::new(egui::RichText::new(&review.file_name).color(muted)).wrap());
        });
    ui.add_space(16.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(format_message!(intl,
                default_message: "Sending this Course will not pair the device or change its existing pairing."))
                .color(muted),
        )
        .wrap(),
    );
}

fn transfer_cleanup(
    ui: &mut Ui,
    cleanup: &garmin_service_api::course_transfer::CourseCleanupReview,
) {
    ui.label(format!(
        "{} / {} · {} bytes",
        cleanup.target.directory, cleanup.file_name, cleanup.byte_count
    ));
}

fn transfer_status(
    ui: &mut Ui,
    intl: &Intl,
    status: &garmin_service_api::course_transfer::CourseTransferStatus,
    device_key: &str,
    enabled: bool,
) -> Option<Action> {
    let phase = transfer_phase_label(intl, status);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let phase_label = ui.label(typography::semibold(phase));
        crate::semantics::target(ui, &phase_label, "routes.transfer.status");
        ui.add(
            egui::Label::new(
                egui::RichText::new(&status.file_name).color(crate::theme::color32(
                    crate::theme::palette(ui).content().text_secondary(),
                )),
            )
            .wrap(),
        );
    });
    if matches!(status.phase, CourseTransferPhase::Running) {
        ui.add_space(8.0);
        crate::progress::indeterminate(ui, 8.0);
        if status
            .progress
            .as_ref()
            .is_some_and(|progress| progress.finishing)
        {
            ui.label(format_message!(intl, default_message: "Upload complete. Finalizing and checking the file on your device."));
        }
    }
    if let CourseTransferPhase::NeedsReview(reason) = &status.phase {
        ui.label(reason);
    }
    if matches!(status.phase, CourseTransferPhase::Missing) {
        ui.add_space(8.0);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.add(
                egui::Label::new(format_message!(
                    intl,
                    id: "routes.transfer.file_missing_explanation",
                    default_message: "Import status is unknown."
                ))
                .wrap()
                .selectable(false),
            );
            ui.add(
                egui::Label::new(format_message!(
                    intl,
                    id: "routes.transfer.file_missing_next_step",
                    default_message: "Disconnect the device and check its Courses menu."
                ))
                .wrap()
                .selectable(false),
            );
        });
        return None;
    }
    let (label, id, action) = match status.phase {
        CourseTransferPhase::NeedsReview(_) => (
            format_message!(intl, default_message: "Review partial cleanup"),
            "routes.transfer.cleanup.review",
            Action::PrepareTransferCleanup {
                device_key: device_key.to_owned(),
                transfer: status.transfer,
            },
        ),
        CourseTransferPhase::Missing
        | CourseTransferPhase::Running
        | CourseTransferPhase::Verified
        | CourseTransferPhase::Accepted => return None,
    };
    transfer_button(ui, &label, id, enabled).then_some(action)
}

fn transfer_phase_label(
    intl: &Intl,
    status: &garmin_service_api::course_transfer::CourseTransferStatus,
) -> String {
    match &status.phase {
        CourseTransferPhase::Running => {
            if status
                .progress
                .as_ref()
                .is_some_and(|progress| progress.finishing)
            {
                format_message!(intl, default_message: "Finishing on device")
            } else {
                format_message!(intl, default_message: "Transfer in progress")
            }
        }
        CourseTransferPhase::Verified | CourseTransferPhase::Accepted => {
            format_message!(intl, default_message: "FIT file verified")
        }
        CourseTransferPhase::Missing => {
            format_message!(
                intl,
                id: "routes.transfer.file_missing_status",
                default_message: "FIT file not found"
            )
        }
        CourseTransferPhase::NeedsReview(_) => {
            format_message!(intl, default_message: "Transfer needs review")
        }
    }
}

fn transfer_storage(
    ui: &mut Ui,
    intl: &Intl,
    transfer: &super::state::TransferState,
    device_key: &str,
    enabled: bool,
) -> Option<Action> {
    if transfer.targets.is_empty() {
        ui.label(
            format_message!(intl, default_message: "No writable FIT Course location was found."),
        );
    }
    for target in &transfer.targets {
        let label = format!("{} · {}", target.storage_label, target.directory);
        let mut button = widgets::button(&label, button::Kind::Secondary, enabled);
        button.width = button::Width::Fill;
        let response = button.show(ui);
        crate::semantics::target(
            ui,
            &response,
            format!("routes.transfer.storage.{}", target.storage_id),
        );
        if response.clicked() {
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
    widgets::action_button(ui, id, label, button::Kind::Secondary, enabled)
}
