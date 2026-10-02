//! Open/save selection using the explorer's tree, breadcrumbs, history, and file table.

use super::{
    Browser, DeviceCatalogEntry, DeviceCatalogEntryKind, DeviceCatalogSnapshot,
    DeviceCatalogStorage, DirectoryId, EntryInteraction, Id, Intl, Selection, Size, TREE_WIDTH, Ui,
    Utf8PathBuf, button, compare_entries, file_name, format_message, icons, input, modal,
    moved_row_index, parent_path, requested_table_movement, show_entry_table,
};
use garmin_service_api::files::{Directory, Operation};

#[derive(serde::Deserialize, serde::Serialize)]
pub enum Action {
    Cancel,
    ShowHiddenFiles(bool),
    Navigate(String),
    Select { path: String, replace: bool },
}

pub struct Chooser {
    browser: Browser,
    operation: Operation,
    name: String,
    loaded: Option<Utf8PathBuf>,
    problem: Option<String>,
    overwrite: Option<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ViewState {
    browser: super::ViewState,
    name: String,
    overwrite: Option<String>,
}

impl Chooser {
    /// Focus manual folder entry on the next frame.
    pub fn edit_location(&mut self) {
        self.browser.edit_location();
    }

    #[must_use]
    pub fn view_state(&self) -> ViewState {
        ViewState {
            browser: self.browser.view_state(),
            name: self.name.clone(),
            overwrite: self.overwrite.clone(),
        }
    }

    pub fn restore_view(&mut self, view: ViewState) {
        self.browser.restore_view(view.browser);
        self.name = view.name;
        self.overwrite = view.overwrite;
    }

    pub fn set_show_hidden_files(&mut self, show: bool) {
        self.browser.set_show_hidden_files(show);
    }

    #[must_use]
    pub fn new(operation: Operation, name: String) -> Self {
        let mut chooser = Self {
            browser: Browser::from_catalog(
                DeviceCatalogSnapshot {
                    device_key: "server-file-chooser".into(),
                    storages: vec![DeviceCatalogStorage {
                        id: "server".into(),
                        label: "/".into(),
                        entries: Vec::new(),
                    }],
                },
                "",
            ),
            operation,
            name,
            loaded: None,
            problem: None,
            overwrite: None,
        };
        chooser.browser.remote_directories = true;
        chooser
    }

    /// Applies one directory response. Ancestors and previously visited branches remain navigable.
    pub fn loaded(&mut self, directory: Directory) {
        let path = Utf8PathBuf::from(directory.path.trim_start_matches('/'));
        let entries = &mut self.browser.catalog.storages[0].entries;
        entries.retain(|entry| !entry.path.starts_with(&path) || entry.path == path);
        for ancestor in path.ancestors().filter(|path| !path.as_str().is_empty()) {
            if !entries.iter().any(|entry| entry.path == ancestor) {
                entries.push(DeviceCatalogEntry {
                    path: ancestor.into(),
                    kind: DeviceCatalogEntryKind::Directory,
                    size: None,
                });
            }
        }
        entries.extend(directory.entries.into_iter().map(|mut entry| {
            entry.path = path.join(entry.path);
            entry
        }));
        entries.sort_by(compare_entries);
        self.browser.set_current(DirectoryId {
            storage_index: 0,
            path: path.clone(),
        });
        self.loaded = Some(path);
        self.problem = None;
    }

    pub fn failed(&mut self, problem: String) {
        self.problem = Some(problem);
        self.loaded = None;
    }

    #[must_use]
    pub fn show(&mut self, ui: &mut Ui, intl: &Intl, busy: bool) -> Option<Action> {
        let title = title(intl, self.operation);
        let mut action = None;
        let mut drag_delta = egui::Vec2::ZERO;
        let bounds = ui.max_rect().intersect(ui.ctx().content_rect());
        let style = ui.style().clone();
        let close = format_message!(intl, default_message: "Close window");
        let mut window = egui::Window::new(&title)
            .frame(crate::window::frame(ui))
            .enabled(ui.is_enabled())
            .title_bar(false)
            .drag_area(egui::WindowDrag::Off)
            .id(Id::new("server-file-chooser"))
            .collapsible(false)
            .default_size(egui::vec2(880.0, 560.0))
            .min_size(egui::vec2(288.0, 320.0))
            .max_size(bounds.size())
            .constrain_to(bounds);
        if let Some(position) = self.browser.window_position {
            window = window.current_pos(position);
        }
        if let Some(response) = window.show(ui.ctx(), |ui| {
            ui.set_style(style);
            match crate::shell::dialog_header(ui, &title, &close) {
                Some(crate::shell::WindowAction::Close) => action = Some(Action::Cancel),
                Some(crate::shell::WindowAction::Drag) => {
                    drag_delta = ui.input(|input| input.pointer.delta());
                }
                _ => {}
            }
            action = action.take().or_else(|| self.show_body(ui, intl, busy));
        }) {
            self.browser.window_position = Some(super::constrained_window_position(
                response.response.rect,
                bounds,
                drag_delta,
            ));
        }
        if self.overwrite.is_some() {
            return self.confirm_overwrite(ui, intl).or(action);
        }
        action
    }

