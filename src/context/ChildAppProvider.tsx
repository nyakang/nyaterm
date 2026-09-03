import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import LockScreen from "@/components/dialog/app/LockScreen";
import { useAppLockState } from "@/hooks/useAppLockState";
import { useIdleLock } from "@/hooks/useIdleLock";
import { DEFAULT_AI_SETTINGS } from "@/lib/aiSettings";
import { DEFAULT_CLOUD_SYNC_SETTINGS } from "@/lib/cloudSync";
import {
  DEFAULT_TERMINAL_FONT_FAMILY,
  getDefaultUiFontFamily,
} from "@/lib/defaultFonts";
import { cloneDefaultActivityBarLayout } from "@/lib/appWorkspace";
import {
  DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
  DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
  DEFAULT_TAB_DOUBLE_CLICK_ACTION,
  DEFAULT_TAB_MIDDLE_CLICK_ACTION,
  DEFAULT_TAB_RIGHT_CLICK_ACTION,
} from "@/lib/interactionSettings";
import { normalizeQuickCommandAppSettings } from "@/lib/quickCommandSettings";
import type {
  AppRuntimeInfo,
  AppSettings,
  Group,
  SavedConnection,
  UiConfig,
} from "@/types/global";
import i18n from "../i18n";
import { invoke } from "../lib/invoke";
import { logger, setLoggerLevel } from "../lib/logger";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../lib/terminalFontSize";
import { AppContext } from "./AppContext";

