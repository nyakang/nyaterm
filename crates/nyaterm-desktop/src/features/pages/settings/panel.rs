use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rust_i18n::t;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity,
    Window, div, prelude::*, px, rgb,
};
use nyaterm_core::{
    AiSettings, AppSettingsSummary, CloudSyncSettings, CloudSyncState, KeywordHighlightConfig,
    TranslationSettings,
};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaInputShell, NyaNumberInputState, NyaSelect, NyaSelectOption,
    NyaSelectState, NyaSettingsLayout, NyaSettingsNavGroup, NyaSettingsNavItem,
};

use crate::features::selects::SelectRegistry;
use crate::features::settings::{
    KeybindingPresentationState, KeywordHighlightPresentationState, SearchEnginePresentationState,
};
use crate::features::text_inputs::number_input_box_from_state;
use crate::features::{
    FontAvailability, FontCatalogKind, FontCatalogLoadState, FontCatalogPresentation,
    FontResolutionStatus, NyaTermApp, normalize_font_family,
};
use crate::models::{
    AiActionEditorField, AiActionListKind, CloudSyncConflictState, CloudSyncSecretDraft,
    GithubGistAuthState, KeywordHighlightEditorField, SettingsTab, SnapshotPasswordPromptState,
    TranslationSecretDraft,
};
use crate::theme::ThemePalette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum SettingsSurface {
    MainPage,
    NativeWindow,
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct SettingsChrome {
    pub palette: ThemePalette,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum SettingsSectionPresentation {
    General,
    Appearance,
    Interaction,
    Keybindings,
    TerminalGeneral,
    Search,
    Translation,
    AiGeneral,
    AiModels,
    AiRules,
    Transfer,
    Security,
    SyncBackup,
}

#[derive(Clone)]
pub(in crate::features) struct SettingsSnapshot {
    pub chrome: SettingsChrome,
    pub surface: SettingsSurface,
    pub active_tab: SettingsTab,
    pub settings: SettingsPresentation,
    pub ai: AiSettingsPresentation,
    pub cloud_sync: CloudSyncPresentation,
    pub translation: TranslationPresentation,
    pub transfer: TransferSettingsPresentation,
    pub text_inputs: HashMap<SharedString, Entity<nyaterm_ui::NyaInputState>>,
    pub number_inputs: HashMap<SharedString, Entity<NyaNumberInputState>>,
    pub draft_open: bool,
    pub draft_dirty: bool,
    pub validation_error: Option<String>,
    pub backup_prompt: Option<SnapshotPasswordPromptState>,
    pub expanded_groups: Arc<[String]>,
    pub section: SettingsSectionPresentation,
}

impl PartialEq for SettingsSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.chrome == other.chrome
            && self.surface == other.surface
            && self.active_tab == other.active_tab
            && self.draft_open == other.draft_open
            && self.draft_dirty == other.draft_dirty
            && self.validation_error == other.validation_error
            && self.backup_prompt == other.backup_prompt
            && self.expanded_groups == other.expanded_groups
            && self.section == other.section
            // The input handles are compared too: a flush whose only change is a newly
            // built input must reach the panel, or the row that revealed it draws an
            // empty box.
            && self.text_inputs == other.text_inputs
            && self.number_inputs == other.number_inputs
            && self.active_section_eq(other)
    }
}

