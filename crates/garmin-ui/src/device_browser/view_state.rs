//! View-only state carried when a file window changes presentation.
use super::{Browser, CompactPane, CreateDirectoryDialog, DirectoryId, Selection, path};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ViewState {
    current: DirectoryId,
    selected: Option<Selection>,
    back: Vec<DirectoryId>,
    forward: Vec<DirectoryId>,
    editor: path::Editor,
    create: Option<CreateDirectoryDialog>,
    removal: Option<Selection>,
    pane: Option<CompactPane>,
}

impl Browser {
    #[must_use]
    pub fn view_state(&self) -> ViewState {
        ViewState {
            current: self.current.clone(),
            selected: self.selected.clone(),
            back: self.back_stack.clone(),
            forward: self.forward_stack.clone(),
            editor: self.path_editor.clone(),
            create: self.create_directory.clone(),
            removal: self.pending_removal.clone(),
            pane: self.compact_pane,
        }
    }

    /// Restores local presentation state; never dispatches a filesystem operation.
    pub fn restore_view(&mut self, view: ViewState) {
        if view.current.storage_index >= self.catalog.storages.len() {
            return;
        }
        self.set_current(view.current);
        self.selected = view.selected;
        self.back_stack = view.back;
        self.forward_stack = view.forward;
        self.path_editor = view.editor;
        self.create_directory = view.create;
        self.pending_removal = view.removal;
        self.compact_pane = view.pane;
    }
}