const DEFAULT_APP_SETTINGS: AppSettings = {
  general: {
    startup_restore: true,
    startup_restore_window_layout: true,
    minimize_to_tray: false,
    boss_key: null,
    confirm_on_close: true,
  },
  appearance: {
    theme: "github-dark",
    custom_themes: [],
    terminal_theme: "default",
    font_family: DEFAULT_TERMINAL_FONT_FAMILY,
    ui_font_family: getDefaultUiFontFamily(),
    font_size: DEFAULT_TERMINAL_FONT_SIZE,
    font_weight: 400,
    font_weight_bold: 700,
    background_opacity: 1.0,
    background_image_path: null,
    background_image_fit: "cover",
    background_image_opacity: 0.45,
    cursor_style: "block",
    cursor_blink: true,
    ui_font_size: 16,
    minimum_contrast_ratio: 1,
    panel_multi_open: false,
    window_transparency: "none",
    window_transparency_tint: 1,
    window_transparency_blur: false,
  },
  proxy: { enabled: false, protocol: "socks5", host: "127.0.0.1", port: 1080 },
  search: { custom_engines: [] },
  translation: {
    target_language: "zh-CN",
    deepl_api_key: "",
    baidu_app_id: "",
    baidu_app_key: "",
    ali_app_id: "",
    ali_app_key: "",
    youdao_app_id: "",
    youdao_app_key: "",
  },
  security: {
    use_os_keyring: true,
    enable_startup_lock: false,
    enable_idle_lock: false,
    idle_lock_minutes: 0,
    host_key_policy: "prompt",
  },
  terminal: {
    scrollback_lines: 10000,
    keep_alive_mode: "compatible",
    keep_alive_interval: 60,
    font_size_delta: 0,
    x11_display: "",
    hardware_acceleration: false,
    keyword_highlights_enabled: false,
    keyword_highlights_across_wrapped_lines: false,
    keyword_highlight_builtin_rules: {},
    keyword_highlights: [],
    action_links_enabled: false,
    action_links_matchers: {
      ipv4: true,
      archive: true,
      host_port: true,
    },
    show_workspace_padding: false,
    show_line_numbers: false,
    show_timestamps: false,
    timestamp_format: "[HH:mm:ss]",
    show_multi_line_paste_dialog: true,
    paste_image_as_path: true,
    reconnect_restore_cwd: false,
  },
  interaction: {
    copy_on_select: false,
    allow_osc52_clipboard_write: false,
    terminal_right_click_action: "menu",
    terminal_zoom_enabled: true,
    command_suggestions_enabled: true,
    command_suggestion_min_chars: DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
    command_suggestion_max_chars: DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
    duplicate_session_command_delay_ms: 1000,
    word_separators: " ()[]{}\"':=,;|&<>",
    alt_as_meta: false,
    ime_compatibility: false,
    default_encoding: "UTF-8",
    tab_double_click_action: DEFAULT_TAB_DOUBLE_CLICK_ACTION,
    tab_middle_click_action: DEFAULT_TAB_MIDDLE_CLICK_ACTION,
    tab_right_click_action: DEFAULT_TAB_RIGHT_CLICK_ACTION,
  },
  recording: {
    auto_start: false,
    default_mode: "transcript",
    base_path: "",
    path_template:
      "{group}/{session}/{yyyy}-{MM}-{dd}/{HH}-{mm}-{ss}-{SSS}-{session_short_id}.log",
    include_timestamps: true,
    include_io_labels: true,
    include_session_metadata: true,
    rotation: { type: "session" },
    existing_file_behavior: "unique",
    memory_limit_bytes: 5 * 1024 * 1024,
    include_binary_transfer_payloads: false,
  },
  transfer: {
    editor_type: "external",
    internal_editor_display: "workspace",
    internal_editor_font_size: 13,
    download_threads: 3,
    upload_threads: 3,
    duplicate_strategy: "ask",
    preserve_timestamps: true,
    resume_broken_transfer: true,
    default_file_permissions: "644",
    max_transfer_retries: 2,
    transfer_buffer_size: 32,
    download_path: "",
    ask_save_location: false,
    default_editor: "",
    recording_path: "",
    recording_include_io_labels: true,
    recording_include_timestamps: true,
    recording_auto_start: false,
    recording_memory_limit_bytes: 5 * 1024 * 1024,
  },
  diagnostics: {
    level: "info",
    retention_days: 7,
  },
  ai: {
    ...DEFAULT_AI_SETTINGS,
  },
  cloud_sync: DEFAULT_CLOUD_SYNC_SETTINGS,
  ui: {
    open_tabs: [],
    terminal_window_layout: null,
    start_workspace_mode: "workbench",
    panel_open_mode: "docked",
    left_width: 256,
    right_width: 288,
    quick_cmd_height: 180,
    quick_cmd_category_width: 176,
    quick_cmd_view_mode: "tile",
    quick_cmd_sort_mode: "created",
    quick_cmd_selected_category: "all",
    active_left_panel: "fileExplorer",
    active_right_panel: "savedConnections",
    left_open_panels: [],
    right_open_panels: [],
    panel_stack_sizes: {},
    network_panel_active_tab: "tunnel",
    security_auth_panel_active_tab: "keys",
    show_quick_cmd_bar: true,
    show_serial_send_panel: false,
    serial_send_height: 180,
    serial_send_clear_after_send: false,
    zoom_level: 1.0,
    language: "en",
    header_status_mode: "session",
    header_status_visible: true,
    show_notes_panel: true,
    show_remote_stats: true,
    remote_stats_interval: 3,
    show_gpu_monitor: false,
    gpu_monitor_interval: 3,
    show_ascend_npu_monitor: false,
    ascend_npu_monitor_interval: 3,
    show_process_manager: false,
    process_manager_interval: 5,
    show_docker_manager: false,
    docker_manager_interval: 10,
    saved_connections_sort_mode: "default",
    saved_connections_expanded_group_ids: [],
    asset_sort_key: null,
    asset_sort_direction: null,
    recent_connection_ids: [],
    transfer_height: 180,
    file_explorer_show_hidden_files: true,
    file_explorer_auto_sync_cwd_connection_ids: [],
    file_explorer_favorite_dirs_by_connection_id: {},
    notes_expanded_folder_ids: [],
    notes_last_selected_node_id: null,
    activity_bar_layout: cloneDefaultActivityBarLayout(),
  },
  keybindings: {},
};

