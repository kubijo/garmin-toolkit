//! Explorer window for files exposed by a connected device.

use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
};

use camino::{Utf8Path, Utf8PathBuf};
use egui::{Align, Align2, EventFilter, Id, Key, Layout, Rect, RichText, Sense, Ui, UiBuilder};
use egui_extras::{Column, TableBuilder};
use egui_ltreeview::{
    Action as TreeAction, CloserState, NodeBuilder, RowLayout, TreeView, TreeViewState,
};
use garmin_color::{Color, theme as color_theme};
use garmin_i18n::{Intl, format_message};
use garmin_service_api::{
    DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogSnapshot, DeviceCatalogStorage,
};

use crate::{Size, button, header_selector, icons, input, modal, theme::color32};

const TREE_WIDTH: f32 = 200.0;
const DETAILS_WIDTH: f32 = 248.0;
const COMPACT_LAYOUT_WIDTH: f32 = 760.0;
const COMPACT_DRAWER_WIDTH: f32 = 300.0;
const WINDOW_DEFAULT_WIDTH: f32 = 960.0;
const WINDOW_DEFAULT_HEIGHT: f32 = 640.0;
const WINDOW_MIN_WIDTH: f32 = 480.0;
const WINDOW_MIN_HEIGHT: f32 = 360.0;
const WINDOW_VIEWPORT_INSET: f32 = 24.0;
const HEADER_HEIGHT: f32 = 32.0;
const TABLE_HEADER_HEIGHT: f32 = 24.0;
const ROW_HEIGHT: f32 = 32.0;
const TREE_ROW_HEIGHT: f32 = 24.0;
const TREE_ROW_GAP: f32 = 0.0;
const TREE_INDENT: f32 = 24.0;
const TREE_EDGE_PADDING: f32 = 8.0;
const TREE_ICON_SLOT_WIDTH: f32 = 28.0;
const TREE_ICON_OPTICAL_OFFSET_Y: f32 = 1.0;
const BOOKMARK_ROW_HEIGHT: f32 = 28.0;
const BOOKMARK_INLINE_PADDING: f32 = 12.0;
const BOOKMARK_ICON_GAP: f32 = 8.0;
const SIDEBAR_SEPARATOR_HEIGHT: f32 = 9.0;
const ICON_SIZE: f32 = 16.0;
const ENTRY_INLINE_PADDING: f32 = 16.0;
const ENTRY_ICON_GAP: f32 = 8.0;
const SIZE_UNIT_SLOT_WIDTH: f32 = 28.0;
const SIZE_UNIT_GAP: f32 = 4.0;
const BREADCRUMB_HEIGHT: f32 = 28.0;
const BREADCRUMB_CONTENT_PADDING: f32 = 8.0;
const BREADCRUMB_ITEM_PADDING: f32 = 8.0;
const BREADCRUMB_ANCESTOR_ALPHA: u8 = 0xA0;
const PATH_NAVIGATION_GAP: f32 = 8.0;
const PATH_NAVIGATION_ITEM_GAP: f32 = 2.0;
const PANE_HEADER_TOP_MARGIN: f32 = 6.0;
const PANE_HEADER_BOTTOM_MARGIN: f32 = 10.0;
const PANE_HEADER_HEIGHT: f32 =
    PANE_HEADER_TOP_MARGIN + BREADCRUMB_HEIGHT + PANE_HEADER_BOTTOM_MARGIN;
const PANE_INLINE_PADDING: f32 = 12.0;
const PANE_CLOSE_BUTTON_SIZE: f32 = 32.0;
const PANE_CONTENT_MARGIN: egui::Margin = egui::Margin::symmetric(12, 0);
const TABLE_BORDER_ALPHA: u8 = 0x40;
const TABLE_STRIPE_CONTRAST_DIVISOR: u8 = 5;
const TABLE_STRIPE_CONTRAST_MULTIPLIER: u8 = 2;
const UPLOAD_ACTION_AREA_HEIGHT: f32 = 52.0;
const UPLOAD_ACTION_INSET: f32 = 12.0;
const NAVIGATION_HISTORY_LIMIT: usize = 64;
const CONTEXT_MENU_WIDTH: f32 = 208.0;
const CONTEXT_MENU_ROW_HEIGHT: f32 = 36.0;
const CONTEXT_MENU_INLINE_PADDING: f32 = 12.0;
const CONTEXT_MENU_ICON_GAP: f32 = 8.0;
const DETAILS_SUMMARY_ROW_HEIGHT: f32 = 20.0;
const DETAILS_SUMMARY_GAP: f32 = 2.0;
const DETAILS_ACTION_HEIGHT: f32 = 32.0;
const DETAILS_ACTION_GAP: f32 = 2.0;
const DETAILS_ACTION_TOP_GAP: f32 = 8.0;
const DETAILS_ACTION_BOTTOM_INSET: f32 = 12.0;
const TOOLKIT_CONTENT_OPACITY: f32 = 0.62;

/// One entry selected in the device explorer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    pub storage_id: String,
    pub storage_label: String,
    pub path: Utf8PathBuf,
    pub kind: DeviceCatalogEntryKind,
    pub size: Option<u64>,
}

impl Selection {
    #[must_use]
    pub fn is_fit_file(&self) -> bool {
        self.kind == DeviceCatalogEntryKind::File
            && self.path.as_str().to_ascii_lowercase().ends_with(".fit")
    }

    fn name(&self) -> &str {
        file_name(&self.path)
    }

    fn display_name(&self) -> &str {
        if self.path.as_str().is_empty() {
            &self.storage_label
        } else {
            self.name()
        }
    }

    fn is_toolkit_managed(&self) -> bool {
        path_is_toolkit_managed(&self.path)
    }
}

