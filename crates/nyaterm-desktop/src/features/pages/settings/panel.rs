use crate::features::ai::agent_management::AgentManagementView;
use crate::features::selects::SelectRegistry;
use crate::features::settings::{
    KeybindingPresentationState, KeywordHighlightPresentationState, SearchEnginePresentationState,
};
use crate::features::sync::CloudSyncLiveState;
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
use gpui::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, WeakEntity, Window, canvas, div, prelude::*, px, rgb,
};
use nyaterm_core::{
    AiSettings, AppSettingsSummary, CloudSyncHistoryEntry, CloudSyncSettings, CloudSyncState,
    KeywordHighlightConfig, TranslationSettings,
};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaInputShell, NyaNumberInputState, NyaSelect, NyaSelectOption,
    NyaSelectState, NyaSettingsLayout, NyaSettingsNavGroup, NyaSettingsNavItem, NyaSliderState,
    NyaSliderValue,
};
use rust_i18n::t;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// Background colours are resolved by the app because the panel is cached: it
/// only repaints when a new snapshot arrives, so wallpaper, window material, and
/// content opacity must travel inside the snapshot to reach it.
#[derive(Clone, Copy, PartialEq)]
pub(in crate::features) struct SettingsChrome {
    pub palette: ThemePalette,
    /// The page's single tinted layer, honouring content opacity.
    pub surface: gpui::Rgba,
    pub transparent_bg: gpui::Rgba,
    pub transparent_surface: gpui::Rgba,
    pub transparent_section_header: gpui::Rgba,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum SettingsSectionPresentation {
    General,
    Appearance,
    Interaction,
    Keybindings,
    TerminalGeneral,
    RemoteMonitoring,
    Search,
    Translation,
    AiGeneral,
    AiModels,
    AiRules,
    AiAgents,
    Transfer,
    Security,
    SyncBackup,
}

#[derive(Clone)]
pub(in crate::features) struct SettingsSnapshot {
    pub chrome: SettingsChrome,
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
            SettingsTab::AiGeneral
            | SettingsTab::AiModels
            | SettingsTab::AiRules
            | SettingsTab::AiAgents => self.ai == other.ai,
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
            | SettingsTab::RemoteMonitoring
            | SettingsTab::TerminalGeneral
            | SettingsTab::Search
            | SettingsTab::Security => self.settings == other.settings,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct SettingsPresentation {
    pub(in crate::features) summary: Arc<AppSettingsSummary>,
    pub(in crate::features) theme_options: Arc<[(String, String)]>,
    pub(in crate::features) theme_diagnostics: Arc<[String]>,
    pub(in crate::features) theme_designer:
        crate::features::settings::theme_designer::ThemeDesignerState,
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
    /// Read-only palette projection from the settings owner, matching terminal paint.
    pub(in crate::features) terminal_ansi: [u32; 16],
    /// Active data directory and its bootstrap state, sourced from `AppRuntime`.
    pub(in crate::features) data_directory: SharedString,
    pub(in crate::features) data_directory_custom: bool,
    pub(in crate::features) data_directory_editable: bool,
    pub(in crate::features) data_directory_restart_pending: bool,
    pub(in crate::features) data_directory_notice: Option<SharedString>,
    pub(in crate::features) data_directory_sync_client: Option<SharedString>,
}

impl SettingsPresentation {
    fn empty(cx: &mut Context<SettingsPanel>) -> Self {
        Self {
            summary: Arc::new(AppSettingsSummary::default()),
            theme_options: nyaterm_ui::available_themes(&Default::default()).into(),
            theme_diagnostics: Arc::from([]),
            theme_designer: Default::default(),
            keyword_config: Arc::new(KeywordHighlightConfig::default()),
            search_engine_presentation: SearchEnginePresentationState {
                expanded_index: None,
                icon_picker_index: None,
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
            terminal_ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
            data_directory: SharedString::default(),
            data_directory_custom: false,
            data_directory_editable: false,
            data_directory_restart_pending: false,
            data_directory_notice: None,
            data_directory_sync_client: None,
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

    pub(in crate::features) fn snapshot_password_prompt_active(&self) -> bool {
        self.snapshot_password_prompt_active
    }

    pub(in crate::features) fn config_path_prompt_active(&self) -> bool {
        self.config_path_prompt_active
    }

    pub(in crate::features) fn terminal_theme_is_dark(&self) -> bool {
        self.terminal_theme_is_dark
    }

    pub(in crate::features) fn builtin_keyword_swatch(&self, id: &str) -> String {
        nyaterm_core::keyword_highlight_presets::builtin_keyword_rule_swatch_ansi(
            id,
            self.terminal_ansi,
            self.terminal_theme_is_dark,
        )
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct AiSettingsPresentation {
    pub(in crate::features) providers: crate::features::ai::ProviderSettingsView,
    pub(in crate::features) config: Arc<AiSettings>,
    pub(in crate::features) manual_model_drafts: Arc<HashMap<String, String>>,
    pub(in crate::features) credential_draft_key_ids: Arc<HashSet<String>>,
    pub(in crate::features) header_errors:
        Arc<HashMap<(String, String), nyaterm_core::ai::provider_settings::CustomHeaderError>>,
    pub(in crate::features) agent_management: AgentManagementView,
    pub(in crate::features) background_tasks: crate::features::mcp::tasks::BackgroundTasksView,
}

impl AiSettingsPresentation {
    fn empty(_cx: &mut Context<SettingsPanel>) -> Self {
        Self {
            providers: Default::default(),
            config: Arc::new(AiSettings::default()),
            manual_model_drafts: Arc::new(HashMap::new()),
            credential_draft_key_ids: Arc::new(HashSet::new()),
            header_errors: Arc::new(HashMap::new()),
            agent_management: AgentManagementView::default(),
            background_tasks: Default::default(),
        }
    }
}

impl AiSettingsPresentation {
    pub(in crate::features) fn settings_config(&self) -> &AiSettings {
        &self.config
    }

    pub(in crate::features) fn settings_manual_model_drafts(&self) -> &HashMap<String, String> {
        &self.manual_model_drafts
    }
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct CloudSyncPresentation {
    pub(in crate::features) v4_preview: Option<crate::features::sync::V4PreviewSummary>,
    pub(in crate::features) settings: Arc<CloudSyncSettings>,
    pub(in crate::features) state: CloudSyncState,
    pub(in crate::features) history: Vec<CloudSyncHistoryEntry>,
    pub(in crate::features) pending_settings: CloudSyncSettings,
    pub(in crate::features) secret_draft: CloudSyncSecretDraft,
    pub(in crate::features) status: String,
    pub(in crate::features) job_running: bool,
    pub(in crate::features) live_state: CloudSyncLiveState,
    pub(in crate::features) conflict: Option<CloudSyncConflictState>,
    pub(in crate::features) github_auth: GithubGistAuthState,
}

impl Default for CloudSyncPresentation {
    fn default() -> Self {
        Self {
            v4_preview: None,
            settings: Arc::new(CloudSyncSettings::default()),
            state: CloudSyncState::default(),
            history: Vec::new(),
            pending_settings: CloudSyncSettings::default(),
            secret_draft: CloudSyncSecretDraft::default(),
            status: String::new(),
            job_running: false,
            live_state: CloudSyncLiveState::Idle,
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

    pub(in crate::features) fn history(&self) -> &[CloudSyncHistoryEntry] {
        &self.history
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

    pub(in crate::features) fn live_state(&self) -> CloudSyncLiveState {
        self.live_state
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
    pub(in crate::features) duplicate_policy:
        nyaterm_files::sftp_transfer_types::SftpDuplicatePolicy,
}

impl TransferSettingsPresentation {
    pub(in crate::features) fn duplicate_policy(
        &self,
    ) -> nyaterm_files::sftp_transfer_types::SftpDuplicatePolicy {
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
    focus: FocusHandle,
    content_scroll_handles: HashMap<String, gpui::ScrollHandle>,
    pub(in crate::features::pages::settings) pane_width: f32,
    pub(in crate::features::pages::settings) pane_height: f32,
    pub(in crate::features::pages::settings) content_width: f32,
    /// Staged V4 conflict selections and candidate choices. They survive
    /// presentation refreshes and are reset whenever the conflict summary
    /// changes (a new preview or an applied batch).
    pub(in crate::features::pages::settings) cloud_sync_conflict_selection: BTreeSet<usize>,
    pub(in crate::features::pages::settings) cloud_sync_conflict_choices: BTreeMap<usize, usize>,
    pub(in crate::features::pages::settings) cloud_sync_conflict_signature: u64,
    /// Whether the per-field conflict rows are expanded below the group cards.
    /// Off by default: the grouped cards are the primary surface.
    pub(in crate::features::pages::settings) cloud_sync_conflict_rows_visible: bool,
    app: WeakEntity<NyaTermApp>,
    snapshot: Option<SettingsSnapshot>,
    pub(in crate::features) settings: SettingsPresentation,
    pub(in crate::features) ai: AiSettingsPresentation,
    pub(in crate::features) cloud_sync: CloudSyncPresentation,
    pub(in crate::features) translation: TranslationPresentation,
    pub(in crate::features) transfer: TransferSettingsPresentation,
    text_inputs: HashMap<SharedString, Entity<nyaterm_ui::NyaInputState>>,
    number_inputs: HashMap<SharedString, Entity<NyaNumberInputState>>,
    pub(in crate::features::pages::settings) image_opacity_slider: Entity<NyaSliderState>,
    pub(in crate::features::pages::settings) content_opacity_slider: Entity<NyaSliderState>,
    pub(in crate::features::pages::settings) window_opacity_slider: Entity<NyaSliderState>,
    pub(in crate::features::pages::settings) data_directory_generation: u64,
    pub(in crate::features::pages::settings) data_directory_checking: bool,
    /// Validated but not yet queued data-directory change shown inline.
    pub(in crate::features::pages::settings) data_directory_choice:
        Option<nyaterm_core::data_directory::PreparedSwitch>,
    pub(in crate::features::pages::settings) image_opacity_focus: gpui::FocusHandle,
    pub(in crate::features::pages::settings) content_opacity_focus: gpui::FocusHandle,
    pub(in crate::features::pages::settings) window_opacity_focus: gpui::FocusHandle,
    last_image_opacity: u8,
    last_content_opacity: u8,
    last_window_opacity: u8,
    slider_subscriptions: Vec<Subscription>,
    pub(in crate::features::pages::settings) theme_picker_controls:
        super::workspace::theme_designer::ThemePickerControls,
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
        let image_opacity_slider = cx.new(|_| NyaSliderState::new().default_value(100.));
        let content_opacity_slider = cx.new(|_| NyaSliderState::new().default_value(82.));
        let window_opacity_slider = cx.new(|_| NyaSliderState::new().default_value(20.));
        let window_subscription = cx.observe(&window_opacity_slider, |this, slider, cx| {
            let NyaSliderValue::Single(value) = slider.read(cx).value() else {
                return;
            };
            let value = value.round().clamp(0., 100.) as u8;
            if value != this.last_window_opacity {
                this.last_window_opacity = value;
                this.set_window_background_opacity(value, cx);
            }
        });
        let image_subscription = cx.observe(&image_opacity_slider, |this, slider, cx| {
            let NyaSliderValue::Single(value) = slider.read(cx).value() else {
                return;
            };
            let value = value.round().clamp(0., 100.) as u8;
            if value != this.last_image_opacity {
                this.last_image_opacity = value;
                this.set_background_image_opacity(value, cx);
            }
        });
        let content_subscription = cx.observe(&content_opacity_slider, |this, slider, cx| {
            let NyaSliderValue::Single(value) = slider.read(cx).value() else {
                return;
            };
            let value = value.round().clamp(0., 100.) as u8;
            if value != this.last_content_opacity {
                this.last_content_opacity = value;
                this.set_background_content_opacity(value, cx);
            }
        });
        Self {
            content_scroll_handles: HashMap::new(),
            pane_width: 0.,
            pane_height: 0.,
            content_width: 0.,
            cloud_sync_conflict_selection: BTreeSet::new(),
            cloud_sync_conflict_choices: BTreeMap::new(),
            cloud_sync_conflict_signature: 0,
            cloud_sync_conflict_rows_visible: false,
            app,
            focus: cx.focus_handle(),
            snapshot: None,
            settings: SettingsPresentation::empty(cx),
            ai: AiSettingsPresentation::empty(cx),
            cloud_sync: CloudSyncPresentation::default(),
            translation: TranslationPresentation::default(),
            transfer: TransferSettingsPresentation::default(),
            text_inputs: HashMap::new(),
            number_inputs: HashMap::new(),
            image_opacity_slider,
            content_opacity_slider,
            window_opacity_slider,
            data_directory_generation: 0,
            data_directory_checking: false,
            data_directory_choice: None,
            image_opacity_focus: cx.focus_handle(),
            content_opacity_focus: cx.focus_handle(),
            window_opacity_focus: cx.focus_handle(),
            last_image_opacity: 100,
            last_content_opacity: 82,
            last_window_opacity: 20,
            slider_subscriptions: vec![
                image_subscription,
                content_subscription,
                window_subscription,
            ],
            theme_picker_controls: Default::default(),
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
        let image_opacity = snapshot.settings.summary.background_image_opacity;
        self.last_image_opacity = image_opacity;
        if self.image_opacity_slider.read(cx).value()
            != NyaSliderValue::Single(f32::from(image_opacity))
        {
            self.image_opacity_slider =
                cx.new(|_| NyaSliderState::new().default_value(f32::from(image_opacity)));
            self.slider_subscriptions[0] =
                cx.observe(&self.image_opacity_slider, |this, slider, cx| {
                    let NyaSliderValue::Single(value) = slider.read(cx).value() else {
                        return;
                    };
                    let value = value.round().clamp(0., 100.) as u8;
                    if value != this.last_image_opacity {
                        this.last_image_opacity = value;
                        this.set_background_image_opacity(value, cx);
                    }
                });
        }
        let content_opacity = snapshot.settings.summary.background_content_opacity;
        self.last_content_opacity = content_opacity;
        if self.content_opacity_slider.read(cx).value()
            != NyaSliderValue::Single(f32::from(content_opacity))
        {
            self.content_opacity_slider =
                cx.new(|_| NyaSliderState::new().default_value(f32::from(content_opacity)));
            self.slider_subscriptions[1] =
                cx.observe(&self.content_opacity_slider, |this, slider, cx| {
                    let NyaSliderValue::Single(value) = slider.read(cx).value() else {
                        return;
                    };
                    let value = value.round().clamp(0., 100.) as u8;
                    if value != this.last_content_opacity {
                        this.last_content_opacity = value;
                        this.set_background_content_opacity(value, cx);
                    }
                });
        }
        let window_opacity = snapshot.settings.summary.window_background_opacity;
        self.last_window_opacity = window_opacity;
        if self.window_opacity_slider.read(cx).value()
            != NyaSliderValue::Single(f32::from(window_opacity))
        {
            self.window_opacity_slider =
                cx.new(|_| NyaSliderState::new().default_value(f32::from(window_opacity)));
            self.slider_subscriptions[2] =
                cx.observe(&self.window_opacity_slider, |this, slider, cx| {
                    let NyaSliderValue::Single(value) = slider.read(cx).value() else {
                        return;
                    };
                    let value = value.round().clamp(0., 100.) as u8;
                    if value != this.last_window_opacity {
                        this.last_window_opacity = value;
                        this.set_window_background_opacity(value, cx);
                    }
                });
        }
        self.settings = snapshot.settings.clone();
        self.ai = snapshot.ai.clone();
        self.cloud_sync = snapshot.cloud_sync.clone();
        self.translation = snapshot.translation.clone();
        self.transfer = snapshot.transfer.clone();
        self.text_inputs = snapshot.text_inputs.clone();
        self.number_inputs = snapshot.number_inputs.clone();
        self.snapshot = Some(snapshot);
        cx.spawn(async move |this, cx| {
            let _ = this.update_in(cx, |panel, window, cx| {
                panel.sync_theme_picker_controls(window, cx)
            });
        })
        .detach();
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

    pub(in crate::features::pages::settings) fn app_runtime(
        &self,
        cx: &Context<Self>,
    ) -> Option<nyaterm_core::runtime::AppRuntime> {
        self.app.upgrade().map(|app| app.read(cx).runtime.clone())
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
        super::settings_input_control(self.form_select_control(
            id,
            options,
            Some(selected_value.into()),
            disabled,
            cx,
        ))
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
        self.settings_chrome().palette
    }

    pub(in crate::features::pages::settings) fn settings_chrome(&self) -> SettingsChrome {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.chrome)
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

    pub(in crate::features::pages::settings) fn select_entity(
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

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.paint_count += 1;
        }
        self.pane_height = f32::from(window.viewport_size().height);

        let Some(snapshot) = self.snapshot.clone() else {
            return div()
                .track_focus(&self.focus)
                .size_full()
                .into_any_element();
        };

        let panel = cx.weak_entity();
        let measurement = canvas(
            move |bounds, _, cx| {
                let width = f32::from(bounds.size.width);
                let Some(panel) = panel.upgrade() else { return };
                if panel.read(cx).pane_width == width {
                    return;
                }
                cx.defer(move |cx| {
                    panel.update(cx, |panel, cx| {
                        if panel.pane_width != width {
                            panel.pane_width = width;
                            cx.notify();
                        }
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        let root = div()
            .track_focus(&self.focus)
            .relative()
            .min_w_0()
            .min_h_0()
            .size_full()
            .flex()
            .flex_col()
            .child(measurement);
        if snapshot.settings.theme_designer.open {
            return root
                .child(self.theme_designer_section(cx))
                .into_any_element();
        }

        let chrome = snapshot.chrome;

        root.bg(chrome.surface)
            .child(self.settings_layout(&snapshot, cx))
            .child(self.settings_action_footer(&snapshot, cx))
            .into_any_element()
    }
}

impl SettingsPanel {
    fn settings_layout(
        &mut self,
        snapshot: &SettingsSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active_item_id = settings_tab_nav_id(snapshot.active_tab);
        let active_label = t!(snapshot.active_tab.i18n_key());
        let scroll_handle = self
            .content_scroll_handles
            .entry(active_item_id.to_string())
            .or_default()
            .clone();
        let content = self.settings_tab_content(snapshot, cx);
        let panel = cx.entity();
        let toggle_panel = panel.clone();
        let measured_panel = panel.downgrade();

        NyaSettingsLayout::new(
            "settings-layout",
            self.settings_nav_groups(snapshot),
            active_item_id,
            content,
        )
        .palette(snapshot.chrome.palette)
        .content_background(snapshot.chrome.transparent_bg)
        .active_title(active_label)
        .sidebar_title(t!("settings.title"))
        .content_scroll_handle(scroll_handle)
        .viewport_width(self.pane_width)
        .on_content_width_changed(move |width, _, cx| {
            let _ = measured_panel.update(cx, |panel, cx| {
                if panel.content_width != width {
                    panel.content_width = width;
                    cx.notify();
                }
            });
        })
        .compact_breakpoint(640.)
        .wide_breakpoint(1024.)
        .on_select(move |item_id, _, cx| {
            if let Some(tab) = settings_tab_from_nav_id(item_id.as_ref()) {
                panel.update(cx, |panel, cx| {
                    panel.with_app(cx, |app, cx| {
                        if tab == SettingsTab::Appearance {
                            app.ensure_appearance_font_options(cx);
                        }
                        app.focus_settings_tab(tab, cx);
                        if tab == SettingsTab::AiAgents {
                            app.refresh_ai_agents(cx);
                        }
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
                "icons/settings/dashboard.svg",
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
                "icons/settings/terminal.svg",
            )
            .accent(palette.success)
            .expanded(snapshot.group_is_expanded("terminal_session"))
            .items([
                settings_nav_item(SettingsTab::TerminalGeneral),
                settings_nav_item(SettingsTab::Search),
                settings_nav_item(SettingsTab::Translation),
                settings_nav_item(SettingsTab::RemoteMonitoring),
            ]),
            NyaSettingsNavGroup::new("ai_group", t!("ai.title"), "icons/settings/ai.svg")
                .accent(0xbc8cff)
                .expanded(snapshot.group_is_expanded("ai_group"))
                .items([
                    settings_nav_item(SettingsTab::AiGeneral),
                    settings_nav_item(SettingsTab::AiModels),
                    settings_nav_item(SettingsTab::AiAgents),
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
            SettingsTab::RemoteMonitoring => self
                .remote_monitoring_settings_section(cx)
                .into_any_element(),
            SettingsTab::Search => self.terminal_search_settings_section(cx).into_any_element(),
            SettingsTab::Translation => self.translation_settings_section(cx).into_any_element(),
            SettingsTab::AiGeneral => self.ai_settings_section(cx).into_any_element(),
            SettingsTab::AiModels => self.ai_models_settings_section(cx).into_any_element(),
            SettingsTab::AiRules => self.ai_rules_settings_section(cx).into_any_element(),
            SettingsTab::AiAgents => self.ai_agents_settings_section(cx).into_any_element(),
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
        let dirty = snapshot.draft_dirty || snapshot.settings.theme_designer.dirty();
        let validation_error = snapshot.validation_error.clone();
        // Keep save actions reachable while the status below explains validation
        // errors. The runtime rejects invalid drafts and keeps settings open.
        let apply_disabled = !dirty;
        let status = validation_error.clone().unwrap_or_else(|| {
            if dirty {
                t!("settings.unappliedChanges").to_string()
            } else {
                String::new()
            }
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
            .bg(snapshot.chrome.transparent_section_header)
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
                            .debug_selector(|| "settings-apply".into())
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
                            .debug_selector(|| "settings-confirm".into())
                            .h(px(28.))
                            .min_w(px(64.))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(rgb(palette.link))
                            .text_size(px(11.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text_on_fill(palette.link)))
                            .cursor_pointer()
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.with_app(cx, |app, cx| {
                                    app.confirm_settings_draft(cx);
                                });
                            }))
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
        SettingsTab::RemoteMonitoring => "settings-tab-remote-monitoring",
        SettingsTab::Search => "settings-tab-search",
        SettingsTab::Translation => "settings-tab-translation",
        SettingsTab::AiGeneral => "settings-tab-ai-general",
        SettingsTab::AiModels => "settings-tab-ai-models",
        SettingsTab::AiRules => "settings-tab-ai-rules",
        SettingsTab::AiAgents => "settings-tab-ai-agents",
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
        "settings-tab-remote-monitoring" => Some(SettingsTab::RemoteMonitoring),
        "settings-tab-search" => Some(SettingsTab::Search),
        "settings-tab-translation" => Some(SettingsTab::Translation),
        "settings-tab-ai-general" => Some(SettingsTab::AiGeneral),
        "settings-tab-ai-models" => Some(SettingsTab::AiModels),
        "settings-tab-ai-rules" => Some(SettingsTab::AiRules),
        "settings-tab-ai-agents" => Some(SettingsTab::AiAgents),
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

    pub(in crate::features) fn clear_keybinding_search(&mut self, cx: &mut Context<Self>) {
        self.with_app(cx, |app, cx| {
            app.settings.clear_keybinding_search();
            app.reset_text_input("settings.keybindings.search", "", cx);
            cx.notify();
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

    pub(in crate::features) fn start_codex_login(
        &mut self,
        device_code: bool,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.start_codex_login(device_code, cx));
    }

    pub(in crate::features) fn copy_external_mcp_config(
        &mut self,
        client: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.copy_external_mcp_config(client, cx));
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

    pub(in crate::features) fn set_window_background_opacity(
        &mut self,
        percent: u8,
        cx: &mut Context<Self>,
    ) {
        self.with_app(cx, |app, cx| app.set_window_background_opacity(percent, cx));
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
    refresh_ai_agents,
    refresh_codex_account,
    refresh_claude_account,
    cancel_codex_login,
    logout_codex,
    copy_codex_device_code,
    open_github_gist_verification_url,
    prompt_background_image,
    prompt_diagnostics_export,
    prompt_keyword_highlight_import,
    reset_all_keybindings,
    reset_remote_monitoring_intervals,
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
    toggle_cloud_sync_auto_check,
    toggle_cloud_sync_auto_pull_remote_changes,
    toggle_cloud_sync_auto_push,
    toggle_cloud_sync_enabled,
    toggle_command_suggestions,
    toggle_confirm_on_close,
    toggle_credential_autofill,
    toggle_cursor_blink,
    toggle_bold_default_foreground,
    toggle_interaction_copy_on_select,
    toggle_mouse_events_require_alt,
    toggle_keyword_highlights,
    toggle_line_role_highlights,
    toggle_minimize_to_tray,
    toggle_multi_line_paste_dialog,
    toggle_osc52_clipboard_write,
    toggle_paste_image_as_path,
    toggle_recording_auto_start,
    toggle_recording_binary_transfer_payloads,
    toggle_recording_io_labels,
    toggle_recording_input,
    toggle_recording_session_metadata,
    toggle_recording_timestamps,
    toggle_s3_virtual_host_style,
    toggle_startup_lock_enabled,
    toggle_idle_lock_enabled,
    toggle_settings_master_password,
    toggle_remote_stats_show_users,
    toggle_startup_restore,
    toggle_startup_restore_window_layout,
    toggle_terminal_action_links,
    toggle_terminal_line_numbers,
    toggle_terminal_wrap_markers,
    toggle_terminal_reconnect_restore_cwd,
    toggle_terminal_timestamps,
    toggle_terminal_workspace_padding,
    toggle_terminal_zebra_stripes,
    toggle_terminal_zebra_stripes_by_row,
    toggle_terminal_zoom_enabled,
    toggle_file_manager_auto_collapse,
    toggle_transfer_ask_save_location,
    toggle_transfer_preserve_timestamps,
    toggle_transfer_resume_broken,
);

#[cfg(test)]
mod tests {
    use super::{FontSelectOptionCache, SettingsPanel};
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::pages::settings::inputs::ALL_SETTINGS_TABS;
    use crate::features::{FontCatalogLoadState, NyaTermApp};
    use crate::models::{AiActionEditorField, AiActionListKind, NavItem, SettingsTab};
    use crate::test_support::TestConfigDir;
    use gpui::{
        AppContext as _, Entity, IntoElement, Modifiers, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, div, prelude::FluentBuilder as _, px,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use std::path::Path;

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
        fill: bool,
    }

    impl Render for MainHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(360.))
                .h(px(720.))
                .when(self.fill, |host| host.size_full())
                .flex()
                .flex_col()
                .child(
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

    struct PanelHost {
        panel: Entity<SettingsPanel>,
    }

    struct ResizablePanelHost {
        app: Entity<NyaTermApp>,
        width: f32,
    }

    struct SettingsBodyHost {
        panel: Entity<SettingsPanel>,
        width: f32,
    }

    impl Render for SettingsBodyHost {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let body = self.panel.update(cx, |panel, cx| {
                panel.content_width = self.width;
                // Exercise provider cards without opening the credential editor.
                panel.ai.providers.selected_id = None;
                let snapshot = panel.snapshot.clone().unwrap();
                panel.settings_tab_content(&snapshot, cx)
            });
            div().w(px(self.width)).child(body)
        }
    }

    #[test]
    fn ai_settings_grids_follow_body_width_in_a_large_window() {
        let dir = TestConfigDir::new("nyaterm-settings-grid-width");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            for _ in 0..3 {
                app.add_ai_provider_preset(nyaterm_core::AiProviderKind::OpenaiCompatible, cx);
            }
            app.focus_settings_tab(SettingsTab::AiModels, cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let panel = cx.update_entity(&app, |app, _| app.settings_panel.clone());
        let provider_selectors: Vec<&'static str> = panel.read_with(&cx, |panel, _| {
            panel
                .ai
                .config
                .provider_credentials
                .iter()
                .map(|credential| {
                    &*Box::leak(format!("ai-provider-card-{}", credential.id).into_boxed_str())
                })
                .collect()
        });
        let (host, vcx) = cx.add_window_view(|_, _| SettingsBodyHost {
            panel: panel.clone(),
            width: 519.,
        });
        vcx.simulate_resize(gpui::size(px(1420.), px(960.)));
        for (width, columns) in [(519., 1), (520., 2), (799., 2), (800., 3)] {
            host.update(vcx, |host, cx| {
                host.width = width;
                cx.notify();
            });
            vcx.update(|window, cx| {
                _ = window.draw(cx);
            });
            vcx.run_until_parked();
            let first = vcx.debug_bounds(provider_selectors[0]).unwrap();
            let second = vcx.debug_bounds(provider_selectors[1]).unwrap();
            let third = vcx.debug_bounds(provider_selectors[2]).unwrap();
            assert_eq!(first.top() == second.top(), columns >= 2);
            assert_eq!(first.top() == third.top(), columns == 3);
            assert!(third.right() <= px(width));
        }
        for (tab, first, second) in [
            (
                SettingsTab::AiGeneral,
                "ai-general-context-field",
                "ai-general-timeout-field",
            ),
            (
                SettingsTab::AiAgents,
                "ai-agent-thread-field",
                "ai-agent-model-field",
            ),
        ] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.focus_settings_tab(tab, cx);
                    app.flush_settings_panel_snapshots(cx);
                });
            });
            for width in [1023., 1024., 519.] {
                host.update(vcx, |host, cx| {
                    host.width = width;
                    cx.notify();
                });
                vcx.update(|window, cx| {
                    _ = window.draw(cx);
                });
                vcx.run_until_parked();
                let first = vcx.debug_bounds(first).unwrap();
                let second = vcx.debug_bounds(second).unwrap();
                assert_eq!(
                    first.top() == second.top(),
                    width >= 1024.,
                    "{tab:?} at {width}"
                );
                assert!(second.right() <= px(width));
            }
        }
    }

    impl Render for ResizablePanelHost {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().w(px(self.width)).h(px(720.)).flex().child(
                self.app
                    .read(cx)
                    .settings_panel
                    .clone()
                    .cached(crate::features::layout::cached_panel_style()),
            )
        }
    }

    #[test]
    fn cached_settings_responds_to_pane_resize_without_window_resize() {
        let dir = TestConfigDir::new("nyaterm-settings-pane-resize");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.open_page(NavItem::Settings, cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let (host, vcx) = cx.add_window_view(|_, _| ResizablePanelHost {
            app: app.clone(),
            width: 395.,
        });
        for (window_width, window_height) in [(1271., 877.), (1289., 919.), (1420., 960.)] {
            vcx.simulate_resize(gpui::size(px(window_width), px(window_height)));
            for (width, sidebar, padding) in [
                (395., 56., 32.),
                (639., 56., 32.),
                (640., 192., 48.),
                (1023., 192., 48.),
                (1024., 224., 64.),
                (395., 56., 32.),
            ] {
                host.update(vcx, |host, cx| {
                    host.width = width;
                    cx.notify();
                });
                for _ in 0..4 {
                    draw(&app, vcx);
                }
                assert_eq!(
                    vcx.debug_bounds("settings-sidebar").unwrap().size.width,
                    px(sidebar)
                );
                vcx.update(|_, cx| {
                    let panel = app.read(cx).settings_panel.read(cx);
                    assert_eq!(panel.pane_width, width);
                    assert_eq!(panel.content_width, width - sidebar - padding);
                });
                let before = vcx.update(|_, cx| paints(&app, cx));
                for _ in 0..3 {
                    draw(&app, vcx);
                }
                assert_eq!(
                    vcx.update(|_, cx| paints(&app, cx)),
                    before,
                    "stable measurements must not repaint the cached panel"
                );
            }
        }
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
            fill: false,
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

    fn click_footer(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext, selector: &'static str) {
        draw(app, vcx);
        let bounds = vcx
            .debug_bounds(selector)
            .expect("settings footer button is drawn");
        vcx.simulate_click(bounds.center(), Modifiers::default());
        draw(app, vcx);
    }

    #[test]
    fn enabled_cloud_sync_switch_can_be_clicked_without_a_master_password() {
        let dir = TestConfigDir::new("nyaterm-settings-cloud-sync-recovery");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(!app.settings.summary().has_master_password);
                app.cloud_sync.toggle_enabled();
                app.rebase_open_settings_draft(cx);
                app.focus_settings_tab(SettingsTab::SyncBackup, cx);
            });
        });
        draw(&app, vcx);
        let control = vcx.debug_bounds("cloud-sync-enabled-control").unwrap();
        vcx.simulate_click(control.center(), Modifiers::default());
        draw(&app, vcx);
        vcx.update(|_, cx| {
            let app = app.read(cx);
            assert!(!app.cloud_sync.settings().enabled);
            assert!(app.settings_draft_dirty());
            assert!(app.pending_settings_validation_error().is_none());
        });
        // Once disabled, the same rendered control must refuse re-enabling it.
        let control = vcx.debug_bounds("cloud-sync-enabled-control").unwrap();
        vcx.simulate_click(control.center(), Modifiers::default());
        draw(&app, vcx);
        vcx.update(|_, cx| assert!(!app.read(cx).cloud_sync.settings().enabled));
    }

    #[test]
    fn invalid_draft_footer_buttons_report_validation_and_keep_settings_open() {
        let dir = TestConfigDir::new("nyaterm-settings-footer-invalid");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, dir.path());
        let error = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.settings
                    .toggle_master_password(false)
                    .expect("stage enabled");
                app.request_settings_panel_refresh(cx);
                app.pending_settings_validation_error().unwrap()
            })
        });
        for selector in ["settings-apply", "settings-confirm"] {
            vcx.update(|_, cx| app.update(cx, |app, _| app.shell.set_status("before click")));
            click_footer(&app, vcx, selector);
            vcx.update(|_, cx| {
                let app = app.read(cx);
                assert!(
                    app.shell.status().contains(&error),
                    "{selector} must invoke validation"
                );
                assert_eq!(app.settings.store_status().message, error);
                assert!(app.settings_tab_is_open());
                assert!(app.shell.has_settings_draft());
                assert!(app.settings_draft_dirty());
                assert!(app.settings.master_password().enabled);
                assert_eq!(
                    app.settings_panel
                        .read(cx)
                        .snapshot()
                        .unwrap()
                        .validation_error,
                    Some(error.clone()),
                    "the footer must retain its validation message"
                );
                let saved = app
                    .store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                        store.load_app_settings_summary()
                    })
                    .expect("read stored settings");
                assert!(!saved.has_master_password);
            });
        }
    }

    #[test]
    fn keyword_highlights_keep_all_rules_visible_and_disable_dependent_controls() {
        fn activate_key(vcx: &mut VisualTestContext, key: &str) {
            let keystroke = vcx.update(|_, cx| {
                gpui::Keystroke::parse_with_os(key, cx.platform_info().os).unwrap()
            });
            vcx.simulate_event(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            vcx.simulate_event(gpui::KeyUpEvent { keystroke });
        }

        struct KeywordHighlightsHost {
            panel: Entity<SettingsPanel>,
        }

        impl Render for KeywordHighlightsHost {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                self.panel.update(cx, |panel, cx| {
                    panel
                        .keyword_highlights_settings_section(cx)
                        .into_any_element()
                })
            }
        }

        for theme in ["github-dark", "github-light"] {
            let dir = TestConfigDir::new("nyaterm-settings-keyword-highlights");
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = app(&mut cx, dir.path());
            let panel = cx.update_entity(&app, |app, cx| {
                app.open_page(NavItem::Settings, cx);
                app.focus_settings_tab(SettingsTab::TerminalGeneral, cx);
                app.update_appearance_theme(theme, cx);
                let mut config = app.settings.keyword_config().clone();
                config.enabled = true;
                app.settings.replace_keyword_config(config);
                app.request_settings_panel_refresh(cx);
                app.flush_settings_panel_snapshots(cx);
                app.settings_panel.clone()
            });
            let (_, vcx) = cx.add_window_view(|_, _| KeywordHighlightsHost { panel });
            for width in [360., 720.] {
                vcx.simulate_resize(gpui::size(px(width), px(1800.)));
                draw(&app, vcx);
                let left = vcx
                    .debug_bounds("settings-keyword-builtin-row-builtin-command")
                    .unwrap();
                let right = vcx
                    .debug_bounds("settings-keyword-builtin-row-builtin-path")
                    .unwrap();
                assert!(
                    left.right() <= right.left(),
                    "rule columns must not overlap"
                );
                assert!(right.right() <= px(width));
                assert!(
                    vcx.debug_bounds("settings-keyword-builtin-row-builtin-variable")
                        .is_some()
                );
                assert!(
                    vcx.debug_bounds("settings-keyword-builtin-row-builtin-perm-special")
                        .is_some()
                );
            }

            let control = "settings-line-role-highlights-control";
            let output = vcx.debug_bounds(control).unwrap();
            vcx.simulate_click(output.center(), Modifiers::default());
            draw(&app, vcx);
            vcx.update(|_, cx| assert!(!app.read(cx).settings.summary().ui_highlight_line_roles));

            let specialized = vcx
                .debug_bounds("settings-keyword-builtin-control-builtin-net-address")
                .unwrap();
            vcx.simulate_click(specialized.center(), Modifiers::default());
            draw(&app, vcx);
            vcx.update(|_, cx| {
                assert!(
                    app.read(cx)
                        .settings
                        .keyword_config()
                        .builtin_rules
                        .is_empty()
                )
            });

            let variable = vcx
                .debug_bounds("settings-keyword-builtin-control-builtin-variable")
                .unwrap();
            vcx.simulate_click(variable.center(), Modifiers::default());
            draw(&app, vcx);
            vcx.update(|_, cx| {
                assert!(!app.read(cx).settings.keyword_config().builtin_rules["builtin-variable"])
            });

            // Keep focus on the dependent switch while the master is disabled,
            // so keyboard activation exercises the disabled control itself.
            let output = vcx.debug_bounds(control).unwrap();
            vcx.simulate_click(output.center(), Modifiers::default());
            draw(&app, vcx);
            vcx.update(|_, cx| app.update(cx, |app, cx| app.toggle_keyword_highlights(cx)));
            draw(&app, vcx);
            activate_key(vcx, "space");
            activate_key(vcx, "enter");
            vcx.simulate_click(output.center(), Modifiers::default());
            vcx.simulate_click(variable.center(), Modifiers::default());
            draw(&app, vcx);
            vcx.update(|_, cx| {
                let app = app.read(cx);
                assert!(app.settings.summary().ui_highlight_line_roles);
                assert!(!app.settings.keyword_config().enabled);
                assert!(!app.settings.keyword_config().builtin_rules["builtin-variable"]);
            });

            vcx.update(|_, cx| app.update(cx, |app, cx| app.toggle_keyword_highlights(cx)));
            draw(&app, vcx);
            vcx.simulate_click(output.center(), Modifiers::default());
            draw(&app, vcx);
            activate_key(vcx, "space");
            draw(&app, vcx);
            vcx.update(|_, cx| assert!(app.read(cx).settings.summary().ui_highlight_line_roles));
        }
    }

    #[test]
    fn row_stripes_subswitch_follows_main_switch_and_keeps_its_preference() {
        let dir = TestConfigDir::new("nyaterm-settings-row-stripes");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, dir.path());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.focus_settings_tab(SettingsTab::TerminalGeneral, cx);
                app.toggle_terminal_zebra_stripes_by_row(cx);
            })
        });
        draw(&app, vcx);
        assert!(vcx.debug_bounds("terminal-row-stripes-setting").is_some());
        vcx.update(|_, cx| app.update(cx, |app, cx| app.toggle_terminal_zebra_stripes(cx)));
        draw(&app, vcx);
        assert!(vcx.debug_bounds("terminal-row-stripes-setting").is_none());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(app.settings.summary().terminal_zebra_stripes_by_row);
                app.toggle_terminal_zebra_stripes(cx);
            })
        });
        draw(&app, vcx);
        assert!(vcx.debug_bounds("terminal-row-stripes-setting").is_some());
    }

    #[test]
    fn clean_footer_disables_apply_and_allows_confirm_even_with_a_validation_error() {
        for invalid_baseline in [false, true] {
            let dir = TestConfigDir::new("nyaterm-settings-footer-clean");
            let mut cx = TestAppContext::single();
            let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let (app, vcx) = hosted(&mut cx, dir.path());
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    if invalid_baseline {
                        app.cloud_sync.toggle_enabled();
                        app.rebase_open_settings_draft(cx);
                    }
                    assert!(!app.settings_draft_dirty());
                    assert_eq!(
                        app.pending_settings_validation_error().is_some(),
                        invalid_baseline
                    );
                    app.shell.set_status("disabled apply must not run");
                    app.settings
                        .set_store_message("disabled apply must not run");
                })
            });
            click_footer(&app, vcx, "settings-apply");
            vcx.update(|_, cx| {
                let app = app.read(cx);
                assert_eq!(app.shell.status(), "disabled apply must not run");
                assert_eq!(
                    app.settings.store_status().message,
                    "disabled apply must not run"
                );
                assert!(app.settings_tab_is_open());
            });
            click_footer(&app, vcx, "settings-confirm");
            vcx.update(|_, cx| {
                let app = app.read(cx);
                assert!(!app.settings_tab_is_open());
                assert!(!app.shell.has_settings_draft());
            });
        }
    }

