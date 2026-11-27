use rust_i18n::t;

use gpui::{Context, KeyDownEvent, Window};
use nyaterm_core::{QuickCommand, QuickCommandCategory, QuickCommandsConfig};
use nyaterm_store::{StoreDomain, store_request};

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::{BottomPanelMode, NavItem, QuickCommandSortMode, QuickCommandViewMode};

use super::helpers::{
    quick_command_sort_mode_setting, quick_command_view_mode_setting, unix_millis_now,
};
use crate::features::settings::SettingsSaveKind;

impl NyaTermApp {
    pub(in crate::features) fn finish_quick_command_reorder(
        &mut self,
        config: Option<QuickCommandsConfig>,
        cx: &mut Context<Self>,
    ) {
        self.commands.clear_quick_drop_target();
        let Some(config) = config else {
            cx.notify();
            return;
        };
        self.commands
            .set_quick_sort_mode(QuickCommandSortMode::Custom);
        self.settings
            .set_quick_command_sort_mode("custom".to_string());
        let settings = self.settings.summary().clone();
        self.shell
            .set_status("saving custom quick command order".to_string());
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                store.save_quick_commands(config)?;
                store.save_quick_command_ui_settings(&settings)?;
                Ok(())
            }),
            |this, event, cx| {
                match event.outcome {
                    Ok(()) => {
                        this.settings
                            .update_store_status("custom quick command order saved", true);
                        this.shell
                            .set_status("custom quick command order saved".to_string());
                    }
                    Err(error) => {
                        this.settings.update_store_status(
                            format!("quick command reorder save failed: {error}"),
                            false,
                        );
                        this.shell
                            .set_status(this.settings.store_status().message.to_string());
                        this.refresh_quick_commands(cx);
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    /// Moves a category one slot among its siblings from the group context menu.
    ///
    /// Persists through `finish_quick_command_reorder`, the same path category
    /// drag-and-drop uses, so both routes agree on ordering and on switching the
    /// list to the custom sort.
    pub(in crate::features) fn move_quick_command_category(
        &mut self,
        category_id: String,
        up: bool,
        cx: &mut Context<Self>,
    ) {
        let config = self.commands.move_quick_category_by_one(&category_id, up);
        if config.is_none() {
            self.shell.set_status(
                "quick command category is already at the end of its group".to_string(),
            );
            cx.notify();
            return;
        }
        self.finish_quick_command_reorder(config, cx);
    }

    pub(in crate::features) fn outdent_quick_command_category(
        &mut self,
        category_id: String,
        cx: &mut Context<Self>,
    ) {
        let config = self.commands.outdent_quick_category(&category_id);
        self.finish_quick_command_reorder(config, cx);
    }

    pub(in crate::features) fn close_quick_command_toolbar_popovers(&mut self) {
        self.commands.close_quick_toolbar_popovers();
    }

    pub(in crate::features) fn refresh_quick_commands(&mut self, cx: &mut Context<Self>) {
        self.submit_store_request(
            0,
            store_request(StoreDomain::Commands, |store| store.load_quick_commands()),
            |this, event, cx| {
                match event.outcome {
                    Ok(config) => {
                        this.replace_quick_command_catalog(config.commands, config.categories, cx)
                    }
                    Err(error) => this.settings.update_store_status(
                        format!("quick command refresh failed: {error}"),
                        false,
                    ),
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn replace_quick_command_catalog(
        &mut self,
        commands: Vec<QuickCommand>,
        categories: Vec<QuickCommandCategory>,
        cx: &mut Context<Self>,
    ) {
        self.commands
            .replace_quick_command_catalog(commands, categories);
        self.sync_quick_command_selected_category(cx);
        self.sync_quick_command_sidebar_expanded_categories(cx);
    }

    pub(in crate::features) fn sync_quick_command_sidebar_expanded_categories(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let expanded = self.commands.quick_sidebar_expanded_category_ids_sorted();
        if self
            .settings
            .summary()
            .ui_quick_cmd_sidebar_expanded_category_ids
            == expanded
        {
            return;
        }
        self.settings
            .set_quick_command_sidebar_expanded_categories(expanded);
        self.save_quick_command_ui_settings(cx);
    }

    pub(in crate::features) fn toggle_quick_command_sidebar_category(
        &mut self,
        category_id: String,
        cx: &mut Context<Self>,
    ) {
        self.commands.toggle_quick_sidebar_category(category_id);
        self.save_quick_command_ui_settings(cx);
        cx.notify();
    }

    pub(in crate::features) fn set_all_quick_command_sidebar_categories_expanded(
        &mut self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .commands
            .set_all_quick_sidebar_categories_expanded(expanded)
        {
            self.save_quick_command_ui_settings(cx);
            cx.notify();
        }
    }

    pub(in crate::features) fn sync_quick_command_selected_category(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let selected = self.commands.quick_selected_category();
        if self.settings.summary().ui_quick_cmd_selected_category == selected {
            return;
        }
        self.settings
            .set_quick_command_selected_category(selected.to_string());
        self.save_quick_command_ui_settings(cx);
    }

    pub(in crate::features) fn select_quick_command_category(
        &mut self,
        category_id: String,
        cx: &mut Context<Self>,
    ) {
        self.commands.select_quick_category(category_id);
        self.sync_quick_command_selected_category(cx);
        cx.notify();
    }

    pub(in crate::features) fn set_quick_command_view_mode(
        &mut self,
        mode: QuickCommandViewMode,
        cx: &mut Context<Self>,
    ) {
        self.apply_quick_command_view_mode_transition(mode, None, cx);
    }

    pub(in crate::features) fn set_quick_command_view_mode_in_window(
        &mut self,
        mode: QuickCommandViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_quick_command_view_mode_transition(mode, Some(window), cx);
    }

    fn apply_quick_command_view_mode_transition(
        &mut self,
        mode: QuickCommandViewMode,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let old_mode = self.commands.quick_view_mode();
        let was_visible = self.quick_commands_is_visible();
        self.commands.set_quick_view_mode(mode);
        if old_mode != mode {
            if old_mode.is_bottom_panel()
                && self.shell.bottom_panel_mode() == BottomPanelMode::QuickCommands
            {
                self.shell
                    .set_bottom_panel_mode_silent(BottomPanelMode::Hidden);
            }
            if old_mode.is_command_bar() {
                self.shell.set_command_bar_visible(false);
            }
            if old_mode.is_sidebar() {
                self.close_quick_commands_side_panel();
            }

            if was_visible {
                match mode {
                    QuickCommandViewMode::List
                    | QuickCommandViewMode::Compact
                    | QuickCommandViewMode::Tile => {
                        self.shell
                            .set_bottom_panel_mode_silent(BottomPanelMode::QuickCommands);
                    }
                    QuickCommandViewMode::CommandBar => {
                        self.shell.set_command_bar_visible(true);
                        if let Some(window) = window {
                            self.focus_active_workspace_surface(window, cx);
                        }
                    }
                    QuickCommandViewMode::Sidebar => {
                        self.open_quick_commands_sidebar_surface(cx);
                    }
                }
            }
            self.persist_ui_layout();
        }
        self.settings
            .set_quick_command_view_mode(quick_command_view_mode_setting(mode).to_string());
        self.save_quick_command_ui_settings(cx);
        cx.notify();
    }

    pub(in crate::features) fn toggle_quick_command_pinned(
        &mut self,
        command_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(mut command) = self
            .commands
            .quick_commands()
            .iter()
            .find(|item| item.id == command_id)
            .cloned()
        else {
            return;
        };
        let next_pinned = !command.pinned.unwrap_or_default();
        command.pinned = next_pinned.then_some(true);
        command.updated_at = Some(unix_millis_now());
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                store.upsert_quick_command(command, None)
            }),
            |this, event, cx| {
                if let Ok(config) = event.outcome {
                    this.replace_quick_command_catalog(config.commands, config.categories, cx);
                } else {
                    this.refresh_quick_commands(cx);
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn move_quick_command_category_from_menu(
        &mut self,
        command_id: String,
        category_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(config) = self
            .commands
            .move_quick_command_to_category(&command_id, category_id)
        else {
            return;
        };
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Commands, move |store| {
                store.save_quick_commands(config)
            }),
            |this, event, cx| {
                if let Err(error) = event.outcome {
                    this.settings.update_store_status(
                        format!("quick command category update failed: {error}"),
                        false,
                    );
                    this.refresh_quick_commands(cx);
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn set_quick_command_sort_mode(
        &mut self,
        mode: QuickCommandSortMode,
        cx: &mut Context<Self>,
    ) {
        self.commands.set_quick_sort_mode(mode);
        self.settings
            .set_quick_command_sort_mode(quick_command_sort_mode_setting(mode).to_string());
        self.save_quick_command_ui_settings(cx);
    }

    pub(in crate::features) fn save_quick_command_ui_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.set_quick_command_view_mode(
            quick_command_view_mode_setting(self.commands.quick_view_mode()).to_string(),
        );
        self.settings.set_quick_command_sort_mode(
            quick_command_sort_mode_setting(self.commands.quick_sort_mode()).to_string(),
        );
        self.settings.set_quick_command_selected_category(
            self.commands.quick_selected_category().to_string(),
        );
        self.settings.set_quick_command_sidebar_expanded_categories(
            self.commands.quick_sidebar_expanded_category_ids_sorted(),
        );
        self.queue_settings_save(SettingsSaveKind::QuickCommands, cx);
    }

    pub(in crate::features) fn apply_quick_command_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        self.commands.set_quick_search_draft(text);
        cx.notify();
    }

    pub(in crate::features) fn toggle_quick_command_ai_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = self.commands.toggle_quick_ai_popover();
        if next {
            let prompt = self.commands.quick_ai_prompt_draft().to_string();
            let input = self.text_input(
                "quick-command.ai-prompt",
                &prompt,
                TextInputSetup::placeholder(t!("ai.placeholder")),
                cx,
            );
            window.focus(&input.read(cx).focus_handle(), cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn apply_quick_command_ai_prompt(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        self.commands.set_quick_ai_prompt_draft(text);
        cx.notify();
    }

    pub(in crate::features) fn submit_quick_command_ai_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.commands.take_quick_ai_prompt() else {
            self.shell
                .set_status("describe a command to generate".to_string());
            cx.notify();
            return;
        };
        self.reset_text_input("quick-command.ai-prompt", "", cx);
        self.close_quick_command_toolbar_popovers();
        self.set_ai_prompt_draft(format!("Generate a shell command for: {prompt}"), cx);
        self.ai
            .set_chat_response_preview("Quick command generation ready");
        self.ai.set_panel_status("quick command AI assist");
        self.ensure_panel_open(NavItem::AiAssistant, cx);
        window.focus(self.ai.chat_focus(), cx);
        cx.notify();
    }

    pub(in crate::features) fn handle_quick_command_ai_prompt_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event.keystroke.key.as_str() {
            "enter" => {
                self.submit_quick_command_ai_prompt(window, cx);
            }
            "escape" => {
                self.commands.close_quick_ai_popover();
                cx.notify();
            }
            _ => return false,
        }
        true
    }
}
