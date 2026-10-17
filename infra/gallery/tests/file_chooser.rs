//! Open/save interactions exercise the same table and navigation as the device explorer.

use egui_kittest::{
    Harness,
    kittest::{By, NodeT as _, Queryable as _},
};
use garmin_i18n::{Language, Translations};
use garmin_service_api::{
    DeviceCatalogEntry, DeviceCatalogEntryKind,
    files::{Directory, Operation},
};
use garmin_ui::device_browser::chooser::{Action, Chooser};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presentation {
    Embedded,
    AfterApplication,
    Platform,
}

struct Model {
    chooser: Chooser,
    action: Option<Action>,
    busy: bool,
    presentation: Presentation,
    enabled: bool,
}

fn harness(operation: Operation, name: &str) -> Harness<'static, Model> {
    let intl = Translations::bundled()
        .expect("catalog")
        .formatter(Language::English)
        .expect("English");
    let mut chooser = Chooser::new(operation, name.into());
    chooser.loaded(Directory {
        path: "/share".into(),
        entries: vec![
            DeviceCatalogEntry {
                path: ".cache".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: ".saved.tar.zst".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(800_000),
            },
            DeviceCatalogEntry {
                path: "Archive".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "saved.tar.zst".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(800_000),
            },
        ],
    });
    let mut installed = false;
    Harness::builder().with_size([960.0, 720.0]).build_ui_state(
        move |ui, model: &mut Model| {
            if !installed {
                garmin_ui::install(ui.ctx());
                installed = true;
                ui.ctx().request_repaint();
                return;
            }
            let action = if model.presentation == Presentation::Platform {
                ui.add_enabled_ui(model.enabled, |ui| {
                    model.chooser.contents(ui, &intl, model.busy)
                })
                .inner
            } else if model.presentation == Presentation::AfterApplication {
                let bounds = ui.max_rect();
                ui.allocate_space(ui.available_size());
                ui.scope_builder(gallery::egui::UiBuilder::new().max_rect(bounds), |ui| {
                    if !model.enabled {
                        ui.disable();
                    }
                    model.chooser.show(ui, &intl, model.busy)
                })
                .inner
            } else {
                model.chooser.show(ui, &intl, model.busy)
            };
            if let Some(action) = action {
                model.action = Some(action);
            }
        },
        Model {
            chooser,
            action: None,
            busy: false,
            presentation: Presentation::Embedded,
            enabled: true,
        },
    )
}

#[test]
fn platform_window_contents_select_and_confirm_overwrite_without_an_inner_window() {
    for operation in [Operation::Open, Operation::Save] {
        let mut view = harness(operation, "saved.tar.zst");
        view.state_mut().presentation = Presentation::Platform;
        view.run();
        assert!(
            view.query(By::new().predicate(|node| node.author_id() == Some("shell-window-close")))
                .is_none()
        );
        view.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.confirm")))
            .click();
        view.run();
        if operation == Operation::Save {
            assert!(view.state().action.is_none());
            view.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.replace")))
                .click();
            view.run();
        }
        assert!(
            matches!(&view.state().action, Some(Action::Select { path, replace }) if path == "/share/saved.tar.zst" && *replace == (operation == Operation::Save))
        );
        view.state_mut().action = None;
        view.state_mut().busy = true;
        view.run_steps(3);
        view.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.cancel")))
            .click();
        view.run_steps(3);
        assert!(matches!(view.state().action, Some(Action::Cancel)));
    }
}

#[test]
fn chooser_after_full_page_remains_visible_and_respects_disabled_host() {
    let mut view = harness(Operation::Save, "new.tar.zst");
    view.state_mut().presentation = Presentation::AfterApplication;
    view.run();
    let confirm = || By::new().predicate(|node| node.author_id() == Some("files.chooser.confirm"));
    let button = view.get(confirm()).rect();
    assert!(
        gallery::egui::Rect::from_min_size(
            gallery::egui::Pos2::ZERO,
            gallery::egui::vec2(960.0, 720.0),
        )
        .contains_rect(button)
    );
    assert!(button.width() > 40.0 && button.height() >= 40.0);
    view.state_mut().enabled = false;
    view.run();
    assert!(view.get(confirm()).accesskit_node().is_disabled());
    view.state_mut().enabled = true;
    view.run();
    view.get(confirm()).click();
    view.run();
    assert!(
        matches!(&view.state().action, Some(Action::Select { path, .. }) if path == "/share/new.tar.zst")
    );
}