    /// Render inside a platform-owned window, without an embedded window frame.
    #[must_use]
    pub fn contents(&mut self, ui: &mut Ui, intl: &Intl, busy: bool) -> Option<Action> {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let action = self.show_body(ui, intl, busy);
        if self.overwrite.is_some() {
            self.confirm_overwrite(ui, intl).or(action)
        } else {
            action
        }
    }

    fn show_body(&mut self, ui: &mut Ui, intl: &Intl, busy: bool) -> Option<Action> {
        let before = self.browser.current.clone();
        let mut action = None;
        let stacked = ui.available_width() < 640.0;
        egui::Panel::bottom("chooser-actions")
            .resizable(false)
            .exact_size(if stacked { 104.0 } else { 56.0 })
            .frame(egui::Frame::NONE.inner_margin(8))
            .show(ui, |ui| {
                action = self.show_actions(ui, intl, busy, stacked);
            });
        let browser_bounds = ui.available_rect_before_wrap();
        ui.add_enabled_ui(!busy, |ui| {
            if ui.available_width() >= 640.0 {
                egui::Panel::left("chooser-tree")
                    .default_size(TREE_WIDTH)
                    .resizable(true)
                    .show(ui, |ui| {
                        self.browser.show_tree(ui, intl, None, None, None);
                    });
            }
            egui::CentralPanel::default().show(ui, |ui| {
                match self.browser.show_breadcrumbs(ui, intl, false, None, None) {
                    Some(super::Action::Refresh) => {
                        action = Some(Action::Navigate(self.current_path()));
                    }
                    Some(super::Action::ShowHiddenFiles(show)) => {
                        action = Some(Action::ShowHiddenFiles(show));
                    }
                    _ => {}
                }
                if let Some(problem) = &self.problem {
                    ui.label(problem);
                }
                if self.loaded.as_ref() == Some(&self.browser.current.path) {
                    action = self.show_files(ui, intl).or(action.take());
                }
            });
        });
        if busy {
            show_loading(ui, intl, browser_bounds);
        }
        if before != self.browser.current {
            self.problem = None;
            self.overwrite = None;
            return Some(Action::Navigate(self.current_path()));
        }
        action
    }

    fn show_files(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        let rows = self.browser.catalog.storages[0]
            .entries
            .iter()
            .filter(|entry| parent_path(&entry.path) == self.browser.current.path)
            .filter(|entry| self.browser.show_hidden_files || !super::is_hidden(&entry.path))
            .filter(|entry| {
                entry.kind == DeviceCatalogEntryKind::Directory
                    || entry.path.as_str().ends_with(".tar.zst")
            })
            .cloned()
            .collect::<Vec<_>>();
        let selected = self
            .browser
            .selected
            .as_ref()
            .map(|value| value.path.as_str());
        let table_id = Id::new(("device-explorer-entry-table", self.browser.device_key()));
        let _ = super::entry_table_background(ui, intl, self.browser.device_key());
        let moved = requested_table_movement(ui, table_id);
        let index = (!rows.is_empty())
            .then(|| moved.map(|direction| moved_row_index(&rows, selected, direction)))
            .flatten();
        let interaction = show_entry_table(
            ui,
            intl,
            self.browser.device_key(),
            &rows,
            selected,
            index,
            None,
        );
        if let Some(EntryInteraction::Select { index, activate }) = interaction {
            let entry = &rows[index];
            self.browser.selected = Some(Selection {
                storage_id: "server".into(),
                storage_label: "/".into(),
                path: entry.path.clone(),
                kind: entry.kind,
                size: entry.size,
            });
            if entry.kind == DeviceCatalogEntryKind::Directory {
                if activate {
                    self.browser.navigate(&DirectoryId {
                        storage_index: 0,
                        path: entry.path.clone(),
                    });
                }
            } else {
                self.name = file_name(&entry.path).into();
                if activate {
                    return self.select();
                }
            }
        }
        None
    }

    fn current_path(&self) -> String {
        format!("/{}", self.browser.current.path)
    }

    fn show_actions(
        &mut self,
        ui: &mut Ui,
        intl: &Intl,
        busy: bool,
        stacked: bool,
    ) -> Option<Action> {
        let mut action = None;
        ui.spacing_mut().item_spacing.y = 8.0;
        if stacked {
            self.show_filename(ui, intl, busy);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let button_width = stacked.then(|| (ui.available_width() - 8.0) / 2.0);
            let (label, icon) = match self.operation {
                Operation::Open => (
                    format_message!(intl, default_message: "Open"),
                    icons::FOLDER_OPEN,
                ),
                Operation::Save => (
                    format_message!(intl, default_message: "Save"),
                    icons::FLOPPY_DISK,
                ),
            };
            let confirm = chooser_button(
                ui,
                &label,
                icon,
                button::Kind::Primary,
                !busy && self.valid_selection(),
                button_width,
            );
            crate::semantics::target(ui, &confirm, "files.chooser.confirm");
            if confirm.clicked() {
                action = self.select();
            }
            let cancel = format_message!(intl, default_message: "Cancel");
            let cancel = chooser_button(
                ui,
                &cancel,
                icons::X,
                button::Kind::Secondary,
                true,
                button_width,
            );
            crate::semantics::target(ui, &cancel, "files.chooser.cancel");
            if cancel.clicked() {
                action = Some(Action::Cancel);
            }
            if !stacked {
                self.show_filename(ui, intl, busy);
            }
        });
        action
    }

