use gallery::prelude::*;
use garmin_color::swatch;
use garmin_model::device::{DeviceStorageState, StorageCapacity};
use garmin_service_api::{
    DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogSnapshot, DeviceCatalogStorage,
    DeviceSnapshot, InspectionState,
};
use garmin_ui::{device, device_browser, profile, shell, workspace};
use std::cell::RefCell;

scene_meta! { title: "Application / Devices / File browser" }

thread_local! {
    static BROWSER: RefCell<Option<device_browser::Browser>> = const { RefCell::new(None) };
    static ROOT_BROWSER: RefCell<Option<device_browser::Browser>> = const { RefCell::new(None) };
    static EMPTY_BROWSER: RefCell<Option<device_browser::Browser>> = const { RefCell::new(None) };
    static NARROW_BROWSER: RefCell<Option<device_browser::Browser>> = const { RefCell::new(None) };
    static GENERIC_FILE_BROWSER: RefCell<Option<(device_browser::Browser, u8)>> = const { RefCell::new(None) };
    static FIT_FILE_BROWSER: RefCell<Option<(device_browser::Browser, u8)>> = const { RefCell::new(None) };
    static CONTEXT_BROWSER: RefCell<Option<(device_browser::Browser, bool)>> = const { RefCell::new(None) };
    static CONTEXT_BROWSER_LIGHT: RefCell<Option<(device_browser::Browser, bool)>> = const { RefCell::new(None) };
    static WINDOW_BROWSER: RefCell<Option<device_browser::Browser>> = const { RefCell::new(None) };
    static NARROW_WINDOW_BROWSER: RefCell<Option<device_browser::Browser>> = const { RefCell::new(None) };
}

const INITIAL_DIRECTORY: &str = "Garmin/Activity/History";

#[scene(default)]
fn populated(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(egui::vec2(960.0, 640.0)).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            BROWSER.with_borrow_mut(|browser| {
                let browser = browser.get_or_insert_with(|| {
                    device_browser::Browser::open_directory(
                        catalog(),
                        &intl,
                        "Mock Watch-o-Matic 9000",
                        "mock-internal",
                        INITIAL_DIRECTORY,
                    )
                    .expect("the gallery catalog and initial directory are valid")
                });
                let _ = browser.show(ui, &intl);
            });
        },
    );
}

#[scene]
fn windowed(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_window_scene(
        ctx,
        ui,
        globals,
        egui::vec2(1_280.0, 720.0),
        &WINDOW_BROWSER,
    );
}

#[scene]
fn compact_window(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_window_scene(
        ctx,
        ui,
        globals,
        egui::vec2(640.0, 640.0),
        &NARROW_WINDOW_BROWSER,
    );
}

fn show_window_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    size: egui::Vec2,
    state: &'static std::thread::LocalKey<RefCell<Option<device_browser::Browser>>>,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(size).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            let profiles = [profile::ProfileProps {
                display_name: "Alex Rider",
                accent: swatch::cyan::G40,
                avatar: None,
            }];
            let devices = [DeviceSnapshot {
                key: "mock:watch-o-matic-9000".to_owned(),
                name: "Mock Watch-o-Matic 9000".to_owned(),
                identifier: Some(36_264_719),
                software_version: Some(3_220),
                inspection: InspectionState::Ready,
                inspection_error: None,
                capabilities: Vec::new(),
                storages: vec![DeviceStorageState {
                    id: "mock-internal".to_owned(),
                    label: "Internal storage".to_owned(),
                    capacity: StorageCapacity::new(32_000_000_000, 8_600_000_000),
                    writable: Some(true),
                }],
            }];
            let page = workspace::Page::Device(devices[0].key.clone());
            let _ = workspace::show(
                ui,
                &workspace::Props {
                    product_name: "Garmin Toolkit Demo",
                    intl: &intl,
                    profiles: &profiles,
                    selected_profile: 0,
                    profile_menu_expanded: false,
                    page: &page,
                    navigation: shell::Navigation::Expanded,
                    devices: &devices,
                    window_controls: None,
                },
                |ui| {
                    let _ = device::show_snapshot(ui, &intl, &devices[0], false);
                },
            );
            state.with_borrow_mut(|browser| {
                let browser = browser.get_or_insert_with(|| {
                    device_browser::Browser::open_directory(
                        catalog(),
                        &intl,
                        &devices[0].name,
                        "mock-internal",
                        INITIAL_DIRECTORY,
                    )
                    .expect("the gallery catalog and initial directory are valid")
                });
                let _ = browser.show_window(ui, &intl);
            });
        },
    );
}

#[scene]
fn storage_root(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_directory_scene(
        ctx,
        ui,
        globals,
        egui::vec2(960.0, 640.0),
        "",
        catalog,
        &ROOT_BROWSER,
    );
}

#[scene]
fn selected_generic_file(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_selected_scene(ctx, ui, globals, "Garmin", 4, &GENERIC_FILE_BROWSER);
}

#[scene]
fn selected_fit_file(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_selected_scene(ctx, ui, globals, INITIAL_DIRECTORY, 2, &FIT_FILE_BROWSER);
}

#[scene]
fn empty_storage(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_directory_scene(
        ctx,
        ui,
        globals,
        egui::vec2(960.0, 640.0),
        "",
        empty_catalog,
        &EMPTY_BROWSER,
    );
}

#[scene]
fn narrow(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_directory_scene(
        ctx,
        ui,
        globals,
        egui::vec2(640.0, 640.0),
        INITIAL_DIRECTORY,
        catalog,
        &NARROW_BROWSER,
    );
}

