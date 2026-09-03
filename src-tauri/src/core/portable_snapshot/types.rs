#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PortableSnapshotKind {
    Sync,
    Backup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentEndpointTargetPlatform {
    Unix,
    Windows,
}

impl AgentEndpointTargetPlatform {
    pub(crate) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

fn agent_endpoint_supported_on_platform(
    endpoint: &config::SshAgentEndpoint,
    platform: AgentEndpointTargetPlatform,
) -> bool {
    match (platform, endpoint) {
        (_, config::SshAgentEndpoint::Auto) => true,
        (AgentEndpointTargetPlatform::Unix, config::SshAgentEndpoint::Environment { .. }) => true,
        (AgentEndpointTargetPlatform::Unix, config::SshAgentEndpoint::UnixSocket { .. }) => true,
        (AgentEndpointTargetPlatform::Windows, config::SshAgentEndpoint::Pageant) => true,
        (AgentEndpointTargetPlatform::Windows, config::SshAgentEndpoint::WindowsOpenSsh) => true,
        _ => false,
    }
}

/// Removes device-specific Agent endpoints that cannot work on the restore target.
pub(crate) fn normalize_backup_sessions_for_platform(
    sessions: &mut config::SessionsConfig,
    platform: AgentEndpointTargetPlatform,
) -> crate::error::AppResult<bool> {
    let mut changed = false;
    for connection in &mut sessions.connections {
        #[allow(deprecated)]
        {
            changed |= config::migrate_legacy_ssh_agent_settings(connection);
        }
        let config::ConnectionType::Ssh {
            auth_agent_endpoint,
            agent_forwarding_config,
            ..
        } = &mut connection.config
        else {
            continue;
        };

        if let Some(endpoint) = auth_agent_endpoint {
            config::validate_ssh_agent_endpoint_shape(endpoint)?;
        }
        if let Some(forwarding) = agent_forwarding_config {
            config::validate_ssh_agent_forwarding_shape(forwarding)?;
        }

        let auth_uses_agent = connection
            .auth
            .as_ref()
            .is_some_and(|auth| auth.mode == "agent");
        if auth_agent_endpoint
            .as_ref()
            .is_some_and(|endpoint| !agent_endpoint_supported_on_platform(endpoint, platform))
        {
            *auth_agent_endpoint = auth_uses_agent.then_some(config::SshAgentEndpoint::Auto);
            changed = true;
        }

        if let Some(forwarding) = agent_forwarding_config {
            let original_len = forwarding.sources.external_agent_endpoints.len();
            forwarding
                .sources
                .external_agent_endpoints
                .retain(|endpoint| agent_endpoint_supported_on_platform(endpoint, platform));
            changed |= original_len != forwarding.sources.external_agent_endpoints.len();

            if forwarding.sources.external_agent_endpoints.is_empty()
                && forwarding.sources.external_agent
            {
                forwarding.sources.external_agent = false;
                changed = true;
            }
            if !forwarding.sources.external_agent && !forwarding.sources.stored_keys {
                if forwarding.enabled {
                    forwarding.enabled = false;
                    changed = true;
                }
            }
        }
    }
    Ok(changed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableSnapshot {
    pub schema_version: u32,
    pub snapshot_kind: PortableSnapshotKind,
    pub revision_id: String,
    pub device_id: String,
    pub created_at_ms: u64,
    pub payload_hash: String,
    pub app_version: String,
    pub settings: PortableAppSettings,
    #[serde(default)]
    pub sessions: config::SessionsConfig,
    #[serde(default)]
    pub keys: config::KeysConfig,
    #[serde(default)]
    pub passwords: config::PasswordsConfig,
    #[serde(default)]
    pub credentials: config::CredentialsConfig,
    #[serde(default)]
    pub otp: config::OtpConfig,
    #[serde(default)]
    pub proxies: Vec<config::ProxyConfig>,
    #[serde(default)]
    pub proxy_groups: Vec<config::ProxyGroup>,
    #[serde(default)]
    pub tunnels: Vec<config::TunnelConfig>,
    #[serde(default)]
    pub tunnel_groups: Vec<config::TunnelGroup>,
    #[serde(default)]
    pub quick_commands: config::QuickCommandsConfig,
    #[serde(default)]
    pub history: Vec<crate::core::history::HistoryEntry>,
    #[serde(default)]
    pub master_key_token: Option<String>,
    #[serde(default)]
    pub known_hosts: String,
    #[serde(default)]
    pub notes: config::NotesSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DecodedPortableSnapshot {
    pub snapshot: PortableSnapshot,
    pub source_payload_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortableSnapshotMeta {
    schema_version: u32,
    snapshot_kind: PortableSnapshotKind,
    revision_id: String,
    device_id: String,
    created_at_ms: u64,
    payload_hash: String,
    app_version: String,
}

impl From<&PortableSnapshot> for PortableSnapshotMeta {
    fn from(snapshot: &PortableSnapshot) -> Self {
        Self {
            schema_version: snapshot.schema_version,
            snapshot_kind: snapshot.snapshot_kind.clone(),
            revision_id: snapshot.revision_id.clone(),
            device_id: snapshot.device_id.clone(),
            created_at_ms: snapshot.created_at_ms,
            payload_hash: snapshot.payload_hash.clone(),
            app_version: snapshot.app_version.clone(),
        }
    }
}

#[derive(Serialize)]
struct SnapshotHashInput<'a> {
    settings: &'a PortableAppSettings,
    sessions: &'a config::SessionsConfig,
    keys: &'a config::KeysConfig,
    passwords: &'a config::PasswordsConfig,
    credentials: &'a config::CredentialsConfig,
    otp: &'a config::OtpConfig,
    proxies: &'a [config::ProxyConfig],
    proxy_groups: &'a [config::ProxyGroup],
    tunnels: &'a [config::TunnelConfig],
    tunnel_groups: &'a [config::TunnelGroup],
    quick_commands: &'a config::QuickCommandsConfig,
    history: &'a [crate::core::history::HistoryEntry],
    master_key_token: &'a Option<String>,
    known_hosts: &'a str,
    notes: &'a config::NotesSnapshot,
}

#[derive(Serialize)]
struct SnapshotRawHashInput<'a> {
    settings: &'a RawValue,
    sessions: &'a RawValue,
    keys: &'a RawValue,
    passwords: &'a RawValue,
    credentials: &'a RawValue,
    otp: &'a RawValue,
    proxies: &'a RawValue,
    proxy_groups: &'a RawValue,
    tunnels: &'a RawValue,
    tunnel_groups: &'a RawValue,
    quick_commands: &'a RawValue,
    history: &'a RawValue,
    master_key_token: &'a RawValue,
    known_hosts: &'a RawValue,
}

#[derive(Serialize)]
struct SnapshotRawHashInputWithNotes<'a> {
    settings: &'a RawValue,
    sessions: &'a RawValue,
    keys: &'a RawValue,
    passwords: &'a RawValue,
    credentials: &'a RawValue,
    otp: &'a RawValue,
    proxies: &'a RawValue,
    proxy_groups: &'a RawValue,
    tunnels: &'a RawValue,
    tunnel_groups: &'a RawValue,
    quick_commands: &'a RawValue,
    history: &'a RawValue,
    master_key_token: &'a RawValue,
    known_hosts: &'a RawValue,
    notes: &'a RawValue,
}

#[derive(Serialize)]
struct LegacySnapshotRawHashInput<'a> {
    settings: &'a RawValue,
    sessions: &'a RawValue,
    keys: &'a RawValue,
    passwords: &'a RawValue,
    credentials: &'a RawValue,
    otp: &'a RawValue,
    proxies: &'a RawValue,
    tunnels: &'a RawValue,
    quick_commands: &'a RawValue,
    history: &'a RawValue,
    master_key_token: &'a RawValue,
    known_hosts: &'a RawValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableUiSettings {
    pub language: Option<String>,
    #[serde(default = "default_portable_panel_open_mode")]
    pub panel_open_mode: String,
    #[serde(default)]
    pub serial_send_clear_after_send: bool,
    #[serde(default = "default_portable_true")]
    pub header_status_visible: bool,
    pub show_remote_stats: bool,
    pub remote_stats_interval: u32,
    #[serde(default)]
    pub show_gpu_monitor: bool,
    #[serde(default = "default_portable_gpu_monitor_interval")]
    pub gpu_monitor_interval: u32,
    #[serde(default)]
    pub show_ascend_npu_monitor: bool,
    #[serde(default = "default_portable_ascend_npu_monitor_interval")]
    pub ascend_npu_monitor_interval: u32,
    #[serde(default)]
    pub show_process_manager: bool,
    #[serde(default = "default_portable_process_manager_interval")]
    pub process_manager_interval: u32,
    #[serde(default)]
    pub show_docker_manager: bool,
    #[serde(default = "default_portable_docker_manager_interval")]
    pub docker_manager_interval: u32,
    pub saved_connections_sort_mode: String,
    #[serde(default)]
    pub asset_sort_key: Option<String>,
    #[serde(default)]
    pub asset_sort_direction: Option<String>,
    pub activity_bar_layout: ActivityBarLayout,
}

fn default_portable_gpu_monitor_interval() -> u32 {
    3
}

fn default_portable_panel_open_mode() -> String {
    "docked".to_string()
}

fn default_portable_true() -> bool {
    true
}

fn default_portable_ascend_npu_monitor_interval() -> u32 {
    3
}

fn default_portable_process_manager_interval() -> u32 {
    5
}

fn default_portable_docker_manager_interval() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableAppSettings {
    pub general: config::GeneralSettings,
    pub appearance: config::AppearanceSettings,
    pub proxy: config::ProxySettings,
    pub search: SearchSettings,
    pub translation: TranslationSettings,
    pub security: config::SecuritySettings,
    pub terminal: TerminalSettings,
    pub interaction: InteractionSettings,
    pub transfer: TransferSettings,
    pub diagnostics: DiagnosticsSettings,
    #[serde(default)]
    pub ai: config::AiSettings,
    pub ui: PortableUiSettings,
}

impl PortableAppSettings {
    pub fn from_app_settings(settings: &AppSettings, snapshot_kind: &PortableSnapshotKind) -> Self {
        let mut security = settings.security.clone();
        security.master_password = None;
        let mut appearance = settings.appearance.clone();
        let mut transfer = settings.transfer.clone();
        if snapshot_kind == &PortableSnapshotKind::Sync {
            strip_device_local_settings(&mut appearance, &mut transfer);
        }
        Self {
            general: settings.general.clone(),
            appearance,
            proxy: settings.proxy.clone(),
            search: settings.search.clone(),
            translation: settings.translation.clone(),
            security,
            terminal: settings.terminal.clone(),
            interaction: settings.interaction.clone(),
            transfer,
            diagnostics: settings.diagnostics.clone(),
            ai: settings.ai.clone(),
            ui: PortableUiSettings {
                language: settings.ui.language.clone(),
                panel_open_mode: settings.ui.panel_open_mode.clone(),
                serial_send_clear_after_send: settings.ui.serial_send_clear_after_send,
                header_status_visible: settings.ui.header_status_visible,
                show_remote_stats: settings.ui.show_remote_stats,
                remote_stats_interval: settings.ui.remote_stats_interval,
                show_gpu_monitor: settings.ui.show_gpu_monitor,
                gpu_monitor_interval: settings.ui.gpu_monitor_interval,
                show_ascend_npu_monitor: settings.ui.show_ascend_npu_monitor,
                ascend_npu_monitor_interval: settings.ui.ascend_npu_monitor_interval,
                show_process_manager: settings.ui.show_process_manager,
                process_manager_interval: settings.ui.process_manager_interval,
                show_docker_manager: settings.ui.show_docker_manager,
                docker_manager_interval: settings.ui.docker_manager_interval,
                saved_connections_sort_mode: settings.ui.saved_connections_sort_mode.clone(),
                asset_sort_key: settings.ui.asset_sort_key.clone(),
                asset_sort_direction: settings.ui.asset_sort_direction.clone(),
                activity_bar_layout: settings.ui.activity_bar_layout.clone(),
            },
        }
    }

