use crate::SceneStateKey as _;
use gallery::prelude::*;

scene_meta! { title: "Components / File chooser" }

thread_local! {
    static CHOOSER: crate::SceneState<(garmin_ui::device_browser::chooser::Chooser, usize, bool), 2> = const { crate::SceneState::empty() };
}

#[scene("Save destination", default)]
fn save_destination(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show(ctx, ui, globals, 0);
}

#[scene("Open file")]
fn open_file(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show(ctx, ui, globals, 1);
}

fn show(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals, initial_mode: usize) {
    let mode = ctx.buttons("mode", &["save", "open", "loading", "error"], initial_mode);
    let width = ctx.slider("width", 880.0, 320.0, 1000.0, 1.0);
    let show_hidden = ctx.toggle("hidden files", false);
    let platform_window = ctx.toggle("platform window", false);
    let edit_path = ctx.toggle("edit path", false);
    let intl = globals.intl();
    stage!(ctx, ui, globals.stage((width, 600.0)), |ui| {
        CHOOSER.with_scene(
            usize::from(edit_path),
            || {
                let mut view = chooser(mode);
                view.set_show_hidden_files(show_hidden);
                if edit_path {
                    view.edit_location();
                }
                (view, mode, show_hidden)
            },
            |(view, previous, previous_hidden)| {
                if *previous != mode {
                    *view = chooser(mode);
                    *previous = mode;
                    view.set_show_hidden_files(show_hidden);
                }
                if *previous_hidden != show_hidden {
                    view.set_show_hidden_files(show_hidden);
                    *previous_hidden = show_hidden;
                }
                let action = if platform_window {
                    view.contents(ui, &intl, mode == 2)
                } else {
                    view.show(ui, &intl, mode == 2)
                };
                if let Some(garmin_ui::device_browser::chooser::Action::Navigate(_)) = action {
                    view.loaded(directory());
                }
            },
        );
    });
}

fn chooser(mode: usize) -> garmin_ui::device_browser::chooser::Chooser {
    use garmin_service_api::files::Operation;
    let operation = if mode == 1 {
        Operation::Open
    } else {
        Operation::Save
    };
    let mut view = garmin_ui::device_browser::chooser::Chooser::new(
        operation,
        "garmin-backup-2026-09-29.tar.zst".into(),
    );
    if mode != 2 {
        view.loaded(directory());
    }
    if mode == 3 {
        view.failed("Permission denied".into());
    }
    view
}

fn directory() -> garmin_service_api::files::Directory {
    use garmin_service_api::{DeviceCatalogEntry, DeviceCatalogEntryKind};
    garmin_service_api::files::Directory {
        path: "/share/backups".into(),
        entries: vec![
            DeviceCatalogEntry {
                path: ".archive".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: ".previous.tar.zst".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(40_000_000),
            },
            DeviceCatalogEntry {
                path: "Archive".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "garmin-backup-2026-09-28.tar.zst".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(80_000_000),
            },
            DeviceCatalogEntry {
                path: "garmin-backup-2026-09-29.tar.zst".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(84_000_000),
            },
        ],
    }
}