const DEFAULT_RUNTIME_INFO: AppRuntimeInfo = {
  portable: false,
  mode: "installed",
  executableDir: "",
  dataDir: "",
  configDir: "",
  logDir: "",
  webviewDataDir: "",
  portableMarkerPath: null,
};

/**
 * Lightweight AppContext provider for child windows (settings, new-session, etc.).
 * Loads/saves appSettings via backend and emits cross-window Tauri events.
 * Tabs, connections, and dialog state are stubbed since child windows don't use them.
 */
export function ChildAppProvider({ children }: { children: ReactNode }) {
  const [appSettings, setAppSettings] =
    useState<AppSettings>(DEFAULT_APP_SETTINGS);
  const loaded = useRef(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const { isLocked, setIsLocked, lockStateLoaded } = useAppLockState();

  const loadAppSettings = useCallback(() => {
    invoke<AppSettings>("get_app_settings")
      .then((cfg) => {
        const normalized = normalizeQuickCommandAppSettings(cfg);
        setAppSettings(normalized);
        setLoggerLevel(normalized.diagnostics.level);
        loaded.current = true;
        setSettingsLoaded(true);
        if (
          normalized.ui?.language &&
          normalized.ui.language !== i18n.language
        ) {
          i18n.changeLanguage(normalized.ui.language);
        }
      })
      .catch(() => {
        loaded.current = true;
        setSettingsLoaded(true);
      });
  }, []);

  useEffect(() => {
    loadAppSettings();
  }, [loadAppSettings]);

  useEffect(() => {
    const unlisten = listen("settings-changed", () => {
      loadAppSettings();
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, [loadAppSettings]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWindow()
      .onFocusChanged((event) => {
        if (event.payload) {
          loadAppSettings();
        }
      })
      .then((dispose) => {
        unlisten = dispose;
      });

    return () => {
      unlisten?.();
    };
  }, [loadAppSettings]);

  useEffect(() => {
    document.documentElement.style.fontSize = `${appSettings.appearance.ui_font_size}px`;
  }, [appSettings.appearance.ui_font_size]);

  useEffect(() => {
    const fontFamily = appSettings.appearance.ui_font_family;
    document.documentElement.style.setProperty("--font-sans", fontFamily);
    document.documentElement.style.setProperty("--font-display", fontFamily);
  }, [appSettings.appearance.ui_font_family]);

  useEffect(() => {
    if (appSettings.ui?.language && appSettings.ui.language !== i18n.language) {
      i18n.changeLanguage(appSettings.ui.language);
    }
  }, [appSettings.ui?.language]);

  useIdleLock(
    appSettings.security.enable_idle_lock
      ? appSettings.security.idle_lock_minutes
      : 0,
    isLocked,
    () => setIsLocked(true),
  );

  const updateAppSettings = useCallback(
    (
      updates:
        | Partial<AppSettings>
        | ((prev: AppSettings) => Partial<AppSettings>),
    ) => {
      setAppSettings((prev) => {
        const nextUpdates =
          typeof updates === "function" ? updates(prev) : updates;
        const next = normalizeQuickCommandAppSettings({
          ...prev,
          ...nextUpdates,
        });
        setLoggerLevel(next.diagnostics.level);
        if (loaded.current) {
          if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
          saveTimerRef.current = setTimeout(() => {
            invoke("save_app_settings", { settings: next }).catch((e) =>
              logger.error({
                domain: "settings.persistence",
                event: "settings.save_failed",
                message: "Failed to save app settings",
                error: e,
              }),
            );
          }, 500);
        }
        return next;
      });
    },
    [],
  );

  const replaceAppSettings = useCallback((next: AppSettings) => {
    if (saveTimerRef.current) {
      clearTimeout(saveTimerRef.current);
      saveTimerRef.current = null;
    }
    const normalized = normalizeQuickCommandAppSettings(next);
    setLoggerLevel(normalized.diagnostics.level);
    setAppSettings(normalized);
  }, []);

  const updateUi = useCallback(
    (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => {
      updateAppSettings((prev) => {
        const nextUpdates =
          typeof updates === "function" ? updates(prev.ui) : updates;
        return { ui: { ...prev.ui, ...nextUpdates } };
      });
    },
    [updateAppSettings],
  );

  const noop = useCallback(() => {}, []);
  const noopString = useCallback(() => "", []);
  const noopPendingTab = useCallback(
    () => ({ tabId: "", createRequestId: crypto.randomUUID() }),
    [],
  );
  const noopPaneConnecting = useCallback(() => null, []);
  const noopBoolean = useCallback(() => false, []);
  const noopAsync = useCallback(async () => {}, []);
  const noopSplitPane = useCallback(() => null, []);

  const emptyConnections = useMemo(() => [] as SavedConnection[], []);
  const emptyGroups = useMemo(() => [] as Group[], []);

  const contextValue = useMemo(
    () => ({
      tabs: [] as never[],
      activeTabId: null,
      setActiveTabId: noop,
      addTab: noopString,
      addPendingTab: noopPendingTab,
      updateTabSession: noop,
      markTabConnectionFailed: noop,
      updatePaneSession: noop,
      replaceSessionReferences: noop,
      markPaneConnectionFailed: noop,
      markPaneConnecting: noopPaneConnecting,
      hasTab: noopBoolean,
      hasPane: noopBoolean,
      setActivePane: noop,
      updateSplitRatio: noop,
      splitPane: noopSplitPane,
      openFileDocument: () => ({ tabId: "", paneId: "", created: false }),
      closePane: noop,
      reorderTabs: noop,
      updateTab: noopAsync,
      closeTabs: noop,
      closeTab: noop,
      persistTabsNow: noopAsync,
      appSettings,
      updateAppSettings,
      replaceAppSettings,
      updateUi,
      savedConnections: emptyConnections,
      savedGroups: emptyGroups,
      refreshConnections: noopAsync,
      recordRecentConnection: noop,
      showNewSession: false,
      setShowNewSession: noop,
      editingConnection: undefined,
      setEditingConnection: noop,
      showSettingsDialog: false,
      setShowSettingsDialog: noop,
      syncGroups: [],
      setSyncGroups: noop,
      broadcastToAll: false,
      setBroadcastToAll: noop,
      isLocked,
      setIsLocked,
      settingsLoaded,
      startupRestoreComplete: true,
      runtimeInfo: DEFAULT_RUNTIME_INFO,
      runtimeInfoLoaded: true,
    }),
    [
      noop,
      noopString,
      noopPendingTab,
      noopPaneConnecting,
      noopBoolean,
      noopAsync,
      noopSplitPane,
      emptyConnections,
      emptyGroups,
      appSettings,
      updateAppSettings,
      replaceAppSettings,
      updateUi,
      isLocked,
      setIsLocked,
      settingsLoaded,
    ],
  );

  const appStateReady = settingsLoaded && lockStateLoaded;
  const showContent = appStateReady && !isLocked;

  return (
    <AppContext.Provider value={contextValue}>
      {!appStateReady ? (
        <div
          className="flex h-screen w-full items-center justify-center bg-background"
          aria-busy="true"
          style={{ backgroundColor: "var(--df-bg, #0d1117)" }}
        >
          <span className="size-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
        </div>
      ) : null}
      {showContent ? children : null}
      {appStateReady && isLocked ? (
        <LockScreen
          hasPassword={!!appSettings.security.master_password}
          onUnlock={() => setIsLocked(false)}
        />
      ) : null}
    </AppContext.Provider>
  );
}