/// An explicit operation requested from the explorer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Close,
    Open(Selection),
    ImportFit(Selection),
    /// Download a file directly, or archive a directory as ZIP and then download it.
    Download(Selection),
    Upload {
        storage_id: String,
        directory: Utf8PathBuf,
    },
    CreateDirectory {
        storage_id: String,
        parent: Utf8PathBuf,
        name: String,
    },
    Remove(Selection),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct DirectoryId {
    storage_index: usize,
    path: Utf8PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ExplorerWindowGeometry {
    default_position: egui::Pos2,
    default_size: egui::Vec2,
    min_size: egui::Vec2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Bookmark {
    kind: BookmarkKind,
    directory: DirectoryId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum BookmarkKind {
    Activities,
    Courses,
    Workouts,
    Music,
    Podcasts,
    Audiobooks,
}

impl BookmarkKind {
    const ALL: [Self; 6] = [
        Self::Activities,
        Self::Courses,
        Self::Workouts,
        Self::Music,
        Self::Podcasts,
        Self::Audiobooks,
    ];

    const fn candidates(self) -> &'static [&'static str] {
        match self {
            Self::Activities => &["Garmin/Activity"],
            Self::Courses => &["Garmin/Courses"],
            Self::Workouts => &["Garmin/Workouts"],
            Self::Music => &["Music", "Garmin/Music"],
            Self::Podcasts => &["Podcasts", "Garmin/Podcasts"],
            Self::Audiobooks => &["Audiobooks", "Garmin/Audiobooks"],
        }
    }

    const fn icon(self) -> icons::Icon {
        match self {
            Self::Activities => icons::ACTIVITY,
            Self::Courses => icons::ROUTE,
            Self::Workouts => icons::PERSON_SIMPLE_RUN,
            Self::Music => icons::MUSIC_NOTES,
            Self::Podcasts => icons::MICROPHONE,
            Self::Audiobooks => icons::BOOK_OPEN,
        }
    }

    fn label(self, intl: &Intl) -> String {
        match self {
            Self::Activities => format_message!(intl, default_message: "Activities"),
            Self::Courses => format_message!(intl, default_message: "Courses"),
            Self::Workouts => format_message!(intl, default_message: "Workouts"),
            Self::Music => format_message!(intl, default_message: "Music"),
            Self::Podcasts => format_message!(intl, default_message: "Podcasts"),
            Self::Audiobooks => format_message!(intl, default_message: "Audiobooks"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RowMove {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum HistoryMove {
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ItemAction {
    Open,
    ImportFit,
    Download,
    CreateDirectory,
    Remove,
}

impl ItemAction {
    const fn icon(self) -> icons::Icon {
        match self {
            Self::Open => icons::ACTIVITY,
            Self::ImportFit => icons::PLUS,
            Self::CreateDirectory => icons::FOLDER_PLUS,
            Self::Download => icons::DOWNLOAD_SIMPLE,
            Self::Remove => icons::TRASH,
        }
    }

    const fn row_kind(self) -> header_selector::RowKind {
        match self {
            Self::Remove => header_selector::RowKind::Danger,
            Self::Open | Self::ImportFit | Self::Download | Self::CreateDirectory => {
                header_selector::RowKind::Default
            }
        }
    }

    fn label(self, intl: &Intl, selection: &Selection) -> String {
        match self {
            Self::Open => format_message!(intl, default_message: "Open"),
            Self::ImportFit => format_message!(intl, default_message: "Import"),
            Self::Download if selection.kind == DeviceCatalogEntryKind::Directory => {
                format_message!(intl, default_message: "Download as ZIP")
            }
            Self::Download => format_message!(intl, default_message: "Download"),
            Self::CreateDirectory => format_message!(intl, default_message: "New folder"),
            Self::Remove => format_message!(intl, default_message: "Remove"),
        }
    }
}

fn available_item_actions(selection: &Selection, can_remove: bool) -> Vec<ItemAction> {
    let mut actions = Vec::with_capacity(5);
    if selection.is_fit_file() {
        actions.extend([ItemAction::Open, ItemAction::ImportFit]);
    }
    actions.push(ItemAction::Download);
    if !selection.is_toolkit_managed() {
        if selection.kind == DeviceCatalogEntryKind::Directory {
            actions.push(ItemAction::CreateDirectory);
        }
        if can_remove {
            actions.push(ItemAction::Remove);
        }
    }
    actions
}

fn activated_item_action(selection: &Selection) -> Option<ItemAction> {
    selection.is_fit_file().then_some(ItemAction::Open)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryInteraction {
    Select { index: usize, activate: bool },
    Context { index: usize, action: ItemAction },
}

struct CreateDirectoryDialog {
    parent: Selection,
    name: String,
    focus_name: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirectoryNameError {
    Unsafe,
    Duplicate,
}

#[derive(Clone, Copy)]
enum PaneSurface {
    Workspace,
    Sidebar,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CompactPane {
    Storage,
    Details,
}

#[derive(Clone, Copy)]
struct ResizeStrokeWidths {
    hovered: f32,
    active: f32,
}

#[derive(Clone, Copy)]
struct WindowPresentation<'a> {
    title: &'a str,
    close_label: &'a str,
    drag_delta: &'a Cell<egui::Vec2>,
}

#[derive(Clone, Copy)]
struct PathHeaderProps<'a> {
    back_label: &'a str,
    forward_label: &'a str,
    can_go_back: bool,
    can_go_forward: bool,
    compact_controls: Option<(&'a str, &'a str, Option<CompactPane>)>,
    close_label: Option<&'a str>,
    window_drag_delta: Option<&'a Cell<egui::Vec2>>,
}

/// Persistent state for an embedded device explorer page.
pub struct Browser {
    catalog: DeviceCatalogSnapshot,
    device_name: String,
    current: DirectoryId,
    selected: Option<Selection>,
    tree_state: TreeViewState<DirectoryId>,
    back_stack: Vec<DirectoryId>,
    forward_stack: Vec<DirectoryId>,
    create_directory: Option<CreateDirectoryDialog>,
    pending_removal: Option<Selection>,
    compact_pane: Option<CompactPane>,
    window_position: Option<egui::Pos2>,
}

impl Browser {
    /// Open an embedded explorer over a validated device catalog snapshot.
    ///
    /// # Errors
    /// Returns a stable diagnostic when the catalog contains unsafe, duplicate, or orphaned paths.
    pub fn open(
        mut catalog: DeviceCatalogSnapshot,
        _intl: &Intl,
        device_name: &str,
    ) -> Result<Self, String> {
        validate_catalog(&catalog)?;
        populate_directory_sizes(&mut catalog);
        for storage in &mut catalog.storages {
            storage.entries.sort_by(compare_entries);
        }
        let current = DirectoryId {
            storage_index: 0,
            path: Utf8PathBuf::new(),
        };
        let mut tree_state = TreeViewState::default();
        tree_state.set_one_selected(current.clone());
        Ok(Self {
            catalog,
            device_name: device_name.to_owned(),
            current,
            selected: None,
            tree_state,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            create_directory: None,
            pending_removal: None,
            compact_pane: None,
            window_position: None,
        })
    }

    /// Open an embedded explorer at a validated directory in one catalog storage.
    ///
    /// # Errors
    /// Returns a stable diagnostic when the catalog is invalid or the requested storage or
    /// directory does not exist.
    pub fn open_directory(
        catalog: DeviceCatalogSnapshot,
        intl: &Intl,
        device_name: &str,
        storage_id: &str,
        path: &str,
    ) -> Result<Self, String> {
        let mut browser = Self::open(catalog, intl, device_name)?;
        let storage_index = browser
            .catalog
            .storages
            .iter()
            .position(|storage| storage.id == storage_id)
            .ok_or_else(|| format!("unknown device storage `{storage_id}`"))?;
        if !path.is_empty()
            && !browser.catalog.storages[storage_index]
                .entries
                .iter()
                .any(|entry| entry.path == path && entry.kind == DeviceCatalogEntryKind::Directory)
        {
            return Err(format!(
                "unknown directory `{path}` in device storage `{storage_id}`"
            ));
        }
        browser.navigate(&DirectoryId {
            storage_index,
            path: path.into(),
        });
        Ok(browser)
    }

    #[must_use]
    pub fn device_key(&self) -> &str {
        &self.catalog.device_key
    }

    /// Replace the bounded catalog after a host mutation while retaining the nearest valid view.
    ///
    /// # Errors
    /// The refreshed catalog must describe the same device and preserve all catalog invariants.
    pub fn refresh(&mut self, mut catalog: DeviceCatalogSnapshot) -> Result<(), String> {
        if catalog.device_key != self.catalog.device_key {
            return Err("refreshed device catalog belongs to a different device".to_owned());
        }
        validate_catalog(&catalog)?;
        populate_directory_sizes(&mut catalog);
        for storage in &mut catalog.storages {
            storage.entries.sort_by(compare_entries);
        }
        let current_storage_id = self.catalog.storages[self.current.storage_index].id.clone();
        let storage_index = catalog
            .storages
            .iter()
            .position(|storage| storage.id == current_storage_id)
            .unwrap_or(0);
        let mut current_path = self.current.path.clone();
        while !current_path.as_str().is_empty()
            && !catalog.storages[storage_index].entries.iter().any(|entry| {
                entry.path == current_path && entry.kind == DeviceCatalogEntryKind::Directory
            })
        {
            current_path = parent_path(&current_path).to_owned();
        }
        self.selected = self.selected.take().and_then(|mut selected| {
            let storage = catalog
                .storages
                .iter()
                .find(|storage| storage.id == selected.storage_id)?;
            let entry = storage.entries.iter().find(|entry| {
                entry.path == selected.path
                    && entry.kind == selected.kind
                    && (entry.kind == DeviceCatalogEntryKind::Directory
                        || entry.size == selected.size)
            })?;
            selected.storage_label.clone_from(&storage.label);
            selected.size = entry.size;
            Some(selected)
        });
        self.catalog = catalog;
        self.current = DirectoryId {
            storage_index,
            path: current_path,
        };
        self.back_stack.clear();
        self.forward_stack.clear();
        self.tree_state.set_one_selected(self.current.clone());
        Ok(())
    }

    /// Draws the explorer in the supplied surface.
    pub fn show(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        self.show_surface(ui, intl, true, None)
    }

    /// Draws the explorer in a movable, resizable in-canvas window.
    pub fn show_window(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        let context = ui.ctx().clone();
        let bounds = ui.max_rect().intersect(context.content_rect());
        let geometry = explorer_window_geometry(bounds);
        let title = format_message!(
            intl,
            default_message: "Files on {device}",
            values: { device: self.device_name.as_str() },
        );
        let close_label = format_message!(intl, default_message: "Close");
        let short_title = format_message!(intl, default_message: "Files");
        let window_id = Id::new(("device-explorer-window", self.device_key()));
        let drag_delta = Cell::new(egui::Vec2::ZERO);
        let position = self.window_position.unwrap_or(geometry.default_position);
        let mut open = true;
        let response = egui::Window::new(&title)
            .id(window_id)
            .order(egui::Order::Foreground)
            .open(&mut open)
            .title_bar(false)
            .drag_area(egui::WindowDrag::Off)
            .current_pos(position)
            .collapsible(false)
            .resizable(true)
            .default_pos(geometry.default_position)
            .default_size(geometry.default_size)
            .min_size(geometry.min_size)
            .max_size(bounds.size())
            .constrain_to(bounds)
            .frame(explorer_window_frame(ui))
            .show(&context, |ui| {
                self.show_surface(
                    ui,
                    intl,
                    false,
                    Some(WindowPresentation {
                        title: &short_title,
                        close_label: &close_label,
                        drag_delta: &drag_delta,
                    }),
                )
            });
        let result = response.and_then(|response| {
            self.window_position = Some(constrained_window_position(
                response.response.rect,
                bounds,
                drag_delta.get(),
            ));
            response.inner.flatten()
        });

        if open { result } else { Some(Action::Close) }
    }

    fn show_surface(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        header: bool,
        window: Option<WindowPresentation<'_>>,
    ) -> Option<Action> {
        let mut detail_request = None;
        let page_action = ui
            .scope(|ui| {
                ui.style_mut().interaction.selectable_labels = false;

                let mut action = if header {
                    let action = self.show_header(ui, intl);
                    ui.add_space(12.0);
                    action
                } else {
                    None
                };

                let panel_height = ui.available_height().max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), panel_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if uses_compact_layout(ui.available_width()) {
                            let panel_rect = ui.available_rect_before_wrap();
                            egui::CentralPanel::default()
                                .frame(pane_frame(ui, PaneSurface::Workspace))
                                .show(ui, |ui| {
                                    if action.is_none() {
                                        action = self.show_entries(
                                            ui,
                                            intl,
                                            true,
                                            window.map(|window| window.close_label),
                                            window.map(|window| window.drag_delta),
                                        );
                                    }
                                });
                            if ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::NONE, Key::Escape)
                            }) {
                                self.compact_pane = None;
                            }
                            if action.is_none() {
                                let selection = self.selected.clone();
                                detail_request = self
                                    .show_compact_pane(ui, intl, panel_rect, selection.as_ref())
                                    .zip(selection);
                            }
                            return;
                        }

                        self.show_wide_panes(ui, intl, window, &mut action, &mut detail_request);
                    },
                );
                action
            })
            .inner;
        let page_action = page_action.or_else(|| {
            detail_request.and_then(|(item_action, selection)| {
                self.handle_item_action(item_action, selection)
            })
        });
        let dialog_action = self.show_dialogs(ui, intl);
        page_action.or(dialog_action)
    }

    fn show_wide_panes(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        window: Option<WindowPresentation<'_>>,
        action: &mut Option<Action>,
        detail_request: &mut Option<(ItemAction, Selection)>,
    ) {
        self.compact_pane = None;
        let resize_strokes = suppress_resize_strokes(ui);
        egui::Panel::left(Id::new(("device-explorer-tree", self.device_key())))
            .resizable(true)
            .show_separator_line(false)
            .default_size(TREE_WIDTH)
            .size_range(160.0..=360.0)
            .frame(pane_frame(ui, PaneSurface::Sidebar))
            .show(ui, |ui| {
                restore_resize_strokes(ui, resize_strokes);
                let _ = self.show_tree(
                    ui,
                    intl,
                    None,
                    window.map(|window| window.title),
                    window.map(|window| window.drag_delta),
                );
            });
        restore_resize_strokes(ui, resize_strokes);

        let selection = self.selected.clone();
        let resize_strokes = suppress_resize_strokes(ui);
        egui::Panel::right(Id::new(("device-explorer-details", self.device_key())))
            .resizable(true)
            .show_separator_line(false)
            .default_size(DETAILS_WIDTH)
            .size_range(210.0..=420.0)
            .frame(pane_frame(ui, PaneSurface::Sidebar))
            .show(ui, |ui| {
                restore_resize_strokes(ui, resize_strokes);
                if action.is_none() {
                    let (requested_action, close) = show_details(
                        ui,
                        intl,
                        selection.as_ref(),
                        window.map(|window| window.close_label),
                        window.map(|window| window.drag_delta),
                    );
                    if close {
                        *action = Some(Action::Close);
                    } else {
                        *detail_request = requested_action.zip(selection.clone());
                    }
                }
            });
        restore_resize_strokes(ui, resize_strokes);

        egui::CentralPanel::default()
            .frame(pane_frame(ui, PaneSurface::Workspace))
            .show(ui, |ui| {
                if action.is_none() && detail_request.is_none() {
                    *action = self.show_entries(
                        ui,
                        intl,
                        false,
                        None,
                        window.map(|window| window.drag_delta),
                    );
                }
            });
    }

    fn show_header(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        let mut action = None;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), HEADER_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let close = format_message!(intl, default_message: "Back to device");
                if (button::IconProps {
                    label: &close,
                    icon: icons::CARET_LEFT,
                    kind: button::Kind::Ghost,
                    size: Size::Small,
                    enabled: true,
                })
                .show(ui)
                .clicked()
                {
                    action = Some(Action::Close);
                }

                let title = format_message!(
                    intl,
                    default_message: "Files on {device}",
                    values: { device: self.device_name.as_str() },
                );
                ui.heading(title);
            },
        );
        action
    }

    fn show_tree(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        close_label: Option<&str>,
        window_title: Option<&str>,
        window_drag_delta: Option<&Cell<egui::Vec2>>,
    ) -> bool {
        let bookmarks = discover_bookmarks(&self.catalog);
        let heading = format_message!(intl, default_message: "Storage");
        let mut close = false;
        pane_header(ui, "storage", window_drag_delta, |ui, header_rect| {
            close = pane_heading(
                ui,
                header_rect,
                icons::FOLDER,
                window_title.unwrap_or(&heading).to_owned(),
                close_label,
            );
        });
        if close {
            return true;
        }

        let tree = TreeView::new(Id::new(("device-explorer-directories", self.device_key())))
            .row_layout(RowLayout::Compact)
            .override_indent(Some(TREE_INDENT))
            .allow_multi_selection(false)
            .allow_drag_and_drop(false);
        let catalog = &self.catalog;
        let (bookmark_target, actions) = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if window_title.is_some() {
                    sidebar_section_heading(ui, &heading);
                }
                let bookmark_target = if bookmarks.is_empty() {
                    None
                } else {
                    let target = show_bookmarks(ui, intl, catalog, &self.current, &bookmarks);
                    sidebar_separator(ui);
                    target
                };

                // `egui_ltreeview` adds the ambient item gap to every row and gives a
                // custom closer only a two-pixel label gap. Keep this dense explorer
                // tree independent of the more generous application-wide spacing.
                ui.spacing_mut().item_spacing.y = TREE_ROW_GAP;
                ui.spacing_mut().item_spacing.x = TREE_EDGE_PADDING;
                ui.spacing_mut().icon_width = TREE_ICON_SLOT_WIDTH;
                ui.spacing_mut().icon_width_inner = ICON_SIZE;
                ui.visuals_mut().widgets.inactive.fg_stroke.width = 0.0;
                let (_, actions) = tree.show_state(ui, &mut self.tree_state, |builder| {
                    for (storage_index, storage) in catalog.storages.iter().enumerate() {
                        show_storage_tree(builder, storage_index, storage);
                    }
                });
                (bookmark_target, actions)
            })
            .inner;
        if let Some(directory) = bookmark_target {
            self.navigate(&directory);
        }

        for tree_action in actions {
            let selected = match tree_action {
                TreeAction::SetSelected(selected) => selected.into_iter().next(),
                TreeAction::Activate(activated) => activated.selected.into_iter().next(),
                _ => None,
            };
            if let Some(directory) = selected {
                self.navigate(&directory);
            }
        }
        false
    }

    fn show_entries(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        compact: bool,
        close_label: Option<&str>,
        window_drag_delta: Option<&Cell<egui::Vec2>>,
    ) -> Option<Action> {
        let storage = &self.catalog.storages[self.current.storage_index];
        let storage_id = storage.id.clone();
        let storage_label = storage.label.clone();
        let rows = storage
            .entries
            .iter()
            .filter(|entry| parent_path(&entry.path) == self.current.path)
            .cloned()
            .collect::<Vec<_>>();

        if self.show_breadcrumbs(ui, intl, compact, close_label, window_drag_delta) {
            return Some(Action::Close);
        }
        let upload = self.show_upload_action(ui, intl);
        if upload.is_some() {
            return upload;
        }

        let selected_path = self
            .selected
            .as_ref()
            .map(|selection| selection.path.as_str());
        let table_id = Id::new(("device-explorer-entry-table", self.device_key()));
        let keyboard_move = requested_table_movement(ui, table_id);
        let keyboard_index = (!rows.is_empty())
            .then(|| keyboard_move.map(|movement| moved_row_index(&rows, selected_path, movement)))
            .flatten();
        let keyboard_context = requested_context_menu(ui, table_id);
        let background_rect = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let background = ui.interact(background_rect, table_id, Sense::click());
        background.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                true,
                format_message!(intl, default_message: "Current directory contents"),
            )
        });
        let interaction = if rows.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(format_message!(intl, default_message: "This folder is empty"));
            });
            None
        } else {
            show_entry_table(
                ui,
                intl,
                self.device_key(),
                &rows,
                selected_path,
                keyboard_index,
                keyboard_context,
            )
        };

        if let Some(interaction) = interaction {
            let (index, activate, context_action) = match interaction {
                EntryInteraction::Select { index, activate } => (index, activate, None),
                EntryInteraction::Context { index, action } => (index, false, Some(action)),
            };
            let entry = &rows[index];
            let selection = Selection {
                storage_id,
                storage_label,
                path: entry.path.clone(),
                kind: entry.kind,
                size: entry.size,
            };
            self.selected = Some(selection.clone());
            if let Some(context_action) = context_action {
                return self.handle_item_action(context_action, selection);
            }
            if !activate {
                return None;
            }
            return match selection.kind {
                DeviceCatalogEntryKind::Directory => {
                    self.navigate(&DirectoryId {
                        storage_index: self.current.storage_index,
                        path: selection.path,
                    });
                    None
                }
                DeviceCatalogEntryKind::File => activated_item_action(&selection)
                    .and_then(|item_action| self.handle_item_action(item_action, selection)),
            };
        }

        let current = self.current_directory_selection()?;
        if background.secondary_clicked() {
            self.selected = Some(current.clone());
            ui.memory_mut(|memory| memory.request_focus(table_id));
        }
        let context_action = show_context_menu(
            ui,
            &background,
            Id::new(("device-explorer-current-directory-menu", self.device_key())),
            &current,
            !current.path.as_str().is_empty(),
            keyboard_context && self.selected.as_ref() == Some(&current),
            intl,
        )?;
        self.handle_item_action(context_action, current)
    }

    fn current_directory_selection(&self) -> Option<Selection> {
        let storage = self.catalog.storages.get(self.current.storage_index)?;
        Some(Selection {
            storage_id: storage.id.clone(),
            storage_label: storage.label.clone(),
            path: self.current.path.clone(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        })
    }

    fn handle_item_action(
        &mut self,
        item_action: ItemAction,
        selection: Selection,
    ) -> Option<Action> {
        match item_action {
            ItemAction::Open => Some(Action::Open(selection)),
            ItemAction::ImportFit => Some(Action::ImportFit(selection)),
            ItemAction::Download => Some(Action::Download(selection)),
            ItemAction::CreateDirectory => {
                self.pending_removal = None;
                self.create_directory = Some(CreateDirectoryDialog {
                    parent: selection,
                    name: String::new(),
                    focus_name: true,
                });
                None
            }
            ItemAction::Remove => {
                self.create_directory = None;
                self.pending_removal = Some(selection);
                None
            }
        }
    }

    fn show_dialogs(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        if self.pending_removal.is_some() {
            return self.show_removal_dialog(ui, intl);
        }
        if self.create_directory.is_some() {
            return self.show_create_directory_dialog(ui, intl);
        }
        None
    }

    fn show_removal_dialog(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        let selection = self.pending_removal.clone()?;
        let title = match selection.kind {
            DeviceCatalogEntryKind::Directory => {
                format_message!(intl, default_message: "Remove this folder?")
            }
            DeviceCatalogEntryKind::File => {
                format_message!(intl, default_message: "Remove this file?")
            }
        };
        let description = match selection.kind {
            DeviceCatalogEntryKind::Directory => format_message!(
                intl,
                default_message: "The folder and everything inside it will be permanently removed from the device.",
            ),
            DeviceCatalogEntryKind::File => format_message!(
                intl,
                default_message: "The file will be permanently removed from the device.",
            ),
        };
        let cancel = format_message!(intl, default_message: "Cancel");
        let remove = format_message!(intl, default_message: "Remove");
        let output = modal::show(
            ui,
            Id::new(("device-explorer-confirm-removal", self.device_key())),
            &modal::Props {
                title: &title,
                description: Some(&description),
                size: modal::Size::Small,
                presentation: modal::Presentation::Modal,
                cancel_label: &cancel,
                backdrop_closes: Some(false),
                primary: modal::Primary {
                    label: &remove,
                    icon: Some(icons::TRASH),
                    kind: modal::PrimaryKind::Danger,
                    enabled: true,
                },
            },
            |ui| {
                ui.strong(selection.display_name());
                if !selection.path.as_str().is_empty() {
                    ui.label(RichText::new(selection.path.as_str()).weak());
                }
            },
        );
        match output.action {
            Some(modal::Action::Cancel) => {
                self.pending_removal = None;
                None
            }
            Some(modal::Action::Primary) => {
                self.pending_removal = None;
                Some(Action::Remove(selection))
            }
            None => None,
        }
    }

    fn show_create_directory_dialog(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        let mut dialog = self.create_directory.take()?;
        let validation = validate_new_directory_name(&dialog.name, &self.catalog, &dialog.parent);
        let validation_message = validation.map(|error| match error {
            DirectoryNameError::Unsafe => format_message!(
                intl,
                default_message: "Use a single folder name without slashes or control characters",
            ),
            DirectoryNameError::Duplicate => format_message!(
                intl,
                default_message: "An item with that name already exists",
            ),
        });
        let title = format_message!(intl, default_message: "Create a folder");
        let description = format_message!(
            intl,
            default_message: "Create it inside {directory}.",
            values: { directory: dialog.parent.display_name() },
        );
        let cancel = format_message!(intl, default_message: "Cancel");
        let create = format_message!(intl, default_message: "Create");
        let name_label = format_message!(intl, default_message: "Folder name");
        let placeholder = format_message!(intl, default_message: "Enter a name");
        let output = modal::show(
            ui,
            Id::new(("device-explorer-create-directory", self.device_key())),
            &modal::Props {
                title: &title,
                description: Some(&description),
                size: modal::Size::Small,
                presentation: modal::Presentation::Modal,
                cancel_label: &cancel,
                backdrop_closes: Some(false),
                primary: modal::Primary {
                    label: &create,
                    icon: Some(icons::FOLDER_PLUS),
                    kind: modal::PrimaryKind::Confirm,
                    enabled: validation.is_none() && !dialog.name.trim().is_empty(),
                },
            },
            |ui| {
                let props = input::Props::new(&name_label)
                    .placeholder(&placeholder)
                    .size(Size::Medium)
                    .message(validation_message.as_deref().map(input::Message::Error));
                let response = input::show(ui, &mut dialog.name, props);
                if dialog.focus_name {
                    response.request_focus();
                    dialog.focus_name = false;
                }
            },
        );
        match output.action {
            Some(modal::Action::Cancel) => None,
            Some(modal::Action::Primary) => Some(Action::CreateDirectory {
                storage_id: dialog.parent.storage_id,
                parent: dialog.parent.path,
                name: dialog.name.trim().to_owned(),
            }),
            None => {
                self.create_directory = Some(dialog);
                None
            }
        }
    }

    fn show_breadcrumbs(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        compact: bool,
        close_label: Option<&str>,
        window_drag_delta: Option<&Cell<egui::Vec2>>,
    ) -> bool {
        let storage_index = self.current.storage_index;
        let storage_label = self.catalog.storages[storage_index].label.clone();
        let current_path = self.current.path.clone();
        let back_label = format_message!(intl, default_message: "Back");
        let forward_label = format_message!(intl, default_message: "Forward");
        let storage_control_label = format_message!(intl, default_message: "Storage");
        let details_control_label = format_message!(intl, default_message: "Details");
        let mut target = None;
        let (history_move, compact_pane, close) = path_header(
            ui,
            &PathHeaderProps {
                back_label: &back_label,
                forward_label: &forward_label,
                can_go_back: !self.back_stack.is_empty(),
                can_go_forward: !self.forward_stack.is_empty(),
                compact_controls: compact.then_some((
                    storage_control_label.as_str(),
                    details_control_label.as_str(),
                    self.compact_pane,
                )),
                close_label,
                window_drag_delta,
            },
            |ui| {
                let root = DirectoryId {
                    storage_index,
                    path: Utf8PathBuf::new(),
                };
                if breadcrumb(ui, current_path.as_str().is_empty(), storage_label).clicked() {
                    target = Some(root);
                }
                let mut path = Utf8PathBuf::new();
                for component in current_path
                    .as_str()
                    .split('/')
                    .filter(|part| !part.is_empty())
                {
                    show_breadcrumb_separator(ui);
                    path.push(component);
                    if breadcrumb(ui, path == current_path, component).clicked() {
                        target = Some(DirectoryId {
                            storage_index,
                            path: path.clone(),
                        });
                    }
                }
            },
        );
        if let Some(compact_pane) = compact_pane {
            self.compact_pane = (self.compact_pane != Some(compact_pane)).then_some(compact_pane);
        }
        match history_move {
            Some(HistoryMove::Back) => self.navigate_back(),
            Some(HistoryMove::Forward) => self.navigate_forward(),
            None => {
                if let Some(target) = target {
                    self.navigate(&target);
                }
            }
        }
        close
    }

    fn show_compact_pane(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        panel_rect: Rect,
        selection: Option<&Selection>,
    ) -> Option<ItemAction> {
        let compact_pane = self.compact_pane?;
        let drawer_width = COMPACT_DRAWER_WIDTH.min(panel_rect.width());
        let drawer_rect = match compact_pane {
            CompactPane::Storage => Rect::from_min_size(
                panel_rect.min,
                egui::vec2(drawer_width, panel_rect.height()),
            ),
            CompactPane::Details => Rect::from_min_size(
                egui::pos2(panel_rect.right() - drawer_width, panel_rect.top()),
                egui::vec2(drawer_width, panel_rect.height()),
            ),
        };
        let close_label = format_message!(intl, default_message: "Close");
        let drawer_palette = crate::theme::palette(ui);
        let mut item_action = None;
        let mut close = egui::Area::new(Id::new((
            "device-explorer-compact-backdrop",
            self.device_key(),
        )))
        .order(egui::Order::Middle)
        .fixed_pos(panel_rect.min)
        .fade_in(false)
        .show(ui.ctx(), |ui| {
            let (rect, response) = ui.allocate_exact_size(panel_rect.size(), Sense::click());
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::ZERO,
                egui::Color32::from_black_alpha(40),
            );
            response.clicked()
        })
        .inner;
        egui::Window::new("device-explorer-compact-pane")
            .id(Id::new((
                "device-explorer-compact-pane",
                self.device_key(),
                compact_pane,
            )))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .fade_in(false)
            .resizable(false)
            .collapsible(false)
            .fixed_rect(drawer_rect)
            .frame(compact_drawer_frame(ui))
            .show(ui.ctx(), |ui| {
                crate::theme::apply_palette(ui.style_mut(), drawer_palette);
                ui.set_min_size(drawer_rect.size());
                match compact_pane {
                    CompactPane::Storage => {
                        close |= self.show_tree(ui, intl, Some(&close_label), None, None);
                    }
                    CompactPane::Details => {
                        let (requested_action, pane_close) =
                            show_details(ui, intl, selection, Some(&close_label), None);
                        item_action = requested_action;
                        close |= pane_close;
                    }
                }
            });
        if close {
            self.compact_pane = None;
        }
        item_action
    }

    fn show_upload_action(&self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        let (storage_id, directory) = self.current_upload_target()?;
        let mut clicked = false;
        let workspace_fill = color32(crate::theme::palette(ui).surfaces().background());
        egui::Panel::bottom(Id::new((
            "device-explorer-upload-action",
            self.device_key(),
        )))
        .exact_size(UPLOAD_ACTION_AREA_HEIGHT)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(workspace_fill))
        .show(ui, |ui| {
            let mut action_rect = ui.max_rect().intersect(ui.clip_rect());
            action_rect.max.y -= UPLOAD_ACTION_INSET;
            let mut action_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(action_rect)
                    .layout(Layout::right_to_left(Align::Center)),
            );
            let upload = format_message!(intl, default_message: "Upload file");
            clicked = button::Props {
                label: &upload,
                icon: Some(icons::UPLOAD_SIMPLE),
                kind: button::Kind::Tertiary,
                size: Size::Small,
                width: button::Width::Fit,
                enabled: true,
            }
            .show(&mut action_ui)
            .clicked();
        });
        clicked.then_some(Action::Upload {
            storage_id,
            directory,
        })
    }

    fn current_upload_target(&self) -> Option<(String, Utf8PathBuf)> {
        if path_is_toolkit_managed(&self.current.path) {
            return None;
        }
        let storage = self.catalog.storages.get(self.current.storage_index)?;
        let is_directory = self.current.path.as_str().is_empty()
            || storage.entries.iter().any(|entry| {
                entry.path == self.current.path && entry.kind == DeviceCatalogEntryKind::Directory
            });
        is_directory.then(|| (storage.id.clone(), self.current.path.clone()))
    }

    fn navigate(&mut self, directory: &DirectoryId) {
        if directory == &self.current || directory.storage_index >= self.catalog.storages.len() {
            return;
        }
        push_history(&mut self.back_stack, self.current.clone());
        self.forward_stack.clear();
        self.set_current(directory.clone());
    }

    fn navigate_back(&mut self) {
        let Some(directory) = self.back_stack.pop() else {
            return;
        };
        push_history(&mut self.forward_stack, self.current.clone());
        self.set_current(directory);
    }

    fn navigate_forward(&mut self) {
        let Some(directory) = self.forward_stack.pop() else {
            return;
        };
        push_history(&mut self.back_stack, self.current.clone());
        self.set_current(directory);
    }

    fn set_current(&mut self, directory: DirectoryId) {
        self.current = directory;
        self.selected = None;
        self.tree_state.set_one_selected(self.current.clone());
        let current = self.current.clone();
        self.open_ancestors(&current);
    }

    fn open_ancestors(&mut self, directory: &DirectoryId) {
        self.tree_state.set_openness(
            DirectoryId {
                storage_index: directory.storage_index,
                path: Utf8PathBuf::new(),
            },
            true,
        );
        let mut path = Utf8PathBuf::new();
        for component in directory
            .path
            .as_str()
            .split('/')
            .filter(|part| !part.is_empty())
        {
            path.push(component);
            self.tree_state.set_openness(
                DirectoryId {
                    storage_index: directory.storage_index,
                    path: path.clone(),
                },
                true,
            );
        }
    }
}

