use super::FileManagerNameKind;
use crate::features::file_manager::pane::{PaneEndpoint, PaneId};
use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::features::transfers::file_open::FileOpenMode;
use crate::features::transfers::file_operations::{FileTransferIntent, FileTransferOperation};
use gpui::{Context, KeyDownEvent, Modifiers, Render, Window, div, prelude::*};
use nyaterm_files::file_browser::file_browser_copy_destination_invalid;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry};
use rust_i18n::t;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct FileManagerDragPayload {
    pub manager_id: String,
    pub pane_id: PaneId,
    pub endpoint: PaneEndpoint,
    pub entries: Vec<SftpFileEntry>,
    pub feedback: Rc<Cell<FileTransferOperation>>,
    pub valid_target: Rc<Cell<bool>>,
    pub same_pane_target: Rc<Cell<bool>>,
}

pub(super) fn drag_operation(same_pane: bool, modifiers: Modifiers) -> FileTransferOperation {
    let copy = if cfg!(target_os = "macos") {
        modifiers.alt
    } else {
        modifiers.control
    };
    let moving = if cfg!(target_os = "macos") {
        modifiers.platform
    } else {
        modifiers.shift
    };
    if copy {
        FileTransferOperation::Copy
    } else if moving || same_pane {
        FileTransferOperation::Move
    } else {
        FileTransferOperation::Copy
    }
}

pub(super) fn accepts_drop(
    payload: &FileManagerDragPayload,
    endpoint: &PaneEndpoint,
    directory: &RemoteFilePath,
    writable: bool,
) -> bool {
    writable
        && !directory.display_path.is_empty()
        && !matches!(endpoint, PaneEndpoint::OfflineConnection(_))
        && !payload.entries.is_empty()
        && payload.entries.iter().all(|entry| {
            !nyaterm_files::file_browser::file_browser_path_is_root(
                payload.endpoint.kind(),
                &entry.path,
            ) && (payload.endpoint != *endpoint
                || !file_browser_copy_destination_invalid(
                    endpoint.kind(),
                    &entry.path,
                    &nyaterm_files::file_browser::file_browser_join(
                        endpoint.kind(),
                        &directory.display_path,
                        &entry.name,
                    ),
                    entry.is_directory(),
                ))
        })
}

