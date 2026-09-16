use crate::SceneStateKey as _;
use gallery::prelude::*;
use garmin_color::swatch;
use garmin_model::device::{DeviceStorageState, StorageCapacity};
use garmin_service_api::{
    DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogSnapshot, DeviceCatalogStorage,
    DeviceSnapshot, InspectionState,
};
use garmin_ui::{device, device_browser, profile, shell, workspace};

scene_meta! { title: "Application / Devices / File browser" }

thread_local! {
    static BROWSERS: crate::SceneState<device_browser::Browser, 6> = const { crate::SceneState::empty() };
    static SELECTED_BROWSERS: crate::SceneState<(device_browser::Browser, u8), 2> = const { crate::SceneState::empty() };
    static CONTEXT_BROWSERS: crate::SceneState<(device_browser::Browser, bool), 2> = const { crate::SceneState::empty() };
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum BrowserSlot {
    Default,
    Root,
    Empty,
    Narrow,
    Window,
    NarrowWindow,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum SelectedBrowserSlot {
    Generic,
    Fit,
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum ContextBrowserSlot {
    Dark,
    Light,
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
            BROWSERS.with_scene(
                BrowserSlot::Default as usize,
                || {
                    device_browser::Browser::open_directory(
                        catalog(),
                        &intl,
                        "Mock Watch-o-Matic 9000",
                        "mock-internal",
                        INITIAL_DIRECTORY,
                    )
                    .expect("the gallery catalog and initial directory are valid")
                },
                |browser| {
                    let _ = browser.show(ui, &intl);
                },
            );
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
        BrowserSlot::Window,
    );
}

#[scene]
fn compact_window(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_window_scene(
        ctx,
        ui,
        globals,
        egui::vec2(640.0, 640.0),
        BrowserSlot::NarrowWindow,
    );
}

fn show_window_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    size: egui::Vec2,
    slot: BrowserSlot,
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
            BROWSERS.with_scene(
                slot as usize,
                || {
                    device_browser::Browser::open_directory(
                        catalog(),
                        &intl,
                        &devices[0].name,
                        "mock-internal",
                        INITIAL_DIRECTORY,
                    )
                    .expect("the gallery catalog and initial directory are valid")
                },
                |browser| {
                    let _ = browser.show_window(ui, &intl);
                },
            );
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
        BrowserSlot::Root,
    );
}

#[scene]
fn selected_generic_file(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_selected_scene(ctx, ui, globals, "Garmin", 4, SelectedBrowserSlot::Generic);
}

#[scene]
fn selected_fit_file(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_selected_scene(
        ctx,
        ui,
        globals,
        INITIAL_DIRECTORY,
        2,
        SelectedBrowserSlot::Fit,
    );
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
        BrowserSlot::Empty,
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
        BrowserSlot::Narrow,
    );
}

fn show_directory_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    size: egui::Vec2,
    directory: &str,
    fixture: fn() -> DeviceCatalogSnapshot,
    slot: BrowserSlot,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(size).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            BROWSERS.with_scene(
                slot as usize,
                || {
                    device_browser::Browser::open_directory(
                        fixture(),
                        &intl,
                        "Mock Watch-o-Matic 9000",
                        "mock-internal",
                        directory,
                    )
                    .expect("the gallery catalog and initial directory are valid")
                },
                |browser| {
                    let _ = browser.show(ui, &intl);
                },
            );
        },
    );
}

fn show_selected_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    directory: &str,
    selection_steps: u8,
    slot: SelectedBrowserSlot,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(egui::vec2(960.0, 640.0)).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            SELECTED_BROWSERS.with_scene(
                slot as usize,
                || {
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
                },
                |(browser, completed_steps)| {
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
                },
            );
        },
    );
}

#[scene]
fn context_menu(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_context_menu_scene(ctx, ui, globals, ContextBrowserSlot::Dark);
}

#[scene]
fn context_menu_light(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &crate::Globals) {
    show_context_menu_scene(ctx, ui, globals, ContextBrowserSlot::Light);
}

fn show_context_menu_scene(
    ctx: &mut SceneCtx<'_>,
    ui: &mut Ui,
    globals: &crate::Globals,
    slot: ContextBrowserSlot,
) {
    stage!(
        ctx,
        ui,
        Stage::Fixed(egui::vec2(960.0, 640.0)).checkerboard(globals.checkerboard()),
        |ui| {
            let intl = globals.intl();
            CONTEXT_BROWSERS.with_scene(
                slot as usize,
                || {
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
                },
                |(browser, selected)| {
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
                },
            );
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