fn push_history(stack: &mut Vec<DirectoryId>, directory: DirectoryId) {
    if stack.last() == Some(&directory) {
        return;
    }
    if stack.len() == NAVIGATION_HISTORY_LIMIT {
        stack.remove(0);
    }
    stack.push(directory);
}

fn uses_compact_layout(width: f32) -> bool {
    width < COMPACT_LAYOUT_WIDTH
}

fn explorer_window_geometry(bounds: Rect) -> ExplorerWindowGeometry {
    let available =
        (bounds.size() - egui::Vec2::splat(WINDOW_VIEWPORT_INSET * 2.0)).max(egui::Vec2::ZERO);
    let default_size = egui::vec2(
        WINDOW_DEFAULT_WIDTH.min(available.x),
        WINDOW_DEFAULT_HEIGHT.min(available.y),
    );
    ExplorerWindowGeometry {
        default_position: bounds.center() - default_size / 2.0,
        min_size: egui::vec2(
            WINDOW_MIN_WIDTH.min(default_size.x),
            WINDOW_MIN_HEIGHT.min(default_size.y),
        ),
        default_size,
    }
}

fn constrained_window_position(window: Rect, bounds: Rect, drag_delta: egui::Vec2) -> egui::Pos2 {
    let target = window.min + drag_delta;
    let maximum = egui::pos2(
        (bounds.right() - window.width()).max(bounds.left()),
        (bounds.bottom() - window.height()).max(bounds.top()),
    );
    egui::pos2(
        target.x.clamp(bounds.left(), maximum.x),
        target.y.clamp(bounds.top(), maximum.y),
    )
}