impl SettingsSnapshot {
    fn active_section_eq(&self, other: &Self) -> bool {
        match self.active_tab {
            SettingsTab::AiGeneral | SettingsTab::AiModels | SettingsTab::AiRules => {
                self.ai == other.ai
            }
            SettingsTab::SyncBackup => {
                self.settings == other.settings && self.cloud_sync == other.cloud_sync
            }
            SettingsTab::Translation => self.translation == other.translation,
            SettingsTab::Transfer => {
                self.settings == other.settings && self.transfer == other.transfer
            }
            SettingsTab::General
            | SettingsTab::Appearance
            | SettingsTab::Interaction
            | SettingsTab::Keybindings
            | SettingsTab::TerminalGeneral
            | SettingsTab::Search
            | SettingsTab::Security => self.settings == other.settings,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct SettingsPresentation {
    pub(in crate::features) summary: Arc<AppSettingsSummary>,
    pub(in crate::features) keyword_config: Arc<KeywordHighlightConfig>,
    pub(in crate::features) search_engine_presentation: SearchEnginePresentationState,
    pub(in crate::features) keyword_highlight_presentation: KeywordHighlightPresentationState,
    pub(in crate::features) keybinding_presentation: KeybindingPresentationState,
    pub(in crate::features) master_password: MasterPasswordPresentation,
    pub(in crate::features) font_catalog: FontCatalogPresentation,
    pub(in crate::features) search_engine_focus: gpui::FocusHandle,
    pub(in crate::features) keyword_highlight_focus: gpui::FocusHandle,
    pub(in crate::features) keybinding_focus: gpui::FocusHandle,
    pub(in crate::features) snapshot_password_prompt: Option<SnapshotPasswordPromptState>,
    pub(in crate::features) snapshot_password_prompt_active: bool,
    pub(in crate::features) config_path_prompt_active: bool,
    pub(in crate::features) terminal_theme_is_dark: bool,
    pub(in crate::features) panel_multi_open: bool,
}

impl SettingsPresentation {
    fn empty(cx: &mut Context<SettingsPanel>) -> Self {
        Self {
            summary: Arc::new(AppSettingsSummary::default()),
            keyword_config: Arc::new(KeywordHighlightConfig::default()),
            search_engine_presentation: SearchEnginePresentationState {
                expanded_index: None,
                icon_picker_index: None,
                actions_index: None,
            },
            keyword_highlight_presentation: KeywordHighlightPresentationState {
                expanded_id: None,
                edit_id: None,
                edit_field: KeywordHighlightEditorField::Name,
            },
            keybinding_presentation: KeybindingPresentationState {
                recording_id: None,
                pending_keys: None,
                search_draft: String::new(),
            },
            master_password: MasterPasswordPresentation::default(),
            font_catalog: FontCatalogPresentation::empty(),
            search_engine_focus: cx.focus_handle(),
            keyword_highlight_focus: cx.focus_handle(),
            keybinding_focus: cx.focus_handle(),
            snapshot_password_prompt: None,
            snapshot_password_prompt_active: false,
            config_path_prompt_active: false,
            terminal_theme_is_dark: true,
            panel_multi_open: false,
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct MasterPasswordPresentation {
    pub enabled: bool,
    pub draft: nyaterm_core::SecretString,
}

impl SettingsPresentation {
    pub(in crate::features) fn summary(&self) -> &AppSettingsSummary {
        &self.summary
    }

    pub(in crate::features) fn keyword_config(&self) -> &KeywordHighlightConfig {
        &self.keyword_config
    }

    pub(in crate::features) fn search_engine_presentation(&self) -> SearchEnginePresentationState {
        self.search_engine_presentation
    }

    pub(in crate::features) fn keyword_highlight_presentation(
        &self,
    ) -> KeywordHighlightPresentationState {
        self.keyword_highlight_presentation.clone()
    }

    pub(in crate::features) fn keybinding_presentation(&self) -> KeybindingPresentationState {
        self.keybinding_presentation.clone()
    }

    pub(in crate::features) fn master_password(&self) -> &MasterPasswordPresentation {
        &self.master_password
    }

    pub(in crate::features) fn font_catalog_state(&self) -> FontCatalogLoadState {
        self.font_catalog.state()
    }

    pub(in crate::features) fn font_availability(
        &self,
        family: &str,
        terminal: bool,
    ) -> FontAvailability {
        self.font_catalog.availability(
            family,
            if terminal {
                FontCatalogKind::Terminal
            } else {
                FontCatalogKind::Ui
            },
        )
    }

    pub(in crate::features) fn resolve_font_stack(
        &self,
        families: &[String],
        terminal: bool,
        platform_default: &str,
    ) -> Option<FontResolutionStatus> {
        self.font_catalog.resolve_stack(
            families,
            if terminal {
                FontCatalogKind::Terminal
            } else {
                FontCatalogKind::Ui
            },
            platform_default,
        )
    }

    pub(in crate::features) fn search_engine_focus(&self) -> &gpui::FocusHandle {
        &self.search_engine_focus
    }

    pub(in crate::features) fn keyword_highlight_focus(&self) -> &gpui::FocusHandle {
        &self.keyword_highlight_focus
    }

    pub(in crate::features) fn keybinding_focus(&self) -> &gpui::FocusHandle {
        &self.keybinding_focus
    }

    pub(in crate::features) fn snapshot_password_prompt(
        &self,
    ) -> Option<SnapshotPasswordPromptState> {
        self.snapshot_password_prompt.clone()
    }

    pub(in crate::features) fn snapshot_password_prompt_active(&self) -> bool {
        self.snapshot_password_prompt_active
    }

    pub(in crate::features) fn config_path_prompt_active(&self) -> bool {
        self.config_path_prompt_active
    }

    pub(in crate::features) fn terminal_theme_is_dark(&self) -> bool {
        self.terminal_theme_is_dark
    }

    pub(in crate::features) fn panel_multi_open(&self) -> bool {
        self.panel_multi_open
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct AiSettingsPresentation {
    pub(in crate::features) config: Arc<AiSettings>,
    pub(in crate::features) model_query: String,
    pub(in crate::features) model_collapsed_groups: Arc<HashSet<String>>,
    pub(in crate::features) manual_model_drafts: Arc<HashMap<String, String>>,
    pub(in crate::features) credential_secret_drafts: Arc<HashMap<String, String>>,
    pub(in crate::features) action_focus: gpui::FocusHandle,
    pub(in crate::features) discovery_pending: bool,
}

impl AiSettingsPresentation {
    fn empty(cx: &mut Context<SettingsPanel>) -> Self {
        Self {
            config: Arc::new(AiSettings::default()),
            model_query: String::new(),
            model_collapsed_groups: Arc::new(HashSet::new()),
            manual_model_drafts: Arc::new(HashMap::new()),
            credential_secret_drafts: Arc::new(HashMap::new()),
            action_focus: cx.focus_handle(),
            discovery_pending: false,
        }
    }
}

impl AiSettingsPresentation {
    pub(in crate::features) fn settings_config(&self) -> &AiSettings {
        &self.config
    }

    pub(in crate::features) fn settings_model_query(&self) -> &str {
        &self.model_query
    }

    pub(in crate::features) fn settings_model_collapsed_groups(&self) -> &HashSet<String> {
        &self.model_collapsed_groups
    }

    pub(in crate::features) fn settings_manual_model_drafts(&self) -> &HashMap<String, String> {
        &self.manual_model_drafts
    }

    pub(in crate::features) fn settings_credential_secret_drafts(
        &self,
    ) -> &HashMap<String, String> {
        &self.credential_secret_drafts
    }

    pub(in crate::features) fn settings_action_focus(&self) -> &gpui::FocusHandle {
        &self.action_focus
    }

    pub(in crate::features) fn discovery_is_pending(&self) -> bool {
        self.discovery_pending
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct CloudSyncPresentation {
    pub(in crate::features) settings: Arc<CloudSyncSettings>,
    pub(in crate::features) state: CloudSyncState,
    pub(in crate::features) pending_settings: CloudSyncSettings,
    pub(in crate::features) secret_draft: CloudSyncSecretDraft,
    pub(in crate::features) status: String,
    pub(in crate::features) job_running: bool,
    pub(in crate::features) conflict: Option<CloudSyncConflictState>,
    pub(in crate::features) github_auth: GithubGistAuthState,
}

impl Default for CloudSyncPresentation {
    fn default() -> Self {
        Self {
            settings: Arc::new(CloudSyncSettings::default()),
            state: CloudSyncState::default(),
            pending_settings: CloudSyncSettings::default(),
            secret_draft: CloudSyncSecretDraft::default(),
            status: String::new(),
            job_running: false,
            conflict: None,
            github_auth: GithubGistAuthState::default(),
        }
    }
}

impl CloudSyncPresentation {
    pub(in crate::features) fn settings(&self) -> &CloudSyncSettings {
        &self.settings
    }

    pub(in crate::features) fn state(&self) -> &CloudSyncState {
        &self.state
    }

    pub(in crate::features) fn pending_settings(&self) -> CloudSyncSettings {
        self.pending_settings.clone()
    }

    pub(in crate::features) fn secret_draft(&self) -> &CloudSyncSecretDraft {
        &self.secret_draft
    }

    pub(in crate::features) fn status(&self) -> &str {
        &self.status
    }

    pub(in crate::features) fn job_running(&self) -> bool {
        self.job_running
    }

    pub(in crate::features) fn conflict(&self) -> Option<&CloudSyncConflictState> {
        self.conflict.as_ref()
    }

    pub(in crate::features) fn github_auth(&self) -> &GithubGistAuthState {
        &self.github_auth
    }
}

#[derive(Clone, Default, PartialEq)]
pub(in crate::features) struct TranslationPresentation {
    pub(in crate::features) settings: TranslationSettings,
    pub(in crate::features) secret_draft: TranslationSecretDraft,
}

impl TranslationPresentation {
    pub(in crate::features) fn settings_draft_snapshot(
        &self,
    ) -> (TranslationSettings, TranslationSecretDraft) {
        (self.settings.clone(), self.secret_draft.clone())
    }
}

#[derive(Clone, Default, PartialEq)]
pub(in crate::features) struct TransferSettingsPresentation {
    pub(in crate::features) duplicate_policy: nyaterm_transport::SftpDuplicatePolicy,
}

impl TransferSettingsPresentation {
    pub(in crate::features) fn duplicate_policy(&self) -> nyaterm_transport::SftpDuplicatePolicy {
        self.duplicate_policy
    }
}

struct FontSelectOptionCache {
    state: Option<FontCatalogLoadState>,
    generation: Option<u64>,
    options: Arc<[NyaSelectOption]>,
    normalized_options: Arc<HashMap<String, String>>,
}

impl FontSelectOptionCache {
    fn empty() -> Self {
        Self {
            state: None,
            generation: None,
            options: Arc::from(Vec::<NyaSelectOption>::new()),
            normalized_options: Arc::new(HashMap::new()),
        }
    }

    fn update(&mut self, state: FontCatalogLoadState, generation: u64, source: &[String]) {
        // Settings snapshots can change for unrelated fields while the system font catalog stays
        // the same. Reuse the descriptors across those renders instead of rebuilding them per
        // configured fallback row.
        if self.state == Some(state) && self.generation == Some(generation) {
            return;
        }

        let options: Arc<[NyaSelectOption]> = source
            .iter()
            .map(|option| {
                NyaSelectOption::new(option.clone(), option.clone()).font_family(option.clone())
            })
            .collect::<Vec<_>>()
            .into();
        let normalized_options = source
            .iter()
            .map(|option| (normalize_font_family(option), option.clone()))
            .collect();

        self.state = Some(state);
        self.generation = Some(generation);
        self.options = options;
        self.normalized_options = Arc::new(normalized_options);
    }
}

pub(in crate::features) struct SettingsPanel {
    app: WeakEntity<NyaTermApp>,
    surface: SettingsSurface,
    snapshot: Option<SettingsSnapshot>,
    pub(in crate::features) settings: SettingsPresentation,
    pub(in crate::features) ai: AiSettingsPresentation,
    pub(in crate::features) cloud_sync: CloudSyncPresentation,
    pub(in crate::features) translation: TranslationPresentation,
    pub(in crate::features) transfer: TransferSettingsPresentation,
    text_inputs: HashMap<SharedString, Entity<nyaterm_ui::NyaInputState>>,
    number_inputs: HashMap<SharedString, Entity<NyaNumberInputState>>,
    selects: SelectRegistry,
    select_subscriptions: Vec<Subscription>,
    ui_font_select_options: FontSelectOptionCache,
    terminal_font_select_options: FontSelectOptionCache,
    #[cfg(test)]
    paint_count: usize,
    #[cfg(test)]
    rebuild_count: usize,
}

impl SettingsPanel {
    pub(in crate::features) fn new(app: WeakEntity<NyaTermApp>, cx: &mut Context<Self>) -> Self {
        Self::new_for_surface(app, SettingsSurface::MainPage, cx)
    }

    pub(in crate::features) fn new_for_surface(
        app: WeakEntity<NyaTermApp>,
        surface: SettingsSurface,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            surface,
            snapshot: None,
            settings: SettingsPresentation::empty(cx),
            ai: AiSettingsPresentation::empty(cx),
            cloud_sync: CloudSyncPresentation::default(),
            translation: TranslationPresentation::default(),
            transfer: TransferSettingsPresentation::default(),
            text_inputs: HashMap::new(),
            number_inputs: HashMap::new(),
            selects: SelectRegistry::default(),
            select_subscriptions: Vec::new(),
            ui_font_select_options: FontSelectOptionCache::empty(),
            terminal_font_select_options: FontSelectOptionCache::empty(),
            #[cfg(test)]
            paint_count: 0,
            #[cfg(test)]
            rebuild_count: 0,
        }
    }

    #[cfg(test)]
    pub(in crate::features) fn surface(&self) -> SettingsSurface {
        self.surface
    }

    #[cfg(test)]
    pub(in crate::features) fn snapshot(&self) -> Option<&SettingsSnapshot> {
        self.snapshot.as_ref()
    }

    #[cfg(test)]
    pub(in crate::features) fn paint_count(&self) -> usize {
        self.paint_count
    }

    pub(in crate::features) fn set_snapshot(
        &mut self,
        snapshot: SettingsSnapshot,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        self.surface = snapshot.surface;
        self.settings = snapshot.settings.clone();
        self.ai = snapshot.ai.clone();
        self.cloud_sync = snapshot.cloud_sync.clone();
        self.translation = snapshot.translation.clone();
        self.transfer = snapshot.transfer.clone();
        self.text_inputs = snapshot.text_inputs.clone();
        self.number_inputs = snapshot.number_inputs.clone();
        self.snapshot = Some(snapshot);
        #[cfg(test)]
        {
            self.rebuild_count += 1;
        }
        cx.notify();
    }

    pub(in crate::features::pages::settings) fn with_app<R: Default>(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>) -> R,
    ) -> R {
        let Some(app) = self.app.upgrade() else {
            return R::default();
        };
        app.update(cx, |app, cx| {
            let result = f(app, cx);
            app.request_settings_panel_refresh(cx);
            result
        })
    }

    pub(in crate::features::pages::settings) fn mcp_host_status(
        &self,
        cx: &Context<Self>,
    ) -> crate::features::mcp::McpHostStatus {
        self.app
            .upgrade()
            .map(|app| app.read(cx).mcp_host_status())
            .unwrap_or(crate::features::mcp::McpHostStatus::Unavailable)
    }

    pub(in crate::features::pages::settings) fn mcp_helper_status(
        &self,
        cx: &Context<Self>,
    ) -> crate::features::ai::McpHelperStatus {
        self.app
            .upgrade()
            .map(|app| app.read(cx).mcp_helper_status())
            .unwrap_or(crate::features::ai::McpHelperStatus::Missing)
    }

    pub(in crate::features::pages::settings) fn form_select_control<I>(
        &mut self,
        id: I,
        options: Vec<NyaSelectOption>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I>
    where
        I: Into<SharedString>,
    {
        let id = id.into();
        let select = self.select_entity(id.clone(), options, selected_value, disabled, cx);

        div()
            .id(id)
            .w_full()
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .child(NyaSelect::new(&select))
    }

    pub(in crate::features::pages::settings) fn font_select_option_catalog(
        &mut self,
        terminal: bool,
    ) -> (Arc<[NyaSelectOption]>, Arc<HashMap<String, String>>) {
        let source = if terminal {
            self.settings.font_catalog.terminal_options()
        } else {
            self.settings.font_catalog.ui_options()
        };
        let state = self.settings.font_catalog.state();
        let generation = self.settings.font_catalog.generation();
        let cache = if terminal {
            &mut self.terminal_font_select_options
        } else {
            &mut self.ui_font_select_options
        };
        cache.update(state, generation, source);
        (
            Arc::clone(&cache.options),
            Arc::clone(&cache.normalized_options),
        )
    }

    pub(in crate::features::pages::settings) fn select_control<I>(
        &mut self,
        id: I,
        options: Vec<NyaSelectOption>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I>
    where
        I: Into<SharedString>,
    {
        let id = id.into();
        let select = self.select_entity(id.clone(), options, selected_value, disabled, cx);

        div()
            .id(id)
            .w_full()
            .max_w(px(360.))
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .child(NyaSelect::new(&select))
    }

    pub(in crate::features::pages::settings) fn font_select_control<I>(
        &mut self,
        id: I,
        options: Arc<[NyaSelectOption]>,
        selected_value: Option<String>,
        placeholder_content: Option<AnyElement>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I>
    where
        I: Into<SharedString>,
    {
        let id = id.into();
        let placeholder = selected_value.clone().unwrap_or_default();
        let select = self.select_entity_shared(id.clone(), options, selected_value, disabled, cx);
        select.update(cx, |state, cx| state.set_placeholder(placeholder, cx));
        let mut select_element = NyaSelect::new(&select).appearance(true);
        if let Some(content) = placeholder_content {
            select_element = select_element.placeholder_content(content);
        }
        div()
            .id(id)
            .w_full()
            .max_w(px(360.))
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .child(select_element)
    }

    pub(in crate::features::pages::settings) fn settings_select_control<I, S>(
        &mut self,
        id: I,
        options: Vec<NyaSelectOption>,
        selected_value: S,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I, S>
    where
        I: Into<SharedString>,
        S: Into<String>,
    {
        div()
            .w(px(260.))
            .max_w_full()
            .child(self.form_select_control(id, options, Some(selected_value.into()), disabled, cx))
    }

    pub(in crate::features::pages::settings) fn settings_select_field<I, L, S>(
        &mut self,
        id: I,
        label: L,
        desc: Option<SharedString>,
        select: (Vec<NyaSelectOption>, S, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I, L, S>
    where
        I: Into<SharedString>,
        L: Into<SharedString>,
        S: Into<String>,
    {
        let (options, selected_value, disabled) = select;
        let palette = self.theme_palette();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight(500.))
                    .text_color(rgb(palette.text))
                    .child(label.into()),
            )
            .when_some(desc, |this, desc| {
                this.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(desc),
                )
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(576.))
                    .child(self.form_select_control(
                        id,
                        options,
                        Some(selected_value.into()),
                        disabled,
                        cx,
                    )),
            )
    }

    pub(in crate::features) fn theme_palette(&self) -> ThemePalette {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.chrome.palette)
            .expect("settings panel render requires a snapshot")
    }

    /// An existing field, without creating one.
    ///
    /// The panel only ever looks inputs up: the entities are owned by `NyaTermApp`'s
    /// registry and arrive here through the snapshot, so the boundary that reveals a
    /// field is the boundary that builds it.
    pub(in crate::features) fn existing_text_input(
        &self,
        id: impl AsRef<str>,
    ) -> Option<Entity<nyaterm_ui::NyaInputState>> {
        self.text_inputs.get(id.as_ref()).cloned()
    }

    /// The text input for `id`, which an activation boundary must already have built.
    ///
    /// Renders nothing if it is missing rather than creating one. That would be a bug
    /// -- an input a tab or a revealed row draws but its ensure list forgot -- so it
    /// trips a debug assertion, and the
    /// `every_settings_surface_draws_only_inputs_it_built` test drives every tab and
    /// every reveal boundary to catch it before a release build ever sees a gap.
    pub(in crate::features) fn existing_text_input_box(
        &self,
        id: impl Into<SharedString>,
        multi_line: bool,
    ) -> AnyElement {
        let id = id.into();
        match self.existing_text_input(&id) {
            Some(field) => {
                let shell = NyaInputShell::new(id, &field);
                if multi_line {
                    shell.multi_line().into_any_element()
                } else {
                    shell.into_any_element()
                }
            }
            None => {
                debug_assert!(false, "settings text input {id} was never built");
                div().into_any_element()
            }
        }
    }

    /// A caption above the input for `id`, looked up rather than built.
    pub(in crate::features) fn existing_text_input_field(
        &self,
        id: impl Into<SharedString>,
        caption: impl Into<SharedString>,
        multi_line: bool,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let caption = caption.into();
        let input = self.existing_text_input_box(id, multi_line);
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .when(!caption.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(caption),
                )
            })
            .child(input)
            .into_any_element()
    }

    /// The number input for `id`, looked up rather than built.
    ///
    /// Its range and disabled flag were frozen when the ensure boundary created it:
    /// `NyaTermApp::number_input` returns early for an id it already has.
    pub(in crate::features) fn existing_number_input_box(
        &self,
        id: impl Into<SharedString>,
    ) -> AnyElement {
        let id = id.into();
        let palette = self.theme_palette();
        match self.number_inputs.get(id.as_ref()).cloned() {
            Some(field) => number_input_box_from_state(id, palette, field).into_any_element(),
            None => {
                debug_assert!(false, "settings number input {id} was never built");
                div().into_any_element()
            }
        }
    }

    fn select_entity(
        &mut self,
        id: SharedString,
        options: Vec<NyaSelectOption>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState> {
        let select = if let Some(select) = self.selects.field(&id) {
            select
        } else {
            self.create_select_entity(
                id.clone(),
                options.clone(),
                selected_value.clone(),
                disabled,
                cx,
            )
        };

        select.update(cx, |select, cx| {
            select.set_options(options, cx);
            select.set_selected_value(selected_value, cx);
            select.set_disabled(disabled, cx);
        });
        select
    }

    fn select_entity_shared(
        &mut self,
        id: SharedString,
        options: Arc<[NyaSelectOption]>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState> {
        let select = if let Some(select) = self.selects.field(&id) {
            select
        } else {
            self.create_select_entity_shared(
                id.clone(),
                Arc::clone(&options),
                selected_value.clone(),
                disabled,
                cx,
            )
        };

        select.update(cx, |select, cx| {
            select.set_options_shared(options, cx);
            select.set_selected_value(selected_value, cx);
            select.set_disabled(disabled, cx);
        });
        select
    }

    fn create_select_entity(
        &mut self,
        id: SharedString,
        options: Vec<NyaSelectOption>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState> {
        let select =
            cx.new(|cx| NyaSelectState::new(cx, options, selected_value).disabled(disabled));
        self.register_select_entity(id, select, cx)
    }

    fn create_select_entity_shared(
        &mut self,
        id: SharedString,
        options: Arc<[NyaSelectOption]>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState> {
        let select =
            cx.new(|cx| NyaSelectState::new_shared(cx, options, selected_value).disabled(disabled));
        self.register_select_entity(id, select, cx)
    }

    fn register_select_entity(
        &mut self,
        id: SharedString,
        select: Entity<NyaSelectState>,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState> {
        let subscription_id = id.clone();
        let subscription =
            cx.subscribe(
                &select,
                move |panel: &mut SettingsPanel, _, event, cx| match event {
                    nyaterm_ui::NyaSelectEvent::Changed(value) => {
                        panel.with_app(cx, |app, cx| {
                            app.on_select_changed(&subscription_id, value.as_deref(), cx);
                        });
                    }
                },
            );
        self.selects.insert_field(id, select.clone());
        self.select_subscriptions.push(subscription);
        select
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.paint_count += 1;
        }

        let Some(snapshot) = self.snapshot.clone() else {
            return div().size_full().into_any_element();
        };

        let palette = snapshot.chrome.palette;
        let settings_title = t!("settings.title");
        let active_group = t!(snapshot.active_tab.group_i18n_key());
        let active_label = t!(snapshot.active_tab.i18n_key());
        let back_label = t!("common.close");
        let native_window = snapshot.surface == SettingsSurface::NativeWindow;

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(palette.bg))
            .when(!native_window, |this| {
                this.child(
                    div()
                        .h(px(36.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .border_b_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.section_header))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight(700.))
                                        .text_color(rgb(palette.text))
                                        .child(settings_title),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(palette.text_dimmed))
                                        .child("·"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(palette.text_muted))
                                        .child(active_group),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(palette.text_dimmed))
                                        .child("/"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight(600.))
                                        .text_color(rgb(palette.text))
                                        .child(active_label.clone()),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from("settings-close"))
                                .h(px(26.))
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_muted))
                                .cursor_pointer()
                                .hover(move |this| {
                                    this.bg(rgb(palette.surface_elevated))
                                        .text_color(rgb(palette.text))
                                })
                                .child(back_label)
                                .on_click(cx.listener(|panel, _, _, cx| {
                                    panel.with_app(cx, |app, cx| app.close_settings(cx));
                                })),
                        ),
                )
            })
            .child(self.settings_layout(&snapshot, window, cx))
            .child(self.settings_action_footer(&snapshot, cx))
            .into_any_element()
    }
}

impl SettingsPanel {
    fn settings_layout(
        &mut self,
        snapshot: &SettingsSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let viewport_width = f32::from(window.viewport_size().width);
        let active_item_id = settings_tab_nav_id(snapshot.active_tab);
        let active_label = t!(snapshot.active_tab.i18n_key());
        let content = self.settings_tab_content(snapshot, cx);
        let panel = cx.entity();
        let toggle_panel = panel.clone();

        NyaSettingsLayout::new(
            "settings-layout",
            self.settings_nav_groups(snapshot),
            active_item_id,
            content,
        )
        .palette(snapshot.chrome.palette)
        .active_title(active_label)
        .sidebar_title(t!("settings.title"))
        .viewport_width(viewport_width)
        .compact_breakpoint(640.)
        .wide_breakpoint(1024.)
        .on_select(move |item_id, _, cx| {
            if let Some(tab) = settings_tab_from_nav_id(item_id.as_ref()) {
                panel.update(cx, |panel, cx| {
                    panel.with_app(cx, |app, cx| {
                        if tab == SettingsTab::Appearance {
                            app.ensure_appearance_font_options(cx);
                        }
                        app.ensure_settings_tab_inputs(tab, cx);
                        app.shell.set_settings_active_tab(tab);
                    });
                });
            }
        })
        .on_toggle_group(move |group_id, _, cx| {
            toggle_panel.update(cx, |panel, cx| {
                panel.with_app(cx, |app, _| {
                    app.shell.toggle_settings_group(group_id.to_string());
                });
            });
        })
    }

    fn settings_nav_groups(&self, snapshot: &SettingsSnapshot) -> Vec<NyaSettingsNavGroup> {
        let palette = snapshot.chrome.palette;
        vec![
            NyaSettingsNavGroup::new(
                "workspace",
                t!("settings.groupWorkspace"),
                "icons/workspace.svg",
            )
            .accent(palette.link)
            .expanded(snapshot.group_is_expanded("workspace"))
            .items([
                settings_nav_item(SettingsTab::General),
                settings_nav_item(SettingsTab::Appearance),
                settings_nav_item(SettingsTab::Interaction),
                settings_nav_item(SettingsTab::Keybindings),
            ]),
            NyaSettingsNavGroup::new(
                "terminal_session",
                t!("settings.groupTerminalSession"),
                "icons/conn/terminal.svg",
            )
            .accent(palette.success)
            .expanded(snapshot.group_is_expanded("terminal_session"))
            .items([
                settings_nav_item(SettingsTab::TerminalGeneral),
                settings_nav_item(SettingsTab::Search),
                settings_nav_item(SettingsTab::Translation),
            ]),
            NyaSettingsNavGroup::new("ai_group", t!("ai.title"), "icons/ai.svg")
                .accent(0xbc8cff)
                .expanded(snapshot.group_is_expanded("ai_group"))
                .items([
                    settings_nav_item(SettingsTab::AiGeneral),
                    settings_nav_item(SettingsTab::AiModels),
                    settings_nav_item(SettingsTab::AiRules),
                ]),
            NyaSettingsNavGroup::standalone([
                settings_nav_item(SettingsTab::Transfer),
                settings_nav_item(SettingsTab::Security),
                settings_nav_item(SettingsTab::SyncBackup),
            ]),
        ]
    }

    fn settings_tab_content(
        &mut self,
        snapshot: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match snapshot.active_tab {
            SettingsTab::General => self.general_settings_section(cx).into_any_element(),
            SettingsTab::Appearance => self.appearance_settings_section(cx).into_any_element(),
            SettingsTab::Interaction => self.interaction_settings_section(cx).into_any_element(),
            SettingsTab::Keybindings => self.keybindings_settings_section(cx).into_any_element(),
            SettingsTab::TerminalGeneral => self
                .terminal_general_settings_section(cx)
                .into_any_element(),
            SettingsTab::Search => self.terminal_search_settings_section(cx).into_any_element(),
            SettingsTab::Translation => self.translation_settings_section(cx).into_any_element(),
            SettingsTab::AiGeneral => self.ai_settings_section(cx).into_any_element(),
            SettingsTab::AiModels => self.ai_models_settings_section(cx).into_any_element(),
            SettingsTab::AiRules => self.ai_rules_settings_section(cx).into_any_element(),
            SettingsTab::Transfer => self.transfer_settings_section(cx).into_any_element(),
            SettingsTab::Security => self.security_settings_section(cx).into_any_element(),
            SettingsTab::SyncBackup => self.cloud_sync_settings_section(cx).into_any_element(),
        }
    }

    fn settings_action_footer(
        &mut self,
        snapshot: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = snapshot.chrome.palette;
        let dirty = snapshot.draft_dirty;
        let validation_error = snapshot.validation_error.clone();
        let apply_disabled = !dirty || validation_error.is_some();
        let confirm_disabled = validation_error.is_some();
        let status = validation_error.clone().unwrap_or_else(|| {
            t!(if dirty {
                "fileEditor.unsavedDesc"
            } else {
                "updater.noUpdate"
            })
            .to_string()
        });
        let cancel_label = t!("common.cancel");
        let apply_label = t!("common.apply");
        let confirm_label = t!("common.confirm");

        div()
            .h(px(52.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_5()
            .py_3()
            .border_t_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.section_header))
            .child(
                div()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(if validation_error.is_some() {
                        rgb(palette.warning)
                    } else {
                        rgb(palette.text_muted)
                    })
                    .child(status),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("settings-cancel")
                            .h(px(28.))
                            .min_w(px(64.))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .text_size(px(11.))
                            .text_color(rgb(palette.text))
                            .cursor_pointer()
                            .hover(move |this| this.bg(rgb(palette.hover)))
                            .child(cancel_label)
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.with_app(cx, |app, cx| app.cancel_settings(cx));
                            })),
                    )
                    .child(
                        div()
                            .id("settings-apply")
                            .h(px(28.))
                            .min_w(px(64.))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .text_size(px(11.))
                            .text_color(if apply_disabled {
                                rgb(palette.text_dimmed)
                            } else {
                                rgb(palette.text)
                            })
                            .when(!apply_disabled, |this| {
                                this.cursor_pointer()
                                    .hover(move |this| this.bg(rgb(palette.hover)))
                                    .on_click(cx.listener(|panel, _, _, cx| {
                                        panel.with_app(cx, |app, cx| {
                                            app.apply_settings_draft(false, cx);
                                        });
                                    }))
                            })
                            .child(apply_label),
                    )
                    .child(
                        div()
                            .id("settings-confirm")
                            .h(px(28.))
                            .min_w(px(64.))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(if confirm_disabled {
                                rgb(palette.surface_elevated)
                            } else {
                                rgb(palette.link)
                            })
                            .text_size(px(11.))
                            .font_weight(FontWeight(600.))
                            .text_color(if confirm_disabled {
                                rgb(palette.text_dimmed)
                            } else {
                                rgb(0xffffff)
                            })
                            .when(!confirm_disabled, |this| {
                                this.cursor_pointer()
                                    .on_click(cx.listener(|panel, _, _, cx| {
                                        panel.with_app(cx, |app, cx| {
                                            app.confirm_settings_draft(cx);
                                        });
                                    }))
                            })
                            .child(confirm_label),
                    ),
            )
    }
}

