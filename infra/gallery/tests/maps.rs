//! Production map controls preserve approval and recovery boundaries in both locales.
use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use garmin_i18n::{Language, Translations};
use garmin_service_api::maps::{Action, CatalogService, Command, Phase, Plan, Recovery, State};

struct Model {
    state: State,
    command: Option<Command>,
}

fn fixture() -> State {
    let id = "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("synthetic UUID");
    State {
        id,
        revision: id,
        device: "fixture".into(),
        service: CatalogService::Simulation,
        phase: Phase::Review,
        components: Vec::new(),
        verified_backup: true,
        dry_run: false,
        actions: vec![Action::Approve, Action::Back],
        storages: Vec::new(),
        storage_error: None,
        plan: Some(Plan {
            approval: id,
            digest: "a".repeat(64),
            removal: false,
            components: vec!["TopoActive Europe".into()],
            download_bytes: 100,
            write_count: 1,
            remove_count: 1,
            remove_bytes: 50,
            paths: vec!["Garmin/Maps/europe.img".into()],
            storage_requirements: Vec::new(),
        }),
        recovery: None,
        progress: Vec::new(),
        active: Vec::new(),
        events: Vec::new(),
        history: Vec::new(),
        error: None,
    }
}

#[test]
fn request_errors_follow_language_changes_and_retain_diagnostics() {
    use garmin_service_api::maps::{Failure, FailureKind};
    let error = garmin_ui::maps::ClientError::Request(Failure {
        kind: FailureKind::StaleRevision,
        message: "host diagnostic: stale revision".into(),
        revision: fixture().revision,
    });
    let translations = Translations::bundled().expect("catalog");
    let mut installed = false;
    let mut harness = Harness::builder().with_size([480.0, 320.0]).build_ui_state(
        move |ui, language: &mut Language| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            error.show(ui, &translations.formatter(*language).expect("locale"));
        },
        Language::English,
    );
    harness.run();
    let _ =
        harness.get_by_label("The map workflow changed. Review the current state and try again.");
    assert!(
        harness
            .query_by_label("host diagnostic: stale revision")
            .is_none()
    );
    *harness.state_mut() = Language::Czech;
    harness.run();
    let _ = harness.get(By::new().predicate(|node| {
        node.value().is_some_and(|text| {
            text.split_whitespace().eq(
                "Stav operace s mapami se změnil. Zkontrolujte aktuální stav a zkuste to znovu."
                    .split_whitespace(),
            )
        })
    }));
    harness.get_by_label("Podrobnosti").click();
    harness.run();
    let _ = harness.get_by_label("host diagnostic: stale revision");
}

#[test]
fn map_disclosures_share_cursor_and_keyboard_expansion() {
    use garmin_service_api::maps::{Outcome, Progress, ProgressStatus};
    for language in [Language::English, Language::Czech] {
        let intl = Translations::bundled()
            .expect("catalog")
            .formatter(language)
            .expect("locale");
        let mut state = fixture();
        state.events.push(Progress {
            stage: "backup".into(),
            label: "Verified original map backup".into(),
            completed: 100,
            total: Some(100),
            bytes: true,
            elapsed_ms: 1_000,
            bytes_per_second: None,
            remaining_seconds: None,
            stalled: false,
            path: None,
            status: ProgressStatus::Completed,
        });
        state.history.push(Outcome {
            id: state.id,
            phase: Phase::Cancelled,
            message: "Original device files restored".into(),
        });
        let outcome_id = format!("maps.outcome.{}", state.id);
        let mut installed = false;
        let mut harness = Harness::builder()
            .with_size([320.0, 1600.0])
            .build_ui(move |ui| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                garmin_ui::maps::show(ui, &intl, &state);
            });
        harness.run();
        for (id, detail) in [
            ("maps.plan.paths", "Garmin/Maps/europe.img"),
            ("maps.operation.history", "Verified original map backup"),
            (outcome_id.as_str(), "Original device files restored"),
        ] {
            let header = harness.get(By::new().predicate(|node| node.author_id() == Some(id)));
            assert_eq!(header.accesskit_node().data().is_expanded(), Some(false));
            header.hover();
            harness.run();
            assert_eq!(
                harness.output().platform_output.cursor_icon,
                gallery::egui::CursorIcon::PointingHand
            );
            harness
                .get(By::new().predicate(|node| node.author_id() == Some(id)))
                .click();
            harness.run();
            assert!(harness.query_by_label(detail).is_some());
            let header = harness.get(By::new().predicate(|node| node.author_id() == Some(id)));
            assert_eq!(header.accesskit_node().data().is_expanded(), Some(true));
            header.focus();
            harness.run();
            harness.key_press(gallery::egui::Key::Space);
            harness.run();
            assert!(harness.query_by_label(detail).is_none());
        }
    }
}