fn suppress_resize_strokes(ui: &mut Ui) -> ResizeStrokeWidths {
    let widgets = &mut ui.visuals_mut().widgets;
    let widths = ResizeStrokeWidths {
        hovered: widgets.hovered.fg_stroke.width,
        active: widgets.active.fg_stroke.width,
    };
    widgets.hovered.fg_stroke.width = 0.0;
    widgets.active.fg_stroke.width = 0.0;
    widths
}

fn restore_resize_strokes(ui: &mut Ui, widths: ResizeStrokeWidths) {
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.hovered.fg_stroke.width = widths.hovered;
    widgets.active.fg_stroke.width = widths.active;
}

fn pane_frame(ui: &Ui, surface: PaneSurface) -> egui::Frame {
    let palette = crate::theme::palette(ui);
    let fill = match surface {
        PaneSurface::Workspace => palette.surfaces().background(),
        PaneSurface::Sidebar => palette.surfaces().layer(color_theme::Level::One),
    };
    let inner_margin = match surface {
        PaneSurface::Workspace => PANE_CONTENT_MARGIN,
        PaneSurface::Sidebar => egui::Margin::ZERO,
    };
    egui::Frame::new()
        .fill(color32(fill))
        .inner_margin(inner_margin)
}

fn explorer_window_frame(ui: &Ui) -> egui::Frame {
    let palette = crate::theme::palette(ui);
    egui::Frame::new()
        .fill(color32(palette.surfaces().background()))
        .stroke(egui::Stroke::new(1.0, color32(palette.borders().strong())))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 32,
            spread: 4,
            color: egui::Color32::from_black_alpha(144),
        })
}

fn compact_drawer_frame(ui: &Ui) -> egui::Frame {
    let palette = crate::theme::palette(ui);
    egui::Frame::new()
        .fill(color32(palette.surfaces().layer(color_theme::Level::One)))
        .stroke(egui::Stroke::new(1.0, color32(palette.borders().subtle())))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 20,
            spread: 0,
            color: egui::Color32::from_black_alpha(40),
        })
}

fn pane_header(
    ui: &mut Ui,
    region: &'static str,
    window_drag_delta: Option<&Cell<egui::Vec2>>,
    content: impl FnOnce(&mut Ui, Rect),
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PANE_HEADER_HEIGHT),
        Sense::hover(),
    );
    let content_rect = pane_header_content_rect(rect);
    window_drag_handle(ui, content_rect, region, window_drag_delta);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    content(&mut child, rect);
}

fn window_drag_handle(
    ui: &mut Ui,
    rect: Rect,
    region: &'static str,
    drag_delta: Option<&Cell<egui::Vec2>>,
) {
    let Some(drag_delta) = drag_delta else {
        return;
    };
    let response = ui.interact(
        rect,
        ui.id().with(("device-explorer-window-drag", region)),
        Sense::drag(),
    );
    if response.dragged_by(egui::PointerButton::Primary) {
        drag_delta.set(drag_delta.get() + response.drag_delta());
    }
    if response.is_pointer_button_down_on() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}

fn sidebar_section_heading(ui: &mut Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), BOOKMARK_ROW_HEIGHT),
        Sense::hover(),
    );
    let content_rect = rect.shrink2(egui::vec2(PANE_INLINE_PADDING, 0.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let palette = crate::theme::palette(&child);
    icons::Props {
        icon: icons::FOLDER,
        size: ICON_SIZE,
        color: palette.content().icon_secondary(),
    }
    .show(&mut child);
    child.strong(label);
}

fn discover_bookmarks(catalog: &DeviceCatalogSnapshot) -> Vec<Bookmark> {
    BookmarkKind::ALL
        .into_iter()
        .filter_map(|kind| {
            catalog
                .storages
                .iter()
                .enumerate()
                .find_map(|(storage_index, storage)| {
                    kind.candidates().iter().find_map(|candidate| {
                        storage
                            .entries
                            .iter()
                            .find(|entry| {
                                entry.kind == DeviceCatalogEntryKind::Directory
                                    && entry.path.as_str().eq_ignore_ascii_case(candidate)
                            })
                            .map(|entry| Bookmark {
                                kind,
                                directory: DirectoryId {
                                    storage_index,
                                    path: entry.path.clone(),
                                },
                            })
                    })
                })
        })
        .collect()
}

fn show_bookmarks(
    ui: &mut Ui,
    intl: &Intl,
    catalog: &DeviceCatalogSnapshot,
    current: &DirectoryId,
    bookmarks: &[Bookmark],
) -> Option<DirectoryId> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut target = None;
        for bookmark in bookmarks {
            let label = bookmark.kind.label(intl);
            let storage = &catalog.storages[bookmark.directory.storage_index];
            let active = bookmark_contains_directory(&bookmark.directory, current);
            if bookmark_row(
                ui,
                bookmark.kind,
                &label,
                &storage.label,
                bookmark.directory.path.as_str(),
                active,
            )
            .clicked()
            {
                target = Some(bookmark.directory.clone());
            }
        }
        target
    })
    .inner
}

fn bookmark_row(
    ui: &mut Ui,
    kind: BookmarkKind,
    label: &str,
    storage_label: &str,
    path: &str,
    active: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), BOOKMARK_ROW_HEIGHT),
        Sense::click(),
    );
    let palette = crate::theme::palette(ui);
    if active || response.hovered() {
        let fill = if active {
            palette.surfaces().layer(color_theme::Level::Two)
        } else {
            palette.surfaces().layer_hover(color_theme::Level::One)
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, color32(fill));
    }
    let content_rect = rect.shrink2(egui::vec2(BOOKMARK_INLINE_PADDING, 0.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    icons::Props {
        icon: kind.icon(),
        size: ICON_SIZE,
        color: palette.content().icon_primary(),
    }
    .show(&mut child);
    child.add_space(BOOKMARK_ICON_GAP);
    child.add(egui::Label::new(label).selectable(false));
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(2.0, color32(palette.interaction().focus())),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("{storage_label} / {path}"))
}

fn bookmark_contains_directory(bookmark: &DirectoryId, current: &DirectoryId) -> bool {
    bookmark.storage_index == current.storage_index
        && (bookmark.path == current.path
            || current
                .path
                .as_str()
                .strip_prefix(bookmark.path.as_str())
                .is_some_and(|suffix| suffix.starts_with('/')))
}

fn sidebar_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SIDEBAR_SEPARATOR_HEIGHT),
        Sense::hover(),
    );
    let palette = crate::theme::palette(ui);
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, color32(palette.borders().subtle().with_alpha(0x40))),
    );
}

