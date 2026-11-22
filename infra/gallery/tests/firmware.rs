//! Both clients render these controls against a host-issued review.
use egui_kittest::{Harness, kittest::Queryable as _};
use garmin_cli_tui::preview::{PreviewScreen, firmware_components};
use garmin_i18n::{Intl, Language, Translations};
use garmin_model::value::Timestamp;
use garmin_service_api::firmware::{
    Action, Inspection, InstalledStatus, Observation, Phase, ReceiptStatus, RetainedTransfer,
    Review, State, Transcript,
};
use uuid::Uuid;

fn formatter(language: Language) -> Intl {
    Translations::bundled()
        .expect("bundled translations")
        .formatter(language)
        .expect("fixture language")
}

#[test]
fn selecting_a_retained_transfer_inspects_only_that_receipt() {
    let intl = formatter(Language::English);
    let time = Timestamp::from_unix_seconds(1_795_262_400).expect("fixture timestamp");
    let date = intl.dates().datetime(
        time.as_jiff()
            .to_zoned(jiff::tz::TimeZone::system())
            .datetime(),
    );
    let receipt = Uuid::from_fields(0x1234_5678, 0, 0, &[0; 8]);
    let state = State {
        has_retained_transfer: true,
        transcript: Transcript::default(),
        inspection: Some(Inspection {
            background: Phase::Consent,
            from_list: false,
        }),
        catalog_id: Uuid::nil(),
        simulated: true,
        revision: Uuid::nil(),
        review: None,
        phase: Phase::Transfers(vec![
            RetainedTransfer {
                receipt: Uuid::nil(),
                registered_at: time,
                status: ReceiptStatus::Confirmed,
            },
            RetainedTransfer {
                receipt,
                registered_at: time,
                status: ReceiptStatus::RecoveryRequired,
            },
        ]),
    };
    let mut installed = false;
    let mut harness = Harness::builder().with_size([320.0, 600.0]).build_ui_state(
        move |ui, action| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            if let Some(selected) = garmin_ui::firmware::show(ui, &intl, &state) {
                *action = Some(selected);
            }
            assert!(ui.min_rect().right() <= 321.0);
        },
        None,
    );
    harness.run();
    harness.get_by_label("Update confirmed");
    harness.get_by_label("Recovery required");
    harness.get_by_label(&format!("{date} · 12345678")).click();
    harness.run();
    assert_eq!(harness.state(), &Some(Action::InspectReceipt(receipt)));
}

#[test]
fn a_retained_transfer_is_reviewed_before_a_new_selection() {
    let intl = formatter(Language::English);
    let components = firmware_components(PreviewScreen::FirmwareEdge);
    let mut state = State {
        has_retained_transfer: true,
        transcript: Transcript::default(),
        simulated: true,
        catalog_id: Uuid::new_v4(),
        revision: Uuid::new_v4(),
        phase: Phase::Available(components),
        review: None,
        inspection: None,
    };
    let mut installed = false;
    let mut harness = Harness::builder().with_size([760.0, 800.0]).build_ui_state(
        move |ui, actions: &mut Vec<Action>| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            if let Some(action) = garmin_ui::firmware::show(ui, &intl, &state) {
                match &action {
                    Action::Inspect => {
                        state.inspection = Some(Inspection {
                            background: state.phase.clone(),
                            from_list: false,
                        });
                        state.phase = Phase::Reconnected(vec![Observation {
                            part_number: "006-DB585-00".into(),
                            name: "DB, Emoji Category Data".into(),
                            offered: "1.00".into(),
                            status: InstalledStatus::Unchanged,
                        }]);
                    }
                    Action::Acknowledge => {
                        state.has_retained_transfer = false;
                        state.phase = state
                            .inspection
                            .take()
                            .expect("open transfer receipt")
                            .background;
                    }
                    _ => {}
                }
                state.revision = Uuid::new_v4();
                actions.push(action);
            }
        },
        Vec::new(),
    );
    harness.run();
    assert!(harness.query_by_label("Review selection").is_none());
    harness.get_by_label("Previous transfer");
    harness.get_by_label("DB, Emoji Category Data · 1.00");
    harness.get_by_label("Installed version unchanged");
    let explanation = harness.get_by_label_contains("If installation is pending");
    assert!(explanation.rect().height() > 20.0);
    harness.get_by_label_contains("select it again.");
    harness.get_by_label("Dismiss transfer receipt").click();
    harness.run();
    assert!(harness.query_by_label("Previous transfer").is_none());
    harness.get_by_label("Review selection");
    assert_eq!(harness.state(), &[Action::Inspect, Action::Acknowledge]);
}

