import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DEFAULT_CLOUD_SYNC_SETTINGS } from "@/lib/cloudSync";
import {
  DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
  DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
} from "@/lib/interactionSettings";
import type { AppSettings, Group, SavedConnection, UiConfig } from "@/types/global";
import i18n from "../i18n";
import { invoke } from "../lib/invoke";
import { logger, setLoggerLevel } from "../lib/logger";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../lib/terminalFontSize";
import { AppContext } from "./AppContext";

const DEFAULT_APP_SETTINGS: AppSettings = {
  general: {
    startup_restore: true,
    minimize_to_tray: false,
    boss_key: null,
    confirm_on_close: true,
  },
  appearance: {
    theme: "github-dark",
    terminal_theme: "default",
    font_family: "JetBrains Mono, 'Noto Sans SC Variable', Consolas, monospace, Inter",
    font_size: DEFAULT_TERMINAL_FONT_SIZE,
    ligatures: false,
    background_opacity: 1.0,
    cursor_style: "block",
    cursor_blink: true,
    ui_font_size: 16,
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
    enable_screen_lock: false,
    idle_lock_minutes: 0,
    host_key_policy: "prompt",
  },
  terminal: {
    scrollback_lines: 10000,
    keep_alive_interval: 60,
    hardware_acceleration: false,
    keyword_highlights_enabled: false,
    keyword_highlights_across_wrapped_lines: false,
    keyword_highlights: [],
    action_links_enabled: false,
    action_links_matchers: {
      ipv4: true,
      archive: true,
      host_port: true,
    },
    show_line_numbers: false,
    show_timestamps: false,
  },
  interaction: {
    copy_on_select: false,
    right_click_paste: false,
    command_suggestions_enabled: true,
    command_suggestion_min_chars: DEFAULT_COMMAND_SUGGESTION_MIN_CHARS,
    command_suggestion_max_chars: DEFAULT_COMMAND_SUGGESTION_MAX_CHARS,
    word_separators: " ()[]{}\"':=,;|&<>",
    default_encoding: "UTF-8",
  },
  transfer: {
    download_threads: 3,
    upload_threads: 3,
    duplicate_strategy: "overwrite",
    preserve_timestamps: true,
    resume_broken_transfer: true,
    default_file_permissions: "644",
    max_transfer_retries: 2,
    transfer_buffer_size: 32,
    download_path: "",
    ask_save_location: false,
    default_editor: "",
    recording_path: "",
  },
  diagnostics: {
    level: "info",
    retention_days: 7,
  },
  ai: {
    enabled: true,
    context_line_limit: 200,
    redaction_enabled: true,
    risk_check_enabled: true,
    allow_save_command: true,
    record_history: true,
    timeout_ms: 60000,
    max_output_tokens: 1200,
    active_profile_id: "openai",
    provider_profiles: [
      {
        id: "openai",
        name: "OpenAI",
        provider_kind: "openai",
        model: "gpt-4o-mini",
        base_url: null,
        api_key: null,
        enabled: true,
      },
      {
        id: "deepseek",
        name: "DeepSeek",
        provider_kind: "deepseek",
        model: "deepseek-chat",
        base_url: null,
        api_key: null,
        enabled: true,
      },
      {
        id: "ollama",
        name: "Ollama",
        provider_kind: "ollama",
        model: "qwen2.5-coder:7b",
        base_url: "http://localhost:11434/v1/",
        api_key: null,
        enabled: true,
      },
    ],
  },
  cloud_sync: DEFAULT_CLOUD_SYNC_SETTINGS,
  ui: {
    open_tabs: [],
    left_width: 256,
    right_width: 288,
    quick_cmd_height: 36,
    active_left_panel: "fileExplorer",
    active_right_panel: "savedConnections",
    show_quick_cmd_bar: true,
    show_serial_send_panel: false,
    serial_send_height: 120,
    zoom_level: 1.0,
    language: "en",
    show_remote_stats: false,
    remote_stats_interval: 3,
    saved_connections_sort_mode: "default",
    transfer_height: 180,
    activity_bar_layout: {
      left_top: ["fileExplorer", "network", "securityAuth"],
      left_bottom: ["syncBackupHistory", "settings"],
      right_top: [
        "savedConnections",
        "aiAssistant",
        "activeSessions",
        "commandHistory",
        "resourceMonitor",
      ],
      right_bottom: ["quickCmdBar", "serialSend", "recording", "lock"],
      show_labels: false,
    },
  },
};

/**
 * Lightweight AppContext provider for child windows (settings, new-session, etc.).
 * Loads/saves appSettings via backend and emits cross-window Tauri events.
 * Tabs, connections, and dialog state are stubbed since child windows don't use them.
 */
export function ChildAppProvider({ children }: { children: ReactNode }) {
  const [appSettings, setAppSettings] = useState<AppSettings>(DEFAULT_APP_SETTINGS);
  const loaded = useRef(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [settingsLoaded, setSettingsLoaded] = useState(false);

  useEffect(() => {
    invoke<AppSettings>("get_app_settings")
      .then((cfg) => {
        setAppSettings(cfg);
        setLoggerLevel(cfg.diagnostics.level);
        loaded.current = true;
        setSettingsLoaded(true);
        if (cfg.ui?.language && cfg.ui.language !== i18n.language) {
          i18n.changeLanguage(cfg.ui.language);
        }
      })
      .catch(() => {
        loaded.current = true;
        setSettingsLoaded(true);
      });
  }, []);

  useEffect(() => {
    document.documentElement.style.fontSize = `${appSettings.appearance.ui_font_size}px`;
  }, [appSettings.appearance.ui_font_size]);

  useEffect(() => {
    if (appSettings.ui?.language && appSettings.ui.language !== i18n.language) {
      i18n.changeLanguage(appSettings.ui.language);
    }
  }, [appSettings.ui?.language]);

  const updateAppSettings = useCallback(
    (updates: Partial<AppSettings> | ((prev: AppSettings) => Partial<AppSettings>)) => {
      setAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev) : updates;
        const next = { ...prev, ...nextUpdates };
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
    setLoggerLevel(next.diagnostics.level);
    setAppSettings(next);
  }, []);

  const updateUi = useCallback(
    (updates: Partial<UiConfig> | ((prev: UiConfig) => Partial<UiConfig>)) => {
      updateAppSettings((prev) => {
        const nextUpdates = typeof updates === "function" ? updates(prev.ui) : updates;
        return { ui: { ...prev.ui, ...nextUpdates } };
      });
    },
    [updateAppSettings],
  );

  const noop = useCallback(() => {}, []);
  const noopString = useCallback(() => "", []);
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
      addPendingTab: noopString,
      updateTabSession: noop,
      markTabConnectionFailed: noop,
      updatePaneSession: noop,
      markPaneConnectionFailed: noop,
      markPaneConnecting: noop,
      setActivePane: noop,
      updateSplitRatio: noop,
      splitPane: noopSplitPane,
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
      isLocked: false,
      setIsLocked: noop,
      settingsLoaded,
    }),
    [
      noop,
      noopString,
      noopAsync,
      noopSplitPane,
      emptyConnections,
      emptyGroups,
      appSettings,
      updateAppSettings,
      replaceAppSettings,
      updateUi,
      settingsLoaded,
    ],
  );

  return <AppContext.Provider value={contextValue}>{children}</AppContext.Provider>;
}