fn path_header(
    ui: &mut Ui,
    props: &PathHeaderProps<'_>,
    content: impl FnOnce(&mut Ui),
) -> (Option<HistoryMove>, Option<CompactPane>, bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PANE_HEADER_HEIGHT),
        Sense::hover(),
    );
    let (header_rect, navigation_rect, path_rect) = path_header_rects(
        rect,
        props.compact_controls.is_some(),
        props.close_label.is_some(),
    );
    window_drag_handle(ui, header_rect, "path", props.window_drag_delta);
    let palette = crate::theme::palette(ui);
    ui.painter().rect_filled(
        path_rect,
        egui::CornerRadius::ZERO,
        color32(palette.surfaces().field(color_theme::Level::One)),
    );

    let mut compact_pane = None;
    let navigation_start = if let Some((storage_label, _, active_pane)) = props.compact_controls {
        let storage_rect =
            Rect::from_min_size(navigation_rect.min, egui::Vec2::splat(BREADCRUMB_HEIGHT));
        if path_icon_button(
            ui,
            storage_rect,
            "storage",
            storage_label,
            icons::FOLDER,
            active_pane == Some(CompactPane::Storage),
        ) {
            compact_pane = Some(CompactPane::Storage);
        }
        storage_rect.right() + PATH_NAVIGATION_ITEM_GAP
    } else {
        navigation_rect.left()
    };
    let back_rect = Rect::from_min_size(
        egui::pos2(navigation_start, navigation_rect.top()),
        egui::Vec2::splat(BREADCRUMB_HEIGHT),
    );
    let forward_rect = back_rect.translate(egui::vec2(
        BREADCRUMB_HEIGHT + PATH_NAVIGATION_ITEM_GAP,
        0.0,
    ));
    let back_clicked = history_button(
        ui,
        back_rect,
        HistoryMove::Back,
        props.back_label,
        icons::CARET_LEFT,
        props.can_go_back,
    );
    let forward_clicked = history_button(
        ui,
        forward_rect,
        HistoryMove::Forward,
        props.forward_label,
        icons::CARET_RIGHT,
        props.can_go_forward,
    );
    let history_move = if back_clicked {
        Some(HistoryMove::Back)
    } else if forward_clicked {
        Some(HistoryMove::Forward)
    } else {
        None
    };

    let (trailing_pane, close) = show_path_trailing_controls(ui, header_rect, props);
    compact_pane = compact_pane.or(trailing_pane);

    let content_rect = path_rect.shrink2(egui::vec2(BREADCRUMB_CONTENT_PADDING, 0.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.set_clip_rect(path_rect);
    child.spacing_mut().button_padding = egui::vec2(BREADCRUMB_ITEM_PADDING, 0.0);
    child.spacing_mut().interact_size.y = BREADCRUMB_HEIGHT;
    content(&mut child);
    (history_move, compact_pane, close)
}

fn show_path_trailing_controls(
    ui: &mut Ui,
    header_rect: Rect,
    props: &PathHeaderProps<'_>,
) -> (Option<CompactPane>, bool) {
    let close_rect = props.close_label.map(|_| {
        Rect::from_min_size(
            egui::pos2(header_rect.right() - BREADCRUMB_HEIGHT, header_rect.top()),
            egui::Vec2::splat(BREADCRUMB_HEIGHT),
        )
    });
    let close = props
        .close_label
        .zip(close_rect)
        .is_some_and(|(label, rect)| path_icon_button(ui, rect, "close", label, icons::X, false));

    let Some((_, details_label, active_pane)) = props.compact_controls else {
        return (None, close);
    };
    let right = close_rect.map_or(header_rect.right(), |rect| {
        rect.left() - PATH_NAVIGATION_ITEM_GAP
    });
    let details_rect = Rect::from_min_size(
        egui::pos2(right - BREADCRUMB_HEIGHT, header_rect.top()),
        egui::Vec2::splat(BREADCRUMB_HEIGHT),
    );
    let details = path_icon_button(
        ui,
        details_rect,
        "details",
        details_label,
        icons::INFO,
        active_pane == Some(CompactPane::Details),
    );
    (details.then_some(CompactPane::Details), close)
}

fn path_header_rects(rect: Rect, compact: bool, close: bool) -> (Rect, Rect, Rect) {
    let header = pane_header_block_rect(rect);
    let compact_navigation_width = if compact {
        BREADCRUMB_HEIGHT + PATH_NAVIGATION_ITEM_GAP
    } else {
        0.0
    };
    let navigation_width =
        BREADCRUMB_HEIGHT.mul_add(2.0, PATH_NAVIGATION_ITEM_GAP) + compact_navigation_width;
    let navigation = Rect::from_min_max(
        header.min,
        egui::pos2(
            (header.left() + navigation_width).min(header.right()),
            header.bottom(),
        ),
    );
    let trailing_controls = u8::from(compact) + u8::from(close);
    let trailing_width = match trailing_controls {
        0 => 0.0,
        count => {
            BREADCRUMB_HEIGHT * f32::from(count)
                + PATH_NAVIGATION_ITEM_GAP * f32::from(count.saturating_sub(1))
                + PATH_NAVIGATION_GAP
        }
    };
    let path = Rect::from_min_max(
        egui::pos2(
            (navigation.right() + PATH_NAVIGATION_GAP).min(header.right()),
            header.top(),
        ),
        egui::pos2(
            (header.right() - trailing_width).max(header.left()),
            header.bottom(),
        ),
    );
    (header, navigation, path)
}

fn path_icon_button(
    ui: &mut Ui,
    rect: Rect,
    id_salt: &'static str,
    label: &str,
    icon: icons::Icon,
    selected: bool,
) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("device-explorer-pane", id_salt)),
        Sense::click(),
    );
    let palette = crate::theme::palette(ui);
    if selected || response.hovered() {
        let fill = if selected {
            palette.surfaces().layer(color_theme::Level::Two)
        } else {
            palette.surfaces().background_hover()
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, color32(fill));
    }
    icons::Props {
        icon,
        size: 14.0,
        color: if response.hovered() || selected {
            palette.content().icon_primary()
        } else {
            palette.content().icon_secondary()
        },
    }
    .paint_at(ui, rect.center());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
        .clicked()
}

fn history_button(
    ui: &mut Ui,
    rect: Rect,
    direction: HistoryMove,
    label: &str,
    icon: icons::Icon,
    enabled: bool,
) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("device-explorer-history", direction)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let palette = crate::theme::palette(ui);
    if enabled && response.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::ZERO,
            color32(palette.surfaces().background_hover()),
        );
    }
    icons::Props {
        icon,
        size: 14.0,
        color: if enabled {
            if response.hovered() {
                palette.content().icon_primary()
            } else {
                palette.content().icon_secondary()
            }
        } else {
            palette.content().icon_disabled()
        },
    }
    .paint_at(ui, rect.center());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let response = response.on_hover_text(label);
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    enabled && response.clicked()
}

fn pane_header_block_rect(rect: Rect) -> Rect {
    Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + PANE_HEADER_TOP_MARGIN),
        egui::pos2(rect.right(), rect.bottom() - PANE_HEADER_BOTTOM_MARGIN),
    )
}

fn pane_header_content_rect(rect: Rect) -> Rect {
    pane_header_block_rect(rect).shrink2(egui::vec2(PANE_INLINE_PADDING, 0.0))
}

fn show_storage_tree(
    builder: &mut egui_ltreeview::TreeViewBuilder<'_, DirectoryId>,
    storage_index: usize,
    storage: &DeviceCatalogStorage,
) {
    let root = DirectoryId {
        storage_index,
        path: Utf8PathBuf::new(),
    };
    let open = builder.node(
        NodeBuilder::dir(root)
            .label(storage.label.clone())
            .closer(|ui, state| show_tree_storage_icon(ui, &state))
            .height(TREE_ROW_HEIGHT)
            .activatable(true)
            .default_open(true)
            .drop_allowed(false),
    );
    if open {
        show_directory_children(builder, storage_index, storage, Utf8Path::new(""));
    }
    builder.close_dir();
}

fn show_directory_children(
    builder: &mut egui_ltreeview::TreeViewBuilder<'_, DirectoryId>,
    storage_index: usize,
    storage: &DeviceCatalogStorage,
    parent: &Utf8Path,
) {
    for entry in storage.entries.iter().filter(|entry| {
        entry.kind == DeviceCatalogEntryKind::Directory && parent_path(&entry.path) == parent
    }) {
        let toolkit_managed = path_is_toolkit_managed(&entry.path);
        let label = file_name(&entry.path).to_owned();
        let id = DirectoryId {
            storage_index,
            path: entry.path.clone(),
        };
        let open = builder.node(
            NodeBuilder::dir(id)
                .label_ui(move |ui| {
                    if toolkit_managed {
                        ui.multiply_opacity(TOOLKIT_CONTENT_OPACITY);
                    }
                    ui.add(egui::Label::new(&label).selectable(false));
                })
                .closer(move |ui, state| {
                    if toolkit_managed {
                        show_tree_toolkit_icon(ui, &state);
                    } else {
                        show_tree_folder_icon(ui, &state);
                    }
                })
                .height(TREE_ROW_HEIGHT)
                .activatable(true)
                .drop_allowed(false),
        );
        if open {
            show_directory_children(builder, storage_index, storage, &entry.path);
        }
        builder.close_dir();
    }
}

fn show_details(
    ui: &mut Ui,
    intl: &Intl,
    selection: Option<&Selection>,
    close_label: Option<&str>,
    window_drag_delta: Option<&Cell<egui::Vec2>>,
) -> (Option<ItemAction>, bool) {
    let mut close = false;
    pane_header(ui, "details", window_drag_delta, |ui, header_rect| {
        close = pane_heading(
            ui,
            header_rect,
            icons::INFO,
            format_message!(intl, default_message: "Details"),
            close_label,
        );
    });
    let action = egui::Frame::new()
        .inner_margin(PANE_CONTENT_MARGIN)
        .show(ui, |ui| show_details_content(ui, intl, selection))
        .inner;
    (action, close)
}

fn pane_heading(
    ui: &mut Ui,
    header_rect: Rect,
    icon: icons::Icon,
    label: String,
    close_label: Option<&str>,
) -> bool {
    let palette = crate::theme::palette(ui);
    icons::Props {
        icon,
        size: ICON_SIZE,
        color: palette.content().icon_secondary(),
    }
    .show(ui);
    ui.strong(label);
    close_label.is_some_and(|close_label| {
        let close_rect = pane_close_button_rect(header_rect);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(close_rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                button::IconProps {
                    label: close_label,
                    icon: icons::X,
                    kind: button::Kind::Ghost,
                    size: Size::Small,
                    enabled: true,
                }
                .show(ui)
                .clicked()
            },
        )
        .inner
    })
}

fn pane_close_button_rect(header_rect: Rect) -> Rect {
    Rect::from_min_size(
        egui::pos2(
            header_rect.right() - PANE_CLOSE_BUTTON_SIZE,
            header_rect.top(),
        ),
        egui::Vec2::splat(PANE_CLOSE_BUTTON_SIZE),
    )
}

struct DetailsContentLayout {
    summary: Rect,
    actions: Option<Rect>,
}

fn details_content_layout(available: Rect, action_count: usize) -> DetailsContentLayout {
    if action_count == 0 {
        return DetailsContentLayout {
            summary: available,
            actions: None,
        };
    }
    let action_bottom = available.bottom() - DETAILS_ACTION_BOTTOM_INSET;
    let action_top = (action_bottom - DETAILS_ACTION_HEIGHT).max(available.top());
    DetailsContentLayout {
        summary: Rect::from_min_max(
            available.min,
            egui::pos2(
                available.right(),
                (action_top - DETAILS_ACTION_TOP_GAP).max(available.top()),
            ),
        ),
        actions: Some(Rect::from_min_max(
            egui::pos2(available.left(), action_top),
            egui::pos2(available.right(), action_bottom),
        )),
    }
}

fn show_details_content(
    ui: &mut Ui,
    intl: &Intl,
    selection: Option<&Selection>,
) -> Option<ItemAction> {
    let actions = selection
        .map(|selection| available_item_actions(selection, !selection.path.as_str().is_empty()))
        .unwrap_or_default();
    let available = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    ui.allocate_rect(available, Sense::hover());
    let layout = details_content_layout(available, actions.len());
    let mut summary_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(layout.summary)
            .layout(Layout::top_down(Align::Min)),
    );
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(&mut summary_ui, |ui| {
            show_details_summary(ui, intl, selection);
        });

    let selection = selection?;
    let action_rect = layout.actions?;
    let mut action_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(action_rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    action_ui.spacing_mut().interact_size.y = DETAILS_ACTION_HEIGHT;
    action_ui.spacing_mut().item_spacing.x = DETAILS_ACTION_GAP;
    actions.into_iter().find(|item_action| {
        button::IconProps {
            label: &item_action.label(intl, selection),
            icon: item_action.icon(),
            kind: button::Kind::Tertiary,
            size: Size::Small,
            enabled: true,
        }
        .show(&mut action_ui)
        .clicked()
    })
}

fn show_details_summary(ui: &mut Ui, intl: &Intl, selection: Option<&Selection>) {
    let Some(selection) = selection else {
        ui.label(format_message!(
            intl,
            default_message: "Select a file or folder to see its details",
        ));
        return;
    };

    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = DETAILS_SUMMARY_ROW_HEIGHT;
        ui.spacing_mut().item_spacing.y = DETAILS_SUMMARY_GAP;
        ui.horizontal(|ui| {
            if selection.is_toolkit_managed() {
                ui.multiply_opacity(TOOLKIT_CONTENT_OPACITY);
            }
            ui.spacing_mut().item_spacing.x = 8.0;
            show_selection_icon(ui, selection, 20.0);
            ui.strong(selection.display_name());
        });
        if !selection.path.as_str().is_empty() {
            ui.add(
                egui::Label::new(RichText::new(selection.path.as_str()).weak())
                    .wrap()
                    .selectable(false),
            );
        }
        show_entry_location(ui, selection);
        if selection.is_toolkit_managed() {
            show_toolkit_notice(ui, intl);
        }
    });
}

fn show_entry_location(ui: &mut Ui, selection: &Selection) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let palette = crate::theme::palette(ui);
        icons::Props {
            icon: icons::HARD_DRIVE,
            size: 14.0,
            color: palette.content().icon_secondary(),
        }
        .show(ui);
        ui.add(egui::Label::new(RichText::new(&selection.storage_label).weak()).selectable(false));
        if let Some(size) = selection.size {
            ui.label(RichText::new("·").weak());
            ui.label(RichText::new(format_bytes(size)).weak());
        }
    });
}

fn show_toolkit_notice(ui: &mut Ui, intl: &Intl) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let palette = crate::theme::palette(ui);
        icons::Props {
            icon: icons::TOOLBOX,
            size: 14.0,
            color: palette.content().icon_secondary(),
        }
        .show(ui);
        ui.add(
            egui::Label::new(
                RichText::new(format_message!(
                    intl,
                    default_message: "System data · browse only",
                ))
                .weak(),
            )
            .selectable(false),
        );
    });
}

fn show_selection_icon(ui: &mut Ui, selection: &Selection, size: f32) {
    if selection.kind == DeviceCatalogEntryKind::Directory && selection.is_toolkit_managed() {
        show_toolkit_icon(ui, size);
    } else {
        show_entry_icon_for_kind(ui, selection.kind, selection.is_fit_file(), size);
    }
}