fn review_harness(language: Language, state: State) -> Harness<'static, (State, Option<Action>)> {
    let intl = formatter(language);
    let mut installed = false;
    Harness::builder()
        .with_size([320.0, 1100.0])
        .build_ui_state(
            move |ui, (snapshot, last_action): &mut (State, Option<Action>)| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                if let Some(action) = garmin_ui::firmware::show(ui, &intl, snapshot) {
                    *last_action = Some(action);
                }
                assert!(ui.min_rect().right() <= ui.ctx().content_rect().right() + 1.0);
            },
            (state, None),
        )
}

#[test]
fn a_changed_version_does_not_prompt_another_installation() {
    let mut harness = review_harness(
        Language::English,
        State {
            phase: Phase::Reconnected(vec![Observation {
                part_number: "006-DB585-00".into(),
                name: "DB, Emoji Category Data".into(),
                offered: "1.00".into(),
                status: InstalledStatus::OtherVersion("100.00".into()),
            }]),
            inspection: Some(Inspection {
                background: Phase::Consent,
                from_list: false,
            }),
            has_retained_transfer: true,
            simulated: true,
            transcript: Transcript::default(),
            catalog_id: Uuid::nil(),
            revision: Uuid::nil(),
            review: None,
        },
    );
    harness.run();
    harness.get_by_label("Device reports 100.00");
    harness.get_by_label_contains("The device reports a different version");
    harness.get_by_label_contains("check for available updates.");
    assert!(harness.query_by_label("Update confirmed").is_none());
    harness.get_by_label("Dismiss transfer receipt").click();
    harness.run();
    assert_eq!(harness.state().1, Some(Action::Acknowledge));
}

#[test]
fn failed_writes_expose_diagnostics_and_offer_the_applicable_next_action() {
    let reason = "Device disconnected while writing Garmin/KBD/Updates/Emojis.db";
    for (retained, label, action) in [
        (false, "Check for updates", Action::Check),
        (true, "Retry", Action::Inspect),
    ] {
        let mut harness = review_harness(
            Language::English,
            State {
                has_retained_transfer: retained,
                transcript: Transcript::default(),
                simulated: false,
                catalog_id: Uuid::nil(),
                revision: Uuid::nil(),
                phase: Phase::WriteFailed(reason.to_owned()),
                review: None,
                inspection: None,
            },
        );
        harness.run();
        harness.get_by_label("Update transfer stopped");
        if retained {
            assert_eq!(harness.state().1.as_ref(), Some(&Action::Inspect));
        }
        assert!(harness.query_by_label(reason).is_none());
        harness.get_by_label("Technical details").click();
        harness.run();
        harness.get_by_label(reason);
        harness.get_by_label(label).click();
        harness.run();
        assert_eq!(harness.state().1.as_ref(), Some(&action));
    }
}

#[test]
fn firmware_controls_show_their_action_across_the_entire_hit_area() {
    use gallery::egui::{CursorIcon, vec2};

    let component = firmware_components(PreviewScreen::FirmwareEdge)
        .into_iter()
        .find(|item| {
            item.download_bytes.is_some()
                && item.staging_issue.is_none()
                && !item.release_notes.is_empty()
        })
        .expect("selectable Edge update with release notes");
    let name = component.name.clone();
    let mut harness = review_harness(
        Language::English,
        State {
            has_retained_transfer: false,
            transcript: Transcript::default(),
            inspection: None,
            catalog_id: Uuid::nil(),
            simulated: true,
            phase: Phase::Available(vec![component]),
            revision: Uuid::nil(),
            review: None,
        },
    );
    for (enabled, cursor) in [
        (true, CursorIcon::PointingHand),
        (false, CursorIcon::NotAllowed),
    ] {
        if !enabled && let Phase::Available(items) = &mut harness.state_mut().0.phase {
            items[0].download_bytes = None;
        }
        harness.run();
        let bounds = harness.get_by_label(&name).rect();
        for point in [
            bounds.left_center() + vec2(8.0, 0.0),
            bounds.right_center() - vec2(4.0, 0.0),
        ] {
            harness.hover_at(point);
            harness.run();
            assert_eq!(harness.output().platform_output.cursor_icon, cursor);
        }
    }
    harness.get_by_label("Release notes").hover();
    harness.run();
    assert_eq!(
        harness.output().platform_output.cursor_icon,
        CursorIcon::PointingHand
    );
}