impl SettingsSnapshot {
    fn group_is_expanded(&self, group: &str) -> bool {
        self.expanded_groups
            .iter()
            .any(|expanded| expanded == group)
    }
}

fn settings_nav_item(tab: SettingsTab) -> NyaSettingsNavItem {
    NyaSettingsNavItem::new(
        settings_tab_nav_id(tab),
        t!(tab.i18n_key()),
        tab.icon_path(),
    )
}

fn settings_tab_nav_id(tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::General => "settings-tab-general",
        SettingsTab::Appearance => "settings-tab-appearance",
        SettingsTab::Interaction => "settings-tab-interaction",
        SettingsTab::Keybindings => "settings-tab-keybindings",
        SettingsTab::TerminalGeneral => "settings-tab-terminal-general",
        SettingsTab::Search => "settings-tab-search",
        SettingsTab::Translation => "settings-tab-translation",
        SettingsTab::AiGeneral => "settings-tab-ai-general",
        SettingsTab::AiModels => "settings-tab-ai-models",
        SettingsTab::AiRules => "settings-tab-ai-rules",
        SettingsTab::Transfer => "settings-tab-transfer",
        SettingsTab::Security => "settings-tab-security",
        SettingsTab::SyncBackup => "settings-tab-sync-backup",
    }
}

