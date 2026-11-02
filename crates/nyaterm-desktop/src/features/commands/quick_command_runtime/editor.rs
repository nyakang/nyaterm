use rust_i18n::t;

use gpui::{Context, KeyDownEvent};
use nyaterm_core::{QuickCommand, QuickCommandCategory, uuid};
use nyaterm_store::StoreDomain;

use crate::features::{NyaTermApp, formatting::non_empty_string};
use crate::models::QuickCommandEditorField;

use super::helpers::{unix_millis_now, validated_quick_command_text};

impl NyaTermApp {
    pub(in crate::features) fn set_quick_command_editor_category(
        &mut self,
        category_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .commands
            .set_quick_editor_category(category_id, String::new())
        {
            self.reset_text_input("quick-command.editor.category", "", cx);
            self.reset_text_input("quick-command.editor.new-category", "", cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn set_quick_command_editor_category_picker_open(
        &mut self,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if self.commands.set_quick_editor_category_picker_open(open) {
            if !open {
                self.reset_text_input("quick-command.editor.category", "", cx);
                self.reset_text_input("quick-command.editor.new-category", "", cx);
            }
            cx.notify();
        }
    }

    pub(in crate::features) fn commit_quick_command_editor_new_category(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.commands.commit_quick_editor_new_category() {
            self.reset_text_input("quick-command.editor.category", "", cx);
            self.reset_text_input("quick-command.editor.new-category", "", cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn set_quick_command_editor_color(
        &mut self,
        color_tag: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if self
            .commands
            .set_quick_editor_color(color_tag.map(ToOwned::to_owned))
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_quick_command_editor_icon(
        &mut self,
        icon_tag: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if self
            .commands
            .set_quick_editor_icon(icon_tag.map(ToOwned::to_owned))
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_quick_command_editor_icon_picker_open(
        &mut self,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if self.commands.set_quick_editor_icon_picker_open(open) {
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_quick_command_editor_pinned(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.commands.toggle_quick_editor_pinned() {
            cx.notify();
        }
    }

    pub(in crate::features) fn set_quick_command_editor_execution_mode(
        &mut self,
        mode: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self.commands.set_quick_editor_execution_mode(mode) {
            cx.notify();
        }
    }

    pub(in crate::features) fn toggle_quick_command_editor_raw_send(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !self.commands.toggle_quick_editor_raw_send() {
            return;
        }
        let raw_send = self
            .commands
            .quick_editor()
            .is_some_and(|editor| editor.raw_send);
        if let Some(input) = self.existing_text_input("quick-command.editor.command") {
            let placeholder = if raw_send {
                t!("quickCommands.rawCommandPlaceholder")
            } else {
                t!("quickCommands.commandPlaceholder")
            };
            input.update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        }
        cx.notify();
    }

    pub(in crate::features) fn save_quick_command_editor(&mut self, cx: &mut Context<Self>) {
        let label_required = t!("quickCommands.errorLabelRequired").to_string();
        let command_required = t!("quickCommands.errorCommandRequired").to_string();
        let categories = self.commands.quick_command_categories().to_vec();
        let Some(editor) = self.commands.quick_editor_snapshot() else {
            return;
        };
        let label = editor.label.trim().to_string();
        if label.is_empty() {
            self.commands
                .set_quick_editor_error(label_required, Some(QuickCommandEditorField::Label));
            cx.notify();
            return;
        }
        let Some(command_text) = validated_quick_command_text(&editor.command) else {
            self.commands
                .set_quick_editor_error(command_required, Some(QuickCommandEditorField::Command));
            cx.notify();
            return;
        };

        let now = unix_millis_now();
        let original = editor.original.clone();
        let category_draft = editor.category_draft.trim().to_string();
        let (category_id, new_category) = if category_draft.is_empty() {
            (editor.category_id.clone(), None)
        } else if let Some(existing) = categories
            .iter()
            .find(|category| category.name.eq_ignore_ascii_case(&category_draft))
        {
            (Some(existing.id.clone()), None)
        } else {
            let category = QuickCommandCategory {
                id: format!("quick-category-{}", uuid()),
                name: category_draft,
                parent_id: None,
                sort_order: categories
                    .iter()
                    .filter(|category| category.parent_id.is_none())
                    .map(|category| category.sort_order)
                    .max()
                    .unwrap_or(-1)
                    .saturating_add(1),
            };
            (Some(category.id.clone()), Some(category))
        };
        let command = QuickCommand {
            id: original
                .as_ref()
                .map(|command| command.id.clone())
                .unwrap_or_else(|| format!("qc-{}", uuid())),
            label,
            command: command_text,
            category_id,
            description: non_empty_string(editor.description.clone()),
            color_tag: editor.color_tag.clone(),
            icon_tag: editor.icon_tag.clone(),
            pinned: editor.pinned.then_some(true),
            execution_mode: Some(editor.execution_mode.clone()),
            raw_send: editor.raw_send.then_some(true),
            source: original.as_ref().and_then(|command| command.source.clone()),
            risk_level: original
                .as_ref()
                .and_then(|command| command.risk_level.clone()),
            updated_at: Some(now),
            created_at: original
                .as_ref()
                .and_then(|command| command.created_at)
                .or(Some(now)),
            use_count: original.as_ref().and_then(|command| command.use_count),
            sort_order: original.as_ref().and_then(|command| command.sort_order),
        };

        let Some(save_generation) = self.commands.begin_quick_editor_save() else {
            return;
        };
        let label = command.label.clone();
        let submitted = self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                store.upsert_quick_command(command, new_category)
            }),
            move |this, event, cx| {
                let current = this.commands.finish_quick_editor_save(save_generation);
                match event.outcome {
                    Ok(config) => {
                        this.replace_quick_command_catalog(config.commands, config.categories, cx);
                        if current {
                            this.commands.close_quick_editor();
                        }
                        this.settings
                            .update_store_status(format!("quick command '{label}' saved"), true);
                    }
                    Err(error) => {
                        if current {
                            this.commands
                                .set_quick_editor_error(error.to_string(), None);
                        }
                        this.settings.update_store_status(
                            format!("quick command save failed: {error}"),
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
        if !submitted {
            self.commands.finish_quick_editor_save(save_generation);
        }
        cx.notify();
    }

    pub(in crate::features) fn handle_quick_command_editor_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.mark_user_activity();
        let keystroke = &event.keystroke;
        let primary = keystroke.modifiers.platform || keystroke.modifiers.control;
        if primary && !keystroke.modifiers.alt && matches!(keystroke.key.as_str(), "s" | "S") {
            self.save_quick_command_editor(cx);
            return true;
        }
        if primary || keystroke.modifiers.alt || keystroke.modifiers.function {
            return false;
        }

        // The boxes own the text and the clipboard; the dialog owns the keys
        // that close or save it, which the boxes leave unconsumed. The script
        // box takes Enter itself, so an Enter arriving here is a save.
        match keystroke.key.as_str() {
            "escape" => self.close_quick_command_editor(cx),
            "enter" => self.save_quick_command_editor(cx),
            _ => return false,
        }
        true
    }

    /// Apply an edit from one of the quick command editor's inputs.
    pub(in crate::features) fn apply_quick_command_editor_input(
        &mut self,
        field: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let field = match field {
            "label" => QuickCommandEditorField::Label,
            "command" => QuickCommandEditorField::Command,
            "category" => QuickCommandEditorField::Category,
            "description" => QuickCommandEditorField::Description,
            _ => return,
        };
        if self.commands.apply_quick_editor_input(field, text) {
            cx.notify();
        }
    }

    pub(in crate::features) fn apply_quick_command_editor_new_category_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if self.commands.apply_quick_editor_new_category(text) {
            cx.notify();
        }
    }
}
