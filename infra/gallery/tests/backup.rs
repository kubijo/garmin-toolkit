//! Pointer automation for the production backup controls and irreversible boundary.
use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use garmin_i18n::{Language, Translations};
use garmin_service_api::snapshots::{SnapshotPreview, SnapshotState, SnapshotStatus};
use garmin_ui::backup::{self, Action, State};

struct Model {
    server_files: bool,
    state: State,
    file: Option<backup::SelectedFile>,
    action: Option<Action>,
}

fn verified() -> SnapshotStatus {
    let id = "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("fixture UUID");
    SnapshotStatus {
        operation: id,
        epoch: id,
        state: SnapshotState::AwaitingApproval,
        transferred: 100,
        total: Some(100),
        error: None,
        preview: Some(SnapshotPreview {
            format_version: 1,
            app_version: "0.1.0".into(),
            created_at: 1_790_640_000,
            database_bytes: 8_000_000,
            database_sha256: "a".repeat(64),
            approval: id,
        }),
    }
}

#[test]
fn backup_status_keeps_the_profile_chooser_centered() {
    for width in [360.0, 960.0, 1808.0] {
        let mut idle_row_top: Option<f32> = None;
        for state in [
            State::Idle,
            State::Restored(backup::RestoreSummary::default()),
        ] {
            let intl = Translations::bundled()
                .expect("catalog")
                .formatter(Language::English)
                .expect("English");
            let mut installed = false;
            let mut view = Harness::builder().with_size([width, 720.0]).build_ui(|ui| {
                if !installed {
                    garmin_ui::install(ui.ctx());
                    installed = true;
                    ui.ctx().request_repaint();
                    return;
                }
                let _ = backup::show_operation(
                    ui,
                    &backup::Props {
                        intl: &intl,
                        state: &state,
                        file: None,
                        enabled: false,
                        server_files: true,
                    },
                );
                let _ = garmin_ui::profile::chooser(
                    ui,
                    &garmin_ui::profile::ChooserProps {
                        intl: &intl,
                        profiles: &[garmin_ui::profile::ProfileProps {
                            display_name: "Alex Rider",
                            accent: garmin_color::swatch::cyan::G40,
                            avatar: None,
                        }],
                        owner_index: Some(0),
                    },
                );
            });
            view.run();
            let row = view
                .get(By::new().predicate(|node| node.author_id() == Some("profile.0")))
                .rect();
            assert!((row.center().x - width / 2.0).abs() <= 1.0);
            assert!(row.left() >= 0.0 && row.right() <= width);
            if let Some(idle_top) = idle_row_top {
                assert!((row.top() - idle_top).abs() <= 1.0);
            } else {
                idle_row_top = Some(row.top());
            }
        }
    }
}

#[test]
fn restored_backup_is_acknowledged_in_a_one_action_modal() {
    let intl = Translations::bundled()
        .expect("catalog")
        .formatter(Language::English)
        .expect("English");
    let file = backup::SelectedFile::RestoreSource("garmin-backup.tar.zst".into());
    let mut installed = false;
    let mut view = Harness::builder().with_size([960.0, 720.0]).build_ui_state(
        move |ui, action: &mut Option<Action>| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            if let Some(chosen) = backup::show_operation(
                ui,
                &backup::Props {
                    intl: &intl,
                    state: &State::Restored(backup::RestoreSummary {
                        elapsed: Some(std::time::Duration::from_secs(18)),
                        archive_bytes: Some(1_200_000),
                        database_bytes: Some(8_000_000),
                        created_at: Some(1_790_640_000),
                    }),
                    file: Some(&file),
                    enabled: false,
                    server_files: true,
                },
            ) {
                *action = Some(chosen);
            }
        },
        None,
    );
    view.run();
    let _ = view.get_by_label("Backup restored");
    let _ = view.get_by_label("garmin-backup.tar.zst");
    let _ = view.get_by_label("Restore time");
    let _ = view.get_by_label("Backup size");
    let _ = view.get_by_label("Database size");
    assert!(view.query_by_label("Clear selection").is_none());
    assert!(view.query_by_label("Cancel").is_none());
    view.get(By::new().predicate(|node| node.author_id() == Some("backup.restore.acknowledge")))
        .click();
    view.run_steps(3);
    assert_eq!(*view.state(), Some(Action::Clear));
}