#[test]
fn narrow_catalog_has_unique_named_actions_and_dispatches_the_matching_component() {
    use garmin_service_api::maps::{Choice, Component};
    for language in [Language::English, Language::Czech] {
        let intl = Translations::bundled()
            .expect("catalog")
            .formatter(language)
            .expect("locale");
        let mut state = fixture();
        state.phase = Phase::Catalog;
        state.actions = vec![Action::Choose, Action::Review];
        state.components.push(Component {
            index: 0,
            name: "TopoActive Europe — Central and Western Europe".into(),
            installed: Some("2025.20".into()),
            available: Some("2026.10".into()),
            operation: garmin_model::map::MapOperation::Update,
            can_install: true,
            can_remove: true,
            choice: Choice::Keep,
            download_bytes: 100,
            cached_files: 0,
            total_files: 1,
        });
        let mut second = state.components[0].clone();
        second.index = 1;
        second.name = "Worldwide routable basemap".into();
        state.components.push(second);
        let mut installed = false;
        let mut harness = Harness::builder()
            .with_size([320.0, 1200.0])
            .build_ui_state(
                move |ui, model: &mut Model| {
                    if !installed {
                        garmin_ui::install(ui.ctx());
                        installed = true;
                        ui.ctx().request_repaint();
                        return;
                    }
                    if let Some(command) = garmin_ui::maps::show(ui, &intl, &model.state) {
                        model.command = Some(command);
                    }
                    assert!(
                        ui.min_rect().right() <= 321.0,
                        "catalog overflow in {language:?}"
                    );
                },
                Model {
                    state,
                    command: None,
                },
            );
        harness.run();
        for (component, action, choice) in [
            (0, "remove", Choice::Remove),
            (1, "install", Choice::Install),
        ] {
            let target = format!("maps.component.{component}.{action}");
            let node =
                harness.get(By::new().predicate(|node| node.author_id() == Some(target.as_str())));
            assert!(
                node.accesskit_node()
                    .label()
                    .is_some_and(|label| !label.is_empty()),
                "missing action label: {target}"
            );
            node.click();
            harness.run();
            assert_eq!(
                harness.state().command,
                Some(Command::Choose { component, choice })
            );
        }
    }
}

#[test]
fn approval_cancel_and_recovery_dispatch_only_the_displayed_choice() {
    for language in [Language::English, Language::Czech] {
        for width in [320.0, 720.0] {
            let intl = Translations::bundled()
                .expect("catalog")
                .formatter(language)
                .expect("locale");
            let mut installed = false;
            let mut harness = Harness::builder()
                .with_size([width, 1200.0])
                .build_ui_state(
                    move |ui, model: &mut Model| {
                        if !installed {
                            garmin_ui::install(ui.ctx());
                            installed = true;
                            ui.ctx().request_repaint();
                            return;
                        }
                        if let Some(command) = garmin_ui::maps::show(ui, &intl, &model.state) {
                            model.command = Some(command);
                        }
                        assert!(
                            ui.min_rect().right() <= width + 1.0,
                            "map view overflow: {:?}, {language:?}, {width}, {:?}",
                            model.state.phase,
                            ui.min_rect()
                        );
                    },
                    Model {
                        state: fixture(),
                        command: None,
                    },
                );
            harness.run();
            assert!(harness.state().command.is_none());
            harness
                .get(By::new().predicate(|node| node.author_id() == Some("maps.Approve")))
                .click();
            harness.run();
            assert_eq!(
                harness.state().command,
                Some(Command::Approve {
                    approval: harness.state().state.plan.as_ref().expect("plan").approval,
                })
            );
            harness.state_mut().state.phase = Phase::Running;
            harness.state_mut().state.actions = vec![Action::Cancel];
            harness.state_mut().command = None;
            harness.run_steps(3);
            assert!(
                harness
                    .query(By::new().predicate(|node| node.author_id() == Some("maps.Approve")))
                    .is_none()
            );
            harness
                .get(By::new().predicate(|node| node.author_id() == Some("maps.Cancel")))
                .click();
            harness.run_steps(3);
            assert_eq!(harness.state().command, Some(Command::Cancel));
            for (action, command) in [
                (Action::Recover, Command::Recover),
                (Action::ClearRecovery, Command::ClearRecovery),
                (Action::DiscardPreparation, Command::DiscardPreparation),
            ] {
                let model = harness.state_mut();
                model.state.phase = Phase::Recovery;
                model.state.actions = vec![action];
                model.state.recovery = Some(Recovery {
                    plan: "a".repeat(64),
                    removal: false,
                    simulated: false,
                    actions: vec![action],
                });
                model.command = None;
                harness.run();
                let id = format!("maps.{action:?}");
                harness
                    .get(By::new().predicate(|node| node.author_id() == Some(id.as_str())))
                    .click();
                harness.run();
                assert_eq!(harness.state().command, Some(command));
            }
        }
    }
}