    #[test]
    fn theme_manager_can_render_browsing_editing_and_unsaved_navigation() {
        use crate::features::settings::theme_designer::ThemeIntent;
        let dir = TestConfigDir::new("nyaterm-theme-manager-view");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.begin_settings_draft(cx);
            app.flush_settings_panel_snapshots(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| MainHost {
            app: host_app,
            fill: true,
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.focus_settings_tab(SettingsTab::Appearance, cx);
                app.theme_designer_action(ThemeIntent::Toggle, cx);
            })
        });
        draw(&app, vcx);
        assert_theme_manager_stays_side_by_side(&app, vcx);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(!app.settings.theme_designer().editable());
                app.theme_designer_action(ThemeIntent::Copy("dracula".into()), cx);
                app.edit_theme_designer(
                    "name",
                    "A long custom theme name that must not displace the editor".into(),
                    cx,
                );
            })
        });
        draw(&app, vcx);
        let name_input = vcx.update(|_, cx| {
            app.read(cx)
                .existing_text_input("theme.designer.name")
                .unwrap()
                .entity_id()
        });
        assert_theme_manager_stays_side_by_side(&app, vcx);
        vcx.update(|_, cx| {
            assert_eq!(
                app.read(cx)
                    .existing_text_input("theme.designer.name")
                    .unwrap()
                    .entity_id(),
                name_input,
            );
            assert!(app.read(cx).settings.theme_designer().dirty());
        });
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(app.existing_text_input("theme.designer.name").is_some());
                app.edit_theme_designer("primary", "invalid".into(), cx);
                app.theme_designer_action(ThemeIntent::Toggle, cx);
            })
        });
        draw(&app, vcx);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                assert!(app.settings.theme_designer().open);
                assert!(app.settings.theme_designer().pending.is_some());
                app.resolve_theme_designer_pending(Some(false), cx);
            })
        });
        draw(&app, vcx);
        vcx.update(|_, cx| assert!(!app.read(cx).settings.theme_designer().open));
    }

    fn assert_theme_manager_stays_side_by_side(
        app: &Entity<NyaTermApp>,
        vcx: &mut VisualTestContext,
    ) {
        for width in [640., 849., 850., 1000., 640.] {
            vcx.simulate_resize(gpui::size(px(width), px(480.)));
            draw(app, vcx);
            let list = vcx.debug_bounds("theme-designer-list").unwrap();
            let editor = vcx.debug_bounds("theme-designer-editor").unwrap();
            assert_eq!(list.size.width, px(if width < 850. { 192. } else { 248. }));
            assert_eq!(list.top(), editor.top());
            assert_eq!(list.size.height, editor.size.height);
            assert!(list.right() <= editor.left());
            assert!(editor.size.width > px(0.));
            assert!(editor.right() <= px(width));
            assert!(editor.bottom() <= px(480.));
        }
    }

    #[test]
    fn window_material_and_content_opacity_reach_the_cached_settings_panel() {
        use nyaterm_core::models::window_material::WindowMaterial;
        let dir = TestConfigDir::new("nyaterm-settings-panel-background");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, dir.path());
        cx.update_entity(&app, |app, cx| {
            app.open_page(NavItem::Settings, cx);
            app.begin_settings_draft(cx);
        });
        for material in [
            WindowMaterial::Blurred,
            WindowMaterial::Transparent,
            WindowMaterial::Opaque,
        ] {
            cx.update_entity(&app, |app, cx| {
                app.set_window_material(material, cx);
                app.set_background_content_opacity(45, cx);
            });
            cx.run_until_parked();
            cx.read_entity(&app, |app, cx| {
                let native = app.shell_background().native_transparent;
                let chrome = app.settings_panel.read(cx).snapshot().unwrap().chrome;
                assert_eq!(chrome.surface, app.shell_surface_color(chrome.palette.bg));
                assert_eq!(chrome.transparent_bg.a, if native { 0. } else { 1. });
                assert_eq!(
                    chrome.transparent_section_header.a,
                    if native { 0. } else { 1. }
                );
                if native {
                    assert!((chrome.surface.a - 0.45).abs() < 0.002);
                } else {
                    assert_eq!(chrome.surface.a, 1.);
                }
            });
        }
        cx.update_entity(&app, |app, cx| app.cancel_settings(cx));
    }

    #[test]
    fn panel_lease_callback_does_not_double_lease() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
    fn keyword_swatches_follow_independent_terminal_themes_in_the_cached_panel() {
        let dir = TestConfigDir::new("nyaterm-settings-keyword-palette");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = hosted(&mut cx, dir.path());
        let panel = vcx.update(|_, cx| main_panel(&app, cx));
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.settings.set_appearance_theme("github-dark".into());
                let mut keywords = app.settings.keyword_config().clone();
                keywords.enabled = true;
                app.settings.replace_keyword_config(keywords);
                app.focus_settings_tab(SettingsTab::TerminalGeneral, cx);
            });
        });

        for theme in [Some("dracula"), Some("github-light"), None] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.settings.set_terminal_theme(theme.map(str::to_string));
                    app.invalidate_paint_theme_caches();
                    app.request_settings_panel_refresh(cx);
                });
            });
            draw(&app, vcx);
            vcx.update(|_, cx| {
                let app = app.read(cx);
                let panel = panel.read(cx);
                let snapshot = panel.snapshot().expect("published settings snapshot");
                assert_eq!(
                    snapshot.chrome.palette.terminal_ansi,
                    nyaterm_ui::theme_palette("github-dark").terminal_ansi
                );
                assert_eq!(
                    snapshot.settings.terminal_ansi,
                    app.terminal_theme_palette().terminal_ansi
                );
                assert_eq!(
                    snapshot.settings.terminal_theme_is_dark(),
                    app.terminal_theme_is_dark()
                );
                for rule in app.resolved_keyword_highlight_rules().iter() {
                    assert_eq!(
                        snapshot.settings.builtin_keyword_swatch(&rule.id),
                        rule.color,
                        "{} under terminal theme {theme:?}",
                        rule.id
                    );
                }
            });
        }
    }

    #[test]
    fn inactive_cloud_sync_state_does_not_rebuild_settings_snapshot() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
    /// `inputs::tests::reveal_boundaries_build_the_inputs_their_rows_draw`, so the
    /// problem is something about this host, not the inputs themselves.
    ///
    /// The boundary coverage this was written for lives in that `inputs.rs` test
    /// instead: it asserts the published snapshot carries every id a revealed row looks
    /// up, and the settings tab renders those rows, so a miss still trips the
    /// `debug_assert!` in `existing_text_input_box`.
    #[test]
    #[ignore = "this fixture never parks once a tab with a registry input is active; cause unknown"]
    fn every_settings_surface_draws_only_inputs_it_built() {
        let test_dir = TestConfigDir::new("nyaterm-settings-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
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
        from_panel(&app, vcx, |app, _, cx| {
            app.add_ai_provider_preset(nyaterm_core::AiProviderKind::OpenaiCompatible, cx)
        });
        let credential_id = vcx.update(|_, cx| {
            app.read(cx)
                .ai
                .settings_config()
                .provider_credentials
                .first()
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
