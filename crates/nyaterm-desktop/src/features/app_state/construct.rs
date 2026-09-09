use rust_i18n::t;

use crate::models::{
    ActivityBarLayoutState, BottomPanelMode, NavItem, PanelSide, SessionEventBridge,
    TerminalFramePipeline,
};
use crate::terminal::initial_terminal_screen;
use gpui::{AppContext as _, Context};
use nyaterm_core::{AppRuntime, uuid};
use nyaterm_store::{BootstrapSnapshot, StoreBlockingClient, StoreUiClient};
#[cfg(test)]
use nyaterm_store::{LoadBootstrap, StoreConfig, StoreRuntime};
use nyaterm_terminal::TerminalOutputDecoder;
use nyaterm_transport::{SessionManager, SftpDuplicatePolicy};
use std::collections::HashMap;
use std::sync::Arc;

use super::NyaTermApp;
use crate::features::ai::{
    AiFeatureFocus, AiFeatureInit, AiFeatureState, AiPanel, ai_active_profile_drafts,
};
use crate::features::assets::StartWorkspaceFeatureState;
use crate::features::commands::{
    CommandFeatureInit, CommandFeatureState, QuickCommandFeatureFocus,
    quick_command_sort_mode_from_setting, quick_command_view_mode_from_setting,
};
use crate::features::connections::{ConnectionFeatureFocus, ConnectionFeatureState};
use crate::features::mcp::McpHostFeatureState;
use crate::features::notes::{NotesFeatureState, NotesPanel};
use crate::features::pages::connections::panel::ConnectionPanel;
use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::transfers::panel::TransferPanel;
use crate::features::panels::{SendCommandFeatureFocus, SendCommandFeatureState};
use crate::features::recording::RecordingFeatureState;
use crate::features::remote::{RemoteOpsFeatureFocus, RemoteOpsFeatureState};
use crate::features::remote_desktop::RemoteDesktopFeatureState;
use crate::features::selects::SelectRegistry;
use crate::features::session::{NativeOtpProvider, SessionFeatureFocus, SessionFeatureState};
use crate::features::settings::{
    SecurityCatalogState, SecurityFeatureFocus, SecurityFeatureState, SettingsFeatureFocus,
    SettingsFeatureInit, SettingsFeatureState,
};
use crate::features::shell::{ShellFeatureInit, ShellFeatureState};
use crate::features::sync::CloudSyncFeatureState;
use crate::features::sync_input::SyncInputFeatureState;
use crate::features::terminal::{TerminalFeatureFocus, TerminalFeatureState};
use crate::features::text_inputs::TextInputRegistry;
use crate::features::transfers::{TransferFeatureFocus, TransferFeatureState};
use crate::features::translation::TranslationFeatureState;
use crate::features::tunnels::{TunnelCatalogState, TunnelFeatureState};
use crate::features::update::UpdateFeatureState;
use crate::models::panel_collapsed_from_persistence;
use crate::terminal::INITIAL_TERMINAL_BANNER;
impl NyaTermApp {
    pub fn from_bootstrap(
        runtime: AppRuntime,
        stores: crate::entities::UiStoreHandles,
        bootstrap: BootstrapSnapshot,
        store_ui: StoreUiClient,
        store_blocking: StoreBlockingClient,
        cx: &mut Context<Self>,
    ) -> Self {
        nyaterm_core::warm_terminal_input_tracker();
        let BootstrapSnapshot {
            database_path,
            connections,
            connection_groups,
            ssh_keys: connection_ssh_keys,
            otp_entries: connection_otp_entries,
            saved_passwords: connection_saved_passwords,
            saved_credentials: connection_saved_credentials,
            tunnels,
            tunnel_groups,
            proxies,
            proxy_groups,
            quick_commands,
            quick_command_categories,
            command_history,
            keyword_highlights,
            settings,
            cloud_sync_settings,
            cloud_sync_state,
            translation_settings,
            ai_settings,
            ai_session_count,
            ai_message_count,
            ai_audit_count,
            open_tabs,
        } = bootstrap;
        let mut settings = settings;
        // Localized child views cache placeholders while they are constructed, so
        // the persisted locale must be active before any of those entities exist.
        crate::i18n::apply_locale(&settings.language);
        let notes_in_layout = settings
            .ui_activity_bar_left_top
            .iter()
            .chain(&settings.ui_activity_bar_left_bottom)
            .chain(&settings.ui_activity_bar_right_top)
            .chain(&settings.ui_activity_bar_right_bottom)
            .any(|id| id == "notes");
        if !notes_in_layout {
            let insert_at = settings
                .ui_activity_bar_left_top
                .iter()
                .position(|id| id == "fileExplorer")
                .map_or(0, |index| index + 1);
            settings
                .ui_activity_bar_left_top
                .insert(insert_at, "notes".to_string());
        }
        let store_status = (
            database_path.display().to_string(),
            "redb connection store online".to_string(),
            true,
        );
        let otp_provider = Arc::new(NativeOtpProvider::new(store_blocking.clone()));
        let transfer_duplicate_policy =
            SftpDuplicatePolicy::from_legacy_value(&settings.transfer_duplicate_strategy);
        let recording = RecordingFeatureState::new(settings.recording_memory_limit_bytes as usize);
        let recording_writer = recording.writer();
        let (ai_model_draft, ai_base_url_draft) = ai_active_profile_drafts(&ai_settings);
        let mcp = McpHostFeatureState::new(&ai_settings.external_mcp);
        let left_panel_width = settings.ui_left_panel_width as f32;
        let right_panel_width = settings.ui_right_panel_width as f32;
        let transfer_panel_height = settings.ui_transfer_height as f32;
        let quick_cmd_height = settings.ui_quick_cmd_height as f32;
        let serial_send_height = settings.ui_serial_send_height as f32;
        let activity_bar_layout = ActivityBarLayoutState {
            left_top: settings.ui_activity_bar_left_top.clone(),
            left_bottom: settings.ui_activity_bar_left_bottom.clone(),
            right_top: settings.ui_activity_bar_right_top.clone(),
            right_bottom: settings.ui_activity_bar_right_bottom.clone(),
            hidden_items: settings.ui_activity_bar_hidden_items.clone(),
            show_labels: settings.ui_activity_bar_show_labels,
        };
        let mut active_left_panel = settings
            .ui_active_left_panel
            .as_deref()
            .and_then(NavItem::from_persistence_id)
            .filter(|item| {
                activity_bar_layout.side_for_entry(item.persistence_id()) == Some(PanelSide::Left)
            });
        let mut active_right_panel = settings
            .ui_active_right_panel
            .as_deref()
            .and_then(NavItem::from_persistence_id)
            .filter(|item| {
                activity_bar_layout.side_for_entry(item.persistence_id()) == Some(PanelSide::Right)
            });
        let mut left_sidebar_collapsed = panel_collapsed_from_persistence(
            settings.ui_left_panel_collapsed,
            settings.ui_panel_multi_open,
            active_left_panel.is_some(),
            !settings.ui_left_open_panels.is_empty(),
        );
        let mut right_inspector_collapsed = panel_collapsed_from_persistence(
            settings.ui_right_panel_collapsed,
            settings.ui_panel_multi_open,
            active_right_panel.is_some(),
            !settings.ui_right_open_panels.is_empty(),
        );
        let security_secrets_unlocked = !settings.has_master_password;
        let mut left_open_panels = settings.ui_left_open_panels.clone();
        let mut right_open_panels = settings.ui_right_open_panels.clone();
        let panel_stack_sizes = settings
            .ui_panel_stack_sizes
            .iter()
            .filter(|(_, value)| **value > 0)
            .map(|(key, value)| (key.clone(), (*value as f32) / 1000.))
            .collect::<HashMap<_, _>>();
        let panel_multi_open = settings.ui_panel_multi_open;
        let panel_open_mode =
            crate::models::PanelOpenMode::from_setting(&settings.ui_panel_open_mode);
        if panel_open_mode.is_floating() {
            active_left_panel = None;
            active_right_panel = None;
            left_open_panels.clear();
            right_open_panels.clear();
            left_sidebar_collapsed = true;
            right_inspector_collapsed = true;
        }
        let mut terminal_output_decoder = TerminalOutputDecoder::default();
        terminal_output_decoder.set_encoding(&settings.interaction_default_encoding);
        let mut terminal_screen = initial_terminal_screen();
        terminal_screen.set_encoding(&settings.interaction_default_encoding);
        let session_manager = Arc::new(SessionManager::new());
        let terminal_frame_pipeline = TerminalFramePipeline::spawn(recording_writer);
        let session_event_bridge = SessionEventBridge::spawn(
            Arc::clone(&session_manager),
            terminal_frame_pipeline.clone(),
            settings.interaction_default_encoding.clone(),
            settings.terminal_scrollback_lines.clamp(100, 100_000) as usize,
        );
        stores.startup_restore.update(cx, |store, _| {
            store.set_loaded_open_tabs(open_tabs);
        });

        let connections_filter_placeholder = t!("savedConnections.filter");
        let command_store = store_blocking.clone();

        let app_entity = cx.entity();
        let remote_panels =
            crate::features::pages::remote::RemotePanels::new(app_entity.downgrade(), cx);
        let connection_panel =
            cx.new(|cx| ConnectionPanel::new(app_entity.downgrade(), cx.focus_handle()));
        let settings_panel = cx.new(|cx| SettingsPanel::new(app_entity.downgrade(), cx));
        let transfer_panel = cx.new(|_| TransferPanel::new(app_entity.downgrade()));
        let notes_panel = cx.new(|cx| NotesPanel::new(app_entity.downgrade(), cx));
        let ai_panel = cx.new(|_| AiPanel::new(app_entity.downgrade()));
        let start_workspace = StartWorkspaceFeatureState::new(&connection_groups, &settings, cx);

        // Settings are loaded after gpui-component initialization, so this is the
        // first point at which the persisted shortcut map can replace the defaults.
        crate::shortcuts::rebuild_keymap(&settings.keybindings, cx);

        let blocking_jobs = crate::blocking_jobs::BlockingJobScheduler::new();
        Self {
            blocking_jobs: blocking_jobs.clone(),
            stores,
            store_ui,
            store_blocking,
            runtime,
            connection_state: ConnectionFeatureState::new(
                connections,
                connection_groups,
                &settings,
                ConnectionFeatureFocus {
                    filter_placeholder: connections_filter_placeholder.into(),
                    editor: cx.focus_handle(),
                },
                cx,
            ),
            start_workspace,
            connection_panel,
            settings_panel,
            native_settings_panel: None,
            transfer_panel,
            notes: NotesFeatureState::new(),
            notes_panel,
            commands: CommandFeatureState::new(CommandFeatureInit {
                commands: quick_commands,
                categories: quick_command_categories,
                history: command_history,
                sort_mode: quick_command_sort_mode_from_setting(&settings.ui_quick_cmd_sort_mode),
                view_mode: quick_command_view_mode_from_setting(&settings.ui_quick_cmd_view_mode),
                focus: QuickCommandFeatureFocus {
                    editor: cx.focus_handle(),
                    details: cx.focus_handle(),
                    variable: cx.focus_handle(),
                },
                store: command_store,
                scheduler: blocking_jobs,
            }),
            send_command: SendCommandFeatureState::new(SendCommandFeatureFocus {
                editor: cx.focus_handle(),
            }),
            terminal: TerminalFeatureState::new(
                terminal_screen,
                terminal_output_decoder,
                terminal_frame_pipeline,
                String::from(INITIAL_TERMINAL_BANNER),
                1.0,
                TerminalFeatureFocus {
                    actions: cx.focus_handle(),
                    terminal: cx.focus_handle(),
                    paste: cx.focus_handle(),
                },
            ),
            ai: AiFeatureState::new(
                AiFeatureInit {
                    settings: ai_settings,
                    model_draft: ai_model_draft,
                    base_url_draft: ai_base_url_draft,
                    chat_session_id: format!("ai-session-{}", uuid()),
                    session_count: ai_session_count,
                    message_count: ai_message_count,
                    audit_count: ai_audit_count,
                },
                AiFeatureFocus {
                    chat: cx.focus_handle(),
                    action: cx.focus_handle(),
                    manual_model: cx.focus_handle(),
                    credential: cx.focus_handle(),
                },
            ),
            ai_panel,
            mcp,
            transfer: TransferFeatureState::new(
                ".".to_string(),
                String::new(),
                transfer_duplicate_policy,
                transfer_panel_height,
                TransferFeatureFocus {
                    queue: cx.focus_handle(),
                    browser: cx.focus_handle(),
                    editor: cx.focus_handle(),
                    preview: cx.focus_handle(),
                    external_sync: cx.focus_handle(),
                },
            ),
            security: SecurityFeatureState::new(
                SecurityCatalogState::new(
                    connection_ssh_keys,
                    connection_otp_entries,
                    connection_saved_passwords,
                    connection_saved_credentials,
                ),
                security_secrets_unlocked,
                "security ready".to_string(),
                SecurityFeatureFocus {
                    key_editor: cx.focus_handle(),
                    otp_editor: cx.focus_handle(),
                    password_editor: cx.focus_handle(),
                    credential_editor: cx.focus_handle(),
                    unlock: cx.focus_handle(),
                    screen_lock: cx.focus_handle(),
                },
            ),
            remote_ops: RemoteOpsFeatureState::new(RemoteOpsFeatureFocus {}),
            remote_panels,
            remote_desktop: RemoteDesktopFeatureState::new(cx.focus_handle()),
            translation: TranslationFeatureState::new(translation_settings),
            update: UpdateFeatureState::new(),
            cloud_sync: CloudSyncFeatureState::new(
                cloud_sync_settings,
                cloud_sync_state,
                Vec::new(),
            ),
            session: SessionFeatureState::new(
                session_manager,
                session_event_bridge,
                otp_provider,
                SessionFeatureFocus {
                    credential: cx.focus_handle(),
                    tab_actions: cx.focus_handle(),
                    color_picker: cx.focus_handle(),
                    info: cx.focus_handle(),
                },
            ),
            text_inputs: TextInputRegistry::default(),
            selects: SelectRegistry::default(),
            shell: ShellFeatureState::new(ShellFeatureInit {
                status: "idle".to_string(),
                bottom_panel_mode: if settings.ui_serial_send_visible {
                    BottomPanelMode::CommandSend
                } else if settings.ui_quick_cmd_visible {
                    BottomPanelMode::QuickCommands
                } else {
                    BottomPanelMode::Hidden
                },
                quick_commands_height: quick_cmd_height,
                command_send_height: serial_send_height,
                active_left_panel,
                active_right_panel,
                left_open_panels,
                right_open_panels,
                panel_stack_sizes,
                panel_open_mode,
                panel_multi_open,
                left_sidebar_collapsed,
                right_inspector_collapsed,
                left_panel_width,
                right_panel_width,
                activity_bar_layout,
            }),
            sync_input: SyncInputFeatureState::new(cx.focus_handle()),
            settings: SettingsFeatureState::new(
                SettingsFeatureInit {
                    summary: settings,
                    keyword_config: keyword_highlights,
                    store_path: store_status.0,
                    store_message: store_status.1,
                    store_ready: store_status.2,
                    ui_font_options: Vec::new(),
                    terminal_font_options: Vec::new(),
                },
                SettingsFeatureFocus {
                    search_engine: cx.focus_handle(),
                    keyword_highlight: cx.focus_handle(),
                    keybindings: cx.focus_handle(),
                },
            ),
            recording,
            tunnel_state: TunnelFeatureState::new(TunnelCatalogState::new(
                tunnels,
                tunnel_groups,
                proxies,
                proxy_groups,
            )),
            #[cfg(test)]
            _test_config_dir: None,
        }
    }