fn show_entry_icon(ui: &mut Ui, entry: &DeviceCatalogEntry) {
    if entry.kind == DeviceCatalogEntryKind::Directory && path_is_toolkit_managed(&entry.path) {
        show_toolkit_icon(ui, ICON_SIZE);
        return;
    }
    let is_fit = entry.kind == DeviceCatalogEntryKind::File
        && entry.path.as_str().to_ascii_lowercase().ends_with(".fit");
    show_entry_icon_for_kind(ui, entry.kind, is_fit, ICON_SIZE);
}

fn show_toolkit_icon(ui: &mut Ui, size: f32) {
    let color = crate::theme::palette(ui).content().icon_secondary();
    icons::Props {
        icon: icons::FOLDER_DASHED,
        size,
        color,
    }
    .show(ui);
}

fn show_entry_icon_for_kind(ui: &mut Ui, kind: DeviceCatalogEntryKind, is_fit: bool, size: f32) {
    let icon = match kind {
        DeviceCatalogEntryKind::Directory => icons::FOLDER,
        DeviceCatalogEntryKind::File if is_fit => icons::ACTIVITY,
        DeviceCatalogEntryKind::File => icons::FILE,
    };
    let palette = crate::theme::palette(ui);
    icons::Props {
        icon,
        size,
        color: entry_icon_color(palette, kind, is_fit),
    }
    .show(ui);
}

fn entry_icon_color(
    palette: &color_theme::Theme,
    kind: DeviceCatalogEntryKind,
    is_fit: bool,
) -> Color {
    match kind {
        DeviceCatalogEntryKind::Directory => palette.content().icon_primary(),
        DeviceCatalogEntryKind::File if is_fit => palette.support().information(),
        DeviceCatalogEntryKind::File => palette.interaction().link_visited(),
    }
}

fn show_breadcrumb_separator(ui: &mut Ui) {
    let palette = crate::theme::palette(ui);
    ui.add(
        egui::Label::new(
            RichText::new("/").color(color32(
                palette
                    .content()
                    .text_secondary()
                    .with_alpha(BREADCRUMB_ANCESTOR_ALPHA),
            )),
        )
        .selectable(false),
    );
}

fn breadcrumb(ui: &mut Ui, current: bool, label: impl Into<String>) -> egui::Response {
    let palette = crate::theme::palette(ui);
    let color = if current {
        palette.content().text_primary()
    } else {
        palette
            .content()
            .text_secondary()
            .with_alpha(BREADCRUMB_ANCESTOR_ALPHA)
    };
    let text = RichText::new(label.into()).color(color32(color));
    let text = if current { text.strong() } else { text };
    let background = ui.painter().add(egui::Shape::Noop);
    let response = ui.add(
        egui::Button::new(text)
            .frame(false)
            .corner_radius(egui::CornerRadius::ZERO)
            .min_size(egui::vec2(0.0, BREADCRUMB_HEIGHT)),
    );
    if response.hovered() || response.has_focus() {
        ui.painter().set(
            background,
            egui::Shape::rect_filled(
                response.rect,
                egui::CornerRadius::ZERO,
                color32(palette.surfaces().background_hover()),
            ),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn show_tree_storage_icon(ui: &mut Ui, state: &CloserState) {
    show_tree_icon(ui, icons::HARD_DRIVE, state.is_hovered, None);
}

fn show_tree_folder_icon(ui: &mut Ui, state: &CloserState) {
    let icon = if state.is_open {
        icons::FOLDER_OPEN
    } else {
        icons::FOLDER
    };
    let folder_color = crate::theme::palette(ui).content().icon_primary();
    show_tree_icon(ui, icon, state.is_hovered, Some(folder_color));
}

fn show_tree_toolkit_icon(ui: &mut Ui, state: &CloserState) {
    ui.multiply_opacity(TOOLKIT_CONTENT_OPACITY);
    show_tree_icon(ui, icons::FOLDER_DASHED, state.is_hovered, None);
}

fn show_tree_icon(ui: &Ui, icon: icons::Icon, hovered: bool, color: Option<Color>) {
    let palette = crate::theme::palette(ui);
    icons::Props {
        icon,
        size: ICON_SIZE,
        color: color.unwrap_or_else(|| {
            if hovered {
                palette.content().icon_primary()
            } else {
                palette.content().icon_secondary()
            }
        }),
    }
    .paint_at(
        ui,
        ui.max_rect().center() + egui::vec2(0.0, TREE_ICON_OPTICAL_OFFSET_Y),
    );
}

fn requested_table_movement(ui: &mut Ui, table_id: Id) -> Option<RowMove> {
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            table_id,
            EventFilter {
                vertical_arrows: true,
                ..EventFilter::default()
            },
        );
    });
    if !ui.memory(|memory| memory.has_focus(table_id)) {
        return None;
    }
    ui.input_mut(|input| {
        if input.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
            Some(RowMove::Previous)
        } else if input.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
            Some(RowMove::Next)
        } else {
            None
        }
    })
}

fn requested_context_menu(ui: &mut Ui, table_id: Id) -> bool {
    if !ui.memory(|memory| memory.has_focus(table_id)) {
        return false;
    }
    ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, Key::F10))
}

fn show_entry_table(
    ui: &mut Ui,
    intl: &Intl,
    device_key: &str,
    rows: &[DeviceCatalogEntry],
    selected_path: Option<&str>,
    keyboard_index: Option<usize>,
    keyboard_context: bool,
) -> Option<EntryInteraction> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        show_entry_table_content(
            ui,
            intl,
            device_key,
            rows,
            selected_path,
            keyboard_index,
            keyboard_context,
        )
    })
    .inner
}

fn show_entry_table_content(
    ui: &mut Ui,
    intl: &Intl,
    device_key: &str,
    rows: &[DeviceCatalogEntry],
    selected_path: Option<&str>,
    keyboard_index: Option<usize>,
    keyboard_context: bool,
) -> Option<EntryInteraction> {
    let table_id = Id::new(("device-explorer-entry-table", device_key));
    let mut interaction = None;
    let mut request_focus = false;
    let mut row_responses = Vec::new();
    let table_clip = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    ui.set_clip_rect(table_clip);
    let table_painter = ui.painter().with_clip_rect(table_clip);
    let palette = crate::theme::palette(ui);
    let stripe = palette.surfaces().background_hover();
    let stripe_alpha =
        stripe.as_rgba()[3] / TABLE_STRIPE_CONTRAST_DIVISOR * TABLE_STRIPE_CONTRAST_MULTIPLIER;
    ui.visuals_mut().faint_bg_color = color32(stripe.with_alpha(stripe_alpha));
    let table_border = palette.borders().subtle().with_alpha(TABLE_BORDER_ALPHA);
    ui.visuals_mut().widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, color32(table_border));
    let mut table = TableBuilder::new(ui)
        .id_salt(("device-explorer-entries", device_key))
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        // A resizable remainder column keeps its initial width in `egui_extras`.
        // Keep the name column fluid so it follows every pane resize; the size
        // column remains the user-adjustable boundary.
        .column(Column::remainder().at_least(180.0).resizable(false))
        .column(Column::initial(100.0).at_least(72.0));
    if let Some(index) = keyboard_index {
        table = table.scroll_to_row(index, Some(egui::Align::Center));
    }
    table
        .header(TABLE_HEADER_HEIGHT, |mut header| {
            header.col(|ui| {
                prepare_entry_cell(ui);
                ui.strong(format_message!(intl, default_message: "Name"));
            });
            header.col(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(ENTRY_INLINE_PADDING);
                    ui.strong(format_message!(intl, default_message: "Size"));
                });
            });
            let response = header.response();
            table_painter.hline(
                response.rect.x_range(),
                response.rect.bottom(),
                egui::Stroke::new(1.0, color32(table_border)),
            );
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                let index = row.index();
                let entry = &rows[index];
                row.set_selected(selected_path == Some(entry.path.as_str()));
                row.col(|ui| {
                    prepare_entry_cell(ui);
                    ui.scope(|ui| {
                        if path_is_toolkit_managed(&entry.path) {
                            ui.multiply_opacity(TOOLKIT_CONTENT_OPACITY);
                        }
                        show_entry_icon(ui, entry);
                        ui.add_space(ENTRY_ICON_GAP);
                        ui.label(file_name(&entry.path));
                    });
                });
                row.col(|ui| {
                    show_table_size(ui, entry.size);
                });
                row_responses.push((index, row.response()));
            });
        });
    if let Some(pointer_interaction) = entry_pointer_interaction(
        ui,
        intl,
        device_key,
        rows,
        row_responses,
        selected_path,
        keyboard_context,
    ) {
        request_focus = true;
        interaction = Some(pointer_interaction);
    }
    if request_focus {
        ui.memory_mut(|memory| memory.request_focus(table_id));
    }
    interaction.or_else(|| {
        keyboard_index.map(|index| EntryInteraction::Select {
            index,
            activate: false,
        })
    })
}

fn entry_pointer_interaction(
    ui: &mut Ui,
    intl: &Intl,
    device_key: &str,
    rows: &[DeviceCatalogEntry],
    row_responses: Vec<(usize, egui::Response)>,
    selected_path: Option<&str>,
    keyboard_context: bool,
) -> Option<EntryInteraction> {
    let mut interaction = None;
    for (index, response) in row_responses {
        let entry = &rows[index];
        let context_action = show_context_menu(
            ui,
            &response,
            Id::new(("device-explorer-entry-menu", device_key, &entry.path)),
            &Selection {
                storage_id: String::new(),
                storage_label: String::new(),
                path: entry.path.clone(),
                kind: entry.kind,
                size: entry.size,
            },
            true,
            keyboard_context && selected_path == Some(entry.path.as_str()),
            intl,
        );
        if let Some(action) = context_action {
            interaction = Some(EntryInteraction::Context { index, action });
        } else if response.double_clicked() {
            interaction = Some(EntryInteraction::Select {
                index,
                activate: true,
            });
        } else if response.clicked() || response.secondary_clicked() {
            interaction = Some(EntryInteraction::Select {
                index,
                activate: false,
            });
        }
    }
    interaction
}

fn show_context_menu(
    ui: &mut Ui,
    response: &egui::Response,
    id: Id,
    selection: &Selection,
    can_remove: bool,
    open_from_keyboard: bool,
    intl: &Intl,
) -> Option<ItemAction> {
    let popup_palette = crate::theme::palette(ui);
    let mut popup = egui::Popup::context_menu(response)
        .id(id)
        .frame(context_menu_frame(ui))
        .width(CONTEXT_MENU_WIDTH);
    if open_from_keyboard {
        popup = popup
            .open_memory(Some(egui::SetOpenCommand::Bool(true)))
            .at_position(response.rect.left_bottom());
    }
    popup
        .show(|ui| {
            crate::theme::apply_palette(ui.style_mut(), popup_palette);
            ui.set_width(CONTEXT_MENU_WIDTH);
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut action = None;
            for item_action in available_item_actions(selection, can_remove) {
                if context_menu_row(
                    ui,
                    &item_action.label(intl, selection),
                    item_action.icon(),
                    item_action == ItemAction::Remove,
                    item_action.row_kind(),
                ) {
                    action = Some(item_action);
                }
            }
            if action.is_some() {
                ui.close();
            }
            action
        })
        .and_then(|response| response.inner)
}

fn context_menu_frame(ui: &Ui) -> egui::Frame {
    let palette = crate::theme::palette(ui);
    egui::Frame::new()
        .fill(color32(palette.surfaces().layer(color_theme::Level::One)))
        .corner_radius(egui::CornerRadius::ZERO)
        .stroke(egui::Stroke::new(1.0, color32(palette.borders().subtle())))
        .shadow(egui::Shadow {
            offset: [0, 3],
            blur: 20,
            spread: 0,
            color: egui::Color32::from_black_alpha(48),
        })
        .inner_margin(egui::Margin::ZERO)
}

fn context_menu_row(
    ui: &mut Ui,
    label: &str,
    icon: icons::Icon,
    divided: bool,
    kind: header_selector::RowKind,
) -> bool {
    let response = header_selector::row_with_width_on_layer(
        ui,
        CONTEXT_MENU_WIDTH,
        CONTEXT_MENU_ROW_HEIGHT,
        divided,
        kind,
        color_theme::Level::One,
        |ui, rect, foreground| {
            let icon_center = egui::pos2(
                rect.left() + CONTEXT_MENU_INLINE_PADDING + ICON_SIZE / 2.0,
                rect.center().y,
            );
            icons::Props {
                icon,
                size: ICON_SIZE,
                color: foreground,
            }
            .paint_at(ui, icon_center);
            ui.painter().text(
                egui::pos2(
                    icon_center.x + ICON_SIZE / 2.0 + CONTEXT_MENU_ICON_GAP,
                    rect.center().y,
                ),
                Align2::LEFT_CENTER,
                label,
                egui::TextStyle::Button.resolve(ui.style()),
                color32(foreground),
            );
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response.clicked()
}

fn prepare_entry_cell(ui: &mut Ui) {
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.add_space(ENTRY_INLINE_PADDING);
}

fn show_table_size(ui: &mut Ui, bytes: Option<u64>) {
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(ENTRY_INLINE_PADDING);
        let Some(bytes) = bytes else {
            return;
        };
        let (value, unit) = format_byte_parts(bytes);
        ui.allocate_ui_with_layout(
            egui::vec2(SIZE_UNIT_SLOT_WIDTH, ui.available_height()),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add_space(SIZE_UNIT_GAP);
                ui.label(unit);
            },
        );
        ui.label(value);
    });
}

