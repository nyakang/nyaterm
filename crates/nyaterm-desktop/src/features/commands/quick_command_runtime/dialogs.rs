use rust_i18n::t;

use gpui::{Context, ParentElement as _, Window, div};
use nyaterm_core::{QuickCommandCategory, uuid};
use nyaterm_store::StoreDomain;
use nyaterm_ui::{NyaConfirmDialog, NyaDialogFooter, NyaDialogWindowExt};

use crate::features::NyaTermApp;
use crate::models::{
    QuickCommandCategoryCreateState, QuickCommandCategoryDeleteState,
    QuickCommandCategoryRenameState, QuickCommandDetailsState, QuickCommandEditorState,
};

impl NyaTermApp {
    pub(in crate::features) fn open_new_quick_command_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let category = self.commands.quick_command_creation_category();
        self.open_new_quick_command_editor_in_category(category, window, cx);
    }

    pub(in crate::features) fn open_quick_command_editor_with_command(
        &mut self,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commands
            .open_quick_editor(QuickCommandEditorState::from_terminal_selection(command));
        self.forget_text_inputs("quick-command.editor.");
        self.shell
            .set_status("quick command editor opened from selection".to_string());
        if !self.open_quick_command_window(cx) {
            window.focus(self.commands.quick_editor_focus(), cx);
        }
        cx.notify();
    }

    /// The child window reads the editor state when
    /// it opens, so seeding the draft first is what carries the category across.
    pub(in crate::features) fn open_new_quick_command_editor_in_category(
        &mut self,
        category_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = QuickCommandEditorState::blank_in_category(
            category_id,
            self.commands.quick_command_categories(),
        );
        self.commands.open_quick_editor(editor);
        // The boxes own their text, so they have to be dropped for the next
        // command to seed from its own values.
        self.forget_text_inputs("quick-command.editor.");
        self.shell
            .set_status("quick command editor opened".to_string());
        if !self.open_quick_command_window(cx) {
            window.focus(self.commands.quick_editor_focus(), cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn open_edit_quick_command_editor(
        &mut self,
        command_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self
            .commands
            .quick_commands()
            .iter()
            .find(|command| command.id == command_id)
            .cloned()
        else {
            self.shell
                .set_status("quick command is no longer available".to_string());
            cx.notify();
            return;
        };
        self.commands
            .open_quick_editor(QuickCommandEditorState::from_command(command));
        // The boxes own their text, so they have to be dropped for the next
        // command to seed from its own values.
        self.forget_text_inputs("quick-command.editor.");
        self.shell
            .set_status("quick command editor opened".to_string());
        if !self.open_quick_command_window(cx) {
            window.focus(self.commands.quick_editor_focus(), cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn close_quick_command_editor(&mut self, cx: &mut Context<Self>) {
        self.commands.close_quick_editor();
        self.forget_text_inputs("quick-command.editor.");
        self.shell
            .set_status("quick command editor closed".to_string());
        cx.notify();
    }

    pub(in crate::features) fn open_quick_command_details(
        &mut self,
        command_id: String,
        x: gpui::Pixels,
        y: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self
            .commands
            .quick_commands()
            .iter()
            .find(|command| command.id == command_id)
            .cloned()
        else {
            self.shell
                .set_status("quick command is no longer available".to_string());
            cx.notify();
            return;
        };
        self.commands
            .request_quick_details(QuickCommandDetailsState { command, x, y });
        self.shell
            .set_status("quick command details opened".to_string());
        window.focus(self.commands.quick_details_focus(), cx);
        cx.notify();
    }

    /// Copy a command verbatim, as the copy button on Tauri's details card does.
    pub(in crate::features) fn copy_quick_command_text(
        &mut self,
        command: String,
        cx: &mut Context<Self>,
    ) {
        if command.trim().is_empty() {
            self.shell
                .set_status("quick command has no text to copy".to_string());
            self.copy_text_with_feedback("copy-quick-command", String::new(), cx);
        } else {
            self.copy_text_with_feedback("copy-quick-command", command, cx);
            self.shell
                .set_status("quick command copied to clipboard".to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn close_quick_command_details(&mut self, cx: &mut Context<Self>) {
        self.commands.clear_quick_details();
        self.shell
            .set_status("quick command details closed".to_string());
        cx.notify();
    }

    pub(in crate::features) fn open_delete_quick_command_confirm(
        &mut self,
        command_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = self
            .commands
            .quick_commands()
            .iter()
            .find(|command| command.id == command_id)
            .cloned()
        else {
            self.shell
                .set_status("quick command is no longer available".to_string());
            cx.notify();
            return;
        };
        self.shell
            .set_status("quick command delete confirmation opened".to_string());
        let title = t!("quickCommands.delete").to_string();
        let message = t!("quickCommands.deleteConfirm", name = command.label).to_string();
        let cancel_label = t!("common.cancel").to_string();
        let delete_label = t!("common.delete").to_string();
        let app = cx.weak_entity();
        let command_id = command.id.clone();
        let command_label = command.label.clone();
        window.open_nya_dialog(cx, move |dialog, _, _| {
            let confirm_app = app.clone();
            let command_id = command_id.clone();
            let command_label = command_label.clone();
            NyaConfirmDialog::new(
                dialog.title(title.clone()).width(384.),
                NyaDialogFooter::new(cancel_label.clone(), delete_label.clone()).danger(),
            )
            .content(div().child(message.clone()))
            .on_confirm(move |_, _, cx| {
                confirm_app
                    .update(cx, |app, cx| {
                        app.confirm_delete_quick_command(
                            command_id.clone(),
                            command_label.clone(),
                            cx,
                        )
                    })
                    .is_ok()
            })
            .on_cancel(|_, _, _| true)
            .into_dialog()
        });
        cx.notify();
    }

    fn confirm_delete_quick_command(
        &mut self,
        command_id: String,
        command_label: String,
        cx: &mut Context<Self>,
    ) {
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                let mut config = store.load_quick_commands()?;
                let before = config.commands.len();
                config.commands.retain(|command| command.id != command_id);
                let deleted = config.commands.len() != before;
                store.save_quick_commands(config.clone())?;
                Ok((config, deleted))
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok((config, deleted)) => {
                        this.replace_quick_command_catalog(config.commands, config.categories, cx);
                        this.settings.update_store_status(
                            if deleted {
                                format!("quick command '{command_label}' deleted")
                            } else {
                                format!("quick command '{command_label}' was already deleted")
                            },
                            deleted,
                        );
                    }
                    Err(error) => this.settings.update_store_status(
                        format!("quick command delete failed: {error}"),
                        false,
                    ),
                }
                this.shell
                    .set_status(this.settings.store_status().message.to_string());
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn open_delete_quick_command_category_confirm(
        &mut self,
        category_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(category) = self
            .commands
            .quick_command_categories()
            .iter()
            .find(|category| category.id == category_id)
            .cloned()
        else {
            self.shell
                .set_status("quick command category is no longer available".to_string());
            cx.notify();
            return;
        };
        let command_count = self
            .commands
            .quick_commands()
            .iter()
            .filter(|command| command.category_id.as_deref() == Some(category.id.as_str()))
            .count();
        let category_name = category.name.clone();
        self.commands
            .request_quick_category_delete(QuickCommandCategoryDeleteState {
                id: category.id,
                name: category_name.clone(),
            });
        self.shell
            .set_status("quick command category delete confirmation opened".to_string());
        let title = t!("quickCommands.deleteCategory").to_string();
        let message = t!(
            "quickCommands.deleteCategoryConfirm",
            name = category_name,
            count = command_count
        )
        .to_string();
        self.open_confirm_dialog_with_cancel(
            (
                title,
                message,
                t!("common.delete").to_string(),
                true,
                |app, _, cx| app.confirm_delete_quick_command_category(cx),
                |app, cx| app.cancel_delete_quick_command_category(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn cancel_delete_quick_command_category(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.commands.clear_quick_category_delete();
        self.shell
            .set_status("quick command category delete cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn confirm_delete_quick_command_category(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(delete) = self.commands.quick_category_delete().cloned() else {
            return true;
        };
        let request_delete = delete.clone();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                let mut config = store.load_quick_commands()?;
                let before_categories = config.categories.len();
                let before_commands = config.commands.len();
                config
                    .categories
                    .retain(|category| category.id != request_delete.id);
                config.commands.retain(|command| {
                    command.category_id.as_deref() != Some(request_delete.id.as_str())
                });
                let deleted_category = config.categories.len() != before_categories;
                let deleted_commands = before_commands.saturating_sub(config.commands.len());
                store.save_quick_commands(config.clone())?;
                Ok((config, deleted_category, deleted_commands))
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok((config, deleted_category, deleted_commands)) => {
                        this.replace_quick_command_catalog(config.commands, config.categories, cx);
                        this.commands.finish_quick_category_delete(&delete.id);
                        this.sync_quick_command_selected_category(cx);
                        this.settings.update_store_status(
                            if deleted_category {
                                format!(
                                    "quick command category '{}' deleted with {} command(s)",
                                    delete.name, deleted_commands
                                )
                            } else {
                                format!(
                                    "quick command category '{}' was already deleted",
                                    delete.name
                                )
                            },
                            deleted_category,
                        );
                    }
                    Err(error) => this.settings.update_store_status(
                        format!("quick command category delete failed: {error}"),
                        false,
                    ),
                }
                this.shell
                    .set_status(this.settings.store_status().message.to_string());
                cx.notify();
            },
            cx,
        );
        true
    }

    pub(in crate::features) fn open_rename_quick_command_category(
        &mut self,
        category_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(category) = self
            .commands
            .quick_command_categories()
            .iter()
            .find(|category| category.id == category_id)
            .cloned()
        else {
            self.shell
                .set_status("quick command category is no longer available".to_string());
            cx.notify();
            return;
        };
        self.reset_text_input("quick-command.category-rename", &category.name, cx);
        self.commands
            .request_quick_category_rename(QuickCommandCategoryRenameState {
                id: category.id,
                original_name: category.name.clone(),
                draft: category.name,
                error: None,
            });
        self.shell
            .set_status("quick command category rename opened".to_string());
        self.open_form_dialog(
            (
                t!("quickCommands.renameCategory").to_string(),
                384.,
                t!("common.confirm").to_string(),
                |app, _, cx| app.quick_command_category_rename_dialog_content(cx),
                |app, _, cx| app.confirm_rename_quick_command_category(cx),
                |app, cx| app.cancel_rename_quick_command_category(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn cancel_rename_quick_command_category(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.commands.clear_quick_category_rename();
        self.forget_text_inputs("quick-command.category-rename");
        self.shell
            .set_status("quick command category rename cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn confirm_rename_quick_command_category(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(rename) = self.commands.quick_category_rename().cloned() else {
            return true;
        };
        let name = rename.draft.trim().to_string();
        if name.is_empty() {
            let message = t!("quickCommands.categoryNameRequired").to_string();
            self.commands.set_quick_category_rename_error(message);
            cx.notify();
            return false;
        }
        if self
            .commands
            .quick_command_categories()
            .iter()
            .any(|category| {
                category.id != rename.id && category.name.trim().eq_ignore_ascii_case(name.as_str())
            })
        {
            let message = t!("quickCommands.categoryNameDuplicated").to_string();
            self.commands.set_quick_category_rename_error(message);
            cx.notify();
            return false;
        }

        let request_rename = rename.clone();
        let request_name = name.clone();
        // The form closes as soon as the request is accepted. Retire its input
        // now; a late store response must not clear a newly opened rename form.
        self.commands.clear_quick_category_rename();
        self.forget_text_inputs("quick-command.category-rename");
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                let mut config = store.load_quick_commands()?;
                let duplicated = config.categories.iter().any(|category| {
                    category.id != request_rename.id
                        && category
                            .name
                            .trim()
                            .eq_ignore_ascii_case(request_name.as_str())
                });
                if duplicated {
                    return Ok((config, false, true));
                }
                let mut renamed = false;
                if let Some(category) = config
                    .categories
                    .iter_mut()
                    .find(|category| category.id == request_rename.id)
                {
                    category.name = request_name;
                    renamed = true;
                }
                store.save_quick_commands(config.clone())?;
                Ok((config, renamed, false))
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok((config, renamed, duplicated)) => {
                        this.replace_quick_command_catalog(config.commands, config.categories, cx);
                        if renamed {
                            this.settings.update_store_status(
                                format!(
                                    "quick command category '{}' renamed to '{}'",
                                    rename.original_name, name
                                ),
                                true,
                            );
                        } else {
                            let message = if duplicated {
                                t!("quickCommands.categoryNameDuplicated").to_string()
                            } else {
                                t!("quickCommands.categoryUnavailable").to_string()
                            };
                            this.settings.update_store_status(message, false);
                        }
                    }
                    Err(error) => {
                        this.settings.update_store_status(
                            format!("quick command category rename failed: {error}"),
                            false,
                        );
                    }
                }
                this.shell
                    .set_status(this.settings.store_status().message.to_string());
                cx.notify();
            },
            cx,
        );
        true
    }

    /// Opens the "add category" dialog. `parent_id` is `None` from a pseudo-row,
    /// which creates a root category.
    pub(in crate::features) fn open_new_quick_command_category(
        &mut self,
        parent_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A stale parent would silently create a root category, so drop the parent
        // rather than attach the new row to something that no longer exists.
        let parent_id = parent_id.filter(|parent| {
            self.commands
                .quick_command_categories()
                .iter()
                .any(|category| &category.id == parent)
        });
        self.commands
            .request_quick_category_create(QuickCommandCategoryCreateState {
                parent_id,
                draft: String::new(),
                error: None,
            });
        self.reset_text_input("quick-command.category-create", "", cx);
        self.shell
            .set_status("quick command category editor opened".to_string());
        self.open_form_dialog(
            (
                t!("quickCommands.addCategory").to_string(),
                384.,
                t!("common.confirm").to_string(),
                |app, _, cx| app.quick_command_category_create_dialog_content(cx),
                |app, _, cx| app.confirm_create_quick_command_category(cx),
                |app, cx| app.cancel_create_quick_command_category(cx),
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn cancel_create_quick_command_category(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.commands.clear_quick_category_create();
        self.shell
            .set_status("quick command category creation cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn confirm_create_quick_command_category(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(create) = self.commands.quick_category_create().cloned() else {
            return true;
        };
        let name = create.draft.trim().to_string();
        if name.is_empty() {
            let message = t!("quickCommands.categoryNameRequired").to_string();
            self.commands.set_quick_category_create_error(message);
            cx.notify();
            return false;
        }
        if self
            .commands
            .quick_command_categories()
            .iter()
            .any(|category| category.name.trim().eq_ignore_ascii_case(name.as_str()))
        {
            let message = t!("quickCommands.categoryNameDuplicated").to_string();
            self.commands.set_quick_category_create_error(message);
            cx.notify();
            return false;
        }

        let parent_id = create.parent_id.clone();
        let request_name = name.clone();
        let status_name = name.clone();
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                let mut config = store.load_quick_commands()?;
                let duplicated = config
                    .categories
                    .iter()
                    .any(|category| category.name.trim().eq_ignore_ascii_case(&request_name));
                if duplicated {
                    return Ok((config, false, true));
                }
                // A parent removed between opening the dialog and confirming would
                // orphan the row, so fall back to a root category.
                let parent_id = parent_id.filter(|parent| {
                    config
                        .categories
                        .iter()
                        .any(|category| &category.id == parent)
                });
                let sort_order = config
                    .categories
                    .iter()
                    .filter(|category| category.parent_id == parent_id)
                    .map(|category| category.sort_order)
                    .max()
                    .unwrap_or(-1)
                    .saturating_add(1);
                config.categories.push(QuickCommandCategory {
                    id: format!("quick-category-{}", uuid()),
                    name: request_name,
                    parent_id,
                    sort_order,
                });
                store.save_quick_commands(config.clone())?;
                Ok((config, true, false))
            }),
            move |this, event, cx| {
                match event.outcome {
                    Ok((config, created, duplicated)) => {
                        this.replace_quick_command_catalog(config.commands, config.categories, cx);
                        if created {
                            this.commands.clear_quick_category_create();
                            this.settings.update_store_status(
                                format!("quick command category '{status_name}' created"),
                                true,
                            );
                        } else {
                            let message = if duplicated {
                                t!("quickCommands.categoryNameDuplicated").to_string()
                            } else {
                                t!("quickCommands.categoryCreateFailed").to_string()
                            };
                            this.commands
                                .set_quick_category_create_error(message.clone());
                            this.settings.update_store_status(message, false);
                        }
                    }
                    Err(error) => {
                        this.commands
                            .set_quick_category_create_error(error.to_string());
                        this.settings.update_store_status(
                            format!("quick command category creation failed: {error}"),
                            false,
                        );
                    }
                }
                this.shell
                    .set_status(this.settings.store_status().message.to_string());
                cx.notify();
            },
            cx,
        );
        true
    }

    /// Apply an edit from the category rename box.
    pub(in crate::features) fn apply_quick_command_category_rename(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.commands.apply_quick_category_rename(text) {
            cx.notify();
        }
    }

    /// Apply an edit from the new-category box.
    pub(in crate::features) fn apply_quick_command_category_create(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.commands.apply_quick_category_create(text) {
            cx.notify();
        }
    }
}