fn settings_tab_from_nav_id(id: &str) -> Option<SettingsTab> {
    match id {
        "settings-tab-general" => Some(SettingsTab::General),
        "settings-tab-appearance" => Some(SettingsTab::Appearance),
        "settings-tab-interaction" => Some(SettingsTab::Interaction),
        "settings-tab-keybindings" => Some(SettingsTab::Keybindings),
        "settings-tab-terminal-general" => Some(SettingsTab::TerminalGeneral),
        "settings-tab-search" => Some(SettingsTab::Search),
        "settings-tab-translation" => Some(SettingsTab::Translation),
        "settings-tab-ai-general" => Some(SettingsTab::AiGeneral),
        "settings-tab-ai-models" => Some(SettingsTab::AiModels),
        "settings-tab-ai-rules" => Some(SettingsTab::AiRules),
        "settings-tab-transfer" => Some(SettingsTab::Transfer),
        "settings-tab-security" => Some(SettingsTab::Security),
        "settings-tab-sync-backup" => Some(SettingsTab::SyncBackup),
        _ => None,
    }
}

impl SettingsPanel {
    pub(in crate::features) fn settings_draft_dirty(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.draft_dirty)
    }

    pub(in crate::features) fn cloud_sync_form_enabled(&self) -> bool {
        let master_password = self.settings.master_password();
        master_password.enabled
            && (self.settings.summary().has_master_password || !master_password.draft.is_empty())
    }

    pub(in crate::features) fn terminal_theme_is_dark(&self) -> bool {
        self.settings.terminal_theme_is_dark()
    }

    pub(in crate::features) fn clear_ai_model_search(&mut self, cx: &mut Context<Self>) {
        self.with_app(cx, |app, cx| {
            app.ai.clear_settings_model_query();
            app.reset_text_input("ai.settings.model-search", "", cx);
            cx.notify();
        });
    }

    pub(in crate::features) fn clear_keybinding_search(&mut self, cx: &mut Context<Self>) {
        self.with_app(cx, |app, cx| {
            app.settings.clear_keybinding_search();
            app.reset_text_input("settings.keybindings.search", "", cx);
            cx.notify();
        });
    }

    pub(in crate::features) fn prompt_encrypted_portable_snapshot_export(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.prompt_encrypted_portable_snapshot_export(window, cx)
        });
    }

    pub(in crate::features) fn prompt_encrypted_portable_snapshot_import(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.prompt_encrypted_portable_snapshot_import(window, cx)
        });
    }

    pub(in crate::features) fn prompt_provider_cloud_sync_push(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.prompt_provider_cloud_sync_push(window, cx)
        });
    }

    pub(in crate::features) fn prompt_provider_cloud_sync_pull(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.prompt_provider_cloud_sync_pull(window, cx)
        });
    }

    pub(in crate::features) fn prompt_cloud_sync_force_push(
        &mut self,
        provider_action: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.prompt_cloud_sync_force_push(provider_action, window, cx);
        });
    }

    pub(in crate::features) fn prompt_cloud_sync_force_pull(
        &mut self,
        provider_action: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.prompt_cloud_sync_force_pull(provider_action, window, cx);
        });
    }

    pub(in crate::features) fn prompt_cloud_sync_recover_current(
        &mut self,
        provider_action: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.prompt_cloud_sync_recover_current(provider_action, window, cx);
        });
    }

    pub(in crate::features) fn toggle_terminal_action_links_matcher(
        &mut self,
        matcher: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.toggle_terminal_action_links_matcher(matcher, cx)
        });
    }

    pub(in crate::features) fn toggle_keyword_highlight_builtin(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.toggle_keyword_highlight_builtin(rule_id, cx)
        });
    }

    pub(in crate::features) fn toggle_keyword_highlight_rule(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.toggle_keyword_highlight_rule(rule_id, cx));
    }

    pub(in crate::features) fn add_keyword_highlight_rule(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.add_keyword_highlight_rule(window, cx));
    }

    pub(in crate::features) fn remove_keyword_highlight_rule(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.remove_keyword_highlight_rule(rule_id, cx));
    }

    pub(in crate::features) fn focus_keyword_highlight_field(
        &mut self,
        rule_id: String,
        field: KeywordHighlightEditorField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.focus_keyword_highlight_field(rule_id, field, window, cx)
        });
    }

    pub(in crate::features) fn handle_keyword_highlight_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.with_app(cx, |app, cx| {
            app.handle_keyword_highlight_key_down(event, window, cx)
        })
    }

    pub(in crate::features) fn set_keyword_highlight_rule_color(
        &mut self,
        rule_id: String,
        dark: bool,
        color: &str,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.set_keyword_highlight_rule_color(rule_id, dark, color, cx);
        });
    }

    pub(in crate::features) fn expand_keyword_highlight_rule(
        &mut self,
        rule_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.expand_keyword_highlight_rule(rule_id, cx));
    }

    pub(in crate::features) fn keyword_highlight_text_input_id(
        rule_id: &str,
        field: KeywordHighlightEditorField,
    ) -> String {
        format!("keyword.highlight.{rule_id}.{}", field.input_key())
    }

    pub(in crate::features) fn toggle_ai_model_enabled(
        &mut self,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.toggle_ai_model_enabled(model_id, cx));
    }

    pub(in crate::features) fn toggle_ai_model_group(
        &mut self,
        group_key: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.toggle_ai_model_group(group_key, cx));
    }

    pub(in crate::features) fn remove_ai_manual_model(
        &mut self,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.remove_ai_manual_model(model_id, cx));
    }

    pub(in crate::features) fn add_ai_manual_model(
        &mut self,
        credential_id: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.add_ai_manual_model(credential_id, name, cx)
        });
    }

    pub(in crate::features) fn focus_ai_manual_model_input(
        &mut self,
        group_key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.focus_ai_manual_model_input(group_key, window, cx)
        });
    }

    pub(in crate::features) fn handle_ai_manual_model_key_down(
        &mut self,
        group_key: &str,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.with_app(cx, |app, cx| {
            app.handle_ai_manual_model_key_down(group_key, event, window, cx)
        })
    }

    pub(in crate::features) fn clear_ai_manual_model_draft(
        &mut self,
        group_key: &str,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.clear_ai_manual_model_draft(group_key, cx));
    }

    pub(in crate::features) fn toggle_ai_credential_enabled(
        &mut self,
        credential_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.toggle_ai_credential_enabled(credential_id, cx)
        });
    }

    pub(in crate::features) fn add_ai_credential(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.add_ai_credential(window, cx));
    }

    pub(in crate::features) fn remove_ai_credential(
        &mut self,
        credential_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.remove_ai_credential(credential_id, cx));
    }

    pub(in crate::features) fn persist_ai_credential_edits(
        &mut self,
        credential_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.persist_ai_credential_edits(credential_id, cx)
        });
    }

    pub(in crate::features) fn toggle_ai_action_enabled(
        &mut self,
        kind: AiActionListKind,
        action_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.toggle_ai_action_enabled(kind, action_id, cx)
        });
    }

    pub(in crate::features) fn remove_ai_action(
        &mut self,
        kind: AiActionListKind,
        action_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.remove_ai_action(kind, action_id, cx));
    }

    pub(in crate::features) fn add_ai_action(
        &mut self,
        kind: AiActionListKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.add_ai_action(kind, window, cx));
    }

    pub(in crate::features) fn focus_ai_action_field(
        &mut self,
        kind: AiActionListKind,
        action_id: String,
        field: AiActionEditorField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.focus_ai_action_field(kind, action_id, field, window, cx)
        });
    }

    pub(in crate::features) fn handle_ai_action_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.with_app(cx, |app, cx| {
            app.handle_ai_action_key_down(event, window, cx)
        })
    }

    pub(in crate::features) fn ai_action_text_input_id(
        kind: AiActionListKind,
        action_id: &str,
        field: AiActionEditorField,
    ) -> String {
        format!(
            "ai.settings.action.{}.{action_id}.{}",
            kind.input_key(),
            field.input_key()
        )
    }

    pub(in crate::features) fn handle_keybinding_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.handle_keybinding_key_down(event, cx));
    }

    pub(in crate::features) fn toggle_search_engine_menu(
        &mut self,
        menu: crate::features::settings::SearchEngineMenu,
        index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        self.with_app(cx, |app, cx| {
            let changed = app.settings.toggle_search_engine_menu(menu, index);
            if changed {
                cx.notify();
            }
            changed
        })
    }

    pub(in crate::features) fn close_search_engine_menus(&mut self, cx: &mut Context<Self>) {
        self.with_app(cx, |app, cx| {
            app.settings.close_search_engine_menus();
            cx.notify();
        });
    }

    pub(in crate::features) fn toggle_search_engine_in_menu(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.toggle_search_engine_in_menu(index, cx));
    }

    pub(in crate::features) fn expand_search_engine(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.expand_search_engine(index, cx));
    }

    pub(in crate::features) fn test_search_engine(&mut self, index: usize, cx: &mut Context<Self>) {
        self.with_app(cx, |app, cx| app.test_search_engine(index, cx));
    }

    pub(in crate::features) fn remove_search_engine(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.remove_search_engine(index, cx));
    }

    pub(in crate::features) fn set_search_engine_icon(
        &mut self,
        index: usize,
        icon: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let icon = icon.map(str::to_string);
        self.with_app(cx, |app, cx| {
            app.set_search_engine_icon(index, icon.as_deref(), cx)
        });
    }

    pub(in crate::features) fn prompt_transfer_default_editor_setting(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.prompt_transfer_default_editor_setting(cx));
    }

    pub(in crate::features) fn prompt_transfer_download_path_setting(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.prompt_transfer_download_path_setting(cx));
    }

    pub(in crate::features) fn prompt_recording_path_setting(&mut self, cx: &mut Context<Self>) {
        self.with_app(cx, |app, cx| app.prompt_recording_path_setting(cx));
    }

    pub(in crate::features) fn clear_translation_secret(
        &mut self,
        provider: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.clear_translation_secret(provider, cx));
    }

    pub(in crate::features) fn add_appearance_fallback_font(
        &mut self,
        terminal: bool,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.add_appearance_fallback_font(terminal, cx));
    }

    pub(in crate::features) fn remove_appearance_font_stack_entry(
        &mut self,
        terminal: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.remove_appearance_font_stack_entry(terminal, index, cx);
        });
    }

    pub(in crate::features) fn set_background_content_opacity(
        &mut self,
        percent: u8,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.set_background_content_opacity(percent, cx)
        });
    }

    pub(in crate::features) fn set_background_image_opacity(
        &mut self,
        percent: u8,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.set_background_image_opacity(percent, cx));
    }

    pub(in crate::features) fn keybinding_conflict_label(
        &self,
        pending_keys: &str,
        exclude_id: &str,
    ) -> Option<String> {
        let pending = crate::shortcuts::ShortcutBinding::parse(pending_keys).ok()?;
        let exclude = crate::shortcuts::ShortcutId::parse(exclude_id)?;
        let conflict = crate::shortcuts::conflicting_shortcut(
            &pending,
            exclude,
            &self.settings.summary().keybindings,
        )?;
        let definition = crate::shortcuts::SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == conflict)?;
        Some(t!(definition.label_key).into_owned())
    }

    pub(in crate::features) fn start_keybinding_recording(
        &mut self,
        shortcut_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| {
            app.start_keybinding_recording(shortcut_id, window, cx);
        });
    }

    pub(in crate::features) fn reset_keybinding(
        &mut self,
        shortcut_id: String,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.reset_keybinding(shortcut_id, cx));
    }

    pub(in crate::features) fn snapshot_password_prompt_banner(
        &mut self,
        prompt: SnapshotPasswordPromptState,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.section_header))
            .px_3()
            .py_2()
            .text_size(px(12.))
            .text_color(rgb(palette.text_muted))
            .child(format!("{:?}", prompt.kind))
            .into_any_element()
    }
}

