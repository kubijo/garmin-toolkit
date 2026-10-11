//! View-only state carried when a file window changes presentation.
use super::{Browser, CompactPane, CreateDirectoryDialog, DirectoryId, Selection, path};
use camino::Utf8PathBuf;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ViewState {
    current: DirectoryId,
    selected: Option<Selection>,
    #[serde(default)]
    additional_selected: Vec<Selection>,
    #[serde(default)]
    selection_anchor: Option<Utf8PathBuf>,
    back: Vec<DirectoryId>,
    forward: Vec<DirectoryId>,
    editor: path::Editor,
    create: Option<CreateDirectoryDialog>,
    removal: Option<Selection>,
    #[serde(default)]
    bulk_removal: Option<Vec<Selection>>,
    pane: Option<CompactPane>,
}

impl Browser {
    #[must_use]
    pub fn view_state(&self) -> ViewState {
        ViewState {
            current: self.current.clone(),
            selected: self.selected.clone(),
            additional_selected: self.additional_selected.clone(),
            selection_anchor: self.selection_anchor.clone(),
            back: self.back_stack.clone(),
            forward: self.forward_stack.clone(),
            editor: self.path_editor.clone(),
            create: self.create_directory.clone(),
            removal: self.pending_removal.clone(),
            bulk_removal: self.pending_bulk_removal.clone(),
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
        self.additional_selected = view.additional_selected;
        self.selection_anchor = view.selection_anchor;
        self.marquee = None;
        self.dragged = None;
        self.drop_action = None;
        self.back_stack = view.back;
        self.forward_stack = view.forward;
        self.path_editor = view.editor;
        self.create_directory = view.create;
        self.pending_removal = view.removal;
        self.pending_bulk_removal = view.bulk_removal;
        self.compact_pane = view.pane;
    }
}