#[test]
fn loading_backdrop_leaves_cancel_accessible() {
    let mut view = harness(Operation::Save, "new.tar.zst");
    view.state_mut().busy = true;
    view.run_steps(3);
    let _ = view.get_by_label("Loading files…");
    view.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.cancel")))
        .click();
    view.run_steps(3);
    assert!(matches!(view.state().action, Some(Action::Cancel)));
}

#[test]
fn hidden_files_toggle_filters_both_tree_and_list() {
    let mut view = harness(Operation::Open, "");
    view.run();
    let hidden_file =
        || By::new().predicate(|node| node.author_id() == Some("files.entry.share/.saved.tar.zst"));
    let _ = view.get_by_label("Show hidden files");
    assert!(view.query(hidden_file()).is_none());
    assert!(view.query_by_label(".cache").is_none());
    view.get(By::new().predicate(|node| node.author_id() == Some("files.hidden")))
        .click();
    view.run();
    assert!(matches!(
        view.state().action,
        Some(Action::ShowHiddenFiles(true))
    ));
    assert!(view.query(hidden_file()).is_some());
    let _ = view.get_by_label("Hide hidden files");
    assert!(
        view.query(
            By::new().predicate(|node| node.author_id() == Some("files.entry.share/.cache"))
        )
        .is_some()
    );
    view.get(By::new().predicate(|node| node.author_id() == Some("files.hidden")))
        .click();
    view.run();
    assert!(matches!(
        view.state().action,
        Some(Action::ShowHiddenFiles(false))
    ));
    assert!(view.query(hidden_file()).is_none());
    assert!(view.query_by_label(".cache").is_none());
    // A host reapplies the selected profile's persisted preference on reopen/switch.
    view.state_mut().chooser.set_show_hidden_files(true);
    view.run();
    assert!(view.query(hidden_file()).is_some());
    view.state_mut().chooser.set_show_hidden_files(false);
    view.run();
    assert!(view.query(hidden_file()).is_none());
}

#[test]
fn save_requires_explicit_overwrite_and_open_only_selects() {
    let mut save = harness(Operation::Save, "saved.tar.zst");
    save.run();
    save.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.confirm")))
        .click();
    save.run();
    assert!(save.state().action.is_none());
    save.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.replace")))
        .click();
    save.run();
    assert!(
        matches!(&save.state().action, Some(Action::Select { path, replace: true }) if path == "/share/saved.tar.zst")
    );

    let mut open = harness(Operation::Open, "");
    open.run();
    open.get(
        By::new().predicate(|node| node.author_id() == Some("files.entry.share/saved.tar.zst")),
    )
    .click();
    open.run();
    open.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.confirm")))
        .click();
    open.run();
    assert!(
        matches!(&open.state().action, Some(Action::Select { path, replace: false }) if path == "/share/saved.tar.zst")
    );
}

#[test]
fn location_bar_accepts_unvisited_paths_and_escape_keeps_the_current_folder() {
    let mut view = harness(Operation::Open, "");
    view.run();
    let edit = || By::new().predicate(|node| node.author_id() == Some("files.path.edit"));
    let input = || By::new().predicate(|node| node.author_id() == Some("files.path.input"));
    view.get(edit()).click();
    view.run();
    view.get(input()).type_text("/home/person/New folder");
    view.run();
    view.key_press(gallery::egui::Key::Enter);
    view.run();
    assert!(
        matches!(&view.state().action, Some(Action::Navigate(path)) if path == "/home/person/New folder")
    );
    view.state_mut().action = None;
    view.key_press_modifiers(gallery::egui::Modifiers::COMMAND, gallery::egui::Key::L);
    view.run();
    view.get(input()).type_text("/discarded");
    view.run();
    view.key_press(gallery::egui::Key::Escape);
    view.run();
    assert!(view.query(input()).is_none());
    assert!(view.state().action.is_none());
    view.get(By::new().predicate(|node| node.author_id() == Some("files.back")))
        .click();
    view.run();
    assert!(matches!(&view.state().action, Some(Action::Navigate(path)) if path == "/share"));
}