    fn show_filename(&mut self, ui: &mut Ui, intl: &Intl, busy: bool) {
        let label = format_message!(intl, default_message: "File name");
        let previous = self.name.clone();
        let response = input::show(
            ui,
            &mut self.name,
            input::Props::new("")
                .placeholder(&label)
                .disabled(busy)
                .subtle_border(true),
        );
        response.widget_info(|| egui::WidgetInfo::text_edit(!busy, &previous, &self.name, &label));
        crate::semantics::target(ui, &response, "files.chooser.name");
        if self.name != previous {
            self.overwrite = None;
        }
    }

    fn valid_selection(&self) -> bool {
        self.loaded.as_ref() == Some(&self.browser.current.path)
            && !self.name.contains(['/', '\0'])
            && self.name.ends_with(".tar.zst")
            && (self.operation == Operation::Save || self.existing())
    }

    fn existing(&self) -> bool {
        let path = self.browser.current.path.join(&self.name);
        self.browser.catalog.storages[0]
            .entries
            .iter()
            .any(|entry| entry.path == path && entry.kind == DeviceCatalogEntryKind::File)
    }

    fn select(&mut self) -> Option<Action> {
        if !self.valid_selection() {
            return None;
        }
        let path = format!("/{}", self.browser.current.path.join(&self.name));
        if self.operation == Operation::Save && self.existing() {
            self.overwrite = Some(path);
            None
        } else {
            Some(Action::Select {
                path,
                replace: false,
            })
        }
    }

    fn confirm_overwrite(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Action> {
        let title = format_message!(intl, default_message: "Replace existing backup?");
        let cancel = format_message!(intl, default_message: "Cancel");
        let replace = format_message!(intl, default_message: "Replace");
        let output = modal::show(
            ui,
            Id::new("chooser-overwrite"),
            &modal::Props {
                title: &title,
                description: self.overwrite.as_deref(),
                size: modal::Size::Medium,
                presentation: modal::Presentation::Modal,
                cancel_label: Some(&cancel),
                backdrop_closes: Some(false),
                primary: modal::Primary {
                    label: &replace,
                    icon: Some(icons::FLOPPY_DISK),
                    kind: modal::PrimaryKind::Danger,
                    enabled: true,
                },
            },
            |_| (),
        );
        crate::semantics::target(ui, &output.primary, "files.chooser.replace");
        if let Some(cancel) = &output.cancel {
            crate::semantics::target(ui, cancel, "files.chooser.keep");
        }
        match output.action {
            Some(modal::Action::Primary) => self.overwrite.take().map(|path| Action::Select {
                path,
                replace: true,
            }),
            Some(modal::Action::Cancel) => {
                self.overwrite = None;
                None
            }
            None => None,
        }
    }
}

#[must_use]
pub fn title(intl: &Intl, operation: Operation) -> String {
    match operation {
        Operation::Open => format_message!(intl, default_message: "Open backup on server"),
        Operation::Save => format_message!(intl, default_message: "Save backup on server"),
    }
}

fn show_loading(ui: &mut Ui, intl: &Intl, bounds: egui::Rect) {
    let palette = crate::theme::palette(ui);
    let color = crate::theme::color32;
    let foreground = color(palette.content().text_primary());
    ui.painter()
        .rect_filled(bounds, 0.0, color(palette.overlay()));
    ui.interact(
        bounds,
        ui.id().with("loading-files"),
        egui::Sense::click_and_drag(),
    );
    let content_bounds = egui::Rect::from_center_size(
        bounds.center(),
        egui::vec2(192.0_f32.min(bounds.width() - 32.0), 64.0),
    );
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(content_bounds)
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    content.spacing_mut().item_spacing.y = 8.0;
    content.add(egui::Spinner::new().size(32.0).color(foreground));
    content.label(
        egui::RichText::new(format_message!(intl, default_message: "Loading files…"))
            .color(foreground),
    );
}

fn chooser_button(
    ui: &mut Ui,
    label: &str,
    icon: icons::Icon,
    kind: button::Kind,
    enabled: bool,
    width: Option<f32>,
) -> egui::Response {
    let props = button::Props {
        label,
        icon: Some(icon),
        kind,
        size: Size::Medium,
        width: if width.is_some() {
            button::Width::Fill
        } else {
            button::Width::Fit
        },
        enabled,
    };
    if let Some(width) = width {
        ui.allocate_ui_with_layout(egui::vec2(width, 40.0), *ui.layout(), |ui| props.show(ui))
            .inner
    } else {
        props.show(ui)
    }
}