#[test]
fn backup_controls_require_confirmation_and_stop_cancellation_at_switch() {
    for language in [Language::English, Language::Czech] {
        for width in [288.0, 360.0, 720.0] {
            let mut harness = harness(language, width);
            harness.run();
            harness
                .get(By::new().predicate(|node| node.author_id() == Some("backup.save")))
                .hover();
            harness.run();
            assert_eq!(
                harness.output().platform_output.cursor_icon,
                gallery::eframe::egui::CursorIcon::PointingHand
            );
            harness
                .get(By::new().predicate(|node| node.author_id() == Some("backup.save")))
                .click();
            harness.run();
            assert_eq!(harness.state().action, Some(Action::Backup));
            let name = "garmin-backup-with-a-long-descriptive-filename-2026-09-29.tar.zst";
            harness.state_mut().file = Some(backup::SelectedFile::SaveDestination(name.into()));
            harness.state_mut().state = State::Starting;
            harness.run_steps(3);
            assert_absent(
                &harness,
                &[
                    "backup.save",
                    "backup.open",
                    "backup.approve",
                    "backup.clear",
                ],
            );
            harness
                .get(By::new().predicate(|node| node.author_id() == Some("backup.cancel")))
                .click();
            harness.run_steps(3);
            assert_eq!(harness.state().action, Some(Action::Cancel));
            harness.state_mut().state = State::Saved;
            harness.run();
            let _ = harness.get_by_label(if language == Language::Czech {
                "Cíl uložení"
            } else {
                "Save destination"
            });
            harness
                .get(By::new().predicate(|node| node.author_id() == Some("backup.clear")))
                .click();
            harness.run();
            assert_eq!(harness.state().action, Some(Action::Clear));
            harness.state_mut().file = None;
            harness.state_mut().state = State::Idle;
            harness.run();
            for id in ["backup.save", "backup.open"] {
                let _ = harness.get(By::new().predicate(|node| node.author_id() == Some(id)));
            }
            harness.state_mut().file = Some(backup::SelectedFile::RestoreSource(name.into()));
            harness.state_mut().state = State::Running(verified());
            harness.state_mut().action = None;
            harness.run();
            let _ = harness.get_by_label(if language == Language::Czech {
                "Zdroj obnovy"
            } else {
                "Restore source"
            });
            assert!(harness.state().action.is_none());
            assert_absent(&harness, &["backup.save", "backup.open", "backup.clear"]);
            let confirm = harness
                .get(By::new().predicate(|node| node.author_id() == Some("backup.approve")))
                .rect();
            assert!(
                confirm.bottom() < 900.0,
                "confirmation outside viewport at {width}: {confirm:?}"
            );
            harness
                .get(By::new().predicate(|node| node.author_id() == Some("backup.approve")))
                .click();
            harness.run();
            assert_eq!(harness.state().action, Some(Action::Approve));
            let mut switching = verified();
            switching.state = SnapshotState::Restoring;
            switching.preview = None;
            harness.state_mut().state = State::Running(switching);
            harness.state_mut().action = None;
            harness.run_steps(3);
            assert_absent(
                &harness,
                &[
                    "backup.save",
                    "backup.open",
                    "backup.cancel",
                    "backup.approve",
                    "backup.clear",
                ],
            );
        }
    }
}

fn harness(language: Language, width: f32) -> Harness<'static, Model> {
    let intl = Translations::bundled()
        .expect("catalog")
        .formatter(language)
        .expect("formatter");
    let mut installed = false;
    Harness::builder().with_size([width, 900.0]).build_ui_state(
        move |ui, model: &mut Model| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            if let Some(action) = backup::show(
                ui,
                &backup::Props {
                    server_files: model.server_files,
                    intl: &intl,
                    state: &model.state,
                    file: model.file.as_ref(),
                    enabled: true,
                },
            ) {
                model.action = Some(action);
            }
            assert!(ui.min_rect().right() <= width + 1.0, "backup view overflow");
        },
        Model {
            server_files: false,
            state: State::Idle,
            file: None,
            action: None,
        },
    )
}