impl FileManagerWorkspace {
    pub(super) fn handle_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for pane in [PaneId::Left, PaneId::Right] {
            if [&self.inputs(pane).path, &self.inputs(pane).search]
                .into_iter()
                .any(|input| {
                    input
                        .read(cx)
                        .component_focus_handle(cx)
                        .contains_focused(window, cx)
                })
            {
                self.active = pane;
                break;
            }
        }
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let primary = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        if primary && key == "l" {
            self.begin_path_edit(self.active, window, cx);
            cx.stop_propagation();
            return;
        }
        let input_focused = [PaneId::Left, PaneId::Right].into_iter().any(|pane| {
            let inputs = self.inputs(pane);
            [&inputs.path, &inputs.search].into_iter().any(|input| {
                input
                    .read(cx)
                    .component_focus_handle(cx)
                    .contains_focused(window, cx)
            })
        });
        if input_focused {
            if key == "escape" && self.active_pane().path_editing {
                self.pane_mut(self.active).path_editing = false;
                self.focus_pane(self.active, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if primary && key == "a" {
            let visible = self.current_visible(self.active, cx);
            self.pane_mut(self.active).select_all(&visible);
        } else if primary && key == "f" {
            window.focus(
                &self
                    .inputs(self.active)
                    .search
                    .read(cx)
                    .component_focus_handle(cx),
                cx,
            );
        } else if modifiers.alt && matches!(key, "left" | "right") {
            if let Some(path) =
                self.pane_mut(self.active)
                    .history_path(if key == "left" { -1 } else { 1 })
            {
                self.navigate_to(self.active, path, window, cx);
            }
        } else if matches!(key, "up" | "down" | "home" | "end") && !primary && !modifiers.alt {
            let visible = self.current_visible(self.active, cx);
            if !visible.is_empty() {
                let current =
                    self.active_pane().selected_path.as_deref().and_then(|key| {
                        visible.iter().position(|entry| entry.matches_identity(key))
                    });
                let index = match key {
                    "home" => 0,
                    "end" => visible.len() - 1,
                    "up" => current.unwrap_or(1).saturating_sub(1),
                    _ => current
                        .map(|index| (index + 1).min(visible.len() - 1))
                        .unwrap_or(0),
                };
                let pane = self.pane_mut(self.active);
                pane.select_visible(
                    visible[index].identity_key(),
                    false,
                    modifiers.shift,
                    &visible,
                );
                pane.list_scroll
                    .scroll_to_item(index + 1, gpui::ScrollStrategy::Nearest);
            }
        } else if !modifiers.modified() {
            match key {
                "tab" => self.focus_pane(self.active.other(), window, cx),
                "enter" => self.open_current(FileOpenMode::Default, window, cx),
                "backspace" => self.navigate_parent(self.active, window, cx),
                "f2" => self.open_name_dialog(self.active, FileManagerNameKind::Rename, window, cx),
                "f3" => self.open_current(FileOpenMode::Preview, window, cx),
                "f4" => self.open_current(FileOpenMode::Edit, window, cx),
                "f5" => self.send_transfer(FileTransferOperation::Copy, window, cx),
                "f6" => self.send_transfer(FileTransferOperation::Move, window, cx),
                "f7" => self.open_name_dialog(self.active, FileManagerNameKind::Folder, window, cx),
                "delete" => self.delete_selection(window, cx),
                "escape" => self.pane_mut(self.active).clear_selection(),
                _ => return,
            }
        } else {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn current_visible(
        &mut self,
        pane: PaneId,
        cx: &gpui::App,
    ) -> std::sync::Arc<[SftpFileEntry]> {
        let show_hidden = self.read_app(cx, |app| {
            app.settings.summary().ui_file_explorer_show_hidden_files
        });
        self.pane_mut(pane).visible_entries(show_hidden)
    }

    pub(super) fn navigate_to(
        &mut self,
        pane: PaneId,
        path: RemoteFilePath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pane(pane, window, cx);
        self.pane_mut(pane).path_editing = false;
        let endpoint = self.pane(pane).endpoint.clone();
        let id = self.id.clone();
        self.dispatch_app(window, cx, move |app, _, cx| {
            if app
                .file_manager
                .workspace(&id)
                .is_some_and(|workspace| workspace.read(cx).pane(pane).endpoint == endpoint)
            {
                app.navigate_file_manager_pane(id, pane, path, cx);
            }
        });
    }

    pub(super) fn navigate_parent(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.pane(pane);
        let path = if state.endpoint.kind()
            == nyaterm_files::file_browser::FileBrowserBackendKind::Remote
        {
            match state.path.parent() {
                Ok(path) => path,
                Err(_) => return,
            }
        } else {
            RemoteFilePath::new(nyaterm_files::file_browser::file_browser_parent(
                state.endpoint.kind(),
                &state.path.display_path,
            ))
        };
        self.navigate_to(pane, path, window, cx);
    }

    fn open_current(&mut self, mode: FileOpenMode, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.active_pane();
        if let Some(entry) = pane
            .selected_path
            .as_deref()
            .and_then(|key| {
                pane.entries
                    .iter()
                    .find(|entry| entry.matches_identity(key))
            })
            .cloned()
        {
            self.open_entry(self.active, entry, mode, window, cx);
        }
    }

    pub(super) fn open_entry(
        &mut self,
        pane: PaneId,
        entry: SftpFileEntry,
        mode: FileOpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entry.is_directory() {
            self.navigate_to(pane, entry.remote_path(), window, cx);
        } else {
            let endpoint = self.pane(pane).endpoint.clone();
            self.dispatch_app(window, cx, move |app, window, cx| {
                app.open_file_for_endpoint(endpoint, entry, mode, window, cx)
            });
        }
    }

    pub(super) fn drop_files(
        &self,
        payload: &FileManagerDragPayload,
        pane: PaneId,
        directory: RemoteFilePath,
        writable: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let endpoint = self.pane(pane).endpoint.clone();
        if self.pane(pane).disconnected || !accepts_drop(payload, &endpoint, &directory, writable) {
            return;
        }
        let operation = drag_operation(
            payload.manager_id == self.id && payload.pane_id == pane,
            window.modifiers(),
        );
        let intent = FileTransferIntent {
            source: payload.endpoint.clone(),
            entries: payload.entries.clone(),
            target: endpoint,
            target_directory: directory,
            operation,
        };
        let id = self.id.clone();
        self.dispatch_app(window, cx, move |app, _, cx| {
            app.submit_file_transfer(id, intent, cx)
        });
    }
}

pub(super) struct FileManagerDragPreview {
    pub payload: FileManagerDragPayload,
}

impl Render for FileManagerDragPreview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let operation =
            match drag_operation(self.payload.same_pane_target.get(), window.modifiers()) {
                FileTransferOperation::Copy => t!("fileManager.copy"),
                FileTransferOperation::Move => t!("fileManager.move"),
            };
        let validity = if self.payload.valid_target.get() {
            ""
        } else {
            " · ⊘"
        };
        div().px_2().py_1().text_xs().child(format!(
            "{operation} · {}{validity}",
            self.payload.entries.len()
        ))
    }
}
