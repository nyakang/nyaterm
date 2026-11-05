use gpui::Context;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::text_inputs::TextInputSetup;
use nyaterm_core::SearchEngineConfig;

use super::helpers::urlencoding_query;

impl NyaTermApp {
    /// Build the two inputs an opened engine row draws.
    ///
    /// The row is the reveal boundary, so it is where they are created; the row's
    /// render only looks them up. The ids carry the row index, so adding or removing
    /// an engine forgets the whole prefix and this rebuilds for whatever row is open.
    /// Keep the ids in step with the lookups in `terminal::search`.
    pub(in crate::features) fn ensure_expanded_search_engine_inputs(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.settings.search_engine_presentation().expanded_index else {
            return;
        };
        let Some(engine) = self
            .settings
            .summary()
            .search_custom_engines
            .get(index)
            .cloned()
        else {
            return;
        };
        self.ensure_text_input(
            format!("settings.search-engine.{index}.name"),
            &engine.name,
            TextInputSetup::placeholder(t!("settings.engineName")),
            cx,
        );
        self.ensure_text_input(
            format!("settings.search-engine.{index}.url"),
            &engine.url_template,
            TextInputSetup::default(),
            cx,
        );
    }

    /// Apply an edit from one of the engine editor's inputs.
    ///
    /// `rest` is what follows `settings.search-engine.` in the field id:
    /// `<index>.name` or `<index>.url`.
    pub(in crate::features) fn apply_search_engine_input(
        &mut self,
        rest: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.apply_search_engine_input(rest, text) {
            return;
        }
        // Persist as it is typed, the way every other settings control does.
        // Trimming waits for the row to close, so a space mid-word survives.
        self.save_terminal_settings(cx);
        self.shell.set_status("search engine edited".to_string());
        cx.notify();
    }

    pub(in crate::features) fn add_search_engine(&mut self, cx: &mut Context<Self>) {
        self.settings.add_search_engine(SearchEngineConfig {
            name: "New Engine".to_string(),
            url_template: "https://example.com/search?q=%s".to_string(),
            icon: None,
            show_in_menu: true,
        });
        // The new row opens in the editor. Ids carry the row index, and every
        // existing row just shifted down, so the previous inputs are dropped
        // and rebuilt for the engine that is now open.
        self.forget_text_inputs("settings.search-engine.");
        self.ensure_expanded_search_engine_inputs(cx);
        self.save_terminal_settings(cx);
        self.shell.set_status("search engine added".to_string());
    }

    /// Close the engine editor so the list shows summary rows.
    ///
    /// A newly added engine opens for editing, but leaving this page must not
    /// leave that editor open. The next visit starts from the summary rows.
    pub(in crate::features) fn collapse_open_search_engine(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.settings.search_engine_presentation().expanded_index else {
            let icon_open = self
                .settings
                .search_engine_presentation()
                .icon_picker_index
                .is_some();
            self.settings.close_search_engine_menus();
            if icon_open {
                cx.notify();
            }
            return;
        };
        self.expand_search_engine(index, cx);
    }

    pub(in crate::features) fn remove_search_engine(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.remove_search_engine(index) {
            return;
        }
        self.forget_text_inputs("settings.search-engine.");
        self.ensure_expanded_search_engine_inputs(cx);
        self.save_terminal_settings(cx);
        self.shell.set_status("search engine removed".to_string());
    }

    pub(in crate::features) fn set_search_engine_icon(
        &mut self,
        index: usize,
        icon: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.set_search_engine_icon(index, icon) {
            return;
        }
        self.save_terminal_settings(cx);
        self.shell
            .set_status("search engine icon updated".to_string());
    }

    pub(in crate::features) fn toggle_search_engine_in_menu(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.toggle_search_engine_in_menu(index) {
            return;
        }
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn expand_search_engine(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(collapsed) = self.settings.toggle_search_engine_expanded(index) else {
            return;
        };
        if collapsed {
            // Closing the row normalizes the engine, so its inputs are dropped and a
            // later re-open rebuilds them from the trimmed values.
            self.forget_text_inputs("settings.search-engine.");
            self.save_terminal_settings(cx);
        } else {
            // Opening the row is what reveals its two fields, so it is what builds them.
            self.ensure_expanded_search_engine_inputs(cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn test_search_engine(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(engine) = self.settings.summary().search_custom_engines.get(index) else {
            self.shell.set_status("search engine not found".to_string());
            cx.notify();
            return;
        };
        if !engine.url_template.contains("%s") {
            self.shell
                .set_status("search engine URL must include %s".to_string());
            cx.notify();
            return;
        }
        let url = engine
            .url_template
            .replace("%s", &urlencoding_query("nyaterm"));
        let name = engine.name.clone();
        self.open_external_url_for_ui(&url, cx);
        self.shell
            .set_status(format!("tested search engine: {name}"));
    }

    pub(in crate::features) fn toggle_terminal_action_links(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_terminal_action_links();
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_terminal_action_links_matcher(
        &mut self,
        which: &'static str,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.toggle_terminal_action_link_matcher(which) {
            return;
        }
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn execute_action_link_command(
        &mut self,
        command: String,
        cx: &mut Context<Self>,
    ) {
        let mut bytes = command.into_bytes();
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        if self.send_terminal_input(bytes, cx) {
            self.shell
                .set_status("action link command sent".to_string());
            cx.notify();
        }
    }
}