fn moved_row_index(
    rows: &[DeviceCatalogEntry],
    selected_path: Option<&str>,
    movement: RowMove,
) -> usize {
    let selected_index =
        selected_path.and_then(|path| rows.iter().position(|entry| entry.path == path));
    match (movement, selected_index) {
        (RowMove::Previous, Some(index)) => index.saturating_sub(1),
        (RowMove::Next, Some(index)) => (index + 1).min(rows.len() - 1),
        (RowMove::Previous, None) => rows.len() - 1,
        (RowMove::Next, None) => 0,
    }
}

fn compare_entries(left: &DeviceCatalogEntry, right: &DeviceCatalogEntry) -> std::cmp::Ordering {
    match (left.kind, right.kind) {
        (DeviceCatalogEntryKind::Directory, DeviceCatalogEntryKind::File) => {
            std::cmp::Ordering::Less
        }
        (DeviceCatalogEntryKind::File, DeviceCatalogEntryKind::Directory) => {
            std::cmp::Ordering::Greater
        }
        _ => left
            .path
            .as_str()
            .to_lowercase()
            .cmp(&right.path.as_str().to_lowercase()),
    }
}

fn populate_directory_sizes(catalog: &mut DeviceCatalogSnapshot) {
    for storage in &mut catalog.storages {
        let mut sizes = HashMap::<Utf8PathBuf, Option<u64>>::new();
        for entry in &storage.entries {
            if entry.kind != DeviceCatalogEntryKind::File {
                continue;
            }
            let mut directory = parent_path(&entry.path);
            while !directory.as_str().is_empty() {
                sizes
                    .entry(directory.to_owned())
                    .and_modify(|total| {
                        *total = match (*total, entry.size) {
                            (Some(total), Some(size)) => total.checked_add(size),
                            _ => None,
                        };
                    })
                    .or_insert(entry.size);
                directory = parent_path(directory);
            }
        }
        for entry in &mut storage.entries {
            if entry.kind == DeviceCatalogEntryKind::Directory {
                entry.size = sizes.get(&entry.path).copied().unwrap_or(Some(0));
            }
        }
    }
}

fn validate_catalog(catalog: &DeviceCatalogSnapshot) -> Result<(), String> {
    if catalog.storages.is_empty() {
        return Err("the device did not expose any browsable storage".to_owned());
    }
    let mut storage_ids = HashSet::new();
    for storage in &catalog.storages {
        if storage.id.is_empty() || !storage_ids.insert(storage.id.clone()) {
            return Err("the device catalog contains an invalid storage identifier".to_owned());
        }
        let mut paths = HashSet::new();
        let directories = storage
            .entries
            .iter()
            .filter(|entry| entry.kind == DeviceCatalogEntryKind::Directory)
            .map(|entry| entry.path.as_str())
            .collect::<HashSet<_>>();
        for entry in &storage.entries {
            validate_path(&entry.path)?;
            if !paths.insert(entry.path.as_str().to_ascii_lowercase()) {
                return Err(format!(
                    "device storage contains duplicate path {}",
                    entry.path
                ));
            }
            let parent = parent_path(&entry.path);
            if !parent.as_str().is_empty() && !directories.contains(parent.as_str()) {
                return Err(format!(
                    "device entry {} has no catalogued parent",
                    entry.path
                ));
            }
        }
    }
    Ok(())
}

fn validate_new_directory_name(
    name: &str,
    catalog: &DeviceCatalogSnapshot,
    parent: &Selection,
) -> Option<DirectoryNameError> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
    {
        return Some(DirectoryNameError::Unsafe);
    }
    let path = parent.path.join(name);
    let duplicate = catalog
        .storages
        .iter()
        .find(|storage| storage.id == parent.storage_id)
        .is_some_and(|storage| {
            storage
                .entries
                .iter()
                .any(|entry| entry.path.as_str().eq_ignore_ascii_case(path.as_str()))
        });
    duplicate.then_some(DirectoryNameError::Duplicate)
}

fn validate_path(path: &Utf8Path) -> Result<(), String> {
    let text = path.as_str();
    let valid = !text.is_empty()
        && !text.starts_with('/')
        && !text.contains(['\\', ':'])
        && path
            .as_str()
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..");
    if valid {
        Ok(())
    } else {
        Err(format!("device catalog contains unsafe path {path:?}"))
    }
}

fn parent_path(path: &Utf8Path) -> &Utf8Path {
    path.parent().unwrap_or_else(|| Utf8Path::new(""))
}

fn path_is_toolkit_managed(path: &Utf8Path) -> bool {
    path.components()
        .next()
        .is_some_and(|component| component.as_str().eq_ignore_ascii_case("GARMIN-TOOLKIT"))
}

fn file_name(path: &Utf8Path) -> &str {
    path.file_name().unwrap_or(path.as_str())
}

fn format_bytes(bytes: u64) -> String {
    let (value, unit) = format_byte_parts(bytes);
    format!("{value} {unit}")
}

fn format_byte_parts(bytes: u64) -> (String, &'static str) {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        (format_scaled_bytes(bytes, GIB), "GiB")
    } else if bytes >= MIB {
        (format_scaled_bytes(bytes, MIB), "MiB")
    } else if bytes >= KIB {
        (format_scaled_bytes(bytes, KIB), "KiB")
    } else {
        (bytes.to_string(), "B")
    }
}