#[test]
fn changing_window_presentation_preserves_folder_selection_and_save_name() {
    let mut view = harness(Operation::Save, "keep-this-name.tar.zst");
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("files.entry.share/Archive")))
        .click();
    view.run();
    let saved = view.state().chooser.view_state();
    let mut chooser = Chooser::new(Operation::Save, "wrong.tar.zst".into());
    chooser.loaded(Directory {
        path: "/share".into(),
        entries: vec![DeviceCatalogEntry {
            path: "Archive".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        }],
    });
    chooser.restore_view(saved);
    view.state_mut().chooser = chooser;
    view.state_mut().presentation = Presentation::Platform;
    view.run();
    assert_eq!(
        view.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.name")))
            .value()
            .as_deref(),
        Some("keep-this-name.tar.zst")
    );
    view.key_press(gallery::egui::Key::Enter);
    view.run();
    assert!(
        matches!(&view.state().action, Some(Action::Navigate(path)) if path == "/share/Archive")
    );
}

#[test]
fn directory_navigation_uses_entry_targets() {
    let mut view = harness(Operation::Save, "new.tar.zst");
    view.run();
    view.get(By::new().predicate(|node| node.author_id() == Some("files.entry.share/Archive")))
        .click();
    view.run();
    view.key_press(gallery::egui::Key::Enter);
    view.run();
    assert!(
        matches!(&view.state().action, Some(Action::Navigate(path)) if path == "/share/Archive")
    );
}

#[test]
fn new_file_save_and_cancel_do_not_request_overwrite() {
    let mut save = harness(Operation::Save, "");
    save.run();
    save.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.name")))
        .click();
    save.run();
    save.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.name")))
        .type_text("new.tar.zst");
    save.run();
    save.get(By::new().predicate(|node| node.author_id() == Some("files.chooser.confirm")))
        .click();
    save.run();
    assert!(matches!(
        &save.state().action,
        Some(Action::Select { replace: false, .. })
    ));
    let mut cancelled = harness(Operation::Open, "saved.tar.zst");
    cancelled.run();
    cancelled
        .get(By::new().predicate(|node| node.author_id() == Some("files.chooser.cancel")))
        .click();
    cancelled.run();
    assert!(matches!(cancelled.state().action, Some(Action::Cancel)));
}

#[test]
fn resize_edge_matches_the_visible_frame() {
    let mut view = harness(Operation::Save, "new.tar.zst");
    view.run();
    let window = view
        .get(By::new().predicate(|node| node.role() == gallery::egui::accesskit::Role::Window))
        .rect();
    let input = view
        .get(By::new().predicate(|node| node.author_id() == Some("files.chooser.name")))
        .rect();
    // One frame pixel and the footer's 8 px padding, without a second shadow inset.
    assert!((input.left() - window.left() - 9.0).abs() < 1.0);
    view.hover_at(window.center_bottom() - gallery::egui::vec2(0.0, 1.0));
    view.run();
    assert_eq!(
        view.output().platform_output.cursor_icon,
        gallery::egui::CursorIcon::ResizeVertical
    );
    let edge = view
        .output()
        .shapes
        .iter()
        .find_map(|shape| {
            if let gallery::egui::epaint::Shape::Path(path) = &shape.shape
                && path.points.len() == 2
                && path
                    .points
                    .iter()
                    .all(|point| (point.y - window.bottom()).abs() < 2.0)
            {
                Some(path)
            } else {
                None
            }
        })
        .expect("hovered bottom resize edge");
    assert!(
        edge.points
            .iter()
            .all(|point| { point.x >= input.left() - 9.0 && point.x <= window.right() })
    );
}

#[test]
#[ignore = "renders chooser resize hover states with a headless GPU"]
fn capture_resize_edges() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/gallery/file-chooser-resize");
    std::fs::create_dir_all(&out)?;
    for (name, theme) in [
        ("dark", gallery::egui::ThemePreference::Dark),
        ("light", gallery::egui::ThemePreference::Light),
    ] {
        let mut view = harness(Operation::Save, "new.tar.zst");
        view.state_mut().presentation = Presentation::AfterApplication;
        view.run();
        view.ctx.set_theme(theme);
        view.run();
        let window = view
            .get(By::new().predicate(|node| node.role() == gallery::egui::accesskit::Role::Window))
            .rect();
        for (edge, point) in [
            ("bottom", window.center_bottom()),
            ("left-bottom", window.left_bottom()),
            ("right-bottom", window.right_bottom()),
        ] {
            view.hover_at(point);
            view.run();
            view.render()?
                .save(out.join(format!("{name}-{edge}.png")))?;
        }
    }
    Ok(())
}
