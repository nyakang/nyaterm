//! settings search_engines state and transitions.

use super::SettingsFeatureState;
use gpui::FocusHandle;
use nyaterm_core::SearchEngineConfig;

pub(super) struct SearchEngineSettingsState {
    pub(super) expanded_index: Option<usize>,
    pub(super) icon_picker_index: Option<usize>,
    pub(super) focus: FocusHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) struct SearchEnginePresentationState {
    pub expanded_index: Option<usize>,
    pub icon_picker_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum SearchEngineMenu {
    Icon,
}

impl SearchEngineSettingsState {
    pub(super) fn close_menus(&mut self) {
        self.icon_picker_index = None;
    }
}

pub(super) fn adjusted_index_after_remove(value: Option<usize>, removed: usize) -> Option<usize> {
    match value {
        Some(index) if index == removed => None,
        Some(index) if index > removed => Some(index - 1),
        other => other,
    }
}

impl SettingsFeatureState {
    pub(in crate::features) fn search_engine_presentation(&self) -> SearchEnginePresentationState {
        SearchEnginePresentationState {
            expanded_index: self.search_engines.expanded_index,
            icon_picker_index: self.search_engines.icon_picker_index,
        }
    }
    pub(in crate::features) fn search_engine_focus(&self) -> &FocusHandle {
        &self.search_engines.focus
    }
    pub(in crate::features) fn apply_search_engine_input(
        &mut self,
        rest: &str,
        text: String,
    ) -> bool {
        let Some((index, field)) = rest.split_once('.') else {
            return false;
        };
        let Ok(index) = index.parse::<usize>() else {
            return false;
        };
        let Some(engine) = self.summary.search_custom_engines.get_mut(index) else {
            return false;
        };
        match field {
            "name" => engine.name = text,
            "url" => engine.url_template = text,
            _ => return false,
        }
        true
    }
    pub(in crate::features) fn add_search_engine(&mut self, engine: SearchEngineConfig) {
        self.summary.search_custom_engines.insert(0, engine);
        // The new row opens for editing. Leaving the page collapses it, so a
        // later visit starts from the summary rows.
        self.search_engines.expanded_index = Some(0);
        self.search_engines.close_menus();
    }
    pub(in crate::features) fn remove_search_engine(&mut self, index: usize) -> bool {
        if index >= self.summary.search_custom_engines.len() {
            return false;
        }
        self.summary.search_custom_engines.remove(index);
        self.search_engines.close_menus();
        self.search_engines.expanded_index =
            adjusted_index_after_remove(self.search_engines.expanded_index, index);
        true
    }
    pub(in crate::features) fn set_search_engine_icon(
        &mut self,
        index: usize,
        icon: Option<&str>,
    ) -> bool {
        let Some(engine) = self.summary.search_custom_engines.get_mut(index) else {
            return false;
        };
        engine.icon = icon.map(str::to_string);
        self.search_engines.icon_picker_index = None;
        true
    }
    pub(in crate::features) fn toggle_search_engine_in_menu(&mut self, index: usize) -> bool {
        let Some(engine) = self.summary.search_custom_engines.get_mut(index) else {
            return false;
        };
        engine.show_in_menu = !engine.show_in_menu;
        true
    }
    /// Returns whether collapsing the row requires normalized values to be persisted.
    pub(in crate::features) fn toggle_search_engine_expanded(
        &mut self,
        index: usize,
    ) -> Option<bool> {
        if index >= self.summary.search_custom_engines.len() {
            return None;
        }
        let collapsed = self.search_engines.expanded_index == Some(index);
        if collapsed {
            self.search_engines.expanded_index = None;
            self.normalize_search_engines();
        } else {
            self.search_engines.expanded_index = Some(index);
        }
        self.search_engines.close_menus();
        Some(collapsed)
    }
    pub(in crate::features) fn toggle_search_engine_menu(
        &mut self,
        menu: SearchEngineMenu,
        index: usize,
    ) -> bool {
        if index >= self.summary.search_custom_engines.len() {
            return false;
        }
        let SearchEngineMenu::Icon = menu;
        let next = self.search_engines.icon_picker_index != Some(index);
        self.search_engines.close_menus();
        if next {
            self.search_engines.icon_picker_index = Some(index);
        }
        true
    }
    pub(in crate::features) fn close_search_engine_menus(&mut self) {
        self.search_engines.close_menus();
    }
    pub(in crate::features) fn normalize_search_engines(&mut self) {
        for engine in &mut self.summary.search_custom_engines {
            engine.name = engine.name.trim().to_string();
            engine.url_template = engine.url_template.trim().to_string();
        }
    }
}