    pub(crate) fn start_shell_environment_preload(&self, cx: &mut Context<Self>) {
        let shell_environment = self.session.manager().shell_environment();
        cx.background_executor()
            .spawn(async move {
                // Preload the complete shell environment after the production window
                // starts so features can share it without affecting test construction.
                if let Err(error) = shell_environment.initialize().await {
                    // Preload failure must not block local terminals; transport keeps an
                    // inherited fallback. Never log shell stderr or environment values.
                    tracing::error!(%error, "shell environment preload failed");
                }
            })
            .detach();
    }

    #[cfg(test)]
    pub fn new(
        runtime: AppRuntime,
        stores: crate::entities::UiStoreHandles,
        cx: &mut Context<Self>,
    ) -> Self {
        let test_config_dir =
            crate::test_support::TestConfigDir::from_path(runtime.data_dir().to_path_buf());
        let store_runtime = StoreRuntime::spawn(StoreConfig {
            config_dir: runtime.config_dir().to_path_buf(),
            portable_key_path: runtime.portable_key_path().map(ToOwned::to_owned),
        })
        .expect("spawn test store runtime");
        let store_ui = store_runtime.ui_client();
        let store_blocking = store_runtime.blocking_client();
        let bootstrap = store_blocking
            .request(0, LoadBootstrap)
            .expect("receive test bootstrap")
            .outcome
            .expect("load test bootstrap");
        let mut app =
            Self::from_bootstrap(runtime, stores, bootstrap, store_ui, store_blocking, cx);
        app._test_config_dir = Some(test_config_dir);
        app
    }
}