#[test]
fn choosing_a_component_requires_review_before_write() {
    for (language, review_label, write_label) in [
        (
            Language::English,
            "Review selection",
            "Write selected updates",
        ),
        (
            Language::Czech,
            "Zkontrolovat výběr",
            "Zapsat vybrané aktualizace",
        ),
    ] {
        let components = firmware_components(PreviewScreen::FirmwareWatchSystem);
        let name = components[0].name.clone();
        let bytes = components[0].download_bytes.expect("fixture delivery");
        let state = State {
            has_retained_transfer: false,
            transcript: Transcript::default(),
            inspection: None,
            catalog_id: Uuid::nil(),
            simulated: true,
            phase: Phase::Available(components),
            revision: Uuid::nil(),
            review: None,
        };
        let mut harness = review_harness(language, state);
        harness.run();
        assert!(harness.state().1.is_none());
        assert!(harness.query_by_label(write_label).is_none());
        harness.get_by_label(&name).click();
        harness.run();
        let review_right = harness.get_by_label(review_label).rect().right();
        assert!(review_right >= 288.0, "review should reach the right inset");
        harness.get_by_label(review_label).click();
        harness.run();
        assert_eq!(harness.state().1, Some(Action::Review(vec![0])));
        let approval = Uuid::from_u128(1);
        let model = harness.state_mut();
        model.0.revision = Uuid::from_u128(2);
        model.0.review = Some(Review {
            selected: vec![0],
            approval,
            download_bytes: bytes,
        });
        model.1 = None;
        harness.run();
        for (width, columns) in [(936.0, 4), (320.0, 2)] {
            use egui_kittest::kittest::By;
            use gallery::egui::{accesskit::Role, vec2};
            harness.set_size(vec2(width, 1100.0));
            harness.run();
            let table = harness.get(By::new().role(Role::Table));
            assert_eq!(
                table.query_all(By::new().role(Role::ColumnHeader)).count(),
                columns
            );
            assert_eq!(table.query_all(By::new().role(Role::Row)).count(), 3);
        }
        let write_right = harness.get_by_label(write_label).rect().right();
        assert!((review_right - write_right).abs() <= 1.0);
        harness.get_by_label(write_label).click();
        harness.run();
        assert_eq!(harness.state().1, Some(Action::Stage(approval)));
        let edit = if language == Language::English {
            "Edit selection"
        } else {
            "Upravit výběr"
        };
        harness.get_by_label(edit).click();
        harness.run();
        assert_eq!(harness.state().1, Some(Action::EditSelection));
        harness.state_mut().0.review = None;
        harness.run();
        assert!(harness.query_by_label(write_label).is_none());
        harness.get_by_label(review_label).click();
        harness.run();
        assert_eq!(harness.state().1, Some(Action::Review(vec![0])));
        let recovery = harness.state_mut();
        recovery.0.inspection = Some(Inspection {
            background: recovery.0.phase.clone(),
            from_list: false,
        });
        recovery.0.phase = Phase::ReviewRecovery {
            approval,
            files: vec!["Internal · Garmin/GUPDATE.GSP".into()],
        };
        recovery.1 = None;
        harness.run();
        harness.get_by_label("Internal · Garmin/GUPDATE.GSP");
        let restore = if language == Language::English {
            "Preserve and restore files"
        } else {
            "Uchovat kopie a obnovit soubory"
        };
        harness.get_by_label(restore).click();
        harness.run();
        assert_eq!(harness.state().1, Some(Action::ApproveRecovery(approval)));
    }
}