fn show_directory_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    size: egui::Vec2,
    directory: &str,
    fixture: fn() -> DeviceCatalogSnapshot,
    state: &'static std::thread::LocalKey<RefCell<Option<device_browser::Browser>>>,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(size).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            state.with_borrow_mut(|browser| {
                let browser = browser.get_or_insert_with(|| {
                    device_browser::Browser::open_directory(
                        fixture(),
                        &intl,
                        "Mock Watch-o-Matic 9000",
                        "mock-internal",
                        directory,
                    )
                    .expect("the gallery catalog and initial directory are valid")
                });
                let _ = browser.show(ui, &intl);
            });
        },
    );
}

fn show_selected_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    directory: &str,
    selection_steps: u8,
    state: &'static std::thread::LocalKey<RefCell<Option<(device_browser::Browser, u8)>>>,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(egui::vec2(960.0, 640.0)).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            state.with_borrow_mut(|state| {
                let (browser, completed_steps) = state.get_or_insert_with(|| {
                    (
                        device_browser::Browser::open_directory(
                            catalog(),
                            &intl,
                            "Mock Watch-o-Matic 9000",
                            "mock-internal",
                            directory,
                        )
                        .expect("the gallery catalog and initial directory are valid"),
                        0,
                    )
                });
                if *completed_steps < selection_steps {
                    let table_id =
                        egui::Id::new(("device-explorer-entry-table", browser.device_key()));
                    ui.memory_mut(|memory| memory.request_focus(table_id));
                    ui.input_mut(|input| {
                        input.events.push(egui::Event::Key {
                            key: egui::Key::ArrowDown,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        });
                    });
                    *completed_steps += 1;
                    ui.ctx().request_repaint();
                }
                let _ = browser.show(ui, &intl);
            });
        },
    );
}

#[scene]
fn context_menu(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_context_menu_scene(ctx, ui, globals, &CONTEXT_BROWSER);
}

#[scene]
fn context_menu_light(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_context_menu_scene(ctx, ui, globals, &CONTEXT_BROWSER_LIGHT);
}

fn show_context_menu_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    state: &'static std::thread::LocalKey<RefCell<Option<(device_browser::Browser, bool)>>>,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(egui::vec2(960.0, 640.0)).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            state.with_borrow_mut(|state| {
                let (browser, selected) = state.get_or_insert_with(|| {
                    (
                        device_browser::Browser::open_directory(
                            catalog(),
                            &intl,
                            "Mock Watch-o-Matic 9000",
                            "mock-internal",
                            INITIAL_DIRECTORY,
                        )
                        .expect("the gallery catalog and initial directory are valid"),
                        false,
                    )
                });
                let table_id =
                    egui::Id::new(("device-explorer-entry-table", "mock:watch-o-matic-9000"));
                ui.memory_mut(|memory| memory.request_focus(table_id));
                ui.input_mut(|input| {
                    input.events.push(egui::Event::Key {
                        key: if *selected {
                            egui::Key::F10
                        } else {
                            egui::Key::ArrowDown
                        },
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: if *selected {
                            egui::Modifiers::SHIFT
                        } else {
                            egui::Modifiers::NONE
                        },
                    });
                });
                let _ = browser.show(ui, &intl);
                if !*selected {
                    *selected = true;
                    ui.ctx().request_repaint();
                }
            });
        },
    );
}

fn catalog() -> DeviceCatalogSnapshot {
    DeviceCatalogSnapshot {
        device_key: "mock:watch-o-matic-9000".to_owned(),
        storages: vec![
            DeviceCatalogStorage {
                id: "mock-internal".to_owned(),
                label: "Mock internal storage".to_owned(),
                entries: entries("city-ride.fit", true),
            },
            DeviceCatalogStorage {
                id: "mock-card".to_owned(),
                label: "Mock memory card".to_owned(),
                entries: entries("made-up-evening-ride.fit", false),
            },
        ],
    }
}

fn empty_catalog() -> DeviceCatalogSnapshot {
    DeviceCatalogSnapshot {
        device_key: "mock:empty-watch-o-matic-9000".to_owned(),
        storages: vec![DeviceCatalogStorage {
            id: "mock-internal".to_owned(),
            label: "Mock internal storage".to_owned(),
            entries: Vec::new(),
        }],
    }
}

fn entries(activity: &str, include_bookmark_examples: bool) -> Vec<DeviceCatalogEntry> {
    let mut entries = vec![
        DeviceCatalogEntry {
            path: "Garmin".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        },
        DeviceCatalogEntry {
            path: "Garmin/Activity".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        },
        DeviceCatalogEntry {
            path: "Garmin/Activity/History".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        },
        DeviceCatalogEntry {
            path: "Garmin/Activity/History/2026".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        },
        DeviceCatalogEntry {
            path: format!("Garmin/Activity/History/{activity}").into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(48_216),
        },
        DeviceCatalogEntry {
            path: "Garmin/Courses".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        },
        DeviceCatalogEntry {
            path: "Garmin/GarminDevice.xml".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(7_412),
        },
    ];
    if include_bookmark_examples {
        entries.extend([
            DeviceCatalogEntry {
                path: "Garmin/Workouts".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Music".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Podcasts".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "GARMIN-TOOLKIT".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "GARMIN-TOOLKIT/transactions".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "GARMIN-TOOLKIT/manifest.toml".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(1_024),
            },
        ]);
    }
    entries
}