fn assert_absent(harness: &Harness<'_, Model>, ids: &[&str]) {
    for id in ids {
        assert!(
            harness
                .query(By::new().predicate(|node| node.author_id() == Some(*id)))
                .is_none(),
            "unexpected control: {id}"
        );
    }
}

#[test]
fn both_locations_dispatch_distinct_actions() {
    for language in [Language::English, Language::Czech] {
        for width in [288.0, 360.0, 720.0] {
            for (id, action) in [
                ("backup.save", Action::Backup),
                ("backup.open", Action::Restore),
                ("backup.server-save", Action::ServerBackup),
                ("backup.server-open", Action::ServerRestore),
            ] {
                let mut harness = harness(language, width);
                harness.state_mut().server_files = true;
                harness.run();
                harness
                    .get(By::new().predicate(|node| node.author_id() == Some(id)))
                    .click();
                harness.run();
                assert_eq!(harness.state().action, Some(action));
            }
        }
    }
}

#[test]
fn recovery_controls_protect_active_work_and_require_review() {
    use backup::recovery::{self, Action as RecoveryAction};
    use garmin_service_api::snapshots::{SnapshotOperation, SnapshotSource};

    for language in [Language::English, Language::Czech] {
        for width in [288.0, 720.0] {
            let intl = Translations::bundled()
                .expect("catalog")
                .formatter(language)
                .expect("formatter");
            let operation = SnapshotOperation {
                status: verified(),
                source: SnapshotSource::ServerRestore(
                    "/share/backups/garmin-backup-with-a-long-name-2026-09-29.tar.zst".into(),
                ),
                active: false,
            };
            let resume = format!("backup.recovery.resume.{}", operation.status.operation);
            let discard = format!("backup.recovery.discard.{}", operation.status.operation);
            let mut installed = false;
            let mut view = Harness::builder().with_size([width, 900.0]).build_ui_state(
                move |ui, (operation, action): &mut (SnapshotOperation, Option<RecoveryAction>)| {
                    if !installed {
                        garmin_ui::install(ui.ctx());
                        installed = true;
                        ui.ctx().request_repaint();
                        return;
                    }
                    if let Some(next) = recovery::show(
                        ui,
                        &recovery::Props {
                            intl: &intl,
                            operations: std::slice::from_ref(operation),
                            loading: false,
                            error: None,
                        },
                    ) {
                        *action = Some(next);
                    }
                    assert!(
                        ui.min_rect().right() <= width + 1.0,
                        "recovery view overflow"
                    );
                },
                (operation, None),
            );
            view.run();
            view.get(By::new().predicate(|node| node.author_id() == Some(&resume)))
                .click();
            view.run();
            assert_eq!(view.state().1, Some(RecoveryAction::Resume(0)));
            assert!(
                view.query(By::new().predicate(|node| node.author_id() == Some("backup.approve")))
                    .is_none()
            );
            view.state_mut().0.active = true;
            view.state_mut().1 = None;
            view.run();
            for id in [&resume, &discard] {
                assert!(
                    view.get(By::new().predicate(|node| node.author_id() == Some(id)))
                        .accesskit_node()
                        .is_disabled()
                );
            }
            view.state_mut().0.active = false;
            view.state_mut().0.status.state = SnapshotState::Restoring;
            view.run();
            assert!(
                view.get(By::new().predicate(|node| node.author_id() == Some(&discard)))
                    .accesskit_node()
                    .is_disabled()
            );
            view.state_mut().0.status.state = SnapshotState::Uploading;
            view.state_mut().0.source = SnapshotSource::Upload;
            view.run();
            assert!(
                view.query(By::new().predicate(|node| node.author_id() == Some(&resume)))
                    .is_none()
            );
            view.get(By::new().predicate(|node| node.author_id() == Some(&discard)))
                .click();
            view.run();
            assert_eq!(view.state().1, Some(RecoveryAction::Discard(0)));
        }
    }
}