macro_rules! forward_app_action {
    ($($name:ident),+ $(,)?) => {
        impl SettingsPanel {
            $(
                pub(in crate::features) fn $name(&mut self, cx: &mut Context<Self>) {
                    self.with_app(cx, |app, cx| app.$name(cx));
                }
            )+
        }
    };
}

forward_app_action!(
    add_search_engine,
    cancel_github_gist_auth,
    cancel_keybinding_recording,
    clear_background_image,
    confirm_keybinding_recording,
    copy_github_gist_user_code,
    discover_ai_models,
    open_github_gist_verification_url,
    prompt_background_image,
    prompt_diagnostics_export,
    prompt_keyword_highlight_import,
    reset_all_keybindings,
    reveal_log_dir,
    run_provider_cloud_sync_test,
    start_github_gist_auth,
    toggle_ai_allow_save_command,
    toggle_ai_claude_enabled,
    toggle_ai_claude_mcp_integration,
    toggle_ai_codex_enabled,
    toggle_ai_codex_mcp_integration,
    toggle_ai_enabled,
    toggle_ai_mcp_enabled,
    toggle_ai_record_history,
    toggle_ai_redaction,
    toggle_alt_as_meta,
    toggle_ascend_npu_monitor_panel,
    toggle_cloud_sync_auto_check,
    toggle_cloud_sync_auto_pull_remote_changes,
    toggle_cloud_sync_auto_push,
    toggle_cloud_sync_enabled,
    toggle_command_suggestions,
    toggle_confirm_on_close,
    toggle_cursor_blink,
    toggle_docker_manager_panel,
    toggle_gpu_monitor_panel,
    toggle_interaction_copy_on_select,
    toggle_interaction_right_click_paste,
    toggle_keyword_highlights,
    toggle_mac_ime_compatibility,
    toggle_minimize_to_tray,
    toggle_multi_line_paste_dialog,
    toggle_notes_panel,
    toggle_osc52_clipboard_write,
    toggle_panel_multi_open,
    toggle_paste_image_as_path,
    toggle_process_manager_panel,
    toggle_recording_auto_start,
    toggle_recording_binary_transfer_payloads,
    toggle_recording_io_labels,
    toggle_recording_session_metadata,
    toggle_recording_timestamps,
    toggle_remote_stats_panel,
    toggle_s3_virtual_host_style,
    toggle_screen_lock_enabled,
    toggle_settings_master_password,
    toggle_startup_restore,
    toggle_startup_restore_window_layout,
    toggle_terminal_action_links,
    toggle_terminal_line_numbers,
    toggle_terminal_low_latency_mode,
    toggle_terminal_reconnect_restore_cwd,
    toggle_terminal_timestamps,
    toggle_terminal_workspace_padding,
    toggle_terminal_zebra_stripes,
    toggle_terminal_zoom_enabled,
    toggle_transfer_ask_save_location,
    toggle_transfer_preserve_timestamps,
    toggle_transfer_resume_broken,
);

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{
        AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, div, px,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::pages::settings::inputs::ALL_SETTINGS_TABS;
    use crate::features::{FontCatalogLoadState, NyaTermApp};
    use crate::models::{AiActionEditorField, AiActionListKind, NavItem, SettingsTab};
    use crate::test_support::TestConfigDir;

    use super::{FontSelectOptionCache, SettingsPanel, SettingsSurface};

    #[test]
    fn font_select_option_cache_rebuilds_after_catalog_load() {
        let mut cache = FontSelectOptionCache::empty();
        cache.update(FontCatalogLoadState::Loading, 1, &["Inter".to_string()]);
        cache.update(
            FontCatalogLoadState::Loaded,
            1,
            &["JetBrains Mono".to_string()],
        );

        assert_eq!(cache.options[0].value(), "JetBrains Mono");
    }

    fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    struct MainHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for MainHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().w(px(360.)).h(px(720.)).flex().flex_col().child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    self.app
                        .read(cx)
                        .settings_panel
                        .clone()
                        .cached(crate::features::layout::cached_panel_style()),
                ),
            )
        }
    }

    struct DualHost {
        main: Entity<SettingsPanel>,
        native: Entity<SettingsPanel>,
    }

    impl Render for DualHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(760.))
                .h(px(720.))
                .flex()
                .gap_2()
                .child(
                    div().flex_1().min_w_0().child(
                        self.main
                            .clone()
                            .cached(crate::features::layout::cached_panel_style()),
                    ),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        self.native
                            .clone()
                            .cached(crate::features::layout::cached_panel_style()),
                    ),
                )
        }
    }

    struct PanelHost {
        panel: Entity<SettingsPanel>,
    }

    impl Render for PanelHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().w(px(360.)).h(px(720.)).child(self.panel.clone())
        }
    }

    fn hosted<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| MainHost {
            app: host_app.clone(),
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            draw(&app, vcx);
        }
        (app, vcx)
    }

    fn draw(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }

    fn main_panel(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> Entity<SettingsPanel> {
        app.read(cx).settings_panel.clone()
    }

    fn paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
        app.read(cx).settings_panel.read(cx).paint_count
    }

    fn rebuilds(panel: &Entity<SettingsPanel>, cx: &mut gpui::App) -> usize {
        panel.read(cx).rebuild_count
    }

    #[test]
    fn panel_lease_callback_does_not_double_lease() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let panel = cx.update_entity(&app, |app, _| app.settings_panel.clone());
        let host_panel = panel.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| PanelHost {
            panel: host_panel.clone(),
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();

        let mut before = false;
        vcx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                before = panel
                    .snapshot()
                    .expect("hosted panel has an initial snapshot")
                    .settings
                    .summary()
                    .startup_restore;
                panel.with_app(cx, |app, cx| {
                    app.toggle_startup_restore(cx);
                });
                assert_eq!(
                    panel
                        .snapshot()
                        .expect("snapshot remains available")
                        .settings
                        .summary()
                        .startup_restore,
                    before,
                    "the panel snapshot must not be flushed while the panel is leased"
                );
            });
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            assert_ne!(
                panel
                    .read(cx)
                    .snapshot()
                    .expect("deferred flush ran")
                    .settings
                    .summary()
                    .startup_restore,
                before,
                "the deferred refresh must publish the app mutation"
            );
        });
    }

    #[test]
    fn repeated_refresh_requests_coalesce_and_clear_for_next_cycle() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let panel = vcx.update(|_, cx| main_panel(&app, cx));
        let initial = vcx.update(|_, cx| rebuilds(&panel, cx));

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.toggle_startup_restore(cx);
                app.request_settings_panel_refresh(cx);
                app.request_settings_panel_refresh(cx);
            });
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| rebuilds(&panel, cx)),
            initial + 1,
            "duplicate requests in one cycle must publish one snapshot"
        );

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.toggle_startup_restore(cx);
                app.request_settings_panel_refresh(cx);
            });
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| rebuilds(&panel, cx)),
            initial + 2,
            "the request flag must clear so a later cycle can rebuild"
        );
    }

    #[test]
    fn unrelated_app_notify_does_not_repaint_cached_settings_panel() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let before = vcx.update(|_, cx| paints(&app, cx));
        assert!(
            before > 0,
            "the panel must have painted at least once, or this proves nothing"
        );

        for _ in 0..5 {
            draw(&app, vcx);
        }

        assert_eq!(
            vcx.update(|_, cx| paints(&app, cx)),
            before,
            "unrelated app notifications must not repaint the cached settings panel"
        );
    }

    #[test]
    fn inactive_cloud_sync_state_does_not_rebuild_settings_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());
        let panel = vcx.update(|_, cx| main_panel(&app, cx));
        let (initial_rebuilds, initial_status) = vcx.update(|_, cx| {
            let panel = panel.read(cx);
            (
                panel.rebuild_count,
                panel
                    .snapshot()
                    .expect("initial snapshot")
                    .cloud_sync
                    .status
                    .clone(),
            )
        });

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.shell.set_settings_active_tab(SettingsTab::General);
                app.cloud_sync.set_status("background cloud sync status");
                app.request_settings_panel_refresh(cx);
            });
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(
                panel.rebuild_count, initial_rebuilds,
                "inactive Cloud Sync state must not move the active Settings snapshot"
            );
            assert_eq!(
                panel
                    .snapshot()
                    .expect("snapshot still present")
                    .cloud_sync
                    .status,
                initial_status,
                "inactive facade data must not be copied into the panel"
            );
        });
    }

    #[test]
    fn main_and_native_surfaces_receive_updates_independently() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let app = app(&mut cx, test_dir.path());
        let native = cx.new(|cx| {
            SettingsPanel::new_for_surface(app.downgrade(), SettingsSurface::NativeWindow, cx)
        });
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            app.register_native_settings_panel(&native, cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let main = cx.update_entity(&app, |app, _| app.settings_panel.clone());
        let host_main = main.clone();
        let host_native = native.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| DualHost {
            main: host_main.clone(),
            native: host_native.clone(),
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.register_native_settings_panel(&native, cx);
                app.flush_settings_panel_snapshots(cx);
            });
        });
        vcx.run_until_parked();

        let (main_before, native_before) =
            vcx.update(|_, cx| (rebuilds(&main, cx), rebuilds(&native, cx)));
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.toggle_startup_restore(cx);
                app.request_settings_panel_refresh(cx);
            });
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let main_panel = main.read(cx);
            let native_panel = native.read(cx);
            assert_eq!(main_panel.surface(), SettingsSurface::MainPage);
            assert_eq!(native_panel.surface(), SettingsSurface::NativeWindow);
            assert_eq!(main_panel.rebuild_count, main_before + 1);
            assert_eq!(native_panel.rebuild_count, native_before + 1);
            assert_eq!(
                main_panel
                    .snapshot()
                    .expect("main snapshot")
                    .settings
                    .summary()
                    .startup_restore,
                native_panel
                    .snapshot()
                    .expect("native snapshot")
                    .settings
                    .summary()
                    .startup_restore,
                "both surfaces must receive the same authoritative state through separate panels"
            );
        });
    }

    fn activate(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext, tab: SettingsTab) {
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| app.focus_settings_tab(tab, cx));
        });
        draw(app, vcx);
    }

    /// Drive an app action the way one of the panel's own controls would.
    ///
    /// Every listener on the panel goes through `with_app`, which requests a snapshot
    /// refresh afterwards, so a test that calls the action directly has to as well.
    fn from_panel(
        app: &Entity<NyaTermApp>,
        vcx: &mut VisualTestContext,
        f: impl FnOnce(&mut NyaTermApp, &mut gpui::Window, &mut gpui::Context<NyaTermApp>),
    ) {
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                f(app, window, cx);
                app.request_settings_panel_refresh(cx);
            });
        });
        draw(app, vcx);
    }

    fn drawn_text_inputs(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext, ids: &[&str]) {
        vcx.update(|_, cx| {
            let panel = app.read(cx).settings_panel.read(cx);
            let snapshot = panel.snapshot().expect("the panel drew with a snapshot");
            for id in ids {
                assert!(
                    snapshot.text_inputs.contains_key(*id),
                    "{id} is drawn but no boundary built it"
                );
            }
        });
    }

    /// Every tab, and every row a tab can reveal, must draw only inputs that some
    /// boundary already built.
    ///
    /// Ignored: with this fixture, activating a tab that holds a registry-backed input
    /// never parks. `General` (selects only) is fine, while `Appearance` (number
    /// inputs) and `Security` (a masked text input and a number input) both spin in the
    /// window's effect flush. It reproduces with the snapshot's input-map comparison
    /// reverted, so it is not that; the cause has not been identified. Note that those
    /// same sections do render without spinning in
    /// `inputs::tests::reveal_boundaries_build_the_inputs_their_rows_draw`, because
    /// `open_page` opens the native settings window and that surface paints them -- so
    /// the problem is something about this host, not the inputs themselves.
    ///
    /// The boundary coverage this was written for lives in that `inputs.rs` test
    /// instead: it asserts the published snapshot carries every id a revealed row looks
    /// up, and the native settings window renders those rows, so a miss still trips the
    /// `debug_assert!` in `existing_text_input_box`.
    #[test]
    #[ignore = "this fixture never parks once a tab with a registry input is active; cause unknown"]
    fn every_settings_surface_draws_only_inputs_it_built() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let (app, vcx) = hosted(&mut cx, test_dir.path());

        // First, before the loop below has visited Security: enabling cloud sync
        // without a master password redirects there, and that jump is not the tab
        // strip.
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| app.toggle_cloud_sync_enabled(cx));
        });
        draw(&app, vcx);
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx).shell.settings_active_tab(),
                SettingsTab::Security,
                "enabling cloud sync without a master password must land on Security"
            );
        });
        drawn_text_inputs(&app, vcx, &["settings.security.master-password"]);

        let before = vcx.update(|_, cx| paints(&app, cx));
        for tab in ALL_SETTINGS_TABS {
            activate(&app, vcx, tab);
        }
        assert!(
            vcx.update(|_, cx| paints(&app, cx)) > before,
            "the hosted panel must actually paint for this test to prove anything"
        );

        // Adding a custom engine expands its row, and only an expanded row draws the
        // two fields. The ids carry the row index, so the add forgets the whole prefix
        // and has to rebuild.
        activate(&app, vcx, SettingsTab::Search);
        from_panel(&app, vcx, |app, _, cx| app.add_search_engine(cx));
        let engine_ids = [
            "settings.search-engine.0.name",
            "settings.search-engine.0.url",
        ];
        drawn_text_inputs(&app, vcx, &engine_ids);
        // Closing and re-opening the row goes through the same boundary.
        from_panel(&app, vcx, |app, _, cx| app.expand_search_engine(0, cx));
        from_panel(&app, vcx, |app, _, cx| app.expand_search_engine(0, cx));
        drawn_text_inputs(&app, vcx, &engine_ids);

        // A freshly added provider credential draws three fields.
        activate(&app, vcx, SettingsTab::AiModels);
        from_panel(&app, vcx, |app, window, cx| {
            app.add_ai_credential(window, cx)
        });
        let credential_id = vcx.update(|_, cx| {
            app.read(cx)
                .ai
                .settings_config()
                .provider_credentials
                .last()
                .expect("the credential was added")
                .id
                .clone()
        });
        drawn_text_inputs(
            &app,
            vcx,
            &[
                &format!("ai.credential.{credential_id}.name"),
                &format!("ai.credential.{credential_id}.base-url"),
                &format!("ai.credential.{credential_id}.api-key"),
            ],
        );

        // A freshly added action draws a name and a prompt, in both lists.
        activate(&app, vcx, SettingsTab::AiRules);
        for kind in [AiActionListKind::Terminal, AiActionListKind::File] {
            from_panel(&app, vcx, |app, window, cx| {
                app.add_ai_action(kind, window, cx)
            });
            let action_id = vcx.update(|_, cx| {
                let config = app.read(cx).ai.settings_config().clone();
                let actions = match kind {
                    AiActionListKind::Terminal => config.terminal_ai_actions,
                    AiActionListKind::File => config.file_ai_actions,
                };
                actions.last().expect("the action was added").id.clone()
            });
            drawn_text_inputs(
                &app,
                vcx,
                &[
                    &SettingsPanel::ai_action_text_input_id(
                        kind,
                        &action_id,
                        AiActionEditorField::Name,
                    ),
                    &SettingsPanel::ai_action_text_input_id(
                        kind,
                        &action_id,
                        AiActionEditorField::Prompt,
                    ),
                ],
            );
        }
    }
}