    pub fn apply_to(
        self,
        mut current: AppSettings,
        snapshot_kind: &PortableSnapshotKind,
    ) -> AppSettings {
        let mut snapshot_security = self.security;
        snapshot_security.migrate_legacy_screen_lock();
        let master_password = current.security.master_password.clone();
        let ui_state = current.ui.clone();
        let device_appearance = current.appearance.clone();
        let device_transfer = current.transfer.clone();

        current.general = self.general;
        current.appearance = self.appearance;
        current.proxy = self.proxy;
        current.search = self.search;
        current.translation = self.translation;
        current.security = snapshot_security;
        current.security.master_password = master_password;
        current.terminal = self.terminal;
        current.interaction = self.interaction;
        current.transfer = self.transfer;
        if snapshot_kind == &PortableSnapshotKind::Sync {
            preserve_device_local_settings(
                &mut current.appearance,
                &mut current.transfer,
                &device_appearance,
                &device_transfer,
            );
        }
        current.diagnostics = self.diagnostics;
        current.ai = self.ai;
        config::normalize_ai_settings(&mut current.ai);
        current.ui.language = self.ui.language;
        current.ui.panel_open_mode = self.ui.panel_open_mode;
        current.ui.serial_send_clear_after_send = self.ui.serial_send_clear_after_send;
        current.ui.header_status_visible = self.ui.header_status_visible;
        current.ui.show_remote_stats = self.ui.show_remote_stats;
        current.ui.remote_stats_interval = self.ui.remote_stats_interval;
        current.ui.show_gpu_monitor = self.ui.show_gpu_monitor;
        current.ui.gpu_monitor_interval = self.ui.gpu_monitor_interval;
        current.ui.show_ascend_npu_monitor = self.ui.show_ascend_npu_monitor;
        current.ui.ascend_npu_monitor_interval = self.ui.ascend_npu_monitor_interval;
        current.ui.show_process_manager = self.ui.show_process_manager;
        current.ui.process_manager_interval = self.ui.process_manager_interval;
        current.ui.show_docker_manager = self.ui.show_docker_manager;
        current.ui.docker_manager_interval = self.ui.docker_manager_interval;
        current.ui.saved_connections_sort_mode = self.ui.saved_connections_sort_mode;
        current.ui.asset_sort_key = self.ui.asset_sort_key;
        current.ui.asset_sort_direction = self.ui.asset_sort_direction;
        current.ui.activity_bar_layout = self.ui.activity_bar_layout;

        // Preserve device-local UI state.
        current.ui.open_tabs = ui_state.open_tabs;
        current.ui.left_width = ui_state.left_width;
        current.ui.right_width = ui_state.right_width;
        current.ui.quick_cmd_height = ui_state.quick_cmd_height;
        current.ui.quick_cmd_category_width = ui_state.quick_cmd_category_width;
        current.ui.quick_cmd_selected_category = ui_state.quick_cmd_selected_category;
        current.ui.active_left_panel = ui_state.active_left_panel;
        current.ui.active_right_panel = ui_state.active_right_panel;
        current.ui.show_quick_cmd_bar = ui_state.show_quick_cmd_bar;
        current.ui.show_serial_send_panel = ui_state.show_serial_send_panel;
        current.ui.serial_send_height = ui_state.serial_send_height;
        current.ui.zoom_level = ui_state.zoom_level;
        current.ui.transfer_height = ui_state.transfer_height;
        current.ui.notes_expanded_folder_ids = ui_state.notes_expanded_folder_ids;
        current.ui.notes_last_selected_node_id = ui_state.notes_last_selected_node_id;
        current
    }
}

fn strip_device_local_settings(
    appearance: &mut config::AppearanceSettings,
    transfer: &mut TransferSettings,
) {
    let defaults = config::AppearanceSettings::default();
    appearance.background_image_path = None;
    appearance.background_image_fit = defaults.background_image_fit;
    appearance.background_image_opacity = defaults.background_image_opacity;
    transfer.download_path.clear();
    transfer.default_editor.clear();
    transfer.recording_path.clear();
}

fn preserve_device_local_settings(
    appearance: &mut config::AppearanceSettings,
    transfer: &mut TransferSettings,
    device_appearance: &config::AppearanceSettings,
    device_transfer: &TransferSettings,
) {
    appearance.background_image_path = device_appearance.background_image_path.clone();
    appearance.background_image_fit = device_appearance.background_image_fit.clone();
    appearance.background_image_opacity = device_appearance.background_image_opacity;
    transfer.download_path = device_transfer.download_path.clone();
    transfer.default_editor = device_transfer.default_editor.clone();
    transfer.recording_path = device_transfer.recording_path.clone();
}

#[allow(deprecated)]
pub fn strip_device_local_sessions(sessions: &mut config::SessionsConfig) {
    for connection in &mut sessions.connections {
        match &mut connection.config {
            config::ConnectionType::LocalTerminal { .. } => {}
            config::ConnectionType::Serial { port_name, .. } => {
                port_name.clear();
            }
            config::ConnectionType::Ssh {
                auth_agent_endpoint,
                legacy_agent_forwarding,
                agent_forwarding_config,
                ..
            } => {
                *auth_agent_endpoint = None;
                *legacy_agent_forwarding = None;
                *agent_forwarding_config = None;
            }
            config::ConnectionType::Telnet { .. }
            | config::ConnectionType::Rdp { .. }
            | config::ConnectionType::Vnc { .. } => {}
        }
    }
}

#[allow(deprecated)]
pub fn preserve_device_local_sessions(
    incoming: &mut config::SessionsConfig,
    current: &config::SessionsConfig,
) {
    for connection in &mut incoming.connections {
        let Some(device_connection) = current
            .connections
            .iter()
            .find(|candidate| candidate.id == connection.id)
        else {
            continue;
        };

        match (&mut connection.config, &device_connection.config) {
            (
                config::ConnectionType::LocalTerminal {
                    shell_path,
                    shell_args,
                    working_dir,
                    ..
                },
                config::ConnectionType::LocalTerminal {
                    shell_path: device_shell_path,
                    shell_args: device_shell_args,
                    working_dir: device_working_dir,
                    ..
                },
            ) => {
                if is_empty_local_terminal_config(shell_path, shell_args, working_dir)
                    && !is_empty_local_terminal_config(
                        device_shell_path,
                        device_shell_args,
                        device_working_dir,
                    )
                {
                    *shell_path = device_shell_path.clone();
                    *shell_args = device_shell_args.clone();
                    *working_dir = device_working_dir.clone();
                }
            }
            (
                config::ConnectionType::Serial { port_name, .. },
                config::ConnectionType::Serial {
                    port_name: device_port_name,
                    ..
                },
            ) => {
                *port_name = device_port_name.clone();
            }
            (
                config::ConnectionType::Ssh {
                    auth_agent_endpoint,
                    legacy_agent_forwarding,
                    agent_forwarding_config,
                    ..
                },
                config::ConnectionType::Ssh {
                    auth_agent_endpoint: device_auth_agent_endpoint,
                    legacy_agent_forwarding: device_legacy_agent_forwarding,
                    agent_forwarding_config: device_agent_forwarding_config,
                    ..
                },
            ) => {
                *auth_agent_endpoint = device_auth_agent_endpoint.clone();
                *legacy_agent_forwarding = *device_legacy_agent_forwarding;
                *agent_forwarding_config = device_agent_forwarding_config.clone();
            }
            _ => {}
        }
    }
}

fn is_empty_local_terminal_config(
    shell_path: &str,
    shell_args: &str,
    working_dir: &Option<String>,
) -> bool {
    shell_path.trim().is_empty()
        && shell_args.trim().is_empty()
        && working_dir
            .as_deref()
            .is_none_or(|working_dir| working_dir.trim().is_empty())
}
