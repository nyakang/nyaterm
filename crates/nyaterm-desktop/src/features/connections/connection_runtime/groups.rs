use rust_i18n::t;

use gpui::{Context, KeyDownEvent, Window};
use nyaterm_core::{Group, uuid};
use nyaterm_store::{StoreDomain, store_request};

use crate::features::NyaTermApp;
use crate::models::ConnectionGroupEditorMode;

impl NyaTermApp {
    pub(in crate::features) fn open_connection_group_editor(
        &mut self,
        group_id: Option<String>,
        parent_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connection_state.close_group_editor();
        self.connection_state.clear_group_editor_field();
        self.clear_connection_search_without_focus(cx);

        let editing = group_id.is_some();
        match group_id {
            Some(id) => {
                let Some(group) = self
                    .connection_state
                    .groups()
                    .iter()
                    .find(|group| group.id == id)
                    .cloned()
                else {
                    self.shell
                        .set_status("connection group is no longer available".to_string());
                    cx.notify();
                    return;
                };
                self.connection_state.begin_rename_group_editor(
                    group.id,
                    group.name,
                    group.parent_id,
                );
            }
            None => {
                self.connection_state.begin_create_group_editor(parent_id);
            }
        }
        self.connection_state
            .build_group_editor_field(t!("savedConnections.folderName").into(), cx);
        if let Some(field) = self.connection_state.group_editor_field() {
            let focus = field.read(cx).focus_handle();
            window.focus(&focus, cx);
            if editing {
                field.update(cx, |field, cx| field.select_all(window, cx));
            }
        }
        self.shell
            .set_status("connection group inline editor opened".to_string());
        cx.notify();
    }

    pub(in crate::features) fn cancel_connection_group_editor(&mut self, cx: &mut Context<Self>) {
        self.connection_state.close_group_editor();
        self.connection_state.clear_group_editor_field();
        self.shell
            .set_status("connection group editor cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn finish_connection_group_editor_from_blur(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.connection_state.active_group_editor_draft() else {
            return;
        };
        if editor.mode == ConnectionGroupEditorMode::Create && editor.name.trim().is_empty() {
            self.cancel_connection_group_editor(cx);
            return;
        }
        self.save_connection_group_editor(cx);
    }

    pub(in crate::features) fn handle_connection_group_editor_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.alt
            || event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.function
        {
            return;
        }
        if event.keystroke.key.as_str() == "escape" {
            cx.stop_propagation();
            self.cancel_connection_group_editor(cx);
        }
    }

    pub(in crate::features) fn save_connection_group_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.connection_state.active_group_editor_draft() else {
            return;
        };
        let name = editor.name.trim().to_string();
        if name.is_empty() {
            let message = t!("savedConnections.folderNameRequired").to_string();
            self.connection_state.set_group_editor_error(message);
            cx.notify();
            return;
        }

        let group = match editor.mode {
            ConnectionGroupEditorMode::Create => Group {
                id: uuid(),
                name,
                parent_id: editor.parent_id.clone(),
                sort_order: self.connection_state.groups().len() as i32,
                created_at_ms: None,
                updated_at_ms: None,
            },
            ConnectionGroupEditorMode::Rename => {
                let Some(id) = editor.id.as_deref() else {
                    self.connection_state.close_group_editor();
                    self.connection_state.clear_group_editor_field();
                    cx.notify();
                    return;
                };
                let Some(mut group) = self
                    .connection_state
                    .groups()
                    .iter()
                    .find(|group| group.id == id)
                    .cloned()
                else {
                    self.connection_state.close_group_editor();
                    self.connection_state.clear_group_editor_field();
                    self.shell
                        .set_status("connection group is no longer available".to_string());
                    cx.notify();
                    return;
                };
                if group.name == name {
                    self.connection_state.close_group_editor();
                    self.connection_state.clear_group_editor_field();
                    self.shell
                        .set_status("connection group rename unchanged".to_string());
                    cx.notify();
                    return;
                }
                group.name = name;
                group
            }
        };

        let persisted = group.clone();
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                store.save_group(&persisted)?;
                store.load_sessions()
            }),
            move |this, event, cx| match event.outcome {
                Ok(sessions) => {
                    if let Some(parent_id) = group.parent_id.clone() {
                        this.connection_state.expand_list_group(parent_id);
                    }
                    this.connection_state.expand_list_group(group.id.clone());
                    this.apply_loaded_sessions(sessions, cx);
                    this.connection_state.close_group_editor();
                    this.connection_state.clear_group_editor_field();
                    this.shell
                        .set_status(format!("saved connection group {}", group.name));
                    cx.notify();
                }
                Err(error) => {
                    this.connection_state
                        .set_group_editor_error(error.to_string());
                    cx.notify();
                }
            },
            cx,
        );
        cx.notify();
    }

    fn clear_connection_search_without_focus(&mut self, cx: &mut Context<Self>) {
        if self.connection_state.list_search_is_empty() {
            return;
        }
        let field = self.connection_state.list_search_field();
        field.update(cx, |field, cx| field.set_content("", cx));
        self.connection_state.set_list_search_text(String::new());
        self.sync_connection_keyboard_active(cx);
    }
}