fn format_scaled_bytes(bytes: u64, unit_bytes: u64) -> String {
    let tenths = (u128::from(bytes) * 10 + u128::from(unit_bytes) / 2) / u128::from(unit_bytes);
    let whole = tenths / 10;
    let fraction = tenths % 10;
    format!("{whole}.{fraction}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(entries: Vec<DeviceCatalogEntry>) -> DeviceCatalogSnapshot {
        DeviceCatalogSnapshot {
            device_key: "mock:device".to_owned(),
            storages: vec![DeviceCatalogStorage {
                id: "internal".to_owned(),
                label: "Internal storage".to_owned(),
                entries,
            }],
        }
    }

    #[test]
    fn centers_the_default_window_with_a_uniform_viewport_inset() {
        let bounds = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1_280.0, 720.0));

        let geometry = explorer_window_geometry(bounds);

        assert_eq!(geometry.default_size, egui::vec2(960.0, 640.0));
        assert_eq!(geometry.min_size, egui::vec2(480.0, 360.0));
        assert_eq!(geometry.default_position, egui::pos2(170.0, 60.0));
    }

    #[test]
    fn narrow_viewport_opens_a_compact_window_inside_the_uniform_inset() {
        let bounds = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 640.0));

        let geometry = explorer_window_geometry(bounds);

        assert_eq!(geometry.default_size, egui::vec2(592.0, 592.0));
        assert_eq!(geometry.default_position, egui::pos2(24.0, 24.0));
        assert!(uses_compact_layout(geometry.default_size.x));
    }

    #[test]
    fn custom_title_drag_stays_inside_the_explorer_bounds() {
        let bounds = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 600.0));
        let window = Rect::from_min_size(egui::pos2(100.0, 120.0), egui::vec2(480.0, 360.0));

        assert_eq!(
            constrained_window_position(window, bounds, egui::vec2(-200.0, -200.0)),
            bounds.min
        );
        assert_eq!(
            constrained_window_position(window, bounds, egui::vec2(1_000.0, 1_000.0)),
            egui::pos2(330.0, 260.0)
        );
    }

    #[test]
    fn rejects_unsafe_paths() {
        let value = catalog(vec![DeviceCatalogEntry {
            path: "Garmin/../secrets".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(1),
        }]);
        assert!(validate_catalog(&value).is_err());
    }

    #[test]
    fn rejects_missing_parent_directories() {
        let value = catalog(vec![DeviceCatalogEntry {
            path: "Garmin/Activity/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(1),
        }]);
        assert!(validate_catalog(&value).is_err());
    }

    #[test]
    fn rejects_case_insensitive_duplicate_paths() {
        let value = catalog(vec![
            DeviceCatalogEntry {
                path: "Garmin".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "GARMIN".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
        ]);

        let error = validate_catalog(&value).expect_err("duplicate paths must be rejected");
        assert!(error.contains("duplicate path"));
    }

    #[test]
    fn derives_recursive_directory_sizes_from_the_bounded_catalog() {
        let mut value = catalog(vec![
            DeviceCatalogEntry {
                path: "Garmin".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Garmin/Activities".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Garmin/Activities/2026".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Garmin/Activities/2026/ride.fit".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(1_024),
            },
            DeviceCatalogEntry {
                path: "Garmin/Activities/summary.fit".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(512),
            },
            DeviceCatalogEntry {
                path: "Garmin/Empty".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
        ]);

        populate_directory_sizes(&mut value);

        let size = |path: &str| {
            value.storages[0]
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .and_then(|entry| entry.size)
        };
        assert_eq!(size("Garmin"), Some(1_536));
        assert_eq!(size("Garmin/Activities"), Some(1_536));
        assert_eq!(size("Garmin/Activities/2026"), Some(1_024));
        assert_eq!(size("Garmin/Empty"), Some(0));
        assert_eq!(size("Garmin/Activities/2026/ride.fit"), Some(1_024));
    }

    #[test]
    fn leaves_a_directory_size_unknown_when_a_descendant_is_unknown_or_overflows() {
        let mut value = catalog(vec![
            DeviceCatalogEntry {
                path: "Unknown".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Unknown/file.bin".into(),
                kind: DeviceCatalogEntryKind::File,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Overflow".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Overflow/large.bin".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(u64::MAX),
            },
            DeviceCatalogEntry {
                path: "Overflow/extra.bin".into(),
                kind: DeviceCatalogEntryKind::File,
                size: Some(1),
            },
        ]);

        populate_directory_sizes(&mut value);

        for path in ["Unknown", "Overflow"] {
            assert_eq!(
                value.storages[0]
                    .entries
                    .iter()
                    .find(|entry| entry.path == path)
                    .and_then(|entry| entry.size),
                None
            );
        }
    }

    #[test]
    fn opens_at_a_valid_nested_directory() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let mut browser = Browser::open_directory(
            catalog(vec![
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
            ]),
            &intl,
            "Mock Watch-o-Matic 9000",
            "internal",
            "Garmin/Activity/History",
        )
        .expect("the nested directory is catalogued");

        assert_eq!(browser.current.storage_index, 0);
        assert_eq!(browser.current.path, "Garmin/Activity/History");
        assert_eq!(browser.back_stack.len(), 1);
        assert!(browser.forward_stack.is_empty());
        assert_eq!(
            browser.current_upload_target(),
            Some(("internal".to_owned(), "Garmin/Activity/History".into()))
        );

        browser.navigate_back();
        assert_eq!(browser.current.path, "");
        assert!(browser.back_stack.is_empty());
        assert_eq!(browser.forward_stack.len(), 1);

        browser.navigate_forward();
        assert_eq!(browser.current.path, "Garmin/Activity/History");
        assert_eq!(browser.back_stack.len(), 1);
        assert!(browser.forward_stack.is_empty());
    }

    #[test]
    fn navigation_switches_storage_without_losing_storage_identity() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let mut browser = Browser::open(
            DeviceCatalogSnapshot {
                device_key: "mock:device".to_owned(),
                storages: vec![
                    DeviceCatalogStorage {
                        id: "internal".to_owned(),
                        label: "Internal storage".to_owned(),
                        entries: Vec::new(),
                    },
                    DeviceCatalogStorage {
                        id: "card".to_owned(),
                        label: "Memory card".to_owned(),
                        entries: Vec::new(),
                    },
                ],
            },
            &intl,
            "Mock Watch-o-Matic 9000",
        )
        .expect("the two-storage catalog is valid");

        browser.navigate(&DirectoryId {
            storage_index: 1,
            path: Utf8PathBuf::new(),
        });

        assert_eq!(browser.current.storage_index, 1);
        assert_eq!(browser.back_stack.len(), 1);
        assert_eq!(
            browser.current_upload_target(),
            Some(("card".to_owned(), Utf8PathBuf::new()))
        );
    }

    #[test]
    fn refresh_falls_back_to_the_nearest_surviving_directory() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let mut browser = Browser::open_directory(
            catalog(vec![
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
            ]),
            &intl,
            "Mock Watch-o-Matic 9000",
            "internal",
            "Garmin/Activity",
        )
        .unwrap();

        browser
            .refresh(catalog(vec![DeviceCatalogEntry {
                path: "Garmin".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            }]))
            .unwrap();

        assert_eq!(browser.current.path, "Garmin");
        assert!(browser.back_stack.is_empty());
        assert!(browser.forward_stack.is_empty());
    }

    #[test]
    fn refresh_updates_the_selected_directory_size() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let sized_catalog = |size| {
            catalog(vec![
                DeviceCatalogEntry {
                    path: "Garmin".into(),
                    kind: DeviceCatalogEntryKind::Directory,
                    size: None,
                },
                DeviceCatalogEntry {
                    path: "Garmin/file.bin".into(),
                    kind: DeviceCatalogEntryKind::File,
                    size: Some(size),
                },
            ])
        };
        let mut browser = Browser::open(sized_catalog(1), &intl, "Mock Watch-o-Matic 9000")
            .expect("the catalog is valid");
        browser.selected = Some(Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: "Garmin".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: Some(1),
        });

        browser
            .refresh(sized_catalog(2))
            .expect("the refreshed catalog is valid");

        assert_eq!(
            browser
                .selected
                .as_ref()
                .and_then(|selection| selection.size),
            Some(2)
        );
    }

    #[test]
    fn pane_headers_share_one_content_block() {
        let outer = Rect::from_min_size(
            egui::pos2(10.0, 20.0),
            egui::vec2(200.0, PANE_HEADER_HEIGHT),
        );
        let content = pane_header_content_rect(outer);
        let path = pane_header_block_rect(outer);
        let close = pane_close_button_rect(outer);

        assert!((content.top() - 26.0).abs() < f32::EPSILON);
        assert!((content.bottom() - 54.0).abs() < f32::EPSILON);
        assert!((content.height() - BREADCRUMB_HEIGHT).abs() < f32::EPSILON);
        assert!((content.left() - 22.0).abs() < f32::EPSILON);
        assert!((content.right() - 198.0).abs() < f32::EPSILON);
        assert_eq!(content.y_range(), path.y_range());
        assert!((path.left() - 10.0).abs() < f32::EPSILON);
        assert!((path.right() - 210.0).abs() < f32::EPSILON);
        assert!((close.top() - outer.top()).abs() < f32::EPSILON);
        assert!((close.right() - outer.right()).abs() < f32::EPSILON);
        assert!((close.width() - PANE_CLOSE_BUTTON_SIZE).abs() < f32::EPSILON);
        assert!((close.height() - PANE_CLOSE_BUTTON_SIZE).abs() < f32::EPSILON);
    }

    #[test]
    fn narrow_browser_uses_the_compact_pane_layout() {
        assert!(uses_compact_layout(COMPACT_LAYOUT_WIDTH - 1.0));
        assert!(!uses_compact_layout(COMPACT_LAYOUT_WIDTH));
    }

    #[test]
    fn detail_actions_are_compact_and_bottom_aligned() {
        let available = Rect::from_min_size(egui::pos2(12.0, 40.0), egui::vec2(224.0, 500.0));
        let layout = details_content_layout(available, 4);
        let actions = layout.actions.expect("four actions create a footer");

        assert!((actions.bottom() - (available.bottom() - 12.0)).abs() < f32::EPSILON);
        assert!((actions.height() - 32.0).abs() < f32::EPSILON);
        assert!((actions.top() - layout.summary.bottom() - 8.0).abs() < f32::EPSILON);
        assert_eq!(details_content_layout(available, 0).actions, None);
    }

    #[test]
    fn recognises_fit_files_case_insensitively() {
        let selection = Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: "Garmin/Activity/RIDE.FIT".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(1),
        };
        assert!(selection.is_fit_file());
    }

    #[test]
    fn contextual_and_visible_controls_share_item_capabilities() {
        let selection = |path: &str, kind| Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: path.into(),
            kind,
            size: None,
        };

        assert_eq!(
            available_item_actions(
                &selection("Garmin/Activity/ride.fit", DeviceCatalogEntryKind::File),
                true,
            ),
            vec![
                ItemAction::Open,
                ItemAction::ImportFit,
                ItemAction::Download,
                ItemAction::Remove,
            ]
        );
        assert_eq!(
            available_item_actions(
                &selection("Garmin/Activity", DeviceCatalogEntryKind::Directory),
                true,
            ),
            vec![
                ItemAction::Download,
                ItemAction::CreateDirectory,
                ItemAction::Remove,
            ]
        );
        assert_eq!(
            available_item_actions(&selection("", DeviceCatalogEntryKind::Directory), false),
            vec![ItemAction::Download, ItemAction::CreateDirectory]
        );
        assert_eq!(
            available_item_actions(
                &selection(
                    "GARMIN-TOOLKIT/transactions",
                    DeviceCatalogEntryKind::Directory,
                ),
                true,
            ),
            vec![ItemAction::Download]
        );
    }

    #[test]
    fn fit_activation_opens_while_import_remains_explicit() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let mut browser = Browser::open(catalog(Vec::new()), &intl, "Mock Watch-o-Matic 9000")
            .expect("the empty catalog is valid");
        let selection = Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: "Garmin/Activity/ride.fit".into(),
            kind: DeviceCatalogEntryKind::File,
            size: Some(42),
        };

        assert_eq!(activated_item_action(&selection), Some(ItemAction::Open));
        assert_eq!(
            browser.handle_item_action(ItemAction::Open, selection.clone()),
            Some(Action::Open(selection.clone()))
        );
        assert_eq!(
            browser.handle_item_action(ItemAction::ImportFit, selection.clone()),
            Some(Action::ImportFit(selection))
        );
    }

    #[test]
    fn item_action_routing_preserves_targets_and_dialog_guards() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let mut browser = Browser::open(catalog(Vec::new()), &intl, "Mock Watch-o-Matic 9000")
            .expect("the empty catalog is valid");
        let directory = Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: "Garmin/Activity".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        };

        assert_eq!(
            browser.handle_item_action(ItemAction::Download, directory.clone()),
            Some(Action::Download(directory.clone()))
        );
        assert_eq!(
            browser.handle_item_action(ItemAction::CreateDirectory, directory.clone()),
            None
        );
        assert_eq!(
            browser
                .create_directory
                .as_ref()
                .map(|dialog| &dialog.parent),
            Some(&directory)
        );
        assert!(
            browser
                .create_directory
                .as_ref()
                .is_some_and(|dialog| dialog.focus_name)
        );
        assert_eq!(
            browser.handle_item_action(ItemAction::Remove, directory.clone()),
            None
        );
        assert_eq!(browser.pending_removal.as_ref(), Some(&directory));
        assert!(browser.create_directory.is_none());
    }

    #[test]
    fn new_folder_dialog_focuses_its_name_field_once() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let mut browser = Browser::open(catalog(Vec::new()), &intl, "Mock Watch-o-Matic 9000")
            .expect("the empty catalog is valid");
        let directory = Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: Utf8PathBuf::new(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        };
        assert_eq!(
            browser.handle_item_action(ItemAction::CreateDirectory, directory),
            None
        );

        let context = egui::Context::default();
        crate::install(&context);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..egui::RawInput::default()
        };
        context
            .run_ui(input, |ui| {
                assert_eq!(browser.show_create_directory_dialog(ui, &intl), None);
            })
            .drop_without_applying_deltas();

        assert!(context.memory(|memory| memory.focused().is_some()));
        assert!(
            browser
                .create_directory
                .as_ref()
                .is_some_and(|dialog| !dialog.focus_name)
        );
    }

    #[test]
    fn shift_f10_requests_the_focused_table_context_menu() {
        let context = egui::Context::default();
        crate::install(&context);
        let table_id = Id::new("context-menu-test-table");
        context.memory_mut(|memory| memory.request_focus(table_id));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(320.0, 200.0),
            )),
            events: vec![egui::Event::Key {
                key: Key::F10,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
            ..egui::RawInput::default()
        };
        let mut requested = false;

        context
            .run_ui(input, |ui| {
                requested = requested_context_menu(ui, table_id);
            })
            .drop_without_applying_deltas();

        assert!(requested);
    }

    #[test]
    fn validates_new_folder_names_within_the_target_directory() {
        let value = catalog(vec![DeviceCatalogEntry {
            path: "Garmin".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        }]);
        let parent = Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: Utf8PathBuf::new(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        };

        assert_eq!(validate_new_directory_name("", &value, &parent), None);
        assert_eq!(
            validate_new_directory_name("New folder", &value, &parent),
            None
        );
        assert!(validate_new_directory_name("../escape", &value, &parent).is_some());
        assert!(validate_new_directory_name("C:escape", &value, &parent).is_some());
        assert!(validate_new_directory_name("garmin", &value, &parent).is_some());
    }

    #[test]
    fn bookmarks_are_discovered_only_for_known_directories_that_exist() {
        let value = catalog(vec![
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
                path: "Unrelated".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
        ]);

        let bookmarks = discover_bookmarks(&value);
        assert_eq!(
            bookmarks
                .iter()
                .map(|bookmark| bookmark.kind)
                .collect::<Vec<_>>(),
            vec![BookmarkKind::Activities, BookmarkKind::Podcasts]
        );
    }

    #[test]
    fn toolkit_namespace_is_browse_only_in_the_explorer() {
        assert!(path_is_toolkit_managed(Utf8Path::new("GARMIN-TOOLKIT")));
        assert!(path_is_toolkit_managed(Utf8Path::new(
            "garmin-toolkit/transactions/active.json"
        )));
        assert!(!path_is_toolkit_managed(Utf8Path::new("Garmin/Activity")));

        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let browser = Browser::open_directory(
            catalog(vec![DeviceCatalogEntry {
                path: "GARMIN-TOOLKIT".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            }]),
            &intl,
            "Mock Watch-o-Matic 9000",
            "internal",
            "GARMIN-TOOLKIT",
        )
        .expect("the toolkit namespace is catalogued");

        assert_eq!(browser.current_upload_target(), None);
    }

    #[test]
    fn directory_download_keeps_the_directory_kind_for_host_archiving() {
        let selection = Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: "Garmin/Activity".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        };

        let action = Action::Download(selection.clone());
        assert_eq!(action, Action::Download(selection));
        let Action::Download(selection) = action else {
            panic!("the action must remain a download");
        };
        assert_eq!(selection.kind, DeviceCatalogEntryKind::Directory);
    }

    #[test]
    fn formats_binary_file_sizes_without_lossy_float_casts() {
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn moves_table_selection_between_visible_rows() {
        let rows = vec![
            DeviceCatalogEntry {
                path: "Garmin/Activity".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
            DeviceCatalogEntry {
                path: "Garmin/Courses".into(),
                kind: DeviceCatalogEntryKind::Directory,
                size: None,
            },
        ];

        assert_eq!(moved_row_index(&rows, None, RowMove::Next), 0);
        assert_eq!(moved_row_index(&rows, None, RowMove::Previous), 1);
        assert_eq!(
            moved_row_index(&rows, Some("Garmin/Activity"), RowMove::Next),
            1
        );
        assert_eq!(
            moved_row_index(&rows, Some("Garmin/Courses"), RowMove::Previous),
            0
        );
    }

    #[test]
    fn arrow_down_moves_the_focused_table_selection() {
        let translations = garmin_i18n::Translations::bundled().expect("catalogs are valid");
        let intl = translations
            .formatter(garmin_i18n::Language::English)
            .expect("English locale is valid");
        let mut browser = Browser::open(
            catalog(vec![
                DeviceCatalogEntry {
                    path: "Activity".into(),
                    kind: DeviceCatalogEntryKind::Directory,
                    size: None,
                },
                DeviceCatalogEntry {
                    path: "Courses".into(),
                    kind: DeviceCatalogEntryKind::Directory,
                    size: None,
                },
            ]),
            &intl,
            "Mock Watch-o-Matic 9000",
        )
        .expect("test catalog is valid");
        browser.selected = Some(Selection {
            storage_id: "internal".to_owned(),
            storage_label: "Internal storage".to_owned(),
            path: "Activity".into(),
            kind: DeviceCatalogEntryKind::Directory,
            size: None,
        });

        let context = egui::Context::default();
        crate::install(&context);
        context.memory_mut(|memory| {
            memory.request_focus(Id::new((
                "device-explorer-entry-table",
                browser.device_key(),
            )));
        });
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 640.0),
            )),
            events: vec![egui::Event::Key {
                key: Key::ArrowDown,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..egui::RawInput::default()
        };
        context
            .run_ui(input, |ui| {
                let _ = browser.show(ui, &intl);
            })
            .drop_without_applying_deltas();

        assert_eq!(
            browser
                .selected
                .as_ref()
                .map(|selection| selection.path.as_str()),
            Some("Courses")
        );
    }
}
