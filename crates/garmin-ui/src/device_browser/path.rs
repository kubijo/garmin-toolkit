//! Editable location bar shared by device browsers and remote file choosers.
use super::{
    BREADCRUMB_HEIGHT, Browser, DeviceCatalogEntryKind, Intl, Size, Ui, Utf8Path, Utf8PathBuf,
    breadcrumb, button, format_message, icons, show_breadcrumb_separator,
};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Editor {
    text: Option<String>,
    focus: bool,
    pub error: Option<String>,
}

impl Browser {
    /// Focus manual folder entry on the next frame.
    pub fn edit_location(&mut self) {
        self.path_editor.text = Some(format!("/{}", self.current.path));
        self.path_editor.error = None;
        self.path_editor.focus = true;
    }

    pub(super) fn show_path(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Utf8PathBuf> {
        let shortcut = ui.is_enabled()
            && ui.is_visible()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::L));
        if shortcut {
            self.edit_location();
        }
        if self.path_editor.text.is_some() {
            return self.edit_path(ui, intl);
        }
        let storage = &self.catalog.storages[self.current.storage_index];
        let mut target = None;
        let root = if storage.label == "/" {
            button::IconProps {
                label: &format_message!(intl, default_message: "Storage"),
                icon: icons::HARD_DRIVE,
                kind: button::Kind::Ghost,
                size: Size::Small,
                enabled: true,
            }
            .show_with_dimension(ui, BREADCRUMB_HEIGHT)
        } else {
            breadcrumb(ui, self.current.path.as_str().is_empty(), &storage.label)
        };
        if root.clicked() {
            target = Some(Utf8PathBuf::new());
        }
        let mut path = Utf8PathBuf::new();
        for component in self
            .current
            .path
            .as_str()
            .split('/')
            .filter(|part| !part.is_empty())
        {
            show_breadcrumb_separator(ui);
            path.push(component);
            if breadcrumb(ui, path == self.current.path, component).clicked() {
                target = Some(path.clone());
            }
        }
        let response = ui.allocate_response(ui.available_size(), egui::Sense::click());
        crate::semantics::target(ui, &response, "files.path.edit");
        let response =
            response.on_hover_text(format_message!(intl, default_message: "Enter a folder path"));
        if response.clicked() {
            self.edit_location();
            ui.ctx().request_repaint();
        }
        target
    }

    fn edit_path(&mut self, ui: &mut Ui, intl: &Intl) -> Option<Utf8PathBuf> {
        let focus = std::mem::take(&mut self.path_editor.focus);
        let text = self.path_editor.text.as_mut()?;
        let id = ui.id().with("path-input");
        let mut output = egui::TextEdit::singleline(text)
            .id(id)
            .desired_width(ui.available_width())
            .min_size(egui::vec2(0.0, BREADCRUMB_HEIGHT))
            .vertical_align(egui::Align::Center)
            .frame(egui::Frame::NONE)
            .margin(egui::vec2(4.0, 0.0))
            .show(ui);
        let response = output.response;
        crate::semantics::target(ui, &response, "files.path.input");
        if focus {
            response.request_focus();
            output
                .state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.chars().count()),
                )));
            output.state.store(ui.ctx(), id);
        }
        if response.has_focus() {
            ui.painter().hline(
                response.rect.x_range(),
                response.rect.bottom(),
                ui.visuals().selection.stroke,
            );
        }
        let escape =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if escape || (response.lost_focus() && !enter) {
            self.path_editor = Editor::default();
            return None;
        }
        if !enter {
            return None;
        }
        let path = normalize(text, &self.current.path);
        let valid = path.as_ref().is_some_and(|path| {
            self.remote_directories
                || path.as_str().is_empty()
                || self.catalog.storages[self.current.storage_index]
                    .entries
                    .iter()
                    .any(|entry| {
                        entry.path == *path && entry.kind == DeviceCatalogEntryKind::Directory
                    })
        });
        if valid {
            self.path_editor = Editor::default();
            path
        } else {
            self.path_editor.error = Some(
                format_message!(intl, default_message: "Folder not found. Check the path and try again."),
            );
            response.request_focus();
            None
        }
    }
}

fn normalize(text: &str, current: &Utf8Path) -> Option<Utf8PathBuf> {
    if text.is_empty() || text.contains(['\0', '\\']) {
        return None;
    }
    let mut path = if text.starts_with('/') {
        Utf8PathBuf::new()
    } else {
        current.to_owned()
    };
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            component => path.push(component),
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entered_paths_are_relative_to_the_folder_or_storage_root() {
        let current = Utf8Path::new("share/backups");
        for (typed, expected) in [
            ("/home/person/Downloads", "home/person/Downloads"),
            ("../Archive", "share/Archive"),
            ("/../../", ""),
            ("./New folder/", "share/backups/New folder"),
        ] {
            assert_eq!(normalize(typed, current).unwrap(), expected);
        }
        assert!(normalize("", current).is_none());
        assert!(normalize("bad\0path", current).is_none());
    }
}
